//! Domain types and pure rules shared by every AgentPulse crate.
//!
//! No I/O and no UI here. Adapters produce [`PollBatch`] and [`LiveSession`],
//! storage folds batches into durable totals, and the app turns both into
//! [`AgentSession`] values for display.
//!
//! All timestamps are Unix milliseconds unless a name ends in `_s`.

use std::collections::{HashMap, VecDeque};
use std::ops::AddAssign;

use serde::{Deserialize, Serialize};

/// A gap between two transcript records longer than this is idle time, not
/// execution time. Used for [`fold_active`].
pub const ACTIVE_GAP_MS: i64 = 60_000;
/// Without a live-state signal, a session whose transcript changed this
/// recently is treated as active.
pub const TRANSCRIPT_ACTIVE_MS: i64 = 30_000;
/// Without a verified process, a session silent for longer than this has ended.
pub const IDLE_TIMEOUT_MS: i64 = 300_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Provider {
    ClaudeCode,
    Codex,
    OpenCode,
}

impl Provider {
    /// Stable key used in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::ClaudeCode => "claude-code",
            Provider::Codex => "codex",
            Provider::OpenCode => "opencode",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::ClaudeCode => "Claude Code",
            Provider::Codex => "Codex",
            Provider::OpenCode => "OpenCode",
        }
    }
}

/// A value together with how it is known. `Unavailable` is displayed as such
/// and never as zero.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reading<T> {
    /// Read directly from the system or from the agent's own records.
    Measured(T),
    /// Derived by AgentPulse from a rule or a price table.
    Estimated(T),
    Unavailable,
}

impl<T> Reading<T> {
    pub fn value(&self) -> Option<&T> {
        match self {
            Reading::Measured(v) | Reading::Estimated(v) => Some(v),
            Reading::Unavailable => None,
        }
    }
}

/// Token counts in disjoint buckets: `input` excludes cached tokens, so the
/// five fields sum to the total without double counting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_write_5m + self.cache_write_1h + self.cache_read
    }

    /// Tokens sent as the prompt of one request (everything except output).
    pub fn prompt(&self) -> u64 {
        self.total() - self.output
    }
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, o: Self) {
        self.input += o.input;
        self.output += o.output;
        self.cache_write_5m += o.cache_write_5m;
        self.cache_write_1h += o.cache_write_1h;
        self.cache_read += o.cache_read;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionStatus {
    /// The agent is executing a turn.
    Active,
    /// The agent's process is alive but it is not working.
    Idle,
    /// The agent is blocked on the user (permission prompt, question).
    Waiting,
    /// The session ended normally.
    Completed,
    /// The session ended and its last response was an API error.
    Failed,
}

impl SessionStatus {
    pub fn is_live(self) -> bool {
        matches!(self, Self::Active | Self::Idle | Self::Waiting)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Idle => "Idle",
            Self::Waiting => "Waiting",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }
}

/// A PID alone is not an identity: PIDs are reused. The start time pins it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Process start, Unix seconds, as reported by the OS.
    pub start_time_s: u64,
}

/// Resources of one agent's process tree (the agent plus its descendants).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceUsage {
    /// Percent of one core, summed over the tree; can exceed 100.
    /// `None` until two samples exist to take a delta from.
    pub cpu_percent: Option<f32>,
    pub rss_bytes: u64,
    pub process_count: u32,
}

/// One agent session as shown to the user.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSession {
    pub provider: Provider,
    pub session_id: String,
    pub project_path: Option<String>,
    /// Model of the most recent main-conversation response.
    pub model: Option<String>,
    pub status: SessionStatus,
    pub started_at_ms: Option<i64>,
    pub last_activity_at_ms: Option<i64>,
    /// Set once the session is no longer live.
    pub ended_at_ms: Option<i64>,
    /// Estimated execution time: see [`fold_active`].
    pub active_ms: u64,
    pub tokens: TokenUsage,
    pub cost_usd: Reading<f64>,
    /// Responses whose model has no price; their cost is missing from `cost_usd`.
    pub unpriced_events: u64,
    /// Output tokens per second over the recent window; see [`ActivityMeter`].
    pub output_rate: f32,
    /// `None` for sessions with no verified local process.
    pub process: Option<ProcessIdentity>,
    pub resources: Reading<ResourceUsage>,
    /// The agent's own cost figure for the session, covering the run that last
    /// wrote it; Unavailable until the agent has written one.
    pub recorded_cost: Reading<RecordedCost>,
}

