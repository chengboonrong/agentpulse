//! All fixtures are synthetic; no data is copied from a Qwen runtime directory.
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::Duration;

use agentpulse_core::{AgentAdapter, PollBatch, Provider, TokenUsage};
use agentpulse_telemetry::QwenAdapter;

const FIXTURE: &str = include_str!("fixtures/qwen/projects/-synthetic-proj/chats/session-a.jsonl");
const PATH: &str = "projects/-synthetic-proj/chats/session-a.jsonl";

fn offsets(b: &PollBatch) -> HashMap<String, u64> {
    b.files.iter().map(|f| (f.path.clone(), f.offset)).collect()
}

#[test]
fn tokens_metadata_and_incremental_reads() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(PATH);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    // The final assistant record is only half written on the first poll.
    let cut = FIXTURE.rfind("{\"uuid\":\"answer-2\"").unwrap();
    fs::write(&p, &FIXTURE[..cut + 20]).unwrap();
    let mut a = QwenAdapter::new(d.path().to_path_buf());
    assert_eq!(a.provider(), Provider::Qwen);
    assert!(a.live_sessions().unwrap().is_empty());
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.provider, Provider::Qwen);
    assert_eq!(b.events.len(), 1, "ui_telemetry must not add usage");
    let e = &b.events[0];
    assert_eq!(e.event_id, "session-a:answer-1");
    assert_eq!(e.session_id, "session-a");
    assert_eq!(e.ts_ms, 1_794_906_002_000);
    assert_eq!(e.model, "coder-model");
    assert_eq!(e.recorded_usd, None);
    assert_eq!(
        e.usage,
        TokenUsage {
            input: 500,
            cache_read: 1500,
            output: 300,
            ..Default::default()
        }
    );
    let s = &b.sessions[0];
    assert_eq!(s.session_id, "session-a");
    assert_eq!(s.project_path.as_deref(), Some("/synthetic/proj"));
    assert_eq!(s.model.as_deref(), Some("coder-model"));
    assert_eq!(
        s.timestamps_ms,
        [
            1_794_906_001_000,
            1_794_906_002_000,
            1_794_906_003_000,
            1_794_906_004_000
        ]
    );
    assert_eq!(b.files[0].offset, cut as u64);
    let off = offsets(&b);
    assert!(a.poll(&off, 0).unwrap().is_empty());
    assert!(
        QwenAdapter::new(d.path().to_path_buf())
            .poll(&off, 0)
            .unwrap()
            .is_empty()
    );
    OpenOptions::new()
        .append(true)
        .open(&p)
        .unwrap()
        .write_all(&FIXTURE.as_bytes()[cut + 20..])
        .unwrap();
    let b = a.poll(&off, 0).unwrap();
    assert_eq!(b.events.len(), 1);
    assert_eq!(b.events[0].event_id, "session-a:answer-2");
    assert_eq!(
        b.events[0].usage,
        TokenUsage {
            input: 0,
            cache_read: 20,
            output: 5,
            ..Default::default()
        }
    );
    assert_eq!(b.sessions[0].model.as_deref(), Some("qwen-test"));
    assert_eq!(b.files[0].offset, FIXTURE.len() as u64);
}

#[test]
fn missing_directories_are_empty() {
    let d = tempfile::tempdir().unwrap();
    for root in [d.path().to_path_buf(), d.path().join("missing")] {
        let mut a = QwenAdapter::new(root);
        assert!(a.poll(&HashMap::new(), 0).unwrap().is_empty());
    }
}

#[test]
fn discovery_and_cached_reads_skip_symlinks_and_other_files() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(PATH);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, FIXTURE).unwrap();
    for relative in [
        "settings.json",
        "projects/other.jsonl",
        "projects/-synthetic-proj/other.jsonl",
        "projects/-synthetic-proj/chats/other.json",
    ] {
        fs::write(d.path().join(relative), FIXTURE).unwrap();
    }
    symlink(&p, p.with_file_name("linked.jsonl")).unwrap();
    symlink(
        p.parent().unwrap().parent().unwrap(),
        d.path().join("projects/linked"),
    )
    .unwrap();
    let other = d.path().join("projects/other");
    fs::create_dir_all(&other).unwrap();
    symlink(p.parent().unwrap(), other.join("chats")).unwrap();
    let mut a =
        QwenAdapter::new(d.path().to_path_buf()).with_rescan_interval(Duration::from_secs(60));
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.files.len(), 1);
    assert_eq!(b.sessions.len(), 1);
    assert_eq!(b.events.len(), 2);
    let off = offsets(&b);
    fs::remove_file(&p).unwrap();
    let bait = d.path().join("bait");
    fs::write(&bait, format!("{FIXTURE}{FIXTURE}")).unwrap();
    symlink(bait, &p).unwrap();
    assert!(a.poll(&off, 0).unwrap().is_empty());
}

#[test]
fn a_symlinked_root_is_followed() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("real").join(PATH);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, FIXTURE).unwrap();
    let root = d.path().join("linked");
    std::os::unix::fs::symlink(d.path().join("real"), &root).unwrap();
    let b = QwenAdapter::new(root).poll(&HashMap::new(), 0).unwrap();
    assert_eq!((b.sessions.len(), b.events.len()), (1, 2));
}

#[test]
fn read_budget_continues_on_next_poll() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("projects/p/chats");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.jsonl"), FIXTURE).unwrap();
    fs::write(dir.join("b.jsonl"), FIXTURE).unwrap();
    let mut a = QwenAdapter::new(d.path().to_path_buf())
        .with_read_budget(1)
        .with_rescan_interval(Duration::ZERO);
    let first = a.poll(&HashMap::new(), 0).unwrap();
    assert!(first.more);
    assert_eq!(first.files.len(), 1);
    let next = a.poll(&offsets(&first), 0).unwrap();
    assert_eq!(next.files.len(), 1);
    assert_ne!(first.files[0].path, next.files[0].path);
}
