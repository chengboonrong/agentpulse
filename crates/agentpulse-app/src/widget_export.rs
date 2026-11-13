//! Publishes a small summary for the WidgetKit extension, `widget.json` in
//! [`WIDGET_DIR`], plus a rate-limited request to reload the widgets.
//!
//! Not an app group container: on macOS 27 this unsandboxed, profile-less app
//! is denied writing there (TCC `SystemPolicyAppDataDetailed`, no prompt), so
//! the sandboxed widget instead reads [`WIDGET_DIR`] through a read-only
//! temporary-exception entitlement (see `scripts/bundle.sh`).
//!
//! Active only when the binary runs from inside `AgentPulse.app` built by
//! `scripts/bundle.sh`. The file holds display strings and counts only: no
//! paths, session ids, prompts or anything else not listed in [`Payload`].

use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use agentpulse_animation::{CPU_SPRINT_ON, CatState, SPRINT_ON, frames, palette};
use agentpulse_core::{Provider, Reading, SessionStatus, TokenUsage};
use serde::Serialize;

use crate::collector::Snapshot;
use crate::format::{self, DASH};

pub const VERSION: u32 = 1;
pub const FILE_NAME: &str = "widget.json";
const MAX_SESSIONS: usize = 4;
const WRITE_EVERY: Duration = Duration::from_secs(10);
/// Rewrite unchanged data this often (writing is free), so the widget's
/// 75-minute age limit never mistakes a quiet but running app for stale data.
const HEARTBEAT: Duration = Duration::from_secs(5 * 60);
/// WidgetKit allows a background app roughly 40 to 70 reloads a day; at most
/// one request per 15 minutes keeps an 8-hour day near 32.
const RELOAD_EVERY: Duration = Duration::from_secs(15 * 60);
/// A change in today's cost alone is the least urgent reason to reload.
const COST_RELOAD_AFTER: Duration = Duration::from_secs(30 * 60);
/// Relative to `$HOME`; fixed (not `AGENTPULSE_DATA_DIR`) because the widget's
/// sandbox exception names this exact path.
pub const WIDGET_DIR: &str = "Library/Application Support/AgentPulse/Widget";
/// Relative to `Contents`.
const APPEX: &str = "PlugIns/AgentPulseWidgets.appex";
/// Relative to `Contents`.
const BRIDGE: &str = "Frameworks/libAgentPulseWidgetBridge.dylib";
const BRIDGE_SYMBOL: &str = "agentpulse_reload_widgets";

