# Orphan and Idle Process Clean-up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** AgentPulse remembers every process an agent starts. After the agent ends, it warns about leftover processes and later ends them automatically. Idle agent sessions only get a warning.

**Architecture:** On every process sample, the collector records the descendants of live agent roots in a new SQLite table. A sweep runs once a minute. It marks orphans whose agent has gone, warns about them, ends them (SIGTERM, then SIGKILL on the next sweep), and logs each clean. It then builds a `CleanupView` that the new Clean-up screen draws. The window sends End and Keep back to the collector through the existing detail channel, which becomes a `Command` enum. The sweep logic lives in `cleanup.rs` behind a small `Host` trait, so tests can run it on fake process tables.

**Tech Stack:** Rust 2024 workspace, rusqlite, GPUI, libc (`proc_pidinfo`, `kill`, `sysctlbyname`), objc2 0.6.5 with `objc2-user-notifications` 0.3.2 (already in `Cargo.lock`).

**Spec:** `docs/superpowers/specs/2026-11-21-orphan-and-idle-cleanup-design.md`

## Global Constraints

- Defaults: warn after **3 hours**, clean after **24 hours**, automatic clean **on**. `clean_after` is never below `warn_after`.
- A warning always comes at least **1 hour** (`WARN_LEAD_MS = 3_600_000`) before an automatic clean.
- Only tracked orphans (not agents, not kept, with `ended_at` set) are ever signalled automatically. No agent session (Active, Idle or Waiting) is signalled automatically.
- Every signal first checks that the PID still has the recorded start time. A reused PID is never signalled.
- Untracked leftovers are only ended by the user, after confirmation.
- No new crates. Use the same `objc2-user-notifications` version as `Cargo.lock` (0.3.2).
- Collector overhead with clean-up on stays at about 1% of one core (`agentpulse --bench`).
- Writing rules for comments, docs and UI copy follow the repo: plain sentences and no em dashes in copy.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass after every task.

## Calls this plan makes (the spec left them to the builder)

- The SIGTERM grace period is one sweep (`SWEEP_INTERVAL = 60 s`). SIGKILL is sent on the next sweep if the same process is still alive.
- The log keeps the newest **100** entries.
- The memory suggestion fires when memory pressure is Warn or Critical, or swap used is at least **2 GiB**. It repeats at most every **6 hours**. It is a notification plus a line at the top of the Clean-up screen. Clicking the notification does not open the screen (that needs a notification delegate; add it if asked).
- Agent processes themselves are not stored; only their descendants are. Idle-session warnings are kept in memory, so they can repeat after an AgentPulse restart.
- An idle session in a shared process (`AgentSession::process == None`) is listed without an End button and says "Shared with other sessions". This replaces the spec's "confirmation names every session that process holds": AgentPulse does not attribute a shared process to any one session, so it has nothing safe to end.
- Untracked candidates: user-owned processes whose parent is PID 1 and whose `p_comm`, lower-cased, starts with `node` or `python`, or contains `spline`.
- Settings offer presets. Warn after: 1, 3, 6, 12 h. Clean after: 12, 24, 48, 72 h. Any positive whole number of hours in `settings.json` is accepted.
- Notifications are sent only when AgentPulse runs from its app bundle (`NSBundle.mainBundle.bundleIdentifier` is set). Otherwise they are written to the log at info level. If notification permission is denied, nothing else changes.

## Review Focus

1. **The agent and the orphan end in the same sample.** A child that exits together with its agent must be deleted, never marked ended and later signalled. Test: `orphan_gone_with_agent_is_dropped` (Task 4).
2. **AgentPulse was closed past `clean_after`.** The first sweep must warn, and not clean until an hour later. Test: `late_start_warns_before_cleaning` (Task 4).
3. **The user lowers `clean_after` below `warn_after` in `settings.json`.** It is raised to `warn_after`. Test: `clean_after_never_below_warn_after` (Task 3).
4. **An orphan is re-adopted.** A tracked orphan that is now under a live agent root gets its clock cleared. Test: `orphan_under_new_agent_is_readopted` (Task 4).
5. **SIGTERM is ignored.** The process gets SIGKILL on the next sweep, and is never killed twice once gone. Test: `ignored_term_gets_kill_next_sweep` (Task 4).

---

### Task 1: Process-crate primitives

