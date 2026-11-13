//! The window: an Overview and a Sessions screen over the hub's latest snapshot.
//! Closing it does not quit; the menu bar item and the Dock icon reopen it.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use agentpulse_animation::{CatAnimator, CatState, Ink, Palette, Sprite, frames, palette};
use agentpulse_core::{AgentSession, Provider, SessionStatus};
use agentpulse_platform_macos::{MainThreadMarker, MenuAction, StatusItem};
#[cfg(feature = "screenshot")]
use anyhow::Context as _;
use gpui::{
    App, Bounds, Context, Div, Entity, FontFeatures, FontWeight, Global, KeyBinding, Menu,
    MenuItem, PathBuilder, QuitMode, Rgba, Task, TitlebarOptions, Window, WindowAppearance,
    WindowBounds, WindowOptions, actions, canvas, div, fill, point, prelude::*, px, rgb, size,
};

use crate::collector::{HISTORY_LEN, Snapshot};
use crate::format::{self, DASH};
use crate::hub::Hub;

actions!(agentpulse, [Quit, CloseWindow]);

#[derive(Default)]
pub struct Options {
    /// Forces dark (`Some(true)`) or light instead of following the system.
    pub appearance: Option<bool>,
    #[cfg(feature = "screenshot")]
    pub screenshot_dir: Option<std::path::PathBuf>,
    /// Window content size in points, instead of the default.
    #[cfg(feature = "screenshot")]
    pub window_size: Option<(f32, f32)>,
}

#[cfg(feature = "screenshot")]
impl Options {
    /// `--screenshot <dir> [--appearance light|dark] [--size <width>x<height>]`
    pub fn screenshot(dir: &str, mut rest: &[&str]) -> anyhow::Result<Self> {
        const USAGE: &str = "usage: agentpulse --screenshot <dir> [--appearance light|dark] [--size <width>x<height>]";
        let mut o = Options {
            screenshot_dir: Some(dir.into()),
            ..Options::default()
        };
        while let [flag, value, tail @ ..] = rest {
            match (*flag, *value) {
                ("--appearance", "light") => o.appearance = Some(false),
                ("--appearance", "dark") => o.appearance = Some(true),
                ("--size", v) => {
                    let (w, h) = v.split_once('x').context(USAGE)?;
                    o.window_size = Some((w.parse()?, h.parse()?));
                }
                _ => anyhow::bail!(USAGE),
            }
            rest = tail;
        }
        anyhow::ensure!(rest.is_empty(), USAGE);
        Ok(o)
    }
}

/// What a reopened window needs.
struct Shared {
    hub: Entity<Hub>,
    appearance: Option<bool>,
}

impl Global for Shared {}

pub fn run(rx: async_channel::Receiver<Snapshot>, options: Options) {
    let app = gpui_platform::application().with_quit_mode(QuitMode::Explicit);
    // Clicking the Dock icon.
    app.on_reopen(show);
    // AppKit calls back from inside its event handling, so menu actions and
    // opened URLs are handled on the next turn of GPUI's loop instead of
    // re-entering it.
    let (tx, actions) = async_channel::unbounded();
    let urls = tx.clone();
    // A widget click opens agentpulse://open; other URLs are ignored.
    app.on_open_urls(move |list| {
        for url in list {
            let ours = url.starts_with("agentpulse://");
            tracing::debug!(ours, "open URL");
            if ours {
                urls.try_send(MenuAction::OpenDashboard).ok();
            }
        }
    });
    app.run(move |cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-w", CloseWindow, None),
        ]);
        cx.set_menus([Menu::new("AgentPulse").items([
            MenuItem::action("Close Window", CloseWindow),
            MenuItem::action("Quit AgentPulse", Quit),
        ])]);
        let mtm = MainThreadMarker::new().expect("GPUI runs this on the main thread");
        let item = StatusItem::new(
            mtm,
            Box::new(move |action| {
                tx.try_send(action).ok();
            }),
        );
        let hub = cx.new(|cx| Hub::new(rx, item, cx));
        cx.set_global(Shared {
            hub,
            appearance: options.appearance,
        });
        cx.spawn(async move |cx| {
            while let Ok(action) = actions.recv().await {
                cx.update(|cx| match action {
                    MenuAction::OpenDashboard => show(cx),
                    MenuAction::Quit => cx.quit(),
                });
            }
        })
        .detach();
        open_window(options, cx);
        cx.activate(true);
    });
}

