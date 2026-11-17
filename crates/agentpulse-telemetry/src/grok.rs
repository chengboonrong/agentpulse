//! Grok CLI adapter: reads each session's `usage.json` for tokens and cost,
//! tails its `events.jsonl` for activity and status, and finds live sessions
//! from the `events.jsonl` files that running `grok` processes hold open.
//!
//! Privacy: a session folder also holds the conversation, prompts, tool
//! definitions and a summary. Discovery yields only `events.jsonl` and
//! `usage.json`, so no other file is ever opened; the working folder comes
//! from the session folder's parent name. Event lines are parsed into a
//! struct naming only `ts` and `type`, so serde skips every other field (tool
//! names, MCP targets, error messages). Of `usage.json` only turn numbers and
//! times, model ids, token counts and cost are read. No line text is stored
//! or logged.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agentpulse_core::{
    AgentAdapter, LiveSession, LiveState, PollBatch, Provider, SessionDelta, TokenUsage, UsageEvent,
};
use agentpulse_process::OpenFiles;
use serde::Deserialize;

use crate::tail::{Groups, Tailer, TranscriptFile, read_lines, ts_ms};

const EVENTS: &str = "events.jsonl";
const USAGE: &str = "usage.json";

/// xAI reports cost in ticks: 1e10 ticks are one US dollar.
const TICKS_PER_USD: f64 = 1e10;

/// Open event logs are looked up at most this often: a session that starts or
/// ends is seen as live or not up to this late.
const OPEN_FILES_INTERVAL: Duration = Duration::from_secs(5);

/// Grok logs a permission request for every tool call, auto-approved ones
/// included (resolved within ms). Only a request still open this long means
/// the session is waiting on the user.
const WAITING_AFTER_MS: i64 = 1_000;

/// A session in a process holding several sessions' event logs is live only
/// if it had activity this recently.
const SHARED_LIVE_WINDOW: Duration = Duration::from_secs(60 * 60);

type OpenFilesFn = Box<dyn FnMut() -> Vec<OpenFiles> + Send>;

pub struct GrokAdapter {
    root: PathBuf,
    tail: Tailer,
    /// Per `events.jsonl` path: what its lines so far established.
    files: HashMap<String, FileState>,
    open_files: OpenFilesFn,
    /// Event logs open per `grok` process, as of the last poll.
    open: Vec<OpenFiles>,
}

/// What the earlier lines of one `events.jsonl` established. Kept across
/// polls, because tailing resumes mid-file.
#[derive(Default)]
struct FileState {
    /// The latest `turn_started` has no later `turn_ended`.
    busy: bool,
    /// When the latest `permission_requested` was made, while unresolved.
    waiting_since: Option<i64>,
    started_ms: Option<i64>,
    /// `model_id` of the latest `turn_started`.
    model: Option<String>,
    /// Newest timestamp of any line read.
    last_ms: Option<i64>,
    /// Byte offset the state above was read up to; `u64::MAX` while a read is
    /// unfinished.
    offset: u64,
}

impl GrokAdapter {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            tail: Tailer::new(),
            files: HashMap::new(),
            open_files: Box::new(open_event_logs()),
            open: Vec::new(),
        }
    }

    /// Replaces the scan of open event logs per `grok` process (for tests).
    pub fn with_open_files(mut self, f: impl FnMut() -> Vec<OpenFiles> + Send + 'static) -> Self {
        self.open_files = Box::new(f);
        self
    }

    pub fn with_rescan_interval(mut self, d: Duration) -> Self {
        self.tail.rescan_interval = d;
        self
    }

    pub fn with_read_budget(mut self, bytes: u64) -> Self {
        self.tail.read_budget = bytes;
        self
    }

    /// `$GROK_HOME` if set and non-empty, else `~/.grok`.
    pub fn default_root() -> Option<PathBuf> {
        root_from(std::env::var_os("GROK_HOME"), std::env::var_os("HOME"))
    }
}

fn root_from(grok_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let set = |v: Option<OsString>| v.filter(|v| !v.is_empty());
    set(grok_home)
        .map(PathBuf::from)
        .or_else(|| set(home).map(|h| PathBuf::from(h).join(".grok")))
}

/// Event logs open per `grok` process, found through libproc, refreshed every
/// [`OPEN_FILES_INTERVAL`].
fn open_event_logs() -> impl FnMut() -> Vec<OpenFiles> + Send {
    let mut last: Option<(Instant, Vec<OpenFiles>)> = None;
    move || {
        if last
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= OPEN_FILES_INTERVAL)
        {
            // The installer's `grok` is a symlink to a versioned binary, and
            // the kernel may name the process after the target, cut to 16
            // bytes (`grok-1.0.88-maco`). A loose match is safe: only
            // processes holding an event log under the root count.
            let open =
                agentpulse_process::processes_where(|c| c == b"grok" || c.starts_with(b"grok-"))
                    .into_iter()
                    .filter_map(|p| {
                        agentpulse_process::open_files(p, |f| f.ends_with("/events.jsonl"))
                    })
                    .collect();
            last = Some((Instant::now(), open));
        }
        last.as_ref().map_or_else(Vec::new, |(_, o)| o.clone())
    }
}

