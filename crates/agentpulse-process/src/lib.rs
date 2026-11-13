//! CPU and memory of each agent's process tree.

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::time::Instant;

use agentpulse_core::{LiveSession, ProcessIdentity, ResourceUsage};

/// Start times are whole seconds, and the agent and the OS read them at
/// slightly different moments.
const START_TOLERANCE_S: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcInfo {
    pub ppid: Option<u32>,
    /// Unix seconds.
    pub start_time_s: u64,
    /// Percent of one core since the previous sample. A process first seen in
    /// this sample counts all its CPU time if it started after the previous
    /// sample, and reads 0 otherwise (its earlier time is not attributable).
    pub cpu_percent: f32,
    pub rss_bytes: u64,
}

#[derive(Clone, Debug, Default)]
pub struct ProcTable {
    pub procs: HashMap<u32, ProcInfo>,
    /// False for the first sample: CPU is a delta and needs two samples.
    pub cpu_valid: bool,
}

pub struct Sampler {
    /// CPU time in nanoseconds at the previous sample, keyed by process
    /// identity so that a reused PID does not inherit another process's time.
    prev_cpu_ns: HashMap<(u32, u64), u64>,
    prev_at: Option<Instant>,
    /// Wall clock of the previous sample, whole Unix seconds (rounded down).
    prev_wall_s: Option<u64>,
    /// Nanoseconds per `ri_*_time` tick (mach absolute time units).
    ns_per_tick: f64,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    // libc deprecates mach_timebase_info in favour of the mach2 crate; the
    // call is stable and one dependency is enough.
    #[allow(deprecated)]
    pub fn new() -> Self {
        let mut tb = libc::mach_timebase_info { numer: 0, denom: 0 };
        // SAFETY: `tb` is a valid out-pointer.
        let ok = unsafe { libc::mach_timebase_info(&mut tb) } == 0 && tb.denom != 0;
        Self {
            prev_cpu_ns: HashMap::new(),
            prev_at: None,
            prev_wall_s: None,
            ns_per_tick: if ok {
                f64::from(tb.numer) / f64::from(tb.denom)
            } else {
                1.0
            },
        }
    }

    /// Snapshot of every process visible to this user. Processes whose
    /// bsdinfo cannot be read are skipped; if only rusage fails the process is
    /// kept (its parent link is needed) with 0 CPU and 0 RSS.
    pub fn sample(&mut self) -> ProcTable {
        let now = Instant::now();
        let wall_ns = self
            .prev_at
            .map(|t| now.duration_since(t).as_nanos() as f64);
        let mut cpu_ns = HashMap::new();
        let mut procs = HashMap::new();
        for pid in list_pids() {
            let Some((ppid, start_time_s)) = bsd_info(pid) else {
                continue;
            };
            let (cpu, rss_bytes) = rusage(pid).unwrap_or((0.0, 0));
            let cpu = cpu * self.ns_per_tick;
            let key = (pid, start_time_s);
            // A process that started after the previous sample accrued all of
            // its CPU time since then. Start times are whole seconds, so it
            // counts as new only from the previous sample's second onwards.
            let started_since = self.prev_wall_s.is_some_and(|p| start_time_s >= p);
            let prev = match self.prev_cpu_ns.get(&key) {
                Some(&prev) => Some(prev as f64),
                None if started_since => Some(0.0),
                None => None,
            };
            let cpu_percent = match (prev, wall_ns) {
                (Some(prev), Some(w)) if w > 0.0 => ((cpu - prev).max(0.0) / w * 100.0) as f32,
                _ => 0.0,
            };
            cpu_ns.insert(key, cpu as u64);
            procs.insert(
                pid,
                ProcInfo {
                    ppid: Some(ppid),
                    start_time_s,
                    cpu_percent,
                    rss_bytes,
                },
            );
        }
        let cpu_valid = self.prev_at.is_some();
        self.prev_cpu_ns = cpu_ns;
        self.prev_at = Some(now);
        self.prev_wall_s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());
        ProcTable { procs, cpu_valid }
    }
}

fn list_pids() -> Vec<u32> {
    // SAFETY: a null buffer asks for the number of pids.
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return Vec::new();
    }
    // Slack for processes that start between the two calls.
    let mut pids = vec![0 as libc::pid_t; n as usize + 64];
    let bytes = (pids.len() * size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: the buffer holds `bytes` bytes. The call returns the pid count.
    let got = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    pids.truncate(got.max(0) as usize);
    pids.into_iter()
        .filter(|&p| p > 0)
        .map(|p| p as u32)
        .collect()
}

