//! All fixtures are synthetic: invented ids, paths, models and token counts.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentpulse_core::{AgentAdapter, FileCursor, LiveState, PollBatch, Provider, TokenUsage};
use agentpulse_process::OpenFiles;
use agentpulse_telemetry::CodexAdapter;

const ROOT_T: &str = "01a1cf1f-0000-7000-8000-000000000001";
const SUB_T: &str = "01a1cf1f-0000-7000-8000-000000000002";
const OTHER_T: &str = "01a1cf1f-0000-7000-8000-000000000003";
const T1: &str = "2026-11-13T02:00:01.000Z";
const T2: &str = "2026-11-13T02:00:02.000Z";
const T3: &str = "2026-11-13T02:00:03.000Z";
const T4: &str = "2026-11-13T02:00:04.000Z";

fn rec(ts: &str, kind: &str, payload: &str) -> String {
    format!(r#"{{"timestamp":"{ts}","ordinal":1,"type":"{kind}","payload":{payload}}}"#)
}

fn meta(thread: &str, session: &str) -> String {
    meta_at(T1, thread, session)
}

fn meta_at(ts: &str, thread: &str, session: &str) -> String {
    rec(
        ts,
        "session_meta",
        &format!(
            r#"{{"id":"{thread}","session_id":"{session}","timestamp":"{ts}","cwd":"/synthetic/proj","cli_version":"0.164.0","source":"cli","originator":"codex_cli","base_instructions":{{"text":"SYNTHETIC instructions"}}}}"#
        ),
    )
}

/// A timestamp minutes ago, for rules that depend on the real clock.
fn recent() -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(5))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn turn(ts: &str, turn: &str, model: &str) -> String {
    rec(
        ts,
        "turn_context",
        &format!(
            r#"{{"turn_id":"{turn}","model":"{model}","cwd":"/synthetic/proj","sandbox_policy":{{"type":"x"}},"summary":"auto"}}"#
        ),
    )
}

fn settings(ts: &str, model: &str, tier: &str) -> String {
    rec(
        ts,
        "event_msg",
        &format!(
            r#"{{"type":"thread_settings_applied","thread_id":"t","thread_settings":{{"model":"{model}","service_tier":{tier}}}}}"#
        ),
    )
}

fn task(ts: &str, kind: &str) -> String {
    rec(
        ts,
        "event_msg",
        &format!(r#"{{"type":"{kind}","turn_id":"turn-1"}}"#),
    )
}

#[allow(clippy::too_many_arguments)]
fn usage(
    ts: &str,
    resp: &str,
    thread: &str,
    session: &str,
    turn: &str,
    input: u64,
    cached: u64,
    output: u64,
) -> String {
    let u = format!(
        r#"{{"input_tokens":{input},"cached_input_tokens":{cached},"cache_write_input_tokens":0,"output_tokens":{output},"reasoning_output_tokens":1,"total_tokens":{}}}"#,
        input + output
    );
    rec(
        ts,
        "token_usage_record",
        &format!(
            r#"{{"thread_id":"{thread}","turn_id":"{turn}","session_id":"{session}","root_turn_id":"{turn}","response_id":"{resp}","usage":{u},"turn_token_usage":{u},"thread_token_usage":{u}}}"#
        ),
    )
}

/// A content record: valid JSON only up to the head, so it is provably not
/// parsed, only its leading timestamp read.
fn content(ts: &str) -> String {
    format!(
        r#"{{"timestamp":"{ts}","ordinal":5,"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"SYNTHETIC PROMPT"#
    )
}

fn token_count(ts: &str) -> String {
    rec(
        ts,
        "event_msg",
        r#"{"type":"token_count","info":{"total_token_usage":{"input_tokens":999999,"output_tokens":999999}}}"#,
    )
}

/// A `token_count` event with plan limits, as Codex 0.164 writes it.
fn limits(ts: &str, used: f64, secondary: &str) -> String {
    rec(
        ts,
        "event_msg",
        &format!(
            r#"{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":999999}}}},"rate_limits":{{"limit_id":"codex","limit_name":null,"primary":{{"used_percent":{used},"window_minutes":10080,"resets_at":1795138706}},"secondary":{secondary},"credits":{{"has_credits":false}},"plan_type":"prolite"}}}}"#
        ),
    )
}

fn write(path: &Path, lines: &[String]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut s = lines.join("\n");
    s.push('\n');
    fs::write(path, s).unwrap();
}

fn append(path: &Path, lines: &[String]) {
    let mut f = OpenOptions::new().append(true).open(path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
}

type Open = Arc<Mutex<Vec<OpenFiles>>>;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    open: Open,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // The kernel reports canonical paths (/private/var/...).
        let root = fs::canonicalize(dir.path()).unwrap();
        Self {
            _dir: dir,
            root,
            open: Arc::default(),
        }
    }

    fn adapter(&self) -> CodexAdapter {
        let open = self.open.clone();
        CodexAdapter::new(self.root.clone()).with_open_files(move || open.lock().unwrap().clone())
    }

    fn rollout(&self, thread: &str) -> PathBuf {
        self.root.join(format!(
            "sessions/2026/11/13/rollout-2026-11-13T02-00-00-{thread}.jsonl"
        ))
    }

    fn hold(&self, pid: u32, paths: &[&Path]) {
        self.open.lock().unwrap().push(OpenFiles {
            pid,
            start_time_s: 1_794_535_000,
            paths: paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        });
    }
}

