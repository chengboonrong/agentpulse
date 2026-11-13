//! SQLite store for session metadata and token usage. Never prompts, code or
//! secrets: adapters do not pass any.

use std::collections::HashMap;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;

use agentpulse_core::{
    ACTIVE_GAP_MS, PollBatch, Provider, RecordedCost, TokenUsage, UsageEvent, fold_active,
};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

const MIGRATIONS: &[&str] = &[
    "
CREATE TABLE sessions (
  provider TEXT NOT NULL, session_id TEXT NOT NULL,
  project_path TEXT, model TEXT,
  started_at INTEGER, last_activity_at INTEGER,
  active_ms INTEGER NOT NULL DEFAULT 0,
  last_error INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (provider, session_id)
);
CREATE TABLE usage_events (
  provider TEXT NOT NULL, event_id TEXT NOT NULL,
  session_id TEXT NOT NULL, model TEXT NOT NULL, ts INTEGER NOT NULL,
  input INTEGER NOT NULL, output INTEGER NOT NULL,
  cache_write_5m INTEGER NOT NULL, cache_write_1h INTEGER NOT NULL, cache_read INTEGER NOT NULL,
  cost_usd REAL,
  PRIMARY KEY (provider, event_id)
);
CREATE INDEX usage_events_session ON usage_events (provider, session_id);
CREATE INDEX usage_events_ts ON usage_events (ts);
CREATE TABLE ingest_files (path TEXT PRIMARY KEY, offset INTEGER NOT NULL);
",
    "
ALTER TABLE sessions ADD COLUMN recorded_cost_usd REAL;
ALTER TABLE sessions ADD COLUMN recorded_cost_as_of INTEGER;
ALTER TABLE sessions ADD COLUMN recorded_cost_unknown_models INTEGER;
-- Re-read transcripts so cost records already passed over are picked up.
-- Everything else in them upserts idempotently.
DELETE FROM ingest_files;
",
];

