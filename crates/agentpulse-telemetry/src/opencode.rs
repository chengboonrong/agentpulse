//! OpenCode adapter: reads tokens, cost and activity from OpenCode's SQLite
//! store, and finds live sessions from the working folders of running
//! `opencode` processes.
//!
//! Privacy: the store also holds every prompt, reply, tool call and diff
//! (`message.data`, `part.data`, session titles and summaries). No query
//! selects a `data` column or a session's `title`, `summary_diffs`, `revert`,
//! `permission` or `metadata`: every value is pulled with `json_extract` /
//! `json_type` inside SQLite, naming only the role, model id, completion time,
//! token counts and cost, so content never leaves SQLite. Of an error only
//! whether there is one is read; of `part` only `time_updated`. The store is
//! opened read-only. Nothing read is logged.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use agentpulse_core::{
    AgentAdapter, FileCursor, LiveSession, LiveState, PollBatch, ProcessIdentity, Provider,
    SessionDelta, TokenUsage, UsageEvent,
};
use anyhow::Context;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};

/// Running processes are looked up at most this often: a session that starts
/// or ends is seen as live or not up to this late.
const PROCESSES_INTERVAL: Duration = Duration::from_secs(5);

/// An unfinished response is Busy only while the session wrote something this
/// recently: an interrupted one stays unfinished for good.
const BUSY_WITHIN_MS: i64 = 120_000;

/// Messages read per poll at most; the rest comes with the next poll.
const MAX_ROWS: usize = 5_000;

/// Messages updated after the watermark, metadata only.
const MESSAGES: &str = "SELECT id, session_id, time_created, time_updated,
    json_extract(data, '$.role'), json_extract(data, '$.modelID'),
    json_extract(data, '$.time.completed'), json_type(data, '$.error') IS NOT NULL,
    json_extract(data, '$.tokens.input'), json_extract(data, '$.tokens.output'),
    json_extract(data, '$.tokens.reasoning'), json_extract(data, '$.tokens.cache.read'),
    json_extract(data, '$.tokens.cache.write'), json_extract(data, '$.cost')
  FROM message WHERE time_updated > ?1 ORDER BY time_updated, id LIMIT ?2";

const SESSION: &str = "SELECT parent_id, directory FROM session WHERE id = ?1";

/// The newest root session in a folder written to since a time.
const LIVE_SESSION: &str = "SELECT id, directory, time_created FROM (
    SELECT s.id, s.directory, s.time_created, max(s.time_updated, coalesce(
        (SELECT max(m.time_updated) FROM message m WHERE m.session_id = s.id), 0)) AS t
    FROM session s WHERE s.parent_id IS NULL AND s.directory = ?1)
  WHERE t >= ?2 ORDER BY t DESC LIMIT 1";

/// Whether a session's newest response is unfinished, and when it or one of
/// its subagents last wrote anything.
const LIVE_STATE: &str = "SELECT
    (SELECT json_extract(data, '$.time.completed') IS NULL AND json_type(data, '$.error') IS NULL
       FROM message WHERE session_id = ?1 AND json_extract(data, '$.role') = 'assistant'
       ORDER BY time_created DESC, id DESC LIMIT 1),
    max(coalesce((SELECT max(time_updated) FROM message WHERE session_id IN
            (SELECT ?1 UNION ALL SELECT id FROM session WHERE parent_id = ?1)), 0),
        coalesce((SELECT max(time_updated) FROM part WHERE session_id IN
            (SELECT ?1 UNION ALL SELECT id FROM session WHERE parent_id = ?1)), 0))";

/// (process, its working folder)
type ProcessesFn = Box<dyn FnMut() -> Vec<(ProcessIdentity, String)> + Send>;

/// Size and modification time of the store and its write-ahead log.
type Stamp = [Option<(u64, SystemTime)>; 2];

pub struct OpenCodeAdapter {
    db: PathBuf,
    conn: Option<Connection>,
    processes: ProcessesFn,
    /// The store's stamp and the cursor after the last poll: both unchanged
    /// means nothing new to read.
    last: Option<(Stamp, Option<u64>)>,
    /// Session id → (root session id, the root's folder).
    roots: HashMap<String, (String, Option<String>)>,
    live: Vec<Live>,
}

/// A live session as of the last poll; its state is decided when asked.
struct Live {
    session: LiveSession,
    unfinished: bool,
    last_ms: i64,
}