/// Brings the window forward, or opens a new one if it was closed.
fn show(cx: &mut App) {
    tracing::debug!(existing = !cx.windows().is_empty(), "show window");
    cx.activate(true);
    match cx.windows().first() {
        Some(w) => {
            w.update(cx, |_, window, _| window.activate_window()).ok();
        }
        None => open_window(
            Options {
                appearance: cx.global::<Shared>().appearance,
                #[cfg(feature = "screenshot")]
                screenshot_dir: None,
                #[cfg(feature = "screenshot")]
                window_size: None,
            },
            cx,
        ),
    }
}

fn open_window(options: Options, cx: &mut App) {
    let hub = cx.global::<Shared>().hub.clone();
    #[cfg(feature = "screenshot")]
    let (w, h) = options.window_size.unwrap_or((1120., 720.));
    #[cfg(not(feature = "screenshot"))]
    let (w, h) = (1120., 720.);
    let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
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
        |window, cx| cx.new(|cx| AppView::new(hub, options, window, cx)),
    );
    if let Err(e) = opened {
        tracing::error!(error = %format!("{e:#}"), "cannot open window");
    }
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

// -------------------------------------------------------------------- view

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Overview,
    Sessions,
}

const STOPPED: &str = "Data collection stopped. Restart AgentPulse.";

/// View state only; the data and motion settings live in the [`Hub`].
struct AppView {
    hub: Entity<Hub>,
    screen: Screen,
    dark: bool,
    forced_dark: Option<bool>,
    cats: HashMap<String, CatAnimator>,
    /// Shown when no agent is live.
    empty_cat: CatAnimator,
    last_tick: Instant,
    active: bool,
    visible: bool,
    animation: Option<Task<()>>,
    #[cfg(feature = "screenshot")]
    screenshot_dir: Option<std::path::PathBuf>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl AppView {
    fn new(
        hub: Entity<Hub>,
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
                this.schedule_animation(cx);
            }),
            cx.observe_window_visibility(window, |this, visibility, _, cx| {
                this.visible = visibility.is_visible();
                this.schedule_animation(cx);
            }),
            cx.observe_in(&hub, window, |this, _, window, cx| this.sync(window, cx)),
        ];
        let mut view = Self {
            hub,
            screen: Screen::Overview,
            dark: options.appearance.unwrap_or(is_dark(window.appearance())),
            forced_dark: options.appearance,
            cats: HashMap::new(),
            empty_cat: CatAnimator::new(),
            last_tick: Instant::now(),
            active: window.is_window_active(),
            visible: window.visibility().is_visible(),
            animation: None,
            #[cfg(feature = "screenshot")]
            screenshot_dir: options.screenshot_dir,
            _subscriptions: subscriptions,
        };
        // A reopened window starts from the hub's current snapshot.
        view.sync(window, cx);
        view
    }

    /// Follows the hub's latest snapshot: one row cat per live session. Once
    /// collection stops, the rows are stale and their cats settle and sleep.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hub = self.hub.read(cx);
        let stopped = hub.stopped;
        let Some(snap) = &hub.snapshot else {
            return;
        };
        let live: Vec<(String, SessionStatus, f32)> = snap
            .sessions
            .iter()
            .filter(|s| s.status.is_live())
            .map(|s| (s.session_id.clone(), s.status, s.output_rate))
            .collect();
        #[cfg(feature = "screenshot")]
        let indexing = snap.indexing;
        self.cats
            .retain(|id, _| live.iter().any(|(live_id, ..)| live_id == id));
        for (id, status, rate) in &live {
            self.cats.entry(id.clone()).or_default().set_input(
                (!stopped).then_some(*status),
                if stopped { 0.0 } else { *rate },
            );
        }
        if live.is_empty() {
            self.empty_cat.set_input(None, 0.0);
        }
        #[cfg(feature = "screenshot")]
        if !indexing && let Some(dir) = self.screenshot_dir.take() {
            cx.spawn_in(window, async move |this, cx| {
                if let Err(e) = screenshot(&this, &dir, cx).await {
                    tracing::error!(error = %format!("{e:#}"), "screenshot failed");
                }
                cx.update(|_, cx| cx.quit()).ok();
            })
            .detach();
        }
        #[cfg(not(feature = "screenshot"))]
        let _ = window;
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

    /// One timer for all row cats, set to the soonest frame change. No timer while
    /// nothing will change, the window is inactive or hidden, or no cat is shown.
    fn schedule_animation(&mut self, cx: &mut Context<Self>) {
        self.animation = None;
        if !(self.active && self.visible) {
            return;
        }
        let motion = self.hub.read(cx).motion;
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
                let motion = this.hub.read(cx).motion;
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

// ------------------------------------------------------------------ layout

/// 4-based spacing scale. Major groups are `GROUP` apart, items in a group `S`..`M`.
mod sp {
    pub const XS: f32 = 4.;
    pub const S: f32 = 8.;
    pub const M: f32 = 12.;
    pub const L: f32 = 16.;
    pub const XL: f32 = 24.;
    pub const GROUP: f32 = 32.;
    pub const XXL: f32 = 48.;
}

/// Type scale in px. Nothing is larger than the screen title.
mod ty {
    pub const LABEL: f32 = 12.;
    pub const BODY: f32 = 13.;
    pub const ROW_TITLE: f32 = 15.;
    pub const TITLE: f32 = 20.;
}

const TITLEBAR_H: f32 = 38.;
const SIDEBAR_W: f32 = 176.;
/// Horizontal padding of a screen's content.
const CONTENT_PAD: f32 = sp::XL;
const AGENT_ROW_H: f32 = 72.;
const CHART_H: f32 = 96.;
const DOT: f32 = 7.;

/// Column title, width (0 takes the remaining space) and right alignment.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Column {
    title: &'static str,
    width: f32,
    numeric: bool,
}

