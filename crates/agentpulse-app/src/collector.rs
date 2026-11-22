//! The collector loop: polls each adapter, stores what it read, samples agent
//! processes and turns it all into a [`Snapshot`]. Knows nothing about GPUI.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use agentpulse_core::{
    ActivityMeter, AgentAdapter, AgentSession, LiveSession, ProcessIdentity, Provider, RateLimits,
    Reading, SessionStatus, TokenUsage, resolve_status,
};
use agentpulse_cost::PriceBook;
use agentpulse_platform_macos::gpu_utilization;
use agentpulse_process::{ProcTable, Sampler, attribute, verify};
use agentpulse_storage::{DayUsage, Lane, ModelCacheReads, Request, SessionRow, Store, Totals};
use chrono::{DateTime, Local, NaiveDate, TimeZone};

use crate::cleanup::{CleanupView, Host, SWEEP_INTERVAL, Tracker};
use crate::phones::PhoneLink;
use crate::settings::{Cleanup, Settings};
use crate::widget_export::{self, WidgetExporter};

const RATE_WINDOW_MS: i64 = 30_000;
pub const HISTORY_LEN: usize = 120;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
pub const PROCESS_INTERVAL: Duration = Duration::from_secs(2);
const REQUERY_INTERVAL: Duration = Duration::from_secs(30);
/// Days in the Usage charts, today included.
pub const USAGE_DAYS: usize = 30;
/// Past days of the Usage charts are queried again at most this often (and at midnight).
const HISTORY_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// After a failed history query, the next try waits at least this long.
const HISTORY_RETRY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, serde::Serialize)]
pub struct Snapshot {
    pub taken_at_ms: i64,
    /// Every shown agent's sessions. Live first (Active, Waiting, Idle), then ended;
    /// each group newest activity first.
    pub sessions: Vec<AgentSession>,
    /// Since local midnight, all shown agents.
    pub today_tokens: TokenUsage,
    /// Estimated, or Unavailable when there are events but none is priced.
    pub today_cost_usd: Reading<f64>,
    pub today_unpriced_events: u64,
    /// Output tokens/s over the last 30 s, all shown agents.
    pub output_rate: f32,
    /// One (unix ms, output tokens/s) sample per tick, most recent last, at most 120.
    pub rate_history: Vec<(i64, f32)>,
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
    /// None until the history has been indexed and queried.
    pub usage: Option<UsageCharts>,
    /// Each shown agent's newest plan-limit reading, in memory only.
    pub limits: BTreeMap<Provider, RateLimits>,
    /// The session the window asked for with [`Collector::show_detail`].
    #[serde(skip)]
    pub detail: Option<Arc<SessionDetail>>,
    /// The last clean-up sweep; empty when the tick ran without clean-up.
    pub cleanup: CleanupView,
}

/// What the window asks of the collector thread.
pub enum Command {
    /// Which session's requests the snapshots carry; None for none.
    Detail(Option<Key>),
    End(ProcessIdentity),
    Keep(ProcessIdentity),
    /// Nothing but a tick now, such as after a setting changed.
    Refresh,
}