**Files:**
- Modify: `crates/agentpulse-process/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub fn roots_of(table: &ProcTable, roots: &[ProcessIdentity]) -> HashMap<u32, u32>`: each PID under a valid root (start time matches), mapped to its nearest root PID. Roots map to themselves. `attribute` is rewritten on top of it, with unchanged behaviour.
  - `pub struct ProcDetails { pub name: String, pub uid: u32 }` (name from `pbi_comm`, uid from `pbi_uid`).
  - `pub fn details(p: ProcessIdentity) -> Option<ProcDetails>`: None when the process is gone or the PID's start time differs.
  - `pub enum Signal { Term, Kill }`
  - `pub fn signal(p: ProcessIdentity, s: Signal) -> std::io::Result<bool>`: checks the start time with `bsd()`, then `libc::kill`. `Ok(false)` when the process is gone or reused. `ESRCH` is `Ok(false)`, other errnos are `Err`.
  - `pub enum Pressure { Normal, Warn, Critical }` and `pub fn memory_pressure() -> Option<Pressure>` (`kern.memorystatus_vm_pressure_level`: 1, 2, 4).
  - `pub fn swap_used_bytes() -> Option<u64>` (`vm.swapusage`, `libc::xsw_usage::xsu_used`).

- [ ] **Step 1: Write the failing tests** (in the existing `tests` module)

```rust
#[test]
fn roots_of_maps_each_descendant_to_its_nearest_root() {
    // 10 root, 11 child, 12 grandchild; 20 is a nested root under 11, 21 its child; 30 unrelated.
    let t = table(&[(10, p(Some(1), 100, 0., 0)), (11, p(Some(10), 101, 0., 0)),
        (12, p(Some(11), 102, 0., 0)), (20, p(Some(11), 103, 0., 0)),
        (21, p(Some(20), 104, 0., 0)), (30, p(Some(1), 105, 0., 0))], true);
    let m = roots_of(&t, &[id(10, 100), id(20, 103)]);
    assert_eq!(m.get(&11), Some(&10));
    assert_eq!(m.get(&12), Some(&10));
    assert_eq!(m.get(&20), Some(&20));
    assert_eq!(m.get(&21), Some(&20));
    assert_eq!(m.get(&30), None);
    // A root whose start time no longer matches is not a root.
    assert!(roots_of(&t, &[id(10, 999)]).is_empty());
}

#[test]
fn details_and_signal_check_the_start_time() {
    let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id();
    let start = bsd(pid).unwrap().pbi_start_tvsec;
    assert_eq!(details(id(pid, start)).unwrap().name, "sleep");
    assert!(details(id(pid, start + 5)).is_none());
    assert!(!signal(id(pid, start + 5), Signal::Term).unwrap(), "reused PID is never signalled");
    assert!(signal(id(pid, start), Signal::Term).unwrap());
    child.wait().unwrap();
    assert!(!signal(id(pid, start), Signal::Kill).unwrap());
}

#[test]
fn memory_readings_are_available() {
    assert!(memory_pressure().is_some());
    assert!(swap_used_bytes().is_some());
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p agentpulse-process`
Expected: compile errors for `roots_of`, `details`, `signal`, `Signal`, `memory_pressure`, `swap_used_bytes`.

- [ ] **Step 3: Implement the interfaces above in `crates/agentpulse-process/src/lib.rs`.** `roots_of` keeps `attribute`'s visited-set cycle guard. `attribute` then sums usage over `roots_of`'s map.

- [ ] **Step 4: Run all process tests**

Run: `cargo test -p agentpulse-process`
Expected: PASS, including every existing `attribute` test.

- [ ] **Step 5: Commit**

```bash
git add crates/agentpulse-process/src/lib.rs
git commit -m "process: roots_of, details, guarded signal, memory readings"
```

---

### Task 2: Storage: tracked processes and the clean-up log

**Files:**
- Modify: `crates/agentpulse-core/src/lib.rs` (add `Provider::parse`)
- Modify: `crates/agentpulse-storage/src/lib.rs` (migration 4 and methods)