const fn col(title: &'static str, width: f32, numeric: bool) -> Column {
    Column {
        title,
        width,
        numeric,
    }
}

const ROW_CAT_SCALE: f32 = 3.;

/// Empty sprite columns left of the cat in every stationary frame, read from
/// the sprite data: room the sprint's speed lines and dust use.
static CAT_LEFT_PAD: LazyLock<usize> = LazyLock::new(|| {
    [
        CatState::Sleeping,
        CatState::Sitting,
        CatState::LookingAround,
        CatState::Running,
    ]
    .into_iter()
    .flat_map(frames)
    .map(|f| {
        (0..f.width())
            .find(|&x| (0..f.height()).any(|y| f.ink(x, y) != Ink::Clear))
            .unwrap_or(f.width())
    })
    .min()
    .unwrap_or(0)
});

/// How far a row cat is shifted left so a stationary cat starts at the content edge.
fn cat_overhang() -> f32 {
    *CAT_LEFT_PAD as f32 * ROW_CAT_SCALE
}

/// The project column of the agent rows never gets narrower than this.
const PROJECT_MIN: f32 = 180.;

/// Which optional parts of the agent rows fit the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AgentColumns {
    cpu: bool,
    /// Second lines of Runtime and Tokens.
    detail: bool,
}

impl AgentColumns {
    /// Richest first; each step gives the project column more room.
    const STEPS: [AgentColumns; 3] = [
        AgentColumns {
            cpu: true,
            detail: true,
        },
        AgentColumns {
            cpu: false,
            detail: true,
        },
        AgentColumns {
            cpu: false,
            detail: false,
        },
    ];

    /// Drops the CPU / RAM column first, then the second lines, until the
    /// project column gets at least `PROJECT_MIN`.
    fn for_window(window_width: f32) -> Self {
        let available = window_width - SIDEBAR_W - 2. * CONTENT_PAD;
        Self::STEPS
            .into_iter()
            .find(|c| available - c.fixed_width() >= PROJECT_MIN)
            .unwrap_or(Self::STEPS[2])
    }

    fn columns(self) -> Vec<Column> {
        // Cat sprites are 32x16, drawn at 3x; the empty columns left of a
        // stationary cat hang into the page gutter.
        let mut c = vec![
            col("", 32. * ROW_CAT_SCALE - cat_overhang(), false),
            col("Project", 0., false),
            col("Status", 112., false),
            col("Runtime", if self.detail { 124. } else { 64. }, true),
            col("Tokens", if self.detail { 76. } else { 56. }, true),
            col("Est. cost", 76., true),
        ];
        if self.cpu {
            c.push(col("CPU / RAM", 64., true));
        }
        c
    }

    /// Width of everything but the project column, gaps included.
    fn fixed_width(self) -> f32 {
        let c = self.columns();
        c.iter().map(|c| c.width).sum::<f32>() + (c.len() - 1) as f32 * sp::M
    }
}