/// Version 1 of the contract the Swift widget decodes (`macos/Shared/Payload.swift`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Payload {
    pub version: u32,
    pub generated_at_ms: i64,
    pub stopped: bool,
    pub agents: Agents,
    pub today: Today,
    pub output_rate: String,
    pub cat: Cat,
    pub sessions: Vec<WidgetSession>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Agents {
    pub active: usize,
    pub waiting: usize,
    pub idle: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Today {
    pub tokens: String,
    pub cost: String,
    pub cost_note: String,
    pub unpriced: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cat {
    pub state: &'static str,
    pub width: usize,
    pub height: usize,
    pub rows: Vec<&'static str>,
    /// Sprite char to RGBA, for the chars the rows use (clear pixels excluded).
    pub palette: BTreeMap<String, [u8; 4]>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WidgetSession {
    /// Last path component only.
    pub project: String,
    pub status: &'static str,
    pub model: String,
    pub started_at_ms: Option<i64>,
    pub tokens: String,
    pub cost: String,
}

/// The single cat the widget shows for the whole snapshot: the menu bar cat's
/// inputs, judged on this one snapshot (a widget is a still, so no smoothing).
pub fn overall_cat(snap: &Snapshot) -> CatState {
    match crate::menubar::cat_input(Some(snap), false) {
        (Some(SessionStatus::Active), rate, cpu) => {
            if rate > SPRINT_ON || cpu.is_some_and(|c| c > CPU_SPRINT_ON) {
                CatState::Sprinting
            } else {
                CatState::Running
            }
        }
        (Some(SessionStatus::Waiting), ..) => CatState::LookingAround,
        (Some(SessionStatus::Idle), ..) => CatState::Sitting,
        _ => CatState::Sleeping,
    }
}

fn cat_name(s: CatState) -> &'static str {
    match s {
        CatState::Sleeping => "sleeping",
        CatState::Sitting => "sitting",
        CatState::LookingAround => "looking_around",
        CatState::Running => "running",
        CatState::Sprinting => "sprinting",
        CatState::Happy => "happy",
        CatState::Surprised => "surprised",
    }
}

fn cat(state: CatState) -> Cat {
    let sprite = &frames(state)[0];
    let pal = palette(Provider::ClaudeCode);
    let mut colors = BTreeMap::new();
    for (y, row) in sprite.rows().iter().enumerate() {
        for (x, c) in row.chars().enumerate() {
            if let Some(rgba) = pal.color(sprite.ink(x, y)) {
                colors.insert(c.to_string(), rgba);
            }
        }
    }
    Cat {
        state: cat_name(state),
        width: sprite.width(),
        height: sprite.height(),
        rows: sprite.rows().to_vec(),
        palette: colors,
    }
}

/// The widget gallery's generic preview: no agents, sleeping cat, no numbers.
/// `scripts/bundle.sh` ships it in the extension so the Swift side never
/// carries its own copy of the sprite.
pub fn placeholder() -> Payload {
    let empty = Snapshot {
        taken_at_ms: 0,
        sessions: vec![],
        today_tokens: TokenUsage::default(),
        today_cost_usd: Reading::Unavailable,
        today_unpriced_events: 0,
        output_rate: 0.0,
        rate_history: vec![],
        agents_cpu_percent: None,
        agents_rss_bytes: None,
        system_gpu_percent: Reading::Unavailable,
        indexing: false,
        errors: vec![],
        usage: None,
        codex_limits: None,
        detail: None,
    };
    let mut p = payload(&empty, false);
    p.today.tokens = DASH.into();
    p.today.cost_note = String::new();
    p.output_rate = DASH.into();
    p
}

/// `agentpulse --widget-placeholder`, used by `scripts/bundle.sh`.
pub fn print_placeholder() -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(&placeholder())?);
    Ok(())
}

pub fn payload(snap: &Snapshot, stopped: bool) -> Payload {
    let count = |st| snap.sessions.iter().filter(|s| s.status == st).count();
    Payload {
        version: VERSION,
        generated_at_ms: snap.taken_at_ms,
        stopped,
        agents: Agents {
            active: count(SessionStatus::Active),
            waiting: count(SessionStatus::Waiting),
            idle: count(SessionStatus::Idle),
        },
        today: Today {
            tokens: format::tokens(snap.today_tokens.total()),
            cost: format::approx_cost(snap.today_cost_usd).unwrap_or_else(|| DASH.into()),
            cost_note: match snap.today_cost_usd {
                Reading::Unavailable => "no price for today's models",
                _ => "estimate, API list prices",
            }
            .into(),
            unpriced: snap.today_unpriced_events,
        },
        output_rate: format!("{:.1} tok/s", snap.output_rate),
        cat: cat(overall_cat(snap)),
        sessions: snap
            .sessions
            .iter()
            .filter(|s| s.status.is_live())
            .take(MAX_SESSIONS)
            .map(|s| WidgetSession {
                project: format::project_name(s.project_path.as_deref()).into(),
                status: s.status.label(),
                model: s.model.clone().unwrap_or_else(|| DASH.into()),
                started_at_ms: s.started_at_ms,
                tokens: format::tokens(s.tokens.total()),
                cost: format::approx_cost(s.cost_usd).unwrap_or_else(|| DASH.into()),
            })
            .collect(),
    }
}

/// Write when the content changed (ignoring `generated_at_ms`) or the
/// heartbeat is due, and never more than once per [`WRITE_EVERY`].
pub fn should_write(last: Option<&Payload>, new: &Payload, since: Option<Duration>) -> bool {
    let Some(since) = since else { return true };
    let changed = last.is_none_or(|l| {
        Payload {
            generated_at_ms: new.generated_at_ms,
            ..l.clone()
        } != *new
    });
    since >= WRITE_EVERY && (changed || since >= HEARTBEAT)
}

/// What a glance at the widget would notice.
fn glance(p: &Payload) -> impl PartialEq + '_ {
    let mut sessions: Vec<(&str, &str)> = p
        .sessions
        .iter()
        .map(|s| (s.project.as_str(), s.status))
        .collect();
    sessions.sort_unstable();
    (&p.agents, sessions, p.cat.state, p.stopped)
}