**Interfaces:**
- Produces:
  - `impl Provider { pub fn parse(key: &str) -> Option<Provider> }`, the inverse of `as_str`.
  - Migration 4, appended to `MIGRATIONS`:
    ```sql
    CREATE TABLE tracked_processes (
      pid INTEGER NOT NULL, start_time_s INTEGER NOT NULL,
      root_pid INTEGER NOT NULL, root_start_s INTEGER NOT NULL,
      provider TEXT NOT NULL, session_id TEXT NOT NULL, name TEXT NOT NULL,
      ended_at_ms INTEGER, warned_at_ms INTEGER, term_sent_at_ms INTEGER,
      kept INTEGER NOT NULL DEFAULT 0,
      PRIMARY KEY (pid, start_time_s));
    CREATE TABLE cleanup_log (
      id INTEGER PRIMARY KEY, at_ms INTEGER NOT NULL, name TEXT NOT NULL,
      provider TEXT, session_id TEXT, rss_bytes INTEGER NOT NULL, automatic INTEGER NOT NULL);
    ```
  - `#[derive(Clone, Debug, PartialEq, Serialize)] pub struct TrackedProcess { pub process: ProcessIdentity, pub root: ProcessIdentity, pub provider: Provider, pub session_id: String, pub name: String, pub ended_at_ms: Option<i64>, pub warned_at_ms: Option<i64>, pub term_sent_at_ms: Option<i64>, pub kept: bool }`
  - `#[derive(Clone, Debug, PartialEq, Serialize)] pub struct CleanupLogEntry { pub at_ms: i64, pub name: String, pub provider: Option<Provider>, pub session_id: Option<String>, pub rss_bytes: u64, pub automatic: bool }`
  - `Store::tracked_processes(&self) -> Result<Vec<TrackedProcess>>` (a row with an unknown provider is skipped with a warning)
  - `Store::save_tracked(&mut self, rows: &[TrackedProcess]) -> Result<()>` (upsert, one transaction)
  - `Store::delete_tracked(&mut self, ids: &[ProcessIdentity]) -> Result<()>` (one transaction)
  - `Store::log_cleanup(&mut self, e: &CleanupLogEntry) -> Result<()>` (inserts, then deletes all but the newest 100)
  - `Store::cleanup_log(&self, limit: usize) -> Result<Vec<CleanupLogEntry>>` (newest first)

- [ ] **Step 1: Write the failing tests** (storage `tests` module; core gets `provider_keys_round_trip`)

```rust
#[test]
fn tracked_processes_survive_reopening() {
    let dir = std::env::temp_dir().join(format!("ap-tracked-{}", std::process::id()));
    let path = dir.join("t.db");
    let row = TrackedProcess { process: ProcessIdentity { pid: 42, start_time_s: 7 },
        root: ProcessIdentity { pid: 40, start_time_s: 5 }, provider: Provider::Codex,
        session_id: "s".into(), name: "node".into(), ended_at_ms: Some(9),
        warned_at_ms: None, term_sent_at_ms: None, kept: true };
    Store::open(&path).unwrap().save_tracked(&[row.clone()]).unwrap();
    assert_eq!(Store::open(&path).unwrap().tracked_processes().unwrap(), vec![row.clone()]);
    let mut s = Store::open(&path).unwrap();
    s.delete_tracked(&[row.process]).unwrap();
    assert!(s.tracked_processes().unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cleanup_log_keeps_the_newest_hundred() {
    let mut s = Store::open_in_memory().unwrap();
    for i in 0..105 {
        s.log_cleanup(&CleanupLogEntry { at_ms: i, name: "python3".into(), provider: None,
            session_id: None, rss_bytes: 1, automatic: true }).unwrap();
    }
    let log = s.cleanup_log(1000).unwrap();
    assert_eq!(log.len(), 100);
    assert_eq!((log[0].at_ms, log[99].at_ms), (104, 5));
}

#[test]
fn migration_four_keeps_existing_data() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate(&mut conn, &MIGRATIONS[..3]).unwrap();
    let mut old = Store { conn };
    old.apply(&batch(vec![ev("e", "s", 1, tokens(3, 4))], vec![delta("s", &[1])]), &flat).unwrap();
    migrate(&mut old.conn, MIGRATIONS).unwrap();
    assert_eq!(old.sessions_since(P, 0).unwrap().len(), 1);
    assert!(old.tracked_processes().unwrap().is_empty());
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p agentpulse-storage -p agentpulse-core`
Expected: compile errors for the new types and methods.

- [ ] **Step 3: Implement `Provider::parse`, migration 4, the two structs and the five `Store` methods.**

- [ ] **Step 4: Run the tests**

Run: `cargo test -p agentpulse-storage -p agentpulse-core`
Expected: PASS, including the existing `recorded_cost_migration_keeps_data_and_rereads_files`.

- [ ] **Step 5: Commit**

```bash
git add crates/agentpulse-core/src/lib.rs crates/agentpulse-storage/src/lib.rs
git commit -m "storage: tracked_processes and cleanup_log (migration 4)"
```

---

### Task 3: The `cleanup` section in settings.json

**Files:**
- Modify: `crates/agentpulse-app/src/settings.rs` (module doc, `State`, `Part`, `parse`, `sections`, `save`)

