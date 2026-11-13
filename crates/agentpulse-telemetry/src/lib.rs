//! Claude Code adapter: reads transcripts and the live-session registry.
//!
//! Privacy: the structs below declare only metadata fields. Message content
//! is never named, so serde skips it; no line text is ever stored or logged.
//! Of `cost-state` records only the totals are read, plus how many models
//! `modelUsage` names (its contents are skipped unread).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agentpulse_core::{
    AgentAdapter, LiveSession, LiveState, PollBatch, Provider, RecordedCost, SessionDelta,
    TokenUsage, UsageEvent,
};
use chrono::NaiveDateTime;
use serde::Deserialize;

mod codex;
mod tail;

pub use codex::CodexAdapter;
use tail::{Groups, Tailer, TranscriptFile, read_lines, ts_ms};

pub struct ClaudeCodeAdapter {
    root: PathBuf,
    tail: Tailer,
}

impl ClaudeCodeAdapter {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            tail: Tailer::new(),
        }
    }

    pub fn with_rescan_interval(mut self, d: Duration) -> Self {
        self.tail.rescan_interval = d;
        self
    }

    pub fn with_read_budget(mut self, bytes: u64) -> Self {
        self.tail.read_budget = bytes;
        self
    }

    /// Number of full discovery scans done so far.
    pub fn full_scans(&self) -> u64 {
        self.tail.full_scans
    }

    /// `$CLAUDE_CONFIG_DIR` if set and non-empty, else `~/.claude`.
    pub fn default_root() -> Option<PathBuf> {
        let env = |k| std::env::var_os(k).filter(|v| !v.is_empty());
        env("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".claude")))
    }
}

#[derive(Deserialize)]
struct Record {
    r#type: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "isApiErrorMessage")]
    is_api_error: Option<bool>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    uuid: Option<String>,
    /// Claude Code copies history into other sessions' files but leaves this
    /// pointing at the session that produced the record.
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    message: Option<Message>,
    // `cost-state` records only.
    #[serde(rename = "totalCostUSD")]
    total_cost_usd: Option<f64>,
    #[serde(rename = "startTime")]
    start_time: Option<f64>,
    #[serde(rename = "totalDuration")]
    total_duration: Option<f64>,
    #[serde(rename = "hasUnknownModelCost")]
    has_unknown_model_cost: Option<bool>,
    /// Only whether it is empty matters: a record with no models is a $0
    /// placeholder, not a figure.
    #[serde(rename = "modelUsage")]
    model_usage: Option<HashMap<String, serde::de::IgnoredAny>>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_creation: Option<CacheCreation>,
    speed: Option<String>,
    inference_geo: Option<String>,
}

#[derive(Deserialize)]
struct CacheCreation {
    ephemeral_5m_input_tokens: Option<u64>,
    ephemeral_1h_input_tokens: Option<u64>,
}

/// For records whose shape is unexpected: they still count as activity.
#[derive(Deserialize)]
struct Fallback {
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

#[derive(Default)]
struct Acc {
    timestamps: Vec<i64>,
    cwd: Option<String>,
    /// (timestamp, model) of the newest main-file usage event.
    model: Option<(i64, String)>,
    last_error: Option<bool>,
    recorded_cost: Option<RecordedCost>,
}

fn to_event(r: &Record, session_id: &str, ts: i64) -> Option<UsageEvent> {
    if r.r#type.as_deref() != Some("assistant") {
        return None;
    }
    let m = r.message.as_ref()?;
    let u = m.usage.as_ref()?;
    if m.model.as_deref() == Some("<synthetic>") {
        return None;
    }
    let event_id =
        m.id.as_ref()
            .or(r.request_id.as_ref())
            .or(r.uuid.as_ref())?
            .clone();
    let mut usage = TokenUsage {
        input: u.input_tokens.unwrap_or(0),
        output: u.output_tokens.unwrap_or(0),
        cache_read: u.cache_read_input_tokens.unwrap_or(0),
        ..Default::default()
    };
    let (w5, w1) = u.cache_creation.as_ref().map_or((0, 0), |c| {
        (
            c.ephemeral_5m_input_tokens.unwrap_or(0),
            c.ephemeral_1h_input_tokens.unwrap_or(0),
        )
    });
    if w5 + w1 > 0 {
        usage.cache_write_5m = w5;
        usage.cache_write_1h = w1;
    } else {
        // 5 minutes is the API's default cache TTL.
        usage.cache_write_5m = u.cache_creation_input_tokens.unwrap_or(0);
    }
    if usage.total() == 0 {
        return None;
    }
    Some(UsageEvent {
        event_id,
        session_id: session_id.to_string(),
        model: m.model.clone().unwrap_or_default(),
        ts_ms: ts,
        usage,
        fast: u.speed.as_deref() == Some("fast"),
        us_inference: u.inference_geo.as_deref() == Some("us"),
        unknown_tier: false,
    })
}