/// What the Usage screen draws.
#[derive(Clone, Debug, serde::Serialize)]
pub struct UsageCharts {
    /// The last [`USAGE_DAYS`] local days, oldest first, today last.
    pub days: Vec<Day>,
    /// Estimated net USD caching saved over those days: cache reads' saving
    /// minus cache writes' premium over input. None while a day in the
    /// window is not covered by a query yet.
    pub cache_saved_usd: Option<f64>,
    /// Cache-using requests without a price (model or tier), left out of the saving.
    pub cache_saved_unpriced: u64,
    /// Sessions with usage today, each with its agent, ordered by first event.
    pub today: Vec<(Provider, Lane)>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Day {
    /// Local midnight that starts the day.
    pub start_ms: i64,
    /// Per agent: None before its first stored event (no data); otherwise its
    /// usage, all zero when it made no requests that day.
    pub agents: Vec<(Provider, Option<DayUsage>)>,
}

/// Every request of one session, for its detail view.
#[derive(Clone, Debug)]
pub struct SessionDetail {
    pub key: Key,
    pub requests: Vec<Request>,
    /// Distinct long-context thresholds of the models it used, ascending.
    pub long_context: Vec<u64>,
}

/// Past days of the Usage charts, which change rarely.
struct History {
    at: Instant,
    midnight: i64,
    /// Per source: past days' usage and the agent's first stored event.
    days: Vec<(Vec<DayUsage>, Option<i64>)>,
    cache_reads: Vec<ModelCacheReads>,
}

/// Today's part of the Usage charts, queried with the session rows.
struct Today {
    /// Local midnight it was queried from.
    midnight: i64,
    days: Vec<Vec<DayUsage>>,
    cache_reads: Vec<ModelCacheReads>,
    lanes: Vec<(Provider, Lane)>,
}

/// Cumulative ingestion counters, for `--dump`.
#[derive(Clone, Copy, Debug, Default)]
pub struct IngestStats {
    pub bytes: u64,
    pub files: u64,
    pub events: u64,
}

/// A session as the collector keys it: ids are unique per provider only.
pub type Key = (Provider, String);

struct Cache {
    at: Instant,
    /// Local midnight the totals were queried from.
    midnight: i64,
    live: BTreeSet<Key>,
    rows: Vec<Vec<SessionRow>>,
    totals: BTreeMap<Provider, Totals>,
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
    /// Per agent, so a hidden agent's output leaves the rate.
    meters: BTreeMap<Provider, ActivityMeter>,
    session_meters: HashMap<Key, ActivityMeter>,
    /// Output tokens already fed to the meters per event (with the event's
    /// time), so a response re-read with fuller usage adds only the increase.
    metered: HashMap<Key, (i64, u64)>,
    table: Option<(Instant, ProcTable)>,
    cache: Option<Cache>,
    rate_history: VecDeque<(i64, f32)>,
    config_errors: Vec<String>,
    query_error: Option<String>,
    history_error: Option<(Instant, String)>,
    detail_error: Option<String>,
    history: Option<History>,
    today: Option<Today>,
    limits: BTreeMap<Provider, RateLimits>,
    /// Left out of the snapshots, still collected.
    hidden: BTreeSet<Provider>,
    /// The last tick saw a live session, hidden or not: the loop's pace.
    live: bool,
    detail_key: Option<Key>,
    detail: Option<Arc<SessionDetail>>,
    tracker: Tracker,
    last_sweep: Option<Instant>,
    /// The last sweep's view, copied into every snapshot taken with clean-up.
    cleanup: CleanupView,
    sweep_error: Option<String>,
    /// From the last End or Keep; kept until a regular sweep after it, so the
    /// sweep it forces does not hide it.
    command_error: Option<String>,
    observe_error: Option<String>,
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
            tracker: Tracker::load(&store)?,
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
            meters: BTreeMap::new(),
            session_meters: HashMap::new(),
            metered: HashMap::new(),
            table: None,
            cache: None,
            rate_history: VecDeque::with_capacity(HISTORY_LEN),
            config_errors,
            query_error: None,
            history_error: None,
            detail_error: None,
            history: None,
            today: None,
            limits: BTreeMap::new(),
            hidden: BTreeSet::new(),
            live: false,
            detail_key: None,
            detail: None,
            last_sweep: None,
            cleanup: CleanupView::default(),
            sweep_error: None,
            command_error: None,
            observe_error: None,
            ingested: IngestStats::default(),
        })
    }

    /// Which session's requests the snapshots carry; None for none.
    pub fn show_detail(&mut self, key: Option<Key>) {
        if key != self.detail_key {
            self.detail = None;
            self.detail_error = None;
        }
        self.detail_key = key;
    }

    /// Agents left out of the snapshots from the next tick on. Their data is
    /// still collected and their processes still cleaned up.
    pub fn set_hidden(&mut self, hidden: BTreeSet<Provider>) {
        self.hidden = hidden;
    }

    /// Applies one command from the window. After End or Keep the next tick
    /// sweeps, so the view shows the change at once.
    pub fn command(&mut self, c: Command, host: &mut dyn Host) {
        let done = match c {
            Command::Detail(key) => return self.show_detail(key),
            Command::Refresh => return,
            Command::End(p) => {
                let fresh;
                let table = match &self.table {
                    Some((_, t)) => t,
                    None => {
                        fresh = Sampler::new().sample();
                        &fresh
                    }
                };
                self.tracker.end(&mut self.store, p, table, now_ms(), host)
            }
            Command::Keep(p) => self.tracker.keep(&mut self.store, p),
        };
        self.last_sweep = None;
        self.command_error = done.err().map(|e| {
            tracing::error!(error = %format!("{e:#}"), "clean-up command failed");
            rust_i18n::t!("common.cleanup_error", error = format!("{e:#}")).to_string()
        });
    }

    /// A tick without clean-up: nothing is tracked, swept, signalled or notified.
    pub fn tick(&mut self) -> Snapshot {
        self.tick_with(None)
    }

    /// With `cleanup`, records the agents' children after each process sample
    /// and sweeps every [`SWEEP_INTERVAL`].
    pub fn tick_with(&mut self, mut cleanup: Option<(&Cleanup, &mut dyn Host)>) -> Snapshot {
        let now = now_ms();
        let midnight = local_midnight_ms(now);
        let mut changed = false;

        for src in &mut self.sources {
            let provider = src.adapter.provider();
            match src.adapter.poll(&self.offsets, midnight.min(now - DAY_MS)) {
                Ok(batch) => {
                    src.poll_error = None;
                    src.more = batch.more;
                    if let Some(l) = batch.rate_limits.as_ref().filter(|l| {
                        self.limits
                            .get(&provider)
                            .is_none_or(|c| l.ts_ms >= c.ts_ms)
                    }) {
                        self.limits.insert(provider, l.clone());
                    }
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
                                    self.meters
                                        .entry(provider)
                                        .or_insert_with(|| ActivityMeter::new(RATE_WINDOW_MS))
                                        .record(e.ts_ms, added);
                                    self.session_meters
                                        .entry((provider, e.session_id.clone()))
                                        .or_insert_with(|| ActivityMeter::new(RATE_WINDOW_MS))
                                        .record(e.ts_ms, added);
                                }
                            }
                            Err(e) => {
                                tracing::error!(error = %format!("{e:#}"), "cannot store poll batch");
                                src.apply_error = Some(
                                    rust_i18n::t!("common.storage_error", error = format!("{e:#}"))
                                        .to_string(),
                                );
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
                    src.live_error = Some(
                        rust_i18n::t!(
                            "common.live_error",
                            provider = provider.label(),
                            error = format!("{e:#}")
                        )
                        .to_string(),
                    );
                    Vec::new()
                }
            };
        }

        let mut sampled = false;
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
            sampled = true;
        }
        let table = self.table.as_ref().map(|(_, t)| t);

        let verified: Vec<(Provider, &LiveSession)> = self
            .sources
            .iter()
            .flat_map(|s| s.live.iter().map(|l| (s.adapter.provider(), l)))
            .filter(|(_, l)| table.is_some_and(|t| verify(t, l).is_some()))
            .collect();
        if let (Some(t), true, Some((_, host))) = (table, sampled, cleanup.as_mut()) {
            let roots: Vec<(ProcessIdentity, Provider, String)> = verified
                .iter()
                .filter(|(_, l)| !l.shared_process)
                .filter_map(|(p, l)| Some((verify(t, l)?, *p, l.session_id.clone())))
                .collect();
            self.observe_error = self
                .tracker
                .observe(&mut self.store, t, &roots, &**host)
                .err()
                .map(|e| {
                    tracing::error!(error = %format!("{e:#}"), "cannot track agent processes");
                    rust_i18n::t!("common.cleanup_error", error = format!("{e:#}")).to_string()
                });
        }
        let live_ids: BTreeSet<Key> = verified
            .iter()
            .map(|(p, l)| (*p, l.session_id.clone()))
            .collect();
        let cutoff = verified
            .iter()
            .filter_map(|(_, l)| l.started_at_ms)
            .fold(now - DAY_MS, i64::min);

        self.metered.retain(|_, m| m.0 > now - RATE_WINDOW_MS);

        let indexing = self.sources.iter().any(|s| s.more);
        let stale = cache_stale(self.cache.as_ref(), changed, &live_ids, midnight);
        if stale {
            let rows: anyhow::Result<Vec<Vec<SessionRow>>> = self
                .sources
                .iter()
                .map(|s| self.store.sessions_since(s.adapter.provider(), cutoff))
                .collect();
            let totals = self.store.totals_by_provider(midnight);
            match rows
                .and_then(|r| Ok((r, totals?)))
                .and_then(|(r, t)| Ok((r, t, self.query_today(midnight)?)))
            {
                Ok((rows, totals, today)) => {
                    self.today = Some(today);
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
                    self.query_error = Some(
                        rust_i18n::t!("common.storage_error", error = format!("{e:#}")).to_string(),
                    );
                }
            }
        }
        // History read while backfilling would be partial: wait for the end.
        if !indexing
            && history_due(
                self.history.as_ref(),
                self.history_error.as_ref().map(|e| e.0),
                midnight,
            )
        {
            match self.query_history(now, midnight) {
                Ok(h) => {
                    self.history = Some(h);
                    self.history_error = None;
                }
                Err(e) => {
                    tracing::error!(error = %format!("{e:#}"), "cannot query usage history");
                    self.history_error = Some((
                        Instant::now(),
                        rust_i18n::t!("common.storage_error", error = format!("{e:#}")).to_string(),
                    ));
                }
            }
        }
        if let Some(key) = &self.detail_key
            && (stale || self.detail.is_none())
        {
            match self.store.session_requests(key.0, &key.1) {
                Ok(requests) => {
                    let mut long_context: Vec<u64> = requests
                        .iter()
                        .filter_map(|r| self.prices.long_context_threshold(&r.model))
                        .collect();
                    long_context.sort_unstable();
                    long_context.dedup();
                    self.detail = Some(Arc::new(SessionDetail {
                        key: key.clone(),
                        requests,
                        long_context,
                    }));
                    self.detail_error = None;
                }
                Err(e) => {
                    tracing::error!(error = %format!("{e:#}"), "cannot query session requests");
                    self.detail_error = Some(
                        rust_i18n::t!("common.storage_error", error = format!("{e:#}")).to_string(),
                    );
                }
            }
        }

        let rates: HashMap<Key, f32> = self
            .session_meters
            .iter_mut()
            .map(|(k, m)| (k.clone(), m.rate_per_sec(now) as f32))
            .collect();
        // Summed here, so showing or hiding an agent needs no new query.
        let mut totals = Totals::default();
        for (_, t) in self
            .cache
            .iter()
            .flat_map(|c| &c.totals)
            .filter(|(p, _)| !self.hidden.contains(p))
        {
            totals += t;
        }
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
        let mut snap = build_snapshot(&groups, &totals, table, &rates, now);

        self.session_meters.retain(|(p, id), _| {
            snap.sessions
                .iter()
                .any(|s| s.provider == *p && &s.session_id == id)
        });
        let mut rate = 0.0;
        for (p, m) in &mut self.meters {
            // Read even when hidden, so its old samples age out.
            let r = m.rate_per_sec(now);
            if !self.hidden.contains(p) {
                rate += r;
            }
        }
        snap.output_rate = rate as f32;
        snap.system_gpu_percent = gpu_utilization().map_or(Reading::Unavailable, Reading::Measured);
        // The clock went back: older samples would land in the future.
        if self.rate_history.back().is_some_and(|&(at, _)| at > now) {
            self.rate_history.clear();
        }
        if self.rate_history.len() == HISTORY_LEN {
            self.rate_history.pop_front();
        }
        self.rate_history.push_back((now, snap.output_rate));
        snap.rate_history = self.rate_history.iter().copied().collect();
        snap.indexing = indexing;
        snap.usage = self.usage_charts(now);
        snap.limits = self
            .limits
            .iter()
            .filter(|(p, _)| !self.hidden.contains(p))
            .map(|(p, l)| (*p, l.clone()))
            .collect();
        snap.detail = self.detail.clone();
        if let Some((cfg, host)) = cleanup {
            if self
                .last_sweep
                .is_none_or(|t| t.elapsed() >= SWEEP_INTERVAL)
            {
                // A sweep forced by a command keeps that command's error.
                let forced = self.last_sweep.is_none();
                if !forced {
                    self.command_error = None;
                }
                self.last_sweep = Some(Instant::now());
                // Without a live agent there is no recent sample to reuse. A forced
                // sweep needs one taken after the command, not up to 2 s before it.
                let fresh;
                let t = match table {
                    Some(t) if sampled || !forced => t,
                    _ => {
                        fresh = Sampler::new().sample();
                        &fresh
                    }
                };
                match self
                    .tracker
                    .sweep(&mut self.store, t, &snap.sessions, cfg, now, host)
                {
                    Ok(v) => {
                        // None: a failed sample; the last view stays.
                        if let Some(v) = v {
                            self.cleanup = v;
                        }
                        self.sweep_error = None;
                    }
                    Err(e) => {
                        tracing::error!(error = %format!("{e:#}"), "clean-up sweep failed");
                        self.sweep_error = Some(
                            rust_i18n::t!("common.cleanup_error", error = format!("{e:#}"))
                                .to_string(),
                        );
                    }
                }
            }
            snap.cleanup = self.cleanup.clone();
        }
        // Only now: the per-session meters, the clean-up sweep and the pace see every agent.
        self.live = snap.sessions.iter().any(|s| s.status.is_live());
        snap.sessions.retain(|s| !self.hidden.contains(&s.provider));
        (snap.agents_cpu_percent, snap.agents_rss_bytes) = resources(&snap.sessions);
        snap.errors = self
            .config_errors
            .iter()
            .cloned()
            .chain(
                self.sources
                    .iter()
                    .filter(|s| !self.hidden.contains(&s.adapter.provider()))
                    .flat_map(|s| {
                        [&s.poll_error, &s.live_error, &s.apply_error]
                            .into_iter()
                            .flatten()
                            .cloned()
                    }),
            )
            .chain(self.query_error.clone())
            .chain(self.history_error.as_ref().map(|e| e.1.clone()))
            .chain(self.detail_error.clone())
            .collect();
        // One store failure often fails several clean-up steps alike: list it once.
        for e in [&self.sweep_error, &self.command_error, &self.observe_error]
            .into_iter()
            .flatten()
        {
            if !snap.errors.contains(e) {
                snap.errors.push(e.clone());
            }
        }
        snap
    }

    fn query_today(&self, midnight: i64) -> anyhow::Result<Today> {
        let mut t = Today {
            midnight,
            days: Vec::new(),
            cache_reads: self.store.cache_reads_by_model(midnight, i64::MAX)?,
            lanes: Vec::new(),
        };
        for s in &self.sources {
            let p = s.adapter.provider();
            t.days.push(self.store.daily_usage(p, midnight, i64::MAX)?);
            t.lanes.extend(
                self.store
                    .lanes_since(p, midnight)?
                    .into_iter()
                    .map(|l| (p, l)),
            );
        }
        t.lanes.sort_by_key(|(_, l)| l.runs[0].0);
        Ok(t)
    }

    fn query_history(&self, now: i64, midnight: i64) -> anyhow::Result<History> {
        let from = local_days(now, USAGE_DAYS)[0].1;
        Ok(History {
            at: Instant::now(),
            midnight,
            days: self
                .sources
                .iter()
                .map(|s| {
                    let p = s.adapter.provider();
                    Ok((
                        self.store.daily_usage(p, from, midnight)?,
                        self.store.first_event_ms(p)?,
                    ))
                })
                .collect::<anyhow::Result<_>>()?,
            cache_reads: self.store.cache_reads_by_model(from, midnight)?,
        })
    }

    /// Past days and today joined; None until both have been queried. A day
    /// neither query covers (history not yet re-read after midnight) is no
    /// data, never zero.
    fn usage_charts(&self, now: i64) -> Option<UsageCharts> {
        let (h, t) = (self.history.as_ref()?, self.today.as_ref()?);
        let calendar = local_days(now, USAGE_DAYS);
        let covered = |start: i64, end: i64| end <= h.midnight || start >= t.midnight;
        let days = calendar
            .iter()
            .map(|(key, start, end)| Day {
                start_ms: *start,
                agents: self
                    .sources
                    .iter()
                    .zip(&h.days)
                    .zip(&t.days)
                    .filter(|((s, _), _)| !self.hidden.contains(&s.adapter.provider()))
                    .map(|((s, (past, first)), today)| {
                        // The cached first event may predate today's first rows.
                        let has_data = first.is_some_and(|f| f < *end)
                            || today
                                .first()
                                .is_some_and(|d| d.day.as_str() <= key.as_str());
                        let usage = (has_data && covered(*start, *end)).then(|| {
                            past.iter()
                                .chain(today)
                                .find(|d| &d.day == key)
                                .cloned()
                                .unwrap_or_else(|| DayUsage {
                                    day: key.clone(),
                                    ..DayUsage::default()
                                })
                        });
                        (s.adapter.provider(), usage)
                    })
                    .collect(),
            })
            .collect();
        let (saved, unpriced) = cache_saving(
            h.cache_reads
                .iter()
                .chain(&t.cache_reads)
                .filter(|m| !self.hidden.contains(&m.provider)),
            &self.prices,
        );
        Some(UsageCharts {
            days,
            // History must end where today starts, or a day is missing or counted twice.
            cache_saved_usd: (h.midnight == t.midnight).then_some(saved),
            cache_saved_unpriced: unpriced,
            today: t
                .lanes
                .iter()
                .filter(|(p, _)| !self.hidden.contains(p))
                .cloned()
                .collect(),
        })
    }
}