impl OpenCodeAdapter {
    pub fn new(db: PathBuf) -> Self {
        Self {
            db,
            conn: None,
            processes: Box::new(opencode_processes()),
            last: None,
            roots: HashMap::new(),
            live: Vec::new(),
        }
    }

    /// Replaces the scan of running `opencode` processes and their working
    /// folders (for tests).
    pub fn with_processes(
        mut self,
        f: impl FnMut() -> Vec<(ProcessIdentity, String)> + Send + 'static,
    ) -> Self {
        self.processes = Box::new(f);
        self
    }

    /// `$XDG_DATA_HOME/opencode/opencode.db` if set and non-empty, else
    /// `~/.local/share/opencode/opencode.db` (macOS too).
    pub fn default_root() -> Option<PathBuf> {
        db_from(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
    }

    fn read(
        &mut self,
        conn: &Connection,
        offsets: &HashMap<String, FileCursor>,
        since_ms: i64,
        batch: &mut PollBatch,
    ) -> rusqlite::Result<()> {
        let procs = (self.processes)();
        self.live = live(conn, &procs)?;
        let key = self.db.to_string_lossy().into_owned();
        let stamp = stamp(&self.db);
        let cursor = offsets.get(&key).map(|c| c.offset);
        if self.last == Some((stamp, cursor)) {
            return Ok(());
        }
        // The cursor is a `time_updated` watermark in ms, not a byte offset,
        // so the collector's "bytes read" figure is meaningless for OpenCode.
        let watermark = cursor.map_or(since_ms - 1, |c| c as i64);
        let rows: Vec<Msg> = conn
            .prepare_cached(MESSAGES)?
            .query_map(params![watermark, MAX_ROWS as i64], Msg::from_row)?
            .collect::<rusqlite::Result<_>>()?;
        let mut next = cursor;
        if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
            batch.more = rows.len() == MAX_ROWS;
            // Cut off mid-millisecond: re-read the last one (upserts are idempotent).
            let wm = if batch.more && last.updated > first.updated {
                last.updated - 1
            } else {
                last.updated
            };
            next = Some(wm as u64);
            batch.files.push(FileCursor {
                path: key,
                offset: wm as u64,
                identity: None,
            });
        }
        let mut accs: BTreeMap<String, Acc> = BTreeMap::new();
        for m in &rows {
            let (root, dir) = root_of(conn, &mut self.roots, &m.session_id)?;
            let acc = accs.entry(root.clone()).or_default();
            acc.cwd = acc.cwd.take().or(dir);
            acc.timestamps.push(m.created);
            acc.timestamps.extend(m.completed);
            if m.role.as_deref() != Some("assistant") {
                continue;
            }
            if m.session_id == root
                && acc
                    .newest
                    .as_ref()
                    .is_none_or(|n| (m.created, &m.id) > (n.0, &n.1))
            {
                acc.newest = Some((m.created, m.id.clone(), m.model.clone(), m.error));
            }
            let Some(ts) = m.completed else { continue };
            let [input, output, reasoning, cache_read, cache_write] = m.tokens;
            let usage = TokenUsage {
                input,
                // Reasoning is billed as output and not counted in it.
                output: output + reasoning,
                cache_write_5m: cache_write,
                cache_write_1h: 0,
                cache_read,
            };
            if usage.total() == 0 {
                continue;
            }
            batch.events.push(UsageEvent {
                event_id: m.id.clone(),
                session_id: root,
                model: m.model.clone().unwrap_or_default(),
                ts_ms: ts,
                usage,
                fast: false,
                us_inference: false,
                unknown_tier: false,
                recorded_usd: m.cost,
            });
        }
        batch.sessions = accs
            .into_iter()
            .map(|(session_id, mut acc)| {
                acc.timestamps.sort_unstable();
                let newest = acc.newest.take();
                SessionDelta {
                    session_id,
                    project_path: acc.cwd,
                    model: newest.as_ref().and_then(|n| n.2.clone()),
                    timestamps_ms: acc.timestamps,
                    last_error: newest.map(|n| n.3),
                    recorded_cost: None,
                }
            })
            .collect();
        batch
            .events
            .sort_by(|a, b| (a.ts_ms, &a.event_id).cmp(&(b.ts_ms, &b.event_id)));
        // Cut off by the cap: the rest is unread although the store is unchanged.
        self.last = (!batch.more).then_some((stamp, next));
        Ok(())
    }
}