/// WidgetKit budgets reloads, so ask at most once per [`RELOAD_EVERY`] and
/// only for a visible change; today's cost alone waits [`COST_RELOAD_AFTER`].
pub fn should_reload(last: Option<&Payload>, new: &Payload, since: Option<Duration>) -> bool {
    let (Some(last), Some(since)) = (last, since) else {
        return true;
    };
    since >= RELOAD_EVERY
        && (glance(last) != glance(new)
            || (last.today.cost != new.today.cost && since >= COST_RELOAD_AFTER))
}

/// Replaces `dir/widget.json` with mode 0600 through a temp file and rename.
/// `writer` names the temp file, so two writers never share one.
pub fn write_atomic(dir: &Path, p: &Payload, writer: &str) -> std::io::Result<()> {
    let tmp = dir.join(format!(".{FILE_NAME}.{}.{writer}.tmp", std::process::id()));
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    serde_json::to_writer(&mut f, p)?;
    f.write_all(b"\n")?;
    drop(f);
    fs::rename(&tmp, dir.join(FILE_NAME))
}

/// State shared by the collector thread, its panic path and the exit hook.
/// Every write happens under this lock.
struct Shared {
    dir: PathBuf,
    reload: Option<extern "C" fn()>,
    /// The payload on disk.
    last: Option<Payload>,
    /// Set once the stopped payload is written; later publishes are no-ops.
    quitting: bool,
}

/// The exporter's state for the exit hook; set once, by the first `detect`.
static AT_EXIT: OnceLock<Arc<Mutex<Shared>>> = OnceLock::new();

fn lock(m: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    // A panic elsewhere must not stop us from marking the data stopped.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Rewrites the data on disk as stopped (the placeholder if nothing was
/// written yet), asks for a reload, and blocks every later publish.
fn mark_stopped(shared: &Mutex<Shared>) {
    let mut sh = lock(shared);
    if sh.quitting {
        return;
    }
    sh.quitting = true;
    let p = Payload {
        stopped: true,
        ..sh.last.clone().unwrap_or_else(placeholder)
    };
    match write_atomic(&sh.dir, &p, "stop") {
        Ok(()) => {
            sh.last = Some(p);
            if let Some(reload) = sh.reload {
                reload();
            }
        }
        Err(e) => tracing::warn!(error = %e, "cannot mark widget data stopped"),
    }
}

/// Runs at process exit (quitting the app ends in `exit`).
extern "C" fn at_exit() {
    if let Some(shared) = AT_EXIT.get() {
        mark_stopped(shared);
    }
}

pub struct WidgetExporter {
    shared: Arc<Mutex<Shared>>,
    written: Option<(Instant, Payload)>,
    reloaded: Option<(Instant, Payload)>,
}

impl WidgetExporter {
    /// Some only when running from inside AgentPulse.app with widgets set up;
    /// None for `cargo run`.
    pub fn detect() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        let contents = exe.parent()?.parent()?;
        let app = contents.parent()?;
        if contents.file_name()? != "Contents" || app.extension()? != "app" {
            return None;
        }
        if !contents.join(APPEX).is_dir() {
            return None;
        }
        let dir = PathBuf::from(std::env::var_os("HOME")?).join(WIDGET_DIR);
        let made = fs::create_dir_all(&dir)
            .and_then(|()| fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)));
        if let Err(e) = made {
            tracing::warn!(error = %e, dir = %dir.display(), "widgets disabled");
            return None;
        }
        let reload = load_bridge(&contents.join(BRIDGE));
        if reload.is_none() {
            tracing::warn!(
                "widget bridge not loaded; widget.json is written but reloads are not requested"
            );
        }
        let exporter = Self::new(dir, reload);
        if AT_EXIT.set(exporter.shared.clone()).is_ok() {
            // SAFETY: registers a plain `extern "C" fn()` that only reads AT_EXIT.
            unsafe { libc::atexit(at_exit) };
        }
        tracing::info!(dir = %lock(&exporter.shared).dir.display(), "exporting widget data");
        Some(exporter)
    }

    fn new(dir: PathBuf, reload: Option<extern "C" fn()>) -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                dir,
                reload,
                last: None,
                quitting: false,
            })),
            written: None,
            reloaded: None,
        }
    }

    /// Marks the data stopped; used when the collector thread panics.
    pub fn stop(&self) {
        mark_stopped(&self.shared);
    }

    pub fn publish(&mut self, snap: &Snapshot) {
        let mut sh = lock(&self.shared);
        if sh.quitting {
            return;
        }
        let new = payload(snap, false);
        let since = |t: &Option<(Instant, Payload)>| t.as_ref().map(|(at, _)| at.elapsed());
        let last = self.written.as_ref().map(|(_, p)| p);
        if should_write(last, &new, since(&self.written)) {
            match write_atomic(&sh.dir, &new, "collector") {
                Ok(()) => sh.last = Some(new.clone()),
                Err(e) => tracing::warn!(error = %e, "cannot write widget data"),
            }
            // Also on failure, so a failing write is retried only every WRITE_EVERY.
            self.written = Some((Instant::now(), new));
        }
        let Some((_, on_disk)) = &self.written else {
            return;
        };
        let last = self.reloaded.as_ref().map(|(_, p)| p);
        if should_reload(last, on_disk, since(&self.reloaded)) {
            if let Some(reload) = sh.reload {
                reload();
            }
            self.reloaded = Some((Instant::now(), on_disk.clone()));
        }
    }
}