/// Estimated net USD caching saved, and the cache-using requests left out
/// because their model or tier has no price.
fn cache_saving<'a>(
    rows: impl IntoIterator<Item = &'a ModelCacheReads>,
    prices: &PriceBook,
) -> (f64, u64) {
    rows.into_iter().fold((0.0, 0), |(usd, skipped), m| {
        let skipped = skipped + m.unpriced_events;
        match prices.net_cache_saving(&m.model, m.cache_read, m.cache_write_5m, m.cache_write_1h) {
            Some(saved) => (usd + saved, skipped),
            None => (usd, skipped + m.events),
        }
    })
}

/// Whether past days must be queried again: none yet, a new local day, or
/// [`HISTORY_INTERVAL`] old; never sooner than [`HISTORY_RETRY`] after a failure.
fn history_due(h: Option<&History>, failed_at: Option<Instant>, midnight: i64) -> bool {
    failed_at.is_none_or(|t| t.elapsed() >= HISTORY_RETRY)
        && h.is_none_or(|h| h.midnight != midnight || h.at.elapsed() >= HISTORY_INTERVAL)
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

    let (agents_cpu_percent, agents_rss_bytes) = resources(&sessions);
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
        agents_cpu_percent,
        agents_rss_bytes,
        system_gpu_percent: Reading::Unavailable,
        indexing: false,
        errors: Vec::new(),
        usage: None,
        limits: BTreeMap::new(),
        detail: None,
        cleanup: CleanupView::default(),
        sessions,
    }
}

