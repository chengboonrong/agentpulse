//! The window: an Overview and a Sessions screen over the latest snapshot.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use agentpulse_animation::{CatAnimator, Motion, Palette, Sprite, palette};
use agentpulse_core::{AgentSession, Provider, Reading, SessionStatus};
use gpui::{
    App, Bounds, Context, Div, FontFeatures, FontWeight, KeyBinding, Menu, MenuItem, PathBuilder,
    QuitMode, Rgba, Task, TitlebarOptions, Window, WindowAppearance, WindowBounds, WindowOptions,
    actions, canvas, div, fill, point, prelude::*, px, rgb, size,
};

use crate::collector::{HISTORY_LEN, Snapshot};
use crate::format::{self, DASH};

actions!(agentpulse, [Quit]);

#[derive(Default)]
pub struct Options {
    /// Forces dark (`Some(true)`) or light instead of following the system.
    pub appearance: Option<bool>,
    #[cfg(feature = "screenshot")]
    pub screenshot_dir: Option<std::path::PathBuf>,
}

pub fn run(rx: async_channel::Receiver<Snapshot>, options: Options) {
    gpui_platform::application()
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx: &mut App| {
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            cx.set_menus([
                Menu::new("AgentPulse").items([MenuItem::action("Quit AgentPulse", Quit)])
            ]);
            let bounds = Bounds::centered(None, size(px(1120.), px(720.)), cx);
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("AgentPulse".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(12.), px(13.))),
                    }),
                    window_min_size: Some(size(px(880.), px(560.))),
                    focus: true,
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| AppView::new(rx, options, window, cx)),
            );
            if let Err(e) = opened {
                tracing::error!(error = %format!("{e:#}"), "cannot open window");
                cx.quit();
            }
            cx.activate(true);
        });
}

// ------------------------------------------------------------------- theme

/// Colours as 0xRRGGBB so the contrast test can check them.
#[derive(Clone, Copy)]
struct Theme {
    window: u32,
    chrome: u32,
    surface: u32,
    border: u32,
    selected: u32,
    text: u32,
    muted: u32,
    accent: u32,
    active: u32,
    waiting: u32,
    idle: u32,
    completed: u32,
    failed: u32,
    banner: u32,
    banner_border: u32,
}

const LIGHT: Theme = Theme {
    window: 0xFFFFFF,
    chrome: 0xF3F3F5,
    surface: 0xF7F7F9,
    border: 0xE1E1E6,
    selected: 0xE1E1E7,
    text: 0x1D1D1F,
    muted: 0x5C5C64,
    accent: 0x0A64D2,
    active: 0x1A7F37,
    waiting: 0x8F5F00,
    idle: 0x4A5590,
    completed: 0x5C5C64,
    failed: 0xC21F2B,
    banner: 0xFFF6E0,
    banner_border: 0xE6C46E,
};

const DARK: Theme = Theme {
    window: 0x1C1C1E,
    chrome: 0x242427,
    surface: 0x26262A,
    border: 0x3A3A40,
    selected: 0x3A3A41,
    text: 0xECECEE,
    muted: 0xA3A3AB,
    accent: 0x4C9BFF,
    active: 0x48C35A,
    waiting: 0xDDA53A,
    idle: 0xA3AEE8,
    completed: 0xA3A3AB,
    failed: 0xFF7B70,
    banner: 0x3A2F17,
    banner_border: 0x6E5826,
};

impl Theme {
    fn status(&self, s: SessionStatus) -> u32 {
        match s {
            SessionStatus::Active => self.active,
            SessionStatus::Waiting => self.waiting,
            SessionStatus::Idle => self.idle,
            SessionStatus::Completed => self.completed,
            SessionStatus::Failed => self.failed,
        }
    }
}