const SESSION_COLUMNS: [Column; 8] = [
    col("Status", 92., false),
    col("Project", 0., false),
    col("Model", 128., false),
    col("Started", 92., true),
    col("Duration", 68., true),
    col("Active (est.)", 80., true),
    col("Tokens", 56., true),
    col("Est. cost", 72., true),
];
const SESSION_MIN_WIDTH: f32 = 880.;

/// Peak of the chart's y axis: the smallest 1, 2 or 5 times a power of ten
/// at or above the history's peak. None when there is nothing to scale.
fn axis_max(history: &[f32]) -> Option<f32> {
    // Any non-finite sample leaves a flat baseline with no peak label.
    if history.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let peak = history.iter().copied().fold(0., f32::max);
    if peak <= 0. {
        return None;
    }
    let base = 10f32.powf(peak.log10().floor());
    [1., 2., 5., 10.]
        .into_iter()
        .map(|m| m * base)
        // Relative tolerance: log10 and powf are not exact.
        .find(|&v| v >= peak * (1. - 1e-6))
}

/// As many decimals as the tidy maximum needs: 0.05, 0.2, 5, 100.
fn axis_label(v: f32) -> String {
    // The small offset absorbs log10's error at exact powers of ten.
    let decimals = (-(v.log10() + 1e-4).floor()).max(0.) as usize;
    format!("{v:.decimals$}")
}

// ------------------------------------------------------------------ render

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let nav = [
            self.nav("Overview", Screen::Overview, t, cx),
            self.nav("Sessions", Screen::Sessions, t, cx),
        ];
        let columns = AgentColumns::for_window(f32::from(window.viewport_size().width));
        let hub = self.hub.read(cx);
        let content = match &hub.snapshot {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(t.muted))
                .child(if hub.stopped {
                    STOPPED
                } else {
                    "Reading agent data…"
                })
                .into_any_element(),
            Some(s) => {
                let notes = notes(s, hub.stopped, t);
                match self.screen {
                    Screen::Overview => self.overview(s, notes, columns, t).into_any_element(),
                    Screen::Sessions => sessions(s, notes, t).into_any_element(),
                }
            }
        };
        let today = hub.snapshot.as_ref().map(|s| today(s, t));
        div()
            .size_full()
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .flex()
            .flex_col()
            .bg(rgb(t.window))
            .text_color(rgb(t.text))
            .text_size(px(ty::BODY))
            .child(
                div()
                    .h(px(TITLEBAR_H))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(t.chrome))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("AgentPulse")),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(SIDEBAR_W))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .bg(rgb(t.chrome))
                            .border_r_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .flex_1()
                                    .p(px(sp::S))
                                    .flex()
                                    .flex_col()
                                    .gap(px(sp::XS))
                                    .children(nav),
                            )
                            .children(today),
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
    ) -> gpui::Stateful<Div> {
        div()
            .id(label)
            .px(px(sp::S))
            .py(px(sp::XS))
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

    fn overview(
        &self,
        s: &Snapshot,
        notes: Vec<Div>,
        columns: AgentColumns,
        t: Theme,
    ) -> impl IntoElement {
        let live: Vec<&AgentSession> = s.sessions.iter().filter(|x| x.status.is_live()).collect();
        let agents = if live.is_empty() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(sp::M))
                .py(px(sp::XXL))
                .child(cat(
                    self.empty_cat.sprite(),
                    palette(Provider::ClaudeCode),
                    4.,
                ))
                .child(
                    div()
                        .text_size(px(ty::ROW_TITLE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("No agents running"),
                )
                .child(secondary(
                    "Start Claude Code in a terminal and it will appear here.",
                    t,
                ))
        } else {
            let cols = columns.columns();
            div().flex().flex_col().child(header(&cols, t)).children(
                live.iter()
                    .map(|x| self.agent_row(x, s.taken_at_ms, columns, &cols, t)),
            )
        };
        div()
            .id("overview")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(CONTENT_PAD))
            .py(px(sp::XL))
            .flex()
            .flex_col()
            .child(heading("Overview"))
            .children(notes.into_iter().map(|n| n.mt(px(sp::M))))
            .child(status_line(s, live.len(), t).mt(px(sp::M)))
            .child(agents.mt(px(sp::GROUP)))
            .child(rate(s, t).mt(px(sp::GROUP)))
    }

    fn agent_row(
        &self,
        s: &AgentSession,
        now_ms: i64,
        columns: AgentColumns,
        cols: &[Column],
        t: Theme,
    ) -> Div {
        let w = |i: usize| cols[i].width;
        let resources = s.resources.value();
        let sprite = self
            .cats
            .get(&s.session_id)
            .map_or(self.empty_cat.sprite(), |c| c.sprite());
        let detail = |text: String| columns.detail.then_some(Some(text));
        div()
            .h(px(AGENT_ROW_H))
            .flex()
            .items_center()
            .gap(px(sp::M))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .w(px(w(0)))
                    .flex_none()
                    .child(div().ml(px(-cat_overhang())).child(cat(
                        sprite,
                        palette(s.provider),
                        ROW_CAT_SCALE,
                    ))),
            )
            .child(project_cell(s, t))
            .child(
                div()
                    .w(px(w(2)))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .child(status_label(s.status, t))
                    .child(match &s.model {
                        Some(m) => secondary(m, t).truncate(),
                        None => secondary(DASH, t),
                    }),
            )
            .child(figures(
                w(3),
                s.wall_ms(now_ms).map(format::duration),
                detail(format!("{} active (est.)", format::duration(s.active_ms))),
                t,
            ))
            .child(figures(
                w(4),
                Some(format::tokens(s.tokens.total())),
                detail(format!("{:.1} tok/s", s.output_rate)),
                t,
            ))
            .child(figures(w(5), format::approx_cost(s.cost_usd), None, t))
            .when(columns.cpu, |row| {
                row.child(figures(
                    w(6),
                    resources
                        .and_then(|r| r.cpu_percent)
                        .map(|c| format!("{c:.1}%")),
                    Some(resources.map(|r| format::bytes(r.rss_bytes))),
                    t,
                ))
            })
    }
}