fn load_bridge(path: &Path) -> Option<extern "C" fn()> {
    let path = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let symbol = CString::new(BRIDGE_SYMBOL).ok()?;
    // SAFETY: both strings are NUL-terminated. The library is never closed,
    // so the symbol stays valid for the life of the process, and the bridge
    // defines it as `@_cdecl` with no arguments and no return value.
    unsafe {
        let handle = libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if handle.is_null() {
            let err = libc::dlerror();
            if !err.is_null() {
                let err = std::ffi::CStr::from_ptr(err).to_string_lossy();
                tracing::warn!(error = %err, "cannot load widget bridge");
            }
            return None;
        }
        let f = libc::dlsym(handle, symbol.as_ptr());
        (!f.is_null()).then(|| std::mem::transmute::<*mut libc::c_void, extern "C" fn()>(f))
    }
}

#[cfg(test)]
mod tests {
    use agentpulse_core::{AgentSession, TokenUsage};

    use super::*;

    fn session(id: &str, status: SessionStatus, path: &str) -> AgentSession {
        AgentSession {
            provider: Provider::ClaudeCode,
            session_id: id.into(),
            project_path: Some(path.into()),
            model: Some("claude-test".into()),
            status,
            started_at_ms: Some(1_000),
            last_activity_at_ms: Some(2_000),
            ended_at_ms: None,
            active_ms: 0,
            tokens: TokenUsage {
                output: 1_500,
                ..Default::default()
            },
            cost_usd: Reading::Estimated(1.234),
            unpriced_events: 0,
            output_rate: 0.0,
            process: None,
            resources: Reading::Unavailable,
            recorded_cost: Reading::Unavailable,
        }
    }

    fn snap(statuses: &[SessionStatus], rate: f32) -> Snapshot {
        Snapshot {
            taken_at_ms: 5_000,
            sessions: statuses
                .iter()
                .enumerate()
                .map(|(i, &s)| session(&format!("id-{i}"), s, &format!("/Users/x/p{i}/")))
                .collect(),
            today_tokens: TokenUsage {
                input: 104_000_000,
                ..Default::default()
            },
            today_cost_usd: Reading::Estimated(55.554),
            today_unpriced_events: 0,
            output_rate: rate,
            rate_history: vec![],
            agents_cpu_percent: None,
            agents_rss_bytes: None,
            system_gpu_percent: Reading::Unavailable,
            indexing: false,
            errors: vec![],
            usage: None,
            codex_limits: None,
            detail: None,
        }
    }

    use SessionStatus::*;

    #[test]
    fn widget_cat_sprints_on_tokens_or_active_cpu() {
        use agentpulse_core::ResourceUsage;
        let busy = |s: &mut Snapshot, i: usize, c: f32| {
            s.sessions[i].resources = Reading::Measured(ResourceUsage {
                cpu_percent: Some(c),
                rss_bytes: 1,
                process_count: 1,
            });
        };
        assert_eq!(overall_cat(&snap(&[Active], 0.0)), CatState::Running);
        assert_eq!(overall_cat(&snap(&[Active], 40.0)), CatState::Sprinting);
        let mut s = snap(&[Active, Idle], 0.0);
        busy(&mut s, 0, 150.0);
        assert_eq!(overall_cat(&s), CatState::Sprinting);
        let mut s = snap(&[Idle, Waiting], 0.0);
        busy(&mut s, 0, 500.0); // not Active: CPU does not count
        assert_eq!(overall_cat(&s), CatState::LookingAround);
        assert_eq!(overall_cat(&snap(&[], 0.0)), CatState::Sleeping);
    }