fn db_from(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let set = |v: Option<OsString>| v.filter(|v| !v.is_empty());
    set(xdg)
        .map(PathBuf::from)
        .or_else(|| set(home).map(|h| PathBuf::from(h).join(".local/share")))
        .map(|d| d.join("opencode/opencode.db"))
}

/// Running `opencode` processes (TUI and `acp`) and their working folders,
/// found through libproc, refreshed every [`PROCESSES_INTERVAL`].
fn opencode_processes() -> impl FnMut() -> Vec<(ProcessIdentity, String)> + Send {
    let mut last: Option<(Instant, Vec<(ProcessIdentity, String)>)> = None;
    move || {
        if last
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= PROCESSES_INTERVAL)
        {
            let found = agentpulse_process::processes_named("opencode")
                .into_iter()
                .filter_map(|p| Some((p, agentpulse_process::cwd(p)?)))
                .collect();
            last = Some((Instant::now(), found));
        }
        last.as_ref().map_or_else(Vec::new, |(_, p)| p.clone())
    }
}

fn open(db: &Path) -> rusqlite::Result<Connection> {
    let c = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // OpenCode writes while running; never wait on it for long.
    c.busy_timeout(Duration::from_millis(250))?;
    Ok(c)
}

fn stamp(db: &Path) -> Stamp {
    let st = |p: &Path| {
        let m = fs::metadata(p).ok()?;
        Some((m.len(), m.modified().ok()?))
    };
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    [st(db), st(Path::new(&wal))]
}

/// The metadata of one message.
struct Msg {
    id: String,
    session_id: String,
    created: i64,
    updated: i64,
    role: Option<String>,
    model: Option<String>,
    completed: Option<i64>,
    error: bool,
    /// input, output, reasoning, cache read, cache write
    tokens: [u64; 5],
    cost: Option<f64>,
}

impl Msg {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        let text = |i| -> rusqlite::Result<Option<String>> {
            Ok(match r.get_ref(i)? {
                ValueRef::Text(t) => std::str::from_utf8(t).ok().map(str::to_string),
                _ => None,
            })
        };
        let num = |i| -> rusqlite::Result<Option<f64>> {
            Ok(match r.get_ref(i)? {
                ValueRef::Integer(v) => Some(v as f64),
                ValueRef::Real(v) => Some(v),
                _ => None,
            })
        };
        let count = |i| -> rusqlite::Result<u64> { Ok(num(i)?.map_or(0, |v| v.max(0.0) as u64)) };
        Ok(Self {
            id: r.get(0)?,
            session_id: r.get(1)?,
            created: r.get(2)?,
            updated: r.get(3)?,
            role: text(4)?,
            model: text(5)?,
            completed: num(6)?.map(|v| v as i64),
            error: r.get(7)?,
            tokens: [count(8)?, count(9)?, count(10)?, count(11)?, count(12)?],
            cost: num(13)?,
        })
    }
}

/// The root of `sid` (following `parent_id` up: subagent spend counts toward
/// its parent) and the root's folder.
fn root_of(
    conn: &Connection,
    roots: &mut HashMap<String, (String, Option<String>)>,
    sid: &str,
) -> rusqlite::Result<(String, Option<String>)> {
    if let Some(r) = roots.get(sid) {
        return Ok(r.clone());
    }
    let mut cur = sid.to_string();
    // Bounded, so a parent cycle cannot hang the poll.
    for _ in 0..16 {
        let Some((parent, dir)) = conn
            .prepare_cached(SESSION)?
            .query_row([&cur], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                ))
            })
            .optional()?
        else {
            break;
        };
        match parent {
            Some(p) if p != cur => cur = p,
            _ => {
                roots.insert(sid.to_string(), (cur.clone(), dir.clone()));
                return Ok((cur, dir));
            }
        }
    }
    Ok((cur, None))
}

