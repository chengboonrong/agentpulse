//! Aggregate-only scan of the real Claude Code data directory.
//! Usage: scan [days]   (default 1). Prints counts only, never record text.

use std::collections::HashMap;
use std::time::Instant;

use agentpulse_core::{AgentAdapter, TokenUsage};
use agentpulse_telemetry::ClaudeCodeAdapter;

fn main() -> anyhow::Result<()> {
    let days: f64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1.0);
    let root = ClaudeCodeAdapter::default_root().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let mut adapter = ClaudeCodeAdapter::new(root);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    let since = now_ms - (days * 86_400_000.0) as i64;

    let start = Instant::now();
    let mut offsets: HashMap<String, u64> = HashMap::new();
    let (mut bytes, mut sessions, mut events, mut polls) = (0u64, 0usize, 0usize, 0);
    let mut tokens = TokenUsage::default();
    loop {
        let b = adapter.poll(&offsets, since)?;
        polls += 1;
        for f in &b.files {
            bytes += f.offset - offsets.get(&f.path).copied().unwrap_or(0);
            offsets.insert(f.path.clone(), f.offset);
        }
        sessions += b.sessions.len();
        events += b.events.len();
        for e in &b.events {
            tokens += e.usage;
        }
        if !b.more {
            break;
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!("window: last {days} day(s); polls: {polls}");
    println!(
        "files read: {}; sessions: {sessions}; events: {events}",
        offsets.len()
    );
    println!(
        "tokens: input {} output {} cache_write_5m {} cache_write_1h {} cache_read {} (total {})",
        tokens.input,
        tokens.output,
        tokens.cache_write_5m,
        tokens.cache_write_1h,
        tokens.cache_read,
        tokens.total()
    );
    let mut times: Vec<u128> = Vec::with_capacity(200);
    let scans_before = adapter.full_scans();
    for _ in 0..200 {
        let t = Instant::now();
        let b = adapter.poll(&offsets, since)?;
        times.push(t.elapsed().as_micros());
        for f in &b.files {
            offsets.insert(f.path.clone(), f.offset);
        }
    }
    let full = adapter.full_scans() - scans_before;
    let avg = times.iter().sum::<u128>() / times.len() as u128;
    println!(
        "200 idle polls: avg {avg} us, max {} us, full scans {full}",
        times.iter().max().unwrap()
    );
    let mut cold = ClaudeCodeAdapter::new(ClaudeCodeAdapter::default_root().unwrap());
    let t = Instant::now();
    cold.poll(&offsets, since)?;
    println!("one forced full scan: {} us", t.elapsed().as_micros());
    let live = adapter.live_sessions()?;
    let mut states: Vec<String> = live.iter().map(|l| format!("{:?}", l.state)).collect();
    states.sort();
    println!("live sessions: {} {:?}", live.len(), states);
    let mb = bytes as f64 / 1_048_576.0;
    println!(
        "bytes read: {bytes} ({mb:.1} MiB); elapsed: {secs:.3}s; {:.1} MiB/s",
        mb / secs.max(1e-9)
    );
    Ok(())
}
