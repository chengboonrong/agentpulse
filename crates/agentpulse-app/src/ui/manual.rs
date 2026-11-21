//! The Clean-up screen's manual cards: Disk caches, Worktrees and This Mac.
//! Check lists what it finds and changes nothing; every action takes the same
//! two clicks as End. Results live in the window's memory only.

use std::collections::{HashMap, HashSet};

use agentpulse_process::Pressure;

use super::cleanup::{CONFIRM, danger, line_row};
use super::settings::{button, card_title};
use super::*;
use crate::tidy::{self, Action, Cache, CacheRow, Mac, Repo, plural};

#[derive(Clone, Copy, PartialEq)]
enum Card {
    Caches,
    Worktrees,
    Mac,
}

/// Title, Check's accessible label, and the line shown before the first Check.
const CARDS: [(Card, &str, &str, &str); 3] = [
    (
        Card::Caches,
        "Disk caches",
        "Check disk caches",
        "Homebrew, uv and npm download caches.",
    ),
    (
        Card::Worktrees,
        "Worktrees",
        "Check worktrees",
        "Worktrees of repos your agents worked in.",
    ),
    (
        Card::Mac,
        "This Mac",
        "Check this Mac",
        "Swap, simulators and virtual machines.",
    ),
];

#[derive(Default)]
pub(super) struct Manual {
    caches: Option<Vec<CacheRow>>,
    worktrees: Option<Vec<Repo>>,
    mac: Option<Mac>,
    /// Per card, in [`Card`] order.
    checking: [bool; 3],
    /// Bytes freed this session, per card.
    freed: [u64; 3],
    running: HashSet<Action>,
    /// What each finished action freed, or its error, until its card's next Check.
    done: HashMap<Action, Result<u64, String>>,
    /// The action whose button was clicked once, and when.
    armed: Option<(Action, Instant)>,
}

enum Checked {
    Caches(Vec<CacheRow>),
    Worktrees(Vec<Repo>),
    Mac(Mac),
}

fn card_of(a: &Action) -> Card {
    match a {
        Action::Clean(_) => Card::Caches,
        Action::Prune(_) | Action::Remove { .. } => Card::Worktrees,
        Action::ShutDownSimulators | Action::DeleteUnavailableSimulators => Card::Mac,
    }
}

/// The button's label, then the armed one's.
fn labels(a: &Action) -> (&'static str, &'static str) {
    match a {
        Action::Clean(Cache::BrewFormulae) | Action::Remove { .. } => ("Remove", "Confirm remove"),
        Action::Clean(_) => ("Clean", "Confirm clean"),
        Action::Prune(_) => ("Prune", "Confirm prune"),
        Action::ShutDownSimulators => ("Shut down all", "Confirm shut down"),
        Action::DeleteUnavailableSimulators => ("Delete unavailable", "Confirm delete"),
    }
}

fn pressure(p: Option<Pressure>) -> &'static str {
    match p {
        Some(Pressure::Normal) => "normal",
        Some(Pressure::Warn) => "high",
        Some(Pressure::Critical) => "critical",
        None => "unknown",
    }
}

fn path(p: &std::path::Path) -> String {
    format::display_path(&p.to_string_lossy())
}

impl AppView {
    pub(super) fn manual_cards(&self, t: Theme, view: &WeakEntity<Self>) -> Vec<Div> {
        let m = &self.manual;
        CARDS
            .into_iter()
            .map(|(which, title, aria, blurb)| {
                let rows = match which {
                    Card::Caches => m.caches.as_ref().map(|c| self.cache_rows(c, t, view)),
                    Card::Worktrees => m.worktrees.as_ref().map(|r| self.worktree_rows(r, t, view)),
                    Card::Mac => m.mac.as_ref().map(|x| self.mac_rows(x, t, view)),
                };
                let body = match (rows, which) {
                    (None, _) => vec![secondary(blurb, t)],
                    (Some(r), Card::Caches) if r.is_empty() => {
                        vec![secondary("No Homebrew, uv or npm found.", t)]
                    }
                    (Some(r), Card::Worktrees) if r.is_empty() => {
                        vec![secondary("No stale worktrees.", t)]
                    }
                    // This Mac always has its swap row.
                    (Some(r), _) => r,
                };
                card(t)
                    .mt(px(sp::L))
                    .gap(px(sp::M))
                    .child(self.card_head(which, title, aria, t, view))
                    .children(body)
            })
            .collect()
    }