    #[test]
    fn sessions_are_live_capped_and_anonymous() {
        let s = snap(&[Active, Waiting, Idle, Idle, Idle, Completed], 0.0);
        let p = payload(&s, false);
        assert_eq!(p.sessions.len(), MAX_SESSIONS);
        assert_eq!(p.sessions[0].project, "p0");
        assert!(p.sessions.iter().all(|x| !x.project.contains('/')));
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("id-0"), "no session ids");
        assert!(!json.contains("/Users"), "no paths");
        assert_eq!(
            (p.agents.active, p.agents.waiting, p.agents.idle),
            (1, 1, 3)
        );
        assert_eq!(p.today.tokens, "104M");
        assert_eq!(p.today.cost, "≈ $55.55");
        assert_eq!(p.sessions[0].cost, "≈ $1.23");
        assert_eq!(p.sessions[0].tokens, "1.5K");

        let mut u = snap(&[Active], 0.0);
        u.today_cost_usd = Reading::Unavailable;
        u.sessions[0].project_path = None;
        let p = payload(&u, false);
        assert_eq!(p.today.cost, DASH);
        assert_eq!(p.sessions[0].project, DASH);
    }

    #[test]
    fn serializes_documented_fields() {
        let v = serde_json::to_value(payload(&snap(&[Active], 28.84), false)).unwrap();
        let keys = |v: &serde_json::Value| {
            let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        assert_eq!(
            keys(&v),
            [
                "agents",
                "cat",
                "generated_at_ms",
                "output_rate",
                "sessions",
                "stopped",
                "today",
                "version"
            ]
        );
        assert_eq!(keys(&v["agents"]), ["active", "idle", "waiting"]);
        assert_eq!(
            keys(&v["today"]),
            ["cost", "cost_note", "tokens", "unpriced"]
        );
        assert_eq!(
            keys(&v["cat"]),
            ["height", "palette", "rows", "state", "width"]
        );
        assert_eq!(
            keys(&v["sessions"][0]),
            [
                "cost",
                "model",
                "project",
                "started_at_ms",
                "status",
                "tokens"
            ]
        );
        assert_eq!(v["version"], 1);
        assert_eq!(v["output_rate"], "28.8 tok/s");
        assert_eq!(v["sessions"][0]["status"], "Active");
        assert_eq!(
            v["cat"]["palette"]["o"],
            serde_json::json!([150, 78, 30, 255])
        );
        assert_eq!(v["cat"]["width"], 32);
        assert_eq!(v["cat"]["rows"].as_array().unwrap().len(), 16);
        assert!(v["cat"]["palette"].get(".").is_none());
    }

    #[test]
    fn cat_follows_agent_mix() {
        let state = |st: &[SessionStatus], rate| payload(&snap(st, rate), false).cat.state;
        assert_eq!(state(&[], 0.0), "sleeping");
        assert_eq!(state(&[Completed, Failed], 99.0), "sleeping");
        assert_eq!(state(&[Idle], 0.0), "sitting");
        assert_eq!(state(&[Idle, Waiting], 0.0), "looking_around");
        assert_eq!(state(&[Waiting, Active], 0.0), "running");
        assert_eq!(state(&[Active], SPRINT_ON), "running");
        assert_eq!(state(&[Active], SPRINT_ON + 1.0), "sprinting");
    }

    fn secs(s: u64) -> Option<Duration> {
        Some(Duration::from_secs(s))
    }

    #[test]
    fn write_decision() {
        let a = payload(&snap(&[Active], 0.0), false);
        let mut later = a.clone();
        later.generated_at_ms += 60_000;
        let mut changed = a.clone();
        changed.output_rate = "1.0 tok/s".into();

        assert!(should_write(None, &a, None), "first write");
        assert!(
            !should_write(Some(&a), &later, secs(60)),
            "only the time changed"
        );
        assert!(should_write(Some(&a), &later, secs(300)), "heartbeat");
        assert!(
            !should_write(Some(&a), &changed, secs(9)),
            "at most every 10 s"
        );
        assert!(should_write(Some(&a), &changed, secs(10)));
    }

    #[test]
    fn reload_decision() {
        let a = payload(&snap(&[Active], 0.0), false);
        let other = |st: &[SessionStatus], rate| payload(&snap(st, rate), false);
        let every = RELOAD_EVERY.as_secs();
        assert_eq!(every, 15 * 60, "at most 4 requests an hour");

        assert!(should_reload(None, &a, None), "first publish");
        let waiting = other(&[Waiting], 0.0);
        assert!(
            !should_reload(Some(&a), &waiting, secs(60)),
            "not every minute"
        );
        assert!(!should_reload(Some(&a), &waiting, secs(every - 1)));
        assert!(should_reload(Some(&a), &waiting, secs(every)), "status");
        let count = other(&[Active, Idle], 0.0);
        assert!(should_reload(Some(&a), &count, secs(every)), "count");
        let cat = other(&[Active], 99.0);
        assert!(should_reload(Some(&a), &cat, secs(every)), "cat");
        let mut stopped = a.clone();
        stopped.stopped = true;
        assert!(should_reload(Some(&a), &stopped, secs(every)), "stopped");
        let mut renamed = a.clone();
        renamed.sessions[0].project = "other".into();
        assert!(should_reload(Some(&a), &renamed, secs(every)), "sessions");

        let mut quiet = a.clone();
        quiet.output_rate = "9.0 tok/s".into();
        quiet.today.tokens = "200M".into();
        quiet.sessions[0].tokens = "2.0K".into();
        assert!(
            !should_reload(Some(&a), &quiet, secs(86_400)),
            "not glanceable"
        );

        let mut cost = a.clone();
        cost.today.cost = "≈ $60.00".into();
        assert!(
            !should_reload(Some(&a), &cost, secs(every)),
            "cost alone waits"
        );
        assert!(!should_reload(Some(&a), &cost, secs(30 * 60 - 1)));
        assert!(
            should_reload(Some(&a), &cost, secs(30 * 60)),
            "cost after 30 min"
        );
    }

    #[test]
    fn placeholder_is_generic() {
        let p = placeholder();
        assert_eq!(p.cat.state, "sleeping");
        assert_eq!(
            (p.agents.active, p.agents.waiting, p.agents.idle),
            (0, 0, 0)
        );
        assert_eq!(
            (p.today.tokens.as_str(), p.today.cost.as_str()),
            (DASH, DASH)
        );
        assert!(p.sessions.is_empty());
        assert_eq!(p.cat.rows, frames(CatState::Sleeping)[0].rows());
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentpulse-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read(dir: &Path) -> serde_json::Value {
        serde_json::from_slice(&fs::read(dir.join(FILE_NAME)).unwrap()).unwrap()
    }

    #[test]
    fn writes_atomically_with_private_mode() {
        let dir = temp_dir("atomic");
        let p = payload(&snap(&[Idle], 0.0), false);
        write_atomic(&dir, &p, "collector").unwrap();
        let mut q = p.clone();
        q.stopped = true;
        write_atomic(&dir, &q, "stop").unwrap();

        let mode = fs::metadata(dir.join(FILE_NAME))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(read(&dir)["stopped"], true);
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE_NAME], "no temp file left behind");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stop_marks_last_payload_and_blocks_later_publishes() {
        let dir = temp_dir("stop");
        let mut w = WidgetExporter::new(dir.clone(), None);
        w.publish(&snap(&[Active], 0.0));
        assert_eq!(read(&dir)["stopped"], false);

        w.stop();
        let v = read(&dir);
        assert_eq!(v["stopped"], true);
        assert_eq!(v["agents"]["active"], 1, "the last data, marked stopped");

        // Nothing may overwrite the stopped mark, even with the write throttle reset.
        w.written = None;
        w.publish(&snap(&[Idle, Idle], 0.0));
        assert_eq!(read(&dir), v, "publish after stop is a no-op");
        w.stop();
        assert_eq!(read(&dir), v, "stopping twice writes once");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stop_before_any_publish_writes_stopped_placeholder() {
        let dir = temp_dir("stop-first");
        WidgetExporter::new(dir.clone(), None).stop();
        let v = read(&dir);
        assert_eq!(v["stopped"], true);
        assert_eq!(v["agents"]["active"], 0);
        fs::remove_dir_all(&dir).unwrap();
    }
}