/// Indexing note and error banner, shown between the title and the content.
fn notes(s: &Snapshot, stopped: bool, t: Theme) -> Vec<Div> {
    let errors: Vec<&str> = s
        .errors
        .iter()
        .map(String::as_str)
        .chain(stopped.then_some(STOPPED))
        .collect();
    let mut notes = Vec::new();
    if s.indexing && !stopped {
        notes.push(secondary("Indexing history…", t));
    }
    if !errors.is_empty() {
        notes.push(
            div()
                .px(px(sp::M))
                .py(px(sp::S))
                .rounded_md()
                .bg(rgb(t.banner))
                .border_1()
                .border_color(rgb(t.banner_border))
                .text_size(px(ty::LABEL))
                .flex()
                .flex_col()
                .gap(px(sp::XS))
                .children(errors.into_iter().map(|e| div().child(e.to_string()))),
        );
    }
    notes
}

/// "3 agents · 1 active · 0 waiting · 2 idle" and the machine figures, as text.
fn status_line(s: &Snapshot, live: usize, t: Theme) -> Div {
    let count = |st| s.sessions.iter().filter(|x| x.status == st).count();
    let fig = |v: String| {
        div()
            .font_weight(FontWeight::MEDIUM)
            .font_features(tabular())
            .child(v)
    };
    let word = |v: &'static str| div().text_color(rgb(t.muted)).child(v);
    let dot = || word("·");
    let cpu = s.agents_cpu_percent.map(|c| format!("{c:.1}%"));
    // With no live agent the counts are all zero; the empty state says so.
    let counts = (live > 0).then(|| {
        div()
            .flex()
            .gap(px(sp::XS))
            .child(fig(live.to_string()))
            .child(word(if live == 1 { "agent" } else { "agents" }))
            .child(dot())
            .child(fig(count(SessionStatus::Active).to_string()))
            .child(word("active"))
            .child(dot())
            .child(fig(count(SessionStatus::Waiting).to_string()))
            .child(word("waiting"))
            .child(dot())
            .child(fig(count(SessionStatus::Idle).to_string()))
            .child(word("idle"))
    });
    div()
        .flex()
        .flex_wrap()
        .justify_between()
        .when(counts.is_none(), |d| d.justify_end())
        .gap(px(sp::S))
        .children(counts)
        .child(
            div()
                .flex()
                .gap(px(sp::XS))
                .child(word("CPU"))
                .child(fig(cpu.clone().unwrap_or(DASH.into())))
                .when(cpu.is_some(), |d| d.child(word("of one core")))
                .child(dot())
                .child(word("RAM"))
                .child(fig(s.agents_rss_bytes.map_or(DASH.into(), format::bytes)))
                .child(dot())
                .child(
                    div()
                        .id("gpu")
                        .text_color(rgb(t.muted))
                        .child("GPU unavailable")
                        .tooltip(move |_, cx| {
                            cx.new(|_| Tooltip {
                                text: "Not collected in this build",
                                t,
                            })
                            .into()
                        }),
                ),
        )
}

