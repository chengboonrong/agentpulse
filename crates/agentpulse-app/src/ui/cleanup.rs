//! The Clean-up screen: processes left running after their agent, idle
//! sessions, and what was ended lately.

use agentpulse_core::ProcessIdentity;
use agentpulse_storage::CleanupLogEntry;

use super::settings::{button, card_title};
use super::*;
use crate::cleanup::{CleanupItem, Reason};

/// How long a clicked End waits for its "Confirm end".
pub(super) const CONFIRM: Duration = Duration::from_secs(5);

const GROUPS: [&str; 3] = ["Leftover processes", "Idle sessions", "Other leftovers"];

fn group(r: &Reason) -> usize {
    match r {
        Reason::Orphan { .. } => 0,
        Reason::IdleSession { .. } => 1,
        Reason::Untracked => 2,
    }
}

/// "40 min" under an hour, else whole hours: "5 h".
fn span(ms: i64) -> String {
    let m = (ms / 60_000).max(1);
    if m < 60 {
        format!("{m} min")
    } else {
        format!("{} h", m / 60)
    }
}

/// Why the row is listed. `project` names an orphan's session, when known.
pub(super) fn why(item: &CleanupItem, project: Option<&str>, now_ms: i64) -> String {
    let since = span(now_ms - item.since_ms);
    match &item.reason {
        Reason::Orphan {
            provider,
            session_id,
        } => {
            let source =
                project.map_or_else(|| session_id.chars().take(8).collect(), str::to_owned);
            format!("From {} · {source}", provider.label())
        }
        Reason::IdleSession { waiting: true, .. } => format!("Waiting on you for {since}"),
        Reason::IdleSession { .. } => format!("Idle for {since}"),
        Reason::Untracked => "Not started by an agent AgentPulse saw".into(),
    }
}

/// When an orphan will be ended, as the last sweep saw it; None for other rows.
fn ends(item: &CleanupItem, now_ms: i64) -> Option<String> {
    let Reason::Orphan { .. } = item.reason else {
        return None;
    };
    Some(match item.clean_at_ms {
        _ if item.kept => "Kept".into(),
        Some(at) => format!("Ends in {}", span(at - now_ms)),
        None => "Automatic end is off".into(),
    })
}

/// The project of the session an orphan came from, if the snapshot lists it.
fn project<'a>(s: &'a Snapshot, r: &Reason) -> Option<&'a str> {
    let Reason::Orphan {
        provider,
        session_id,
    } = r
    else {
        return None;
    };
    s.sessions
        .iter()
        .find(|x| x.provider == *provider && x.session_id == *session_id)
        .and_then(|x| x.project_path.as_deref())
        .map(|p| format::project_name(Some(p)))
}

fn send(c: Command, cx: &mut App) {
    cx.global::<Shared>().details.send(c).ok();
}

impl AppView {
    pub(super) fn cleanup_screen(
        &self,
        s: &Snapshot,
        notes: Vec<Div>,
        t: Theme,
        view: WeakEntity<Self>,
    ) -> impl IntoElement {
        let c = &s.cleanup;
        let armed = self.armed.map(|(p, _)| p);
        let mut page = charts::scroll_page("cleanup", &self.page_scroll)
            .child(heading("Clean-up"))
            .children(notes.into_iter().map(|n| n.mt(px(sp::M))))
            .when(c.memory_low, |d| {
                d.child(
                    div()
                        .mt(px(sp::M))
                        .px(px(sp::M))
                        .py(px(sp::S))
                        .rounded(px(10.))
                        .bg(rgb(t.banner))
                        .border_1()
                        .border_color(rgb(t.banner_border))
                        .text_size(px(ty::LABEL))
                        .child("Memory is low. Ending leftover processes can free some."),
                )
            });
        let mut gap = sp::XL;
        for (g, title) in GROUPS.into_iter().enumerate() {
            let rows: Vec<Div> = c
                .items
                .iter()
                .enumerate()
                .filter(|(_, i)| group(&i.reason) == g)
                .map(|(n, i)| {
                    let line = [Some(why(i, project(s, &i.reason), s.taken_at_ms))]
                        .into_iter()
                        .chain([ends(i, s.taken_at_ms)])
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    let keep = g == 0 && !i.kept;
                    item_row(n, i, &line, armed, keep, t, &view)
                })
                .collect();
            if rows.is_empty() {
                continue;
            }
            page = page.child(
                card(t)
                    .mt(px(gap))
                    .gap(px(sp::M))
                    .child(card_title(title))
                    .children(rows),
            );
            gap = sp::L;
        }
        if c.items.is_empty() {
            page = page.child(secondary("Nothing left behind.", t).mt(px(sp::XL)));
        }
        page.when(!c.log.is_empty(), |d| {
            d.child(
                card(t)
                    .mt(px(sp::L))
                    .gap(px(sp::M))
                    .child(card_title("Recently ended"))
                    .children(c.log.iter().map(|e| log_row(e, s.taken_at_ms, t))),
            )
        })
    }