/// One live session per process: the newest root session in the process's
/// working folder written to since the process started. Two processes in one
/// folder resolving to one session: the newer process keeps it.
// ponytail: cwd match, one session per process; a server hosting several folders (opencode serve/web) is not attributed.
fn live(conn: &Connection, procs: &[(ProcessIdentity, String)]) -> rusqlite::Result<Vec<Live>> {
    let mut out: BTreeMap<String, Live> = BTreeMap::new();
    for (p, cwd) in procs {
        let start_ms = p.start_time_s as i64 * 1000;
        let Some((sid, dir, created)) = conn
            .prepare_cached(LIVE_SESSION)?
            .query_row(params![cwd, start_ms], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            })
            .optional()?
        else {
            continue;
        };
        if out
            .get(&sid)
            .is_some_and(|l| l.session.proc_start_s >= Some(p.start_time_s))
        {
            continue;
        }
        let (unfinished, last_ms) = conn.prepare_cached(LIVE_STATE)?.query_row([&sid], |r| {
            Ok((r.get::<_, Option<bool>>(0)?.unwrap_or(false), r.get(1)?))
        })?;
        let session = LiveSession {
            session_id: sid.clone(),
            pid: p.pid,
            proc_start_s: Some(p.start_time_s),
            started_at_ms: created,
            state: LiveState::Idle,
            project_path: dir,
            shared_process: false,
        };
        out.insert(
            sid,
            Live {
                session,
                unfinished,
                last_ms,
            },
        );
    }
    Ok(out.into_values().collect())
}

#[derive(Default)]
struct Acc {
    timestamps: Vec<i64>,
    cwd: Option<String>,
    /// The root's own newest response: (created, id, model, error).
    newest: Option<(i64, String, Option<String>, bool)>,
}

impl AgentAdapter for OpenCodeAdapter {
    fn provider(&self) -> Provider {
        Provider::OpenCode
    }

    fn poll(
        &mut self,
        offsets: &HashMap<String, FileCursor>,
        since_ms: i64,
    ) -> anyhow::Result<PollBatch> {
        let mut batch = PollBatch {
            provider: Provider::OpenCode,
            files: vec![],
            events: vec![],
            sessions: vec![],
            more: false,
            rate_limits: None,
        };
        // No store: OpenCode is not installed or never ran.
        if !self.db.is_file() {
            (self.conn, self.last) = (None, None);
            self.live.clear();
            return Ok(batch);
        }
        // Taken out so that an error drops it: the next poll reopens.
        let conn = match self.conn.take() {
            Some(c) => c,
            None => open(&self.db).context("cannot open the OpenCode store")?,
        };
        if let Err(e) = self.read(&conn, offsets, since_ms, &mut batch) {
            self.last = None;
            return Err(e).context("cannot read the OpenCode store");
        }
        self.conn = Some(conn);
        Ok(batch)
    }

    /// One per running `opencode` process with a session in its working
    /// folder, as of the last poll. Busy while the newest response is
    /// unfinished and the session wrote within [`BUSY_WITHIN_MS`]; never
    /// Waiting (OpenCode does not store permission prompts).
    fn live_sessions(&self) -> anyhow::Result<Vec<LiveSession>> {
        let now_ms = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        Ok(self
            .live
            .iter()
            .map(|l| LiveSession {
                state: if l.unfinished && now_ms - l.last_ms <= BUSY_WITHIN_MS {
                    LiveState::Busy
                } else {
                    LiveState::Idle
                },
                ..l.session.clone()
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_data_home_wins_when_set_and_non_empty() {
        let s = |v: &str| Some(OsString::from(v));
        let db = |p: &str| Some(PathBuf::from(p));
        assert_eq!(db_from(s("/x"), s("/h")), db("/x/opencode/opencode.db"));
        assert_eq!(
            db_from(s(""), s("/h")),
            db("/h/.local/share/opencode/opencode.db")
        );
        assert_eq!(
            db_from(None, s("/h")),
            db("/h/.local/share/opencode/opencode.db")
        );
        assert_eq!(db_from(None, None), None);
    }

    #[test]
    fn no_query_selects_a_content_column() {
        for sql in [MESSAGES, SESSION, LIVE_SESSION, LIVE_STATE] {
            let rest = sql
                .replace("json_extract(data, '$.", "")
                .replace("json_type(data, '$.", "");
            assert!(!rest.contains("data"), "{sql}");
            for col in ["title", "summary", "revert", "permission", "metadata"] {
                assert!(!rest.contains(col), "{col} in {sql}");
            }
            // Only the existence of an error is read, never its contents.
            assert!(!sql.contains("json_extract(data, '$.error"), "{sql}");
        }
    }
}
