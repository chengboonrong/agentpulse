//! The processes each agent started, and the sweep that warns about and ends
//! the ones left running after their agent is gone.
//!
//! Only tracked orphans (not kept, with their agent gone) are ever signalled
//! automatically, and only after a warning at least `WARN_LEAD_MS` old. Agent
//! sessions and untracked leftovers are only listed; the user ends them.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use agentpulse_core::{AgentSession, ProcessIdentity, Provider, SessionStatus};
use agentpulse_process::{Pressure, ProcDetails, ProcInfo, ProcTable, Signal};
use agentpulse_storage::{CleanupLogEntry, Store, TrackedProcess};
use serde::Serialize;

use crate::format;
use crate::settings::Cleanup;

pub const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
pub const WARN_LEAD_MS: i64 = 3_600_000;
pub const SWAP_SUGGEST_BYTES: u64 = 2 << 30;
pub const SUGGEST_REPEAT_MS: i64 = 6 * 3_600_000;
pub const LOG_SHOWN: usize = 20;

const HOUR_MS: i64 = 3_600_000;
/// SIGKILL follows SIGTERM after one sweep.
const KILL_GRACE_MS: i64 = SWEEP_INTERVAL.as_millis() as i64;

/// The OS side, faked in tests. Object-safe: passed as `&dyn Host` / `&mut dyn Host`.
pub trait Host {
    fn details(&self, p: ProcessIdentity) -> Option<ProcDetails>;
    /// False: gone, reused or failed (logged).
    fn signal(&mut self, p: ProcessIdentity, s: Signal) -> bool;
    fn notify(&mut self, title: &str, body: &str);
    /// Memory pressure and swap used.
    fn memory(&self) -> (Option<Pressure>, Option<u64>);
    fn my_uid(&self) -> u32;
}

#[derive(Default)]
pub struct SystemHost {
    /// Processes whose signal failure was already logged, so a process that
    /// keeps failing writes one warning, not one per sweep.
    failed: HashSet<ProcessIdentity>,
}

impl Host for SystemHost {
    fn details(&self, p: ProcessIdentity) -> Option<ProcDetails> {
        agentpulse_process::details(p)
    }

    fn signal(&mut self, p: ProcessIdentity, s: Signal) -> bool {
        agentpulse_process::signal(p, s).unwrap_or_else(|e| {
            if self.failed.insert(p) {
                tracing::warn!(pid = p.pid, signal = ?s, "could not signal process: {e}");
            }
            false
        })
    }

    fn notify(&mut self, title: &str, body: &str) {
        agentpulse_platform_macos::notify(title, body);
    }

    fn memory(&self) -> (Option<Pressure>, Option<u64>) {
        (
            agentpulse_process::memory_pressure(),
            agentpulse_process::swap_used_bytes(),
        )
    }