    fn card_head(
        &self,
        card: Card,
        title: &'static str,
        aria: &'static str,
        t: Theme,
        view: &WeakEntity<Self>,
    ) -> Div {
        let i = card as usize;
        let freed = self.manual.freed[i];
        let check = if self.manual.checking[i] {
            secondary("Checking…", t).flex_none().into_any_element()
        } else {
            let v = view.clone();
            button(("tidy-check", i), "Check", t)
                .aria_label(aria)
                .on_click(move |_, _, cx| {
                    v.update(cx, |v, cx| v.check(card, cx)).ok();
                })
                .into_any_element()
        };
        div()
            .flex()
            .items_center()
            .gap(px(sp::M))
            .child(div().flex_1().child(card_title(title)))
            .when(freed > 0, |d| {
                d.child(secondary(&format!("Freed {}", format::bytes(freed)), t).flex_none())
            })
            .child(check)
    }

    /// A row with an action button, or with `note` (why there is none) when `action` is None.
    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        n: usize,
        name: &str,
        line: String,
        bytes: u64,
        action: Option<Action>,
        note: Option<&str>,
        t: Theme,
        view: &WeakEntity<Self>,
    ) -> Div {
        let m = &self.manual;
        let done = action.as_ref().and_then(|a| m.done.get(a));
        // A failed action's status and stderr replace the line.
        let line = match done {
            Some(Err(e)) => e.clone(),
            _ => line,
        };
        let row = line_row(name, &line, bytes, t);
        let Some(a) = action else {
            return row.when_some(note, |d, note| d.child(secondary(note, t).flex_none()));
        };
        let end: gpui::AnyElement = match done {
            _ if m.running.contains(&a) => secondary("Cleaning…", t).flex_none().into_any_element(),
            Some(Ok(0)) => secondary("Done", t).flex_none().into_any_element(),
            Some(Ok(freed)) => secondary(&format!("Freed {}", format::bytes(*freed)), t)
                .flex_none()
                .into_any_element(),
            Some(Err(_)) | None => {
                let (label, confirm_label) = labels(&a);
                let armed = m.armed.as_ref().is_some_and(|(x, _)| *x == a);
                let (b, label) = if armed {
                    (danger(("tidy", n), confirm_label, t), confirm_label)
                } else {
                    (button(("tidy", n), label, t), label)
                };
                let v = view.clone();
                b.aria_label(format!("{label} {name}"))
                    .on_click(move |_, _, cx| {
                        v.update(cx, |v, cx| v.click(a.clone(), cx)).ok();
                    })
                    .into_any_element()
            }
        };
        row.child(end)
    }

    fn cache_rows(&self, caches: &[CacheRow], t: Theme, view: &WeakEntity<Self>) -> Vec<Div> {
        caches
            .iter()
            .enumerate()
            .map(|(k, r)| {
                let a = Action::Clean(r.cache);
                let line = match (&r.error, r.cache) {
                    (Some(e), _) => e.clone(),
                    (None, Cache::BrewFormulae) => r.formulae.join(", "),
                    (None, _) => r.dir.as_deref().map(path).unwrap_or_default(),
                };
                // What's left after a clean.
                let bytes = match self.manual.done.get(&a) {
                    Some(Ok(freed)) => r.bytes.saturating_sub(*freed),
                    _ => r.bytes,
                };
                // A failed Check offers nothing to clean.
                let action = r.error.is_none().then_some(a);
                self.row(k, r.cache.name(), line, bytes, action, None, t, view)
            })
            .collect()
    }

    fn worktree_rows(&self, repos: &[Repo], t: Theme, view: &WeakEntity<Self>) -> Vec<Div> {
        let mut rows = Vec::new();
        for repo in repos {
            let name = format::project_name(repo.main.to_str());
            if repo.prunable > 0 {
                let line = format!(
                    "{} already deleted",
                    plural(repo.prunable, "worktree folder")
                );
                let a = Action::Prune(repo.main.clone());
                rows.push(self.row(1000 + rows.len(), name, line, 0, Some(a), None, t, view));
            }
            for s in &repo.stale {
                let line = format!("{} · {}", s.why, path(&s.path));
                // Remove only when nothing would be lost.
                let a = s.blocker.is_none().then(|| Action::Remove {
                    main: repo.main.clone(),
                    path: s.path.clone(),
                });
                let n = 1000 + rows.len();
                rows.push(self.row(n, &s.name, line, s.bytes, a, s.blocker.as_deref(), t, view));
            }
        }
        rows
    }

    fn mac_rows(&self, mac: &Mac, t: Theme, view: &WeakEntity<Self>) -> Vec<Div> {
        let total = mac.swap_total.map_or(format::DASH.into(), format::bytes);
        let swap = format!("Of {total} · Memory pressure {}", pressure(mac.pressure));
        let mut rows = vec![self.row(
            2000,
            "Swap used",
            swap,
            mac.swap_used.unwrap_or(0),
            None,
            None,
            t,
            view,
        )];
        if let Some(s) = &mac.simulators {
            let shut = (s.booted > 0).then_some(Action::ShutDownSimulators);
            let line = format!("{} booted", plural(s.booted, "simulator"));
            rows.push(self.row(2001, "iOS simulators", line, 0, shut, None, t, view));
            let delete = (s.unavailable > 0).then_some(Action::DeleteUnavailableSimulators);
            let line = format!(
                "{} with no runtime installed",
                plural(s.unavailable, "simulator")
            );
            rows.push(self.row(
                2002,
                "Unavailable simulators",
                line,
                s.unavailable_bytes,
                delete,
                None,
                t,
                view,
            ));
        }
        // Report only: stopping a VM or an emulator can corrupt its disk.
        for (k, (product, rss)) in mac.vms.iter().enumerate() {
            let line = "Quit it from its own app to free this".to_string();
            rows.push(self.row(2100 + k, product, line, *rss, None, None, t, view));
        }
        rows
    }

    /// Every session's project in the snapshot, de-duplicated, and the live ones'.
    fn projects(&self, cx: &App) -> (Vec<String>, Vec<String>) {
        let (mut projects, mut live) = (Vec::new(), Vec::new());
        if let Some(s) = &self.hub.read(cx).snapshot {
            for x in &s.sessions {
                if let Some(p) = &x.project_path {
                    projects.push(p.clone());
                    if x.status.is_live() {
                        live.push(p.clone());
                    }
                }
            }
        }
        projects.sort();
        projects.dedup();
        (projects, live)
    }

    /// Runs a card's Check off the UI thread. Changes nothing on disk.
    fn check(&mut self, card: Card, cx: &mut Context<Self>) {
        let i = card as usize;
        if self.manual.checking[i] {
            return;
        }
        self.manual.checking[i] = true;
        self.manual.done.retain(|a, _| card_of(a) != card);
        let (projects, live) = self.projects(cx);
        let job = cx.background_spawn(async move {
            match card {
                Card::Caches => Checked::Caches(tidy::check_caches()),
                Card::Worktrees => Checked::Worktrees(tidy::check_worktrees(&projects, &live)),
                Card::Mac => Checked::Mac(tidy::check_mac()),
            }
        });
        cx.spawn(async move |this, cx| {
            let found = job.await;
            this.update(cx, |this, cx| {
                let m = &mut this.manual;
                m.checking[i] = false;
                match found {
                    Checked::Caches(c) => m.caches = Some(c),
                    Checked::Worktrees(r) => m.worktrees = Some(r),
                    Checked::Mac(x) => m.mac = Some(x),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The first click arms `a` for [`CONFIRM`]; a second click within that runs it.
    fn click(&mut self, a: Action, cx: &mut Context<Self>) {
        let m = &mut self.manual;
        if m.armed
            .as_ref()
            .is_some_and(|(x, at)| *x == a && at.elapsed() < CONFIRM)
        {
            m.armed = None;
            self.run(a, cx);
        } else {
            let at = Instant::now();
            m.armed = Some((a.clone(), at));
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(CONFIRM).await;
                this.update(cx, |this, cx| {
                    // Only this arming: a later click has its own timer.
                    let m = &mut this.manual;
                    if m.armed.as_ref().is_some_and(|(x, t)| *x == a && *t == at) {
                        m.armed = None;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// Runs a confirmed action off the UI thread and records what it freed.
    fn run(&mut self, a: Action, cx: &mut Context<Self>) {
        if !self.manual.running.insert(a.clone()) {
            return;
        }
        let job = {
            let a = a.clone();
            // The live sessions now, not at Check: an agent may have started since.
            let (_, live) = self.projects(cx);
            cx.background_spawn(async move { tidy::act(&a, &live) })
        };
        cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                if let Err(e) = &result {
                    tracing::warn!(error = %e, action = ?a, "clean-up action failed");
                }
                let m = &mut this.manual;
                m.running.remove(&a);
                if let Ok(freed) = result {
                    m.freed[card_of(&a) as usize] += freed;
                }
                m.done.insert(a, result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