fn is_dark(a: WindowAppearance) -> bool {
    matches!(a, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

// ------------------------------------------------------------------ motion

fn motion() -> Motion {
    let off = std::env::var("AGENTPULSE_ANIMATION").is_ok_and(|v| v == "off");
    let speed = std::env::var("AGENTPULSE_ANIMATION_SPEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    Motion {
        enabled: !off && !reduce_motion(),
        speed,
    }
}

#[cfg(target_os = "macos")]
fn reduce_motion() -> bool {
    // GPUI does not read this setting on macOS.
    let on = objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
    tracing::debug!(reduce_motion = on, "read accessibility setting");
    on
}

#[cfg(not(target_os = "macos"))]
fn reduce_motion() -> bool {
    false
}

// -------------------------------------------------------------------- view

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Overview,
    Sessions,
}

const STOPPED: &str = "Data collection stopped. Restart AgentPulse.";

struct AppView {
    snapshot: Option<Snapshot>,
    /// The snapshot channel closed; the last snapshot is no longer current.
    stopped: bool,
    screen: Screen,
    dark: bool,
    forced_dark: Option<bool>,
    cats: HashMap<String, CatAnimator>,
    /// Shown when no agent is live.
    empty_cat: CatAnimator,
    motion: Motion,
    last_tick: Instant,
    active: bool,
    visible: bool,
    animation: Option<Task<()>>,
    _snapshots: Task<()>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl AppView {
    fn new(
        rx: async_channel::Receiver<Snapshot>,
        options: Options,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe_window_appearance(window, |this, window, cx| {
                this.dark = this.forced_dark.unwrap_or(is_dark(window.appearance()));
                cx.notify();
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                this.active = window.is_window_active();
                if this.active {
                    let m = motion();
                    if m != this.motion {
                        tracing::info!(
                            enabled = m.enabled,
                            speed = m.speed,
                            "animation settings changed"
                        );
                    }
                    this.motion = m;
                }
                this.schedule_animation(cx);
            }),
            cx.observe_window_visibility(window, |this, visibility, _, cx| {
                this.visible = visibility.is_visible();
                this.schedule_animation(cx);
            }),
        ];
        #[cfg(feature = "screenshot")]
        let mut screenshot_dir = options.screenshot_dir;
        let snapshots = cx.spawn_in(window, async move |this, cx| {
            while let Ok(snap) = rx.recv().await {
                #[cfg(feature = "screenshot")]
                let indexing = snap.indexing;
                if this.update(cx, |this, cx| this.apply(snap, cx)).is_err() {
                    return;
                }
                #[cfg(feature = "screenshot")]
                if !indexing && let Some(dir) = screenshot_dir.take() {
                    // Separate task, so snapshots keep arriving while it waits.
                    let this = this.clone();
                    cx.spawn(async move |cx| {
                        if let Err(e) = screenshot(&this, &dir, cx).await {
                            tracing::error!(error = %format!("{e:#}"), "screenshot failed");
                        }
                        cx.update(|_, cx| cx.quit()).ok();
                    })
                    .detach();
                }
            }
            // The collector thread is gone: say so rather than show its last numbers as live.
            tracing::error!("collector stopped");
            this.update(cx, |this, cx| {
                this.stopped = true;
                cx.notify();
            })
            .ok();
        });
        let m = motion();
        tracing::info!(
            reduce_motion = reduce_motion(),
            animation = m.enabled,
            speed = m.speed,
            "animation settings"
        );
        Self {
            snapshot: None,
            stopped: false,
            screen: Screen::Overview,
            dark: options.appearance.unwrap_or(is_dark(window.appearance())),
            forced_dark: options.appearance,
            cats: HashMap::new(),
            empty_cat: CatAnimator::new(),
            motion: m,
            last_tick: Instant::now(),
            active: window.is_window_active(),
            visible: window.visibility().is_visible(),
            animation: None,
            _snapshots: snapshots,
            _subscriptions: subscriptions,
        }
    }

    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        let live: Vec<&AgentSession> = snap
            .sessions
            .iter()
            .filter(|s| s.status.is_live())
            .collect();
        self.cats
            .retain(|id, _| live.iter().any(|s| &s.session_id == id));
        for s in &live {
            self.cats
                .entry(s.session_id.clone())
                .or_default()
                .set_input(Some(s.status), s.output_rate);
        }
        if live.is_empty() {
            self.empty_cat.set_input(None, 0.0);
        }
        self.snapshot = Some(snap);
        self.schedule_animation(cx);
        cx.notify();
    }

    /// Cats currently on screen.
    fn visible_cats(&self) -> Vec<&CatAnimator> {
        if self.screen != Screen::Overview {
            return vec![];
        }
        if self.cats.is_empty() {
            vec![&self.empty_cat]
        } else {
            self.cats.values().collect()
        }
    }

    /// One timer for all cats, set to the soonest frame change. No timer while
    /// nothing will change, the window is inactive or hidden, or no cat is shown.
    fn schedule_animation(&mut self, cx: &mut Context<Self>) {
        self.animation = None;
        if !(self.active && self.visible) {
            return;
        }
        let motion = self.motion;
        let Some(next) = self
            .visible_cats()
            .iter()
            .filter_map(|c| c.next_frame_in(motion))
            .min()
        else {
            return;
        };
        let wait = next.saturating_sub(self.last_tick.elapsed());
        self.animation = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                let dt = this.last_tick.elapsed();
                this.last_tick = Instant::now();
                let mut repaint = false;
                for cat in this.cats.values_mut().chain([&mut this.empty_cat]) {
                    repaint |= cat.tick(dt, motion);
                }
                if repaint {
                    cx.notify();
                }
                this.schedule_animation(cx);
            })
            .ok();
        }));
    }

    fn theme(&self) -> Theme {
        if self.dark { DARK } else { LIGHT }
    }
}