fn bsd_info(pid: u32) -> Option<(u32, u64)> {
    // SAFETY: proc_bsdinfo is plain data; the kernel fills at most its size.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (got == size).then_some((info.pbi_ppid, info.pbi_start_tvsec))
}

/// (user + system CPU time in mach ticks, resident bytes)
fn rusage(pid: u32) -> Option<(f64, u64)> {
    // SAFETY: rusage_info_v2 is plain data; RUSAGE_INFO_V2 fills exactly it.
    let mut ri: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V2,
            (&mut ri as *mut libc::rusage_info_v2).cast(),
        )
    };
    (rc == 0).then(|| {
        (
            (ri.ri_user_time + ri.ri_system_time) as f64,
            ri.ri_resident_size,
        )
    })
}

/// The identity of the process behind `live`, or None when the PID is gone or
/// now belongs to a different process (PID reuse).
pub fn verify(table: &ProcTable, live: &LiveSession) -> Option<ProcessIdentity> {
    let p = table.procs.get(&live.pid)?;
    let ok = if let Some(recorded) = live.proc_start_s {
        p.start_time_s.abs_diff(recorded) <= START_TOLERANCE_S
    } else if let Some(started_ms) = live.started_at_ms {
        // A process cannot start after the session it hosts.
        i128::from(p.start_time_s) * 1000
            <= i128::from(started_ms) + 1000 * START_TOLERANCE_S as i128
    } else {
        false
    };
    ok.then_some(ProcessIdentity {
        pid: live.pid,
        start_time_s: p.start_time_s,
    })
}