/// A session's cost as the agent itself recorded it (Claude Code's
/// `cost-state`), cumulative over the session's life.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedCost {
    pub usd: f64,
    /// When the figure was current: start time plus total duration.
    pub as_of_ms: i64,
    /// The agent had no price for some model, so `usd` is short by that much.
    pub unknown_models: bool,
}

impl AgentSession {
    /// Wall-clock runtime: start to end, or start to `now_ms` while live.
    pub fn wall_ms(&self, now_ms: i64) -> Option<u64> {
        let start = self.started_at_ms?;
        let end = self.ended_at_ms.unwrap_or(now_ms);
        Some(end.saturating_sub(start).max(0) as u64)
    }
}

// ---------------------------------------------------------------- ingestion

/// Token usage of one API response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageEvent {
    /// Unique per provider and stable across re-reads, so ingestion is idempotent.
    pub event_id: String,
    pub session_id: String,
    pub model: String,
    pub ts_ms: i64,
    pub usage: TokenUsage,
    /// Billed at the fast-mode rate.
    pub fast: bool,
    /// Billed at the US-only inference rate.
    pub us_inference: bool,
    /// Billed at a service tier the price table does not cover; left unpriced.
    pub unknown_tier: bool,
}

/// What one poll learned about a session, besides token usage.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionDelta {
    pub session_id: String,
    pub project_path: Option<String>,
    pub model: Option<String>,
    /// Timestamps of every record seen in this poll, ascending.
    pub timestamps_ms: Vec<i64>,
    /// Whether the newest main-conversation response seen was an API error.
    /// `None` if this poll saw no such response.
    pub last_error: Option<bool>,
    /// The latest (largest `as_of_ms`) cost record seen in this poll.
    pub recorded_cost: Option<RecordedCost>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileCursor {
    pub path: String,
    /// Byte offset just past the last complete line consumed.
    pub offset: u64,
}

/// Everything an adapter read in one poll. Applied atomically by storage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PollBatch {
    pub provider: Provider,
    pub files: Vec<FileCursor>,
    pub events: Vec<UsageEvent>,
    pub sessions: Vec<SessionDelta>,
    /// The adapter stopped at its read budget; poll again for the rest.
    pub more: bool,
    /// The newest plan-limit reading the adapter knows of (Codex only).
    pub rate_limits: Option<RateLimits>,
}

/// One usage window of an agent's subscription plan, as the agent reported it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateWindow {
    pub used_percent: f64,
    pub window_minutes: Option<u64>,
    /// Unix seconds.
    pub resets_at_s: Option<i64>,
}

/// A plan-limit reading (Codex `token_count` events). Metadata only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateLimits {
    /// Time of the record that carried it.
    pub ts_ms: i64,
    pub plan_type: Option<String>,
    pub primary: Option<RateWindow>,
    pub secondary: Option<RateWindow>,
}

impl PollBatch {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.events.is_empty() && self.sessions.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveState {
    Busy,
    Idle,
    Waiting,
    /// The agent reported a state AgentPulse does not know.
    Unknown,
}

/// A session the agent itself says is running in a local process.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiveSession {
    pub session_id: String,
    pub pid: u32,
    /// Process start the agent recorded, Unix seconds, if it recorded one.
    pub proc_start_s: Option<u64>,
    pub started_at_ms: Option<i64>,
    pub state: LiveState,
    pub project_path: Option<String>,
    /// The process also hosts other sessions, so its resources are not this
    /// session's alone: the session is live but shows no process or resources.
    pub shared_process: bool,
}