#[cfg(feature = "screenshot")]
async fn screenshot(
    this: &gpui::WeakEntity<AppView>,
    dir: &std::path::Path,
    cx: &mut gpui::AsyncWindowContext,
) -> anyhow::Result<()> {
    use std::time::Duration;
    std::fs::create_dir_all(dir)?;
    cx.background_executor().timer(Duration::from_secs(3)).await;
    cx.update(|window, cx| capture(window, cx, &dir.join("overview.png")))??;
    this.update(cx, |this, cx| {
        this.screen = Screen::Sessions;
        this.schedule_animation(cx);
        cx.notify();
    })?;
    cx.update(|window, cx| capture(window, cx, &dir.join("sessions.png")))??;
    tracing::info!(dir = %dir.display(), "saved screenshots");
    Ok(())
}

/// Draws a fresh frame and reads it back. Drawing here rather than waiting
/// for the display link also works while the window is not on screen.
#[cfg(feature = "screenshot")]
fn capture(window: &mut Window, cx: &mut App, path: &std::path::Path) -> anyhow::Result<()> {
    window.draw(cx).clear(cx);
    window.render_to_image()?.save(path)?;
    Ok(())
}

impl Render for AppView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let indexing = !self.stopped && self.snapshot.as_ref().is_some_and(|s| s.indexing);
        let content = match &self.snapshot {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(t.muted))
                .child(if self.stopped {
                    STOPPED
                } else {
                    "Reading agent data…"
                })
                .into_any_element(),
            Some(s) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .children(banner(
                    s.errors
                        .iter()
                        .map(String::as_str)
                        .chain(self.stopped.then_some(STOPPED)),
                    t,
                ))
                .child(match self.screen {
                    Screen::Overview => self.overview(s, t).into_any_element(),
                    Screen::Sessions => sessions(s, t).into_any_element(),
                })
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(t.window))
            .text_color(rgb(t.text))
            .text_size(px(13.))
            .child(
                div()
                    .h(px(38.))
                    .flex_none()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(t.chrome))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("AgentPulse"))
                    .when(indexing, |d| {
                        d.child(
                            div()
                                .absolute()
                                .right(px(16.))
                                .top_0()
                                .h_full()
                                .flex()
                                .items_center()
                                .text_xs()
                                .text_color(rgb(t.muted))
                                .child("Indexing history…"),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(176.))
                            .flex_none()
                            .p_2()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .bg(rgb(t.chrome))
                            .border_r_1()
                            .border_color(rgb(t.border))
                            .child(self.nav("Overview", Screen::Overview, t, cx))
                            .child(self.nav("Sessions", Screen::Sessions, t, cx)),
                    )
                    .child(div().flex_1().min_w_0().flex().flex_col().child(content)),
            )
    }
}

