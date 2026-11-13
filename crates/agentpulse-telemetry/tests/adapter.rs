//! All fixtures are synthetic: invented ids, paths and token counts.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use agentpulse_core::{AgentAdapter, LiveState, PollBatch, Provider, TokenUsage};
use agentpulse_telemetry::ClaudeCodeAdapter;

fn asst(ts: &str, msg_id: &str, model: &str, out: u64) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{ts}","cwd":"/synthetic/proj","message":{{"id":"{msg_id}","model":"{model}","usage":{{"input_tokens":2,"output_tokens":{out},"cache_read_input_tokens":10,"cache_creation_input_tokens":0}}}}}}"#
    )
}

fn write(path: &Path, lines: &[String]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut s = lines.join("\n");
    s.push('\n');
    fs::write(path, s).unwrap();
}

fn append(path: &Path, text: &str) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

fn setup() -> (tempfile::TempDir, ClaudeCodeAdapter, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let proj = d.path().join("projects/-synthetic-proj");
    let a = ClaudeCodeAdapter::new(d.path().to_path_buf());
    (d, a, proj)
}

fn offsets(b: &PollBatch) -> HashMap<String, u64> {
    b.files.iter().map(|f| (f.path.clone(), f.offset)).collect()
}

const T1: &str = "2026-11-13T02:00:01.000Z";
const T2: &str = "2026-11-13T02:00:02.000Z";
const T3: &str = "2026-11-13T02:00:03.000Z";

#[test]
fn provider_and_missing_dirs() {
    let d = tempfile::tempdir().unwrap();
    let mut a = ClaudeCodeAdapter::new(d.path().to_path_buf());
    assert_eq!(a.provider(), Provider::ClaudeCode);
    assert!(a.poll(&HashMap::new(), 0).unwrap().is_empty());
    assert!(a.live_sessions().unwrap().is_empty());
}

#[test]
fn repeated_message_id_counts_once() {
    let (_d, mut a, proj) = setup();
    let l = asst(T1, "msg_1", "m-a", 50);
    write(&proj.join("s1.jsonl"), &[l.clone(), l.clone(), l]);
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.events.len(), 1);
    assert_eq!(b.events[0].event_id, "msg_1");
    assert_eq!(b.events[0].session_id, "s1");
    assert_eq!(
        b.events[0].usage,
        TokenUsage {
            input: 2,
            output: 50,
            cache_read: 10,
            ..Default::default()
        }
    );
    assert_eq!(b.sessions[0].timestamps_ms.len(), 3);
    assert!(!b.more);
}

#[test]
fn subagent_belongs_to_parent_session() {
    let (_d, mut a, proj) = setup();
    write(&proj.join("s1.jsonl"), &[asst(T1, "msg_main", "m-main", 5)]);
    write(
        &proj.join("s1/subagents/agent-x.jsonl"),
        &[asst(T3, "msg_sub", "m-sub", 7)],
    );
    fs::write(proj.join("s1/subagents/agent-x.meta.json"), "{}").unwrap();
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.sessions.len(), 1);
    let s = &b.sessions[0];
    assert_eq!(s.session_id, "s1");
    assert_eq!(s.timestamps_ms.len(), 2);
    assert_eq!(
        s.model.as_deref(),
        Some("m-main"),
        "subagent never sets model"
    );
    assert_eq!(b.events.len(), 2);
    assert!(b.events.iter().all(|e| e.session_id == "s1"));
    assert_eq!(b.files.len(), 2);
}

#[test]
fn synthetic_and_zero_usage_make_no_event() {
    let (_d, mut a, proj) = setup();
    let zero = asst(T2, "msg_zero", "m-a", 0)
        .replace(r#""input_tokens":2"#, r#""input_tokens":0"#)
        .replace(
            r#""cache_read_input_tokens":10"#,
            r#""cache_read_input_tokens":0"#,
        );
    write(
        &proj.join("s1.jsonl"),
        &[asst(T1, "msg_syn", "<synthetic>", 9), zero],
    );
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert!(b.events.is_empty());
    assert_eq!(b.sessions[0].timestamps_ms.len(), 2);
}

#[test]
fn malformed_and_partial_lines() {
    let (_d, mut a, proj) = setup();
    let path = proj.join("s1.jsonl");
    write(
        &path,
        &[
            asst(T1, "msg_1", "m-a", 1),
            "{not json".into(),
            asst(T2, "msg_2", "m-a", 2),
        ],
    );
    let full = asst(T3, "msg_3", "m-a", 3);
    let (head, tail) = full.split_at(30);
    append(&path, head);
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.events.len(), 2, "bad line skipped, later lines parse");
    let off = offsets(&b);
    let consumed = fs::read(&path).unwrap().len() - head.len();
    assert_eq!(off[&path.to_string_lossy().to_string()], consumed as u64);

    append(&path, &format!("{tail}\n"));
    let b2 = a.poll(&off, 0).unwrap();
    assert_eq!(b2.events.len(), 1);
    assert_eq!(b2.events[0].event_id, "msg_3");
    assert_eq!(b2.sessions[0].timestamps_ms.len(), 1);
}

