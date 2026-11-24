//! Synthetic metadata only; no real Gemini session content.
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;

use agentpulse_core::{AgentAdapter, FileCursor, PollBatch, Provider, TokenUsage};
use agentpulse_telemetry::GeminiAdapter;

const META: &str = r#"{"sessionId":"session-a","startTime":"2026-11-17T09:00:00Z","kind":"main"}
"#;
const ANSWER: &str = r#"{"id":"answer","timestamp":"2026-11-17T09:00:02Z","type":"gemini","model":"gemini-test","tokens":{"input":2000,"cached":1500,"output":300,"thoughts":50,"tool":100,"total":2450}}
"#;
fn offsets(b: &PollBatch) -> HashMap<String, FileCursor> {
    b.files
        .iter()
        .map(|f| (f.path.clone(), f.clone()))
        .collect()
}
fn setup() -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let chats = d.path().join("tmp/project/chats");
    fs::create_dir_all(&chats).unwrap();
    fs::write(
        d.path().join("tmp/project/.project_root"),
        " /synthetic/project \n",
    )
    .unwrap();
    (d, chats)
}
#[test]
fn tokens_metadata_duplicates_and_partial_lines() {
    let (d, chats) = setup();
    let path = chats.join("session-file.jsonl");
    let ignored = concat!(
        "{\"$set\":{\"model\":\"ignored\"}}\n",
        "{\"$patch\":{}}\n{\"$rewindTo\":\"answer\"}\n",
        "{\"id\":\"user\",\"timestamp\":\"2026-11-17T09:00:01Z\",\"type\":\"user\"}\n",
        "{\"id\":\"answer\",\"timestamp\":\"2026-11-17T09:00:02Z\",\"type\":\"gemini\",\"tokens\":null}\n",
        "{\"id\":\"info\",\"timestamp\":\"2026-11-17T09:00:03Z\",\"type\":\"info\",\"tokens\":{\"input\":999}}\n",
        "{\"id\":\"error\",\"timestamp\":\"2026-11-17T09:00:04Z\",\"type\":\"error\",\"tokens\":{\"input\":999}}\n"
    );
    let repeated = ANSWER.trim_end().trim_end_matches('}').to_string() + ",\"toolCalls\":[]}\n";
    fs::write(&path, format!("{META}{ignored}{ANSWER}{repeated}{{\"id\":")).unwrap();
    let mut a = GeminiAdapter::new(d.path().into());
    assert_eq!(a.provider(), Provider::Gemini);
    assert!(a.live_sessions().unwrap().is_empty());
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.events.len(), 1);
    let e = &b.events[0];
    assert_eq!(e.event_id, "gemini:session-a:answer");
    assert_eq!(e.session_id, "session-a");
    assert_eq!(e.model, "gemini-test");
    assert_eq!(e.recorded_usd, None);
    assert_eq!(
        e.usage,
        TokenUsage {
            input: 600,
            cache_read: 1500,
            output: 350,
            ..Default::default()
        }
    );
    let s = &b.sessions[0];
    assert_eq!(s.session_id, "session-a");
    assert_eq!(s.project_path.as_deref(), Some("/synthetic/project"));
    assert_eq!(s.model.as_deref(), Some("gemini-test"));
    assert_eq!(s.timestamps_ms.first(), Some(&1_794_906_000_000));
    assert_eq!(s.timestamps_ms.last(), Some(&1_794_906_004_000));
    let off = offsets(&b);
    assert!(a.poll(&off, 0).unwrap().is_empty());
    let mut restarted = GeminiAdapter::new(d.path().into());
    assert!(restarted.poll(&off, 0).unwrap().is_empty());
    OpenOptions::new().append(true).open(&path).unwrap().write_all(b"\"next\",\"timestamp\":\"2026-11-17T09:00:05Z\",\"type\":\"gemini\",\"model\":\"new-model\",\"tokens\":{\"input\":1,\"cached\":20,\"output\":2,\"thoughts\":3}}\n").unwrap();
    let next = restarted.poll(&off, 0).unwrap();
    assert_eq!(next.events.len(), 1);
    assert_eq!(next.events[0].event_id, "gemini:session-a:next");
    assert_eq!(next.events[0].usage.input, 0);
    assert_eq!(next.events[0].usage.output, 5);
    assert_eq!(next.sessions[0].model.as_deref(), Some("new-model"));
    let off = offsets(&next);
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(ANSWER.as_bytes())
        .unwrap();
    assert_eq!(
        restarted.poll(&off, 0).unwrap().events[0].event_id,
        "gemini:session-a:answer"
    );
}
#[test]
fn subagents_and_discovery_boundaries() {
    use std::os::unix::fs::symlink;
    let (d, chats) = setup();
    fs::create_dir_all(chats.join("parent/deeper")).unwrap();
    fs::write(
        chats.join("session-parent-file.jsonl"),
        META.replace("session-a", "parent"),
    )
    .unwrap();
    fs::write(
        chats.join("parent/sub.jsonl"),
        format!(
            "{}{}",
            META.replace("session-a", "child")
                .replace("main", "subagent"),
            ANSWER
        ),
    )
    .unwrap();
    for name in [
        "legacy.json",
        "bad.unreadable-1.jsonl",
        "bad.tmp-1.jsonl",
        "parent/deeper/hidden.jsonl",
    ] {
        fs::write(chats.join(name), format!("{META}{ANSWER}")).unwrap();
    }
    symlink(chats.join("parent/sub.jsonl"), chats.join("link.jsonl")).unwrap();
    symlink(chats.join("parent"), chats.join("linked")).unwrap();
    let mut a = GeminiAdapter::new(d.path().into());
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.files.len(), 2);
    assert_eq!(b.sessions.len(), 1);
    assert_eq!(b.events.len(), 1);
    assert_eq!(b.events[0].session_id, "parent");
    assert_eq!(b.events[0].event_id, "gemini:child:answer");
    assert_eq!(b.sessions[0].session_id, "parent");
    assert_eq!(b.sessions[0].model, None);
    assert_eq!(
        b.sessions[0].project_path.as_deref(),
        Some("/synthetic/project")
    );
    let off = offsets(&b);
    fs::remove_file(chats.join("parent/sub.jsonl")).unwrap();
    symlink(chats.join("legacy.json"), chats.join("parent/sub.jsonl")).unwrap();
    assert!(a.poll(&off, 0).unwrap().is_empty());
}
#[test]
fn fallback_id_and_missing_or_symlinked_project_path() {
    let (d, chats) = setup();
    fs::write(chats.join("fallback.jsonl"), ANSWER).unwrap();
    let project = d.path().join("tmp/project/.project_root");
    fs::remove_file(&project).unwrap();
    for linked in [false, true] {
        if linked {
            let bait = d.path().join("bait");
            fs::write(&bait, "/private/path").unwrap();
            std::os::unix::fs::symlink(bait, &project).unwrap();
        }
        let b = GeminiAdapter::new(d.path().into())
            .poll(&HashMap::new(), 0)
            .unwrap();
        assert_eq!(b.sessions[0].session_id, "fallback");
        assert_eq!(b.sessions[0].project_path, None);
    }
    assert!(
        GeminiAdapter::new(d.path().join("missing"))
            .poll(&HashMap::new(), 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn budgets_since_window_and_replacement() {
    use std::time::Duration;
    let (d, chats) = setup();
    let path = chats.join("a.jsonl");
    fs::write(&path, format!("{META}{ANSWER}")).unwrap();
    fs::write(chats.join("b.jsonl"), format!("{META}{ANSWER}")).unwrap();
    assert!(
        GeminiAdapter::new(d.path().into())
            .poll(&HashMap::new(), i64::MAX)
            .unwrap()
            .is_empty()
    );
    let mut a = GeminiAdapter::new(d.path().into())
        .with_read_budget(1)
        .with_rescan_interval(Duration::ZERO);
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert!(b.more);
    assert_eq!(b.files.len(), 1);
    let next = a.poll(&offsets(&b), 0).unwrap();
    assert_eq!(next.files.len(), 1);
    assert_ne!(next.files[0].path, b.files[0].path);
    let mut saved = offsets(&b);
    saved.extend(offsets(&next));
    let replacement = chats.join("replacement");
    fs::write(&replacement, format!("{META}{ANSWER}")).unwrap();
    fs::rename(replacement, &path).unwrap();
    a = a.with_read_budget(u64::MAX);
    let b = a.poll(&saved, 0).unwrap();
    assert_eq!(b.events.len(), 1);
    assert_eq!(b.events[0].event_id, "gemini:session-a:answer");
    assert_eq!(b.files.len(), 1);
    assert_eq!(
        b.files[0].path,
        fs::canonicalize(path).unwrap().to_string_lossy()
    );
}

#[test]
fn subagent_fallback_and_project_directory_symlinks() {
    use std::os::unix::fs::symlink;
    let (d, chats) = setup();
    fs::create_dir(chats.join("parent")).unwrap();
    fs::write(chats.join("parent/child.jsonl"), ANSWER).unwrap();
    let b = GeminiAdapter::new(d.path().into())
        .poll(&HashMap::new(), 0)
        .unwrap();
    assert_eq!(b.events[0].event_id, "gemini:child:answer");
    assert_eq!(b.events[0].session_id, "parent");
    symlink(chats.parent().unwrap(), d.path().join("tmp/linked-project")).unwrap();
    let other = d.path().join("tmp/other");
    fs::create_dir(&other).unwrap();
    symlink(&chats, other.join("chats")).unwrap();
    let root = d.path().join("linked-root");
    symlink(d.path(), &root).unwrap();
    let b = GeminiAdapter::new(root).poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.files.len(), 1);
}

#[test]
fn stale_offsets_replay_with_metadata_session_id() {
    let (d, chats) = setup();
    let path = chats.join("different-file-id.jsonl");
    fs::write(&path, META).unwrap();
    let mut adapter = GeminiAdapter::new(d.path().into());
    let saved = offsets(&adapter.poll(&HashMap::new(), 0).unwrap());
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(ANSWER.as_bytes())
        .unwrap();
    let first = adapter.poll(&saved, 0).unwrap();
    let replay = adapter.poll(&saved, 0).unwrap();
    assert_eq!(first.events.len(), 1);
    assert_eq!(replay.events.len(), 1);
    assert_eq!(replay.events[0].event_id, "gemini:session-a:answer");
    assert_eq!(replay.events[0].session_id, "session-a");
    assert_eq!(replay.events[0].usage, first.events[0].usage);
}

#[test]
fn tokens_arrive_in_a_later_poll() {
    let (d, chats) = setup();
    let path = chats.join("session-file.jsonl");
    fs::write(&path, format!("{META}{}", ANSWER.replace(r#"{"input":2000,"cached":1500,"output":300,"thoughts":50,"tool":100,"total":2450}"#, "null"))).unwrap();
    let mut adapter = GeminiAdapter::new(d.path().into());
    let first = adapter.poll(&HashMap::new(), 0).unwrap();
    assert!(first.events.is_empty());
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(ANSWER.as_bytes())
        .unwrap();
    let second = adapter.poll(&offsets(&first), 0).unwrap();
    assert_eq!(second.events.len(), 1);
    assert_eq!(second.events[0].event_id, "gemini:session-a:answer");
    assert!(
        adapter
            .poll(&offsets(&second), 0)
            .unwrap()
            .events
            .is_empty()
    );
}

#[test]
fn project_path_read_is_bounded() {
    let (d, chats) = setup();
    fs::write(chats.join("session.jsonl"), format!("{META}{ANSWER}")).unwrap();
    fs::write(d.path().join("tmp/project/.project_root"), "x".repeat(8192)).unwrap();
    let batch = GeminiAdapter::new(d.path().into())
        .poll(&HashMap::new(), 0)
        .unwrap();
    assert_eq!(batch.sessions[0].project_path.as_ref().unwrap().len(), 4096);
}

#[test]
fn subagent_model_does_not_override_main_model() {
    let (d, chats) = setup();
    fs::create_dir(chats.join("parent")).unwrap();
    fs::write(
        chats.join("parent.jsonl"),
        format!("{}{ANSWER}", META.replace("session-a", "parent")),
    )
    .unwrap();
    fs::write(
        chats.join("parent/sub.jsonl"),
        format!(
            "{}{}",
            META.replace("session-a", "child"),
            ANSWER
                .replace("gemini-test", "child-model")
                .replace("09:00:02", "09:00:03")
        ),
    )
    .unwrap();
    let batch = GeminiAdapter::new(d.path().into())
        .poll(&HashMap::new(), 0)
        .unwrap();
    assert_eq!(batch.events.len(), 2);
    assert_eq!(batch.sessions.len(), 1);
    assert_eq!(batch.sessions[0].model.as_deref(), Some("gemini-test"));
    assert!(
        batch
            .events
            .iter()
            .any(|event| event.model == "child-model")
    );
}