fn offsets(b: &PollBatch) -> HashMap<String, FileCursor> {
    b.files
        .iter()
        .map(|f| (f.path.clone(), f.clone()))
        .collect()
}

fn session<'a>(b: &'a PollBatch, sid: &str) -> &'a agentpulse_core::SessionDelta {
    b.sessions.iter().find(|s| s.session_id == sid).unwrap()
}

#[test]
fn missing_root_is_empty_not_an_error() {
    let f = Fixture::new();
    let mut a = CodexAdapter::new(f.root.join("no-codex-here")).with_open_files(Vec::new);
    assert_eq!(a.provider(), Provider::Codex);
    assert!(a.poll(&HashMap::new(), 0).unwrap().is_empty());
    assert!(a.live_sessions().unwrap().is_empty());
}

#[test]
fn usage_maps_buckets_and_counts_each_response_once() {
    let f = Fixture::new();
    let p = f.rollout(ROOT_T);
    let u = usage(T2, "resp_1", ROOT_T, ROOT_T, "turn-1", 1_000, 600, 50);
    write(
        &p,
        &[
            meta(ROOT_T, ROOT_T),
            turn(T1, "turn-1", "gpt-6.1-sol"),
            u.clone(),
            u,
            token_count(T3),
        ],
    );
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.provider, Provider::Codex);
    assert_eq!(b.events.len(), 1);
    let e = &b.events[0];
    assert_eq!(
        (e.event_id.as_str(), e.session_id.as_str()),
        ("resp_1", ROOT_T)
    );
    assert_eq!(
        e.usage,
        TokenUsage {
            input: 400,
            cache_read: 600,
            output: 50,
            ..Default::default()
        }
    );
    // The cumulative token_count line never adds tokens.
    assert_eq!(e.usage.total(), 1_050);
    let s = session(&b, ROOT_T);
    assert_eq!(s.project_path.as_deref(), Some("/synthetic/proj"));
    assert_eq!(s.model.as_deref(), Some("gpt-6.1-sol"));
    assert_eq!(s.last_error, None);
}

#[test]
fn subagent_tokens_belong_to_the_root_session() {
    let f = Fixture::new();
    write(
        &f.rollout(ROOT_T),
        &[
            meta(ROOT_T, ROOT_T),
            turn(T1, "turn-1", "gpt-6.1-sol"),
            usage(T2, "resp_root", ROOT_T, ROOT_T, "turn-1", 10, 0, 1),
        ],
    );
    // A subagent rollout: its own session_meta, then its parent's copy.
    write(
        &f.rollout(SUB_T),
        &[
            meta(SUB_T, ROOT_T),
            meta(ROOT_T, ROOT_T),
            turn(T1, "turn-s", "gpt-6-luna"),
            usage(T4, "resp_sub", SUB_T, ROOT_T, "turn-s", 20, 0, 2),
        ],
    );
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.sessions.len(), 1, "one root session, no row per thread");
    assert!(b.events.iter().all(|e| e.session_id == ROOT_T));
    let model = |id: &str| {
        b.events
            .iter()
            .find(|e| e.event_id == id)
            .unwrap()
            .model
            .clone()
    };
    assert_eq!(
        (model("resp_root"), model("resp_sub")),
        ("gpt-6.1-sol".into(), "gpt-6-luna".into())
    );
    // The session shows the root thread's model, not the subagent's newer one,
    // even though the subagent's file repeats the parent's session_meta.
    assert_eq!(session(&b, ROOT_T).model.as_deref(), Some("gpt-6.1-sol"));
    assert_eq!(b.files.len(), 2);
}

#[test]
fn cache_writes_are_billed_as_input() {
    let f = Fixture::new();
    let u = usage(T2, "resp_1", ROOT_T, ROOT_T, "turn-1", 1_000, 600, 50).replace(
        r#""cache_write_input_tokens":0"#,
        r#""cache_write_input_tokens":100"#,
    );
    write(&f.rollout(ROOT_T), &[meta(ROOT_T, ROOT_T), u]);
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(
        b.events[0].usage,
        TokenUsage {
            input: 400,
            cache_read: 600,
            output: 50,
            ..Default::default()
        }
    );
}