/// Resource usage per root, keyed by root PID. Each process counts toward
/// exactly one root: its nearest ancestor (or itself) that is a root.
pub fn attribute(table: &ProcTable, roots: &[ProcessIdentity]) -> HashMap<u32, ResourceUsage> {
    let valid: HashSet<u32> = roots
        .iter()
        .filter(|r| {
            table
                .procs
                .get(&r.pid)
                .is_some_and(|p| p.start_time_s == r.start_time_s)
        })
        .map(|r| r.pid)
        .collect();
    let mut out: HashMap<u32, ResourceUsage> = valid
        .iter()
        .map(|&pid| {
            let usage = ResourceUsage {
                cpu_percent: table.cpu_valid.then_some(0.0),
                rss_bytes: 0,
                process_count: 0,
            };
            (pid, usage)
        })
        .collect();

    for (&pid, info) in &table.procs {
        let mut seen = HashSet::new();
        let (mut cur, mut p) = (pid, info);
        let root = loop {
            if valid.contains(&cur) {
                break Some(cur);
            }
            // The visited set stops parent cycles.
            let Some(parent) = p.ppid else { break None };
            let Some(next) = table.procs.get(&parent) else {
                break None;
            };
            if !seen.insert(cur) {
                break None;
            }
            (cur, p) = (parent, next);
        };
        if let Some(u) = root.and_then(|r| out.get_mut(&r)) {
            u.rss_bytes += info.rss_bytes;
            u.process_count += 1;
            if let Some(c) = &mut u.cpu_percent {
                *c += info.cpu_percent;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentpulse_core::LiveState;

    fn p(ppid: Option<u32>, start: u64, cpu: f32, rss: u64) -> ProcInfo {
        ProcInfo {
            ppid,
            start_time_s: start,
            cpu_percent: cpu,
            rss_bytes: rss,
        }
    }

    fn table(procs: &[(u32, ProcInfo)], cpu_valid: bool) -> ProcTable {
        ProcTable {
            procs: procs.iter().copied().collect(),
            cpu_valid,
        }
    }

    fn id(pid: u32, start_time_s: u64) -> ProcessIdentity {
        ProcessIdentity { pid, start_time_s }
    }

    fn live(pid: u32, proc_start_s: Option<u64>, started_at_ms: Option<i64>) -> LiveSession {
        LiveSession {
            session_id: "s".into(),
            pid,
            proc_start_s,
            started_at_ms,
            state: LiveState::Idle,
            project_path: None,
        }
    }

    const T: u64 = 1_700_000_000;

    #[test]
    fn verify_detects_pid_reuse() {
        let t = table(&[(100, p(None, T + 500, 0.0, 1))], true);
        assert_eq!(verify(&t, &live(100, Some(T), None)), None);
        let t = table(&[(100, p(None, T + 2, 0.0, 1))], true);
        assert_eq!(verify(&t, &live(100, Some(T), None)), Some(id(100, T + 2)));
        let t = table(&[(100, p(None, T - 2, 0.0, 1))], true);
        assert_eq!(verify(&t, &live(100, Some(T), None)), Some(id(100, T - 2)));
        let t = table(&[(100, p(None, T + 3, 0.0, 1))], true);
        assert_eq!(verify(&t, &live(100, Some(T), None)), None);
    }

    #[test]
    fn verify_falls_back_to_session_start() {
        let session_ms = (T as i64) * 1000;
        let before = table(&[(100, p(None, T - 3600, 0.0, 1))], true);
        assert_eq!(
            verify(&before, &live(100, None, Some(session_ms))),
            Some(id(100, T - 3600))
        );
        let after = table(&[(100, p(None, T + 10, 0.0, 1))], true);
        assert_eq!(verify(&after, &live(100, None, Some(session_ms))), None);
        assert_eq!(verify(&before, &live(100, None, None)), None);
        assert_eq!(verify(&before, &live(999, Some(T), Some(session_ms))), None);
    }

    fn tree() -> ProcTable {
        table(
            &[
                (10, p(Some(1), T, 1.0, 100)),
                (11, p(Some(10), T, 2.0, 200)),
                (12, p(Some(10), T, 4.0, 400)),
                (13, p(Some(12), T, 8.0, 800)),
                (20, p(Some(1), T, 16.0, 1600)),
            ],
            true,
        )
    }

    #[test]
    fn tree_sums_descendants_only() {
        let r = attribute(&tree(), &[id(10, T)]);
        assert_eq!(r.len(), 1);
        let u = r[&10];
        assert_eq!(u.process_count, 4);
        assert_eq!(u.rss_bytes, 1500);
        assert_eq!(u.cpu_percent, Some(15.0));
    }

    #[test]
    fn nested_roots_do_not_double_count() {
        let r = attribute(&tree(), &[id(10, T), id(12, T)]);
        assert_eq!((r[&10].process_count, r[&10].rss_bytes), (2, 300));
        assert_eq!((r[&12].process_count, r[&12].rss_bytes), (2, 1200));
        assert_eq!(r[&10].cpu_percent, Some(3.0));
        assert_eq!(r[&12].cpu_percent, Some(12.0));
        assert_eq!(r.values().map(|u| u.rss_bytes).sum::<u64>(), 1500);
    }

    #[test]
    fn stale_or_missing_roots_are_omitted() {
        let r = attribute(&tree(), &[id(10, T + 1), id(77, T), id(20, T)]);
        assert_eq!(r.keys().copied().collect::<Vec<_>>(), vec![20]);
    }

    #[test]
    fn cycles_and_missing_parents_terminate() {
        let t = table(
            &[
                (10, p(Some(1), T, 1.0, 100)),
                (30, p(Some(31), T, 1.0, 1)),
                (31, p(Some(30), T, 1.0, 1)),
                (40, p(Some(999), T, 1.0, 1)),
            ],
            true,
        );
        let r = attribute(&t, &[id(10, T)]);
        assert_eq!(r[&10].process_count, 1);
    }

    #[test]
    fn cpu_is_none_until_valid() {
        let mut t = tree();
        t.cpu_valid = false;
        let u = attribute(&t, &[id(10, T)])[&10];
        assert_eq!(u.cpu_percent, None);
        assert_eq!(u.rss_bytes, 1500);
    }

    #[test]
    fn sampler_sees_current_process() {
        let mut s = Sampler::new();
        let first = s.sample();
        assert!(!first.cpu_valid);
        let me = first.procs[&std::process::id()];
        assert!(me.rss_bytes > 0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(me.start_time_s <= now && now - me.start_time_s < 3600);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(s.sample().cpu_valid);
    }

    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn cpu_of(t: &ProcTable, c: &Child) -> f32 {
        t.procs[&c.0.id()].cpu_percent
    }

    #[test]
    fn busy_child_reads_near_one_core() {
        let c = Child(
            std::process::Command::new("/usr/bin/yes")
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut s = Sampler::new();
        s.sample();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let cpu = cpu_of(&s.sample(), &c);
        assert!((60.0..=140.0).contains(&cpu), "yes read {cpu}%");
    }

    #[test]
    fn child_started_between_samples_counts_its_cpu() {
        let mut s = Sampler::new();
        s.sample();
        let c = Child(
            std::process::Command::new("/usr/bin/yes")
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        std::thread::sleep(std::time::Duration::from_millis(400));
        let cpu = cpu_of(&s.sample(), &c);
        assert!(cpu > 20.0, "new yes read {cpu}%");
    }

    #[test]
    fn idle_child_reads_zero_not_stale() {
        let c = Child(
            std::process::Command::new("/bin/sleep")
                .arg("30")
                .spawn()
                .unwrap(),
        );
        let mut s = Sampler::new();
        s.sample();
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let cpu = cpu_of(&s.sample(), &c);
            assert!(cpu < 1.0, "sleep read {cpu}%");
        }
    }
}