struct Tooltip {
    text: &'static str,
    t: Theme,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(sp::S))
            .py(px(sp::XS))
            .rounded_md()
            .bg(rgb(self.t.surface))
            .border_1()
            .border_color(rgb(self.t.border))
            .text_size(px(ty::LABEL))
            .text_color(rgb(self.t.text))
            .child(self.text)
    }
}

/// The output rate: current value and a chart of the last two minutes.
fn rate(s: &Snapshot, t: Theme) -> Div {
    let max = axis_max(&s.rate_history);
    let axis_w = px(40.);
    div()
        .flex()
        .flex_col()
        .child(section_label("Output rate", t).mb(px(sp::S)))
        .child(
            div()
                .flex()
                .gap(px(sp::XL))
                .child(
                    div()
                        .w(px(144.))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .font_features(tabular())
                                .child(format!("{:.1}", s.output_rate)),
                        )
                        .child(secondary("tok/s, 30 s average", t)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(sp::XS))
                        .child(
                            div()
                                .flex()
                                .gap(px(sp::S))
                                .child(
                                    // One label line taller than the chart and raised
                                    // half a line, so each label centres on its line.
                                    div()
                                        .w(axis_w)
                                        .h(px(CHART_H + sp::L))
                                        .mt(px(-sp::S))
                                        .flex_none()
                                        .flex()
                                        .flex_col()
                                        .justify_between()
                                        .items_end()
                                        .text_size(px(ty::LABEL))
                                        .line_height(px(sp::L))
                                        .text_color(rgb(t.muted))
                                        .font_features(tabular())
                                        .child(max.map(axis_label).unwrap_or_default())
                                        .child("0"),
                                )
                                .child(chart(s.rate_history.clone(), max, t)),
                        )
                        .child(
                            div()
                                .ml(axis_w + px(sp::S))
                                .flex()
                                .justify_between()
                                .text_size(px(ty::LABEL))
                                .text_color(rgb(t.muted))
                                .child("2 min ago")
                                .child("now"),
                        ),
                ),
        )
}

/// Baseline, a hairline at the axis maximum and the rate path scaled to it.
fn chart(history: Vec<f32>, max: Option<f32>, t: Theme) -> impl IntoElement {
    let (line, base, grid) = (rgb(t.accent), rgb(t.muted), rgb(t.border));
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let hairline = |y, color| {
                fill(
                    Bounds::new(point(b.left(), y), size(b.size.width, px(1.))),
                    color,
                )
            };
            window.paint_quad(hairline(b.bottom() - px(1.), base));
            let Some(max) = max else { return };
            window.paint_quad(hairline(b.top(), grid));
            if history.len() < 2 {
                return;
            }
            // Fixed spacing, so the line grows in from the right as samples arrive.
            let step = b.size.width * (1.0 / (HISTORY_LEN - 1) as f32);
            let x0 = b.right() - step * (history.len() - 1) as f32;
            let height = b.size.height - px(1.5);
            let mut path = PathBuilder::stroke(px(1.5));
            for (i, v) in history.iter().enumerate() {
                let p = point(
                    x0 + step * i as f32,
                    b.bottom() - px(1.) - height * (v / max),
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
    .h(px(CHART_H))
}

/// Pinned to the bottom of the sidebar on both screens.
fn today(s: &Snapshot, t: Theme) -> Div {
    let tk = s.today_tokens;
    let line = |label: &'static str, value: u64| {
        div()
            .flex()
            .justify_between()
            .text_size(px(ty::LABEL))
            .child(div().text_color(rgb(t.muted)).child(label))
            .child(div().font_features(tabular()).child(format::tokens(value)))
    };
    let (cost, note) = match format::approx_cost(s.today_cost_usd) {
        Some(c) => (c, "estimate, API list prices"),
        None => (DASH.into(), "no price for today's models"),
    };
    div()
        .flex_none()
        .p(px(sp::L))
        .border_t_1()
        .border_color(rgb(t.border))
        .flex()
        .flex_col()
        .gap(px(sp::XS))
        .child(section_label("Today", t).mb(px(sp::XS)))
        .child(
            div().flex().justify_between().child("Tokens").child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .font_features(tabular())
                    .child(format::tokens(tk.total())),
            ),
        )
        .child(line("Input", tk.input))
        .child(line("Output", tk.output))
        .child(line("Cache write", tk.cache_write_5m + tk.cache_write_1h))
        .child(line("Cache read", tk.cache_read))
        .child(
            div()
                .mt(px(sp::M))
                .font_weight(FontWeight::MEDIUM)
                .font_features(tabular())
                .child(cost),
        )
        .child(secondary(note, t))
        .children((s.today_unpriced_events > 0).then(|| {
            secondary(
                &format!("{} responses unpriced", s.today_unpriced_events),
                t,
            )
        }))
}