#[test]
fn rereading_from_an_unstored_offset_uses_the_state_of_that_offset() {
    let f = Fixture::new();
    let p = f.rollout(ROOT_T);
    write(
        &p,
        &[
            meta(ROOT_T, ROOT_T),
            turn(T1, "turn-1", "gpt-6.1-sol"),
            settings(T1, "gpt-6.1-sol", r#""default""#),
        ],
    );
    let mut a = f.adapter();
    let stored = offsets(&a.poll(&HashMap::new(), 0).unwrap());
    // A standard-tier response, then the tier switches to priority.
    append(
        &p,
        &[
            usage(T2, "resp_1", ROOT_T, ROOT_T, "turn-1", 10, 0, 1),
            settings(T3, "gpt-6.1-sol", r#""priority""#),
        ],
    );
    let fast = |b: PollBatch| {
        b.events
            .iter()
            .find(|e| e.event_id == "resp_1")
            .unwrap()
            .fast
    };
    assert!(!fast(a.poll(&stored, 0).unwrap()));
    // The store did not keep that batch: the same lines are read again.
    assert!(
        !fast(a.poll(&stored, 0).unwrap()),
        "re-read with the tier as of the stored offset"
    );
}

#[test]
fn discovery_skips_symlinked_and_unreadable_folders() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    write(&f.rollout(ROOT_T), &[meta(ROOT_T, ROOT_T)]);
    let sessions = f.root.join("sessions");
    symlink(&sessions, sessions.join("loop")).unwrap();
    let locked = sessions.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let b = f.adapter().poll(&HashMap::new(), 0);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let b = b.unwrap();
    assert_eq!(b.files.len(), 1, "read once, not again through the link");
}

#[test]
fn model_comes_from_the_records_turn_then_thread_settings() {
    let f = Fixture::new();
    write(
        &f.rollout(ROOT_T),
        &[
            meta(ROOT_T, ROOT_T),
            settings(T1, "gpt-6-astra", r#""default""#),
            turn(T1, "turn-a", "gpt-6.1-terra"),
            turn(T2, "turn-b", "gpt-6-sol"),
            // Turn a's record comes after turn b started: still turn a's model.
            usage(T3, "resp_a", ROOT_T, ROOT_T, "turn-a", 10, 0, 1),
            usage(T3, "resp_x", ROOT_T, ROOT_T, "turn-unknown", 10, 0, 1),
        ],
    );
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    let model = |id: &str| {
        b.events
            .iter()
            .find(|e| e.event_id == id)
            .unwrap()
            .model
            .clone()
    };
    assert_eq!(model("resp_a"), "gpt-6.1-terra");
    assert_eq!(
        model("resp_x"),
        "gpt-6-astra",
        "falls back to thread settings"
    );
}

#[test]
fn resuming_from_an_offset_seeds_state_from_earlier_lines() {
    let f = Fixture::new();
    let sub = f.rollout(SUB_T);
    write(
        &sub,
        &[
            meta(SUB_T, ROOT_T),
            settings(T1, "gpt-6-astra", r#""priority""#),
            turn(T1, "turn-1", "gpt-6.1-terra"),
            task(T1, "task_started"),
        ],
    );
    let b1 = f.adapter().poll(&HashMap::new(), 0).unwrap();
    append(
        &sub,
        &[
            usage(T2, "resp_late", SUB_T, ROOT_T, "turn-1", 10, 0, 1),
            content(T3),
        ],
    );
    // A fresh adapter (an app restart) resumes at the stored offset.
    let mut a = f.adapter();
    f.hold(7, &[&sub]);
    let b2 = a.poll(&offsets(&b1), 0).unwrap();
    assert_eq!(b2.events.len(), 1);
    let e = &b2.events[0];
    assert_eq!(e.model, "gpt-6.1-terra");
    assert!(e.fast, "service tier seeded from before the offset");
    // Activity from the appended lines goes to the root session.
    assert_eq!(b2.sessions.len(), 1);
    assert_eq!(session(&b2, ROOT_T).timestamps_ms.len(), 2);
    let live = a.live_sessions().unwrap();
    assert_eq!(live[0].session_id, ROOT_T);
    assert_eq!(live[0].state, LiveState::Busy, "task state seeded too");
    assert_eq!(live[0].project_path.as_deref(), Some("/synthetic/proj"));
}

#[test]
fn open_rollout_already_read_to_the_end_still_reports_its_session() {
    let f = Fixture::new();
    let (root, sub) = (f.rollout(ROOT_T), f.rollout(SUB_T));
    write(&root, &[meta(ROOT_T, ROOT_T)]);
    write(&sub, &[meta(SUB_T, ROOT_T), task(T2, "task_started")]);
    let b1 = f.adapter().poll(&HashMap::new(), 0).unwrap();
    // App restart: every offset is at the end, nothing new is appended.
    f.hold(9, &[&root, &sub]);
    let mut a = f.adapter();
    assert!(a.poll(&offsets(&b1), 0).unwrap().is_empty());
    let live = a.live_sessions().unwrap();
    assert_eq!(
        live.len(),
        1,
        "the subagent thread is not a session of its own"
    );
    let l = &live[0];
    assert_eq!((l.session_id.as_str(), l.shared_process), (ROOT_T, false));
    assert_eq!(l.state, LiveState::Busy);
    assert_eq!(l.started_at_ms, Some(1_794_535_201_000));
    assert_eq!(l.project_path.as_deref(), Some("/synthetic/proj"));
}

#[test]
fn service_tier_sets_pricing_flags() {
    let f = Fixture::new();
    let mut lines = vec![meta(ROOT_T, ROOT_T), turn(T1, "turn-1", "gpt-6.1-sol")];
    for (i, tier) in [
        r#""default""#,
        "null",
        r#""fast""#,
        r#""priority""#,
        r#""flex""#,
    ]
    .iter()
    .enumerate()
    {
        lines.push(settings(T1, "gpt-6.1-sol", tier));
        lines.push(usage(
            T2,
            &format!("resp_{i}"),
            ROOT_T,
            ROOT_T,
            "turn-1",
            10,
            0,
            1,
        ));
    }
    write(&f.rollout(ROOT_T), &lines);
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    let flags = |i: usize| {
        let e = b
            .events
            .iter()
            .find(|e| e.event_id == format!("resp_{i}"))
            .unwrap();
        (e.fast, e.unknown_tier, e.us_inference)
    };
    assert_eq!(flags(0), (false, false, false));
    assert_eq!(flags(1), (false, false, false), "absent tier is standard");
    assert_eq!(flags(2), (true, false, false));
    assert_eq!(flags(3), (true, false, false));
    assert_eq!(
        flags(4),
        (false, true, false),
        "unknown tier stays unpriced"
    );
}

#[test]
fn rollouts_open_outside_the_root_are_ignored() {
    let f = Fixture::new();
    let other = Fixture::new();
    let p = other.rollout(ROOT_T);
    write(&p, &[meta(ROOT_T, ROOT_T), task(T1, "task_started")]);
    let mut a = f.adapter();
    f.hold(42, &[&p]);
    a.poll(&HashMap::new(), 0).unwrap();
    assert!(a.live_sessions().unwrap().is_empty());
}

#[test]
fn status_follows_tasks_of_open_rollouts() {
    let f = Fixture::new();
    let p = f.rollout(ROOT_T);
    write(&p, &[meta(ROOT_T, ROOT_T), task(T1, "task_started")]);
    let mut a = f.adapter();
    f.hold(42, &[&p]);
    let state = |a: &mut CodexAdapter, off: &mut HashMap<String, FileCursor>| {
        off.extend(offsets(&a.poll(off, 0).unwrap()));
        let live = a.live_sessions().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(
            (live[0].pid, live[0].proc_start_s),
            (42, Some(1_794_535_000))
        );
        assert!(!live[0].shared_process);
        live[0].state
    };
    let mut off = HashMap::new();
    assert_eq!(state(&mut a, &mut off), LiveState::Busy);
    append(&p, &[task(T2, "task_complete")]);
    assert_eq!(state(&mut a, &mut off), LiveState::Idle);
    append(&p, &[task(T3, "task_started")]);
    assert_eq!(state(&mut a, &mut off), LiveState::Busy);
    append(&p, &[task(T4, "turn_aborted")]);
    assert_eq!(state(&mut a, &mut off), LiveState::Idle);
    // Closed rollout: no longer live after the next poll.
    f.open.lock().unwrap().clear();
    a.poll(&off, 0).unwrap();
    assert!(a.live_sessions().unwrap().is_empty());
}

#[test]
fn a_process_hosting_several_sessions_is_shared() {
    let f = Fixture::new();
    let (root, sub, other) = (f.rollout(ROOT_T), f.rollout(SUB_T), f.rollout(OTHER_T));
    let now = recent();
    write(&root, &[meta_at(&now, ROOT_T, ROOT_T)]);
    write(&sub, &[meta(SUB_T, ROOT_T), task(T1, "task_started")]);
    write(&other, &[meta_at(&now, OTHER_T, OTHER_T)]);
    // One process with a root thread and its subagent: one session, attached.
    f.hold(10, &[&root, &sub]);
    let mut a = f.adapter();
    a.poll(&HashMap::new(), 0).unwrap();
    let live = a.live_sessions().unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(
        (live[0].session_id.as_str(), live[0].shared_process),
        (ROOT_T, false)
    );
    assert_eq!(
        live[0].state,
        LiveState::Busy,
        "a busy subagent makes the session busy"
    );
    // An app-server holding two sessions: both live, neither attached.
    f.open.lock().unwrap().clear();
    f.hold(20, &[&root, &other]);
    a.poll(&HashMap::new(), 0).unwrap();
    let live = a.live_sessions().unwrap();
    assert_eq!(live.len(), 2);
    assert!(live.iter().all(|l| l.shared_process && l.pid == 20));
    // The same session also held alone by another process: that one wins.
    f.hold(30, &[&other]);
    a.poll(&HashMap::new(), 0).unwrap();
    let live = a.live_sessions().unwrap();
    let o = live.iter().find(|l| l.session_id == OTHER_T).unwrap();
    assert_eq!((o.pid, o.shared_process), (30, false));
    // Busy in a shared process, idle in one holding it alone: the lone holder
    // wins and stays Busy.
    f.open.lock().unwrap().clear();
    f.hold(20, &[&root, &sub, &other]);
    f.hold(40, &[&root]);
    a.poll(&HashMap::new(), 0).unwrap();
    let live = a.live_sessions().unwrap();
    let r = live.iter().find(|l| l.session_id == ROOT_T).unwrap();
    assert_eq!((r.pid, r.shared_process), (40, false));
    assert_eq!(r.state, LiveState::Busy);
}

#[test]
fn old_threads_in_a_shared_process_are_not_live() {
    let f = Fixture::new();
    let (root, sub, other) = (f.rollout(ROOT_T), f.rollout(SUB_T), f.rollout(OTHER_T));
    let old = "2026-01-01T00:00:00.000Z";
    // ROOT_T's own rollout is old, but its subagent was active minutes ago.
    write(&root, &[meta_at(old, ROOT_T, ROOT_T)]);
    write(&sub, &[meta_at(old, SUB_T, ROOT_T), content(&recent())]);
    write(&other, &[meta_at(old, OTHER_T, OTHER_T)]);
    f.hold(20, &[&root, &sub, &other]);
    let mut a = f.adapter();
    let off = offsets(&a.poll(&HashMap::new(), 0).unwrap());
    let ids = |a: &CodexAdapter| -> Vec<String> {
        a.live_sessions()
            .unwrap()
            .into_iter()
            .map(|l| l.session_id)
            .collect()
    };
    assert_eq!(ids(&a), [ROOT_T]);
    // After a restart the activity is learned from the lines already read.
    let mut a = f.adapter();
    a.poll(&off, 0).unwrap();
    assert_eq!(ids(&a), [ROOT_T]);
    // Held alone, an old thread stays live.
    f.hold(30, &[&other]);
    a.poll(&off, 0).unwrap();
    assert_eq!(ids(&a), [ROOT_T, OTHER_T]);
}

#[test]
fn open_old_rollout_is_read_and_closed_one_is_not() {
    let f = Fixture::new();
    let (live, gone) = (f.rollout(ROOT_T), f.rollout(OTHER_T));
    write(
        &live,
        &[
            meta(ROOT_T, ROOT_T),
            usage(T2, "resp_live", ROOT_T, ROOT_T, "t", 5, 0, 1),
        ],
    );
    write(
        &gone,
        &[
            meta(OTHER_T, OTHER_T),
            usage(T2, "resp_gone", OTHER_T, OTHER_T, "t", 5, 0, 1),
        ],
    );
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    for p in [&live, &gone] {
        fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    f.hold(5, &[&live]);
    let b = f.adapter().poll(&HashMap::new(), 2_000_000_000).unwrap();
    let ids: Vec<_> = b.events.iter().map(|e| e.event_id.as_str()).collect();
    assert_eq!(ids, ["resp_live"]);
}

#[test]
fn moving_a_rollout_to_the_archive_rereads_the_same_events() {
    let f = Fixture::new();
    let p = f.rollout(ROOT_T);
    write(
        &p,
        &[
            meta(ROOT_T, ROOT_T),
            turn(T1, "turn-1", "gpt-6.1-sol"),
            usage(T2, "resp_1", ROOT_T, ROOT_T, "turn-1", 30, 10, 3),
            usage(T3, "resp_2", ROOT_T, ROOT_T, "turn-1", 40, 20, 4),
        ],
    );
    let mut a = f.adapter();
    let b1 = a.poll(&HashMap::new(), 0).unwrap();
    let archived = f.root.join(format!(
        "archived_sessions/rollout-2026-11-13T02-00-00-{ROOT_T}.jsonl"
    ));
    fs::create_dir_all(archived.parent().unwrap()).unwrap();
    fs::rename(&p, &archived).unwrap();
    let mut a = f.adapter();
    let b2 = a.poll(&offsets(&b1), 0).unwrap();
    assert_eq!(b2.files.len(), 1);
    assert_eq!(b2.files[0].path, archived.to_string_lossy());
    assert_eq!(
        b1.events, b2.events,
        "same ids, sessions and usage: upserts are no-ops"
    );
}

#[test]
fn content_records_are_not_parsed() {
    let f = Fixture::new();
    write(
        &f.rollout(ROOT_T),
        &[
            meta(ROOT_T, ROOT_T),
            content(T2),
            token_count(T3),
            rec(
                T4,
                "response_item",
                r#"{"type":"function_call_output","response_id":"resp_fake","usage":{"input_tokens":5}}"#,
            ),
        ],
    );
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert!(b.events.is_empty());
    // Their leading timestamps still count as activity.
    assert_eq!(session(&b, ROOT_T).timestamps_ms.len(), 4);
}

#[test]
fn hot_rollout_append_is_picked_up_without_rescan() {
    let f = Fixture::new();
    let p = f.rollout(ROOT_T);
    write(
        &p,
        &[
            meta(ROOT_T, ROOT_T),
            usage(T2, "resp_1", ROOT_T, ROOT_T, "t", 5, 0, 1),
        ],
    );
    let mut a = f.adapter();
    let b = a.poll(&HashMap::new(), 0).unwrap();
    append(&p, &[usage(T3, "resp_2", ROOT_T, ROOT_T, "t", 6, 0, 1)]);
    let b2 = a.poll(&offsets(&b), 0).unwrap();
    assert_eq!(b2.events.len(), 1);
    assert_eq!(b2.events[0].event_id, "resp_2");
    assert_eq!(b2.events[0].session_id, ROOT_T);
}

#[test]
fn backfill_stops_at_the_read_budget() {
    let f = Fixture::new();
    for t in [ROOT_T, SUB_T, OTHER_T] {
        write(
            &f.rollout(t),
            &[
                meta(t, t),
                usage(T2, &format!("resp_{t}"), t, t, "x", 5, 0, 1),
            ],
        );
    }
    let mut a = f.adapter().with_read_budget(1);
    let (mut off, mut events, mut polls) = (HashMap::new(), 0, 0);
    loop {
        let b = a.poll(&off, 0).unwrap();
        polls += 1;
        events += b.events.len();
        off.extend(offsets(&b));
        if !b.more {
            break;
        }
    }
    assert_eq!((events, polls), (3, 4));
}

#[test]
fn plan_limits_take_the_newest_reading_and_survive_a_restart() {
    let f = Fixture::new();
    let (root, other) = (f.rollout(ROOT_T), f.rollout(OTHER_T));
    write(
        &root,
        &[
            meta(ROOT_T, ROOT_T),
            limits(T3, 62.0, "null"),
            // Written out of order: the earlier record does not win.
            limits(T2, 50.0, "null"),
        ],
    );
    write(
        &other,
        &[
            meta(OTHER_T, OTHER_T),
            limits(
                T1,
                10.0,
                r#"{"used_percent":3.5,"window_minutes":300,"resets_at":1795000000}"#,
            ),
        ],
    );
    let mut a = f.adapter();
    let b = a.poll(&HashMap::new(), 0).unwrap();
    let l = b.rate_limits.clone().unwrap();
    assert_eq!(l.ts_ms, 1_794_535_203_000);
    assert_eq!(l.plan_type.as_deref(), Some("prolite"));
    let p = l.primary.unwrap();
    assert_eq!(
        (p.used_percent, p.window_minutes, p.resets_at_s),
        (62.0, Some(10_080), Some(1_795_138_706))
    );
    assert_eq!(l.secondary, None);
    // The cumulative token figures beside the limits are never read as usage.
    assert!(b.events.is_empty());

    // A newer secondary window is read too.
    append(
        &other,
        &[limits(
            T4,
            70.0,
            r#"{"used_percent":3.5,"window_minutes":300,"resets_at":1795000000}"#,
        )],
    );
    let l = a.poll(&offsets(&b), 0).unwrap().rate_limits.unwrap();
    assert_eq!(l.primary.unwrap().used_percent, 70.0);
    assert_eq!(l.secondary.unwrap().window_minutes, Some(300));

    // After a restart, an open rollout read to its end is seeded, limits included.
    let mut stored = offsets(&b);
    stored.insert(
        other.to_string_lossy().into_owned(),
        FileCursor {
            path: other.to_string_lossy().into_owned(),
            offset: fs::metadata(&other).unwrap().len(),
            identity: Some(agentpulse_core::FileIdentity {
                birth: fs::metadata(&other)
                    .unwrap()
                    .created()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64),
                ino: std::os::unix::fs::MetadataExt::ino(&fs::metadata(&other).unwrap()),
            }),
        },
    );
    f.hold(3, &[&other]);
    let mut a = f.adapter();
    let b2 = a.poll(&stored, 0).unwrap();
    assert!(b2.is_empty());
    assert_eq!(b2.rate_limits.unwrap().primary.unwrap().used_percent, 70.0);
}

#[test]
fn malformed_rate_limits_never_drop_the_line() {
    let f = Fixture::new();
    let bad_limits = rec(
        T3,
        "event_msg",
        r#"{"type":"token_count","rate_limits":{"primary":{"used_percent":"high"},"plan_type":7}}"#,
    );
    let bad_usage = usage(T4, "resp_1", ROOT_T, ROOT_T, "turn-1", 100, 0, 5)
        .replace(r#""usage":"#, r#""rate_limits":["unexpected"],"usage":"#);
    write(
        &f.rollout(ROOT_T),
        &[
            meta(ROOT_T, ROOT_T),
            turn(T1, "turn-1", "gpt-6.1-sol"),
            limits(T2, 40.0, "null"),
            bad_limits,
            bad_usage,
        ],
    );
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    // The usage record still counts.
    assert_eq!(b.events.len(), 1);
    assert_eq!(b.events[0].usage.output, 5);
    // Both lines' timestamps still count as activity.
    let ts = &session(&b, ROOT_T).timestamps_ms;
    assert!(ts.contains(&1_794_535_203_000) && ts.contains(&1_794_535_204_000));
    // The last well-formed reading stands.
    let l = b.rate_limits.unwrap();
    assert_eq!(
        (l.ts_ms, l.primary.unwrap().used_percent),
        (1_794_535_202_000, 40.0)
    );
}

#[test]
fn replacement_rebuilds_rollout_state() {
    for restart in [false, true] {
        let f = Fixture::new();
        let path = f.rollout(ROOT_T);
        write(
            &path,
            &[
                meta(ROOT_T, "s-old"),
                turn(T1, "turn-1", "model-old"),
                usage(T2, "resp-1", ROOT_T, "s-old", "turn-1", 10, 0, 2),
            ],
        );
        f.hold(1, &[&path]);
        let mut a = f.adapter();
        let off = offsets(&a.poll(&HashMap::new(), 0).unwrap());
        let replacement = path.with_extension("replacement");
        write(
            &replacement,
            &[
                meta(ROOT_T, "s-new"),
                turn(T1, "turn-1", "model-new"),
                usage(T2, "resp-2", ROOT_T, "s-new", "turn-1", 10, 0, 2),
            ],
        );
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            fs::metadata(&replacement).unwrap().len()
        );
        fs::rename(replacement, &path).unwrap();
        if restart {
            a = f.adapter();
        }
        let b = a.poll(&off, 0).unwrap();
        assert_eq!(b.events.len(), 1);
        assert_eq!(b.events[0].session_id, "s-new");
        assert_eq!(b.events[0].model, "model-new");
        assert!(a.poll(&offsets(&b), 0).unwrap().is_empty());
    }
}

// ruzstd's encoder keeps these fixtures synthetic and independent of a system CLI.
fn compress(path: &Path) -> PathBuf {
    let archive = path.with_extension("jsonl.zst");
    let bytes = fs::read(path).unwrap();
    fs::write(
        &archive,
        ruzstd::encoding::compress_to_vec(&bytes[..], ruzstd::encoding::CompressionLevel::Fastest),
    )
    .unwrap();
    archive
}

fn archive_lines() -> Vec<String> {
    vec![
        meta(ROOT_T, ROOT_T),
        turn(T1, "turn-1", "gpt-6.1-sol"),
        usage(T2, "resp_1", ROOT_T, ROOT_T, "turn-1", 30, 10, 3),
    ]
}

#[test]
fn archive_forms_and_twins_choose_one_rollout() {
    for mode in [
        "plain",
        "compressed",
        "twins",
        "newer-plain",
        "empty-compressed",
        "empty-newer-plain",
    ] {
        let f = Fixture::new();
        let plain = f.root.join(format!(
            "archived_sessions/rollout-2026-11-13T02-00-00-{ROOT_T}.jsonl"
        ));
        write(&plain, &archive_lines());
        let archive = compress(&plain);
        let equal_time = fs::metadata(&plain).unwrap().modified().unwrap();
        fs::File::options()
            .write(true)
            .open(&archive)
            .unwrap()
            .set_modified(equal_time)
            .unwrap();
        match mode {
            "plain" => fs::remove_file(&archive).unwrap(),
            "compressed" => fs::remove_file(&plain).unwrap(),
            "newer-plain" => {
                append(
                    &plain,
                    &[usage(T3, "resp_2", ROOT_T, ROOT_T, "turn-1", 40, 20, 4)],
                );
                fs::File::options()
                    .write(true)
                    .open(&plain)
                    .unwrap()
                    .set_modified(equal_time + std::time::Duration::from_secs(1))
                    .unwrap();
            }
            "empty-newer-plain" => {
                fs::write(&plain, []).unwrap();
                fs::File::open(&plain)
                    .unwrap()
                    .set_modified(equal_time + std::time::Duration::from_secs(1))
                    .unwrap();
            }
            "empty-compressed" => fs::write(&archive, []).unwrap(),
            _ => {}
        }
        let batch = f.adapter().poll(&HashMap::new(), 0).unwrap();
        assert_eq!(batch.files.len(), 1, "{mode}");
        let compressed = matches!(mode, "compressed" | "twins" | "empty-newer-plain");
        let selected = if compressed { &archive } else { &plain };
        assert_eq!(batch.files[0].path, selected.to_string_lossy(), "{mode}");
        assert_eq!(batch.files[0].offset, fs::metadata(selected).unwrap().len());
        assert_eq!(
            batch.events.len(),
            if mode == "newer-plain" { 2 } else { 1 }
        );
        assert_eq!(batch.events[0].usage.total(), 33);
        assert_eq!(batch.events[0].model, "gpt-6.1-sol");
        assert_eq!(batch.events[0].session_id, ROOT_T);
        assert_eq!(
            batch.events.iter().map(|e| e.usage.total()).sum::<u64>(),
            if mode == "newer-plain" { 77 } else { 33 }
        );
    }
}

#[test]
fn compressed_archive_skips_unchanged_and_rebuilds_on_size_change() {
    let f = Fixture::new();
    let plain = f.rollout(ROOT_T);
    write(&plain, &archive_lines());
    let archive = compress(&plain);
    fs::remove_file(&plain).unwrap();
    let mut adapter = f.adapter();
    let first = adapter.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(first.events.len(), 1);
    let saved = offsets(&first);
    // Same inode and size, but no longer decodable: any reopen would fail.
    let size = fs::metadata(&archive).unwrap().len() as usize;
    fs::write(&archive, vec![0; size]).unwrap();
    assert!(adapter.poll(&saved, 0).unwrap().is_empty());
    assert!(f.adapter().poll(&saved, 0).unwrap().is_empty());
    write(&plain, &archive_lines());
    append(
        &plain,
        &[usage(T3, "resp_2", ROOT_T, ROOT_T, "turn-1", 40, 20, 4)],
    );
    compress(&plain);
    fs::remove_file(&plain).unwrap();
    let changed = adapter.poll(&saved, 0).unwrap();
    assert_eq!(changed.events.len(), 2);
    assert_eq!(changed.events[0].model, "gpt-6.1-sol");
    assert_eq!(
        changed.files[0].offset,
        fs::metadata(&archive).unwrap().len()
    );
}

#[test]
fn switching_archive_forms_through_storage_is_idempotent() {
    use agentpulse_storage::Store;
    for compressed_first in [false, true] {
        let f = Fixture::new();
        let plain = f.rollout(ROOT_T);
        write(&plain, &archive_lines());
        let archive = compress(&plain);
        let compressed_bytes = fs::read(&archive).unwrap();
        fs::remove_file(if compressed_first { &plain } else { &archive }).unwrap();
        let mut store = Store::open(&f.root.join("usage.db")).unwrap();
        let price = |_: &agentpulse_core::UsageEvent| Some(0.5);
        let first = f.adapter().poll(&HashMap::new(), 0).unwrap();
        assert_eq!(first.events.len(), 1);
        store.apply(&first, &price).unwrap();
        let before = store.sessions_since(Provider::Codex, 0).unwrap();
        if compressed_first {
            fs::remove_file(&archive).unwrap();
            write(&plain, &archive_lines());
        } else {
            fs::remove_file(&plain).unwrap();
            fs::write(&archive, compressed_bytes).unwrap();
        }
        let replay = f.adapter().poll(&store.offsets().unwrap(), 0).unwrap();
        assert_eq!(replay.events, first.events);
        store.apply(&replay, &price).unwrap();
        assert_eq!(store.sessions_since(Provider::Codex, 0).unwrap(), before);
        assert!(
            f.adapter()
                .poll(&store.offsets().unwrap(), 0)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn compressed_partial_line_is_ignored_and_corruption_keeps_cursor() {
    let f = Fixture::new();
    let plain = f.rollout(ROOT_T);
    write(&plain, &archive_lines());
    OpenOptions::new()
        .append(true)
        .open(&plain)
        .unwrap()
        .write_all(usage(T3, "partial", ROOT_T, ROOT_T, "turn-1", 5, 0, 1).as_bytes())
        .unwrap();
    let archive = compress(&plain);
    fs::remove_file(&plain).unwrap();
    let first = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(first.events.len(), 1);
    assert_eq!(first.files[0].offset, fs::metadata(&archive).unwrap().len());
    fs::write(&archive, b"corrupt").unwrap();
    let failed = f.adapter().poll(&offsets(&first), 0).unwrap();
    assert!(failed.files.is_empty());
}

#[test]
fn compressed_checksum_corruption_keeps_cursor() {
    let f = Fixture::new();
    let plain = f.rollout(ROOT_T);
    write(&plain, &archive_lines());
    let archive = compress(&plain);
    fs::remove_file(&plain).unwrap();
    let mut bytes = fs::read(&archive).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&archive, bytes).unwrap();
    let batch = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert!(batch.files.is_empty());
    assert!(batch.events.is_empty());
    assert!(batch.sessions.is_empty());
}

#[test]
fn compressed_multiple_frames_and_truncated_second_frame() {
    let f = Fixture::new();
    let plain = f.rollout(ROOT_T);
    write(&plain, &archive_lines());
    let archive = compress(&plain);
    let mut bytes = fs::read(&archive).unwrap();
    write(
        &plain,
        &[usage(T3, "resp_2", ROOT_T, ROOT_T, "turn-1", 40, 20, 4)],
    );
    compress(&plain);
    bytes.extend(fs::read(&archive).unwrap());
    fs::remove_file(&plain).unwrap();
    fs::write(&archive, &bytes).unwrap();
    let batch = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(batch.events.len(), 2);
    bytes.truncate(bytes.len() - 5);
    fs::write(&archive, bytes).unwrap();
    let batch = f.adapter().poll(&offsets(&batch), 0).unwrap();
    assert!(batch.events.is_empty());
    assert!(batch.sessions.is_empty());
    assert!(batch.files.is_empty());
}

#[test]
fn compressed_line_split_across_frames_is_read() {
    let f = Fixture::new();
    let plain = f.rollout(ROOT_T);
    write(&plain, &archive_lines());
    let text = fs::read(&plain).unwrap();
    // Cut mid-way through the usage line, as parallel compressors may.
    let cut = text.len() - 20;
    let frame = |b: &[u8]| {
        ruzstd::encoding::compress_to_vec(b, ruzstd::encoding::CompressionLevel::Fastest)
    };
    let mut bytes = frame(&text[..cut]);
    bytes.extend(frame(&text[cut..]));
    fs::remove_file(&plain).unwrap();
    fs::write(plain.with_extension("jsonl.zst"), bytes).unwrap();
    let batch = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.files.len(), 1);
}

#[test]
fn old_archive_without_cursor_is_not_read() {
    let f = Fixture::new();
    let plain = f.rollout(ROOT_T);
    write(&plain, &archive_lines());
    let archive = compress(&plain);
    fs::remove_file(&plain).unwrap();
    fs::File::open(&archive)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
        .unwrap();
    let mut adapter = f.adapter();
    assert!(adapter.poll(&HashMap::new(), 2000).unwrap().is_empty());
}

#[test]
fn open_archive_does_not_make_a_thread_live_or_bypass_since() {
    let f = Fixture::new();
    let plain = f.rollout(ROOT_T);
    write(&plain, &archive_lines());
    let archive = compress(&plain);
    fs::remove_file(&plain).unwrap();
    fs::File::open(&archive)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
        .unwrap();
    f.hold(42, &[&archive]);
    let mut adapter = f.adapter();
    assert!(adapter.poll(&HashMap::new(), 2000).unwrap().is_empty());
    assert!(adapter.live_sessions().unwrap().is_empty());
}

#[test]
fn broken_archive_is_skipped_until_size_or_identity_changes() {
    for change_identity in [false, true] {
        let f = Fixture::new();
        let plain = f.rollout(ROOT_T);
        write(&plain, &archive_lines());
        let archive = compress(&plain);
        let good = fs::read(&archive).unwrap();
        fs::remove_file(&plain).unwrap();
        fs::write(&archive, vec![0; good.len()]).unwrap();
        let mut adapter = f.adapter().with_rescan_interval(std::time::Duration::ZERO);
        assert!(adapter.poll(&HashMap::new(), 0).unwrap().is_empty());
        // Same identity and size: even replacing the bytes with a valid frame must
        // not cause another decode on the next poll.
        fs::write(&archive, &good).unwrap();
        assert!(adapter.poll(&HashMap::new(), 0).unwrap().is_empty());
        if change_identity {
            let replacement = archive.with_extension("replacement");
            fs::write(&replacement, &good).unwrap();
            fs::rename(replacement, &archive).unwrap();
        } else {
            OpenOptions::new()
                .append(true)
                .open(&archive)
                .unwrap()
                .write_all(&good)
                .unwrap();
        }
        assert_eq!(adapter.poll(&HashMap::new(), 0).unwrap().events.len(), 1);
    }
}