#[test]
fn unchanged_files_yield_empty_batch() {
    let (_d, mut a, proj) = setup();
    write(&proj.join("s1.jsonl"), &[asst(T1, "msg_1", "m-a", 1)]);
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert!(a.poll(&offsets(&b), 0).unwrap().is_empty());
}

#[test]
fn cache_write_split_and_fallback() {
    let (_d, mut a, proj) = setup();
    let split = r#"{"type":"assistant","timestamp":"2026-11-13T02:00:01Z","message":{"id":"a","model":"m","usage":{"cache_creation_input_tokens":300,"cache_creation":{"ephemeral_5m_input_tokens":100,"ephemeral_1h_input_tokens":200},"speed":"fast","inference_geo":"us"}}}"#;
    let plain = r#"{"type":"assistant","timestamp":"2026-11-13T02:00:02Z","message":{"id":"b","model":"m","usage":{"cache_creation_input_tokens":300,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0},"speed":"standard","inference_geo":"global"}}}"#;
    write(&proj.join("s1.jsonl"), &[split.into(), plain.into()]);
    let b = a.poll(&HashMap::new(), 0).unwrap();
    let get = |id: &str| b.events.iter().find(|e| e.event_id == id).unwrap();
    assert_eq!(
        (get("a").usage.cache_write_5m, get("a").usage.cache_write_1h),
        (100, 200)
    );
    assert!(get("a").fast && get("a").us_inference);
    assert_eq!(
        (get("b").usage.cache_write_5m, get("b").usage.cache_write_1h),
        (300, 0)
    );
    assert!(!get("b").fast && !get("b").us_inference);
}

#[test]
fn truncated_file_is_reread() {
    let (_d, mut a, proj) = setup();
    let path = proj.join("s1.jsonl");
    write(
        &path,
        &[asst(T1, "msg_1", "m-a", 1), asst(T2, "msg_2", "m-a", 2)],
    );
    let b = a.poll(&HashMap::new(), 0).unwrap();
    write(&path, &[asst(T3, "msg_9", "m-a", 9)]);
    let b2 = a.poll(&offsets(&b), 0).unwrap();
    assert_eq!(b2.events.len(), 1);
    assert_eq!(b2.events[0].event_id, "msg_9");
}

#[test]
fn old_file_without_offset_is_ignored() {
    let (d, mut a, proj) = setup();
    let path = proj.join("s1.jsonl");
    write(&path, &[asst(T1, "msg_1", "m-a", 1)]);
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert!(a.poll(&HashMap::new(), 2_000_000_000).unwrap().is_empty());
    // A fresh adapter: the cached scan treats the old file as cold.
    let mut a = ClaudeCodeAdapter::new(d.path().to_path_buf());
    assert!(!a.poll(&HashMap::new(), 0).unwrap().is_empty());
}