impl AppView {
    fn nav(
        &self,
        label: &'static str,
        screen: Screen,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(label)
            .px_2()
            .py_1()
            .rounded_md()
            .when(self.screen == screen, |d| {
                d.bg(rgb(t.selected)).font_weight(FontWeight::MEDIUM)
            })
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.screen = screen;
                this.schedule_animation(cx);
                cx.notify();
            }))
    }

    fn overview(&self, s: &Snapshot, t: Theme) -> impl IntoElement {
        let count = |st| s.sessions.iter().filter(|x| x.status == st).count();
        let (active, waiting, idle) = (
            count(SessionStatus::Active),
            count(SessionStatus::Waiting),
            count(SessionStatus::Idle),
        );
        let tk = s.today_tokens;
        let unpriced = (s.today_unpriced_events > 0)
            .then(|| format!("{} responses unpriced", s.today_unpriced_events));
        let (cost, cost_note) = match s.today_cost_usd {
            Reading::Measured(v) | Reading::Estimated(v) => (
                format!("≈ {}", format::cost(v)),
                "estimate, API list prices",
            ),
            Reading::Unavailable => (DASH.into(), "no price for today's models"),
        };
        let no_process = "No live agent process";
        let cpu_note = match (s.agents_cpu_percent, s.agents_rss_bytes) {
            (Some(_), _) => "% of one core, all agent process trees",
            (None, Some(_)) => "Measuring, needs a second sample",
            (None, None) => no_process,
        };
        let live: Vec<&AgentSession> = s.sessions.iter().filter(|x| x.status.is_live()).collect();

        div()
            .id("overview")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_6()
            .flex()
            .flex_col()
            .gap_5()
            .child(heading("Overview"))
            .child(
                div()
                    .grid()
                    .grid_cols(3)
                    .gap_3()
                    .child(card(
                        t,
                        "Active agents",
                        (active + waiting + idle).to_string(),
                        vec![format!("{active} active, {waiting} waiting, {idle} idle")],
                    ))
                    .child(card(
                        t,
                        "Tokens today",
                        format::tokens(tk.total()),
                        vec![
                            format!(
                                "{} input · {} output",
                                format::tokens(tk.input),
                                format::tokens(tk.output)
                            ),
                            format!(
                                "{} cache write · {} cache read",
                                format::tokens(tk.cache_write_5m + tk.cache_write_1h),
                                format::tokens(tk.cache_read)
                            ),
                        ],
                    ))
                    .child(card(
                        t,
                        "Estimated cost today",
                        cost,
                        [Some(cost_note.to_string()), unpriced]
                            .into_iter()
                            .flatten()
                            .collect(),
                    ))
                    .child(card(
                        t,
                        "Agent CPU",
                        s.agents_cpu_percent
                            .map_or(DASH.into(), |c| format!("{c:.1}%")),
                        vec![cpu_note.into()],
                    ))
                    .child(card(
                        t,
                        "Agent RAM",
                        s.agents_rss_bytes.map_or(DASH.into(), format::bytes),
                        vec![
                            if s.agents_rss_bytes.is_some() {
                                "Resident memory, all agent process trees"
                            } else {
                                no_process
                            }
                            .into(),
                        ],
                    ))
                    .child(card(
                        t,
                        "GPU",
                        DASH.into(),
                        vec!["Unavailable — not collected in this build".into()],
                    )),
            )
            .child(
                div()
                    .p_3()
                    .rounded_lg()
                    .bg(rgb(t.surface))
                    .border_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .w(px(180.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(label("Output token rate", t))
                            .child(big_number(format!("{:.1}", s.output_rate)))
                            .child(label("tok/s, 30 s average", t)),
                    )
                    .child(sparkline(s.rate_history.clone(), t)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Live agents"))
                    .child(if live.is_empty() {
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_3()
                            .py_8()
                            .child(cat(
                                self.empty_cat.sprite(),
                                palette(Provider::ClaudeCode),
                                4.,
                            ))
                            .child(div().text_color(rgb(t.muted)).child("No agents running"))
                            .into_any_element()
                    } else {
                        h_scroll(
                            "live-scroll",
                            LIVE_MIN_WIDTH,
                            header(&LIVE_COLUMNS, t).into_any_element(),
                            live.iter().map(|x| self.live_row(x, s.taken_at_ms, t)),
                        )
                        .into_any_element()
                    }),
            )
    }

    fn live_row(&self, s: &AgentSession, now_ms: i64, t: Theme) -> gpui::AnyElement {
        let w = |i: usize| LIVE_COLUMNS[i].1;
        let resources = s.resources.value();
        let sprite = self
            .cats
            .get(&s.session_id)
            .map_or(self.empty_cat.sprite(), |c| c.sprite());
        row(t)
            .child(
                div()
                    .w(px(w(0)))
                    .flex_none()
                    .child(cat(sprite, palette(s.provider), 2.)),
            )
            .child(project_cell(s, t))
            .child(text_cell(w(2), s.model.clone(), t))
            .child(status_cell(s.status, w(3), t))
            .child(num(w(4), s.wall_ms(now_ms).map(format::duration), t))
            .child(num(w(5), Some(format::duration(s.active_ms)), t))
            .child(num(w(6), Some(format::tokens(s.tokens.total())), t))
            .child(num(w(7), cost_text(s.cost_usd), t))
            .child(num(
                w(8),
                resources
                    .and_then(|r| r.cpu_percent)
                    .map(|c| format!("{c:.1}%")),
                t,
            ))
            .child(num(w(9), resources.map(|r| format::bytes(r.rss_bytes)), t))
            .child(num(w(10), Some(format!("{:.1}", s.output_rate)), t))
            .into_any_element()
    }
}

// ---------------------------------------------------------------- sessions

fn sessions(s: &Snapshot, t: Theme) -> impl IntoElement {
    let w = |i: usize| SESSION_COLUMNS[i].1;
    let n = s.sessions.len();
    div()
        .flex_1()
        .min_h_0()
        .p_6()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .flex()
                .items_baseline()
                .gap_3()
                .child(heading("Sessions"))
                .child(label(
                    &format!(
                        "{n} {} in the last 24 hours or still running",
                        if n == 1 { "session" } else { "sessions" }
                    ),
                    t,
                )),
        )
        .child(
            div().flex_1().min_h_0().flex().flex_col().child(
                div()
                    .id("sessions-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .child(
                        div()
                            .min_w(px(SESSION_MIN_WIDTH))
                            .flex()
                            .flex_col()
                            .child(header(&SESSION_COLUMNS, t))
                            .children(s.sessions.iter().map(|x| {
                                row(t)
                                    .child(status_cell(x.status, w(0), t))
                                    .child(project_cell(x, t))
                                    .child(text_cell(w(2), x.model.clone(), t))
                                    .child(num(
                                        w(3),
                                        x.started_at_ms
                                            .map(|ms| format::start_time(ms, s.taken_at_ms)),
                                        t,
                                    ))
                                    .child(num(
                                        w(4),
                                        x.wall_ms(s.taken_at_ms).map(format::duration),
                                        t,
                                    ))
                                    .child(num(w(5), Some(format::duration(x.active_ms)), t))
                                    .child(num(w(6), Some(format::tokens(x.tokens.total())), t))
                                    .child(num(w(7), cost_text(x.cost_usd), t))
                            })),
                    ),
            ),
        )
}

