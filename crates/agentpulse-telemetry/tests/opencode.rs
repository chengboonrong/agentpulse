//! All fixtures are synthetic: invented ids, paths, models and token counts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agentpulse_core::{
    AgentAdapter, FileCursor, LiveState, PollBatch, ProcessIdentity, Provider, SessionDelta,
    TokenUsage,
};
use agentpulse_telemetry::OpenCodeAdapter;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

const S1: &str = "ses_synthetic1";
const S2: &str = "ses_synthetic2";
const KID: &str = "ses_synthetic_kid";
const DIR: &str = "/synthetic/my proj";
/// 2026-11-17T09:00:00Z
const T: i64 = 1_794_906_000_000;
const MARKER: &str = "SYNTHETIC-PRIVATE-7f3a";

/// The real tables (opencode 1.19.3), content columns included.
const SCHEMA: &str = "
CREATE TABLE session (id text PRIMARY KEY, project_id text NOT NULL, parent_id text,
  slug text NOT NULL, directory text NOT NULL, title text NOT NULL, version text NOT NULL,
  summary_diffs text, revert text, permission text, time_created integer NOT NULL,
  time_updated integer NOT NULL, metadata text);
CREATE TABLE message (id text PRIMARY KEY, session_id text NOT NULL,
  time_created integer NOT NULL, time_updated integer NOT NULL, data text NOT NULL);
CREATE TABLE part (id text PRIMARY KEY, message_id text NOT NULL, session_id text NOT NULL,
  time_created integer NOT NULL, time_updated integer NOT NULL, data text NOT NULL);";

type Procs = Arc<Mutex<Vec<(ProcessIdentity, String)>>>;

struct Fixture {
    _dir: tempfile::TempDir,
    db: PathBuf,
    conn: Connection,
    procs: Procs,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        Self {
            _dir: dir,
            db,
            conn,
            procs: Arc::default(),
        }
    }

    fn adapter(&self) -> OpenCodeAdapter {
        let procs = self.procs.clone();
        OpenCodeAdapter::new(self.db.clone()).with_processes(move || procs.lock().unwrap().clone())
    }

    fn session(&self, id: &str, parent: Option<&str>, dir: &str, created: i64, updated: i64) {
        self.conn
            .execute(
                "INSERT INTO session VALUES (?1, 'prj', ?2, 'slug', ?3, ?4, '1.19.3', ?4, ?4, ?4, ?5, ?6, ?4)",
                params![id, parent, dir, MARKER, created, updated],
            )
            .unwrap();
    }

    fn message(&self, id: &str, sid: &str, created: i64, updated: i64, data: Value) {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO message VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, sid, created, updated, data.to_string()],
            )
            .unwrap();
    }

    fn part(&self, id: &str, mid: &str, sid: &str, updated: i64) {
        let data = json!({"type": "text", "text": MARKER}).to_string();
        self.conn
            .execute(
                "INSERT OR REPLACE INTO part VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                params![id, mid, sid, updated, data],
            )
            .unwrap();
    }

    fn run(&self, pid: u32, start_ms: i64, cwd: &str) {
        self.procs.lock().unwrap().push((
            ProcessIdentity {
                pid,
                start_time_s: (start_ms / 1000) as u64,
            },
            cwd.to_string(),
        ));
    }
}

/// An assistant message. `tokens`: input, output, reasoning, cache read, cache write.
fn assistant(model: &str, completed: Option<i64>, tokens: [u64; 5], cost: Option<f64>) -> Value {
    let [input, output, reasoning, read, write] = tokens;
    let mut v = json!({
        "role": "assistant", "modelID": model, "providerID": "synthetic", "agent": "build",
        "mode": "build", "path": {"cwd": DIR, "root": DIR},
        "time": {"created": T},
        "tokens": {"input": input, "output": output, "reasoning": reasoning,
                   "cache": {"read": read, "write": write},
                   "total": input + output + reasoning + read + write},
    });
    if let Some(c) = completed {
        v["time"]["completed"] = json!(c);
    }
    if let Some(c) = cost {
        v["cost"] = json!(c);
    }
    v
}