/// `sessions/<encoded cwd>/<session id>/<file>` → the session id.
fn session_id(path: &Path) -> Option<String> {
    Some(path.parent()?.file_name()?.to_str()?.to_string())
}

/// `sessions/<encoded cwd>/<session id>/<file>` → the decoded cwd.
fn project_path(path: &Path) -> Option<String> {
    let dir = path.parent()?.parent()?.file_name()?.to_str()?;
    percent_encoding::percent_decode_str(dir)
        .decode_utf8()
        .ok()
        .map(|d| d.into_owned())
}

#[derive(Deserialize)]
struct Event {
    ts: Option<String>,
    r#type: Option<String>,
    /// `turn_started`: the model chosen for the turn.
    model_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageFile {
    updated_at: Option<String>,
    #[serde(default)]
    turns: Vec<Turn>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Turn {
    turn_number: Option<u64>,
    ended_at: Option<String>,
    primary_model_id: Option<String>,
    #[serde(default)]
    model_usage: BTreeMap<String, ModelUsage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelUsage {
    /// Includes `cached_read_tokens`.
    input_tokens: Option<u64>,
    /// Includes reasoning tokens.
    output_tokens: Option<u64>,
    cached_read_tokens: Option<u64>,
    cache_creation_tokens: Option<u64>,
    cost_usd_ticks: Option<u64>,
}

/// Folds one event line into the file's state, and its timestamp into `out`.
fn fold_event(line: &[u8], st: &mut FileState, out: Option<&mut Vec<i64>>) {
    let Ok(e) = serde_json::from_slice::<Event>(line) else {
        return;
    };
    let ts = e.ts.as_deref().and_then(ts_ms);
    st.started_ms = st.started_ms.or(ts);
    st.last_ms = st.last_ms.max(ts);
    match e.r#type.as_deref() {
        Some("turn_started") => {
            (st.busy, st.waiting_since) = (true, None);
            st.model = e.model_id.filter(|m| !m.is_empty()).or(st.model.take());
        }
        Some("turn_ended") => (st.busy, st.waiting_since) = (false, None),
        // A request without a time counts as old: waiting.
        Some("permission_requested") => st.waiting_since = Some(ts.unwrap_or(0)),
        Some("permission_resolved") => st.waiting_since = None,
        _ => {}
    }
    if let Some(out) = out {
        out.extend(ts);
    }
}

/// One event per (turn, model) of a parsed `usage.json`, and the newest
/// turn's model. Event ids are stable, so re-reading the rewritten file
/// upserts the same rows.
fn usage_events(u: &UsageFile, sid: &str) -> (Vec<UsageEvent>, Option<String>) {
    let mut events = vec![];
    for t in &u.turns {
        let Some(n) = t.turn_number else { continue };
        let Some(ts) = t
            .ended_at
            .as_ref()
            .or(u.updated_at.as_ref())
            .and_then(|s| ts_ms(s))
        else {
            continue;
        };
        for (model, m) in &t.model_usage {
            let cached = m.cached_read_tokens.unwrap_or(0);
            let usage = TokenUsage {
                input: m.input_tokens.unwrap_or(0).saturating_sub(cached),
                output: m.output_tokens.unwrap_or(0),
                cache_write_5m: m.cache_creation_tokens.unwrap_or(0),
                cache_write_1h: 0,
                cache_read: cached,
            };
            if usage.total() == 0 {
                continue;
            }
            events.push(UsageEvent {
                event_id: format!("{sid}:{n}:{model}"),
                session_id: sid.to_string(),
                model: model.clone(),
                ts_ms: ts,
                usage,
                fast: false,
                us_inference: false,
                unknown_tier: false,
                recorded_usd: m.cost_usd_ticks.map(|t| t as f64 / TICKS_PER_USD),
            });
        }
    }
    let model = u
        .turns
        .iter()
        .filter(|t| t.primary_model_id.is_some())
        .max_by_key(|t| t.turn_number)
        .and_then(|t| t.primary_model_id.clone());
    (events, model)
}

/// A regular file (not a symlink) at `path`.
fn is_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

/// Every session folder under `sessions/<encoded cwd>/`, one group per
/// session with its `events.jsonl` and `usage.json` (whichever exist). No
/// other file name is ever yielded. Symlinked folders are not followed.
fn discover(root: &Path) -> std::io::Result<Groups> {
    // Open-file paths from the kernel are canonical; keys must match them.
    let root = fs::canonicalize(root)?;
    let mut out = vec![];
    for project in fs::read_dir(root.join("sessions"))?.flatten() {
        if !project.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(sessions) = fs::read_dir(project.path()) else {
            tracing::warn!(path = %project.path().display(), "cannot read session folder");
            continue;
        };
        for s in sessions.flatten() {
            let Some(sid) = s.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !s.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let files: Vec<TranscriptFile> = [EVENTS, USAGE]
                .into_iter()
                .map(|n| s.path().join(n))
                .filter(|p| is_file(p))
                .map(|path| TranscriptFile {
                    is_main: path.ends_with(EVENTS),
                    path,
                })
                .collect();
            if !files.is_empty() {
                out.push((sid, files));
            }
        }
    }
    Ok(out)
}

#[derive(Default)]
struct Acc {
    timestamps: Vec<i64>,
    cwd: Option<String>,
    model: Option<String>,
}

impl AgentAdapter for GrokAdapter {
    fn provider(&self) -> Provider {
        Provider::Grok
    }

    fn poll(&mut self, offsets: &HashMap<String, u64>, since_ms: i64) -> anyhow::Result<PollBatch> {
        let mut batch = PollBatch {
            provider: Provider::Grok,
            files: vec![],
            events: vec![],
            sessions: vec![],
            more: false,
            rate_limits: None,
        };
        // live_sessions reports from the same scan.
        self.open = (self.open_files)();
        // Only event logs under this adapter's root: another `GROK_HOME` is not ours.
        let root = fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        for o in &mut self.open {
            o.paths.retain(|p| Path::new(p).starts_with(&root));
        }
        self.open.retain(|o| !o.paths.is_empty());
        // A session whose event log is open keeps its whole history, however old.
        let live: HashSet<String> = self
            .open
            .iter()
            .flat_map(|o| &o.paths)
            .filter_map(|p| session_id(Path::new(p)))
            .collect();
        // An open event log read to its end before this run started is not
        // read again: learn its start and turn state from the lines already
        // consumed.
        for path in self.open.iter().flat_map(|o| &o.paths) {
            let (false, Some(&end)) = (self.files.contains_key(path), offsets.get(path)) else {
                continue;
            };
            let st = self.files.entry(path.clone()).or_default();
            match read_lines(Path::new(path), 0, Some(end), |l| fold_event(l, st, None)) {
                Ok(o) => st.offset = o,
                Err(_) => {
                    self.files.remove(path);
                    tracing::warn!(path = %path, "cannot read event log");
                }
            }
        }
        let mut accs: BTreeMap<String, Acc> = BTreeMap::new();
        let mut events: Vec<UsageEvent> = vec![];
        let files = &mut self.files;
        let root = &self.root;
        (batch.files, batch.more) = self.tail.poll(
            &live,
            offsets,
            since_ms,
            || discover(root),
            |sid, f, start| {
                let acc = |accs: &mut BTreeMap<String, Acc>| {
                    let a = accs.entry(sid.to_string()).or_default();
                    a.cwd = a.cwd.take().or_else(|| project_path(&f.path));
                };
                // Discovery skipped symlinks, but the file may have been
                // replaced by one since: check again just before opening.
                if !is_file(&f.path) {
                    return Ok(start);
                }
                if f.path.ends_with(USAGE) {
                    // Rewritten whole at each turn's end: always read it all.
                    // A file caught mid-write does not parse; leaving the
                    // cursor where it was makes the next poll retry.
                    let bytes = fs::read(&f.path)?;
                    let Ok(u) = serde_json::from_slice::<UsageFile>(&bytes) else {
                        return Ok(start);
                    };
                    let (evs, model) = usage_events(&u, sid);
                    acc(&mut accs);
                    let a = accs.get_mut(sid).expect("just inserted");
                    a.model = model.or(a.model.take());
                    // Turn ends are activity too, so a session whose event
                    // log is missing still shows (replays are no-ops).
                    a.timestamps.extend(evs.iter().map(|e| e.ts_ms));
                    events.extend(evs);
                    return Ok(bytes.len() as u64);
                }
                let key = f.path.to_string_lossy().into_owned();
                // State read past `start` (a failed read, or a batch the store
                // did not keep) is rebuilt.
                if files.get(&key).is_some_and(|s| s.offset != start) {
                    files.remove(&key);
                }
                let seed = start > 0 && !files.contains_key(&key);
                let st = files.entry(key).or_default();
                st.offset = u64::MAX;
                if seed {
                    // Resuming mid-file: learn the start and turn state from
                    // the lines before the offset.
                    read_lines(&f.path, 0, Some(start), |l| fold_event(l, st, None))?;
                }
                let mut ts = vec![];
                let end = read_lines(&f.path, start, None, |l| fold_event(l, st, Some(&mut ts)))?;
                st.offset = end;
                if end > start {
                    acc(&mut accs);
                    let a = accs.get_mut(sid).expect("just inserted");
                    a.timestamps.extend(ts);
                    // Until the first turn ends there is no `usage.json`:
                    // name the model the turn started with meanwhile.
                    if a.model.is_none() && !f.path.with_file_name(USAGE).exists() {
                        a.model = st.model.clone();
                    }
                }
                Ok(end)
            },
        )?;
        batch.sessions = accs
            .into_iter()
            .map(|(session_id, mut acc)| {
                acc.timestamps.sort_unstable();
                SessionDelta {
                    session_id,
                    project_path: acc.cwd,
                    model: acc.model,
                    timestamps_ms: acc.timestamps,
                    last_error: None,
                    recorded_cost: None,
                }
            })
            .collect();
        events.sort_by(|a, b| (a.ts_ms, &a.event_id).cmp(&(b.ts_ms, &b.event_id)));
        batch.events = events;
        Ok(batch)
    }

    /// One per session whose `events.jsonl` some `grok` process holds open, as
    /// of the last poll. A process holding several sessions' logs is shared:
    /// its sessions are live (if active within [`SHARED_LIVE_WINDOW`]), but
    /// its resources cannot be split between them.
    fn live_sessions(&self) -> anyhow::Result<Vec<LiveSession>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let recent = now.saturating_sub(SHARED_LIVE_WINDOW).as_millis() as i64;
        let now_ms = now.as_millis() as i64;
        let mut out: BTreeMap<String, LiveSession> = BTreeMap::new();
        for o in &self.open {
            let sessions: BTreeMap<String, &String> = o
                .paths
                .iter()
                .filter_map(|p| Some((session_id(Path::new(p))?, p)))
                .collect();
            let shared = sessions.len() > 1;
            for (sid, path) in sessions {
                let st = self.files.get(path);
                if shared && st.and_then(|s| s.last_ms) < Some(recent) {
                    continue;
                }
                let l = LiveSession {
                    session_id: sid.clone(),
                    pid: o.pid,
                    proc_start_s: Some(o.start_time_s),
                    started_at_ms: st.and_then(|s| s.started_ms),
                    state: match st {
                        Some(s)
                            if s.waiting_since
                                .is_some_and(|t| now_ms - t >= WAITING_AFTER_MS) =>
                        {
                            LiveState::Waiting
                        }
                        Some(s) if s.busy => LiveState::Busy,
                        _ => LiveState::Idle,
                    },
                    project_path: project_path(Path::new(path)),
                    shared_process: shared,
                };
                match out.get_mut(&sid) {
                    // Held by several processes: prefer one that holds it
                    // alone, and any state but Idle.
                    Some(prev) => {
                        let state = if l.state == LiveState::Idle {
                            prev.state
                        } else {
                            l.state
                        };
                        if prev.shared_process && !l.shared_process {
                            *prev = l;
                        }
                        prev.state = state;
                    }
                    None => {
                        out.insert(sid, l);
                    }
                }
            }
        }
        Ok(out.into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_home_wins_when_set_and_non_empty() {
        let s = |v: &str| Some(OsString::from(v));
        assert_eq!(root_from(s("/g"), s("/home/x")), Some(PathBuf::from("/g")));
        assert_eq!(
            root_from(s(""), s("/home/x")),
            Some(PathBuf::from("/home/x/.grok"))
        );
        assert_eq!(
            root_from(None, s("/home/x")),
            Some(PathBuf::from("/home/x/.grok"))
        );
        assert_eq!(root_from(None, None), None);
    }

    #[test]
    fn discovery_yields_only_the_two_metadata_files() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path().join("sessions/%2Fp/sid");
        fs::create_dir_all(&s).unwrap();
        for n in [
            EVENTS,
            USAGE,
            "chat_history.jsonl",
            "updates.jsonl",
            "summary.json",
            "system_prompt.txt",
            "prompt_context.json",
            "tool_definitions.json",
            "signals.json",
        ] {
            fs::write(s.join(n), "{}\n").unwrap();
        }
        let groups = discover(dir.path()).unwrap();
        let names: Vec<String> = groups
            .iter()
            .flat_map(|(_, fs)| fs)
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [EVENTS, USAGE]);
    }

    #[test]
    fn session_and_folder_come_from_the_path() {
        let p = Path::new("/r/sessions/%2FUsers%2Fx%2Fmy%20proj/01a1-sid/events.jsonl");
        assert_eq!(session_id(p).as_deref(), Some("01a1-sid"));
        assert_eq!(project_path(p).as_deref(), Some("/Users/x/my proj"));
    }
}