    /// End's first click arms it for [`CONFIRM`]; a second click within that ends the process.
    fn end(&mut self, p: ProcessIdentity, cx: &mut Context<Self>) {
        if self
            .armed
            .is_some_and(|(a, at)| a == p && at.elapsed() < CONFIRM)
        {
            self.armed = None;
            send(Command::End(p), cx);
        } else {
            let armed = Some((p, Instant::now()));
            self.armed = armed;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(CONFIRM).await;
                this.update(cx, |this, cx| {
                    // Only this arming: a later click has its own timer.
                    if this.armed == armed {
                        this.armed = None;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }
}

/// A name over one muted line, then memory, then whatever the caller adds.
fn line_row(name: &str, line: &str, rss_bytes: u64, t: Theme) -> Div {
    div()
        .px(px(sp::S))
        .flex()
        .items_center()
        .gap(px(sp::M))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .truncate()
                        .child(name.to_string()),
                )
                .child(secondary(line, t)),
        )
        .child(if rss_bytes == 0 {
            div()
                .flex_none()
                .text_color(rgb(t.muted))
                .child(format::DASH)
        } else {
            div()
                .flex_none()
                .font_weight(FontWeight::MEDIUM)
                .child(format::bytes(rss_bytes))
        })
}

fn item_row(
    n: usize,
    i: &CleanupItem,
    line: &str,
    armed: Option<ProcessIdentity>,
    keep: bool,
    t: Theme,
    view: &WeakEntity<AppView>,
) -> Div {
    let row = line_row(&i.name, line, i.rss_bytes, t);
    let Some(p) = i.process else {
        return row.child(secondary("Shared with other sessions", t).flex_none());
    };
    let confirm = armed == Some(p);
    let label = if confirm { "Confirm end" } else { "End" };
    let end = if confirm {
        // Ending is the one action here that cannot be undone: the fail ribbon's red.
        div()
            .id(("end", n))
            .role(Role::Button)
            .flex_none()
            .px(px(sp::M))
            .py(px(sp::XS))
            .rounded(px(8.))
            .with_focus_ring(t)
            .bg(rgb(t.fail))
            .text_color(rgb(ON_RIBBON))
            .font_weight(FontWeight::MEDIUM)
            .hover(|s| s.opacity(0.9))
            .child(label)
    } else {
        button(("end", n), label, t)
    };
    let (end_view, name) = (view.clone(), i.name.clone());
    row.when(keep, |d| {
        d.child(
            button(("keep", n), "Keep", t)
                .aria_label(format!("Keep {name}"))
                .on_click(move |_, _, cx| send(Command::Keep(p), cx)),
        )
    })
    .child(
        end.aria_label(format!("{label} {}", i.name))
            .on_click(move |_, _, cx| {
                end_view.update(cx, |v, cx| v.end(p, cx)).ok();
            }),
    )
}

fn log_row(e: &CleanupLogEntry, now_ms: i64, t: Theme) -> Div {
    let by = if e.automatic {
        "automatically"
    } else {
        "by you"
    };
    let line = format!("Ended {by} · {} ago", ago(now_ms - e.at_ms));
    line_row(&e.name, &line, e.rss_bytes, t)
}

/// One row of each kind, the log, and low memory, for screenshots; the first
/// row's End is the one to arm.
#[cfg(feature = "screenshot")]
pub(super) fn fixture(now: i64) -> crate::cleanup::CleanupView {
    const H: i64 = 3_600_000;
    const MB: u64 = 1 << 20;
    let p = |pid| {
        Some(ProcessIdentity {
            pid,
            start_time_s: 1,
        })
    };
    let item = |process, name: &str, reason, rss_bytes, since_ms, clean_at_ms| CleanupItem {
        process,
        name: name.into(),
        reason,
        rss_bytes,
        since_ms,
        warned: true,
        clean_at_ms,
        kept: false,
    };
    let orphan = |provider, id: &str| Reason::Orphan {
        provider,
        session_id: id.into(),
    };
    let idle = |provider, waiting| Reason::IdleSession {
        provider,
        session_id: "s".into(),
        waiting,
    };
    let log = |at_ms, name: &str, rss_bytes, automatic| CleanupLogEntry {
        at_ms,
        name: name.into(),
        provider: None,
        session_id: None,
        rss_bytes,
        automatic,
    };
    let mut items = vec![
        item(
            p(4242),
            "node",
            orphan(Provider::Codex, "0193a7c2-4b1e-7d2a"),
            1400 * MB,
            now - 21 * H,
            Some(now + 3 * H),
        ),
        item(
            p(4243),
            "vite",
            orphan(Provider::ClaudeCode, "8d1f0c3e"),
            300 * MB,
            now - 30 * 60_000,
            None,
        ),
        item(
            p(5001),
            "Claude Code · agentpulse",
            idle(Provider::ClaudeCode, true),
            600 * MB,
            now - 5 * H,
            None,
        ),
        item(
            None,
            "Codex · website",
            idle(Provider::Codex, false),
            0,
            now - 40 * 60_000,
            None,
        ),
        item(p(6100), "python3.12", Reason::Untracked, 90 * MB, 0, None),
    ];
    // vite: kept.
    items[1].kept = true;
    crate::cleanup::CleanupView {
        items,
        log: vec![
            log(now - 2 * H, "esbuild", 210 * MB, true),
            log(now - 26 * H, "node", 1100 * MB, false),
        ],
        memory_low: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000;

    fn item(reason: Reason, since_ms: i64) -> CleanupItem {
        CleanupItem {
            process: None,
            name: "node".into(),
            reason,
            rss_bytes: 0,
            since_ms,
            warned: false,
            clean_at_ms: None,
            kept: false,
        }
    }

    #[test]
    fn why_reads_plainly() {
        let idle = |waiting| Reason::IdleSession {
            provider: Provider::Codex,
            session_id: "s".into(),
            waiting,
        };
        let orphan = Reason::Orphan {
            provider: Provider::Codex,
            session_id: "0193a7c2-4b1e".into(),
        };
        assert_eq!(
            why(&item(idle(true), 0), None, 5 * H),
            "Waiting on you for 5 h"
        );
        assert_eq!(
            why(&item(idle(false), 0), None, 5 * H + 59 * 60_000),
            "Idle for 5 h"
        );
        assert_eq!(
            why(&item(idle(false), 0), None, 40 * 60_000),
            "Idle for 40 min"
        );
        assert_eq!(
            why(&item(orphan.clone(), 0), Some("agentpulse"), H),
            "From Codex · agentpulse"
        );
        assert_eq!(why(&item(orphan, 0), None, H), "From Codex · 0193a7c2");
        assert_eq!(
            why(&item(Reason::Untracked, 0), None, 0),
            "Not started by an agent AgentPulse saw"
        );
    }

    #[test]
    fn ends_reads_plainly() {
        let orphan = |clean_at_ms, kept| CleanupItem {
            clean_at_ms,
            kept,
            ..item(
                Reason::Orphan {
                    provider: Provider::Codex,
                    session_id: "s".into(),
                },
                0,
            )
        };
        assert_eq!(
            ends(&orphan(Some(3 * H), false), 0).as_deref(),
            Some("Ends in 3 h")
        );
        assert_eq!(
            ends(&orphan(Some(40 * 60_000), false), 0).as_deref(),
            Some("Ends in 40 min")
        );
        assert_eq!(ends(&orphan(None, true), 0).as_deref(), Some("Kept"));
        assert_eq!(
            ends(&orphan(None, false), 0).as_deref(),
            Some("Automatic end is off")
        );
        assert_eq!(ends(&item(Reason::Untracked, 0), 0), None);
    }
}