fn user() -> Value {
    json!({"role": "user", "time": {"created": T}, "summary": {"title": MARKER}})
}

fn offsets(b: &PollBatch) -> HashMap<String, FileCursor> {
    b.files
        .iter()
        .map(|f| (f.path.clone(), f.clone()))
        .collect()
}

fn delta<'a>(b: &'a PollBatch, sid: &str) -> &'a SessionDelta {
    b.sessions.iter().find(|s| s.session_id == sid).unwrap()
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[test]
fn missing_store_is_empty_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = OpenCodeAdapter::new(dir.path().join("opencode.db")).with_processes(Vec::new);
    assert_eq!(a.provider(), Provider::OpenCode);
    assert!(a.poll(&HashMap::new(), 0).unwrap().is_empty());
    assert!(a.live_sessions().unwrap().is_empty());
}

#[test]
fn buckets_and_recorded_cost() {
    let f = Fixture::new();
    f.session(S1, None, DIR, T, T + 9);
    let tokens = [100, 20, 5, 300, 40];
    f.message(
        "msg_a",
        S1,
        T + 1,
        T + 2,
        assistant("model-a", Some(T + 2), tokens, Some(0.0123)),
    );
    // No cost recorded: the price table decides.
    f.message(
        "msg_b",
        S1,
        T + 3,
        T + 4,
        assistant("model-b", Some(T + 4), [1, 1, 0, 0, 0], None),
    );
    // No tokens: no event.
    f.message(
        "msg_c",
        S1,
        T + 5,
        T + 6,
        assistant("model-b", Some(T + 6), [0; 5], Some(0.0)),
    );
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.provider, Provider::OpenCode);
    assert_eq!(b.events.len(), 2);
    let e = &b.events[0];
    assert_eq!((e.event_id.as_str(), e.session_id.as_str()), ("msg_a", S1));
    assert_eq!((e.model.as_str(), e.ts_ms), ("model-a", T + 2));
    // Reasoning is not inside output, and is billed as output.
    assert_eq!(
        e.usage,
        TokenUsage {
            input: 100,
            output: 25,
            cache_read: 300,
            cache_write_5m: 40,
            cache_write_1h: 0,
        }
    );
    assert_eq!(e.recorded_usd, Some(0.0123));
    assert_eq!(b.events[1].recorded_usd, None);
    let s = delta(&b, S1);
    assert_eq!(s.project_path.as_deref(), Some(DIR));
    assert_eq!(s.model.as_deref(), Some("model-b"));
    assert_eq!(s.last_error, Some(false));
}

#[test]
fn only_completed_responses_are_events() {
    let f = Fixture::new();
    f.session(S1, None, DIR, T, T);
    f.message("msg_1", S1, T + 1, T + 1, user());
    f.message(
        "msg_2",
        S1,
        T + 2,
        T + 3,
        assistant("m", Some(T + 3), [5, 5, 0, 0, 0], Some(0.1)),
    );
    // Unfinished: activity only.
    f.message(
        "msg_3",
        S1,
        T + 4,
        T + 5,
        assistant("m", None, [7, 7, 0, 0, 0], None),
    );
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.events.len(), 1);
    assert_eq!(b.events[0].event_id, "msg_2");
    // Every message's creation, and each completion.
    assert_eq!(delta(&b, S1).timestamps_ms, [T + 1, T + 2, T + 3, T + 4]);
    assert_eq!(delta(&b, S1).last_error, Some(false));
    // The newest response failed.
    let mut err = assistant("m", Some(T + 7), [0; 5], None);
    err["error"] = json!({"name": "APIError", "data": {"message": MARKER}});
    f.message("msg_4", S1, T + 6, T + 7, err);
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(delta(&b, S1).last_error, Some(true));
}