/// One agent product (Claude Code, Codex, ...).
///
/// Adapters are polled from a background thread, so methods may block on file
/// I/O. Errors are recoverable: the caller logs them, shows the adapter as
/// degraded and polls again later. Malformed input inside a readable file is
/// skipped, not returned as an error.
pub trait AgentAdapter: Send {
    fn provider(&self) -> Provider;

    /// Read what was appended since `offsets` (path → byte offset). Files with
    /// no offset are read from the start if modified at or after `since_ms`
    /// and ignored otherwise.
    fn poll(&mut self, offsets: &HashMap<String, u64>, since_ms: i64) -> anyhow::Result<PollBatch>;

    /// Sessions the agent reports as running. Empty for agents that keep no
    /// such record; their sessions then have no process and no live state.
    fn live_sessions(&self) -> anyhow::Result<Vec<LiveSession>>;
}

// -------------------------------------------------------------------- rules

/// Fold record timestamps into estimated execution time.
///
/// Time between two consecutive records counts as active when the gap is at
/// most `gap_ms`. Timestamps at or before `last_ms` are ignored, which makes
/// replaying the same records a no-op.
pub fn fold_active(
    mut last_ms: Option<i64>,
    mut active_ms: u64,
    timestamps_ms: &[i64],
    gap_ms: i64,
) -> (Option<i64>, u64) {
    for &ts in timestamps_ms {
        match last_ms {
            Some(last) if ts <= last => continue,
            Some(last) if ts - last <= gap_ms => active_ms += (ts - last) as u64,
            _ => {}
        }
        last_ms = Some(ts);
    }
    (last_ms, active_ms)
}

/// Decide a session's status from what was observed.
///
/// `live` must be `Some` only when the session's process was verified alive
/// (PID and start time both match). A live process alone never means
/// `Active`: that needs the agent's own busy state, or, when the agent reports
/// no state AgentPulse knows (`Unknown`), transcript activity within
/// [`TRANSCRIPT_ACTIVE_MS`]. A live process is never Completed or Failed.
pub fn resolve_status(
    live: Option<LiveState>,
    last_activity_ms: Option<i64>,
    last_error: bool,
    now_ms: i64,
) -> SessionStatus {
    match live {
        Some(LiveState::Busy) => SessionStatus::Active,
        Some(LiveState::Waiting) => SessionStatus::Waiting,
        Some(LiveState::Idle) => SessionStatus::Idle,
        Some(LiveState::Unknown) => match last_activity_ms {
            Some(t) if now_ms - t <= TRANSCRIPT_ACTIVE_MS => SessionStatus::Active,
            _ => SessionStatus::Idle,
        },
        None => {
            let silent = last_activity_ms.map(|t| now_ms - t);
            match silent {
                Some(s) if s <= TRANSCRIPT_ACTIVE_MS => SessionStatus::Active,
                Some(s) if s <= IDLE_TIMEOUT_MS => SessionStatus::Idle,
                _ if last_error => SessionStatus::Failed,
                _ => SessionStatus::Completed,
            }
        }
    }
}

/// Rolling token rate. Fed with per-response token counts at the time the
/// response was recorded; reports tokens per second over the window.
///
/// Agents log a response when it completes, so tokens arrive in bursts. The
/// window turns those bursts into a rate that is meaningful to display and
/// safe to drive animation with. Cumulative totals are never used as a rate.
#[derive(Clone, Debug)]
pub struct ActivityMeter {
    window_ms: i64,
    samples: VecDeque<(i64, u64)>,
}

impl ActivityMeter {
    pub fn new(window_ms: i64) -> Self {
        Self {
            window_ms,
            samples: VecDeque::new(),
        }
    }

    pub fn record(&mut self, ts_ms: i64, tokens: u64) {
        self.samples.push_back((ts_ms, tokens));
    }