/// The sessions' summed CPU and memory; None without a measured process, and
/// CPU also while not yet valid.
fn resources(sessions: &[AgentSession]) -> (Option<f32>, Option<u64>) {
    let measured: Vec<_> = sessions
        .iter()
        .filter_map(|s| s.resources.value())
        .collect();
    if measured.is_empty() {
        return (None, None);
    }
    (
        measured.iter().map(|u| u.cpu_percent).sum(),
        Some(measured.iter().map(|u| u.rss_bytes).sum()),
    )
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
    local_date(now_ms, &Local)
        .and_then(|d| day_start(d, &Local))
        .unwrap_or(now_ms - DAY_MS)
}

fn local_date<Tz: TimeZone>(ms: i64, tz: &Tz) -> Option<NaiveDate> {
    DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(tz).date_naive())
}

/// The first instant of local day `d`: midnight, or where a DST change skips
/// midnight, the first minute after the gap.
fn day_start<Tz: TimeZone>(d: NaiveDate, tz: &Tz) -> Option<i64> {
    let midnight = d.and_hms_opt(0, 0, 0)?;
    (0..24 * 60).find_map(|m| {
        (midnight + chrono::Duration::minutes(m))
            .and_local_timezone(tz.clone())
            .earliest()
            .map(|t| t.timestamp_millis())
    })
}