#[derive(Clone, Debug, PartialEq)]
pub struct SessionRow {
    pub session_id: String,
    pub project_path: Option<String>,
    pub model: Option<String>,
    pub started_at_ms: Option<i64>,
    pub last_activity_at_ms: Option<i64>,
    pub active_ms: u64,
    pub last_error: bool,
    pub tokens: TokenUsage,
    /// Sum over priced events.
    pub cost_usd: f64,
    pub priced_events: u64,
    pub unpriced_events: u64,
    /// The agent's own latest cost record for the session.
    pub recorded_cost: Option<RecordedCost>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Totals {
    pub tokens: TokenUsage,
    pub cost_usd: f64,
    pub priced_events: u64,
    pub unpriced_events: u64,
    pub sessions: u64,
}

pub struct Store {
    conn: Connection,
}

/// Applies each step the database has not seen yet, each in its own
/// transaction, tracked by `PRAGMA user_version`.
fn migrate(conn: &mut Connection, migrations: &[&str]) -> Result<()> {
    let version = conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as usize;
    if version > migrations.len() {
        bail!(
            "database schema version {version} is newer than this app understands ({})",
            migrations.len()
        );
    }
    for (i, sql) in migrations.iter().enumerate().skip(version) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
        tx.commit()?;
    }
    Ok(())
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
        let mut conn =
            Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        migrate(&mut conn, MIGRATIONS)?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        migrate(&mut conn, MIGRATIONS)?;
        Ok(Self { conn })
    }

    pub fn offsets(&self) -> Result<HashMap<String, u64>> {
        let mut stmt = self.conn.prepare("SELECT path, offset FROM ingest_files")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Applies a whole poll in one transaction, so offsets never run ahead of
    /// the data they cover. Replaying a batch changes nothing.
    pub fn apply(
        &mut self,
        batch: &PollBatch,
        price: &dyn Fn(&UsageEvent) -> Option<f64>,
    ) -> Result<()> {
        let provider = batch.provider.as_str();
        let tx = self.conn.transaction()?;

        for e in &batch.events {
            let u = &e.usage;
            // A re-read of the same response may carry fuller usage (streaming
            // writes it more than once): keep the larger one. session_id stays.
            tx.execute(
                "INSERT INTO usage_events (provider, event_id, session_id, model, ts,
                    input, output, cache_write_5m, cache_write_1h, cache_read, cost_usd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT (provider, event_id) DO UPDATE SET
                    model = excluded.model, ts = excluded.ts,
                    input = excluded.input, output = excluded.output,
                    cache_write_5m = excluded.cache_write_5m,
                    cache_write_1h = excluded.cache_write_1h,
                    cache_read = excluded.cache_read, cost_usd = excluded.cost_usd
                 WHERE excluded.input + excluded.output + excluded.cache_write_5m
                       + excluded.cache_write_1h + excluded.cache_read
                     > usage_events.input + usage_events.output + usage_events.cache_write_5m
                       + usage_events.cache_write_1h + usage_events.cache_read",
                params![
                    provider,
                    e.event_id,
                    e.session_id,
                    e.model,
                    e.ts_ms,
                    u.input as i64,
                    u.output as i64,
                    u.cache_write_5m as i64,
                    u.cache_write_1h as i64,
                    u.cache_read as i64,
                    price(e),
                ],
            )?;
        }

        for d in &batch.sessions {
            let stored = tx
                .query_row(
                    "SELECT project_path, model, started_at, last_activity_at, active_ms, last_error
                     FROM sessions WHERE provider = ?1 AND session_id = ?2",
                    params![provider, d.session_id],
                    |r| {
                        Ok((
                            r.get::<_, Option<String>>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, Option<i64>>(2)?,
                            r.get::<_, Option<i64>>(3)?,
                            r.get::<_, i64>(4)?,
                            r.get::<_, bool>(5)?,
                        ))
                    },
                )
                .optional()?
                .unwrap_or((None, None, None, None, 0, false));
            let (path, model, started, last, active, error) = stored;
            let (last, active) = fold_active(last, active as u64, &d.timestamps_ms, ACTIVE_GAP_MS);
            let started = [started, d.timestamps_ms.iter().copied().min()]
                .into_iter()
                .flatten()
                .min();
            // An upsert, not a replace, so the recorded cost columns survive.
            tx.execute(
                "INSERT INTO sessions (provider, session_id, project_path, model,
                    started_at, last_activity_at, active_ms, last_error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (provider, session_id) DO UPDATE SET
                    project_path = excluded.project_path, model = excluded.model,
                    started_at = excluded.started_at,
                    last_activity_at = excluded.last_activity_at,
                    active_ms = excluded.active_ms, last_error = excluded.last_error",
                params![
                    provider,
                    d.session_id,
                    path.or_else(|| d.project_path.clone()),
                    d.model.clone().or(model),
                    started,
                    last,
                    active as i64,
                    d.last_error.unwrap_or(error),
                ],
            )?;
            // Records are cumulative: the one current at the later time wins.
            if let Some(c) = &d.recorded_cost {
                tx.execute(
                    "UPDATE sessions SET recorded_cost_usd = ?3, recorded_cost_as_of = ?4,
                        recorded_cost_unknown_models = ?5
                     WHERE provider = ?1 AND session_id = ?2
                       AND (recorded_cost_as_of IS NULL OR recorded_cost_as_of < ?4)",
                    params![provider, d.session_id, c.usd, c.as_of_ms, c.unknown_models],
                )?;
            }
        }

        for f in &batch.files {
            tx.execute(
                "INSERT INTO ingest_files (path, offset) VALUES (?1, ?2)
                 ON CONFLICT (path) DO UPDATE SET offset = excluded.offset",
                params![f.path, f.offset as i64],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Sessions of `provider` active at or after `since_ms`, newest first.
    /// Totals cover all of each session's events, not only those after `since_ms`.
    pub fn sessions_since(&self, provider: Provider, since_ms: i64) -> Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.session_id, s.project_path, s.model, s.started_at, s.last_activity_at,
                    s.active_ms, s.last_error,
                    COALESCE(SUM(e.input), 0), COALESCE(SUM(e.output), 0),
                    COALESCE(SUM(e.cache_write_5m), 0), COALESCE(SUM(e.cache_write_1h), 0),
                    COALESCE(SUM(e.cache_read), 0),
                    COALESCE(SUM(e.cost_usd), 0.0), COUNT(e.cost_usd),
                    COUNT(e.event_id) - COUNT(e.cost_usd),
                    s.recorded_cost_usd, s.recorded_cost_as_of, s.recorded_cost_unknown_models
             FROM sessions s
             LEFT JOIN usage_events e
               ON e.provider = s.provider AND e.session_id = s.session_id
             WHERE s.provider = ?1 AND s.last_activity_at >= ?2
             GROUP BY s.session_id
             ORDER BY s.last_activity_at DESC",
        )?;
        let rows = stmt.query_map(params![provider.as_str(), since_ms], |r| {
            Ok(SessionRow {
                session_id: r.get(0)?,
                project_path: r.get(1)?,
                model: r.get(2)?,
                started_at_ms: r.get(3)?,
                last_activity_at_ms: r.get(4)?,
                active_ms: r.get::<_, i64>(5)? as u64,
                last_error: r.get(6)?,
                tokens: TokenUsage {
                    input: r.get::<_, i64>(7)? as u64,
                    output: r.get::<_, i64>(8)? as u64,
                    cache_write_5m: r.get::<_, i64>(9)? as u64,
                    cache_write_1h: r.get::<_, i64>(10)? as u64,
                    cache_read: r.get::<_, i64>(11)? as u64,
                },
                cost_usd: r.get(12)?,
                priced_events: r.get::<_, i64>(13)? as u64,
                unpriced_events: r.get::<_, i64>(14)? as u64,
                recorded_cost: match (r.get(15)?, r.get(16)?) {
                    (Some(usd), Some(as_of_ms)) => Some(RecordedCost {
                        usd,
                        as_of_ms,
                        unknown_models: r.get::<_, Option<bool>>(17)?.unwrap_or(false),
                    }),
                    _ => None,
                },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn totals_since(&self, since_ms: i64) -> Result<Totals> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(input), 0), COALESCE(SUM(output), 0),
                    COALESCE(SUM(cache_write_5m), 0), COALESCE(SUM(cache_write_1h), 0),
                    COALESCE(SUM(cache_read), 0),
                    COALESCE(SUM(cost_usd), 0.0), COUNT(cost_usd),
                    COUNT(*) - COUNT(cost_usd),
                    COUNT(DISTINCT provider || char(31) || session_id)
             FROM usage_events WHERE ts >= ?1",
            params![since_ms],
            |r| {
                Ok(Totals {
                    tokens: TokenUsage {
                        input: r.get::<_, i64>(0)? as u64,
                        output: r.get::<_, i64>(1)? as u64,
                        cache_write_5m: r.get::<_, i64>(2)? as u64,
                        cache_write_1h: r.get::<_, i64>(3)? as u64,
                        cache_read: r.get::<_, i64>(4)? as u64,
                    },
                    cost_usd: r.get(5)?,
                    priced_events: r.get::<_, i64>(6)? as u64,
                    unpriced_events: r.get::<_, i64>(7)? as u64,
                    sessions: r.get::<_, i64>(8)? as u64,
                })
            },
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentpulse_core::{FileCursor, SessionDelta};

    const P: Provider = Provider::ClaudeCode;

    fn tokens(input: u64, output: u64) -> TokenUsage {
        TokenUsage {
            input,
            output,
            ..Default::default()
        }
    }

    fn ev(id: &str, session: &str, ts: i64, usage: TokenUsage) -> UsageEvent {
        UsageEvent {
            event_id: id.into(),
            session_id: session.into(),
            model: "m".into(),
            ts_ms: ts,
            usage,
            fast: false,
            us_inference: false,
            unknown_tier: false,
        }
    }

    fn delta(session: &str, ts: &[i64]) -> SessionDelta {
        SessionDelta {
            session_id: session.into(),
            timestamps_ms: ts.to_vec(),
            ..Default::default()
        }
    }

    fn batch(events: Vec<UsageEvent>, sessions: Vec<SessionDelta>) -> PollBatch {
        PollBatch {
            provider: P,
            files: vec![FileCursor {
                path: "/a.jsonl".into(),
                offset: 42,
            }],
            events,
            sessions,
            more: false,
        }
    }

    fn flat(_: &UsageEvent) -> Option<f64> {
        Some(1.0)
    }

    #[test]
    fn applying_a_batch_twice_equals_once() {
        let b = batch(
            vec![
                ev("e1", "s", 100, tokens(10, 5)),
                ev("e2", "s", 200, tokens(1, 1)),
            ],
            vec![delta("s", &[100, 200])],
        );
        let mut once = Store::open_in_memory().unwrap();
        once.apply(&b, &flat).unwrap();
        let mut twice = Store::open_in_memory().unwrap();
        twice.apply(&b, &flat).unwrap();
        twice.apply(&b, &flat).unwrap();
        assert_eq!(
            once.sessions_since(P, 0).unwrap(),
            twice.sessions_since(P, 0).unwrap()
        );
        assert_eq!(
            once.totals_since(0).unwrap(),
            twice.totals_since(0).unwrap()
        );
        assert_eq!(once.offsets().unwrap(), twice.offsets().unwrap());
        assert_eq!(twice.totals_since(0).unwrap().tokens, tokens(11, 6));
    }

    #[test]
    fn larger_usage_wins_in_either_order() {
        let small = batch(vec![ev("e", "s", 100, tokens(1, 1))], vec![]);
        let mut big_ev = ev("e", "s", 150, tokens(10, 20));
        big_ev.model = "m2".into();
        let big = batch(vec![big_ev], vec![]);
        for order in [[&small, &big], [&big, &small]] {
            let mut s = Store::open_in_memory().unwrap();
            for b in order {
                s.apply(b, &flat).unwrap();
            }
            let t = s.totals_since(0).unwrap();
            assert_eq!(t.tokens, tokens(10, 20));
            assert_eq!(t.priced_events, 1);
            let model: String = s
                .conn
                .query_row("SELECT model FROM usage_events", [], |r| r.get(0))
                .unwrap();
            assert_eq!(model, "m2");
        }
    }

    #[test]
    fn event_seen_in_two_sessions_stays_with_the_first() {
        let mut s = Store::open_in_memory().unwrap();
        s.apply(
            &batch(
                vec![ev("e", "a", 100, tokens(5, 5))],
                vec![delta("a", &[100]), delta("b", &[100])],
            ),
            &flat,
        )
        .unwrap();
        s.apply(
            &batch(vec![ev("e", "b", 100, tokens(50, 50))], vec![]),
            &flat,
        )
        .unwrap();
        let rows = s.sessions_since(P, 0).unwrap();
        let a = rows.iter().find(|r| r.session_id == "a").unwrap();
        let b = rows.iter().find(|r| r.session_id == "b").unwrap();
        assert_eq!(a.tokens, tokens(50, 50));
        assert_eq!(b.tokens, TokenUsage::default());
        assert_eq!(s.totals_since(0).unwrap().tokens, tokens(50, 50));
        assert_eq!(s.totals_since(0).unwrap().sessions, 1);
    }

    #[test]
    fn active_time_folds_across_batches() {
        let mut s = Store::open_in_memory().unwrap();
        s.apply(
            &batch(
                vec![],
                vec![delta("s", &[0, 10_000, 30_000, 330_000, 335_000])],
            ),
            &flat,
        )
        .unwrap();
        let r = &s.sessions_since(P, 0).unwrap()[0];
        assert_eq!(
            (r.active_ms, r.started_at_ms, r.last_activity_at_ms),
            (35_000, Some(0), Some(335_000))
        );
        s.apply(&batch(vec![], vec![delta("s", &[340_000])]), &flat)
            .unwrap();
        let r = &s.sessions_since(P, 0).unwrap()[0];
        assert_eq!(
            (r.active_ms, r.started_at_ms, r.last_activity_at_ms),
            (40_000, Some(0), Some(340_000))
        );
    }

    #[test]
    fn session_fields_update_only_when_present() {
        let mut s = Store::open_in_memory().unwrap();
        let mut d = delta("s", &[10]);
        d.project_path = Some("/p1".into());
        d.model = Some("m1".into());
        d.last_error = Some(true);
        s.apply(&batch(vec![], vec![d]), &flat).unwrap();
        let mut d = delta("s", &[20]);
        d.project_path = Some("/p2".into());
        d.model = Some("m2".into());
        s.apply(&batch(vec![], vec![d]), &flat).unwrap();
        let r = &s.sessions_since(P, 0).unwrap()[0];
        assert_eq!(r.project_path.as_deref(), Some("/p1"));
        assert_eq!(r.model.as_deref(), Some("m2"));
        assert!(r.last_error);
        let mut d = delta("s", &[30]);
        d.last_error = Some(false);
        s.apply(&batch(vec![], vec![d]), &flat).unwrap();
        assert!(!s.sessions_since(P, 0).unwrap()[0].last_error);
        // An earlier first timestamp lowers started_at.
        s.apply(&batch(vec![], vec![delta("s", &[5])]), &flat)
            .unwrap();
        assert_eq!(s.sessions_since(P, 0).unwrap()[0].started_at_ms, Some(5));
    }

    #[test]
    fn unpriced_events_are_counted_and_cost_nothing() {
        let mut s = Store::open_in_memory().unwrap();
        let price = |e: &UsageEvent| (e.event_id != "u").then_some(2.5);
        s.apply(
            &batch(
                vec![
                    ev("p1", "s", 1, tokens(1, 0)),
                    ev("p2", "s", 2, tokens(1, 0)),
                    ev("u", "s", 3, tokens(1, 0)),
                ],
                vec![delta("s", &[1, 2, 3])],
            ),
            &price,
        )
        .unwrap();
        let r = &s.sessions_since(P, 0).unwrap()[0];
        assert_eq!(
            (r.cost_usd, r.priced_events, r.unpriced_events),
            (5.0, 2, 1)
        );
        let t = s.totals_since(0).unwrap();
        assert_eq!(
            (t.cost_usd, t.priced_events, t.unpriced_events),
            (5.0, 2, 1)
        );
        s.apply(
            &batch(vec![ev("none", "s2", 4, tokens(1, 0))], vec![]),
            &|_| None,
        )
        .unwrap();
        assert_eq!(s.totals_since(0).unwrap().unpriced_events, 2);
    }

    #[test]
    fn totals_boundary_is_inclusive() {
        let mut s = Store::open_in_memory().unwrap();
        s.apply(
            &batch(
                vec![
                    ev("a", "s", 99, tokens(1, 0)),
                    ev("b", "s", 100, tokens(2, 0)),
                ],
                vec![],
            ),
            &flat,
        )
        .unwrap();
        assert_eq!(s.totals_since(100).unwrap().tokens, tokens(2, 0));
        assert_eq!(s.totals_since(101).unwrap().tokens, TokenUsage::default());
        assert_eq!(s.totals_since(101).unwrap().sessions, 0);
    }

    #[test]
    fn sessions_since_keeps_empty_sessions_and_filters_provider() {
        let mut s = Store::open_in_memory().unwrap();
        s.apply(&batch(vec![], vec![delta("empty", &[500])]), &flat)
            .unwrap();
        let mut other = batch(vec![], vec![delta("codex-s", &[600])]);
        other.provider = Provider::Codex;
        s.apply(&other, &flat).unwrap();
        s.apply(&batch(vec![], vec![delta("old", &[10])]), &flat)
            .unwrap();
        let rows = s.sessions_since(P, 100).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "empty");
        assert_eq!(rows[0].tokens, TokenUsage::default());
        assert_eq!(
            (
                rows[0].cost_usd,
                rows[0].priced_events,
                rows[0].unpriced_events
            ),
            (0.0, 0, 0)
        );
        assert_eq!(
            s.sessions_since(Provider::Codex, 100).unwrap()[0].session_id,
            "codex-s"
        );
    }

    #[test]
    fn same_ids_under_two_providers_never_collide() {
        let mut s = Store::open_in_memory().unwrap();
        let mut d = delta("same", &[100]);
        d.model = Some("claude-m".into());
        s.apply(
            &batch(vec![ev("same-event", "same", 100, tokens(1, 2))], vec![d]),
            &flat,
        )
        .unwrap();
        let mut d = delta("same", &[200]);
        d.model = Some("codex-m".into());
        let mut codex = batch(vec![ev("same-event", "same", 200, tokens(10, 20))], vec![d]);
        codex.provider = Provider::Codex;
        codex.files[0].path = "/codex/rollout.jsonl".into();
        s.apply(&codex, &flat).unwrap();
        let claude = &s.sessions_since(P, 0).unwrap()[0];
        let codex = &s.sessions_since(Provider::Codex, 0).unwrap()[0];
        assert_eq!(
            (claude.tokens, claude.model.as_deref()),
            (tokens(1, 2), Some("claude-m"))
        );
        assert_eq!(
            (codex.tokens, codex.model.as_deref()),
            (tokens(10, 20), Some("codex-m"))
        );
        let t = s.totals_since(0).unwrap();
        assert_eq!((t.tokens, t.sessions), (tokens(11, 22), 2));
    }

    #[test]
    fn sessions_are_ordered_newest_first() {
        let mut s = Store::open_in_memory().unwrap();
        s.apply(
            &batch(
                vec![],
                vec![delta("a", &[100]), delta("b", &[300]), delta("c", &[200])],
            ),
            &flat,
        )
        .unwrap();
        let ids: Vec<_> = s
            .sessions_since(P, 0)
            .unwrap()
            .into_iter()
            .map(|r| r.session_id)
            .collect();
        assert_eq!(ids, ["b", "c", "a"]);
    }

    #[test]
    fn offsets_round_trip() {
        let mut s = Store::open_in_memory().unwrap();
        let mut b = batch(vec![], vec![]);
        b.files = vec![
            FileCursor {
                path: "/a".into(),
                offset: 1,
            },
            FileCursor {
                path: "/b".into(),
                offset: 2,
            },
        ];
        s.apply(&b, &flat).unwrap();
        b.files = vec![FileCursor {
            path: "/a".into(),
            offset: 9,
        }];
        s.apply(&b, &flat).unwrap();
        let o = s.offsets().unwrap();
        assert_eq!(
            o,
            HashMap::from([("/a".to_string(), 9), ("/b".to_string(), 2)])
        );
    }

    #[test]
    fn concurrent_sessions_keep_their_own_totals() {
        let mut s = Store::open_in_memory().unwrap();
        s.apply(
            &batch(
                vec![
                    ev("1", "a", 1, tokens(1, 0)),
                    ev("2", "b", 2, tokens(2, 0)),
                    ev("3", "c", 3, tokens(3, 0)),
                    ev("4", "c", 4, tokens(4, 0)),
                ],
                vec![delta("a", &[1]), delta("b", &[2]), delta("c", &[3, 4])],
            ),
            &flat,
        )
        .unwrap();
        let rows = s.sessions_since(P, 0).unwrap();
        assert_eq!(rows.len(), 3);
        let by = |id: &str| {
            rows.iter()
                .find(|r| r.session_id == id)
                .unwrap()
                .tokens
                .input
        };
        assert_eq!((by("a"), by("b"), by("c")), (1, 2, 7));
        assert_eq!(s.totals_since(0).unwrap().sessions, 3);
    }

    #[test]
    fn migrations_upgrade_stepwise_and_refuse_newer_databases() {
        let step1 = "CREATE TABLE t (a INTEGER);";
        let step2 = "ALTER TABLE t ADD COLUMN b INTEGER NOT NULL DEFAULT 7;";
        let version = |c: &Connection| {
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap()
        };

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, &[step1]).unwrap();
        conn.execute("INSERT INTO t (a) VALUES (1)", []).unwrap();
        assert_eq!(version(&conn), 1);

        migrate(&mut conn, &[step1, step2]).unwrap();
        assert_eq!(version(&conn), 2);
        let row: (i64, i64) = conn
            .query_row("SELECT a, b FROM t", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(row, (1, 7));

        migrate(&mut conn, &[step1, step2]).unwrap();
        assert_eq!(version(&conn), 2);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM t", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );

        assert!(migrate(&mut conn, &[step1]).is_err());
    }

    fn cost(usd: f64, as_of_ms: i64, unknown_models: bool) -> RecordedCost {
        RecordedCost {
            usd,
            as_of_ms,
            unknown_models,
        }
    }

    fn costed(session: &str, ts: &[i64], c: RecordedCost) -> SessionDelta {
        SessionDelta {
            recorded_cost: Some(c),
            ..delta(session, ts)
        }
    }

    #[test]
    fn recorded_cost_keeps_the_latest_in_either_order_and_survives_updates() {
        let early = batch(vec![], vec![costed("s", &[10], cost(1.5, 1_000, true))]);
        let late = batch(vec![], vec![costed("s", &[20], cost(2.25, 2_000, false))]);
        for order in [[&early, &late], [&late, &early]] {
            let mut s = Store::open_in_memory().unwrap();
            for b in order {
                s.apply(b, &flat).unwrap();
                s.apply(b, &flat).unwrap();
            }
            let r = &s.sessions_since(P, 0).unwrap()[0];
            assert_eq!(r.recorded_cost, Some(cost(2.25, 2_000, false)));
            // Later activity without a cost record leaves it in place.
            s.apply(&batch(vec![], vec![delta("s", &[30])]), &flat)
                .unwrap();
            let r = &s.sessions_since(P, 0).unwrap()[0];
            assert_eq!(r.recorded_cost, Some(cost(2.25, 2_000, false)));
            assert_eq!(r.last_activity_at_ms, Some(30));
        }
        let mut s = Store::open_in_memory().unwrap();
        s.apply(&batch(vec![], vec![delta("none", &[1])]), &flat)
            .unwrap();
        assert_eq!(s.sessions_since(P, 0).unwrap()[0].recorded_cost, None);
    }

    #[test]
    fn recorded_cost_migration_keeps_data_and_rereads_files() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, &MIGRATIONS[..1]).unwrap();
        let mut old = Store { conn };
        old.apply(
            &batch(vec![ev("e", "s", 1, tokens(3, 4))], vec![delta("s", &[1])]),
            &flat,
        )
        .unwrap();
        assert_eq!(old.offsets().unwrap().len(), 1);

        migrate(&mut old.conn, MIGRATIONS).unwrap();
        assert_eq!(
            old.conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(old.offsets().unwrap().is_empty(), "transcripts are re-read");
        let r = &old.sessions_since(P, 0).unwrap()[0];
        assert_eq!((r.tokens, r.recorded_cost), (tokens(3, 4), None));
        // The re-read converges on the same rows, now with the cost record.
        old.apply(
            &batch(
                vec![ev("e", "s", 1, tokens(3, 4))],
                vec![costed("s", &[1], cost(0.5, 9, false))],
            ),
            &flat,
        )
        .unwrap();
        let r = &old.sessions_since(P, 0).unwrap()[0];
        assert_eq!(r.tokens, tokens(3, 4));
        assert_eq!(r.active_ms, 0);
        assert_eq!(r.recorded_cost, Some(cost(0.5, 9, false)));
        assert_eq!(old.totals_since(0).unwrap().priced_events, 1);
    }

    #[test]
    fn open_creates_private_file_and_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");
        let file = dir.join("agentpulse.db");
        let mut s = Store::open(&file).unwrap();
        s.apply(
            &batch(vec![ev("e", "s", 1, tokens(1, 1))], vec![delta("s", &[1])]),
            &flat,
        )
        .unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&dir), 0o700);
        drop(s);
        // Reopening an existing database works and keeps the data.
        assert_eq!(
            Store::open(&file).unwrap().totals_since(0).unwrap().tokens,
            tokens(1, 1)
        );
    }
}