**Interfaces:**
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq, Serialize)] pub struct Cleanup { pub warn_after_hours: u32, pub clean_after_hours: u32, pub auto_clean: bool }`, with `Default` = 3, 24, true.
  - `pub const WARN_CHOICES: [u32; 4] = [1, 3, 6, 12];` and `pub const CLEAN_CHOICES: [u32; 4] = [12, 24, 48, 72];`
  - `Settings::cleanup(&self) -> Cleanup` and `Settings::set_cleanup(&self, c: Cleanup)`. Both apply `clean_after_hours = max(clean_after_hours, warn_after_hours)`. A value of 0 is a bad value, and that field falls back to its default.
  - File shape: `"cleanup": {"warn_after_hours": 3, "clean_after_hours": 24, "auto_clean": true}`.

- [ ] **Step 1: Write the failing tests** (settings `tests` module; reuse `state()` and `temp_dir()`)

```rust
#[test]
fn cleanup_defaults_and_round_trip() {
    assert_eq!(state("{}").cleanup, Cleanup { warn_after_hours: 3, clean_after_hours: 24, auto_clean: true });
    let dir = temp_dir("cleanup-round-trip");
    let s = Settings::load(&dir);
    s.set_cleanup(Cleanup { warn_after_hours: 6, clean_after_hours: 48, auto_clean: false });
    assert_eq!(Settings::load(&dir).cleanup(),
        Cleanup { warn_after_hours: 6, clean_after_hours: 48, auto_clean: false });
}