    fn my_uid(&self) -> u32 {
        // SAFETY: getuid has no preconditions and cannot fail.
        unsafe { libc::getuid() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Reason {
    Orphan {
        provider: Provider,
        session_id: String,
    },
    IdleSession {
        provider: Provider,
        session_id: String,
        waiting: bool,
    },
    Untracked,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CleanupItem {
    /// None: a shared process, which cannot be ended from here.
    pub process: Option<ProcessIdentity>,
    pub name: String,
    pub reason: Reason,
    pub rss_bytes: u64,
    /// When the agent ended (orphan) or last activity (idle session); 0 for untracked.
    pub since_ms: i64,
    pub warned: bool,
    /// Orphans with auto clean on: when it will be ended.
    pub clean_at_ms: Option<i64>,
    /// An orphan the user kept: never warned about or ended. False for other rows.
    pub kept: bool,
    /// SIGTERM sent and the process still alive: it cannot be ended or kept again.
    pub ending: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CleanupView {
    pub items: Vec<CleanupItem>,
    pub log: Vec<CleanupLogEntry>,
    pub memory_low: bool,
}

pub struct Tracker {
    rows: HashMap<ProcessIdentity, TrackedProcess>,
    /// In memory only, so idle warnings can repeat after a restart.
    warned_sessions: HashSet<(Provider, String)>,
    last_suggest_ms: Option<i64>,
    /// Rows changed in memory but not yet saved. Retried by every write, so a
    /// failed save is not lost.
    dirty: HashSet<ProcessIdentity>,
    /// Log entries for signals already sent but not yet saved. A sweep saves
    /// them before it sends any SIGKILL, so no process is ended unlogged.
    unlogged: Vec<CleanupLogEntry>,
    /// Untracked and idle-session processes the user ended, with their SIGTERM
    /// time, until they are gone. In memory only.
    ending: HashMap<ProcessIdentity, i64>,
}

/// `p`'s entry in `table`, if the PID still belongs to that process.
fn alive(table: &ProcTable, p: ProcessIdentity) -> Option<&ProcInfo> {
    table
        .procs
        .get(&p.pid)
        .filter(|i| i.start_time_s == p.start_time_s)
}

/// "a", "a and b", "a, b and c", "a, b, c and 2 more", in name order.
fn names(mut v: Vec<String>) -> String {
    v.sort();
    match v.len() {
        n @ 4.. => format!("{} and {} more", v[..3].join(", "), n - 3),
        n @ 2.. => format!("{} and {}", v[..n - 1].join(", "), v[n - 1]),
        _ => v.concat(),
    }
}

/// Untracked candidates: `node*`, `python*` and Spline processes.
fn leftover_name(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with("node") || n.starts_with("python") || n.contains("spline")
}

impl Tracker {
    pub fn load(store: &Store) -> anyhow::Result<Self> {
        Ok(Self {
            rows: store
                .tracked_processes()?
                .into_iter()
                .map(|r| (r.process, r))
                .collect(),
            warned_sessions: HashSet::new(),
            last_suggest_ms: None,
            dirty: HashSet::new(),
            unlogged: Vec::new(),
            ending: HashMap::new(),
        })
    }

    /// Saves the pending log entries, then the dirty rows. On an error both stay
    /// pending for the next call. Log entries go first: a saved term_sent_at_ms
    /// without its entry would let a sweep after a restart send an unlogged SIGKILL.
    fn flush(&mut self, store: &mut Store) -> anyhow::Result<()> {
        while let Some(e) = self.unlogged.first() {
            store.log_cleanup(e)?;
            self.unlogged.remove(0);
        }
        if !self.dirty.is_empty() {
            let rows: Vec<TrackedProcess> = self
                .dirty
                .iter()
                .filter_map(|p| self.rows.get(p).cloned())
                .collect();
            store.save_tracked(&rows)?;
            self.dirty.clear();
        }
        Ok(())
    }

    /// After each process sample with live agents. Records new descendants of `roots`
    /// (one `details()` call per new process), re-adopts tracked rows that are now under a
    /// live root (clears ended, warned and term times), and writes only changed rows.
    pub fn observe(
        &mut self,
        store: &mut Store,
        table: &ProcTable,
        roots: &[(ProcessIdentity, Provider, String)],
        host: &dyn Host,
    ) -> anyhow::Result<()> {
        let ids: Vec<ProcessIdentity> = roots.iter().map(|r| r.0).collect();
        let mut by_pid = HashMap::new();
        for r in roots {
            by_pid.entry(r.0.pid).or_insert(r);
        }
        for (pid, root_pid) in agentpulse_process::roots_of(table, &ids) {
            // Agent processes themselves are not tracked.
            if pid == root_pid {
                continue;
            }
            let (root, provider, session_id) = by_pid[&root_pid];
            let process = ProcessIdentity {
                pid,
                start_time_s: table.procs[&pid].start_time_s,
            };
            if let Some(row) = self.rows.get_mut(&process) {
                let same = row.root == *root
                    && row.provider == *provider
                    && row.session_id == *session_id
                    && row.ended_at_ms.is_none()
                    && row.warned_at_ms.is_none()
                    && row.term_sent_at_ms.is_none();
                if !same {
                    row.root = *root;
                    row.provider = *provider;
                    row.session_id.clone_from(session_id);
                    row.ended_at_ms = None;
                    row.warned_at_ms = None;
                    row.term_sent_at_ms = None;
                    self.dirty.insert(process);
                }
                continue;
            }
            let Some(d) = host.details(process) else {
                continue;
            };
            let row = TrackedProcess {
                process,
                root: *root,
                provider: *provider,
                session_id: session_id.clone(),
                name: d.name,
                ended_at_ms: None,
                warned_at_ms: None,
                term_sent_at_ms: None,
                kept: false,
            };
            self.rows.insert(process, row);
            self.dirty.insert(process);
        }
        self.flush(store)
    }

    /// Once per SWEEP_INTERVAL. Order: retry unsaved writes; delete rows whose process is gone;
    /// set ended_at = now on rows whose root is gone; send SIGKILL where term_sent_at_ms is set
    /// and to other processes the user ended; warn; clean; warn idle sessions; memory
    /// suggestion; build the view. None: skipped, because the table does not hold
    /// AgentPulse itself (a failed sample), so it cannot say what is gone.
    pub fn sweep(
        &mut self,
        store: &mut Store,
        table: &ProcTable,
        sessions: &[AgentSession],
        cfg: &Cleanup,
        now_ms: i64,
        host: &mut dyn Host,
    ) -> anyhow::Result<Option<CleanupView>> {
        if !table.procs.contains_key(&std::process::id()) {
            return Ok(None);
        }
        let warn_ms = i64::from(cfg.warn_after_hours) * HOUR_MS;
        let clean_ms = i64::from(cfg.clean_after_hours) * HOUR_MS;
        let agents: HashSet<ProcessIdentity> = sessions
            .iter()
            .filter(|s| s.status.is_live())
            .filter_map(|s| s.process)
            .collect();

        // Writes that failed earlier go first: no SIGKILL below until every
        // SIGTERM before it is logged.
        self.flush(store)?;
        // Nothing is pending now, so the store holds every change, including
        // those of another AgentPulse on the same database (a Keep, say).
        self.rows = store
            .tracked_processes()?
            .into_iter()
            .map(|r| (r.process, r))
            .collect();

        // First, so a child that exited with its agent is never marked ended.
        // A row that is now a live agent is dropped too: agents are never cleaned.
        let gone: Vec<ProcessIdentity> = self
            .rows
            .keys()
            .filter(|&&p| alive(table, p).is_none() || agents.contains(&p))
            .copied()
            .collect();
        if !gone.is_empty() {
            store.delete_tracked(&gone)?;
            for p in &gone {
                self.rows.remove(p);
                self.dirty.remove(p);
            }
        }

        for row in self.rows.values_mut() {
            if row.ended_at_ms.is_none() && alive(table, row.root).is_none() {
                row.ended_at_ms = Some(now_ms);
                self.dirty.insert(row.process);
            }
        }

        // term_sent_at_ms is set only by a SIGTERM that was delivered.
        for row in self.rows.values() {
            if !row.kept
                && row
                    .term_sent_at_ms
                    .is_some_and(|t| now_ms - t >= KILL_GRACE_MS)
            {
                host.signal(row.process, Signal::Kill);
            }
        }
        self.ending.retain(|&p, _| alive(table, p).is_some());
        for (&p, &t) in &self.ending {
            if now_ms - t >= KILL_GRACE_MS {
                host.signal(p, Signal::Kill);
            }
        }

        // One notification per kind per sweep: (single-item body, name).
        let mut warned = Vec::new();
        for row in self.rows.values_mut() {
            let Some(ended) = row.ended_at_ms else {
                continue;
            };
            if row.kept
                || row.warned_at_ms.is_some()
                || row.term_sent_at_ms.is_some()
                || now_ms - ended < warn_ms
            {
                continue;
            }
            let body = format!(
                "{} from a finished {} session is still running.",
                row.name,
                row.provider.label()
            );
            warned.push((body, row.name.clone()));
            row.warned_at_ms = Some(now_ms);
            self.dirty.insert(row.process);
        }
        match warned.len() {
            0 => {}
            1 => host.notify("Leftover process", &warned[0].0),
            n => host.notify(
                &format!("{n} leftover processes"),
                &format!(
                    "{} from finished agent sessions are still running.",
                    names(warned.into_iter().map(|w| w.1).collect())
                ),
            ),
        }

        // (single-item body, name, rss, hours since the agent ended).
        let mut cleaned = Vec::new();
        if cfg.auto_clean {
            for row in self.rows.values_mut() {
                let (Some(ended), Some(warned)) = (row.ended_at_ms, row.warned_at_ms) else {
                    continue;
                };
                // The root check guards against a sample that missed a live agent.
                if row.kept
                    || row.term_sent_at_ms.is_some()
                    || now_ms - ended < clean_ms
                    || now_ms - warned < WARN_LEAD_MS
                    || alive(table, row.root).is_some()
                {
                    continue;
                }
                // A failed SIGTERM leaves the row as it was, so the next sweep retries it.
                if !host.signal(row.process, Signal::Term) {
                    continue;
                }
                row.term_sent_at_ms = Some(now_ms);
                self.dirty.insert(row.process);
                let rss = alive(table, row.process).map_or(0, |i| i.rss_bytes);
                self.unlogged.push(CleanupLogEntry {
                    at_ms: now_ms,
                    name: row.name.clone(),
                    provider: Some(row.provider),
                    session_id: Some(row.session_id.clone()),
                    rss_bytes: rss,
                    automatic: true,
                });
                let hours = (now_ms - ended) / HOUR_MS;
                let body = format!(
                    "{} ({} MB) was ended after {hours} hours.",
                    row.name,
                    rss >> 20
                );
                cleaned.push((body, row.name.clone(), rss, hours));
            }
        }
        match cleaned.len() {
            0 => {}
            1 => host.notify("Ended leftover process", &cleaned[0].0),
            n => {
                let rss: u64 = cleaned.iter().map(|c| c.2).sum();
                let hours = cleaned.iter().map(|c| c.3).min().unwrap_or(0);
                let body = format!(
                    "{} were ended after {hours} hours, freeing {} MB.",
                    names(cleaned.into_iter().map(|c| c.1).collect()),
                    rss >> 20
                );
                host.notify(&format!("Ended {n} leftover processes"), &body);
            }
        }

        self.flush(store)?;

        let mut idle = Vec::new();
        // (single-item body, "Codex in project", hours since last activity).
        let mut idle_new = Vec::new();
        for s in sessions {
            let waiting = match s.status {
                SessionStatus::Idle => false,
                SessionStatus::Waiting => true,
                _ => continue,
            };
            let Some(last) = s.last_activity_at_ms.filter(|&l| now_ms - l >= warn_ms) else {
                continue;
            };
            let label = s.provider.label();
            let project = format::project_name(s.project_path.as_deref());
            if self
                .warned_sessions
                .insert((s.provider, s.session_id.clone()))
            {
                let state = if waiting { "waiting on you" } else { "idle" };
                let hours = (now_ms - last) / HOUR_MS;
                let body =
                    format!("{label} session in {project} has been {state} for {hours} hours.");
                idle_new.push((body, format!("{label} in {project}"), hours));
            }
            idle.push(CleanupItem {
                process: s.process,
                name: format!("{label} · {project}"),
                reason: Reason::IdleSession {
                    provider: s.provider,
                    session_id: s.session_id.clone(),
                    waiting,
                },
                rss_bytes: s.resources.value().map_or(0, |r| r.rss_bytes),
                since_ms: last,
                warned: true,
                clean_at_ms: None,
                kept: false,
                ending: s.process.is_some_and(|p| self.ending.contains_key(&p)),
            });
        }

        match idle_new.len() {
            0 => {}
            1 => host.notify("Idle session", &idle_new[0].0),
            n => {
                let hours = idle_new.iter().map(|i| i.2).min().unwrap_or(0);
                let body = format!(
                    "{} have had no activity for {hours} hours or more.",
                    names(idle_new.into_iter().map(|i| i.1).collect())
                );
                host.notify(&format!("{n} idle sessions"), &body);
            }
        }

        let uid = host.my_uid();
        let mut untracked = Vec::new();
        for (&pid, info) in &table.procs {
            let p = ProcessIdentity {
                pid,
                start_time_s: info.start_time_s,
            };
            if info.ppid != Some(1) || self.rows.contains_key(&p) || agents.contains(&p) {
                continue;
            }
            let Some(d) = host
                .details(p)
                .filter(|d| d.uid == uid && leftover_name(&d.name))
            else {
                continue;
            };
            untracked.push(CleanupItem {
                process: Some(p),
                name: d.name,
                reason: Reason::Untracked,
                rss_bytes: info.rss_bytes,
                since_ms: 0,
                warned: false,
                clean_at_ms: None,
                kept: false,
                ending: self.ending.contains_key(&p),
            });
        }

        let (pressure, swap) = host.memory();
        let memory_low = matches!(pressure, Some(Pressure::Warn | Pressure::Critical))
            || swap.is_some_and(|s| s >= SWAP_SUGGEST_BYTES);
        if memory_low
            && self
                .last_suggest_ms
                .is_none_or(|t| now_ms - t >= SUGGEST_REPEAT_MS)
        {
            host.notify(
                "Memory is low",
                "Open Clean-up in AgentPulse to end leftover processes.",
            );
            self.last_suggest_ms = Some(now_ms);
        }

        let mut orphans: Vec<CleanupItem> = self
            .rows
            .values()
            .filter_map(|row| {
                let ended = row.ended_at_ms?;
                // Not yet warned: the warning comes at the next sweep past
                // warn_after, and the clean at least WARN_LEAD_MS after it.
                let clean_at = (cfg.auto_clean && !row.kept).then(|| {
                    let warned = row
                        .warned_at_ms
                        .unwrap_or_else(|| now_ms.max(ended + warn_ms));
                    (ended + clean_ms).max(warned + WARN_LEAD_MS)
                });
                Some(CleanupItem {
                    process: Some(row.process),
                    name: row.name.clone(),
                    reason: Reason::Orphan {
                        provider: row.provider,
                        session_id: row.session_id.clone(),
                    },
                    rss_bytes: alive(table, row.process).map_or(0, |i| i.rss_bytes),
                    since_ms: ended,
                    warned: row.warned_at_ms.is_some(),
                    clean_at_ms: clean_at,
                    kept: row.kept,
                    ending: row.term_sent_at_ms.is_some() && !row.kept,
                })
            })
            .collect();
        let mut items = Vec::new();
        for group in [&mut orphans, &mut idle, &mut untracked] {
            group.sort_by_key(|i| Reverse(i.rss_bytes));
            items.append(group);
        }
        Ok(Some(CleanupView {
            items,
            log: store.cleanup_log(LOG_SHOWN)?,
            memory_low,
        }))
    }

    /// The user's End: SIGTERM now and a log entry with automatic = false. A tracked row
    /// gets term_sent_at_ms and loses an earlier Keep; any other process goes into
    /// `ending`. Either way a sweep after KILL_GRACE_MS sends SIGKILL. Does nothing
    /// for a process whose SIGTERM was already sent.
    pub fn end(
        &mut self,
        store: &mut Store,
        p: ProcessIdentity,
        table: &ProcTable,
        now_ms: i64,
        host: &mut dyn Host,
    ) -> anyhow::Result<()> {
        let sent = match self.rows.get(&p) {
            Some(row) => row.term_sent_at_ms.is_some() && !row.kept,
            None => self.ending.contains_key(&p),
        };
        if sent {
            return Ok(());
        }
        // The name is read before the signal, while the process still exists.
        let name = match self.rows.get(&p) {
            Some(row) => row.name.clone(),
            None => match host.details(p) {
                Some(d) => d.name,
                None => return Ok(()),
            },
        };
        if !host.signal(p, Signal::Term) {
            return Ok(());
        }
        let (mut provider, mut session_id) = (None, None);
        if let Some(row) = self.rows.get_mut(&p) {
            row.term_sent_at_ms = Some(now_ms);
            row.kept = false;
            self.dirty.insert(p);
            provider = Some(row.provider);
            session_id = Some(row.session_id.clone());
        } else {
            self.ending.insert(p, now_ms);
        }
        self.unlogged.push(CleanupLogEntry {
            at_ms: now_ms,
            name,
            provider,
            session_id,
            rss_bytes: alive(table, p).map_or(0, |i| i.rss_bytes),
            automatic: false,
        });
        self.flush(store)
    }

    /// The user's Keep: kept = true, so this process is never warned about or cleaned again.
    /// It applies in memory at once, even if the save fails and is retried later.
    pub fn keep(&mut self, store: &mut Store, p: ProcessIdentity) -> anyhow::Result<()> {
        if let Some(row) = self.rows.get_mut(&p).filter(|r| !r.kept) {
            row.kept = true;
            self.dirty.insert(p);
        }
        self.flush(store)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::{HashMap, HashSet};

    use agentpulse_core::{
        AgentSession, ProcessIdentity, Provider, Reading, ResourceUsage, SessionStatus, TokenUsage,
    };
    use agentpulse_process::{Pressure, ProcDetails, ProcInfo, ProcTable, Signal};
    use agentpulse_storage::{Store, TrackedProcess};

    use super::*;
    use crate::settings::Cleanup;

    const H: i64 = 3_600_000;
    const T0: i64 = 1_000 * H;
    const MIN: i64 = 60_000;
    const UID: u32 = 501;

    /// Records what the sweep asks of the OS. `signal` succeeds only for
    /// identities in `alive`, so tests can model gone and reused processes.
    #[derive(Default)]
    pub(crate) struct FakeHost {
        pub(crate) details: HashMap<ProcessIdentity, ProcDetails>,
        pub(crate) alive: HashSet<ProcessIdentity>,
        pub(crate) signals: Vec<(u32, Signal)>,
        pub(crate) notes: Vec<(String, String)>,
        pub(crate) memory: (Option<Pressure>, Option<u64>),
        pub(crate) uid: u32,
    }

    impl Host for FakeHost {
        fn details(&self, p: ProcessIdentity) -> Option<ProcDetails> {
            self.details.get(&p).cloned()
        }
        fn signal(&mut self, p: ProcessIdentity, s: Signal) -> bool {
            self.signals.push((p.pid, s));
            self.alive.contains(&p)
        }
        fn notify(&mut self, title: &str, body: &str) {
            self.notes.push((title.into(), body.into()));
        }
        fn memory(&self) -> (Option<Pressure>, Option<u64>) {
            self.memory
        }
        fn my_uid(&self) -> u32 {
            self.uid
        }
    }

    fn id(pid: u32, start: u64) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            start_time_s: start,
        }
    }

    /// Rows of (pid, ppid, start, rss), plus this process, as in every real sample.
    fn table(rows: &[(u32, u32, u64, u64)]) -> ProcTable {
        let me = (std::process::id(), 0, 1, 0);
        ProcTable {
            procs: rows
                .iter()
                .chain([&me])
                .map(|&(pid, ppid, start, rss)| {
                    let info = ProcInfo {
                        ppid: Some(ppid),
                        start_time_s: start,
                        cpu_percent: 0.0,
                        rss_bytes: rss,
                    };
                    (pid, info)
                })
                .collect(),
            cpu_valid: true,
        }
    }

    fn host() -> FakeHost {
        FakeHost {
            uid: UID,
            ..FakeHost::default()
        }
    }

    fn add(h: &mut FakeHost, p: ProcessIdentity, name: &str, uid: u32) {
        let d = ProcDetails {
            name: name.into(),
            uid,
        };
        h.details.insert(p, d);
        h.alive.insert(p);
    }

    const RSS: u64 = 50 << 20;

    /// Agent 10 (Codex, session "s") with child 11, observed once.
    fn tracked() -> (Store, Tracker, FakeHost) {
        let mut store = Store::open_in_memory().unwrap();
        let mut t = Tracker::load(&store).unwrap();
        let mut h = host();
        add(&mut h, id(11, 111), "python3", UID);
        let live = table(&[(10, 1, 100, 0), (11, 10, 111, RSS)]);
        let roots = [(id(10, 100), Provider::Codex, "s".to_string())];
        t.observe(&mut store, &live, &roots, &h).unwrap();
        (store, t, h)
    }

    /// The table once agent 10 is gone and 11 was reparented to launchd.
    fn orphaned() -> ProcTable {
        table(&[(11, 1, 111, RSS)])
    }

    fn sweep(
        t: &mut Tracker,
        s: &mut Store,
        table: &ProcTable,
        cfg: &Cleanup,
        now: i64,
        h: &mut FakeHost,
    ) -> CleanupView {
        t.sweep(s, table, &[], cfg, now, h).unwrap().unwrap()
    }

    fn row(s: &Store) -> TrackedProcess {
        let rows = s.tracked_processes().unwrap();
        assert_eq!(rows.len(), 1);
        rows[0].clone()
    }

    fn session(id: &str, status: SessionStatus, process: Option<ProcessIdentity>) -> AgentSession {
        AgentSession {
            provider: Provider::ClaudeCode,
            session_id: id.into(),
            project_path: Some(format!("/work/{id}")),
            model: None,
            status,
            started_at_ms: Some(0),
            last_activity_at_ms: Some(T0 - 48 * H),
            ended_at_ms: None,
            active_ms: 0,
            tokens: TokenUsage::default(),
            cost_usd: Reading::Unavailable,
            unpriced_events: 0,
            output_rate: 0.0,
            process,
            resources: Reading::Measured(ResourceUsage {
                cpu_percent: None,
                rss_bytes: 7,
                process_count: 1,
            }),
            recorded_cost: Reading::Unavailable,
        }
    }

    #[test]
    fn orphan_is_warned_then_cleaned_then_killed() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        let v = sweep(&mut t, &mut s, &orphaned(), &cfg, T0, &mut h);
        assert_eq!(row(&s).ended_at_ms, Some(T0));
        assert!(h.notes.is_empty());
        assert_eq!(v.items.len(), 1);
        assert_eq!(v.items[0].clean_at_ms, Some(T0 + 24 * H));
        assert_eq!(
            v.items[0].reason,
            Reason::Orphan {
                provider: Provider::Codex,
                session_id: "s".into()
            }
        );

        let v = sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 3 * H, &mut h);
        assert_eq!(h.notes.len(), 1);
        assert_eq!(h.notes[0].0, "Leftover process");
        assert_eq!(
            h.notes[0].1,
            "python3 from a finished Codex session is still running."
        );
        assert!(h.signals.is_empty());
        assert!(v.items[0].warned);

        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 24 * H, &mut h);
        assert_eq!(h.signals, vec![(11, Signal::Term)]);
        let log = s.cleanup_log(LOG_SHOWN).unwrap();
        assert_eq!(log.len(), 1);
        assert!(log[0].automatic);
        assert_eq!(log[0].rss_bytes, RSS);
        assert_eq!(
            h.notes.last().unwrap(),
            &(
                "Ended leftover process".to_string(),
                "python3 (50 MB) was ended after 24 hours.".to_string()
            )
        );

        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 24 * H + MIN, &mut h);
        assert_eq!(h.signals.last(), Some(&(11, Signal::Kill)));
    }