    pub fn rate_per_sec(&mut self, now_ms: i64) -> f64 {
        let cutoff = now_ms - self.window_ms;
        self.samples.retain(|&(ts, _)| ts > cutoff && ts <= now_ms);
        let tokens: u64 = self.samples.iter().map(|&(_, n)| n).sum();
        tokens as f64 / (self.window_ms as f64 / 1000.0)
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_time_counts_short_gaps_only() {
        // 10s + 20s active, then a 5 min pause, then 5s.
        let ts = [0, 10_000, 30_000, 330_000, 335_000];
        assert_eq!(
            fold_active(None, 0, &ts, ACTIVE_GAP_MS),
            (Some(335_000), 35_000)
        );
    }

    #[test]
    fn active_time_is_idempotent_on_replay() {
        let ts = [0, 10_000, 30_000];
        let (last, active) = fold_active(None, 0, &ts, ACTIVE_GAP_MS);
        assert_eq!(
            fold_active(last, active, &ts, ACTIVE_GAP_MS),
            (last, active)
        );
        // Continuing later only adds the new part.
        assert_eq!(
            fold_active(last, active, &[30_000, 40_000], ACTIVE_GAP_MS),
            (Some(40_000), 40_000)
        );
    }

    #[test]
    fn alive_process_is_not_active_unless_busy() {
        let now = 1_000_000;
        assert_eq!(
            resolve_status(Some(LiveState::Idle), Some(now), false, now),
            SessionStatus::Idle
        );
        assert_eq!(
            resolve_status(Some(LiveState::Unknown), Some(now - 5_000), false, now),
            SessionStatus::Active
        );
        assert_eq!(
            resolve_status(Some(LiveState::Unknown), Some(now - 600_000), true, now),
            SessionStatus::Idle
        );
        assert_eq!(
            resolve_status(Some(LiveState::Unknown), None, false, now),
            SessionStatus::Idle
        );
        assert_eq!(
            resolve_status(Some(LiveState::Busy), None, false, now),
            SessionStatus::Active
        );
        assert_eq!(
            resolve_status(Some(LiveState::Waiting), None, true, now),
            SessionStatus::Waiting
        );
    }

    #[test]
    fn status_without_process_follows_transcript_age() {
        let now = 10_000_000;
        assert_eq!(
            resolve_status(None, Some(now - 5_000), false, now),
            SessionStatus::Active
        );
        assert_eq!(
            resolve_status(None, Some(now - 120_000), false, now),
            SessionStatus::Idle
        );
        assert_eq!(
            resolve_status(None, Some(now - 600_000), false, now),
            SessionStatus::Completed
        );
        assert_eq!(
            resolve_status(None, Some(now - 600_000), true, now),
            SessionStatus::Failed
        );
        assert_eq!(
            resolve_status(None, None, false, now),
            SessionStatus::Completed
        );
    }

    #[test]
    fn rate_uses_only_the_window() {
        let mut m = ActivityMeter::new(30_000);
        m.record(1_000, 9_000); // falls out of the window
        m.record(40_000, 300);
        m.record(50_000, 600);
        assert_eq!(m.rate_per_sec(60_000), 30.0);
        assert_eq!(m.rate_per_sec(200_000), 0.0);
        assert!(m.is_empty());
    }

    #[test]
    fn token_buckets_sum_without_overlap() {
        let mut a = TokenUsage {
            input: 1,
            output: 2,
            cache_write_5m: 3,
            cache_write_1h: 4,
            cache_read: 5,
        };
        a += a;
        assert_eq!(a.total(), 30);
        assert_eq!(a.prompt(), 26);
    }

    #[test]
    fn wall_clock_runs_until_end_or_now() {
        let mut s = AgentSession {
            provider: Provider::ClaudeCode,
            session_id: "s".into(),
            project_path: None,
            model: None,
            status: SessionStatus::Active,
            started_at_ms: Some(1_000),
            last_activity_at_ms: None,
            ended_at_ms: None,
            active_ms: 0,
            tokens: TokenUsage::default(),
            cost_usd: Reading::Unavailable,
            unpriced_events: 0,
            output_rate: 0.0,
            process: None,
            resources: Reading::Unavailable,
            recorded_cost: Reading::Unavailable,
        };
        assert_eq!(s.wall_ms(5_000), Some(4_000));
        s.ended_at_ms = Some(3_000);
        assert_eq!(s.wall_ms(5_000), Some(2_000));
    }
}