// ---------------------------------------------------------------- sessions

fn sessions(s: &Snapshot, notes: Vec<Div>, t: Theme) -> impl IntoElement {
    let w = |i: usize| SESSION_COLUMNS[i].width;
    let n = s.sessions.len();
    div()
        .flex_1()
        .min_h_0()
        .px(px(CONTENT_PAD))
        .pt(px(sp::XL))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(sp::M))
                .child(heading("Sessions"))
                .child(secondary(
                    &format!(
                        "{n} {} in the last 24 hours or still running",
                        if n == 1 { "session" } else { "sessions" }
                    ),
                    t,
                )),
        )
        .children(notes.into_iter().map(|n| n.mt(px(sp::M))))
        .child(
            div()
                .id("sessions-scroll")
                .mt(px(sp::GROUP))
                .flex_1()
                .min_h_0()
                .overflow_scroll()
                .child(
                    div()
                        .min_w(px(SESSION_MIN_WIDTH))
                        .pb(px(sp::XL))
                        .flex()
                        .flex_col()
                        .child(header(&SESSION_COLUMNS, t))
                        .children(s.sessions.iter().map(|x| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(sp::M))
                                .py(px(sp::M))
                                .border_b_1()
                                .border_color(rgb(t.border))
                                .child(status_label(x.status, t).w(px(w(0))).flex_none())
                                .child(project_cell(x, t))
                                .child(match &x.model {
                                    Some(m) => {
                                        div().w(px(w(2))).flex_none().truncate().child(m.clone())
                                    }
                                    None => secondary(DASH, t).w(px(w(2))).flex_none(),
                                })
                                .child(figures(
                                    w(3),
                                    x.started_at_ms
                                        .map(|ms| format::start_time(ms, s.taken_at_ms)),
                                    None,
                                    t,
                                ))
                                .child(figures(
                                    w(4),
                                    x.wall_ms(s.taken_at_ms).map(format::duration),
                                    None,
                                    t,
                                ))
                                .child(figures(w(5), Some(format::duration(x.active_ms)), None, t))
                                .child(figures(
                                    w(6),
                                    Some(format::tokens(x.tokens.total())),
                                    None,
                                    t,
                                ))
                                .child(figures(w(7), format::approx_cost(x.cost_usd), None, t))
                        })),
                ),
        )
}

// ------------------------------------------------------------------ pieces

fn heading(text: &'static str) -> Div {
    div()
        .text_size(px(ty::TITLE))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

fn section_label(text: &'static str, t: Theme) -> Div {
    secondary(text, t).font_weight(FontWeight::MEDIUM)
}

fn secondary(text: &str, t: Theme) -> Div {
    div()
        .text_size(px(ty::LABEL))
        .text_color(rgb(t.muted))
        .child(text.to_string())
}

/// The quiet header row of a table.
fn header(columns: &[Column], t: Theme) -> Div {
    let mut d = div()
        .flex()
        .gap(px(sp::M))
        .py(px(sp::S))
        .border_b_1()
        .border_color(rgb(t.border))
        .text_size(px(ty::LABEL))
        .text_color(rgb(t.muted));
    for c in columns {
        let cell = if c.width == 0. {
            div().flex_1().min_w_0()
        } else {
            div().w(px(c.width)).flex_none()
        };
        d = d.child(cell.when(c.numeric, |c| c.text_right()).child(c.title));
    }
    d
}

/// Project name over its full path; takes all spare width.
fn project_cell(s: &AgentSession, t: Theme) -> Div {
    let path = s.project_path.as_deref();
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .truncate()
                .text_size(px(ty::ROW_TITLE))
                .font_weight(FontWeight::SEMIBOLD)
                .child(format::project_name(path).to_string()),
        )
        .child(secondary(path.unwrap_or("Project path unknown"), t).truncate())
}

