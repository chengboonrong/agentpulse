//! All fixtures are synthetic: invented ids, paths, models and token counts.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentpulse_core::{AgentAdapter, LiveState, PollBatch, Provider, TokenUsage, UsageEvent};
use agentpulse_process::OpenFiles;
use agentpulse_telemetry::GrokAdapter;

const SID: &str = "01a1d096-0000-7000-8000-000000000001";
const OTHER: &str = "01a1d096-0000-7000-8000-000000000002";
/// `/synthetic/my proj`, encoded as Grok names the folder.
const CWD_DIR: &str = "%2Fsynthetic%2Fmy%20proj";
const T1: &str = "2026-11-17T09:00:01.000Z";
const T2: &str = "2026-11-17T09:00:02.000Z";
const T3: &str = "2026-11-17T09:00:03.000Z";
const T4: &str = "2026-11-17T09:00:04.000Z";

fn event(ts: &str, kind: &str) -> String {
    format!(
        r#"{{"ts":"{ts}","type":"{kind}","tool_name":"SYNTHETIC","error_message":"SYNTHETIC"}}"#
    )
}

/// (model, inputTokens, cachedReadTokens, outputTokens, costUsdTicks)
type Model<'a> = (&'a str, u64, u64, u64, Option<u64>);

fn turn(n: u64, ended: &str, models: &[Model]) -> String {
    let usage: Vec<String> = models
        .iter()
        .map(|(m, input, cached, output, ticks)| {
            let ticks = ticks.map_or(String::new(), |t| format!(r#","costUsdTicks":{t}"#));
            format!(
                r#""{m}":{{"inputTokens":{input},"outputTokens":{output},"cachedReadTokens":{cached},"cacheCreationTokens":0,"reasoningTokens":1,"totalTokens":{},"modelCalls":2{ticks}}}"#,
                input + output
            )
        })
        .collect();
    format!(
        r#"{{"turnNumber":{n},"endedAt":"{ended}","primaryModelId":"{}","inputTokens":1,"modelUsage":{{{}}}}}"#,
        models[0].0,
        usage.join(",")
    )
}

fn usage_json(turns: &[String]) -> String {
    format!(
        r#"{{"sessionId":"{SID}","updatedAt":"{T4}","session":{{"inputTokens":999999,"primaryModelId":"x"}},"turns":[{}]}}"#,
        turns.join(",")
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

    fn adapter(&self) -> GrokAdapter {
        let open = self.open.clone();
        GrokAdapter::new(self.root.clone()).with_open_files(move || open.lock().unwrap().clone())
    }

    fn dir(&self, sid: &str) -> PathBuf {
        self.root.join("sessions").join(CWD_DIR).join(sid)
    }

    fn events(&self, sid: &str) -> PathBuf {
        self.dir(sid).join("events.jsonl")
    }

    fn usage(&self, sid: &str, turns: &[String]) -> PathBuf {
        let p = self.dir(sid).join("usage.json");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, usage_json(turns)).unwrap();
        p
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

fn offsets(b: &PollBatch) -> HashMap<String, u64> {
    b.files.iter().map(|f| (f.path.clone(), f.offset)).collect()
}

fn session<'a>(b: &'a PollBatch, sid: &str) -> &'a agentpulse_core::SessionDelta {
    b.sessions.iter().find(|s| s.session_id == sid).unwrap()
}

fn ids(events: &[UsageEvent]) -> Vec<&str> {
    events.iter().map(|e| e.event_id.as_str()).collect()
}

#[test]
fn missing_root_is_empty_not_an_error() {
    let f = Fixture::new();
    let mut a = GrokAdapter::new(f.root.join("no-grok-here")).with_open_files(Vec::new);
    assert_eq!(a.provider(), Provider::Grok);
    assert!(a.poll(&HashMap::new(), 0).unwrap().is_empty());
    assert!(a.live_sessions().unwrap().is_empty());
    // A root without `sessions/` is empty too.
    let mut a = f.adapter();
    assert!(a.poll(&HashMap::new(), 0).unwrap().is_empty());
}

#[test]
fn usage_maps_buckets_and_grok_cost() {
    let f = Fixture::new();
    f.usage(
        SID,
        &[turn(
            1,
            T2,
            &[("grok-test-build", 2_000, 1_500, 300, Some(15_509_660_000))],
        )],
    );
    write(
        &f.events(SID),
        &[event(T1, "turn_started"), event(T2, "turn_ended")],
    );
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.provider, Provider::Grok);
    assert_eq!(b.events.len(), 1);
    let e = &b.events[0];
    assert_eq!(e.event_id, format!("{SID}:1:grok-test-build"));
    assert_eq!(e.session_id, SID);
    assert_eq!(e.model, "grok-test-build");
    assert_eq!(e.ts_ms, 1_794_906_002_000);
    // inputTokens includes the cached tokens; outputTokens includes reasoning.
    assert_eq!(
        e.usage,
        TokenUsage {
            input: 500,
            cache_read: 1_500,
            output: 300,
            ..Default::default()
        }
    );
    // 1e10 ticks per US dollar.
    assert!((e.recorded_usd.unwrap() - 1.550_966).abs() < 1e-12);
    let s = session(&b, SID);
    assert_eq!(s.project_path.as_deref(), Some("/synthetic/my proj"));
    assert_eq!(s.model.as_deref(), Some("grok-test-build"));
    // Two event lines, plus the turn's end from usage.json.
    assert_eq!(s.timestamps_ms.len(), 3);
}

#[test]
fn a_turn_with_two_models_is_one_event_per_model() {
    let f = Fixture::new();
    f.usage(
        SID,
        &[
            turn(1, T1, &[("grok-a", 10, 0, 1, Some(10))]),
            turn(
                2,
                T3,
                &[
                    ("grok-b", 20, 5, 2, Some(20)),
                    ("grok-a", 30, 0, 3, None),
                    ("grok-empty", 0, 0, 0, Some(0)),
                ],
            ),
        ],
    );
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(
        ids(&b.events),
        [
            format!("{SID}:1:grok-a"),
            format!("{SID}:2:grok-a"),
            format!("{SID}:2:grok-b"),
        ]
    );
    // Without a recorded charge the price table decides.
    assert_eq!(b.events[1].recorded_usd, None);
    assert_eq!(b.events[2].recorded_usd, Some(20.0 / 1e10));
    // The newest turn's primary model.
    assert_eq!(session(&b, SID).model.as_deref(), Some("grok-b"));
    // The same file read again gives the same ids.
    let again = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(again.events, b.events);
}

#[test]
fn rereads_are_idempotent() {
    let f = Fixture::new();
    let t1 = turn(1, T1, &[("grok-a", 100, 40, 10, Some(1_000))]);
    f.usage(SID, std::slice::from_ref(&t1));
    write(&f.events(SID), &[event(T1, "turn_started")]);
    let b1 = f.adapter().poll(&HashMap::new(), 0).unwrap();
    let off = offsets(&b1);
    assert_eq!(off.len(), 2);
    // A restart at the stored offsets: unchanged files yield nothing.
    let b2 = f.adapter().poll(&off, 0).unwrap();
    assert!(b2.is_empty());
    // A new turn rewrites usage.json: the old turn comes back unchanged, the
    // new one is new.
    f.usage(SID, &[t1, turn(2, T3, &[("grok-a", 50, 0, 5, Some(500))])]);
    let b3 = f.adapter().poll(&off, 0).unwrap();
    assert_eq!(b3.events.len(), 2);
    assert_eq!(b3.events[0], b1.events[0]);
    assert_eq!(b3.events[1].event_id, format!("{SID}:2:grok-a"));
}

#[test]
fn half_written_usage_is_retried() {
    let f = Fixture::new();
    let full = usage_json(&[turn(1, T1, &[("grok-a", 10, 0, 1, Some(1))])]);
    let p = f.dir(SID).join("usage.json");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, &full[..full.len() / 2]).unwrap();
    let mut a = f.adapter();
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert!(b.events.is_empty());
    assert!(b.files.is_empty(), "cursor not advanced");
    fs::write(&p, &full).unwrap();
    let b = a.poll(&offsets(&b), 0).unwrap();
    assert_eq!(b.events.len(), 1);
    assert_eq!(b.files[0].offset, full.len() as u64);
}