#[test]
fn clean_after_never_below_warn_after() {
    let c = state(r#"{"cleanup": {"warn_after_hours": 12, "clean_after_hours": 2}}"#).cleanup;
    assert_eq!((c.warn_after_hours, c.clean_after_hours), (12, 12));
    let c = state(r#"{"cleanup": {"warn_after_hours": 0, "clean_after_hours": "x", "auto_clean": 1}}"#).cleanup;
    assert_eq!(c, Cleanup::default());
}
```

Also extend `every_section_round_trips` and `a_save_keeps_other_instances_sections_and_unknown_keys` so they cover `cleanup`.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p agentpulse-app settings::`
Expected: compile errors for `Cleanup` and `set_cleanup`.

- [ ] **Step 3: Implement the interfaces above,** following the `animation` section's pattern (`field()` per value, `Part::Cleanup`). Add the section to the module doc's JSON example.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p agentpulse-app settings::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agentpulse-app/src/settings.rs
git commit -m "settings: cleanup section (warn after, clean after, auto clean)"
```

---

### Task 4: Lineage and sweep logic (`cleanup.rs`)

**Files:**
- Create: `crates/agentpulse-app/src/cleanup.rs`
- Modify: `crates/agentpulse-app/src/main.rs` (`mod cleanup;`)

**Interfaces:**
- Consumes: Task 1 (`roots_of`, `ProcDetails`, `Signal`, `Pressure`), Task 2 (`TrackedProcess`, `CleanupLogEntry`, `Store` methods), Task 3 (`settings::Cleanup`).
- Produces:
  ```rust
  pub const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
  pub const WARN_LEAD_MS: i64 = 3_600_000;
  pub const SWAP_SUGGEST_BYTES: u64 = 2 << 30;
  pub const SUGGEST_REPEAT_MS: i64 = 6 * 3_600_000;
  pub const LOG_SHOWN: usize = 20;

  /// The OS side, faked in tests. Object-safe: passed as `&dyn Host` / `&mut dyn Host`.
  pub trait Host {
      fn details(&self, p: ProcessIdentity) -> Option<ProcDetails>;
      fn signal(&mut self, p: ProcessIdentity, s: Signal) -> bool; // false: gone, reused or failed (logged)
      fn notify(&mut self, title: &str, body: &str);
      fn memory(&self) -> (Option<Pressure>, Option<u64>);         // pressure, swap used
      fn my_uid(&self) -> u32;
  }
  pub struct SystemHost; // impl Host via agentpulse_process + agentpulse_platform_macos::notify (Task 6)

  #[derive(Clone, Debug, PartialEq, serde::Serialize)]
  pub enum Reason {
      Orphan { provider: Provider, session_id: String },
      IdleSession { provider: Provider, session_id: String, waiting: bool },
      Untracked,
  }
  #[derive(Clone, Debug, PartialEq, serde::Serialize)]
  pub struct CleanupItem {
      pub process: Option<ProcessIdentity>, // None: shared process, cannot be ended here
      pub name: String,
      pub reason: Reason,
      pub rss_bytes: u64,
      pub since_ms: i64,               // agent ended (orphan) or last activity (idle); 0 for untracked
      pub warned: bool,
      pub clean_at_ms: Option<i64>,    // orphans with auto clean on: when it will be ended
  }
  #[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
  pub struct CleanupView { pub items: Vec<CleanupItem>, pub log: Vec<CleanupLogEntry>, pub memory_low: bool }

  pub struct Tracker { /* rows: HashMap<ProcessIdentity, TrackedProcess>, warned_sessions: HashSet<(Provider, String)>, last_suggest_ms: Option<i64> */ }
  impl Tracker {
      pub fn load(store: &Store) -> anyhow::Result<Self>;
      /// After each process sample with live agents. Records new descendants of `roots`
      /// (one `details()` call per new process), re-adopts tracked rows that are now under a
      /// live root (clears ended, warned and term times), and writes only changed rows.
      pub fn observe(&mut self, store: &mut Store, table: &ProcTable,
          roots: &[(ProcessIdentity, Provider, String)], host: &dyn Host) -> anyhow::Result<()>;
      /// Once per SWEEP_INTERVAL. Order: delete rows whose process is gone; set ended_at = now
      /// on rows whose root is gone; send SIGKILL where term_sent_at_ms is set; warn; clean;
      /// warn idle sessions; memory suggestion; build the view.
      pub fn sweep(&mut self, store: &mut Store, table: &ProcTable, sessions: &[AgentSession],
          cfg: &Cleanup, now_ms: i64, host: &mut dyn Host) -> anyhow::Result<CleanupView>;
      /// The user's End: SIGTERM now and a log entry with automatic = false. A tracked row
      /// gets term_sent_at_ms, so the next sweep can send SIGKILL.
      pub fn end(&mut self, store: &mut Store, p: ProcessIdentity, table: &ProcTable,
          now_ms: i64, host: &mut dyn Host) -> anyhow::Result<()>;
      /// The user's Keep: kept = true, so this process is never warned about or cleaned again.
      pub fn keep(&mut self, store: &mut Store, p: ProcessIdentity) -> anyhow::Result<()>;
  }
  ```
- Rules (from the spec):
  - **Warn** an orphan when it is not kept, `warned_at_ms` is None and `now - ended_at >= warn_after`. Notification title "Leftover process", body "`<name>` from a finished `<agent label>` session is still running." Then set `warned_at_ms`.
  - **Clean** when `auto_clean`, not kept, `term_sent_at_ms` is None, `now - ended_at >= clean_after` and `now - warned_at >= WARN_LEAD_MS`. Send `Signal::Term`, set `term_sent_at_ms`, log with `automatic = true` and the RSS from `table`. Then notify "Ended leftover process": "`<name>` (`<MB>` MB) was ended after `<n>` hours."
  - **Idle sessions:** Idle or Waiting `AgentSession` with `now - last_activity_at_ms >= warn_after`. Notify once per session key, never signal. `CleanupItem.process` is `session.process`, which is None for a shared process.
  - **Untracked:** `table` entries with `ppid == Some(1)`, not tracked, `details().uid == my_uid()`, and a name rule from "Calls this plan makes". Listed only, never warned about or cleaned.
  - **Memory suggestion:** `memory_low = pressure ∈ {Warn, Critical} || swap ≥ SWAP_SUGGEST_BYTES`. Notify "Memory is low", "Open Clean-up in AgentPulse to end leftover processes." at most once per `SUGGEST_REPEAT_MS`.
  - **View:** `items` holds orphans, then idle sessions, then untracked, each group biggest RSS first. `log = store.cleanup_log(LOG_SHOWN)`.

- [ ] **Step 1: Write the failing tests** (in `cleanup.rs`). Use a `FakeHost` that records signals and notifications, returns `details` from a map and a fixed `memory()`. Use a `table(&[(pid, ppid, start, rss)])` helper and `H = 3_600_000`. Each test runs on `Store::open_in_memory()`.

```rust
#[test] fn orphan_is_warned_then_cleaned_then_killed() {
    // root 10 (live) with child 11. Observe; then the next table has 11 under ppid 1 and no 10.
    // sweep at t0: ended_at = t0, no notification.
    // sweep at t0+3H: one "Leftover process" notification, no signal.
    // sweep at t0+24H: signals == [(11, Term)], one log entry with automatic == true.
    // sweep at t0+24H+60s with 11 still present: signals ends with (11, Kill).
}
#[test] fn ignored_term_gets_kill_next_sweep() { /* as above; once 11 is gone from the table, no further signal and the row is deleted */ }
#[test] fn late_start_warns_before_cleaning() {
    // Save a row with ended_at_ms = t0 and warned_at_ms = None, then Tracker::load.
    // sweep at t0+30H: warned, signals empty. sweep at t0+30H+59min: signals empty.
    // sweep at t0+31H: signals == [(11, Term)].
}
#[test] fn orphan_gone_with_agent_is_dropped() { /* the table after observe has neither 10 nor 11: row deleted, never ended, never signalled */ }
#[test] fn orphan_under_new_agent_is_readopted() { /* orphaned and warned 11 now under new live root 20: ended, warned and term are None; no signal at +24H */ }
#[test] fn kept_process_is_never_warned_or_cleaned() { /* keep(11) after orphaning: no notifications or signals through +48H */ }
#[test] fn auto_clean_off_only_warns() { /* cfg.auto_clean = false: warned at +3H, no signal at +48H, item.clean_at_ms == None */ }
#[test] fn reused_pid_is_never_signalled() { /* the table at +24H has pid 11 with a different start: row deleted, FakeHost.signals empty */ }
#[test] fn agent_sessions_are_never_signalled() {
    // Idle, Waiting and Active sessions with process Some(..) and last activity 48H ago:
    // Idle and Waiting are notified once each (a second sweep adds none), Active is not; signals empty;
    // the items carry Reason::IdleSession with waiting set correctly.
}
#[test] fn shared_idle_session_has_no_process() { /* session.process == None: item.process == None */ }
#[test] fn untracked_leftovers_are_listed_not_touched() {
    // ppid-1 processes named "node", "Python", "spline-mcp" and "bash", plus a "node" owned by another uid:
    // items are the first three as Reason::Untracked; no signal or notification at any time.
}
#[test] fn manual_end_logs_and_follows_up_with_kill() { /* end(untracked p): Term, log automatic == false; end(tracked) then sweep +60s while alive: Kill */ }
#[test] fn memory_suggestion_repeats_at_most_every_six_hours() { /* FakeHost memory (Some(Warn), None): notified at t0, not at t0+1H, again at t0+6H; view.memory_low true */ }
```

Write each test body in full, with the assertions given in its comment.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p agentpulse-app cleanup::`
Expected: compile errors (module empty).

