//! The window: Overview, Sessions (with a session's detail) and Usage screens
//! over the hub's latest snapshot, and the Cats settings screen. Closing it
//! does not quit; the menu bar item and the Dock icon reopen it.

mod charts;

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use agentpulse_animation::{CatAnimator, CatState, Ink, Look, Sprite, frames};
use agentpulse_core::{AgentSession, Provider, SessionStatus};
use agentpulse_platform_macos::{MainThreadMarker, MenuAction, StatusItem};
#[cfg(feature = "screenshot")]
use anyhow::Context as _;
use gpui::{
    App, Bounds, Context, Div, Entity, FocusHandle, FontFeatures, FontWeight, Global, KeyBinding,
    Menu, MenuItem, PathBuilder, QuitMode, Rgba, Role, SharedString, Task, TitlebarOptions,
    WeakEntity, Window, WindowAppearance, WindowBounds, WindowOptions, actions, canvas, div, fill,
    point, prelude::*, px, rgb, size, transparent_black,
};

use crate::collector::{HISTORY_LEN, Key, Snapshot};
use crate::format::{self, DASH};
use crate::hub::Hub;
use crate::settings::CatSettings;

mod cats;

actions!(agentpulse, [Quit, CloseWindow, FocusNext, FocusPrev]);

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
    /// Asks the collector for one session's requests (None: no session).
    details: Sender<Option<Key>>,
    cats: CatSettings,
}

impl Global for Shared {}

pub fn run(
    rx: async_channel::Receiver<Snapshot>,
    details: Sender<Option<Key>>,
    options: Options,
    cats: CatSettings,
) {
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
            KeyBinding::new("tab", FocusNext, None),
            KeyBinding::new("shift-tab", FocusPrev, None),
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
            details,
            cats,
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
    let cats = cx.global::<Shared>().cats.clone();
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
        |window, cx| cx.new(|cx| AppView::new(hub, cats, options, window, cx)),
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
    Usage,
    Cats,
}

const STOPPED: &str = "Data collection stopped. Restart AgentPulse.";

/// A row cat's session. Keyed by provider too: session ids are unique per agent only.
type CatKey = (Provider, String);

