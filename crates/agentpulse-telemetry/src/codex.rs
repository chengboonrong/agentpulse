//! Codex CLI adapter: tails rollout files, and finds live threads from the
//! rollouts that running `codex` processes hold open.
//!
//! Privacy: rollout lines carry prompts, responses, reasoning, tool calls and
//! file contents. Each line's record type is read from its first bytes, and
//! only the metadata records listed in [`wanted`] are parsed, into structs
//! that name metadata fields only, so serde skips everything else. No line
//! text is stored or logged.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agentpulse_core::{
    AgentAdapter, LiveSession, LiveState, PollBatch, Provider, SessionDelta, TokenUsage, UsageEvent,
};
use agentpulse_process::OpenFiles;
use serde::Deserialize;

use crate::tail::{Groups, Tailer, TranscriptFile, read_lines, ts_ms};

/// The record type and an event's subtype sit in a line's first bytes.
const HEAD: usize = 256;

/// Open rollouts are looked up at most this often: a session that starts or
/// ends is seen as live or not up to this late. The lookup walks the process
/// table and asks each `codex` process for its open files (several ms).
const OPEN_FILES_INTERVAL: Duration = Duration::from_secs(5);

/// A thread in a process holding several sessions' rollouts (VS Code, the
/// app-server) is live only if its session had activity this recently: such
/// processes keep rollouts from days ago open.
const SHARED_LIVE_WINDOW: Duration = Duration::from_secs(60 * 60);

type OpenFilesFn = Box<dyn FnMut() -> Vec<OpenFiles> + Send>;

pub struct CodexAdapter {
    root: PathBuf,
    tail: Tailer,
    /// Per rollout path: what its lines so far established.
    files: HashMap<String, FileState>,
    open_files: OpenFilesFn,
    /// Rollouts open per `codex` process, as of the last poll.
    open: Vec<OpenFiles>,
}

/// What the earlier lines of one rollout established. Kept across polls,
/// because tailing resumes mid-file.
#[derive(Default)]
struct FileState {
    /// The root session (`session_meta.session_id`); a subagent thread names
    /// its parent's.
    session: Option<String>,
    /// This file's thread is the root thread of its session.
    root_thread: bool,
    cwd: Option<String>,
    started_ms: Option<i64>,
    /// `thread_settings.model` of the latest settings line.
    settings_model: Option<String>,
    service_tier: Option<String>,
    /// turn id → model, from `turn_context`.
    turns: HashMap<String, String>,
    /// The latest `task_started` has no later `task_complete` / `turn_aborted`.
    busy: bool,
    /// Newest timestamp of any line read.
    last_ms: Option<i64>,
    /// Byte offset the state above was read up to; `u64::MAX` while a read is
    /// unfinished.
    offset: u64,
}

impl CodexAdapter {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            tail: Tailer::new(),
            files: HashMap::new(),
            open_files: Box::new(open_rollouts()),
            open: Vec::new(),
        }
    }

    /// Replaces the scan of open rollouts per `codex` process (for tests).
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

    /// `$CODEX_HOME` if set and non-empty, else `~/.codex`.
    pub fn default_root() -> Option<PathBuf> {
        let env = |k| std::env::var_os(k).filter(|v| !v.is_empty());
        env("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".codex")))
    }
}

/// Rollouts open per `codex` process, found through libproc, refreshed every
/// [`OPEN_FILES_INTERVAL`].
fn open_rollouts() -> impl FnMut() -> Vec<OpenFiles> + Send {
    let mut last: Option<(Instant, Vec<OpenFiles>)> = None;
    move || {
        if last
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= OPEN_FILES_INTERVAL)
        {
            let open = agentpulse_process::processes_named("codex")
                .into_iter()
                .filter_map(|p| agentpulse_process::open_files(p, is_rollout))
                .collect();
            last = Some((Instant::now(), open));
        }
        last.as_ref().map_or_else(Vec::new, |(_, o)| o.clone())
    }
}