#[test]
fn subagent_usage_lands_on_the_parent() {
    let f = Fixture::new();
    f.session(S1, None, DIR, T, T);
    f.session(KID, Some(S1), DIR, T + 1, T + 1);
    f.message(
        "msg_p",
        S1,
        T + 1,
        T + 2,
        assistant("parent-model", Some(T + 2), [1, 1, 0, 0, 0], None),
    );
    f.message(
        "msg_k",
        KID,
        T + 3,
        T + 4,
        assistant("kid-model", Some(T + 4), [2, 2, 0, 0, 0], None),
    );
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert!(b.events.iter().all(|e| e.session_id == S1));
    assert_eq!(b.events.len(), 2);
    assert_eq!(b.sessions.len(), 1);
    // The model is the parent's own; the subagent's activity counts.
    let s = delta(&b, S1);
    assert_eq!(s.model.as_deref(), Some("parent-model"));
    assert_eq!(s.timestamps_ms, [T + 1, T + 2, T + 3, T + 4]);
}

#[test]
fn the_watermark_reads_each_change_once() {
    let f = Fixture::new();
    f.session(S1, None, DIR, T, T);
    f.message(
        "msg_old",
        S1,
        T - 10,
        T - 10,
        assistant("m", Some(T - 10), [1, 1, 0, 0, 0], None),
    );
    f.message(
        "msg_1",
        S1,
        T + 1,
        T + 2,
        assistant("m", Some(T + 2), [10, 1, 0, 0, 0], None),
    );
    let mut a = f.adapter();
    // First run: only what changed since `since_ms`.
    let b1 = a.poll(&HashMap::new(), T).unwrap();
    assert_eq!(b1.events.len(), 1);
    assert_eq!(b1.files.len(), 1);
    assert_eq!(b1.files[0].path, f.db.to_string_lossy());
    assert_eq!(b1.files[0].offset, (T + 2) as u64);
    let off = offsets(&b1);
    assert!(a.poll(&off, T).unwrap().is_empty());
    // An updated response comes back under the same id, with more tokens.
    f.message(
        "msg_1",
        S1,
        T + 1,
        T + 8,
        assistant("m", Some(T + 8), [30, 3, 0, 0, 0], None),
    );
    let b2 = a.poll(&off, T).unwrap();
    assert_eq!(b2.events.len(), 1);
    assert_eq!(b2.events[0].event_id, "msg_1");
    assert_eq!(b2.events[0].usage.input, 30);
    // A restart at the stored cursor reads nothing older.
    let off = offsets(&b2);
    assert!(f.adapter().poll(&off, 0).unwrap().is_empty());
    // A batch the store did not keep is read again.
    let b3 = a.poll(&offsets(&b1), T).unwrap();
    assert_eq!(b3.events, b2.events);
}

#[test]
fn a_large_backlog_comes_in_several_polls() {
    let f = Fixture::new();
    f.session(S1, None, DIR, T, T);
    let tx = f.conn.unchecked_transaction().unwrap();
    for i in 0..5_001 {
        let ts = T + i;
        f.message(
            &format!("msg_{i:05}"),
            S1,
            ts,
            ts,
            assistant("m", Some(ts), [1, 0, 0, 0, 0], None),
        );
    }
    tx.commit().unwrap();
    let mut a = f.adapter();
    let mut off = HashMap::new();
    let mut ids = std::collections::BTreeSet::new();
    let mut polls = 0;
    loop {
        let b = a.poll(&off, 0).unwrap();
        ids.extend(b.events.iter().map(|e| e.event_id.clone()));
        off.extend(offsets(&b));
        polls += 1;
        if !b.more {
            break;
        }
    }
    assert_eq!(polls, 2);
    assert_eq!(ids.len(), 5_001);
}

#[test]
fn content_never_leaves_the_store() {
    let f = Fixture::new();
    let now = now_ms();
    f.session(S1, None, DIR, now - 5_000, now - 1_000);
    f.message("msg_u", S1, now - 4_000, now - 4_000, user());
    let mut err = assistant("m", Some(now - 2_000), [3, 3, 0, 0, 0], Some(0.5));
    err["error"] = json!({"name": "APIError", "data": {"message": MARKER}});
    err["system"] = json!(MARKER);
    f.message("msg_a", S1, now - 3_000, now - 2_000, err);
    f.part("prt_1", "msg_u", S1, now - 1_500);
    f.run(1, now - 60_000, DIR);
    let mut a = f.adapter();
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.events.len(), 1);
    let live = a.live_sessions().unwrap();
    assert_eq!(live.len(), 1);
    let shown = format!("{b:?} {live:?}");
    assert!(!shown.contains(MARKER), "{shown}");
}

