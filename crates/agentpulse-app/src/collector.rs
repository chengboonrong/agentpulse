//! The collector loop: polls each adapter, stores what it read, samples agent
//! processes and turns it all into a [`Snapshot`]. Knows nothing about GPUI.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::thread;
use std::time::{Duration, Instant};

use agentpulse_core::{
    ActivityMeter, AgentAdapter, AgentSession, LiveSession, ProcessIdentity, Provider, Reading,
    SessionStatus, TokenUsage, resolve_status,
};
use agentpulse_cost::PriceBook;
use agentpulse_platform_macos::gpu_utilization;
use agentpulse_process::{ProcTable, Sampler, attribute, verify};
use agentpulse_storage::{SessionRow, Store, Totals};
use chrono::{DateTime, Local};

use crate::widget_export::WidgetExporter;

const RATE_WINDOW_MS: i64 = 30_000;
pub const HISTORY_LEN: usize = 120;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
pub const PROCESS_INTERVAL: Duration = Duration::from_secs(2);
const REQUERY_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, serde::Serialize)]
pub struct Snapshot {
    pub taken_at_ms: i64,
    /// Every agent's sessions. Live first (Active, Waiting, Idle), then ended;
    /// each group newest activity first.
    pub sessions: Vec<AgentSession>,
    /// Since local midnight, all agents.
    pub today_tokens: TokenUsage,
    /// Estimated, or Unavailable when there are events but none is priced.
    pub today_cost_usd: Reading<f64>,
    pub today_unpriced_events: u64,
    /// Output tokens/s over the last 30 s, all sessions.
    pub output_rate: f32,
    /// One sample per tick, most recent last, at most 120.
    pub rate_history: Vec<f32>,
    /// Sum over live sessions' process trees; None when no live process or CPU not yet valid.
    pub agents_cpu_percent: Option<f32>,
    /// None when no live process.
    pub agents_rss_bytes: Option<u64>,
    /// GPU busy percent of the whole Mac. Never attributed to an agent.
    pub system_gpu_percent: Reading<u8>,
    /// Initial backfill still in progress.
    pub indexing: bool,
    /// Current adapter, storage and configuration problems; empty when healthy.
    pub errors: Vec<String>,
}

/// Cumulative ingestion counters, for `--dump`.
#[derive(Clone, Copy, Debug, Default)]
pub struct IngestStats {
    pub bytes: u64,
    pub files: u64,
    pub events: u64,
}

/// A session as the collector keys it: ids are unique per provider only.
type Key = (Provider, String);

struct Cache {
    at: Instant,
    /// Local midnight the totals were queried from.
    midnight: i64,
    live: BTreeSet<Key>,
    rows: Vec<Vec<SessionRow>>,
    totals: Totals,
}

/// One adapter and its own errors, so a broken agent never hides another's data.
struct Source {
    adapter: Box<dyn AgentAdapter>,
    /// Its last poll stopped at the read budget.
    more: bool,
    live: Vec<LiveSession>,
    poll_error: Option<String>,
    live_error: Option<String>,
    apply_error: Option<String>,
}

pub struct Collector {
    sources: Vec<Source>,
    store: Store,
    prices: PriceBook,
    sampler: Sampler,
    offsets: HashMap<String, u64>,
    meter: ActivityMeter,
    session_meters: HashMap<Key, ActivityMeter>,
    /// Output tokens already fed to the meters per event (with the event's
    /// time), so a response re-read with fuller usage adds only the increase.
    metered: HashMap<Key, (i64, u64)>,
    table: Option<(Instant, ProcTable)>,
    cache: Option<Cache>,
    rate_history: VecDeque<f32>,
    config_errors: Vec<String>,
    query_error: Option<String>,
    pub ingested: IngestStats,
}