fn is_rollout(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

/// `rollout-<timestamp>-<thread id>.jsonl` → the thread id (a 36-char UUID).
fn thread_id(path: &str) -> Option<String> {
    let stem = path.rsplit('/').next()?.strip_suffix(".jsonl")?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    (stem.starts_with("rollout-") && id.bytes().filter(|&b| b == b'-').count() == 4)
        .then(|| id.to_string())
}

#[derive(Deserialize)]
struct Line {
    timestamp: Option<String>,
    r#type: Option<String>,
    payload: Option<Payload>,
}

/// The metadata fields of the record types in [`wanted`]; nothing else.
#[derive(Default, Deserialize)]
struct Payload {
    /// `event_msg` subtype.
    r#type: Option<String>,
    /// `session_meta`: this thread.
    id: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    turn_id: Option<String>,
    model: Option<String>,
    response_id: Option<String>,
    usage: Option<Usage>,
    thread_settings: Option<ThreadSettings>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct ThreadSettings {
    model: Option<String>,
    service_tier: Option<String>,
}

/// The string value following `key` within `head`.
fn field<'a>(head: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let at = head.windows(key.len()).position(|w| w == key)? + key.len();
    let rest = &head[at..];
    Some(&rest[..rest.iter().position(|&b| b == b'"')?])
}

/// Whether a line is a record AgentPulse reads, judged from its first bytes.
/// A line whose type is not in the head is parsed (as metadata only) rather
/// than dropped.
fn wanted(head: &[u8]) -> bool {
    match field(head, br#""type":""#) {
        Some(b"session_meta" | b"turn_context" | b"token_usage_record") => true,
        Some(b"event_msg") => matches!(
            field(head, br#""payload":{"type":""#),
            None | Some(
                b"task_started" | b"task_complete" | b"turn_aborted" | b"thread_settings_applied"
            )
        ),
        Some(_) => false,
        None => true,
    }
}

#[derive(Default)]
struct Acc {
    timestamps: Vec<i64>,
    cwd: Option<String>,
    /// (timestamp, model) of the root thread's newest usage event.
    model: Option<(i64, String)>,
}

struct Out<'a> {
    accs: &'a mut BTreeMap<String, Acc>,
    events: &'a mut HashMap<String, UsageEvent>,
}

/// Folds one line into the file's state and, unless seeding (`out` None),
/// into the poll's sessions and events.
fn fold_line(line: &[u8], thread: &str, st: &mut FileState, out: Option<&mut Out>) {
    let head = &line[..line.len().min(HEAD)];
    if !wanted(head) {
        // Still activity: the timestamp leads the line.
        let ts = field(head, br#""timestamp":""#)
            .and_then(|t| std::str::from_utf8(t).ok())
            .and_then(ts_ms);
        st.last_ms = st.last_ms.max(ts);
        if let Some(out) = out {
            let sid = st.session.clone().unwrap_or_else(|| thread.to_string());
            out.accs.entry(sid).or_default().timestamps.extend(ts);
        }
        return;
    }
    let Ok(rec) = serde_json::from_slice::<Line>(line) else {
        return;
    };
    let ts = rec.timestamp.as_deref().and_then(ts_ms);
    st.last_ms = st.last_ms.max(ts);
    let p = rec.payload.unwrap_or_default();
    let kind = rec.r#type.as_deref();
    match (kind, p.r#type.as_deref()) {
        // A subagent's rollout also carries its parent's `session_meta`: only
        // this file's own thread sets its state.
        (Some("session_meta"), _) if p.id.as_deref() == Some(thread) => {
            st.session = p.session_id.clone().filter(|s| !s.is_empty());
            st.root_thread = st.session.is_some() && p.id == st.session;
            st.cwd = p.cwd.clone().filter(|c| !c.is_empty());
            st.started_ms = ts;
        }
        (Some("turn_context"), _) => {
            if let (Some(t), Some(m)) = (&p.turn_id, &p.model) {
                st.turns.insert(t.clone(), m.clone());
            }
        }
        (Some("event_msg"), Some("thread_settings_applied")) => {
            if let Some(s) = &p.thread_settings {
                if s.model.is_some() {
                    st.settings_model = s.model.clone();
                }
                st.service_tier = s.service_tier.clone();
            }
        }
        (Some("event_msg"), Some("task_started")) => st.busy = true,
        (Some("event_msg"), Some("task_complete" | "turn_aborted")) => st.busy = false,
        _ => {}
    }
    let Some(out) = out else { return };
    // Usage records name their root session; subagent threads count toward it.
    let sid = p
        .session_id
        .clone()
        .filter(|s| !s.is_empty() && kind == Some("token_usage_record"))
        .or_else(|| st.session.clone())
        .unwrap_or_else(|| thread.to_string());
    let acc = out.accs.entry(sid.clone()).or_default();
    acc.timestamps.extend(ts);
    if acc.cwd.is_none() {
        acc.cwd = st.cwd.clone();
    }
    if kind != Some("token_usage_record") {
        return;
    }
    let Some(ts) = ts else { return };
    let Some(ev) = to_event(&p, st, &sid, ts) else {
        return;
    };
    if st.root_thread && !ev.model.is_empty() && acc.model.as_ref().is_none_or(|(t, _)| ts >= *t) {
        acc.model = Some((ts, ev.model.clone()));
    }
    match out.events.get(&ev.event_id) {
        Some(old) if old.usage.total() >= ev.usage.total() => {}
        _ => {
            out.events.insert(ev.event_id.clone(), ev);
        }
    }
}

fn to_event(p: &Payload, st: &FileState, session_id: &str, ts: i64) -> Option<UsageEvent> {
    let u = p.usage.as_ref()?;
    let event_id = p.response_id.clone().filter(|r| !r.is_empty())?;
    let cached = u.cached_input_tokens.unwrap_or(0);
    // input_tokens includes cached input. Cache writes stay in input: OpenAI
    // bills them as normal input.
    let usage = TokenUsage {
        input: u.input_tokens.unwrap_or(0).saturating_sub(cached),
        output: u.output_tokens.unwrap_or(0),
        cache_write_5m: 0,
        cache_write_1h: 0,
        cache_read: cached,
    };
    if usage.total() == 0 {
        return None;
    }
    let model = p
        .turn_id
        .as_ref()
        .and_then(|t| st.turns.get(t))
        .or(st.settings_model.as_ref())
        .cloned()
        .unwrap_or_default();
    let (fast, unknown_tier) = match st.service_tier.as_deref() {
        None | Some("default") => (false, false),
        Some("fast" | "priority") => (true, false),
        Some(_) => (false, true),
    };
    Some(UsageEvent {
        event_id,
        session_id: session_id.to_string(),
        model,
        ts_ms: ts,
        usage,
        fast,
        us_inference: false,
        unknown_tier,
    })
}

/// Errors below `dir` are logged and skipped. Symlinked folders are not
/// followed (no loops).
fn walk(dir: &Path, out: &mut Groups) -> std::io::Result<()> {
    for e in fs::read_dir(dir)?.flatten() {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            if walk(&p, out).is_err() {
                tracing::warn!(path = %p.display(), "cannot read rollout folder");
            }
        } else if let Some(id) = p.to_str().filter(|s| is_rollout(s)).and_then(thread_id) {
            out.push((
                id,
                vec![TranscriptFile {
                    path: p,
                    is_main: true,
                }],
            ));
        }
    }
    Ok(())
}

/// Every rollout under `sessions/` and `archived_sessions/`, one group per
/// thread file. Compressed archives (`.jsonl.zst`) are not read.
fn discover(root: &Path) -> std::io::Result<Groups> {
    // Open-file paths from the kernel are canonical; keys must match them.
    let root = fs::canonicalize(root)?;
    let mut out = vec![];
    let mut found = false;
    for dir in ["sessions", "archived_sessions"] {
        match walk(&root.join(dir), &mut out) {
            Ok(()) => found = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    if !found {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    Ok(out)
}

impl AgentAdapter for CodexAdapter {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn poll(&mut self, offsets: &HashMap<String, u64>, since_ms: i64) -> anyhow::Result<PollBatch> {
        let mut batch = PollBatch {
            provider: Provider::Codex,
            files: vec![],
            events: vec![],
            sessions: vec![],
            more: false,
        };
        // live_sessions reports from the same scan.
        self.open = (self.open_files)();
        // Only rollouts under this adapter's root: another `CODEX_HOME` is not ours.
        let root = fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        for o in &mut self.open {
            o.paths.retain(|p| Path::new(p).starts_with(&root));
        }
        self.open.retain(|o| !o.paths.is_empty());
        // A thread whose rollout is open keeps its whole history, however old.
        let live: HashSet<String> = self
            .open
            .iter()
            .flat_map(|o| &o.paths)
            .filter_map(|p| thread_id(p))
            .collect();
        // An open rollout read to its end before this run started is not read
        // again: learn its session, start, cwd, models and task state from
        // the lines already consumed.
        for path in self.open.iter().flat_map(|o| &o.paths) {
            let (false, Some(&end)) = (self.files.contains_key(path), offsets.get(path)) else {
                continue;
            };
            let thread = thread_id(path).unwrap_or_default();
            let st = self.files.entry(path.clone()).or_default();
            match read_lines(Path::new(path), 0, Some(end), |l| {
                fold_line(l, &thread, st, None)
            }) {
                Ok(o) => st.offset = o,
                Err(_) => {
                    self.files.remove(path);
                    tracing::warn!(path = %path, "cannot read rollout");
                }
            }
        }
        let (mut accs, mut events) = (BTreeMap::new(), HashMap::new());
        let mut out = Out {
            accs: &mut accs,
            events: &mut events,
        };
        let files = &mut self.files;
        let root = &self.root;
        (batch.files, batch.more) = self.tail.poll(
            &live,
            offsets,
            since_ms,
            || discover(root),
            |thread, f, start| {
                let key = f.path.to_string_lossy().into_owned();
                // State read past `start` (a failed read, or a batch the store
                // did not keep) would price the re-read lines with later
                // settings: rebuild it.
                if files.get(&key).is_some_and(|s| s.offset != start) {
                    files.remove(&key);
                }
                let seed = start > 0 && !files.contains_key(&key);
                let st = files.entry(key).or_default();
                st.offset = u64::MAX;
                if seed {
                    // Resuming mid-file: learn the session, models, tier and
                    // task state from the lines before the offset.
                    read_lines(&f.path, 0, Some(start), |l| fold_line(l, thread, st, None))?;
                }
                let end = read_lines(&f.path, start, None, |l| {
                    fold_line(l, thread, st, Some(&mut out))
                })?;
                st.offset = end;
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
                    model: acc.model.map(|(_, m)| m),
                    timestamps_ms: acc.timestamps,
                    last_error: None,
                    recorded_cost: None,
                }
            })
            .collect();
        batch.events = events.into_values().collect();
        batch
            .events
            .sort_by(|a, b| (a.ts_ms, &a.event_id).cmp(&(b.ts_ms, &b.event_id)));
        Ok(batch)
    }

    /// One per session with a rollout open in some `codex` process, as of the
    /// last poll. A process holding rollouts of more than one session is
    /// shared: its sessions are live (if active within [`SHARED_LIVE_WINDOW`]),
    /// but its resources cannot be split between them.
    fn live_sessions(&self) -> anyhow::Result<Vec<LiveSession>> {
        let recent = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| {
                d.saturating_sub(SHARED_LIVE_WINDOW).as_millis() as i64
            });
        let mut out: BTreeMap<String, LiveSession> = BTreeMap::new();
        for o in &self.open {
            let mut sessions: BTreeMap<String, Vec<Option<&FileState>>> = BTreeMap::new();
            for path in &o.paths {
                let st = self.files.get(path);
                let Some(sid) = st
                    .and_then(|s| s.session.clone())
                    .or_else(|| thread_id(path))
                else {
                    continue;
                };
                sessions.entry(sid).or_default().push(st);
            }
            let shared = sessions.len() > 1;
            for (sid, states) in sessions {
                let states: Vec<&FileState> = states.into_iter().flatten().collect();
                if shared && states.iter().filter_map(|s| s.last_ms).max() < Some(recent) {
                    continue;
                }
                let busy = states.iter().any(|s| s.busy);
                let l = LiveSession {
                    session_id: sid.clone(),
                    pid: o.pid,
                    proc_start_s: Some(o.start_time_s),
                    started_at_ms: states.iter().filter_map(|s| s.started_ms).min(),
                    state: if busy {
                        LiveState::Busy
                    } else {
                        LiveState::Idle
                    },
                    project_path: states.iter().find_map(|s| s.cwd.clone()),
                    shared_process: shared,
                };
                match out.get_mut(&sid) {
                    // Held by several processes: prefer one that holds it
                    // alone, and Busy if any of them is.
                    Some(prev) => {
                        let busy = busy || prev.state == LiveState::Busy;
                        if prev.shared_process && !l.shared_process {
                            *prev = l;
                        }
                        if busy {
                            prev.state = LiveState::Busy;
                        }
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
    fn thread_id_from_file_name() {
        assert_eq!(
            thread_id("/x/rollout-2026-11-12T17-38-01-01a1cf1f-0cba-7370-8553-6b299fb729e3.jsonl")
                .as_deref(),
            Some("01a1cf1f-0cba-7370-8553-6b299fb729e3")
        );
        assert_eq!(thread_id("/x/rollout-short.jsonl"), None);
        assert_eq!(
            thread_id("/x/other-2026-11-12T17-38-01-01a1cf1f-0cba-7370-8553-6b299fb729e3.jsonl"),
            None
        );
        assert!(!is_rollout("/x/rollout-a.jsonl.zst"));
    }

    #[test]
    fn head_filter_keeps_only_metadata_records() {
        let line = |t: &str, sub: &str| {
            format!(
                r#"{{"timestamp":"2026-11-13T02:00:01.000Z","ordinal":7,"type":"{t}","payload":{{"type":"{sub}","content":"x"}}}}"#
            )
        };
        for (t, sub) in [
            ("session_meta", "x"),
            ("turn_context", "x"),
            ("token_usage_record", "x"),
            ("event_msg", "task_started"),
            ("event_msg", "task_complete"),
            ("event_msg", "turn_aborted"),
            ("event_msg", "thread_settings_applied"),
        ] {
            assert!(wanted(line(t, sub).as_bytes()), "{t}/{sub}");
        }
        for (t, sub) in [
            ("response_item", "message"),
            ("response_item", "function_call_output"),
            ("event_msg", "item_completed"),
            ("event_msg", "token_count"),
            ("event_msg", "agent_message"),
            ("compacted", "x"),
            ("world_state", "x"),
        ] {
            assert!(!wanted(line(t, sub).as_bytes()), "{t}/{sub}");
        }
        // A type beyond the head is parsed (metadata only) rather than lost.
        assert!(wanted(br#"{"timestamp":"2026-11-13T02:00:01.000Z"}"#));
    }
}