#[test]
fn status_follows_turns_and_permissions() {
    let f = Fixture::new();
    let p = f.events(SID);
    write(
        &p,
        &[event(T1, "mcp_init_completed"), event(T1, "turn_started")],
    );
    let mut a = f.adapter();
    f.hold(42, &[&p]);
    let state = |a: &mut GrokAdapter, off: &mut HashMap<String, u64>| {
        off.extend(offsets(&a.poll(off, 0).unwrap()));
        let live = a.live_sessions().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].session_id, SID);
        assert_eq!(
            (live[0].pid, live[0].proc_start_s),
            (42, Some(1_794_535_000))
        );
        assert!(!live[0].shared_process);
        assert_eq!(live[0].project_path.as_deref(), Some("/synthetic/my proj"));
        live[0].state
    };
    let mut off = HashMap::new();
    assert_eq!(state(&mut a, &mut off), LiveState::Busy);
    append(&p, &[event(T2, "permission_requested")]);
    assert_eq!(state(&mut a, &mut off), LiveState::Waiting);
    append(
        &p,
        &[event(T2, "permission_resolved"), event(T2, "phase_changed")],
    );
    assert_eq!(state(&mut a, &mut off), LiveState::Busy);
    // A request made just now (an auto-approval in flight) is not Waiting yet.
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    append(&p, &[event(&now, "permission_requested")]);
    assert_eq!(state(&mut a, &mut off), LiveState::Busy);
    append(&p, &[event(&now, "permission_resolved")]);
    assert_eq!(state(&mut a, &mut off), LiveState::Busy);
    append(&p, &[event(T3, "turn_ended")]);
    assert_eq!(state(&mut a, &mut off), LiveState::Idle);
    // Closed event log: no longer live after the next poll.
    f.open.lock().unwrap().clear();
    a.poll(&off, 0).unwrap();
    assert!(a.live_sessions().unwrap().is_empty());
}