impl Collector {
    /// `config_errors` are startup problems (such as an unreadable price
    /// table) that stay listed in every snapshot.
    pub fn new(
        adapters: Vec<Box<dyn AgentAdapter>>,
        store: Store,
        prices: PriceBook,
        config_errors: Vec<String>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            offsets: store.offsets()?,
            sources: adapters
                .into_iter()
                .map(|adapter| Source {
                    adapter,
                    // Until the first poll, assume there is history to read.
                    more: true,
                    live: Vec::new(),
                    poll_error: None,
                    live_error: None,
                    apply_error: None,
                })
                .collect(),
            store,
            prices,
            sampler: Sampler::new(),
            meter: ActivityMeter::new(RATE_WINDOW_MS),
            session_meters: HashMap::new(),
            metered: HashMap::new(),
            table: None,
            cache: None,
            rate_history: VecDeque::with_capacity(HISTORY_LEN),
            config_errors,
            query_error: None,
            ingested: IngestStats::default(),
        })
    }

    pub fn tick(&mut self) -> Snapshot {
        let now = now_ms();
        let midnight = local_midnight_ms(now);
        let mut changed = false;

        for src in &mut self.sources {
            let provider = src.adapter.provider();
            match src.adapter.poll(&self.offsets, midnight.min(now - DAY_MS)) {
                Ok(batch) => {
                    src.poll_error = None;
                    src.more = batch.more;
                    if !batch.is_empty() {
                        match self.store.apply(&batch, &|e| self.prices.cost(e)) {
                            Ok(()) => {
                                src.apply_error = None;
                                changed = true;
                                for f in &batch.files {
                                    let old = self.offsets.insert(f.path.clone(), f.offset);
                                    self.ingested.bytes +=
                                        f.offset.saturating_sub(old.unwrap_or(0));
                                    self.ingested.files += 1;
                                }
                                self.ingested.events += batch.events.len() as u64;
                                // Backfilled history is older than the window and never counts as live output.
                                for e in batch
                                    .events
                                    .iter()
                                    .filter(|e| e.ts_ms > now - RATE_WINDOW_MS)
                                {
                                    let key = (provider, e.event_id.clone());
                                    let seen = self.metered.get(&key).map_or(0, |m| m.1);
                                    if e.usage.output <= seen {
                                        continue;
                                    }
                                    let added = e.usage.output - seen;
                                    self.metered.insert(key, (e.ts_ms, e.usage.output));
                                    self.meter.record(e.ts_ms, added);
                                    self.session_meters
                                        .entry((provider, e.session_id.clone()))
                                        .or_insert_with(|| ActivityMeter::new(RATE_WINDOW_MS))
                                        .record(e.ts_ms, added);
                                }
                            }
                            Err(e) => {
                                tracing::error!(error = %format!("{e:#}"), "cannot store poll batch");
                                src.apply_error = Some(format!("Storage: {e:#}"));
                                // The same batch would be re-read at once; wait for the next tick instead.
                                src.more = false;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), provider = provider.label(), "adapter poll failed");
                    src.poll_error = Some(format!("{}: {e:#}", provider.label()));
                    src.more = false;
                }
            }

            src.live = match src.adapter.live_sessions() {
                Ok(l) => {
                    src.live_error = None;
                    l
                }
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), provider = provider.label(), "cannot read live sessions");
                    src.live_error = Some(format!("{} live sessions: {e:#}", provider.label()));
                    Vec::new()
                }
            };
        }

        if self.sources.iter().all(|s| s.live.is_empty()) {
            self.table = None;
        } else if self
            .table
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= PROCESS_INTERVAL)
        {
            if self.table.is_none() {
                // After a pause, CPU deltas against a long-gone sample would mislead.
                self.sampler = Sampler::new();
            }
            self.table = Some((Instant::now(), self.sampler.sample()));
        }
        let table = self.table.as_ref().map(|(_, t)| t);

        let verified: Vec<(Provider, &LiveSession)> = self
            .sources
            .iter()
            .flat_map(|s| s.live.iter().map(|l| (s.adapter.provider(), l)))
            .filter(|(_, l)| table.is_some_and(|t| verify(t, l).is_some()))
            .collect();
        let live_ids: BTreeSet<Key> = verified
            .iter()
            .map(|(p, l)| (*p, l.session_id.clone()))
            .collect();
        let cutoff = verified
            .iter()
            .filter_map(|(_, l)| l.started_at_ms)
            .fold(now - DAY_MS, i64::min);

        self.metered.retain(|_, m| m.0 > now - RATE_WINDOW_MS);

        if cache_stale(self.cache.as_ref(), changed, &live_ids, midnight) {
            let rows: anyhow::Result<Vec<Vec<SessionRow>>> = self
                .sources
                .iter()
                .map(|s| self.store.sessions_since(s.adapter.provider(), cutoff))
                .collect();
            let totals = self.store.totals_since(midnight);
            match rows.and_then(|r| Ok((r, totals?))) {
                Ok((rows, totals)) => {
                    self.query_error = None;
                    self.cache = Some(Cache {
                        at: Instant::now(),
                        midnight,
                        live: live_ids,
                        rows,
                        totals,
                    });
                }
                Err(e) => {
                    tracing::error!(error = %format!("{e:#}"), "cannot query sessions");
                    self.query_error = Some(format!("Storage: {e:#}"));
                }
            }
        }

        let rates: HashMap<Key, f32> = self
            .session_meters
            .iter_mut()
            .map(|(k, m)| (k.clone(), m.rate_per_sec(now) as f32))
            .collect();
        let empty = Totals {
            tokens: TokenUsage::default(),
            cost_usd: 0.0,
            priced_events: 0,
            unpriced_events: 0,
            sessions: 0,
        };
        let totals = self.cache.as_ref().map_or(&empty, |c| &c.totals);
        let groups: Vec<Group> = self
            .sources
            .iter()
            .enumerate()
            .map(|(i, s)| Group {
                provider: s.adapter.provider(),
                rows: self
                    .cache
                    .as_ref()
                    .and_then(|c| c.rows.get(i))
                    .map_or(&[][..], |r| &r[..]),
                live: &s.live,
            })
            .collect();
        let mut snap = build_snapshot(&groups, totals, table, &rates, now);

        self.session_meters.retain(|(p, id), _| {
            snap.sessions
                .iter()
                .any(|s| s.provider == *p && &s.session_id == id)
        });
        snap.output_rate = self.meter.rate_per_sec(now) as f32;
        snap.system_gpu_percent = gpu_utilization().map_or(Reading::Unavailable, Reading::Measured);
        if self.rate_history.len() == HISTORY_LEN {
            self.rate_history.pop_front();
        }
        self.rate_history.push_back(snap.output_rate);
        snap.rate_history = self.rate_history.iter().copied().collect();
        snap.indexing = self.sources.iter().any(|s| s.more);
        snap.errors = self
            .config_errors
            .iter()
            .cloned()
            .chain(self.sources.iter().flat_map(|s| {
                [&s.poll_error, &s.live_error, &s.apply_error]
                    .into_iter()
                    .flatten()
                    .cloned()
            }))
            .chain(self.query_error.clone())
            .collect();
        snap
    }
}