// ------------------------------------------------------------------ pieces

/// Column title and width; a width of 0 takes the remaining space.
type Column = (&'static str, f32);

const LIVE_COLUMNS: [Column; 11] = [
    // Cat sprites are 32 px wide, drawn at 2x.
    ("", 64.),
    ("Project", 0.),
    ("Model", 112.),
    ("Status", 76.),
    ("Runtime", 60.),
    ("Active (est.)", 76.),
    ("Tokens", 52.),
    ("Est. cost", 60.),
    ("CPU", 52.),
    ("RAM", 60.),
    ("tok/s", 44.),
];
const LIVE_MIN_WIDTH: f32 = 876.;

const SESSION_COLUMNS: [Column; 8] = [
    ("Status", 92.),
    ("Project", 0.),
    ("Model", 128.),
    ("Started", 92.),
    ("Duration", 68.),
    ("Active (est.)", 80.),
    ("Tokens", 56.),
    ("Est. cost", 64.),
];
const SESSION_MIN_WIDTH: f32 = 860.;

fn heading(text: &'static str) -> Div {
    div()
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

fn label(text: &str, t: Theme) -> Div {
    div()
        .text_xs()
        .text_color(rgb(t.muted))
        .child(text.to_string())
}

fn big_number(text: String) -> Div {
    div()
        .text_size(px(22.))
        .font_weight(FontWeight::SEMIBOLD)
        .font_features(tabular())
        .child(text)
}

fn card(t: Theme, title: &str, value: String, details: Vec<String>) -> Div {
    div()
        .p_3()
        .rounded_lg()
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(t.border))
        .flex()
        .flex_col()
        .gap_1()
        .child(label(title, t))
        .child(big_number(value))
        .children(details.iter().map(|d| label(d, t)))
}

fn banner<'a>(errors: impl Iterator<Item = &'a str>, t: Theme) -> Option<Div> {
    let errors: Vec<&str> = errors.collect();
    (!errors.is_empty()).then(|| {
        div()
            .mx_6()
            .mt_4()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgb(t.banner))
            .border_1()
            .border_color(rgb(t.banner_border))
            .text_xs()
            .flex()
            .flex_col()
            .gap_0p5()
            .children(errors.into_iter().map(|e| div().child(e.to_string())))
    })
}