/// Claude Code's own cumulative cost figure for the run that wrote it,
/// current at start + duration. Records naming no model carry $0 whatever the
/// session spent, so they are skipped rather than allowed to win.
fn to_recorded_cost(r: &Record) -> Option<RecordedCost> {
    if r.r#type.as_deref() != Some("cost-state")
        || r.model_usage.as_ref().is_none_or(HashMap::is_empty)
    {
        return None;
    }
    let usd = r.total_cost_usd.filter(|v| v.is_finite() && *v >= 0.0)?;
    let as_of = r.start_time? + r.total_duration?;
    Some(RecordedCost {
        usd,
        as_of_ms: as_of.is_finite().then_some(as_of as i64)?,
        unknown_models: r.has_unknown_model_cost == Some(true),
    })
}

fn fold_line(
    line: &[u8],
    file_sid: &str,
    is_main: bool,
    accs: &mut BTreeMap<String, Acc>,
    events: &mut HashMap<String, UsageEvent>,
    skipped: &mut u64,
) {
    let own_id = |id: Option<String>| id.filter(|i| !i.is_empty());
    let rec = match serde_json::from_slice::<Record>(line) {
        Ok(r) => r,
        Err(_) => match serde_json::from_slice::<Fallback>(line) {
            Ok(f) => {
                let sid = own_id(f.session_id).unwrap_or_else(|| file_sid.to_string());
                accs.entry(sid)
                    .or_default()
                    .timestamps
                    .extend(f.timestamp.as_deref().and_then(ts_ms));
                return;
            }
            Err(_) => {
                *skipped += 1;
                return;
            }
        },
    };
    let sid = own_id(rec.session_id.clone()).unwrap_or_else(|| file_sid.to_string());
    // Only a main file's own records describe its session's current state.
    let own = is_main && sid == file_sid;
    let ts = rec.timestamp.as_deref().and_then(ts_ms);
    let acc = accs.entry(sid.clone()).or_default();
    acc.timestamps.extend(ts);
    if acc.cwd.is_none() {
        acc.cwd = rec.cwd.clone().filter(|c| !c.is_empty());
    }
    if own && rec.r#type.as_deref() == Some("assistant") {
        acc.last_error = Some(rec.is_api_error == Some(true));
    }
    if let Some(c) = to_recorded_cost(&rec)
        && acc
            .recorded_cost
            .is_none_or(|old| c.as_of_ms > old.as_of_ms)
    {
        acc.recorded_cost = Some(c);
    }
    let Some(ts) = ts else { return };
    let Some(ev) = to_event(&rec, &sid, ts) else {
        return;
    };
    if own && !ev.model.is_empty() && acc.model.as_ref().is_none_or(|(t, _)| ts >= *t) {
        acc.model = Some((ts, ev.model.clone()));
    }
    match events.get(&ev.event_id) {
        Some(old) if old.usage.total() >= ev.usage.total() => {}
        _ => {
            events.insert(ev.event_id.clone(), ev);
        }
    }
}

fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return vec![];
    };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl") && p.is_file())
        .collect()
}