#[test]
fn resuming_from_an_offset_seeds_state_from_earlier_lines() {
    let f = Fixture::new();
    let p = f.events(SID);
    write(
        &p,
        &[event(T1, "turn_started"), event(T2, "permission_requested")],
    );
    let b1 = f.adapter().poll(&HashMap::new(), 0).unwrap();
    // A fresh adapter (an app restart) resumes at the stored offset.
    let mut a = f.adapter();
    f.hold(7, &[&p]);
    // Read to the end before the restart: state still learned.
    assert!(a.poll(&offsets(&b1), 0).unwrap().is_empty());
    let live = a.live_sessions().unwrap();
    assert_eq!(live[0].state, LiveState::Waiting);
    assert_eq!(live[0].started_at_ms, Some(1_794_906_001_000));
    // Appended after the restart, read mid-file by another fresh adapter
    // while the log is not seen open.
    append(&p, &[event(T3, "permission_resolved")]);
    f.open.lock().unwrap().clear();
    let mut a = f.adapter();
    let mut off = offsets(&b1);
    let b2 = a.poll(&off, 0).unwrap();
    assert_eq!(session(&b2, SID).timestamps_ms, [1_794_906_003_000]);
    off.extend(offsets(&b2));
    f.hold(7, &[&p]);
    assert!(a.poll(&off, 0).unwrap().is_empty());
    let live = a.live_sessions().unwrap();
    assert_eq!(live[0].state, LiveState::Busy, "turn state seeded");
    assert_eq!(live[0].started_at_ms, Some(1_794_906_001_000));
}

#[test]
fn a_process_holding_several_sessions_is_shared() {
    let f = Fixture::new();
    let now = (chrono::Utc::now() - chrono::Duration::minutes(5))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let (a_p, b_p) = (f.events(SID), f.events(OTHER));
    write(&a_p, &[event(&now, "turn_started")]);
    // Old: not live in a shared process.
    write(&b_p, &[event(T1, "turn_started")]);
    let mut a = f.adapter();
    f.hold(9, &[&a_p, &b_p]);
    a.poll(&HashMap::new(), 0).unwrap();
    let live = a.live_sessions().unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session_id, SID);
    assert!(live[0].shared_process);
    assert_eq!(live[0].state, LiveState::Busy);
}