/// Whether stored rows and totals must be queried again: after new data, a
/// change in the live set, local midnight, or every [`REQUERY_INTERVAL`].
fn cache_stale(cache: Option<&Cache>, changed: bool, live: &BTreeSet<Key>, midnight: i64) -> bool {
    cache.is_none_or(|c| {
        changed || &c.live != live || c.midnight != midnight || c.at.elapsed() >= REQUERY_INTERVAL
    })
}

/// One adapter's stored rows and live sessions.
pub struct Group<'a> {
    pub provider: Provider,
    pub rows: &'a [SessionRow],
    pub live: &'a [LiveSession],
}

/// Everything in a [`Snapshot`] that follows from stored rows, the live
/// sessions and the process table. Rates, history, indexing and errors are
/// left empty for the caller.
///
/// Every verified live process is a root of process attribution, including a
/// process shared by several sessions: its tree is cut out of any agent above
/// it (nested agents never count twice) but shown for none of its sessions.
pub fn build_snapshot(
    groups: &[Group],
    totals: &Totals,
    table: Option<&ProcTable>,
    rates: &HashMap<Key, f32>,
    now_ms: i64,
) -> Snapshot {
    let verified: Vec<HashMap<&str, (&LiveSession, ProcessIdentity)>> = groups
        .iter()
        .map(|g| {
            g.live
                .iter()
                .filter_map(|l| Some((l.session_id.as_str(), (l, verify(table?, l)?))))
                .collect()
        })
        .collect();
    let roots: Vec<ProcessIdentity> = verified
        .iter()
        .flat_map(|v| v.values().map(|(_, p)| *p))
        .collect();
    let usage = table.map(|t| attribute(t, &roots)).unwrap_or_default();

    let mut sessions: Vec<AgentSession> = Vec::new();
    for (g, verified) in groups.iter().zip(&verified) {
        // A live session whose transcript has no rows yet still gets a row.
        let missing: Vec<SessionRow> = verified
            .iter()
            .filter(|(id, _)| !g.rows.iter().any(|r| r.session_id == **id))
            .map(|(id, (l, _))| SessionRow {
                session_id: id.to_string(),
                project_path: l.project_path.clone(),
                model: None,
                started_at_ms: l.started_at_ms,
                last_activity_at_ms: None,
                active_ms: 0,
                last_error: false,
                tokens: TokenUsage::default(),
                cost_usd: 0.0,
                priced_events: 0,
                unpriced_events: 0,
                recorded_cost: None,
            })
            .collect();
        sessions.extend(
            g.rows
                .iter()
                .filter(|r| {
                    verified.contains_key(r.session_id.as_str())
                        || r.last_activity_at_ms.is_some_and(|t| t >= now_ms - DAY_MS)
                })
                .chain(&missing)
                .map(|r| {
                    let live = verified.get(r.session_id.as_str());
                    let status = resolve_status(
                        live.map(|(l, _)| l.state),
                        r.last_activity_at_ms,
                        r.last_error,
                        now_ms,
                    );
                    let process = live.filter(|(l, _)| !l.shared_process).map(|(_, p)| *p);
                    AgentSession {
                        provider: g.provider,
                        session_id: r.session_id.clone(),
                        project_path: r
                            .project_path
                            .clone()
                            .or_else(|| live.and_then(|(l, _)| l.project_path.clone())),
                        model: r.model.clone(),
                        status,
                        started_at_ms: r
                            .started_at_ms
                            .or_else(|| live.and_then(|(l, _)| l.started_at_ms)),
                        last_activity_at_ms: r.last_activity_at_ms,
                        ended_at_ms: r.last_activity_at_ms.filter(|_| !status.is_live()),
                        active_ms: r.active_ms,
                        tokens: r.tokens,
                        cost_usd: cost_reading(r.cost_usd, r.priced_events, r.unpriced_events),
                        unpriced_events: r.unpriced_events,
                        output_rate: rates
                            .get(&(g.provider, r.session_id.clone()))
                            .copied()
                            .unwrap_or(0.0),
                        process,
                        resources: process
                            .and_then(|p| usage.get(&p.pid))
                            .map_or(Reading::Unavailable, |u| Reading::Measured(*u)),
                        recorded_cost: r
                            .recorded_cost
                            .map_or(Reading::Unavailable, Reading::Measured),
                    }
                }),
        );
    }

    let rank = |s: SessionStatus| match s {
        SessionStatus::Active => 0,
        SessionStatus::Waiting => 1,
        SessionStatus::Idle => 2,
        SessionStatus::Completed | SessionStatus::Failed => 3,
    };
    let recency = |s: &AgentSession| s.last_activity_at_ms.or(s.started_at_ms);
    sessions.sort_by(|a, b| {
        rank(a.status)
            .cmp(&rank(b.status))
            .then_with(|| recency(b).cmp(&recency(a)))
            .then_with(|| a.session_id.cmp(&b.session_id))
            .then_with(|| a.provider.cmp(&b.provider))
    });

    let measured: Vec<_> = sessions
        .iter()
        .filter_map(|s| s.resources.value())
        .collect();
    Snapshot {
        taken_at_ms: now_ms,
        today_tokens: totals.tokens,
        today_cost_usd: cost_reading(
            totals.cost_usd,
            totals.priced_events,
            totals.unpriced_events,
        ),
        today_unpriced_events: totals.unpriced_events,
        output_rate: 0.0,
        rate_history: Vec::new(),
        agents_cpu_percent: if measured.is_empty() {
            None
        } else {
            measured.iter().map(|u| u.cpu_percent).sum()
        },
        agents_rss_bytes: (!measured.is_empty())
            .then(|| measured.iter().map(|u| u.rss_bytes).sum()),
        system_gpu_percent: Reading::Unavailable,
        indexing: false,
        errors: Vec::new(),
        sessions,
    }
}