/// session id → its main file followed by its subagent files.
fn discover(projects: &Path) -> std::io::Result<Groups> {
    let mut groups: BTreeMap<String, Vec<TranscriptFile>> = BTreeMap::new();
    for slug in fs::read_dir(projects)?.flatten() {
        let Ok(entries) = fs::read_dir(slug.path()) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
                continue;
            };
            if p.is_dir() {
                for sub in jsonl_files(&p.join("subagents")) {
                    groups
                        .entry(stem.clone())
                        .or_default()
                        .push(TranscriptFile {
                            path: sub,
                            is_main: false,
                        });
                }
            } else if p.extension().is_some_and(|x| x == "jsonl") {
                groups.entry(stem).or_default().push(TranscriptFile {
                    path: p,
                    is_main: true,
                });
            }
        }
    }
    for files in groups.values_mut() {
        files.sort_by(|a, b| b.is_main.cmp(&a.is_main).then_with(|| a.path.cmp(&b.path)));
    }
    Ok(groups.into_iter().collect())
}

impl AgentAdapter for ClaudeCodeAdapter {
    fn provider(&self) -> Provider {
        Provider::ClaudeCode
    }

    fn poll(&mut self, offsets: &HashMap<String, u64>, since_ms: i64) -> anyhow::Result<PollBatch> {
        let mut batch = PollBatch {
            provider: Provider::ClaudeCode,
            files: vec![],
            events: vec![],
            sessions: vec![],
            more: false,
            rate_limits: None,
        };
        // A session still running keeps its whole history, however old its files are.
        let live: HashSet<String> = match self.live_sessions() {
            Ok(l) => l.into_iter().map(|l| l.session_id).collect(),
            Err(e) => {
                tracing::warn!(error = %e, "cannot read live-session registry");
                HashSet::new()
            }
        };
        let mut accs: BTreeMap<String, Acc> = BTreeMap::new();
        let mut events = HashMap::new();
        let mut skipped = 0u64;
        let projects = self.root.join("projects");
        (batch.files, batch.more) = self.tail.poll(
            &live,
            offsets,
            since_ms,
            || discover(&projects),
            |sid, f, start| {
                read_lines(&f.path, start, None, |line| {
                    fold_line(line, sid, f.is_main, &mut accs, &mut events, &mut skipped)
                })
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
                    last_error: acc.last_error,
                    recorded_cost: acc.recorded_cost,
                }
            })
            .collect();
        if skipped > 0 {
            tracing::debug!(skipped, "skipped unparseable transcript lines");
        }
        batch.events = events.into_values().collect();
        batch
            .events
            .sort_by(|a, b| (a.ts_ms, &a.event_id).cmp(&(b.ts_ms, &b.event_id)));
        Ok(batch)
    }

    fn live_sessions(&self) -> anyhow::Result<Vec<LiveSession>> {
        #[derive(Deserialize)]
        struct Reg {
            pid: Option<u32>,
            #[serde(rename = "sessionId")]
            session_id: Option<String>,
            cwd: Option<String>,
            #[serde(rename = "startedAt")]
            started_at: Option<i64>,
            #[serde(rename = "procStart")]
            proc_start: Option<String>,
            status: Option<String>,
        }
        let rd = match fs::read_dir(self.root.join("sessions")) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        let mut out = vec![];
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Ok(bytes) = fs::read(&p) else { continue };
            let Ok(r) = serde_json::from_slice::<Reg>(&bytes) else {
                continue;
            };
            let (Some(pid), Some(session_id)) = (r.pid, r.session_id) else {
                continue;
            };
            out.push(LiveSession {
                session_id,
                pid,
                // `ps -o lstart=` prints UTC on the machines this was verified on.
                proc_start_s: r
                    .proc_start
                    .and_then(|s| {
                        NaiveDateTime::parse_from_str(s.trim(), "%a %b %e %H:%M:%S %Y").ok()
                    })
                    .and_then(|d| u64::try_from(d.and_utc().timestamp()).ok()),
                started_at_ms: r.started_at,
                state: match r.status.as_deref() {
                    Some("busy") => LiveState::Busy,
                    Some("idle") => LiveState::Idle,
                    Some("waiting") => LiveState::Waiting,
                    _ => LiveState::Unknown,
                },
                project_path: r.cwd,
                shared_process: false,
            });
        }
        Ok(out)
    }
}