fn row(t: Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_2()
        .min_h(px(44.))
        .py_1()
        .border_b_1()
        .border_color(rgb(t.border))
        .text_xs()
}

fn header(columns: &[Column], t: Theme) -> Div {
    let mut d = row(t)
        .min_h(px(28.))
        .text_color(rgb(t.muted))
        .font_weight(FontWeight::MEDIUM);
    for &(title, width) in columns {
        let cell = if width == 0. {
            div().flex_1().min_w_0()
        } else {
            div().w(px(width)).flex_none()
        };
        // Numeric columns are right-aligned like their values.
        let numeric = !matches!(title, "" | "Project" | "Model" | "Status");
        d = d.child(cell.when(numeric, |c| c.text_right()).child(title));
    }
    d
}

/// A horizontally scrollable table that keeps its columns at narrow widths.
fn h_scroll(
    id: &'static str,
    min_width: f32,
    header: gpui::AnyElement,
    rows: impl Iterator<Item = gpui::AnyElement>,
) -> impl IntoElement {
    div().id(id).overflow_x_scroll().child(
        div()
            .min_w(px(min_width))
            .flex()
            .flex_col()
            .child(header)
            .children(rows),
    )
}

fn project_cell(s: &AgentSession, t: Theme) -> Div {
    let path = s.project_path.as_deref();
    let name = path
        .and_then(|p| p.trim_end_matches('/').rsplit('/').next())
        .filter(|n| !n.is_empty())
        .unwrap_or(DASH);
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .child(name.to_string()),
        )
        .child(
            div()
                .truncate()
                .text_size(px(11.))
                .text_color(rgb(t.muted))
                .child(path.unwrap_or("Project path unknown").to_string()),
        )
}

fn text_cell(width: f32, text: Option<String>, t: Theme) -> Div {
    let cell = div().w(px(width)).flex_none().truncate();
    match text {
        Some(text) => cell.child(text),
        None => cell.text_color(rgb(t.muted)).child(DASH),
    }
}

fn status_cell(s: SessionStatus, width: f32, t: Theme) -> Div {
    let color = rgb(t.status(s));
    div()
        .w(px(width))
        .flex_none()
        .flex()
        .items_center()
        .gap_1p5()
        .text_color(color)
        .font_weight(FontWeight::MEDIUM)
        .child(div().size(px(7.)).flex_none().rounded_full().bg(color))
        .child(s.label())
}