- [ ] **Step 3: Implement `cleanup.rs` to the interfaces and rules above.** `SystemHost` can call `tracing::info!` in `notify` until Task 6 wires the real notifications.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p agentpulse-app cleanup::`
Expected: PASS (13 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/agentpulse-app/src/cleanup.rs crates/agentpulse-app/src/main.rs
git commit -m "cleanup: lineage tracking and the warn/clean sweep"
```

---

### Task 5: Wire the tracker into the collector

**Files:**
- Modify: `crates/agentpulse-app/src/collector.rs` (`Collector`, `tick`, `Snapshot`, `spawn`, tests)
- Modify: `crates/agentpulse-app/src/main.rs` (`gui`, `dump`, `bench` call sites)
- Modify: `crates/agentpulse-app/src/ui.rs` (`Shared.details` and `run` take `Sender<Command>`; `show_detail` sends `Command::Detail`)

**Interfaces:**
- Consumes: Task 4 (`Tracker`, `Host`, `SystemHost`, `CleanupView`, `SWEEP_INTERVAL`), Task 3 (`Settings::cleanup`).
- Produces:
  - `pub enum Command { Detail(Option<Key>), End(ProcessIdentity), Keep(ProcessIdentity) }` in `collector.rs`. It replaces `Receiver<Option<Key>>` in `spawn`. Only the newest of several queued `Detail` commands is applied, as today; every `End` and `Keep` is applied.
  - `Snapshot.cleanup: CleanupView` (serialized, so it shows in `--dump`).
  - `Collector::tick_with(&mut self, cleanup: Option<(&Cleanup, &mut dyn Host)>) -> Snapshot`. With None, observe and sweep are skipped and `snap.cleanup` is empty. `tick()` is `tick_with(None)`, so `--dump` never signals or notifies. `--bench` uses `tick_with(Some((&Cleanup { auto_clean: false, ..Cleanup::default() }, &mut SystemHost)))`, so the overhead it measures includes clean-up but nothing is ever ended.
  - `Collector::command(&mut self, c: Command, host: &mut dyn Host)`
  - `spawn(collector, sender, commands: Receiver<Command>, settings: Settings, widgets, phones)`. It reads `settings.cleanup()` each tick.