#[test]
fn old_file_of_live_session_is_read() {
    let (d, mut a, proj) = setup();
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    for sid in ["live", "gone"] {
        let path = proj.join(format!("{sid}.jsonl"));
        write(&path, &[asst(T1, &format!("msg_{sid}"), "m-a", 1)]);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    let reg = d.path().join("sessions");
    fs::create_dir_all(&reg).unwrap();
    fs::write(reg.join("42.json"), r#"{"pid":42,"sessionId":"live"}"#).unwrap();
    let b = a.poll(&HashMap::new(), 2_000_000_000).unwrap();
    assert_eq!(b.sessions.len(), 1);
    assert_eq!(b.sessions[0].session_id, "live");
    assert_eq!(b.events.len(), 1);
    assert_eq!(b.events[0].event_id, "msg_live");
}

#[test]
fn concurrent_sessions_stay_separate() {
    let (_d, mut a, proj) = setup();
    write(&proj.join("s1.jsonl"), &[asst(T1, "msg_a", "m-a", 1)]);
    write(&proj.join("s2.jsonl"), &[asst(T2, "msg_b", "m-b", 2)]);
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(b.sessions.len(), 2);
    for s in &b.sessions {
        let ev: Vec<_> = b
            .events
            .iter()
            .filter(|e| e.session_id == s.session_id)
            .collect();
        assert_eq!(ev.len(), 1);
    }
    let s2 = b.sessions.iter().find(|s| s.session_id == "s2").unwrap();
    assert_eq!(s2.model.as_deref(), Some("m-b"));
    assert_eq!(s2.project_path.as_deref(), Some("/synthetic/proj"));
}

#[test]
fn last_error_and_model_come_from_main_file() {
    let (_d, mut a, proj) = setup();
    let err = r#"{"type":"assistant","timestamp":"2026-11-13T02:00:02Z","isApiErrorMessage":true,"message":{"id":"e","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#;
    write(
        &proj.join("s1.jsonl"),
        &[asst(T1, "msg_1", "m-main", 1), err.into()],
    );
    write(
        &proj.join("s1/subagents/a.jsonl"),
        &[asst(T3, "msg_s", "m-sub", 1)],
    );
    let b = a.poll(&HashMap::new(), 0).unwrap();
    let s = &b.sessions[0];
    assert_eq!(s.last_error, Some(true));
    assert_eq!(s.model.as_deref(), Some("m-main"));

    let (_d2, mut a2, proj2) = setup();
    write(&proj2.join("s1.jsonl"), &[asst(T1, "msg_1", "m-main", 1)]);
    assert_eq!(
        a2.poll(&HashMap::new(), 0).unwrap().sessions[0].last_error,
        Some(false)
    );
}

#[test]
fn live_sessions_parse() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("sessions");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("56129.json"), r#"{"pid":56129,"sessionId":"20c7b214-synthetic","cwd":"/Users/x/proj","startedAt":1794536615135,"procStart":"Fri Nov 13 02:23:34 2026","status":"busy","updatedAt":1794536615534}"#).unwrap();
    fs::write(
        dir.join("7.json"),
        r#"{"pid":7,"sessionId":"other","status":"thinking-hard"}"#,
    )
    .unwrap();
    fs::write(dir.join("8.json"), "{broken").unwrap();
    // ps pads single-digit days with a space.
    fs::write(
        dir.join("6.json"),
        r#"{"pid":6,"sessionId":"pad","procStart":"Tue Nov  3 02:23:34 2026"}"#,
    )
    .unwrap();
    fs::write(dir.join("9.key"), r#"{"pid":9,"sessionId":"k"}"#).unwrap();
    let mut live = ClaudeCodeAdapter::new(d.path().to_path_buf())
        .live_sessions()
        .unwrap();
    live.sort_by_key(|l| l.pid);
    assert_eq!(live.len(), 3);
    assert_eq!(live[0].proc_start_s, Some(1793672614));
    let live = &live[1..];
    assert_eq!(live[1].proc_start_s, Some(1794536614));
    assert_eq!(live[1].started_at_ms, Some(1794536615135));
    assert_eq!(live[1].state, LiveState::Busy);
    assert_eq!(live[1].project_path.as_deref(), Some("/Users/x/proj"));
    assert_eq!(live[0].state, LiveState::Unknown);
    assert_eq!(live[0].proc_start_s, None);
}

#[test]
fn string_message_still_contributes_timestamp() {
    let (_d, mut a, proj) = setup();
    write(
        &proj.join("s1.jsonl"),
        &[format!(
            r#"{{"type":"user","timestamp":"{T1}","message":"synthetic text"}}"#
        )],
    );
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert!(b.events.is_empty());
    assert_eq!(b.sessions[0].timestamps_ms, vec![1794535201000]);
}

#[test]
fn hot_file_append_is_picked_up_immediately() {
    let (_d, mut a, proj) = setup();
    let path = proj.join("s1.jsonl");
    write(&path, &[asst(T1, "msg_1", "m-a", 1)]);
    let b = a.poll(&HashMap::new(), 0).unwrap();
    append(&path, &format!("{}\n", asst(T2, "msg_2", "m-a", 2)));
    let b2 = a.poll(&offsets(&b), 0).unwrap();
    assert_eq!(b2.events.len(), 1);
    assert_eq!(b2.events[0].event_id, "msg_2");
    assert_eq!(a.full_scans(), 1, "second poll must not rescan");
}

#[test]
fn new_session_found_after_rescan_interval() {
    let (_d, a, proj) = setup();
    let mut a = a.with_rescan_interval(Duration::from_millis(30));
    write(&proj.join("s1.jsonl"), &[asst(T1, "msg_1", "m-a", 1)]);
    let b = a.poll(&HashMap::new(), 0).unwrap();
    write(&proj.join("s2.jsonl"), &[asst(T2, "msg_2", "m-a", 2)]);
    std::thread::sleep(Duration::from_millis(80));
    let b2 = a.poll(&offsets(&b), 0).unwrap();
    assert_eq!(b2.events.len(), 1);
    assert_eq!(b2.events[0].session_id, "s2");
}

#[test]
fn new_live_session_forces_rescan() {
    let (d, mut a, proj) = setup();
    write(&proj.join("s1.jsonl"), &[asst(T1, "msg_1", "m-a", 1)]);
    let b = a.poll(&HashMap::new(), 0).unwrap();
    write(&proj.join("s2.jsonl"), &[asst(T2, "msg_2", "m-a", 2)]);
    let reg = d.path().join("sessions");
    fs::create_dir_all(&reg).unwrap();
    fs::write(reg.join("5.json"), r#"{"pid":5,"sessionId":"s2"}"#).unwrap();
    let b2 = a.poll(&offsets(&b), 0).unwrap();
    assert_eq!(b2.events.len(), 1);
    assert_eq!(b2.events[0].session_id, "s2");
    assert_eq!(a.full_scans(), 2);
}

#[test]
fn cold_file_append_is_found_by_next_full_scan() {
    let (_d, a, proj) = setup();
    let mut a = a.with_rescan_interval(Duration::from_millis(30));
    let path = proj.join("s1.jsonl");
    write(&path, &[asst(T1, "msg_1", "m-a", 1)]);
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let size = fs::metadata(&path).unwrap().len();
    let off = HashMap::from([(path.to_string_lossy().to_string(), size)]);
    assert!(a.poll(&off, 0).unwrap().is_empty());
    append(&path, &format!("{}\n", asst(T2, "msg_2", "m-a", 2)));
    assert!(
        a.poll(&off, 0).unwrap().is_empty(),
        "cold files wait for the rescan"
    );
    std::thread::sleep(Duration::from_millis(80));
    let b = a.poll(&off, 0).unwrap();
    assert_eq!(b.events.len(), 1);
    assert_eq!(b.events[0].event_id, "msg_2");
}

#[test]
fn backfill_keeps_doing_full_scans() {
    let (_d, a, proj) = setup();
    let mut a = a.with_read_budget(1);
    for i in 1..=3 {
        write(
            &proj.join(format!("s{i}.jsonl")),
            &[asst(T1, &format!("msg_{i}"), "m-a", 1)],
        );
    }
    let (mut off, mut polls, mut events) = (HashMap::new(), 0, 0);
    loop {
        let b = a.poll(&off, 0).unwrap();
        polls += 1;
        events += b.events.len();
        off.extend(offsets(&b));
        assert_eq!(a.full_scans(), polls);
        if !b.more {
            break;
        }
    }
    assert_eq!(events, 3);
}

fn asst_for(sid: &str, ts: &str, msg_id: &str, model: &str) -> String {
    asst(ts, msg_id, model, 5).replacen(
        "{\"type\"",
        &format!("{{\"sessionId\":\"{sid}\",\"type\""),
        1,
    )
}

fn copied_history() -> (tempfile::TempDir, ClaudeCodeAdapter, PathBuf, PathBuf) {
    let (d, a, proj) = setup();
    let (zzz, aaa) = (proj.join("zzz.jsonl"), proj.join("aaa.jsonl"));
    let orig = asst_for("zzz", T1, "msg_z", "m-zzz");
    write(&zzz, std::slice::from_ref(&orig));
    write(&aaa, &[orig, asst_for("aaa", T3, "msg_a", "m-aaa")]);
    (d, a, zzz, aaa)
}

fn ids<'a>(b: &'a PollBatch, sid: &str) -> Vec<&'a str> {
    b.events
        .iter()
        .filter(|e| e.session_id == sid)
        .map(|e| e.event_id.as_str())
        .collect()
}

#[test]
fn copied_history_belongs_to_original_session() {
    let (_d, mut a, _z, _a) = copied_history();
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(ids(&b, "zzz"), ["msg_z"]);
    assert_eq!(ids(&b, "aaa"), ["msg_a"]);
    assert_eq!(b.events.len(), 2);
    let ts = |s: &str| {
        b.sessions
            .iter()
            .find(|d| d.session_id == s)
            .unwrap()
            .timestamps_ms
            .len()
    };
    // zzz: own record + the copy; aaa: only its own record.
    assert_eq!((ts("zzz"), ts("aaa")), (2, 1));
}

#[test]
fn copied_history_in_separate_polls_either_order() {
    for first in ["aaa", "zzz"] {
        let (d, mut a, zzz, aaa) = copied_history();
        let (f1, f2) = if first == "aaa" {
            (aaa, zzz)
        } else {
            (zzz, aaa)
        };
        let k = |p: &Path| p.to_string_lossy().to_string();
        // Poll 1 reads only the first file: the other has a far-future "since".
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        File::options()
            .write(true)
            .open(&f2)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let b1 = a.poll(&HashMap::new(), 2_000_000_000).unwrap();
        assert_eq!(b1.files.len(), 1);
        assert_eq!(b1.files[0].path, k(&f1));
        let mut a = ClaudeCodeAdapter::new(d.path().to_path_buf());
        let b2 = a.poll(&offsets(&b1), 0).unwrap();
        let mut all: Vec<_> = b1.events.iter().chain(&b2.events).collect();
        all.sort_by_key(|e| (e.session_id.clone(), e.event_id.clone()));
        all.dedup_by_key(|e| e.event_id.clone());
        let pairs: Vec<_> = all
            .iter()
            .map(|e| (e.session_id.as_str(), e.event_id.as_str()))
            .collect();
        assert_eq!(pairs, [("aaa", "msg_a"), ("zzz", "msg_z")], "first={first}");
    }
}

#[test]
fn record_without_session_id_uses_file_id() {
    let (_d, mut a, proj) = setup();
    write(&proj.join("s1.jsonl"), &[asst(T1, "msg_1", "m-a", 1)]);
    let b = a.poll(&HashMap::new(), 0).unwrap();
    assert_eq!(ids(&b, "s1"), ["msg_1"]);
}

#[test]
fn copied_records_do_not_set_model_or_error() {
    let (_d, mut a, proj) = setup();
    let err = r#"{"sessionId":"zzz","type":"assistant","timestamp":"2026-11-13T02:00:09Z","isApiErrorMessage":true,"message":{"id":"e","model":"<synthetic>","usage":{"input_tokens":0}}}"#;
    write(
        &proj.join("zzz.jsonl"),
        &[asst_for("zzz", T1, "msg_z", "m-zzz")],
    );
    write(
        &proj.join("aaa.jsonl"),
        &[asst_for("zzz", T3, "msg_copy", "m-copy"), err.into()],
    );
    let b = a.poll(&HashMap::new(), 0).unwrap();
    let z = b.sessions.iter().find(|s| s.session_id == "zzz").unwrap();
    assert_eq!(z.model.as_deref(), Some("m-zzz"));
    assert_eq!(z.last_error, Some(false));
    assert!(b.sessions.iter().all(|s| s.session_id != "aaa"));
}

#[test]
fn unfound_live_session_forces_one_rescan_only() {
    let (d, a, proj) = setup();
    let mut a = a.with_rescan_interval(Duration::from_millis(300));
    write(&proj.join("s1.jsonl"), &[asst(T1, "msg_1", "m-a", 1)]);
    let mut off = offsets(&a.poll(&HashMap::new(), 0).unwrap());
    let reg = d.path().join("sessions");
    fs::create_dir_all(&reg).unwrap();
    fs::write(reg.join("5.json"), r#"{"pid":5,"sessionId":"fresh"}"#).unwrap();
    for _ in 0..4 {
        off.extend(offsets(&a.poll(&off, 0).unwrap()));
    }
    assert_eq!(a.full_scans(), 2, "initial scan + exactly one forced");
    write(&proj.join("fresh.jsonl"), &[asst(T2, "msg_f", "m-a", 2)]);
    std::thread::sleep(Duration::from_millis(400));
    let b = a.poll(&off, 0).unwrap();
    assert_eq!(ids(&b, "fresh"), ["msg_f"]);
}
