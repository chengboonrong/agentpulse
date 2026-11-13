mod collector;
mod format;
mod hub;
mod menubar;
mod settings;
mod ui;
mod widget_export;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use agentpulse_core::AgentAdapter;
use agentpulse_cost::PriceBook;
use agentpulse_storage::Store;
use agentpulse_telemetry::{ClaudeCodeAdapter, CodexAdapter};
use anyhow::{Context, bail};
use collector::Collector;
use tracing_subscriber::filter::LevelFilter;

const USAGE: &str = "usage: agentpulse [--dump | --bench]";

fn main() -> anyhow::Result<()> {
    let level = std::env::var("AGENTPULSE_LOG")
        .ok()
        .and_then(|v| v.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::INFO);
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_max_level(level)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => gui(ui::Options::default()),
        ["--dump"] => dump(),
        ["--bench"] => bench(),
        ["--widget-placeholder"] => widget_export::print_placeholder(),
        #[cfg(feature = "screenshot")]
        ["--menubar-preview", dir] => menubar::preview(std::path::Path::new(dir)),
        #[cfg(feature = "screenshot")]
        ["--screenshot", dir, ref rest @ ..] => gui(ui::Options::screenshot(dir, rest)?),
        _ => bail!(USAGE),
    }
}

fn data_dir() -> anyhow::Result<PathBuf> {
    if let Some(d) = std::env::var_os("AGENTPULSE_DATA_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join("Library/Application Support/AgentPulse"))
}

fn open_collector() -> anyhow::Result<Collector> {
    let dir = data_dir()?;
    let store = Store::open(&dir.join("agentpulse.db"))?;
    let mut errors = Vec::new();
    let prices = PriceBook::load(&dir.join("pricing.toml")).unwrap_or_else(|e| {
        tracing::warn!(error = %format!("{e:#}"), "using built-in prices");
        errors.push(format!("Price table ignored, using built-in prices: {e:#}"));
        PriceBook::builtin()
    });
    let root = ClaudeCodeAdapter::default_root().context("cannot locate ~/.claude")?;
    let mut adapters: Vec<Box<dyn AgentAdapter>> = vec![Box::new(ClaudeCodeAdapter::new(root))];
    // No home directory means no ~/.codex either: nothing to watch, not an error.
    adapters.extend(
        CodexAdapter::default_root()
            .map(|r| Box::new(CodexAdapter::new(r)) as Box<dyn AgentAdapter>),
    );
    Collector::new(adapters, store, prices, errors)
}

fn gui(options: ui::Options) -> anyhow::Result<()> {
    let (tx, rx) = async_channel::bounded(1);
    let (detail_tx, detail_rx) = std::sync::mpsc::channel();
    let cats = settings::CatSettings::load(&data_dir()?);
    let widgets = widget_export::WidgetExporter::detect(cats.clone());
    collector::spawn(open_collector()?, tx, detail_rx, widgets)?;
    ui::run(rx, detail_tx, options, cats);
    Ok(())
}

fn dump() -> anyhow::Result<()> {
    let mut c = open_collector()?;
    let start = Instant::now();
    while c.tick().indexing {}
    let took = start.elapsed();
    let i = c.ingested;
    eprintln!(
        "backfill: {:.2} s, {} files, {} bytes read, {} events ingested",
        took.as_secs_f64(),
        i.files,
        i.bytes,
        i.events
    );
    // CPU needs a second process sample, and samples are at least PROCESS_INTERVAL apart.
    thread::sleep(collector::PROCESS_INTERVAL + Duration::from_millis(200));
    println!("{}", serde_json::to_string_pretty(&c.tick())?);
    Ok(())
}

/// (user + system CPU time, peak resident bytes) of this process.
fn own_usage() -> (Duration, u64) {
    // SAFETY: rusage is plain data and getrusage only writes into it.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let tv = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    // macOS reports ru_maxrss in bytes.
    (tv(ru.ru_utime) + tv(ru.ru_stime), ru.ru_maxrss as u64)
}

fn bench() -> anyhow::Result<()> {
    let mut c = open_collector()?;
    while c.tick().indexing {}
    const TICKS: u32 = 20;
    let (cpu_before, _) = own_usage();
    let start = Instant::now();
    let mut ticks = Vec::new();
    for _ in 0..TICKS {
        let t = Instant::now();
        c.tick();
        ticks.push(t.elapsed());
        thread::sleep(Duration::from_secs(1));
    }
    let wall = start.elapsed();
    let (cpu_after, peak_rss) = own_usage();
    let cpu = cpu_after - cpu_before;
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let avg = ticks.iter().copied().sum::<Duration>() / TICKS;
    let max = ticks.iter().copied().max().unwrap_or_default();
    println!(
        "ticks:          {TICKS}, 1 s apart, over {:.3} s",
        wall.as_secs_f64()
    );
    println!(
        "tick duration:  avg {:.3} ms, max {:.3} ms",
        ms(avg),
        ms(max)
    );
    println!(
        "own CPU time:   {:.3} ms (user+system), {:.3}% of one core",
        ms(cpu),
        cpu.as_secs_f64() / wall.as_secs_f64() * 100.0
    );
    println!(
        "peak RSS:       {} bytes ({})",
        peak_rss,
        format::bytes(peak_rss)
    );
    Ok(())
}