#[test]
fn event_logs_open_outside_the_root_are_ignored() {
    let f = Fixture::new();
    let other = Fixture::new();
    let p = other.events(SID);
    write(&p, &[event(T1, "turn_started")]);
    let mut a = f.adapter();
    f.hold(42, &[&p]);
    a.poll(&HashMap::new(), 0).unwrap();
    assert!(a.live_sessions().unwrap().is_empty());
}

#[test]
fn only_events_and_usage_are_ever_opened() {
    let f = Fixture::new();
    f.usage(SID, &[turn(1, T1, &[("grok-a", 10, 0, 1, Some(1))])]);
    write(&f.events(SID), &[event(T1, "turn_started")]);
    let dir = f.dir(SID);
    let forbidden = [
        "chat_history.jsonl",
        "updates.jsonl",
        "summary.json",
        "system_prompt.txt",
        "prompt_context.json",
        "tool_definitions.json",
    ];
    // Readable, and shaped so that reading any of them would leave a trace:
    // a complete event line (a cursor and a timestamp) that is also valid
    // JSON for the usage parser (a cursor).
    let bait = format!("{}\n", event(T2, "turn_started"));
    for name in forbidden {
        fs::write(dir.join(name), &bait).unwrap();
    }
    // A session folder holding only forbidden files is not a session.
    let only_forbidden = f.dir(OTHER);
    fs::create_dir_all(&only_forbidden).unwrap();
    for name in forbidden {
        fs::write(only_forbidden.join(name), &bait).unwrap();
    }
    // An events.jsonl that is a symlink to the conversation is not followed.
    let third = "01a1d096-0000-7000-8000-000000000003";
    let linked = f.dir(third);
    fs::create_dir_all(&linked).unwrap();
    fs::write(linked.join("chat_history.jsonl"), &bait).unwrap();
    std::os::unix::fs::symlink(
        linked.join("chat_history.jsonl"),
        linked.join("events.jsonl"),
    )
    .unwrap();
    let mut a = f.adapter();
    f.hold(42, &[&f.events(SID)]);
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.files.len(), 2, "{:?}", b.files);
    assert!(
        b.files
            .iter()
            .all(|c| c.path.starts_with(&*dir.to_string_lossy())
                && (c.path.ends_with("/events.jsonl") || c.path.ends_with("/usage.json"))),
        "{:?}",
        b.files
    );
    assert_eq!(b.events.len(), 1);
    assert_eq!(b.sessions.len(), 1);
    // The bait's timestamp (T2) was never read.
    assert!(!session(&b, SID).timestamps_ms.contains(&1_794_906_002_000));
}

#[test]
fn model_shows_before_the_first_turn_ends() {
    let f = Fixture::new();
    let p = f.events(SID);
    write(
        &p,
        &[format!(
            r#"{{"ts":"{T1}","type":"turn_started","model_id":"grok-chosen","turn_number":0}}"#
        )],
    );
    // Rescan every poll, so the usage.json written below is found at once.
    let mut a = f.adapter().with_rescan_interval(std::time::Duration::ZERO);
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(session(&b, SID).model.as_deref(), Some("grok-chosen"));
    // Once usage.json exists, its model is the one reported.
    f.usage(SID, &[turn(1, T2, &[("grok-billed", 10, 0, 1, Some(1))])]);
    append(&p, &[event(T3, "turn_ended")]);
    let b = a.poll(&offsets(&b), 0).unwrap();
    assert_eq!(session(&b, SID).model.as_deref(), Some("grok-billed"));
}

#[test]
fn a_session_without_an_event_log_still_has_activity() {
    let f = Fixture::new();
    f.usage(SID, &[turn(1, T2, &[("grok-a", 10, 0, 1, Some(1))])]);
    let b = f.adapter().poll(&HashMap::new(), 0).unwrap();
    assert_eq!(session(&b, SID).timestamps_ms, vec![1_794_906_002_000]);
}