    #[test]
    fn ignored_term_gets_kill_next_sweep() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        for now in [T0, T0 + 3 * H, T0 + 24 * H, T0 + 24 * H + MIN] {
            sweep(&mut t, &mut s, &orphaned(), &cfg, now, &mut h);
        }
        assert_eq!(h.signals, vec![(11, Signal::Term), (11, Signal::Kill)]);
        sweep(
            &mut t,
            &mut s,
            &table(&[]),
            &cfg,
            T0 + 24 * H + 2 * MIN,
            &mut h,
        );
        sweep(
            &mut t,
            &mut s,
            &table(&[]),
            &cfg,
            T0 + 24 * H + 3 * MIN,
            &mut h,
        );
        assert_eq!(h.signals.len(), 2);
        assert!(s.tracked_processes().unwrap().is_empty());
    }

    #[test]
    fn one_notification_per_kind_per_sweep() {
        let mut s = Store::open_in_memory().unwrap();
        let mut t = Tracker::load(&s).unwrap();
        let mut h = host();
        let kids = [(11, "python3"), (12, "node"), (13, "vite"), (14, "esbuild")];
        let mut live = vec![(10, 1, 100, 0)];
        let mut gone = Vec::new();
        for (pid, name) in kids {
            add(&mut h, id(pid, 100 + u64::from(pid)), name, UID);
            live.push((pid, 10, 100 + u64::from(pid), RSS));
            gone.push((pid, 1, 100 + u64::from(pid), RSS));
        }
        let roots = [(id(10, 100), Provider::Codex, "s".to_string())];
        t.observe(&mut s, &table(&live), &roots, &h).unwrap();
        let gone = table(&gone);
        let cfg = Cleanup::default();
        sweep(&mut t, &mut s, &gone, &cfg, T0, &mut h);
        sweep(&mut t, &mut s, &gone, &cfg, T0 + 3 * H, &mut h);
        assert_eq!(
            h.notes,
            [(
                "4 leftover processes".to_string(),
                "esbuild, node, python3 and 1 more from finished agent sessions are still running."
                    .to_string()
            )]
        );
        sweep(&mut t, &mut s, &gone, &cfg, T0 + 24 * H, &mut h);
        assert_eq!(h.signals.len(), 4);
        assert_eq!(h.notes.len(), 2);
        assert_eq!(
            h.notes[1],
            (
                "Ended 4 leftover processes".to_string(),
                "esbuild, node, python3 and 1 more were ended after 24 hours, freeing 200 MB."
                    .to_string()
            )
        );
    }

    #[test]
    fn a_table_without_agentpulse_is_not_swept() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0, &mut h);
        // A failed list_pids(): an empty table.
        let failed = ProcTable::default();
        for now in [T0 + 3 * H, T0 + 48 * H] {
            let v = t.sweep(&mut s, &failed, &[], &cfg, now, &mut h).unwrap();
            assert_eq!(v, None);
        }
        assert_eq!(row(&s).warned_at_ms, None);
        assert!(h.notes.is_empty() && h.signals.is_empty());
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 48 * H, &mut h);
        assert_eq!(row(&s).warned_at_ms, Some(T0 + 48 * H));
    }

    #[test]
    fn root_back_in_the_table_is_not_cleaned() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0, &mut h);
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 3 * H, &mut h);
        // The sample at T0 missed agent 10; it is still running.
        let back = table(&[(10, 1, 100, 0), (11, 1, 111, RSS)]);
        sweep(&mut t, &mut s, &back, &cfg, T0 + 24 * H, &mut h);
        assert!(h.signals.is_empty());
        assert_eq!(row(&s).term_sent_at_ms, None);
    }

    #[test]
    fn keep_from_another_instance_is_honoured() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0, &mut h);
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 3 * H, &mut h);
        // A second AgentPulse on the same database.
        let mut other = Tracker::load(&s).unwrap();
        other.keep(&mut s, id(11, 111)).unwrap();
        let v = sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 24 * H, &mut h);
        assert!(h.signals.is_empty());
        assert!(v.items[0].kept);
    }

    #[test]
    fn late_start_warns_before_cleaning() {
        let mut s = Store::open_in_memory().unwrap();
        let saved = TrackedProcess {
            process: id(11, 111),
            root: id(10, 100),
            provider: Provider::Codex,
            session_id: "s".into(),
            name: "python3".into(),
            ended_at_ms: Some(T0),
            warned_at_ms: None,
            term_sent_at_ms: None,
            kept: false,
        };
        s.save_tracked(&[saved]).unwrap();
        let mut t = Tracker::load(&s).unwrap();
        let mut h = host();
        add(&mut h, id(11, 111), "python3", UID);
        let cfg = Cleanup::default();

        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 30 * H, &mut h);
        assert_eq!(row(&s).warned_at_ms, Some(T0 + 30 * H));
        assert_eq!(h.notes.len(), 1);
        assert!(h.signals.is_empty());
        sweep(
            &mut t,
            &mut s,
            &orphaned(),
            &cfg,
            T0 + 30 * H + 59 * MIN,
            &mut h,
        );
        assert!(h.signals.is_empty());
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 31 * H, &mut h);
        assert_eq!(h.signals, vec![(11, Signal::Term)]);
    }

    #[test]
    fn orphan_gone_with_agent_is_dropped() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        for now in [T0, T0 + 3 * H, T0 + 24 * H, T0 + 48 * H] {
            let v = sweep(&mut t, &mut s, &table(&[]), &cfg, now, &mut h);
            assert!(v.items.is_empty());
        }
        assert!(s.tracked_processes().unwrap().is_empty());
        assert!(h.notes.is_empty());
        assert!(h.signals.is_empty());
    }

    #[test]
    fn orphan_under_new_agent_is_readopted() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0, &mut h);
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 3 * H, &mut h);
        assert!(row(&s).warned_at_ms.is_some());

        let adopted = table(&[(20, 1, 120, 0), (11, 20, 111, RSS)]);
        let roots = [(id(20, 120), Provider::ClaudeCode, "s2".to_string())];
        t.observe(&mut s, &adopted, &roots, &h).unwrap();
        let r = row(&s);
        assert_eq!(
            (r.ended_at_ms, r.warned_at_ms, r.term_sent_at_ms),
            (None, None, None)
        );
        assert_eq!(
            (r.root, r.provider, r.session_id.as_str()),
            (id(20, 120), Provider::ClaudeCode, "s2")
        );

        let v = sweep(&mut t, &mut s, &adopted, &cfg, T0 + 24 * H, &mut h);
        assert!(h.signals.is_empty());
        assert!(v.items.is_empty());
        assert_eq!(h.notes.len(), 1);
    }

    #[test]
    fn kept_process_is_never_warned_or_cleaned() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0, &mut h);
        t.keep(&mut s, id(11, 111)).unwrap();
        assert!(row(&s).kept);
        for now in [T0 + 3 * H, T0 + 24 * H, T0 + 24 * H + MIN, T0 + 48 * H] {
            let v = sweep(&mut t, &mut s, &orphaned(), &cfg, now, &mut h);
            assert_eq!(v.items[0].clean_at_ms, None);
            assert!(v.items[0].kept);
        }
        assert!(h.notes.is_empty());
        assert!(h.signals.is_empty());
    }

    #[test]
    fn auto_clean_off_only_warns() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup {
            auto_clean: false,
            ..Cleanup::default()
        };
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0, &mut h);
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 3 * H, &mut h);
        assert_eq!(h.notes.len(), 1);
        let v = sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 48 * H, &mut h);
        assert!(h.signals.is_empty());
        assert_eq!(v.items[0].clean_at_ms, None);
        // Not kept: the screen reads "Automatic end is off", not "Kept".
        assert!(!v.items[0].kept);
    }

    #[test]
    fn reused_pid_is_never_signalled() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0, &mut h);
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 3 * H, &mut h);
        let reused = table(&[(11, 1, 999, RSS)]);
        h.alive.insert(id(11, 999));
        sweep(&mut t, &mut s, &reused, &cfg, T0 + 24 * H, &mut h);
        sweep(&mut t, &mut s, &reused, &cfg, T0 + 48 * H, &mut h);
        assert!(s.tracked_processes().unwrap().is_empty());
        assert!(h.signals.is_empty());
    }

    #[test]
    fn agent_sessions_are_never_signalled() {
        let mut s = Store::open_in_memory().unwrap();
        let mut h = host();
        let (idle, waiting, active) = (id(40, 140), id(41, 141), id(42, 142));
        for p in [idle, waiting, active] {
            add(&mut h, p, "claude", UID);
        }
        // An idle agent that was itself tracked as a child of an earlier agent.
        let nested = TrackedProcess {
            process: idle,
            root: id(10, 100),
            provider: Provider::Codex,
            session_id: "old".into(),
            name: "claude".into(),
            ended_at_ms: Some(T0 - 48 * H),
            warned_at_ms: Some(T0 - 48 * H),
            term_sent_at_ms: None,
            kept: false,
        };
        s.save_tracked(&[nested]).unwrap();
        let mut t = Tracker::load(&s).unwrap();
        let live = table(&[(40, 50, 140, 0), (41, 50, 141, 0), (42, 50, 142, 0)]);
        let sessions = [
            session("i", SessionStatus::Idle, Some(idle)),
            session("w", SessionStatus::Waiting, Some(waiting)),
            session("a", SessionStatus::Active, Some(active)),
        ];
        let cfg = Cleanup::default();
        let v = t
            .sweep(&mut s, &live, &sessions, &cfg, T0, &mut h)
            .unwrap()
            .unwrap();
        // Both idle sessions in one notification.
        assert_eq!(
            h.notes,
            [(
                "2 idle sessions".to_string(),
                "Claude Code in i and Claude Code in w have had no activity for 48 hours or more."
                    .to_string()
            )]
        );
        t.sweep(&mut s, &live, &sessions, &cfg, T0 + MIN, &mut h)
            .unwrap();
        t.sweep(&mut s, &live, &sessions, &cfg, T0 + 48 * H, &mut h)
            .unwrap();
        assert_eq!(h.notes.len(), 1);
        assert!(h.signals.is_empty());
        let reasons: Vec<_> = v.items.iter().map(|i| (i.process, &i.reason)).collect();
        assert_eq!(reasons.len(), 2);
        assert!(reasons.contains(&(
            Some(idle),
            &Reason::IdleSession {
                provider: Provider::ClaudeCode,
                session_id: "i".into(),
                waiting: false
            }
        )));
        assert!(reasons.contains(&(
            Some(waiting),
            &Reason::IdleSession {
                provider: Provider::ClaudeCode,
                session_id: "w".into(),
                waiting: true
            }
        )));
    }

    #[test]
    fn shared_idle_session_has_no_process() {
        let mut s = Store::open_in_memory().unwrap();
        let mut t = Tracker::load(&s).unwrap();
        let mut h = host();
        let sessions = [session("i", SessionStatus::Idle, None)];
        let v = t
            .sweep(
                &mut s,
                &table(&[]),
                &sessions,
                &Cleanup::default(),
                T0,
                &mut h,
            )
            .unwrap()
            .unwrap();
        assert_eq!(v.items.len(), 1);
        assert_eq!(v.items[0].process, None);
        assert_eq!(v.items[0].since_ms, T0 - 48 * H);
    }

    #[test]
    fn untracked_leftovers_are_listed_not_touched() {
        let mut s = Store::open_in_memory().unwrap();
        let mut t = Tracker::load(&s).unwrap();
        let mut h = host();
        let procs = [
            (30, "node", UID, 3),
            (31, "Python", UID, 2),
            (32, "spline-mcp", UID, 1),
            (33, "bash", UID, 9),
            (34, "node", 0, 9),
        ];
        for (pid, name, uid, _) in procs {
            add(&mut h, id(pid, 100 + pid as u64), name, uid);
        }
        let rows: Vec<_> = procs
            .iter()
            .map(|&(pid, _, _, rss)| (pid, 1, 100 + pid as u64, rss))
            .collect();
        let tbl = table(&rows);
        let cfg = Cleanup::default();
        for now in [T0, T0 + 3 * H, T0 + 24 * H, T0 + 48 * H] {
            let v = sweep(&mut t, &mut s, &tbl, &cfg, now, &mut h);
            let names: Vec<_> = v.items.iter().map(|i| i.name.as_str()).collect();
            assert_eq!(names, ["node", "Python", "spline-mcp"]);
            assert!(v.items.iter().all(|i| i.reason == Reason::Untracked));
            assert!(
                v.items
                    .iter()
                    .all(|i| i.since_ms == 0 && i.clean_at_ms.is_none())
            );
        }
        assert!(h.signals.is_empty());
        assert!(h.notes.is_empty());
    }

    #[test]
    fn manual_end_logs_and_follows_up_with_kill() {
        let (mut s, mut t, mut h) = tracked();
        add(&mut h, id(30, 130), "node", UID);
        let tbl = table(&[(11, 1, 111, RSS), (30, 1, 130, 5)]);
        t.end(&mut s, id(30, 130), &tbl, T0, &mut h).unwrap();
        assert_eq!(h.signals, vec![(30, Signal::Term)]);
        let log = s.cleanup_log(LOG_SHOWN).unwrap();
        assert_eq!(log.len(), 1);
        assert!(!log[0].automatic);
        assert_eq!((log[0].name.as_str(), log[0].rss_bytes), ("node", 5));

        let cfg = Cleanup::default();
        sweep(&mut t, &mut s, &tbl, &cfg, T0, &mut h);
        t.end(&mut s, id(11, 111), &tbl, T0 + MIN, &mut h).unwrap();
        assert_eq!(row(&s).term_sent_at_ms, Some(T0 + MIN));
        let log = s.cleanup_log(LOG_SHOWN).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].provider, Some(Provider::Codex));
        sweep(&mut t, &mut s, &tbl, &cfg, T0 + 2 * MIN, &mut h);
        // Both ignored their SIGTERM: the untracked node too gets SIGKILL.
        let mut kills = h.signals[2..].to_vec();
        kills.sort_by_key(|k| k.0);
        assert_eq!(kills, [(11, Signal::Kill), (30, Signal::Kill)]);
    }

    #[test]
    fn second_end_sends_nothing_and_logs_nothing() {
        let (mut s, mut t, mut h) = tracked();
        add(&mut h, id(30, 130), "node", UID);
        let tbl = table(&[(11, 1, 111, RSS), (30, 1, 130, 5)]);
        sweep(&mut t, &mut s, &tbl, &Cleanup::default(), T0, &mut h);
        for now in [T0 + MIN, T0 + MIN + 1] {
            t.end(&mut s, id(11, 111), &tbl, now, &mut h).unwrap();
            t.end(&mut s, id(30, 130), &tbl, now, &mut h).unwrap();
        }
        assert_eq!(h.signals, vec![(11, Signal::Term), (30, Signal::Term)]);
        assert_eq!(s.cleanup_log(LOG_SHOWN).unwrap().len(), 2);
        assert_eq!(row(&s).term_sent_at_ms, Some(T0 + MIN));
    }

    #[test]
    fn ended_untracked_gets_kill_after_the_grace() {
        let mut s = Store::open_in_memory().unwrap();
        let mut t = Tracker::load(&s).unwrap();
        let mut h = host();
        add(&mut h, id(30, 130), "node", UID);
        let tbl = table(&[(30, 1, 130, 5)]);
        let cfg = Cleanup::default();
        t.end(&mut s, id(30, 130), &tbl, T0, &mut h).unwrap();
        let v = sweep(&mut t, &mut s, &tbl, &cfg, T0 + MIN - 1, &mut h);
        assert_eq!(h.signals, vec![(30, Signal::Term)]);
        assert!(v.items[0].ending);
        sweep(&mut t, &mut s, &tbl, &cfg, T0 + MIN, &mut h);
        assert_eq!(h.signals, vec![(30, Signal::Term), (30, Signal::Kill)]);
        // Gone: no further signal, and End works again for a new process.
        sweep(&mut t, &mut s, &table(&[]), &cfg, T0 + 2 * MIN, &mut h);
        sweep(&mut t, &mut s, &table(&[]), &cfg, T0 + 3 * MIN, &mut h);
        assert_eq!(h.signals.len(), 2);
        assert!(t.ending.is_empty());
    }

    #[test]
    fn ending_is_shown_after_end() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        let v = sweep(&mut t, &mut s, &orphaned(), &cfg, T0, &mut h);
        assert!(!v.items[0].ending);
        t.end(&mut s, id(11, 111), &orphaned(), T0 + 1, &mut h)
            .unwrap();
        let v = sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 2, &mut h);
        assert!(v.items[0].ending);
    }

    #[test]
    fn end_on_kept_orphan_still_follows_up_with_kill() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0, &mut h);
        t.keep(&mut s, id(11, 111)).unwrap();
        t.end(&mut s, id(11, 111), &orphaned(), T0 + MIN, &mut h)
            .unwrap();
        assert!(!row(&s).kept);
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 2 * MIN, &mut h);
        assert_eq!(h.signals, vec![(11, Signal::Term), (11, Signal::Kill)]);
    }

    #[test]
    fn clean_state_survives_reload() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        for now in [T0, T0 + 3 * H, T0 + 24 * H] {
            sweep(&mut t, &mut s, &orphaned(), &cfg, now, &mut h);
        }
        let again = Tracker::load(&s).unwrap();
        let r = &again.rows[&id(11, 111)];
        assert_eq!(
            (r.ended_at_ms, r.warned_at_ms, r.term_sent_at_ms),
            (Some(T0), Some(T0 + 3 * H), Some(T0 + 24 * H))
        );
        assert!(t.dirty.is_empty() && t.unlogged.is_empty());
    }

    #[test]
    fn failed_term_is_retried_not_killed() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0, &mut h);
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 3 * H, &mut h);
        // Still in the table, but the signal is refused.
        h.alive.clear();
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 24 * H, &mut h);
        sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 24 * H + MIN, &mut h);
        assert_eq!(h.signals, vec![(11, Signal::Term), (11, Signal::Term)]);
        assert_eq!(row(&s).term_sent_at_ms, None);
        assert!(s.cleanup_log(LOG_SHOWN).unwrap().is_empty());
        assert_eq!(h.notes.len(), 1);

        h.alive.insert(id(11, 111));
        sweep(
            &mut t,
            &mut s,
            &orphaned(),
            &cfg,
            T0 + 24 * H + 2 * MIN,
            &mut h,
        );
        assert_eq!(s.cleanup_log(LOG_SHOWN).unwrap().len(), 1);
        sweep(
            &mut t,
            &mut s,
            &orphaned(),
            &cfg,
            T0 + 24 * H + 3 * MIN,
            &mut h,
        );
        assert_eq!(h.signals[2..], [(11, Signal::Term), (11, Signal::Kill)]);
    }

    #[test]
    fn keep_after_term_blocks_kill() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup::default();
        for now in [T0, T0 + 3 * H, T0 + 24 * H] {
            sweep(&mut t, &mut s, &orphaned(), &cfg, now, &mut h);
        }
        assert_eq!(h.signals, vec![(11, Signal::Term)]);
        t.keep(&mut s, id(11, 111)).unwrap();
        for now in [T0 + 24 * H + MIN, T0 + 48 * H] {
            sweep(&mut t, &mut s, &orphaned(), &cfg, now, &mut h);
        }
        assert_eq!(h.signals, vec![(11, Signal::Term)]);
    }

    #[test]
    fn clean_at_waits_for_the_warning_lead() {
        let (mut s, mut t, mut h) = tracked();
        let cfg = Cleanup {
            warn_after_hours: 3,
            clean_after_hours: 3,
            auto_clean: true,
        };
        let v = sweep(&mut t, &mut s, &orphaned(), &cfg, T0, &mut h);
        assert_eq!(v.items[0].clean_at_ms, Some(T0 + 3 * H + WARN_LEAD_MS));
        // Warned late, at +5 h: the clean moves with the warning.
        let v = sweep(&mut t, &mut s, &orphaned(), &cfg, T0 + 5 * H, &mut h);
        assert_eq!(v.items[0].clean_at_ms, Some(T0 + 5 * H + WARN_LEAD_MS));
    }

    #[test]
    fn memory_suggestion_repeats_at_most_every_six_hours() {
        let mut s = Store::open_in_memory().unwrap();
        let mut t = Tracker::load(&s).unwrap();
        let mut h = host();
        h.memory = (Some(Pressure::Warn), None);
        let cfg = Cleanup::default();
        let v = sweep(&mut t, &mut s, &table(&[]), &cfg, T0, &mut h);
        assert!(v.memory_low);
        assert_eq!(h.notes.len(), 1);
        assert_eq!(h.notes[0].0, "Memory is low");
        sweep(&mut t, &mut s, &table(&[]), &cfg, T0 + H, &mut h);
        assert_eq!(h.notes.len(), 1);
        sweep(&mut t, &mut s, &table(&[]), &cfg, T0 + 6 * H, &mut h);
        assert_eq!(h.notes.len(), 2);
    }
}
