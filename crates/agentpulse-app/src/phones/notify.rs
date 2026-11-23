//! Held snapshot statuses, with no I/O.

use std::collections::HashMap;

use agentpulse_core::{AgentSession, Provider, SessionStatus};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Waiting,
    Finished,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct Notice {
    pub kind: Kind,
    pub agent: String,
    pub project: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Choices {
    pub waiting: bool,
    pub finished: bool,
    pub failed: bool,
}

impl Choices {
    pub fn allows(&self, kind: Kind) -> bool {
        match kind {
            Kind::Waiting => self.waiting,
            Kind::Finished => self.finished,
            Kind::Failed => self.failed,
        }
    }
}

pub const HOLD_MS: i64 = 10_000;

struct Stretch {
    status: SessionStatus,
    since: i64,
    /// When the hold started: `since`, or the first snapshot after a gap (the Mac slept).
    hold_from: i64,
    before: Option<SessionStatus>,
    fired: bool,
    before_fired: bool,
}

fn finished(status: SessionStatus) -> bool {
    matches!(status, SessionStatus::Idle | SessionStatus::Completed)
}

fn same(a: SessionStatus, b: SessionStatus) -> bool {
    a == b || (finished(a) && finished(b))
}

#[derive(Default)]
pub struct Detector {
    stretches: HashMap<(Provider, String), Stretch>,
    last_snapshot_ms: Option<i64>,
}

impl Detector {
    pub fn events(&mut self, sessions: &[AgentSession], now_ms: i64) -> Vec<Notice> {
        if self
            .last_snapshot_ms
            .is_some_and(|last| now_ms - last > 30_000)
        {
            for stretch in self.stretches.values_mut() {
                stretch.hold_from = now_ms;
            }
        }
        self.last_snapshot_ms = Some(now_ms);
        let mut next = HashMap::with_capacity(sessions.len());
        let mut events = Vec::new();
        for s in sessions {
            let key = (s.provider, s.session_id.clone());
            let mut stretch = match self.stretches.remove(&key) {
                Some(mut old) if same(old.status, s.status) => {
                    old.status = s.status;
                    old
                }
                Some(old) => {
                    let back = old.before.is_some_and(|b| same(b, s.status))
                        && now_ms - old.since < HOLD_MS;
                    Stretch {
                        status: s.status,
                        since: now_ms,
                        hold_from: now_ms,
                        before: Some(old.status),
                        fired: back && old.before_fired,
                        before_fired: old.fired,
                    }
                }
                None => Stretch {
                    status: s.status,
                    since: now_ms,
                    hold_from: now_ms,
                    before: None,
                    fired: false,
                    before_fired: false,
                },
            };
            let kind = match stretch.status {
                SessionStatus::Waiting if stretch.before.is_some() => Some(Kind::Waiting),
                status if finished(status) && stretch.before == Some(SessionStatus::Active) => {
                    Some(Kind::Finished)
                }
                SessionStatus::Failed if stretch.before.is_some_and(SessionStatus::is_live) => {
                    Some(Kind::Failed)
                }
                _ => None,
            };
            if !stretch.fired
                && now_ms - stretch.hold_from >= HOLD_MS
                && let Some(kind) = kind
            {
                stretch.fired = true;
                events.push(Notice {
                    kind,
                    agent: s.provider.label().into(),
                    project: crate::format::project_name(s.project_path.as_deref()).into(),
                });
            }
            next.insert(key, stretch);
        }
        self.stretches = next;
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentpulse_core::{AgentSession, Provider, Reading, SessionStatus, TokenUsage};

    fn session(id: &str, status: SessionStatus) -> AgentSession {
        AgentSession {
            provider: Provider::ClaudeCode,
            session_id: id.into(),
            project_path: Some("/private/work/project".into()),
            model: None,
            status,
            started_at_ms: None,
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
        }
    }

    #[test]
    fn holds_each_kind_and_fires_once() {
        use SessionStatus::*;
        for (before, after, kind) in [
            (Active, Waiting, Kind::Waiting),
            (Active, Idle, Kind::Finished),
            (Active, Completed, Kind::Finished),
            (Active, Failed, Kind::Failed),
            (Idle, Failed, Kind::Failed),
            (Waiting, Failed, Kind::Failed),
        ] {
            let mut d = Detector::default();
            assert!(d.events(&[session("a", before)], 0).is_empty());
            assert!(d.events(&[session("a", after)], 100).is_empty());
            assert!(d.events(&[session("a", after)], 10_099).is_empty());
            let events = d.events(&[session("a", after)], 10_100);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].kind, kind);
            assert_eq!(events[0].agent, "Claude Code");
            assert_eq!(events[0].project, "project");
            assert!(d.events(&[session("a", after)], 30_000).is_empty());
        }
    }

    #[test]
    fn fired_stretches_survive_short_blips() {
        use SessionStatus::*;
        for (status, blip, returned) in [
            (Waiting, Idle, Waiting),
            (Waiting, Active, Waiting),
            (Idle, Active, Completed),
        ] {
            let mut d = Detector::default();
            d.events(&[session("a", Active)], 0);
            d.events(&[session("a", status)], 1_000);
            assert_eq!(d.events(&[session("a", status)], 11_000).len(), 1);
            d.events(&[session("a", status)], 30_000);
            d.events(&[session("a", status)], 59_000);
            assert!(d.events(&[session("a", blip)], 60_000).is_empty());
            assert!(d.events(&[session("a", returned)], 61_000).is_empty());
            assert!(d.events(&[session("a", returned)], 81_000).is_empty());
        }
    }

    #[test]
    fn pending_finished_restarts_after_a_blip() {
        use SessionStatus::*;
        let mut d = Detector::default();
        d.events(&[session("a", Active)], 0);
        d.events(&[session("a", Idle)], 1_000);
        d.events(&[session("a", Active)], 6_000);
        d.events(&[session("a", Idle)], 7_000);
        assert!(d.events(&[session("a", Idle)], 16_999).is_empty());
        assert_eq!(
            d.events(&[session("a", Idle)], 17_000)[0].kind,
            Kind::Finished
        );
        assert!(d.events(&[session("a", Idle)], 27_000).is_empty());
    }

    #[test]
    fn sleep_restarts_the_hold_without_rearming_fired_stretches() {
        use SessionStatus::*;
        let mut d = Detector::default();
        d.events(&[session("a", Active)], 0);
        d.events(&[session("a", Waiting)], 1_000);
        let wake = 7_201_000;
        assert!(d.events(&[session("a", Waiting)], wake).is_empty());
        assert!(d.events(&[session("a", Waiting)], wake + 9_999).is_empty());
        assert_eq!(d.events(&[session("a", Waiting)], wake + 10_000).len(), 1);
        assert!(d.events(&[session("a", Waiting)], wake * 2).is_empty());
        assert!(
            d.events(&[session("a", Waiting)], wake * 2 + 10_000)
                .is_empty()
        );
    }

    #[test]
    fn blips_cancel_pending_notices() {
        use SessionStatus::*;
        for status in [Waiting, Idle, Completed, Failed] {
            let mut d = Detector::default();
            d.events(&[session("a", Active)], 0);
            assert!(d.events(&[session("a", status)], 100).is_empty());
            assert!(d.events(&[session("a", Active)], 200).is_empty());
            assert!(d.events(&[session("a", Active)], 20_000).is_empty());
        }
    }

    #[test]
    fn first_seen_and_returning_sessions_never_fire() {
        use SessionStatus::*;
        let mut d = Detector::default();
        for status in [Waiting, Idle, Completed, Failed] {
            assert!(d.events(&[session("a", status)], 0).is_empty());
            assert!(d.events(&[session("a", status)], 20_000).is_empty());
            assert!(d.events(&[], 21_000).is_empty());
            assert!(d.events(&[session("a", status)], 22_000).is_empty());
            assert!(d.events(&[session("a", status)], 40_000).is_empty());
            d.events(&[], 41_000);
        }
        d.events(&[session("known", Active)], 50_000);
        assert!(
            d.events(&[session("known", Active), session("new", Waiting)], 51_000)
                .is_empty()
        );
        assert!(d.events(&[session("new", Waiting)], 70_000).is_empty());
    }

    #[test]
    fn idle_and_completed_share_a_stretch_but_waiting_is_not_finished() {
        use SessionStatus::*;
        let mut d = Detector::default();
        d.events(&[session("a", Active)], 0);
        assert!(d.events(&[session("a", Idle)], 100).is_empty());
        assert!(d.events(&[session("a", Completed)], 5_000).is_empty());
        assert_eq!(
            d.events(&[session("a", Completed)], 10_100)[0].kind,
            Kind::Finished
        );
        assert!(d.events(&[session("a", Idle)], 20_000).is_empty());
        d.events(&[session("a", Waiting)], 21_000);
        assert!(d.events(&[session("a", Idle)], 22_000).is_empty());
        assert!(d.events(&[session("a", Completed)], 40_000).is_empty());
    }

    #[test]
    fn provider_is_part_of_the_identity() {
        let mut d = Detector::default();
        d.events(&[session("same", SessionStatus::Active)], 0);
        let mut b = session("same", SessionStatus::Completed);
        b.provider = Provider::Codex;
        assert!(d.events(&[b.clone()], 100).is_empty());
        assert!(d.events(&[b], 20_000).is_empty());
    }

    #[test]
    fn a_long_turn_across_a_gap_is_not_a_blip() {
        use SessionStatus::*;
        let mut d = Detector::default();
        d.events(&[session("a", Active)], 0);
        d.events(&[session("a", Idle)], 1_000);
        assert_eq!(d.events(&[session("a", Idle)], 11_000).len(), 1);
        d.events(&[session("a", Active)], 20_000);
        d.events(&[session("a", Active)], 400_000);
        // The Mac slept; the turn ended meanwhile.
        assert!(d.events(&[session("a", Idle)], 460_000).is_empty());
        let events = d.events(&[session("a", Idle)], 470_000);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, Kind::Finished);
    }
}