/// The last `n` local days, oldest first, today last: date as SQLite's
/// `date(…, 'localtime')` writes it, and the day's start and end in ms.
/// Calendar days, so a day is 23 or 25 hours long across a DST change.
pub fn local_days(now_ms: i64, n: usize) -> Vec<(String, i64, i64)> {
    days_in(now_ms, n, &Local)
}

fn days_in<Tz: TimeZone>(now_ms: i64, n: usize, tz: &Tz) -> Vec<(String, i64, i64)> {
    let Some(today) = local_date(now_ms, tz) else {
        return Vec::new();
    };
    (0..n as u64)
        .rev()
        .filter_map(|back| {
            let d = today.checked_sub_days(chrono::Days::new(back))?;
            let start = day_start(d, tz)?;
            let end = day_start(d.succ_opt()?, tz)?;
            Some((d.format("%Y-%m-%d").to_string(), start, end))
        })
        .collect()
}

/// Runs the collector on its own thread until the receiver is dropped,
/// publishing each snapshot to the widgets when `widgets` is set, and the same
/// widget payload to paired phones through `phones`. Clean-up and the hidden
/// agents follow `settings` as they are at each tick. If the loop
/// panics, the widgets are marked stopped and the thread ends, which closes
/// the channel.
/// `host` is what the sweep observes and acts through; `cleanup`, when given,
/// replaces the stored clean-up settings.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    mut collector: Collector,
    sender: async_channel::Sender<Snapshot>,
    commands: Receiver<Command>,
    settings: Settings,
    mut widgets: Option<WidgetExporter>,
    phones: Option<PhoneLink>,
    mut host: Box<dyn Host + Send>,
    cleanup: Option<Cleanup>,
) -> std::io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("agentpulse-collector".into())
        .spawn(move || {
            // The collector is dropped right after a panic, so its state is never reused.
            let run = std::panic::catch_unwind(AssertUnwindSafe(|| {
                loop {
                    let cfg = cleanup.unwrap_or_else(|| settings.cleanup());
                    collector.set_hidden(settings.agents().hidden());
                    let snap = collector.tick_with(Some((&cfg, host.as_mut())));
                    let indexing = snap.indexing;
                    let live = collector.live;
                    if let Some(w) = &mut widgets {
                        w.publish(&snap);
                    }
                    if let Some(p) = phones.as_ref().filter(|p| p.wants_payload()) {
                        p.publish(widget_export::payload(&snap, false, &p.looks()));
                    }
                    if sender.send_blocking(snap).is_err() {
                        return;
                    }
                    let wait = if indexing {
                        Duration::ZERO
                    } else {
                        Duration::from_secs(if live { 1 } else { 5 })
                    };
                    // A command ends the wait at once.
                    match commands.recv_timeout(wait) {
                        Ok(first) => {
                            // Only the newest of several queued Detail requests matters;
                            // every End and Keep is applied.
                            let mut detail = None;
                            for c in std::iter::once(first).chain(commands.try_iter()) {
                                match c {
                                    Command::Detail(key) => detail = Some(key),
                                    c => collector.command(c, host.as_mut()),
                                }
                            }
                            if let Some(key) = detail {
                                collector.show_detail(key);
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => thread::sleep(wait),
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
    use crate::cleanup;

    const P: Provider = Provider::ClaudeCode;
    const NOW: i64 = 1_800_000_000_000;
    const START_S: u64 = 1_799_990_000;

    #[derive(Clone)]
    struct Fake {
        provider: Provider,
        polls: Arc<Mutex<VecDeque<anyhow::Result<PollBatch>>>>,
        live: Arc<Mutex<Vec<LiveSession>>>,
    }

    impl Fake {
        fn new(provider: Provider) -> Self {
            Self {
                provider,
                polls: Arc::default(),
                live: Arc::default(),
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
            Ok(self.live.lock().unwrap().clone())
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
            rate_limits: None,
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
            recorded_usd: None,
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
        let rates: Vec<f32> = s.rate_history.iter().map(|&(_, v)| v).collect();
        assert_eq!(rates, [0.0, 10.0]);
        assert!(s.rate_history.windows(2).all(|w| w[0].0 <= w[1].0));
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
            totals: BTreeMap::new(),
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
    fn a_hidden_agent_leaves_the_snapshot_and_comes_back() {
        let (claude, codex) = (Fake::new(Provider::ClaudeCode), Fake::new(Provider::Codex));
        let mut c = Collector::new(
            vec![Box::new(claude.clone()), Box::new(codex.clone())],
            Store::open_in_memory().unwrap(),
            PriceBook::builtin(),
            vec![],
        )
        .unwrap();
        let t = now_ms() - 1_000;
        let limits = |ts_ms| RateLimits {
            ts_ms,
            plan_type: None,
            primary: None,
            secondary: None,
        };
        let mut e = event("e1", "s1", "claude-sonnet-5-5", t, 30);
        e.usage.cache_read = 30;
        let mut b = batch(vec![e], vec![delta("s1", &[t])]);
        b.rate_limits = Some(limits(1));
        claude.polls.lock().unwrap().push_back(Ok(b));
        let mut e = event("e1", "s2", "claude-sonnet-5-5", t, 60);
        e.usage.cache_read = 50;
        let mut b = batch(vec![e], vec![delta("s2", &[t])]);
        b.provider = Provider::Codex;
        b.files[0].path = "/synthetic/codex.jsonl".into();
        b.rate_limits = Some(limits(2));
        codex.polls.lock().unwrap().push_back(Ok(b));
        let usage = |s: &Snapshot| s.usage.clone().expect("queried once not indexing");
        let agents = |s: &Snapshot| {
            let u = usage(s);
            let mut seen: BTreeSet<Provider> = u.today.iter().map(|(p, _)| *p).collect();
            seen.extend(u.days.iter().flat_map(|d| d.agents.iter().map(|a| a.0)));
            seen.extend(s.sessions.iter().map(|x| x.provider));
            seen.extend(s.limits.keys());
            seen
        };
        let saved = |s: &Snapshot| usage(s).cache_saved_usd.unwrap();

        let both = c.tick();
        let all = BTreeSet::from([Provider::ClaudeCode, Provider::Codex]);
        assert_eq!(agents(&both), all);
        assert_eq!((both.sessions.len(), both.today_tokens.output), (2, 90));
        assert_eq!(both.output_rate, 3.0);
        assert!((saved(&both) - 80.0 * 1.9 / 1e6).abs() < 1e-12);

        c.set_hidden(BTreeSet::from([Provider::Codex]));
        let s = c.tick();
        assert_eq!(agents(&s), BTreeSet::from([Provider::ClaudeCode]));
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(usage(&s).today.len(), 1);
        assert!(usage(&s).days.iter().all(|d| d.agents.len() == 1));
        assert_eq!(s.today_tokens, s.sessions[0].tokens);
        assert_eq!(s.today_cost_usd, s.sessions[0].cost_usd);
        assert_ne!(s.today_cost_usd, both.today_cost_usd);
        assert_eq!(s.output_rate, 1.0);
        assert!((saved(&s) - 30.0 * 1.9 / 1e6).abs() < 1e-12);
        assert_eq!(s.limits[&Provider::ClaudeCode].ts_ms, 1);

        // Shown again: everything is back on the next tick, without new data.
        c.set_hidden(BTreeSet::new());
        let again = c.tick();
        assert_eq!(agents(&again), all);
        assert_eq!(again.sessions.len(), 2);
        assert_eq!(usage(&again).today.len(), 2);
        assert!(usage(&again).days.iter().all(|d| d.agents.len() == 2));
        assert_eq!(again.today_tokens, both.today_tokens);
        assert_eq!(again.today_cost_usd, both.today_cost_usd);
        assert_eq!(again.output_rate, 3.0);
        assert_eq!(saved(&again), saved(&both));
        assert_eq!(again.limits[&Provider::Codex].ts_ms, 2);
    }

    #[test]
    fn a_hidden_agent_is_still_cleaned_up_and_keeps_the_pace() {
        let (claude, codex) = (Fake::new(Provider::ClaudeCode), Fake::new(Provider::Codex));
        let mut c = Collector::new(
            vec![Box::new(claude.clone()), Box::new(codex.clone())],
            Store::open_in_memory().unwrap(),
            PriceBook::builtin(),
            vec![],
        )
        .unwrap();
        c.set_hidden(BTreeSet::from([Provider::Codex]));
        // An idle Codex session, two hours silent, hosted by this test process.
        let me = std::process::id();
        let start = Sampler::new().sample().procs[&me].start_time_s;
        let last = now_ms() - 2 * 3_600_000;
        let mut b = batch(
            vec![event("e1", "idle", "gpt-6.1-sol", last, 5)],
            vec![delta("idle", &[last])],
        );
        b.provider = Provider::Codex;
        codex.polls.lock().unwrap().push_back(Ok(b));
        codex.live.lock().unwrap().push(LiveSession {
            proc_start_s: Some(start),
            ..live("idle", me, LiveState::Idle)
        });
        let cfg = Cleanup {
            warn_after_hours: 1,
            ..Cleanup::default()
        };
        let mut host = cleanup::tests::FakeHost::default();

        let s = c.tick_with(Some((&cfg, &mut host)));
        assert!(s.sessions.is_empty());
        assert!(c.live, "a live hidden agent keeps the 1 s pace");
        assert!(s.cleanup.items.iter().any(|i| i.reason
            == cleanup::Reason::IdleSession {
                provider: Provider::Codex,
                session_id: "idle".into(),
                waiting: false,
            }));

        codex
            .polls
            .lock()
            .unwrap()
            .push_back(Err(anyhow::anyhow!("synthetic codex failure")));
        let s = c.tick_with(Some((&cfg, &mut host)));
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert!(s.sessions.is_empty());
        assert!(c.live);
        // Shown again, its error is listed.
        c.set_hidden(BTreeSet::new());
        codex
            .polls
            .lock()
            .unwrap()
            .push_back(Err(anyhow::anyhow!("synthetic codex failure")));
        let s = c.tick_with(Some((&cfg, &mut host)));
        assert_eq!(s.sessions.len(), 1);
        assert!(s.errors[0].starts_with("Codex: synthetic codex failure"));
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
    fn local_days_are_contiguous_calendar_days_ending_today() {
        let now = now_ms();
        let days = local_days(now, USAGE_DAYS);
        assert_eq!(days.len(), USAGE_DAYS);
        let (today, start, end) = days.last().unwrap();
        assert!(*start <= now && now < *end);
        assert_eq!(*start, local_midnight_ms(now));
        assert_eq!(today, &Local::now().format("%Y-%m-%d").to_string());
        assert!(days.windows(2).all(|w| w[0].2 == w[1].1 && w[0].0 < w[1].0));
    }

    #[test]
    fn cache_saving_is_net_of_write_premium_and_skips_unpriced() {
        let rows = [
            ModelCacheReads {
                provider: P,
                model: "claude-sonnet-5-5".into(),
                cache_read: 1_000_000,
                cache_write_5m: 1_000_000,
                cache_write_1h: 0,
                events: 4,
                // Unknown tier: not in the sums, only counted.
                unpriced_events: 2,
            },
            ModelCacheReads {
                provider: P,
                model: "no-such-model".into(),
                cache_read: 5_000_000,
                cache_write_5m: 0,
                cache_write_1h: 0,
                events: 0,
                unpriced_events: 3,
            },
        ];
        let (usd, skipped) = cache_saving(&rows, &PriceBook::builtin());
        // Reads save 2.0 − 0.1 per M; 5 m writes cost 2.5 − 2.0 per M more than input.
        assert!((usd - (1.9 - 0.5)).abs() < 1e-9, "{usd}");
        assert_eq!(skipped, 5);
    }

    #[test]
    fn usage_days_tell_no_data_from_zero_and_keep_the_newest_limits() {
        let (claude, codex) = (Fake::new(Provider::ClaudeCode), Fake::new(Provider::Codex));
        let mut c = Collector::new(
            vec![Box::new(claude.clone()), Box::new(codex.clone())],
            Store::open_in_memory().unwrap(),
            PriceBook::builtin(),
            vec![],
        )
        .unwrap();
        let now = now_ms();
        let days = local_days(now, USAGE_DAYS);
        // Noon three days ago, and just after midnight today.
        let old = days[USAGE_DAYS - 4].1 + 12 * 3_600_000;
        let today = days[USAGE_DAYS - 1].1 + 1;
        let mut cached = event("e2", "s1", "claude-sonnet-5-5", today, 0);
        cached.usage.cache_read = 30;
        claude.polls.lock().unwrap().push_back(Ok(batch(
            vec![
                event("e1", "s1", "claude-sonnet-5-5", old, 1_000_000),
                cached,
                event("e3", "s2", "no-such-model", today, 5),
            ],
            vec![delta("s1", &[old, today]), delta("s2", &[today])],
        )));
        let limits = |ts_ms, used_percent| agentpulse_core::RateLimits {
            ts_ms,
            plan_type: Some("prolite".into()),
            primary: Some(agentpulse_core::RateWindow {
                used_percent,
                window_minutes: Some(10_080),
                resets_at_s: None,
            }),
            secondary: None,
        };
        let mut b = batch(vec![], vec![]);
        b.provider = Provider::Codex;
        b.files.clear();
        b.rate_limits = Some(limits(200, 62.0));
        codex.polls.lock().unwrap().push_back(Ok(b.clone()));
        let s = c.tick();
        let u = s.usage.as_ref().expect("queried once not indexing");
        assert_eq!(u.days.len(), USAGE_DAYS);
        let claude_day = |i: usize| u.days[i].agents[0].1.clone();
        assert_eq!(u.days[0].agents[0].0, Provider::ClaudeCode);
        assert_eq!(claude_day(USAGE_DAYS - 5), None, "before the first event");
        assert!((claude_day(USAGE_DAYS - 4).unwrap().cost_usd - 10.000_02).abs() < 1e-9);
        let gap = claude_day(USAGE_DAYS - 3).unwrap();
        assert_eq!(
            (gap.cost_usd, gap.priced_events),
            (0.0, 0),
            "a measured zero"
        );
        let t = claude_day(USAGE_DAYS - 1).unwrap();
        assert_eq!(
            (t.priced_events, t.unpriced_events, t.cache_read),
            (1, 1, 30)
        );
        // Codex stored nothing: no data on every day, never zero.
        assert!(u.days.iter().all(|d| d.agents[1].1.is_none()));
        assert!((u.cache_saved_usd.unwrap() - 30.0 * 1.9 / 1e6).abs() < 1e-12);
        assert_eq!(u.cache_saved_unpriced, 0);
        assert_eq!(u.today.len(), 2);
        assert_eq!(s.limits[&Provider::Codex].ts_ms, 200);
        assert!(!s.limits.contains_key(&Provider::ClaudeCode));

        // An older reading does not replace a newer one.
        b.rate_limits = Some(limits(100, 10.0));
        codex.polls.lock().unwrap().push_back(Ok(b));
        let s = c.tick();
        let p = s.limits[&Provider::Codex].primary.clone().unwrap();
        assert_eq!(p.used_percent, 62.0);

        // A session's requests arrive once asked for.
        c.show_detail(Some((Provider::ClaudeCode, "s1".into())));
        let d = c.tick().detail.unwrap();
        assert_eq!(d.requests.len(), 2);
        assert!(d.long_context.is_empty());
        c.show_detail(None);
        assert!(c.tick().detail.is_none());
    }

    /// A zone whose DST starts at midnight, as America/Santiago's does on
    /// 2026-09-06: 00:00 at −04:00 becomes 01:00 at −03:00.
    #[derive(Clone, Copy, Debug)]
    struct MidnightDst;

    impl MidnightDst {
        fn gap() -> chrono::NaiveDateTime {
            NaiveDate::from_ymd_opt(2026, 9, 6)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
        }
        fn offsets() -> (chrono::FixedOffset, chrono::FixedOffset) {
            let h = |n| chrono::FixedOffset::west_opt(n * 3600).unwrap();
            (h(4), h(3))
        }
    }

    impl TimeZone for MidnightDst {
        type Offset = chrono::FixedOffset;
        fn from_offset(_: &chrono::FixedOffset) -> Self {
            MidnightDst
        }
        fn offset_from_local_date(&self, d: &NaiveDate) -> chrono::MappedLocalTime<Self::Offset> {
            self.offset_from_local_datetime(&d.and_hms_opt(12, 0, 0).unwrap())
        }
        fn offset_from_local_datetime(
            &self,
            l: &chrono::NaiveDateTime,
        ) -> chrono::MappedLocalTime<Self::Offset> {
            let (winter, summer) = Self::offsets();
            if *l < Self::gap() {
                chrono::MappedLocalTime::Single(winter)
            } else if *l < Self::gap() + chrono::Duration::hours(1) {
                chrono::MappedLocalTime::None
            } else {
                chrono::MappedLocalTime::Single(summer)
            }
        }
        fn offset_from_utc_date(&self, d: &NaiveDate) -> Self::Offset {
            self.offset_from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
        }
        fn offset_from_utc_datetime(&self, u: &chrono::NaiveDateTime) -> Self::Offset {
            let (winter, summer) = Self::offsets();
            // The gap starts at 00:00 −04:00, 04:00 UTC.
            if *u < Self::gap() + chrono::Duration::hours(4) {
                winter
            } else {
                summer
            }
        }
    }

    #[test]
    fn a_day_whose_midnight_does_not_exist_starts_after_the_gap() {
        let switch = MidnightDst::gap().and_utc().timestamp_millis() + 4 * 3_600_000;
        let now = switch + 30 * 3_600_000;
        let days = days_in(now, 3, &MidnightDst);
        let keys: Vec<_> = days.iter().map(|d| d.0.as_str()).collect();
        assert_eq!(keys, ["2026-09-05", "2026-09-06", "2026-09-07"]);
        // 01:00 −03:00 is the same instant as the 00:00 −04:00 that never came.
        assert_eq!(days[1].1, switch);
        assert_eq!(days[1].2 - days[1].1, 23 * 3_600_000);
        assert!(days.windows(2).all(|w| w[0].2 == w[1].1));
        assert!(days[2].1 <= now && now < days[2].2);
    }

    #[test]
    fn a_day_history_has_not_covered_yet_is_no_data_not_zero() {
        let (mut c, fake) = collector();
        let days = local_days(now_ms(), USAGE_DAYS);
        let noon = |i: usize| days[i].1 + 12 * 3_600_000;
        let n = USAGE_DAYS;
        let mut cached = event("e2", "s1", "claude-sonnet-5-5", noon(n - 2), 0);
        cached.usage.cache_read = 1_000;
        fake.polls.lock().unwrap().push_back(Ok(batch(
            vec![
                event("e1", "s1", "claude-sonnet-5-5", noon(n - 4), 1_000),
                cached,
                event("e3", "s1", "claude-sonnet-5-5", days[n - 1].1 + 1, 1),
            ],
            vec![delta("s1", &[noon(n - 4), noon(n - 2), days[n - 1].1 + 1])],
        )));
        let u = c.tick().usage.unwrap();
        assert!(u.days[n - 2].agents[0].1.is_some());
        assert!(u.cache_saved_usd.is_some());

        // History as last read before midnight: it ends where yesterday starts.
        let h = c.history.as_mut().unwrap();
        h.midnight = days[n - 2].1;
        h.days[0].0.retain(|d| d.day < days[n - 2].0);
        h.cache_reads.clear();
        let u = c.usage_charts(now_ms()).unwrap();
        let claude = |i: usize| u.days[i].agents[0].1.clone();
        assert!(claude(n - 4).unwrap().cost_usd > 0.0);
        assert_eq!(claude(n - 3).unwrap().priced_events, 0, "a measured zero");
        assert_eq!(claude(n - 2), None, "not queried yet");
        assert_eq!(claude(n - 1).unwrap().priced_events, 1);
        assert_eq!(u.cache_saved_usd, None, "a day of cache use is missing");
    }

    #[test]
    fn first_ever_events_count_before_history_is_read_again() {
        let (mut c, fake) = collector();
        // History is read once, with nothing stored.
        assert!(
            c.tick()
                .usage
                .unwrap()
                .days
                .iter()
                .all(|d| d.agents[0].1.is_none())
        );
        let t = now_ms() - 1_000;
        fake.polls.lock().unwrap().push_back(Ok(batch(
            vec![event("e1", "s1", "claude-sonnet-5-5", t, 5)],
            vec![delta("s1", &[t])],
        )));
        let u = c.tick().usage.unwrap();
        assert_eq!(c.history.as_ref().unwrap().days[0].1, None, "not re-read");
        assert_eq!(
            u.days[USAGE_DAYS - 1].agents[0]
                .1
                .as_ref()
                .unwrap()
                .priced_events,
            1
        );
        assert_eq!(u.days[USAGE_DAYS - 2].agents[0].1, None);
    }

    #[test]
    fn history_retries_back_off_and_errors_stay_apart() {
        let h = History {
            at: Instant::now(),
            midnight: NOW,
            days: vec![],
            cache_reads: vec![],
        };
        assert!(!history_due(Some(&h), None, NOW));
        assert!(history_due(Some(&h), None, NOW + DAY_MS));
        assert!(history_due(None, None, NOW));
        assert!(!history_due(None, Some(Instant::now()), NOW), "backing off");
        let earlier = Instant::now().checked_sub(HISTORY_RETRY + Duration::from_secs(1));
        assert!(history_due(None, earlier, NOW));

        // A successful sessions query does not clear a history failure.
        let (mut c, _) = collector();
        c.tick();
        c.history_error = Some((Instant::now(), "Storage: synthetic history failure".into()));
        c.detail_error = Some("Storage: synthetic detail failure".into());
        let s = c.tick();
        assert!(s.errors.iter().any(|e| e.contains("history failure")));
        assert!(s.errors.iter().any(|e| e.contains("detail failure")));
        // Leaving the detail view drops its error.
        c.show_detail(Some((P, "s".into())));
        assert!(c.detail_error.is_none());
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

    #[test]
    fn sweep_runs_without_live_agents_and_fills_the_snapshot() {
        let (mut c, _fake) = collector();
        let mut host = cleanup::tests::FakeHost {
            memory: (Some(agentpulse_process::Pressure::Warn), None),
            ..Default::default()
        };
        let snap = c.tick_with(Some((&Cleanup::default(), &mut host)));
        // Only a sweep sets memory_low.
        assert!(snap.cleanup.memory_low);
        assert_ne!(snap.cleanup, CleanupView::default());
        assert!(
            snap.cleanup
                .items
                .iter()
                .all(|i| i.reason == cleanup::Reason::Untracked)
        );
        assert!(snap.errors.is_empty());
        // Without clean-up (as --dump runs), nothing is swept.
        assert_eq!(c.tick().cleanup, CleanupView::default());
    }

    #[test]
    fn end_command_shows_in_the_next_tick() {
        let (mut c, _fake) = collector();
        let mut host = cleanup::tests::FakeHost::default();
        let p = ProcessIdentity {
            pid: 999_999,
            start_time_s: 1,
        };
        let d = agentpulse_process::ProcDetails {
            name: "node".into(),
            uid: 0,
        };
        host.details.insert(p, d);
        host.alive.insert(p);
        // The first tick sweeps, so the next regular sweep is a minute away.
        let cfg = Cleanup::default();
        assert!(c.tick_with(Some((&cfg, &mut host))).cleanup.log.is_empty());
        c.command(Command::Keep(p), &mut host);
        c.command(Command::End(p), &mut host);
        assert_eq!(
            host.signals,
            vec![(999_999, agentpulse_process::Signal::Term)]
        );
        let snap = c.tick_with(Some((&cfg, &mut host)));
        assert_eq!(snap.cleanup.log.len(), 1);
        assert_eq!(snap.cleanup.log[0].name, "node");
        assert!(snap.errors.is_empty());
    }
}