/// View state only; the data and motion settings live in the [`Hub`].
struct AppView {
    hub: Entity<Hub>,
    screen: Screen,
    /// The session whose detail replaces the Sessions table.
    detail: Option<Key>,
    /// Scroll position of the Usage and session detail pages.
    page_scroll: gpui::ScrollHandle,
    dark: bool,
    forced_dark: Option<bool>,
    cats: HashMap<CatKey, CatAnimator>,
    /// Shown when no agent is live.
    empty_cat: CatAnimator,
    cat_settings: CatSettings,
    /// Whose cat the Cats screen edits.
    cats_agent: Provider,
    /// The Cats screen's preview: sitting and running.
    preview: [CatAnimator; 2],
    /// Holds keyboard focus when no control does, so Tab reaches the controls.
    focus: FocusHandle,
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
        cat_settings: CatSettings,
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
        let preview = [SessionStatus::Idle, SessionStatus::Active].map(|status| {
            let mut a = CatAnimator::new();
            a.set_input(Some(status), 0.0, None);
            a
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut view = Self {
            hub,
            screen: Screen::Overview,
            detail: None,
            page_scroll: gpui::ScrollHandle::new(),
            dark: options.appearance.unwrap_or(is_dark(window.appearance())),
            forced_dark: options.appearance,
            cats: HashMap::new(),
            empty_cat: CatAnimator::new(),
            cat_settings,
            cats_agent: Provider::ClaudeCode,
            preview,
            focus,
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
        let live: Vec<(CatKey, SessionStatus, f32, Option<f32>)> = snap
            .sessions
            .iter()
            .filter(|s| s.status.is_live())
            .map(|s| {
                let cpu = s.resources.value().and_then(|r| r.cpu_percent);
                (
                    (s.provider, s.session_id.clone()),
                    s.status,
                    s.output_rate,
                    cpu,
                )
            })
            .collect();
        #[cfg(feature = "screenshot")]
        let indexing = snap.indexing;
        self.cats
            .retain(|id, _| live.iter().any(|(live_id, ..)| live_id == id));
        for (id, status, rate, cpu) in &live {
            self.cats.entry(id.clone()).or_default().set_input(
                (!stopped).then_some(*status),
                if stopped { 0.0 } else { *rate },
                cpu.filter(|_| !stopped),
            );
        }
        if live.is_empty() {
            self.empty_cat.set_input(None, 0.0, None);
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
        if self.screen == Screen::Cats {
            return self.preview.iter().collect();
        }
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
                let all = this.cats.values_mut().chain([&mut this.empty_cat]);
                // The preview only moves while it is on screen.
                let preview = (this.screen == Screen::Cats).then_some(&mut this.preview);
                for cat in all.chain(preview.into_iter().flatten()) {
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

    /// Opens a session's detail, or closes it with None.
    fn show_detail(&mut self, key: Option<Key>, cx: &mut Context<Self>) {
        if key != self.detail {
            cx.global::<Shared>().details.send(key.clone()).ok();
            self.detail = key;
        }
        // Each page opens at its top.
        self.page_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
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
    this.update(cx, |this, cx| {
        this.screen = Screen::Usage;
        cx.notify();
    })?;
    cx.update(|window, cx| capture(window, cx, &dir.join("usage.png")))??;
    this.update(cx, |this, cx| {
        this.page_scroll.scroll_to_bottom();
        cx.notify();
    })?;
    cx.update(|window, cx| capture(window, cx, &dir.join("usage-end.png")))??;
    // The most expensive session listed, if any.
    let key = this.update(cx, |this, cx| {
        let key = this.hub.read(cx).snapshot.as_ref().and_then(|s| {
            s.sessions
                .iter()
                .max_by(|a, b| {
                    let usd = |x: &AgentSession| x.cost_usd.value().copied().unwrap_or(0.0);
                    usd(a).total_cmp(&usd(b))
                })
                .map(|x| (x.provider, x.session_id.clone()))
        });
        this.screen = Screen::Sessions;
        this.show_detail(key.clone(), cx);
        key
    })?;
    if key.is_some() {
        for _ in 0..50 {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let ready = this.update(cx, |this, cx| {
                let detail = this
                    .hub
                    .read(cx)
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.detail.clone());
                detail.is_some_and(|d| Some(&d.key) == key.as_ref())
            })?;
            if ready {
                break;
            }
        }
        cx.update(|window, cx| capture(window, cx, &dir.join("session-detail.png")))??;
        this.update(cx, |this, cx| {
            this.page_scroll.scroll_to_bottom();
            cx.notify();
        })?;
        cx.update(|window, cx| capture(window, cx, &dir.join("session-detail-end.png")))??;
    }
    for (agent, file) in [
        (Provider::ClaudeCode, "cats.png"),
        (Provider::Codex, "cats-codex.png"),
    ] {
        this.update(cx, |this, cx| {
            this.screen = Screen::Cats;
            this.cats_agent = agent;
            this.schedule_animation(cx);
            cx.notify();
        })?;
        cx.update(|window, cx| capture(window, cx, &dir.join(file)))??;
    }
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
            col("Est. cost", if self.detail { 128. } else { 76. }, true),
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

const SESSION_COLUMNS: [Column; 9] = [
    col("Status", 92., false),
    col("Project", 0., false),
    col("Model", 116., false),
    col("Started", 92., true),
    col("Duration", 68., true),
    col("Active (est.)", 80., true),
    col("Tokens", 56., true),
    col("Est. cost", 72., true),
    col("Claude Code", 76., true),
];
/// The content width of the default 1120 pt window, so the table does not
/// scroll sideways there.
const SESSION_MIN_WIDTH: f32 = 896.;
/// Below the Sessions table: what the two cost columns are.
const COST_NOTE: &str = "Est. cost is AgentPulse's estimate at API list prices. Claude Code is the \
    session's own cost record, covering the Claude Code run that last wrote it and written when a \
    session ends or saves its state, so a running session often has none yet (—). +? means Claude Code had no price for a model it used. \
    Codex keeps no such record, so its sessions always show —.";

/// Peak of the chart's y axis: the smallest 1, 2, 3, 4, 5, 6 or 8 times a power of ten
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
    [1., 2., 3., 4., 5., 6., 8., 10.]
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
            self.nav("Usage", Screen::Usage, t, cx),
            self.nav("Cats", Screen::Cats, t, cx),
        ];
        let columns = AgentColumns::for_window(f32::from(window.viewport_size().width));
        let view = cx.weak_entity();
        let hub = self.hub.read(cx);
        let content = match (self.screen, &hub.snapshot) {
            // Needs no agent data.
            (Screen::Cats, _) => self.cats_screen(t, view.clone()).into_any_element(),
            (_, None) => div()
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
            (Screen::Overview, Some(s)) => self
                .overview(s, notes(s, hub.stopped, t), columns, t)
                .into_any_element(),
            (Screen::Sessions, Some(s)) => match &self.detail {
                Some(key) => charts::session_detail(
                    s,
                    key,
                    notes(s, hub.stopped, t),
                    t,
                    view,
                    &self.page_scroll,
                )
                .into_any_element(),
                None => sessions(s, notes(s, hub.stopped, t), t, view).into_any_element(),
            },
            (Screen::Usage, Some(s)) => {
                charts::usage(s, notes(s, hub.stopped, t), t, &self.page_scroll).into_any_element()
            }
        };
        let today = hub.snapshot.as_ref().map(|s| today(s, t));
        div()
            .size_full()
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .track_focus(&self.focus)
            .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
            .on_action(|_: &FocusPrev, window, cx| window.focus_prev(cx))
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
            .role(Role::Tab)
            .aria_label(label)
            .aria_selected(self.screen == screen)
            // One point less padding makes room for the focus ring's border,
            // so the sidebar lays out exactly as it did without one.
            .px(px(sp::S - 1.))
            .py(px(sp::XS - 1.))
            .rounded_md()
            .with_focus_ring(t)
            .when(self.screen == screen, |d| {
                d.bg(rgb(t.selected)).font_weight(FontWeight::MEDIUM)
            })
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.screen = screen;
                this.show_detail(None, cx);
                this.schedule_animation(cx);
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
                    self.cat_settings.looks().get(Provider::ClaudeCode),
                    4.,
                ))
                .child(
                    div()
                        .text_size(px(ty::ROW_TITLE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("No agents running"),
                )
                .child(secondary(
                    "Start Claude Code or Codex and it will appear here.",
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
            .get(&(s.provider, s.session_id.clone()))
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
                        self.cat_settings.looks().get(s.provider),
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
            .child(figures(
                w(5),
                format::approx_cost(s.cost_usd),
                detail(recorded_line(
                    s.provider,
                    format::recorded_cost(s.recorded_cost),
                )),
                t,
            ))
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
    let gpu = s.system_gpu_percent.value().map(|g| format!("{g}%"));
    let has_gpu = gpu.is_some();
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
                        .flex()
                        .gap(px(sp::XS))
                        .child(word("GPU"))
                        .child(gpu.clone().map_or(word("unavailable"), fig))
                        .when(gpu.is_some(), |d| d.child(word("whole Mac")))
                        .tooltip(move |_, cx| {
                            cx.new(|_| Tooltip {
                                text: if has_gpu {
                                    "GPU use of the whole Mac, not of any one agent"
                                } else {
                                    "Not readable on this Mac"
                                }
                                .into(),
                                t,
                            })
                            .into()
                        }),
                ),
        )
}

struct Tooltip {
    text: SharedString,
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
            .child(self.text.clone())
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

fn sessions(
    s: &Snapshot,
    notes: Vec<Div>,
    t: Theme,
    view: WeakEntity<AppView>,
) -> impl IntoElement {
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
                        .children(s.sessions.iter().enumerate().map(|(i, x)| {
                            let key = (x.provider, x.session_id.clone());
                            let view = view.clone();
                            div()
                                .id(("session", i))
                                .cursor_pointer()
                                .hover(|d| d.bg(rgb(t.surface)))
                                .on_click(move |_, _, cx| {
                                    view.update(cx, |v, cx| v.show_detail(Some(key.clone()), cx))
                                        .ok();
                                })
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
                                .child(figures(
                                    w(8),
                                    format::recorded_cost(x.recorded_cost),
                                    None,
                                    t,
                                ))
                        }))
                        .child(secondary(COST_NOTE, t).mt(px(sp::M)))
                        .child(secondary("Click a session for its requests.", t).mt(px(sp::S))),
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

/// Project name over the agent and the full path; takes all spare width. The
/// agent is named in text, never told by the cat's colour alone.
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
        .child(
            secondary(
                &format!(
                    "{} · {}",
                    s.provider.label(),
                    path.map_or("project path unknown".into(), format::display_path)
                ),
                t,
            )
            .truncate(),
        )
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

/// Second line under the estimate: Claude Code's own cost record, blank for
/// other agents. The Overview cost column is 128 pt, so a long figure
/// ("$123.45 +?") takes the short label rather than lose its start.
fn recorded_line(provider: Provider, value: Option<String>) -> String {
    if provider != Provider::ClaudeCode {
        return String::new();
    }
    let value = value.unwrap_or(DASH.into());
    let label = if value.chars().count() > 8 {
        "Claude"
    } else {
        "Claude Code"
    };
    format!("{label} {value}")
}

/// A keyboard focus ring that takes no extra room: the border is always there,
/// coloured only while focused from the keyboard.
trait FocusRing: StatefulInteractiveElement + Styled {
    fn with_focus_ring(self, t: Theme) -> Self {
        self.tab_index(0)
            .border_1()
            .border_color(transparent_black())
            .focus_visible(|s| s.border_color(rgb(t.text)))
    }
}

impl<E: StatefulInteractiveElement + Styled> FocusRing for E {}

fn rgba([r, g, b, a]: [u8; 4]) -> Rgba {
    Rgba {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a: a as f32 / 255.,
    }
}

/// One run of equal colour in a sprite row: x, y, length, colour.
type Run = (usize, usize, usize, Rgba);

/// A look's sprite as runs, memoised: a pattern costs a pass over every pixel,
/// and the Cats screen repaints all its swatches on each preview frame.
fn runs(sprite: &'static Sprite, look: Look) -> Arc<[Run]> {
    type Key = (&'static str, Option<&'static str>, usize);
    static CACHE: LazyLock<Mutex<HashMap<Key, Arc<[Run]>>>> = LazyLock::new(Default::default);
    let key = (
        look.coat.id,
        look.eye.map(|e| e.id),
        std::ptr::from_ref(sprite) as usize,
    );
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    // ponytail: dropped wholesale when full; an LRU if anyone browses that many looks.
    if cache.len() >= 4096 {
        cache.clear();
    }
    cache
        .entry(key)
        .or_insert_with(|| {
            let w = sprite.width();
            let pixels = look.paint(sprite);
            let mut runs = Vec::new();
            for (y, row) in pixels.chunks(w).enumerate() {
                let mut x = 0;
                while x < w {
                    let (start, color) = (x, row[x]);
                    while x < w && row[x] == color {
                        x += 1;
                    }
                    if let Some(c) = color {
                        runs.push((start, y, x - start, rgba(c)));
                    }
                }
            }
            runs.into()
        })
        .clone()
}

/// Paints a sprite with square pixels at an integer scale, one quad per run of
/// equal colour in a row.
fn cat(sprite: &'static Sprite, look: Look, scale: f32) -> impl IntoElement {
    let (w, h) = (sprite.width(), sprite.height());
    let runs = runs(sprite, look);
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            // Whole-point origin keeps every sprite pixel on device-pixel edges.
            let origin = point(b.origin.x.round(), b.origin.y.round());
            for &(x, y, len, color) in runs.iter() {
                window.paint_quad(fill(
                    Bounds::new(
                        origin + point(px(x as f32 * scale), px(y as f32 * scale)),
                        size(px(len as f32 * scale), px(scale)),
                    ),
                    color,
                ));
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
    fn runs_rebuild_the_painted_pixels_and_are_memoised() {
        use agentpulse_animation::{find_coat, find_eye};
        let sprite = &frames(CatState::Sitting)[0];
        let look = Look {
            coat: find_coat("calico").unwrap(),
            eye: find_eye("blue"),
        };
        let first = runs(sprite, look);
        assert!(Arc::ptr_eq(&first, &runs(sprite, look)));
        let own_eyes = runs(sprite, Look { eye: None, ..look });
        assert!(!Arc::ptr_eq(&first, &own_eyes));
        let w = sprite.width();
        let mut rebuilt = vec![None; w * sprite.height()];
        for &(x, y, len, color) in first.iter() {
            rebuilt[y * w + x..][..len].fill(Some(color));
        }
        let painted: Vec<_> = look
            .paint(sprite)
            .into_iter()
            .map(|p| p.map(rgba))
            .collect();
        assert!(rebuilt == painted);
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
        assert_eq!(axis_max(&[28.8, 3.]), Some(30.));
        assert_eq!(axis_max(&[65.8]), Some(80.));
        assert_eq!(axis_max(&[100.]), Some(100.));
        assert_eq!(axis_max(&[110.]), Some(200.));
        assert_eq!(axis_max(&[1.0]), Some(1.));
        assert_eq!(axis_max(&[0.3]), Some(0.3));
        // The label always shows the value the top line stands for.
        let label = |peak: f32| axis_max(&[peak]).map(axis_label);
        for (peak, shown) in [
            (0.004, "0.004"),
            (0.015, "0.02"),
            (0.033, "0.04"),
            (0.1, "0.1"),
            (0.2, "0.2"),
            (0.4, "0.4"),
            (1., "1"),
            (4.2, "5"),
            (28.8, "30"),
            (100., "100"),
            (110., "200"),
            (203., "300"),
        ] {
            assert_eq!(label(peak).as_deref(), Some(shown), "peak {peak}");
        }
    }

    #[test]
    fn recorded_line_keeps_the_figure_whole() {
        let cc = |v: Option<&str>| recorded_line(Provider::ClaudeCode, v.map(Into::into));
        assert_eq!(cc(None), "Claude Code —");
        assert_eq!(cc(Some("$4.62")), "Claude Code $4.62");
        assert_eq!(cc(Some("$1234.50")), "Claude Code $1234.50");
        assert_eq!(cc(Some("$99.99 +?")), "Claude $99.99 +?");
        // Claude Code's own record means nothing for another agent.
        assert_eq!(recorded_line(Provider::Codex, None), "");
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
        // The Sessions table fits the default window without sideways scrolling,
        // and its project column keeps a usable width there.
        let fixed: f32 = SESSION_COLUMNS.iter().map(|c| c.width).sum::<f32>()
            + (SESSION_COLUMNS.len() - 1) as f32 * sp::M;
        const { assert!(SESSION_MIN_WIDTH <= 1120. - SIDEBAR_W - 2. * CONTENT_PAD) };
        assert!(
            SESSION_MIN_WIDTH - fixed >= 140.,
            "{}",
            SESSION_MIN_WIDTH - fixed
        );
        // The cat's overhang fits in the gutter, so sprint lines are not clipped.
        assert!(cat_overhang() > 0. && cat_overhang() <= CONTENT_PAD);
    }

    #[test]
    fn contrast_formula() {
        assert!((contrast(0x000000, 0xFFFFFF) - 21.0).abs() < 1e-9);
        assert!((contrast(0x777777, 0xFFFFFF) - 4.48).abs() < 0.01);
    }
}