- Behaviour:
  - After each new process sample (the `self.table = Some(...)` branch), call `tracker.observe` with the verified, non-shared roots `(ProcessIdentity, Provider, session_id)`.
  - When `SWEEP_INTERVAL` has passed since the last sweep, call `tracker.sweep` with the current table. When no agent is live (`self.table == None`), use a one-off `Sampler::new().sample()`. Keep the last view in `self.cleanup` and copy it into every snapshot.
  - Tracker errors go into `snap.errors` as `"Clean-up: {e:#}"`, like the other storage errors.

- [ ] **Step 1: Write the failing test** (collector `tests` module, using the existing `collector()` and `Fake`)

```rust
#[test]
fn sweep_runs_without_live_agents_and_fills_the_snapshot() {
    let (mut c, _fake) = collector();
    let mut host = cleanup::tests::FakeHost::default(); // make FakeHost pub(crate) in Task 4's tests module
    let snap = c.tick_with(Some((&Cleanup::default(), &mut host)));
    assert!(snap.cleanup.items.iter().all(|i| i.reason == cleanup::Reason::Untracked));
    assert!(snap.errors.is_empty());
    // Without clean-up (as --dump runs), nothing is swept.
    assert_eq!(c.tick().cleanup, CleanupView::default());
}
```

- [ ] **Step 2: Run it and see it fail**

Run: `cargo test -p agentpulse-app collector::`
Expected: compile errors for `tick_with` and `Snapshot.cleanup`.

- [ ] **Step 3: Implement the wiring above.** Update `main.rs` and `ui.rs` to the new `spawn` and `run` signatures.

- [ ] **Step 4: Run the whole workspace and the bench**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.
Run: `cargo run --release -p agentpulse-app -- --bench` (update `bench()` to `tick_with` as above first)
Expected: collector CPU about 1% of one core, as in `docs/roadmap.md` (1.05% with three live agents). Note the figure in the commit message.

- [ ] **Step 5: Commit**

```bash
git add crates/agentpulse-app/src/collector.rs crates/agentpulse-app/src/main.rs crates/agentpulse-app/src/ui.rs
git commit -m "collector: track agent children, sweep for orphans, Command channel"
```

---

### Task 6: macOS notifications

**Files:**
- Create: `crates/agentpulse-platform-macos/src/notify.rs`
- Modify: `crates/agentpulse-platform-macos/src/lib.rs` (`mod notify; pub use notify::notify;`)
- Modify: `crates/agentpulse-platform-macos/Cargo.toml`. Add `objc2-user-notifications = { version = "0.3.2", default-features = false, features = ["std", "block2", "UNUserNotificationCenter", "UNNotificationRequest", "UNNotificationContent", "UNNotificationTrigger"] }`, plus the `NSBundle` feature on `objc2-foundation`.
- Modify: `crates/agentpulse-app/src/cleanup.rs` (`SystemHost::notify` calls it)

**Interfaces:**
- Produces: `pub fn notify(title: &str, body: &str)`. When `NSBundle::mainBundle().bundleIdentifier()` is None, it does nothing beyond `tracing::info!(title, body, "notification (no app bundle)")`. On the first call it requests `.alert | .sound` authorization once (kept in a `OnceLock`). Each call adds a `UNNotificationRequest` with a fresh identifier and no trigger. Errors are logged at warn level, never returned.

- [ ] **Step 1: Write a failing test** in `notify.rs`: `notify_without_a_bundle_is_a_no_op` calls `notify("t", "b")` from `cargo test` (no bundle) and must return without panicking.

- [ ] **Step 2: Run it and see it fail**

Run: `cargo test -p agentpulse-platform-macos notify`
Expected: compile error (`notify` not defined).

- [ ] **Step 3: Implement `notify`, and switch `SystemHost::notify` to it.**

- [ ] **Step 4: Verify**

Run: `cargo test --workspace`
Expected: PASS.
Run: `scripts/bundle.sh`, then open the app. Temporarily set `"cleanup": {"warn_after_hours": 1, ...}` and drive the test case from Task 8. Expected: macOS asks for notification permission once, then a "Leftover process" notification appears.

- [ ] **Step 5: Commit**

```bash
git add crates/agentpulse-platform-macos crates/agentpulse-app/src/cleanup.rs Cargo.lock
git commit -m "platform-macos: notifications through UserNotifications"
```

---

### Task 7: The Clean-up screen and settings card