fn cost_reading(usd: f64, priced: u64, unpriced: u64) -> Reading<f64> {
    if priced == 0 && unpriced > 0 {
        Reading::Unavailable
    } else {
        Reading::Estimated(usd)
    }
}

fn now_ms() -> i64 {
    Local::now().timestamp_millis()
}

fn local_midnight_ms(now_ms: i64) -> i64 {
    DateTime::from_timestamp_millis(now_ms)
        .map(|t| t.with_timezone(&Local).date_naive())
        .and_then(|d| d.and_hms_opt(0, 0, 0)?.and_local_timezone(Local).earliest())
        .map_or(now_ms - DAY_MS, |t| t.timestamp_millis())
}

/// Runs the collector on its own thread until the receiver is dropped,
/// publishing each snapshot to the widgets when `widgets` is set. If the loop
/// panics, the widgets are marked stopped and the thread ends, which closes
/// the channel.
pub fn spawn(
    mut collector: Collector,
    sender: async_channel::Sender<Snapshot>,
    mut widgets: Option<WidgetExporter>,
) -> std::io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("agentpulse-collector".into())
        .spawn(move || {
            // The collector is dropped right after a panic, so its state is never reused.
            let run = std::panic::catch_unwind(AssertUnwindSafe(|| {
                loop {
                    let snap = collector.tick();
                    let indexing = snap.indexing;
                    let live = snap.sessions.iter().any(|s| s.status.is_live());
                    if let Some(w) = &mut widgets {
                        w.publish(&snap);
                    }
                    if sender.send_blocking(snap).is_err() {
                        return;
                    }
                    if !indexing {
                        thread::sleep(Duration::from_secs(if live { 1 } else { 5 }));
                    }
                }
            }));
            if let Err(panic) = run {
                let msg = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                tracing::error!(panic = %msg, "collector thread panicked; data collection stopped");
                if let Some(w) = &widgets {
                    w.stop();
                }
            }
        })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use agentpulse_core::{FileCursor, LiveState, PollBatch, SessionDelta, UsageEvent};
    use agentpulse_process::ProcInfo;

    use super::*;

    const P: Provider = Provider::ClaudeCode;
    const NOW: i64 = 1_800_000_000_000;
    const START_S: u64 = 1_799_990_000;

    #[derive(Clone)]
    struct Fake {
        provider: Provider,
        polls: Arc<Mutex<VecDeque<anyhow::Result<PollBatch>>>>,
    }

    impl Fake {
        fn new(provider: Provider) -> Self {
            Self {
                provider,
                polls: Arc::default(),
            }
        }
    }

    impl AgentAdapter for Fake {
        fn provider(&self) -> Provider {
            self.provider
        }

        fn poll(&mut self, _: &HashMap<String, u64>, _: i64) -> anyhow::Result<PollBatch> {
            self.polls.lock().unwrap().pop_front().unwrap_or_else(|| {
                let mut b = batch(vec![], vec![]);
                b.provider = self.provider;
                Ok(b)
            })
        }

        fn live_sessions(&self) -> anyhow::Result<Vec<LiveSession>> {
            Ok(vec![])
        }
    }

    fn collector() -> (Collector, Fake) {
        let fake = Fake::new(P);
        let c = Collector::new(
            vec![Box::new(fake.clone())],
            Store::open_in_memory().unwrap(),
            PriceBook::builtin(),
            vec![],
        )
        .unwrap();
        (c, fake)
    }

    fn snapshot(rows: &[SessionRow], live: &[LiveSession], t: Option<&ProcTable>) -> Snapshot {
        let g = [Group {
            provider: P,
            rows,
            live,
        }];
        build_snapshot(&g, &totals(), t, &HashMap::new(), NOW)
    }

    fn batch(events: Vec<UsageEvent>, sessions: Vec<SessionDelta>) -> PollBatch {
        PollBatch {
            provider: P,
            files: vec![FileCursor {
                path: "/synthetic/a.jsonl".into(),
                offset: 1,
            }],
            events,
            sessions,
            more: false,
        }
    }

    fn event(id: &str, session: &str, model: &str, ts: i64, output: u64) -> UsageEvent {
        UsageEvent {
            event_id: id.into(),
            session_id: session.into(),
            model: model.into(),
            ts_ms: ts,
            usage: TokenUsage {
                input: 10,
                output,
                ..Default::default()
            },
            fast: false,
            us_inference: false,
            unknown_tier: false,
        }
    }

    fn delta(session: &str, ts: &[i64]) -> SessionDelta {
        SessionDelta {
            session_id: session.into(),
            project_path: Some(format!("/synthetic/{session}")),
            timestamps_ms: ts.to_vec(),
            ..Default::default()
        }
    }

    fn row(id: &str, last: i64, output: u64) -> SessionRow {
        SessionRow {
            session_id: id.into(),
            project_path: Some(format!("/synthetic/{id}")),
            model: Some("m".into()),
            started_at_ms: Some(last - 60_000),
            last_activity_at_ms: Some(last),
            active_ms: 1_000,
            last_error: false,
            tokens: TokenUsage {
                output,
                ..Default::default()
            },
            cost_usd: 0.5,
            priced_events: 1,
            unpriced_events: 0,
            recorded_cost: None,
        }
    }

    fn live(id: &str, pid: u32, state: LiveState) -> LiveSession {
        LiveSession {
            session_id: id.into(),
            pid,
            proc_start_s: Some(START_S),
            started_at_ms: Some(START_S as i64 * 1000),
            state,
            project_path: Some(format!("/registry/{id}")),
            shared_process: false,
        }
    }

    fn table(pids: &[u32], cpu_valid: bool) -> ProcTable {
        let info = |ppid| ProcInfo {
            ppid: Some(ppid),
            start_time_s: START_S,
            cpu_percent: 2.5,
            rss_bytes: 1_000,
        };
        let mut procs: HashMap<u32, ProcInfo> = pids.iter().map(|&p| (p, info(1))).collect();
        // One child per agent, attributed to its parent.
        procs.extend(pids.iter().map(|&p| (p + 1, info(p))));
        ProcTable { procs, cpu_valid }
    }

    fn totals() -> Totals {
        Totals {
            tokens: TokenUsage::default(),
            cost_usd: 0.0,
            priced_events: 0,
            unpriced_events: 0,
            sessions: 0,
        }
    }

    fn get<'a>(s: &'a Snapshot, id: &str) -> &'a AgentSession {
        s.sessions.iter().find(|x| x.session_id == id).unwrap()
    }

    #[test]
    fn adapter_error_keeps_stored_sessions_and_clears_on_success() {
        let (mut c, fake) = collector();
        let t = now_ms() - 60_000;
        {
            let mut q = fake.polls.lock().unwrap();
            q.push_back(Ok(batch(
                vec![event("e1", "s1", "claude-sonnet-4-5", t, 5)],
                vec![delta("s1", &[t])],
            )));
            q.push_back(Err(anyhow::anyhow!("synthetic failure")));
        }
        let first = c.tick();
        assert!(first.errors.is_empty());
        assert_eq!(first.sessions.len(), 1);

        let failed = c.tick();
        assert_eq!(failed.sessions.len(), 1);
        assert_eq!(failed.sessions[0].tokens.output, 5);
        assert!(
            failed
                .errors
                .iter()
                .any(|e| e.contains("synthetic failure"))
        );
        assert!(!failed.indexing);

        assert!(c.tick().errors.is_empty());
    }

    #[test]
    fn concurrent_live_sessions_sort_before_ended() {
        let rows = [
            row("busy", NOW - 50_000, 1),
            row("wait", NOW - 40_000, 2),
            row("idle", NOW - 1_000, 3),
            row("done", NOW - 3_600_000, 4),
        ];
        let live = [
            live("busy", 100, LiveState::Busy),
            live("wait", 200, LiveState::Waiting),
            live("idle", 300, LiveState::Idle),
        ];
        let t = table(&[100, 200, 300], true);
        let s = snapshot(&rows, &live, Some(&t));
        let order: Vec<_> = s.sessions.iter().map(|x| x.session_id.as_str()).collect();
        assert_eq!(order, ["busy", "wait", "idle", "done"]);
        let statuses: Vec<_> = s.sessions.iter().map(|x| x.status).collect();
        use SessionStatus::*;
        assert_eq!(statuses, [Active, Waiting, Idle, Completed]);
        let outputs: Vec<_> = s.sessions.iter().map(|x| x.tokens.output).collect();
        assert_eq!(outputs, [1, 2, 3, 4]);
        assert_eq!(get(&s, "busy").process.unwrap().pid, 100);
        assert_eq!(
            get(&s, "busy").resources.value().unwrap().process_count,
            2,
            "child counted with its agent"
        );
        assert_eq!(get(&s, "done").ended_at_ms, Some(NOW - 3_600_000));
        assert_eq!(get(&s, "busy").ended_at_ms, None);
        assert_eq!(s.agents_cpu_percent, Some(15.0));
        assert_eq!(s.agents_rss_bytes, Some(6_000));
    }

    #[test]
    fn dead_or_reused_pid_is_not_live() {
        let rows = [
            row("gone", NOW - 60_000, 1),
            row("reused", NOW - 600_000, 1),
        ];
        let mut reused = live("reused", 200, LiveState::Busy);
        reused.proc_start_s = Some(START_S - 3_600);
        let live = [live("gone", 100, LiveState::Busy), reused];
        let t = table(&[200], true);
        let s = snapshot(&rows, &live, Some(&t));
        for id in ["gone", "reused"] {
            let x = get(&s, id);
            assert_eq!(x.process, None);
            assert_eq!(x.resources, Reading::Unavailable);
        }
        // Transcript-age rule: 1 min silent is Idle, 10 min is ended.
        assert_eq!(get(&s, "gone").status, SessionStatus::Idle);
        assert_eq!(get(&s, "reused").status, SessionStatus::Completed);
        assert_eq!(s.agents_cpu_percent, None);
        assert_eq!(s.agents_rss_bytes, None);
    }

    #[test]
    fn live_session_without_transcript_appears() {
        let live = [live("fresh", 100, LiveState::Idle)];
        let t = table(&[100], true);
        let s = snapshot(&[], &live, Some(&t));
        let x = get(&s, "fresh");
        assert_eq!(x.status, SessionStatus::Idle);
        assert_eq!(x.tokens, TokenUsage::default());
        assert_eq!(x.project_path.as_deref(), Some("/registry/fresh"));
        assert_eq!(x.started_at_ms, Some(START_S as i64 * 1000));
        assert_eq!(x.cost_usd, Reading::Estimated(0.0));
    }

    #[test]
    fn unpriced_cost_is_unavailable_and_no_events_cost_nothing() {
        let (mut c, fake) = collector();
        let t = now_ms() - 60_000;
        fake.polls.lock().unwrap().push_back(Ok(batch(
            vec![event("e1", "s1", "no-such-model", t, 5)],
            vec![delta("s1", &[t]), delta("s2", &[t])],
        )));
        let s = c.tick();
        assert_eq!(get(&s, "s1").cost_usd, Reading::Unavailable);
        assert_eq!(get(&s, "s1").unpriced_events, 1);
        assert_eq!(get(&s, "s2").cost_usd, Reading::Estimated(0.0));
        assert_eq!(s.today_cost_usd, Reading::Unavailable);
        assert_eq!(s.today_unpriced_events, 1);
    }

    #[test]
    fn backfilled_events_do_not_raise_rate() {
        let (mut c, fake) = collector();
        let old = now_ms() - 600_000;
        fake.polls.lock().unwrap().push_back(Ok(batch(
            vec![event("old", "s1", "m", old, 3_000)],
            vec![delta("s1", &[old])],
        )));
        let s = c.tick();
        assert_eq!(s.output_rate, 0.0);
        assert_eq!(get(&s, "s1").output_rate, 0.0);

        let recent = now_ms() - 1_000;
        fake.polls.lock().unwrap().push_back(Ok(batch(
            vec![event("new", "s1", "m", recent, 300)],
            vec![delta("s1", &[recent])],
        )));
        let s = c.tick();
        assert_eq!(s.output_rate, 10.0);
        assert_eq!(get(&s, "s1").output_rate, 10.0);
        assert_eq!(s.rate_history, [0.0, 10.0]);
    }

    #[test]
    fn rereading_a_response_meters_only_its_growth() {
        let (mut c, fake) = collector();
        let ts = now_ms() - 1_000;
        let partial = event("r1", "s1", "m", ts, 10);
        let full = event("r1", "s1", "m", ts, 100);
        {
            let mut q = fake.polls.lock().unwrap();
            q.push_back(Ok(batch(vec![partial], vec![delta("s1", &[ts])])));
            q.push_back(Ok(batch(vec![full.clone()], vec![])));
            q.push_back(Ok(batch(vec![full], vec![])));
        }
        c.tick();
        c.tick();
        let s = c.tick();
        // 100 tokens over the 30 s window, not 110 or 210.
        assert!(
            (s.output_rate - 100.0 / 30.0).abs() < 1e-4,
            "{}",
            s.output_rate
        );
        assert!((get(&s, "s1").output_rate - 100.0 / 30.0).abs() < 1e-4);

        let (mut c, fake) = collector();
        let same = event("r2", "s1", "m", ts, 100);
        {
            let mut q = fake.polls.lock().unwrap();
            for _ in 0..3 {
                q.push_back(Ok(batch(vec![same.clone()], vec![delta("s1", &[ts])])));
            }
        }
        c.tick();
        c.tick();
        assert!((c.tick().output_rate - 100.0 / 30.0).abs() < 1e-4);
    }

    #[test]
    fn cache_is_stale_after_midnight() {
        let live = BTreeSet::new();
        let cache = Cache {
            at: Instant::now(),
            midnight: NOW,
            live: live.clone(),
            rows: vec![],
            totals: totals(),
        };
        assert!(!cache_stale(Some(&cache), false, &live, NOW));
        assert!(cache_stale(Some(&cache), false, &live, NOW + DAY_MS));
        assert!(cache_stale(Some(&cache), true, &live, NOW));
        assert!(cache_stale(
            Some(&cache),
            false,
            &[(P, "s".to_string())].into(),
            NOW
        ));
        assert!(cache_stale(None, false, &live, NOW));
    }

    #[test]
    fn recorded_cost_is_per_session_and_stays_out_of_today() {
        let c = agentpulse_core::RecordedCost {
            usd: 9.0,
            as_of_ms: NOW - 5_000,
            unknown_models: true,
        };
        let rows = [
            SessionRow {
                recorded_cost: Some(c),
                ..row("costed", NOW - 1_000, 1)
            },
            row("plain", NOW - 2_000, 1),
        ];
        let mut t = totals();
        t.cost_usd = 1.0;
        t.priced_events = 2;
        let g = [Group {
            provider: P,
            rows: &rows,
            live: &[],
        }];
        let s = build_snapshot(&g, &t, None, &HashMap::new(), NOW);
        assert_eq!(get(&s, "costed").recorded_cost, Reading::Measured(c));
        assert_eq!(get(&s, "costed").cost_usd, Reading::Estimated(0.5));
        assert_eq!(get(&s, "plain").recorded_cost, Reading::Unavailable);
        assert_eq!(s.today_cost_usd, Reading::Estimated(1.0));
    }

    #[test]
    fn a_failing_adapter_keeps_the_other_agents_data() {
        let (claude, codex) = (Fake::new(Provider::ClaudeCode), Fake::new(Provider::Codex));
        let mut c = Collector::new(
            vec![Box::new(claude.clone()), Box::new(codex.clone())],
            Store::open_in_memory().unwrap(),
            PriceBook::builtin(),
            vec![],
        )
        .unwrap();
        let t = now_ms() - 60_000;
        claude.polls.lock().unwrap().push_back(Ok(batch(
            vec![event("e1", "s1", "claude-sonnet-5-5", t, 5)],
            vec![delta("s1", &[t])],
        )));
        codex
            .polls
            .lock()
            .unwrap()
            .push_back(Err(anyhow::anyhow!("synthetic codex failure")));
        let s = c.tick();
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(s.sessions[0].provider, Provider::ClaudeCode);
        assert_eq!(s.sessions[0].tokens.output, 5);
        assert_eq!(s.errors.len(), 1);
        assert!(s.errors[0].starts_with("Codex: synthetic codex failure"));
        assert!(c.tick().errors.is_empty());
    }

    #[test]
    fn same_session_id_under_two_providers_stays_separate() {
        let (claude, codex) = (Fake::new(Provider::ClaudeCode), Fake::new(Provider::Codex));
        let mut c = Collector::new(
            vec![Box::new(claude.clone()), Box::new(codex.clone())],
            Store::open_in_memory().unwrap(),
            PriceBook::builtin(),
            vec![],
        )
        .unwrap();
        let t = now_ms() - 1_000;
        claude.polls.lock().unwrap().push_back(Ok(batch(
            vec![event("same-event", "same", "claude-sonnet-5-5", t, 30)],
            vec![delta("same", &[t])],
        )));
        let mut b = batch(
            vec![event("same-event", "same", "gpt-6.1-sol", t, 60)],
            vec![delta("same", &[t])],
        );
        b.provider = Provider::Codex;
        b.files[0].path = "/synthetic/codex.jsonl".into();
        codex.polls.lock().unwrap().push_back(Ok(b));
        let s = c.tick();
        assert_eq!(s.sessions.len(), 2);
        let by = |p: Provider| s.sessions.iter().find(|x| x.provider == p).unwrap();
        assert_eq!(by(Provider::ClaudeCode).tokens.output, 30);
        assert_eq!(by(Provider::Codex).tokens.output, 60);
        assert_eq!(by(Provider::ClaudeCode).output_rate, 1.0);
        assert_eq!(by(Provider::Codex).output_rate, 2.0);
        assert_eq!(s.today_tokens.output, 90);
    }

    #[test]
    fn shared_codex_process_is_live_but_counted_for_no_one() {
        let info = |ppid, rss| ProcInfo {
            ppid: Some(ppid),
            start_time_s: START_S,
            cpu_percent: 1.0,
            rss_bytes: rss,
        };
        // Claude 100 → child 101 and codex app-server 200 → its child 201.
        let t = ProcTable {
            procs: HashMap::from([
                (100, info(1, 1)),
                (101, info(100, 10)),
                (200, info(100, 100)),
                (201, info(200, 1_000)),
            ]),
            cpu_valid: true,
        };
        let claude = [live("c", 100, LiveState::Busy)];
        let rows = [row("c", NOW - 1_000, 1)];
        let codex_rows = [row("a", NOW - 1_000, 1), row("b", NOW - 2_000, 1)];
        // An app-server hosting sessions a and b; then a process hosting only a.
        let shared = ["a", "b"].map(|id| LiveSession {
            shared_process: true,
            ..live(id, 200, LiveState::Idle)
        });
        let alone = [live("a", 200, LiveState::Idle)];
        for codex in [&shared[..], &alone[..]] {
            let is_shared = codex.len() == 2;
            let g = [
                Group {
                    provider: Provider::ClaudeCode,
                    rows: &rows,
                    live: &claude,
                },
                Group {
                    provider: Provider::Codex,
                    rows: &codex_rows,
                    live: codex,
                },
            ];
            let s = build_snapshot(&g, &totals(), Some(&t), &HashMap::new(), NOW);
            let c = get(&s, "c").resources.value().copied().unwrap();
            // The codex tree is a nested agent either way: never in Claude's.
            assert_eq!(
                (c.process_count, c.rss_bytes),
                (2, 11),
                "shared={is_shared}"
            );
            let a = get(&s, "a");
            assert_eq!(a.provider, Provider::Codex);
            assert_eq!(a.status, SessionStatus::Idle, "live either way");
            if is_shared {
                for x in [a, get(&s, "b")] {
                    assert_eq!(x.status, SessionStatus::Idle);
                    assert_eq!((x.process, x.resources), (None, Reading::Unavailable));
                }
                // Counted for no one, so not in the totals either.
                assert_eq!(s.agents_rss_bytes, Some(11));
            } else {
                assert_eq!(a.process.unwrap().pid, 200);
                assert_eq!(a.resources.value().unwrap().rss_bytes, 1_100);
                assert_eq!(s.agents_rss_bytes, Some(11 + 1_100));
            }
        }
    }

    #[test]
    fn cpu_unavailable_until_valid_but_rss_summed() {
        let rows = [row("a", NOW - 1_000, 1)];
        let live = [live("a", 100, LiveState::Busy)];
        let t = table(&[100], false);
        let s = snapshot(&rows, &live, Some(&t));
        assert_eq!(s.agents_cpu_percent, None);
        assert_eq!(s.agents_rss_bytes, Some(2_000));
    }
}