#[test]
fn live_sessions_follow_process_folders() {
    let f = Fixture::new();
    let now = now_ms();
    let start = now - 3_600_000;
    // The process's session: newest root session in its folder since it started.
    f.session("ses_older", None, DIR, start - 9_000, start - 5_000);
    f.session(S1, None, DIR, start + 1_000, now - 1_000);
    f.session(KID, Some(S1), DIR, now - 500, now - 500);
    f.message(
        "msg_1",
        S1,
        now - 2_000,
        now - 1_500,
        assistant("m", None, [1, 0, 0, 0, 0], None),
    );
    // Another folder, idle and old but written after its process started.
    f.session(S2, None, "/synthetic/home", start, start + 60_000);
    f.message(
        "msg_2",
        S2,
        start + 10_000,
        start + 60_000,
        assistant("m", None, [1, 0, 0, 0, 0], None),
    );
    f.run(11, start, DIR);
    f.run(12, start - 60_000, "/synthetic/home");
    // A process in a folder with no session since it started is not live.
    f.run(13, now - 10, "/synthetic/elsewhere");
    let mut a = f.adapter();
    a.poll(&HashMap::new(), 0).unwrap();
    let mut live = a.live_sessions().unwrap();
    live.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    assert_eq!(live.len(), 2);
    let (l1, l2) = (&live[0], &live[1]);
    assert_eq!((l1.session_id.as_str(), l1.pid), (S1, 11));
    assert_eq!(l1.proc_start_s, Some((start / 1000) as u64));
    assert_eq!(l1.started_at_ms, Some(start + 1_000));
    assert_eq!(l1.project_path.as_deref(), Some(DIR));
    assert!(!l1.shared_process);
    // Unfinished and written to just now.
    assert_eq!(l1.state, LiveState::Busy);
    // Unfinished but silent for long: an interrupted response.
    assert_eq!((l2.session_id.as_str(), l2.state), (S2, LiveState::Idle));
    // Finished: Idle.
    f.message(
        "msg_1",
        S1,
        now - 2_000,
        now,
        assistant("m", Some(now), [1, 0, 0, 0, 0], None),
    );
    a.poll(&HashMap::new(), 0).unwrap();
    let live = a.live_sessions().unwrap();
    assert!(live.iter().all(|l| l.state == LiveState::Idle));
    // Two processes in one folder: the newer one keeps the session.
    f.run(14, start + 5_000, DIR);
    a.poll(&HashMap::new(), 0).unwrap();
    let live = a.live_sessions().unwrap();
    assert_eq!(live.len(), 2);
    assert_eq!(live.iter().find(|l| l.session_id == S1).unwrap().pid, 14);
    // Processes gone: nothing live.
    f.procs.lock().unwrap().clear();
    a.poll(&HashMap::new(), 0).unwrap();
    assert!(a.live_sessions().unwrap().is_empty());
}

#[test]
fn a_subagent_writing_keeps_its_parent_busy() {
    let f = Fixture::new();
    let now = now_ms();
    let start = now - 3_600_000;
    f.session(S1, None, DIR, start + 1_000, start + 2_000);
    f.session(KID, Some(S1), DIR, start + 1_500, start + 2_000);
    // The parent's response has waited on its subagent for ten minutes.
    f.message(
        "msg_1",
        S1,
        start + 1_000,
        now - 600_000,
        assistant("m", None, [1, 0, 0, 0, 0], None),
    );
    f.message(
        "msg_k",
        KID,
        now - 600_000,
        now - 600_000,
        assistant("m", None, [1, 0, 0, 0, 0], None),
    );
    f.part("prt_k", "msg_k", KID, now - 1_000);
    f.run(11, start, DIR);
    let mut a = f.adapter();
    a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(a.live_sessions().unwrap()[0].state, LiveState::Busy);
}