fn num(width: f32, text: Option<String>, t: Theme) -> Div {
    let cell = div()
        .w(px(width))
        .flex_none()
        .text_right()
        .whitespace_nowrap()
        .font_features(tabular());
    match text {
        Some(text) => cell.child(text),
        None => cell.text_color(rgb(t.muted)).child(DASH),
    }
}

fn cost_text(c: Reading<f64>) -> Option<String> {
    c.value().map(|v| format::cost(*v))
}

fn sparkline(history: Vec<f32>, t: Theme) -> impl IntoElement {
    let (line, base) = (rgb(t.accent), rgb(t.border));
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            window.paint_quad(fill(
                Bounds::new(
                    point(b.left(), b.bottom() - px(1.)),
                    size(b.size.width, px(1.)),
                ),
                base,
            ));
            if history.len() < 2 {
                return;
            }
            let max = history.iter().copied().fold(1.0, f32::max);
            // Fixed spacing, so the line grows in from the right as samples arrive.
            let step = b.size.width * (1.0 / (HISTORY_LEN - 1) as f32);
            let x0 = b.right() - step * (history.len() - 1) as f32;
            let height = b.size.height - px(3.);
            let mut path = PathBuilder::stroke(px(1.5));
            for (i, v) in history.iter().enumerate() {
                let p = point(
                    x0 + step * i as f32,
                    b.bottom() - px(1.5) - height * (v / max),
                );
                if i == 0 {
                    path.move_to(p);
                } else {
                    path.line_to(p);
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, line);
            }
        },
    )
    .flex_1()
    .h(px(56.))
}

/// Paints a sprite with square pixels at an integer scale, one quad per run of
/// equal ink in a row.
fn cat(sprite: &'static Sprite, palette: Palette, scale: f32) -> impl IntoElement {
    let (w, h) = (sprite.width(), sprite.height());
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            // Whole-point origin keeps every sprite pixel on device-pixel edges.
            let origin = point(b.origin.x.round(), b.origin.y.round());
            for y in 0..h {
                let mut x = 0;
                while x < w {
                    let (start, ink) = (x, sprite.ink(x, y));
                    while x < w && sprite.ink(x, y) == ink {
                        x += 1;
                    }
                    if let Some([r, g, bl, a]) = palette.color(ink) {
                        let color = Rgba {
                            r: r as f32 / 255.,
                            g: g as f32 / 255.,
                            b: bl as f32 / 255.,
                            a: a as f32 / 255.,
                        };
                        window.paint_quad(fill(
                            Bounds::new(
                                origin + point(px(start as f32 * scale), px(y as f32 * scale)),
                                size(px((x - start) as f32 * scale), px(scale)),
                            ),
                            color,
                        ));
                    }
                }
            }
        },
    )
    .w(px(w as f32 * scale))
    .h(px(h as f32 * scale))
    .flex_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: u32) -> f64 {
        let ch = |shift: u32| {
            let v = ((c >> shift) & 0xFF) as f64 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
    }

    fn contrast(a: u32, b: u32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_meets_wcag_aa_in_both_themes() {
        for t in [LIGHT, DARK] {
            let status = [t.active, t.waiting, t.idle, t.completed, t.failed];
            // Each text colour against every background it is drawn on.
            let pairs = [
                (
                    &[t.text][..],
                    &[t.window, t.chrome, t.surface, t.selected, t.banner][..],
                ),
                (&[t.muted], &[t.window, t.chrome, t.surface]),
                (&status, &[t.window, t.surface]),
            ];
            for (fgs, bgs) in pairs {
                for (&fg, &bg) in fgs.iter().flat_map(|f| bgs.iter().map(move |b| (f, b))) {
                    let c = contrast(fg, bg);
                    assert!(c >= 4.5, "{fg:06x} on {bg:06x}: {c:.2}");
                }
            }
            assert!(contrast(t.accent, t.surface) >= 3.0, "sparkline");
        }
    }

    #[test]
    fn contrast_formula() {
        assert!((contrast(0x000000, 0xFFFFFF) - 21.0).abs() < 1e-9);
        assert!((contrast(0x777777, 0xFFFFFF) - 4.48).abs() < 0.01);
    }
}