/// Dot and label: the label carries the meaning, not the colour alone.
fn status_label(s: SessionStatus, t: Theme) -> Div {
    let color = rgb(t.status(s));
    div()
        .flex()
        .items_center()
        .gap(px(sp::S))
        .text_color(color)
        .font_weight(FontWeight::MEDIUM)
        .child(div().size(px(DOT)).flex_none().rounded_full().bg(color))
        .child(s.label())
}

/// A right-aligned figure, with an optional secondary line below; None shows the dash.
fn figures(width: f32, top: Option<String>, below: Option<Option<String>>, t: Theme) -> Div {
    let value = |text: Option<String>, size: f32, color: u32| {
        let d = div()
            .text_size(px(size))
            .whitespace_nowrap()
            .font_features(tabular());
        match text {
            Some(text) => d.text_color(rgb(color)).child(text),
            None => d.text_color(rgb(t.muted)).child(DASH),
        }
    };
    div()
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .items_end()
        .overflow_hidden()
        .child(value(top, ty::BODY, t.text))
        .children(below.map(|b| value(b, ty::LABEL, t.muted)))
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
            // The chart line is a graphical object; it sits on the window background.
            assert!(contrast(t.accent, t.window) >= 3.0, "chart line");
        }
    }

    #[test]
    fn axis_max_rounds_up_to_a_tidy_number() {
        assert_eq!(axis_max(&[]), None);
        assert_eq!(axis_max(&[0.]), None);
        assert_eq!(axis_max(&[1., f32::NAN]), None);
        assert_eq!(axis_max(&[f32::INFINITY]), None);
        assert_eq!(axis_max(&[28.8, 3.]), Some(50.));
        assert_eq!(axis_max(&[65.8]), Some(100.));
        assert_eq!(axis_max(&[100.]), Some(100.));
        assert_eq!(axis_max(&[110.]), Some(200.));
        assert_eq!(axis_max(&[1.0]), Some(1.));
        assert_eq!(axis_max(&[0.3]), Some(0.5));
        // The label always shows the value the top line stands for.
        let label = |peak: f32| axis_max(&[peak]).map(axis_label);
        for (peak, shown) in [
            (0.004, "0.005"),
            (0.015, "0.02"),
            (0.033, "0.05"),
            (0.1, "0.1"),
            (0.2, "0.2"),
            (0.4, "0.5"),
            (1., "1"),
            (4.2, "5"),
            (28.8, "50"),
            (100., "100"),
            (110., "200"),
        ] {
            assert_eq!(label(peak).as_deref(), Some(shown), "peak {peak}");
        }
    }

    #[test]
    fn agent_columns_drop_cpu_then_detail_and_keep_the_project_wide() {
        let full = AgentColumns {
            cpu: true,
            detail: true,
        };
        let no_cpu = AgentColumns {
            cpu: false,
            detail: true,
        };
        let bare = AgentColumns {
            cpu: false,
            detail: false,
        };
        assert_eq!(AgentColumns::for_window(1120.), full);
        assert_eq!(AgentColumns::for_window(980.), no_cpu);
        assert_eq!(AgentColumns::for_window(880.), bare);
        // From the minimum window width (880) to well past the default, the
        // chosen columns plus the project's minimum fit the content width.
        for w in (880..=2400).step_by(4).map(|w| w as f32) {
            let available = w - SIDEBAR_W - 2. * CONTENT_PAD;
            let c = AgentColumns::for_window(w);
            assert!(c.fixed_width() + PROJECT_MIN <= available, "{w} pt: {c:?}");
        }
        assert_eq!(full.columns().len(), 7);
        assert_eq!(bare.columns().len(), 6);
        assert!(bare.fixed_width() < no_cpu.fixed_width());
        // The cat's overhang fits in the gutter, so sprint lines are not clipped.
        assert!(cat_overhang() > 0. && cat_overhang() <= CONTENT_PAD);
    }

    #[test]
    fn contrast_formula() {
        assert!((contrast(0x000000, 0xFFFFFF) - 21.0).abs() < 1e-9);
        assert!((contrast(0x777777, 0xFFFFFF) - 4.48).abs() < 0.01);
    }
}