**Files:**
- Create: `crates/agentpulse-app/src/ui/cleanup.rs`
- Modify: `crates/agentpulse-app/src/ui.rs` (`Screen::Cleanup`, a nav item "Clean-up" between Usage and Cats, the content match arm, a `Command` sender)
- Modify: `crates/agentpulse-app/src/ui/settings.rs` (`cleanup_card` after `animation_card`)
- Modify: `crates/agentpulse-app/src/hub.rs` (`set_cleanup(&mut self, c: Cleanup, cx)` calls `settings.set_cleanup` and notifies)

**Interfaces:**
- Consumes: `Snapshot.cleanup` (Task 5), `Command` (Task 5), `Cleanup`, `WARN_CHOICES` and `CLEAN_CHOICES` (Task 3).
- Produces: `pub(super) fn screen(view: &CleanupView, now_ms: i64, t: Theme, entity: &Entity<AppView>, scroll: &ScrollHandle) -> impl IntoElement` in `ui/cleanup.rs`.
- What it shows (follow DESIGN.md and the existing card, `row`, `button` and `secondary` helpers):
  - With `memory_low`: a line at the top, "Memory is low. Ending leftover processes can free some."
  - Three groups: "Leftover processes", "Idle sessions", "Other leftovers". Each row shows the name, why it is here ("From Codex · <project or session>", "Idle for 5 h", "Waiting on you for 5 h", "Not started by an agent AgentPulse saw"), memory, and for orphans "Ends in 3 h" or "Ends automatically is off".
  - Buttons: **End**, which needs a second click on "Confirm end" within 5 s (the row keeps that state in `AppView`), and **Keep** for orphans only. A row with `process == None` shows "Shared with other sessions" and no End button.
  - A "Recently ended" list from `log`: name, memory, "automatically" or "by you", and relative time.
  - Empty state: "Nothing left behind."
  - Settings card "Clean-up": radio groups "Warn after" (WARN_CHOICES, shown as "1 h") and "End after" (CLEAN_CHOICES), styled like the Speed group. An "End leftover processes automatically" switch. Below them, the secondary text "AgentPulse only ends processes it saw an agent start. Idle sessions are never ended for you."

- [ ] **Step 1: Write the failing test** in `ui/cleanup.rs` for the pure text helper `pub(super) fn why(item: &CleanupItem, now_ms: i64) -> String`, covering each `Reason`:

```rust
#[test]
fn why_reads_plainly() {
    let h = 3_600_000;
    assert_eq!(why(&item(Reason::IdleSession { provider: Provider::Codex, session_id: "s".into(), waiting: true }, 0), 5 * h), "Waiting on you for 5 h");
    assert_eq!(why(&item(Reason::Untracked, 0), 0), "Not started by an agent AgentPulse saw");
}
```

- [ ] **Step 2: Run it and see it fail**

Run: `cargo test -p agentpulse-app ui::cleanup`
Expected: compile errors.

- [ ] **Step 3: Implement the screen, the nav item, the settings card and `Hub::set_cleanup`.**

- [ ] **Step 4: Verify**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.
Run: `cargo run -p agentpulse-app --features screenshot -- --screenshot /tmp/ap-shots`, and check the Settings image for the Clean-up card. Then run `cargo run -p agentpulse-app`, open Clean-up, and check light and dark mode and the minimum window size (880 × 560). Expected: no sideways scroll, and End needs a confirmation.

- [ ] **Step 5: Commit**

```bash
git add crates/agentpulse-app/src/ui.rs crates/agentpulse-app/src/ui crates/agentpulse-app/src/hub.rs
git commit -m "ui: Clean-up screen and settings card"
```

---

### Task 8: Real-Mac check and docs

**Files:**
- Modify: `docs/roadmap.md` (a "Done: orphan clean-up" entry under Phase 3, with the bench figure)
- Modify: `README.md` (one line on the Clean-up screen in the screen list)

- [ ] **Step 1: The spec's manual check.** In `settings.json` set `"cleanup": {"warn_after_hours": 1, "clean_after_hours": 1, "auto_clean": true}`. For a minutes-scale run, temporarily build with the hour constant scaled down, and do not commit that. Start Claude Code and have it run `python3 -m http.server 8765 &`. Quit Claude Code. Expected:
  - Clean-up lists `python3` under "Leftover processes".
  - A warning notification appears, then an automatic end.
  - `lsof -i :8765` is empty afterwards.
  - "Recently ended" shows the entry.
- [ ] **Step 2: Check that Keep survives a restart.** Repeat with Keep pressed, quit and relaunch AgentPulse. Expected: the process is still listed as kept and is never ended.
- [ ] **Step 3: Update the docs and commit**

```bash
git add docs/roadmap.md README.md
git commit -m "docs: orphan clean-up in roadmap and README"
```
