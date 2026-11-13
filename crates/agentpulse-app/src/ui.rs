//! The window: Overview, Sessions (with a session's detail) and Usage screens
//! over the hub's latest snapshot, and the Cats settings screen. Closing it
//! does not quit; the menu bar item and the Dock icon reopen it.

mod charts;

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use agentpulse_animation::{CatAnimator, CatState, Ink, Look, Sprite, frames};
use agentpulse_core::{AgentSession, Provider, SessionStatus};
use agentpulse_platform_macos::{MainThreadMarker, MenuAction, StatusItem};
#[cfg(feature = "screenshot")]
use anyhow::Context as _;
use gpui::{
    Animation, AnimationExt, App, Bounds, Context, Div, ElementId, Entity, FocusHandle,
    FontFeatures, FontWeight, Global, KeyBinding, Menu, MenuItem, PathBuilder, QuitMode, Rgba,
    Role, SharedString, Stateful, Task, TitlebarOptions, WeakEntity, Window, WindowAppearance,
    WindowBounds, WindowOptions, actions, canvas, div, fill, point, prelude::*, px, rgb, size,
    transparent_black,
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

/// The rounded design of the system font (SF Pro Rounded) in every weight,
/// named by CoreText as the plain one is `.AppleSystemUIFont`. Where it is
/// missing, GPUI falls back to the system font.
const FONT_FAMILY: &str = ".AppleSystemUIFontRounded";

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
    /// The ground behind every screen.
    window: u32,
    /// The sidebar.
    chrome: u32,
    /// Hover and quiet fills on the ground.
    surface: u32,
    border: u32,
    selected: u32,
    text: u32,
    muted: u32,
    /// Warnings in charts (long-context line, a limit nearly used).
    waiting: u32,
    failed: u32,
    banner: u32,
    banner_border: u32,
    /// Roster cells and chart cards, and their keyline.
    card: u32,
    keyline: u32,
    /// A cat's stage, by agent; ended sessions stand on a neutral one.
    stage_claude: u32,
    stage_codex: u32,
    stage_ended: u32,
    /// The pale ring around the cell that needs you.
    ring: u32,
    /// Ribbons, one per state, with white labels except the pale ended one.
    need: u32,
    sprint: u32,
    work: u32,
    idle: u32,
    ended: u32,
    ended_text: u32,
    fail: u32,
    /// Chart series per agent.
    series_claude: u32,
    series_codex: u32,
}

const ON_RIBBON: u32 = 0xFFFFFF;

const LIGHT: Theme = Theme {
    window: 0xFFF6E8,
    chrome: 0xF1EAF3,
    surface: 0xF8EAD4,
    border: 0xE8DACB,
    selected: 0xE3D5E8,
    text: 0x2B2430,
    muted: 0x66596C,
    waiting: 0xB9520A,
    failed: 0xB4423E,
    banner: 0xFFF3E0,
    banner_border: 0xE6C46E,
    card: 0xFFFFFF,
    keyline: 0xEBDCC7,
    stage_claude: 0xFFE9DC,
    stage_codex: 0xE4E8F3,
    stage_ended: 0xF1EEF2,
    ring: 0xFFD6E5,
    need: 0xD92D6A,
    sprint: 0xB9520A,
    work: 0x2F6FE0,
    idle: 0x6E737B,
    ended: 0xE5E1E8,
    ended_text: 0x5A4F60,
    fail: 0xB4423E,
    series_claude: 0xD06A2E,
    series_codex: 0x5F6FCB,
};

const DARK: Theme = Theme {
    window: 0x1F1823,
    chrome: 0x2A2030,
    surface: 0x30263A,
    border: 0x3E3245,
    selected: 0x45384E,
    text: 0xF3ECF5,
    muted: 0xB8ACBF,
    waiting: 0xE59A55,
    failed: 0xF0857C,
    banner: 0x3A2F17,
    banner_border: 0x6E5826,
    card: 0x2B2231,
    keyline: 0x40344A,
    stage_claude: 0x4A3329,
    stage_codex: 0x2E3452,
    stage_ended: 0x342C39,
    ring: 0x5C2440,
    need: 0xD92D6A,
    sprint: 0xB9520A,
    work: 0x2F6FE0,
    idle: 0x6E737B,
    ended: 0x4A4150,
    ended_text: 0xE0D8E4,
    fail: 0xB4423E,
    series_claude: 0xF0A070,
    series_codex: 0xA3AEF0,
};

impl Theme {
    fn stage(&self, p: Provider) -> u32 {
        match p {
            Provider::ClaudeCode => self.stage_claude,
            _ => self.stage_codex,
        }
    }

    /// Ribbon fill and label colour.
    fn ribbon(&self, r: Ribbon) -> (u32, u32) {
        match r {
            Ribbon::NeedsYou => (self.need, ON_RIBBON),
            Ribbon::Sprinting => (self.sprint, ON_RIBBON),
            Ribbon::Working => (self.work, ON_RIBBON),
            Ribbon::Idle => (self.idle, ON_RIBBON),
            Ribbon::Failed => (self.fail, ON_RIBBON),
            Ribbon::Ended => (self.ended, self.ended_text),
        }
    }
}

/// A session's state as its ribbon shows it. The order is the roster's rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Ribbon {
    NeedsYou,
    Sprinting,
    Working,
    Idle,
    Failed,
    Ended,
}

impl Ribbon {
    fn of(status: SessionStatus, sprinting: bool) -> Self {
        match status {
            SessionStatus::Waiting => Self::NeedsYou,
            SessionStatus::Active if sprinting => Self::Sprinting,
            SessionStatus::Active => Self::Working,
            SessionStatus::Idle => Self::Idle,
            SessionStatus::Failed => Self::Failed,
            SessionStatus::Completed => Self::Ended,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::NeedsYou => "needs you",
            Self::Sprinting => "sprinting",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Failed => "failed",
            Self::Ended => "ended",
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
    /// Each roster cell's ribbon at the last render, to spot a change of rank.
    ranks: HashMap<CatKey, Ribbon>,
    /// Cats bouncing after their cell changed rank: when, and a fresh id.
    bounced: HashMap<CatKey, (Instant, u64)>,
    bounce_seq: u64,
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
            ranks: HashMap::new(),
            bounced: HashMap::new(),
            bounce_seq: 0,
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
        // The Sessions table and a session's profile show live cats too.
        if self.screen == Screen::Sessions {
            return self.cats.values().collect();
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
        this.page_scroll.scroll_to_bottom();
        cx.notify();
    })?;
    cx.update(|window, cx| capture(window, cx, &dir.join("overview-end.png")))??;
    this.update(cx, |this, cx| {
        this.page_scroll.set_offset(point(px(0.), px(0.)));
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
}

/// Type scale in px. Nothing is larger than the screen title.
mod ty {
    pub const LABEL: f32 = 12.;
    pub const BODY: f32 = 13.;
    pub const RIBBON: f32 = 11.;
    pub const ROW_TITLE: f32 = 15.;
    pub const PLATE: f32 = 17.;
    pub const FIGURE: f32 = 22.;
    pub const TITLE: f32 = 26.;
}

const TITLEBAR_H: f32 = 38.;
const SIDEBAR_W: f32 = 176.;
/// Horizontal padding of a screen's content.
const CONTENT_PAD: f32 = sp::XL;
const CHART_H: f32 = 96.;

/// Roster cells: never narrower than this, `CELL_GAP` apart, at most three across.
const CELL_MIN: f32 = 340.;
const CELL_GAP: f32 = sp::L;
const MAX_COLUMNS: usize = 3;
/// A cell's stage is a square this wide; its cat is drawn at `STAGE_SCALE`.
const STAGE: f32 = 96.;
const STAGE_SCALE: f32 = 3.;
const CELL_RADIUS: f32 = 18.;
/// The pale ring around the cell that needs you.
const RING: f32 = 3.;
/// Ended sessions shown after the live ones, newest first.
const RECENT_ENDED: usize = 3;
/// The cat's hop when its cell changes rank.
const BOUNCE: Duration = Duration::from_millis(250);
const BOUNCE_PX: f32 = 6.;

/// How many whole cells fit across a window this wide.
fn roster_columns(window_width: f32) -> usize {
    let available = window_width - SIDEBAR_W - 2. * CONTENT_PAD;
    (((available + CELL_GAP) / (CELL_MIN + CELL_GAP)).floor() as usize).clamp(1, MAX_COLUMNS)
}

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

/// How far a cat drawn at `scale` is shifted left so a stationary cat starts at its box's edge.
fn cat_overhang(scale: f32) -> f32 {
    *CAT_LEFT_PAD as f32 * scale
}

/// The Sessions table's cats.
const TABLE_CAT_SCALE: f32 = 2.;
const TABLE_GAP: f32 = 10.;
/// A stationary table cat's width, and the ribbon beside it.
fn table_cat_w() -> f32 {
    32. * TABLE_CAT_SCALE - cat_overhang(TABLE_CAT_SCALE)
}

const SESSION_COLUMNS: [Column; 9] = [
    col("Status", 128., false),
    col("Project", 0., false),
    col("Model", 104., false),
    col("Started", 92., true),
    col("Duration", 68., true),
    col("Active (est.)", 80., true),
    col("Tokens", 56., true),
    col("Est. cost", 72., true),
    col("Claude Code", 76., true),
];
/// The narrowest the project column gets before the table scrolls sideways.
const PROJECT_MIN: f32 = 140.;
/// Left out where the whole table does not fit: Model, Started and Claude
/// Code's own cost record, so the columns shown end on a whole column.
const NARROW_HIDDEN: [usize; 3] = [2, 3, 8];
const NARROW_NOTE: &str = "Est. cost is AgentPulse's estimate at API list prices. Widen the window for Model, Started and Claude Code's own cost record.";

/// The Sessions columns shown in a window this wide.
fn session_columns(window_width: f32) -> Vec<usize> {
    let all: Vec<usize> = (0..SESSION_COLUMNS.len()).collect();
    if table_width(&all) <= window_width - SIDEBAR_W - 2. * CONTENT_PAD {
        all
    } else {
        all.into_iter()
            .filter(|i| !NARROW_HIDDEN.contains(i))
            .collect()
    }
}

/// The least width that shows these columns whole.
fn table_width(columns: &[usize]) -> f32 {
    columns
        .iter()
        .map(|&i| SESSION_COLUMNS[i].width)
        .sum::<f32>()
        + (columns.len() - 1) as f32 * TABLE_GAP
        + PROJECT_MIN
}
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
        let width = f32::from(window.viewport_size().width);
        let view = cx.weak_entity();
        let hub = self.hub.read(cx);
        let order = hub
            .snapshot
            .as_ref()
            .map(|s| roster(&s.sessions, |x| self.sprinting(x)))
            .unwrap_or_default();
        if let Some(s) = &hub.snapshot {
            self.track_ranks(s, &order, hub.motion.enabled);
        }
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
                .overview(s, &order, notes(s, hub.stopped, t), width, t, view)
                .into_any_element(),
            (Screen::Sessions, Some(s)) => match &self.detail {
                Some(key) => charts::session_detail(
                    s,
                    key,
                    self.profile(s, key, t),
                    notes(s, hub.stopped, t),
                    t,
                    view,
                    &self.page_scroll,
                )
                .into_any_element(),
                None => self
                    .sessions(s, notes(s, hub.stopped, t), width, t, view)
                    .into_any_element(),
            },
            (Screen::Usage, Some(s)) => {
                charts::usage(s, notes(s, hub.stopped, t), t, &self.page_scroll).into_any_element()
            }
        };
        div()
            .size_full()
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .track_focus(&self.focus)
            .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
            .on_action(|_: &FocusPrev, window, cx| window.focus_prev(cx))
            .flex()
            .bg(rgb(t.window))
            .font_family(FONT_FAMILY)
            .font_features(tabular())
            .text_color(rgb(t.text))
            .text_size(px(ty::BODY))
            .child(
                // Full height under the traffic lights, as in a native sidebar.
                div()
                    .w(px(SIDEBAR_W))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .bg(rgb(t.chrome))
                    .border_r_1()
                    .border_color(rgb(t.border))
                    .child(div().h(px(TITLEBAR_H)).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .px(px(sp::S))
                            .pt(px(sp::S))
                            .flex()
                            .flex_col()
                            .gap(px(sp::XS))
                            .children(nav),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    // Clear of the title bar, where the window is dragged.
                    .child(div().h(px(TITLEBAR_H - sp::M)).flex_none())
                    .child(div().flex_1().min_h_0().flex().flex_col().child(content)),
            )
    }
}

/// The roster's order: live sessions ranked needs-you, sprinting, working,
/// idle, then the newest few ended ones. Within a rank, the snapshot's order
/// (newest activity first). Indexes into the snapshot's sessions.
fn roster(
    sessions: &[AgentSession],
    sprinting: impl Fn(&AgentSession) -> bool,
) -> Vec<(usize, Ribbon)> {
    let live = sessions
        .iter()
        .enumerate()
        .filter(|(_, x)| x.status.is_live());
    let ended = sessions
        .iter()
        .enumerate()
        .filter(|(_, x)| !x.status.is_live())
        .take(RECENT_ENDED);
    let mut order: Vec<(usize, Ribbon)> = live
        .chain(ended)
        .map(|(i, x)| (i, Ribbon::of(x.status, sprinting(x))))
        .collect();
    // Stable: equal ranks keep the snapshot's order.
    order.sort_by_key(|&(_, r)| r);
    order
}

/// "6 min", "2 h 05 min", "3 d 4 h": a span at a glance.
fn ago(ms: i64) -> String {
    let m = ms.max(0) / 60_000;
    match m {
        0 => "under 1 min".into(),
        1..60 => format!("{m} min"),
        60..1_440 => format!("{} h {:02} min", m / 60, m % 60),
        _ => format!("{} d {} h", m / 1_440, m % 1_440 / 60),
    }
}

/// "118 tok/s", "4.2 tok/s".
fn rate_text(r: f32) -> String {
    if r >= 9.95 {
        format!("{r:.0} tok/s")
    } else {
        format!("{r:.1} tok/s")
    }
}

/// One honest line about what the session is doing, from measured fields only.
fn note(x: &AgentSession, r: Ribbon, now_ms: i64) -> String {
    let since = x.last_activity_at_ms.map(|at| ago(now_ms - at));
    let ended_at = x
        .ended_at_ms
        .or(x.last_activity_at_ms)
        .map(|ms| format::start_time(ms, now_ms));
    match r {
        Ribbon::NeedsYou => match since {
            Some(s) => format!("Needs you for {s}"),
            None => "Needs you".into(),
        },
        Ribbon::Sprinting | Ribbon::Working => {
            let mut line = rate_text(x.output_rate);
            if let Some(c) = x.resources.value().and_then(|r| r.cpu_percent) {
                line += &format!(" · CPU {c:.0}%");
            }
            line
        }
        Ribbon::Idle => match since {
            Some(s) => format!("Idle {s}"),
            None => "Idle".into(),
        },
        Ribbon::Failed => match ended_at {
            Some(at) => format!("Failed {at} · API error"),
            None => "Failed · API error".into(),
        },
        Ribbon::Ended => match ended_at {
            Some(at) => format!("Ended {at}"),
            None => "Ended".into(),
        },
    }
}

impl AppView {
    fn sprinting(&self, x: &AgentSession) -> bool {
        self.cats
            .get(&(x.provider, x.session_id.clone()))
            .is_some_and(|c| c.state() == CatState::Sprinting)
    }

    /// Notes each cell's ribbon; a cell whose ribbon changed bounces its cat once.
    fn track_ranks(&mut self, s: &Snapshot, order: &[(usize, Ribbon)], motion: bool) {
        let mut ranks = HashMap::with_capacity(order.len());
        for &(i, r) in order {
            let key = (s.sessions[i].provider, s.sessions[i].session_id.clone());
            if motion && self.ranks.get(&key).is_some_and(|old| *old != r) {
                self.bounce_seq += 1;
                self.bounced
                    .insert(key.clone(), (Instant::now(), self.bounce_seq));
            }
            ranks.insert(key, r);
        }
        self.ranks = ranks;
        self.bounced.retain(|_, (at, _)| at.elapsed() < BOUNCE);
    }

    fn nav(
        &self,
        label: &'static str,
        screen: Screen,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let on = self.screen == screen;
        div()
            .id(label)
            .role(Role::Tab)
            .aria_label(label)
            .aria_selected(on)
            // One point less padding makes room for the focus ring's border,
            // so the sidebar lays out exactly as it did without one.
            .px(px(sp::M - 1.))
            .py(px(6. - 1.))
            .rounded(px(8.))
            .with_focus_ring(t)
            .map(|d| {
                if on {
                    d.bg(rgb(t.selected)).font_weight(FontWeight::BOLD)
                } else {
                    d.text_color(rgb(t.muted)).hover(|s| s.bg(rgb(t.surface)))
                }
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
        order: &[(usize, Ribbon)],
        notes: Vec<Div>,
        width: f32,
        t: Theme,
        view: WeakEntity<Self>,
    ) -> impl IntoElement {
        let live = order
            .iter()
            .filter(|(i, _)| s.sessions[*i].status.is_live());
        let count = |r: &[Ribbon]| live.clone().filter(|(_, x)| r.contains(x)).count();
        let (need, working, resting) = (
            count(&[Ribbon::NeedsYou]),
            count(&[Ribbon::Sprinting, Ribbon::Working]),
            count(&[Ribbon::Idle]),
        );
        let any_live = live.clone().next().is_some();
        let mut cells: Vec<gpui::AnyElement> = Vec::new();
        if !any_live {
            cells.push(self.empty_cell(t).into_any_element());
        }
        cells.extend(order.iter().map(|&(i, r)| {
            self.cell(i, &s.sessions[i], r, s.taken_at_ms, t, &view)
                .into_any_element()
        }));
        let pills = any_live.then(|| {
            div()
                .flex()
                .gap(px(sp::S))
                .child(pill(
                    &match need {
                        0 => "no one needs you".to_string(),
                        1 => "1 needs you".to_string(),
                        n => format!("{n} need you"),
                    },
                    need > 0,
                    t,
                ))
                .child(pill(&format!("{working} working"), false, t))
                .child(pill(&format!("{resting} resting"), false, t))
        });
        div()
            .id("overview")
            .track_scroll(&self.page_scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(CONTENT_PAD))
            .pt(px(sp::M))
            .pb(px(sp::XL))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(sp::M))
                    .child(heading("Roster"))
                    .children(pills)
                    .child(today_figure(s, t).ml_auto()),
            )
            .children(notes.into_iter().map(|n| n.mt(px(sp::M))))
            .child(
                div()
                    .mt(px(sp::XL))
                    .grid()
                    .grid_cols(roster_columns(width) as u16)
                    .gap(px(CELL_GAP))
                    .children(cells),
            )
            .child(legend(t).mt(px(sp::XL)))
            .child(
                card(t)
                    .mt(px(sp::XL))
                    .child(rate(s, t))
                    .child(machine_line(s, t).mt(px(sp::L))),
            )
    }

    /// A roster cell: the cat on its agent's stage, name plate and ribbon, one
    /// line about what it is doing, and its figures. A click opens its profile.
    fn cell(
        &self,
        i: usize,
        x: &AgentSession,
        r: Ribbon,
        now_ms: i64,
        t: Theme,
        view: &WeakEntity<Self>,
    ) -> Stateful<Div> {
        let key = (x.provider, x.session_id.clone());
        let ended = !x.status.is_live();
        let need = r == Ribbon::NeedsYou;
        let name = format::project_name(x.project_path.as_deref()).to_string();
        let line = note(x, r, now_ms);
        let label = format!("{name}, {}, {}, {line}", x.provider.label(), r.label());
        let view = view.clone();
        let open = key.clone();
        div()
            .id(("cell", i))
            .role(Role::Button)
            .aria_label(label)
            .tab_index(0)
            .cursor_pointer()
            .p(px(RING))
            .rounded(px(CELL_RADIUS + RING))
            .when(need, |d| d.bg(rgb(t.ring)))
            .focus_visible(|s| s.bg(rgb(t.text)))
            .on_click(move |_, _, cx| {
                view.update(cx, |v, cx| {
                    v.screen = Screen::Sessions;
                    v.show_detail(Some(open.clone()), cx);
                    v.schedule_animation(cx);
                })
                .ok();
            })
            .child(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(sp::L - 2.))
                    .px(px(sp::L))
                    .py(px(sp::M + 2.))
                    .rounded(px(CELL_RADIUS))
                    .bg(rgb(t.card))
                    .border_2()
                    .border_color(rgb(if need { t.need } else { t.keyline }))
                    .child(self.stage(x, &key, STAGE, STAGE, STAGE_SCALE, t))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(sp::XS + 2.))
                            .child(plate(&name, r, ty::PLATE, ended, t))
                            .child(div().truncate().text_color(rgb(t.muted)).child(line))
                            .child(facts(x, t)),
                    ),
            )
    }

    /// The cat on its stage: the live animation, or asleep and faded once the
    /// session has ended. It hops once when its cell has just changed rank.
    fn stage(&self, x: &AgentSession, key: &CatKey, w: f32, h: f32, scale: f32, t: Theme) -> Div {
        let ended = !x.status.is_live();
        let look = self.cat_settings.looks().get(x.provider);
        let sprite = if ended {
            &frames(CatState::Sleeping)[0]
        } else {
            self.cats
                .get(key)
                .map_or(self.empty_cat.sprite(), |c| c.sprite())
        };
        // Centre the stationary cat; a sprint's speed lines run off the left.
        let cat = div()
            .relative()
            .ml(px(-cat_overhang(scale)))
            .child(cat_with(sprite, look, scale, ended));
        let cat = match self.bounced.get(key) {
            Some(&(_, n)) => cat
                .with_animation(
                    ElementId::NamedInteger("bounce".into(), n),
                    Animation::new(BOUNCE).with_easing(|v: f32| 1. - (1. - v).powi(3)),
                    |d, v| d.top(px(-BOUNCE_PX * (std::f32::consts::PI * v).sin())),
                )
                .into_any_element(),
            None => cat.into_any_element(),
        };
        div()
            .w(px(w))
            .h(px(h))
            .flex_none()
            .rounded(px(14.))
            .bg(rgb(if ended {
                t.stage_ended
            } else {
                t.stage(x.provider)
            }))
            .overflow_hidden()
            .flex()
            .items_end()
            .justify_center()
            .pb(px(h * 0.1))
            .child(cat)
    }

    /// Stands first in the roster while no agent is live.
    fn empty_cell(&self, t: Theme) -> Div {
        div().p(px(RING)).child(
            div()
                .flex()
                .items_center()
                .gap(px(sp::L - 2.))
                .px(px(sp::L))
                .py(px(sp::M + 2.))
                .rounded(px(CELL_RADIUS))
                .bg(rgb(t.card))
                .border_2()
                .border_color(rgb(t.keyline))
                .child(
                    div()
                        .size(px(STAGE))
                        .flex_none()
                        .rounded(px(14.))
                        .bg(rgb(t.stage_ended))
                        .overflow_hidden()
                        .flex()
                        .items_end()
                        .justify_center()
                        .pb(px(STAGE * 0.1))
                        .child(div().ml(px(-cat_overhang(STAGE_SCALE))).child(cat(
                            self.empty_cat.sprite(),
                            self.cat_settings.looks().get(Provider::ClaudeCode),
                            STAGE_SCALE,
                        ))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(sp::XS + 2.))
                        .child(
                            div()
                                .text_size(px(ty::PLATE))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child("No agents running"),
                        )
                        .child(
                            div()
                                .text_color(rgb(t.muted))
                                .child("Start Claude Code or Codex and it will appear here."),
                        ),
                ),
        )
    }

    /// A session's profile above its charts: a large stage, plate, ribbon and figures.
    fn profile(&self, s: &Snapshot, key: &Key, t: Theme) -> Div {
        let Some(x) = s
            .sessions
            .iter()
            .find(|x| x.provider == key.0 && x.session_id == key.1)
        else {
            return div()
                .child(heading(key.0.label()))
                .child(secondary("This session is no longer listed.", t));
        };
        let r = Ribbon::of(x.status, self.sprinting(x));
        let path = x.project_path.as_deref();
        let mut sub = vec![path.map_or("project path unknown".into(), format::display_path)];
        if let Some(m) = &x.model {
            sub.push(m.clone());
        }
        let mut when = vec![];
        if let Some(ms) = x.started_at_ms {
            when.push(format!("started {}", format::start_time(ms, s.taken_at_ms)));
        }
        if let Some(ms) = x.wall_ms(s.taken_at_ms) {
            when.push(format::duration(ms));
        }
        when.push(format!("{} active (est.)", format::duration(x.active_ms)));
        div()
            .flex()
            .items_center()
            .gap(px(sp::XL))
            .child(
                self.stage(x, key, 168., 120., 4., t)
                    .rounded(px(CELL_RADIUS)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(sp::XS + 2.))
                    .child(plate(
                        format::project_name(path),
                        r,
                        ty::TITLE,
                        !x.status.is_live(),
                        t,
                    ))
                    .child(div().truncate().child(note(x, r, s.taken_at_ms)))
                    .child(facts(x, t))
                    .child(secondary(&sub.join(" · "), t).truncate())
                    .child(secondary(&when.join(" · "), t))
                    .children((x.provider == Provider::ClaudeCode).then(|| {
                        secondary(
                            &format!(
                                "Claude Code's own cost record: {}",
                                format::recorded_cost(x.recorded_cost).unwrap_or(DASH.into())
                            ),
                            t,
                        )
                    })),
            )
    }
}

/// Name plate: the project's name, heavy, and the state's ribbon.
fn plate(name: &str, r: Ribbon, size: f32, ended: bool, t: Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(sp::S))
        .min_w_0()
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(size))
                .font_weight(FontWeight::EXTRA_BOLD)
                .when(ended, |d| d.text_color(rgb(t.muted)))
                .child(name.to_string()),
        )
        .child(ribbon(r, t))
}

fn ribbon(r: Ribbon, t: Theme) -> Div {
    let (bg, fg) = t.ribbon(r);
    div()
        .flex_none()
        .px(px(sp::S))
        .rounded_full()
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .text_size(px(ty::RIBBON))
        .line_height(px(17.))
        .font_weight(FontWeight::BOLD)
        .child(r.label())
}

/// ≈ cost, tokens and the agent, the figures emphasised.
fn facts(x: &AgentSession, t: Theme) -> Div {
    let figure = |v: String| div().font_weight(FontWeight::BOLD).child(v);
    div()
        .flex()
        .gap(px(sp::M + 2.))
        .text_size(px(ty::LABEL))
        .text_color(rgb(t.muted))
        .whitespace_nowrap()
        .overflow_hidden()
        .child(div().text_color(rgb(t.text)).child(figure(
            format::approx_cost(x.cost_usd).unwrap_or(DASH.into()),
        )))
        .child(
            div()
                .flex()
                .gap(px(3.))
                .child(figure(format::tokens(x.tokens.total())).text_color(rgb(t.text)))
                .child("tok"),
        )
        .child(x.provider.label())
}

/// A count in the roster header; the needs-you count is filled in its ribbon colour.
fn pill(text: &str, need: bool, t: Theme) -> Div {
    div()
        .px(px(sp::M))
        .py(px(sp::XS))
        .rounded_full()
        .border_2()
        .text_size(px(12.5))
        .font_weight(FontWeight::BOLD)
        .map(|d| {
            if need {
                d.bg(rgb(t.need))
                    .border_color(rgb(t.need))
                    .text_color(rgb(ON_RIBBON))
            } else {
                d.bg(rgb(t.card)).border_color(rgb(t.keyline))
            }
        })
        .child(text.to_string())
}

/// Today's estimated cost and the Codex plan limit, at the right of the header.
fn today_figure(s: &Snapshot, t: Theme) -> Div {
    let (cost, what) = match format::approx_cost(s.today_cost_usd) {
        Some(c) => (c, "today · estimate"),
        None => (DASH.into(), "today · no price for today's models"),
    };
    let limit = s
        .codex_limits
        .as_ref()
        .and_then(|l| l.primary.as_ref().or(l.secondary.as_ref()));
    let limit = match limit {
        None => format!("Codex limit {DASH}"),
        Some(w) if w.resets_at_s.is_some_and(|r| r * 1000 < s.taken_at_ms) => {
            "Codex limit out of date".into()
        }
        Some(w) => format!(
            "Codex {} limit {:.0}%",
            charts::window_name(w.window_minutes),
            w.used_percent
        ),
    };
    div()
        .flex()
        .flex_col()
        .items_end()
        .child(
            div()
                .text_size(px(ty::FIGURE))
                .font_weight(FontWeight::EXTRA_BOLD)
                .child(cost),
        )
        .child(secondary(&format!("{what} · {limit}"), t))
}

/// What each ribbon colour means.
fn legend(t: Theme) -> Div {
    div().flex().flex_wrap().gap(px(sp::L)).children(
        [
            (t.need, "needs you"),
            (t.sprint, "sprinting"),
            (t.work, "working"),
            (t.idle, "idle"),
            (t.fail, "failed"),
            (t.ended, "ended"),
        ]
        .map(|(c, label)| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .size(px(10.))
                        .rounded_full()
                        .bg(rgb(c))
                        .when(c == t.ended, |d| {
                            d.border_1().border_color(rgb(t.ended_text))
                        }),
                )
                .child(secondary(label, t))
        }),
    )
}

/// A rounded card on the ground, holding one chart or figure group.
fn card(t: Theme) -> Div {
    div()
        .p(px(sp::L + 4.))
        .rounded(px(CELL_RADIUS))
        .bg(rgb(t.card))
        .border_2()
        .border_color(rgb(t.keyline))
        .flex()
        .flex_col()
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
                .rounded(px(10.))
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

/// The agents' CPU and memory, and the whole Mac's GPU, as text.
fn machine_line(s: &Snapshot, t: Theme) -> Div {
    let fig = |v: String| div().font_weight(FontWeight::MEDIUM).child(v);
    let word = |v: &'static str| div().text_color(rgb(t.muted)).child(v);
    let dot = || word("·");
    let cpu = s.agents_cpu_percent.map(|c| format!("{c:.1}%"));
    let gpu = s.system_gpu_percent.value().map(|g| format!("{g}%"));
    let has_gpu = gpu.is_some();
    div()
        .flex()
        .flex_wrap()
        .gap(px(sp::XS))
        .text_size(px(ty::LABEL))
        .child(word("All agents: CPU"))
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
            .rounded(px(8.))
            .bg(rgb(self.t.card))
            .border_1()
            .border_color(rgb(self.t.border))
            .font_family(FONT_FAMILY)
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
                        .w(px(184.))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(ty::FIGURE))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child(format!("{:.1}", s.output_rate)),
                        )
                        .child(secondary("tok/s, 30 s average, all agents", t)),
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
    let (line, base, grid) = (rgb(t.text), rgb(t.muted), rgb(t.border));
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
            let mut path = PathBuilder::stroke(px(2.));
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

// ---------------------------------------------------------------- sessions

impl AppView {
    fn sessions(
        &self,
        s: &Snapshot,
        notes: Vec<Div>,
        width: f32,
        t: Theme,
        view: WeakEntity<AppView>,
    ) -> impl IntoElement {
        let w = |i: usize| SESSION_COLUMNS[i].width;
        let columns = session_columns(width);
        let show = |i: usize| columns.contains(&i);
        let shown: Vec<Column> = columns.iter().map(|&i| SESSION_COLUMNS[i]).collect();
        let n = s.sessions.len();
        div()
            .flex_1()
            .min_h_0()
            .px(px(CONTENT_PAD))
            .pt(px(sp::M))
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
                    .mt(px(sp::XL))
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .child(
                        div()
                            .min_w(px(table_width(&columns)))
                            .pb(px(sp::XL))
                            .flex()
                            .flex_col()
                            .child(header(&shown, t))
                            .children(s.sessions.iter().enumerate().map(|(i, x)| {
                                let key = (x.provider, x.session_id.clone());
                                let r = Ribbon::of(x.status, self.sprinting(x));
                                let ended = !x.status.is_live();
                                let sprite = if ended {
                                    &frames(CatState::Sleeping)[0]
                                } else {
                                    self.cats
                                        .get(&key)
                                        .map_or(self.empty_cat.sprite(), |c| c.sprite())
                                };
                                let look = self.cat_settings.looks().get(x.provider);
                                let view = view.clone();
                                div()
                                    .id(("session", i))
                                    .role(Role::Button)
                                    .aria_label(format!(
                                        "{}, {}, {}",
                                        format::project_name(x.project_path.as_deref()),
                                        x.provider.label(),
                                        r.label()
                                    ))
                                    .tab_index(0)
                                    .cursor_pointer()
                                    .rounded(px(10.))
                                    .hover(|d| d.bg(rgb(t.surface)))
                                    .focus_visible(|d| d.bg(rgb(t.selected)))
                                    .on_click(move |_, _, cx| {
                                        view.update(cx, |v, cx| {
                                            v.show_detail(Some(key.clone()), cx)
                                        })
                                        .ok();
                                    })
                                    .flex()
                                    .items_center()
                                    .gap(px(TABLE_GAP))
                                    .py(px(sp::S))
                                    .border_b_1()
                                    .border_color(rgb(t.border))
                                    .child(
                                        div()
                                            .w(px(w(0)))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .gap(px(sp::S))
                                            .child(
                                                div()
                                                    .w(px(table_cat_w()))
                                                    .flex_none()
                                                    .overflow_hidden()
                                                    .child(
                                                        div()
                                                            .ml(px(-cat_overhang(TABLE_CAT_SCALE)))
                                                            .child(cat_with(
                                                                sprite,
                                                                look,
                                                                TABLE_CAT_SCALE,
                                                                ended,
                                                            )),
                                                    ),
                                            )
                                            .child(ribbon(r, t)),
                                    )
                                    .child(project_cell(x, t))
                                    .when(show(2), |d| {
                                        d.child(match &x.model {
                                            Some(m) => div()
                                                .w(px(w(2)))
                                                .flex_none()
                                                .truncate()
                                                .child(m.clone()),
                                            None => secondary(DASH, t).w(px(w(2))).flex_none(),
                                        })
                                    })
                                    .when(show(3), |d| {
                                        d.child(figures(
                                            w(3),
                                            x.started_at_ms
                                                .map(|ms| format::start_time(ms, s.taken_at_ms)),
                                            None,
                                            t,
                                        ))
                                    })
                                    .child(figures(
                                        w(4),
                                        x.wall_ms(s.taken_at_ms).map(format::duration),
                                        None,
                                        t,
                                    ))
                                    .child(figures(
                                        w(5),
                                        Some(format::duration(x.active_ms)),
                                        None,
                                        t,
                                    ))
                                    .child(figures(
                                        w(6),
                                        Some(format::tokens(x.tokens.total())),
                                        None,
                                        t,
                                    ))
                                    .child(figures(w(7), format::approx_cost(x.cost_usd), None, t))
                                    .when(show(8), |d| {
                                        d.child(figures(
                                            w(8),
                                            format::recorded_cost(x.recorded_cost),
                                            None,
                                            t,
                                        ))
                                    })
                            }))
                            .child(
                                secondary(if show(8) { COST_NOTE } else { NARROW_NOTE }, t)
                                    .mt(px(sp::M)),
                            )
                            .child(secondary("Click a session for its requests.", t).mt(px(sp::S))),
                    ),
            )
    }
}
// ------------------------------------------------------------------ pieces

fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(ty::TITLE))
        .font_weight(FontWeight::EXTRA_BOLD)
        .child(text.into())
}

fn section_label(text: &'static str, t: Theme) -> Div {
    secondary(text, t).font_weight(FontWeight::BOLD)
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
        .gap(px(TABLE_GAP))
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
                .font_weight(FontWeight::BOLD)
                .when(!s.status.is_live(), |d| d.text_color(rgb(t.muted)))
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

/// A right-aligned figure, with an optional secondary line below; None shows the dash.
fn figures(width: f32, top: Option<String>, below: Option<Option<String>>, t: Theme) -> Div {
    let value = |text: Option<String>, size: f32, color: u32| {
        let d = div().text_size(px(size)).whitespace_nowrap();
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

/// An ended session's cat: its coat in grey, a little faded, so the live cats lead.
fn faded(c: Rgba) -> Rgba {
    let grey = 0.299 * c.r + 0.587 * c.g + 0.114 * c.b;
    Rgba {
        r: grey,
        g: grey,
        b: grey,
        a: c.a * 0.6,
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
    cat_with(sprite, look, scale, false)
}

/// [`cat`], [`faded`] when `ended`.
fn cat_with(sprite: &'static Sprite, look: Look, scale: f32, ended: bool) -> impl IntoElement {
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
                    if ended { faded(color) } else { color },
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
            // Each text colour against every background it is drawn on.
            let pairs = [
                (
                    &[t.text][..],
                    &[t.window, t.chrome, t.surface, t.selected, t.banner, t.card][..],
                ),
                (
                    &[t.muted],
                    &[t.window, t.chrome, t.surface, t.selected, t.card],
                ),
                (&[ON_RIBBON], &[t.need, t.sprint, t.work, t.idle, t.fail]),
                (&[t.ended_text], &[t.ended]),
            ];
            for (fgs, bgs) in pairs {
                for (&fg, &bg) in fgs.iter().flat_map(|f| bgs.iter().map(move |b| (f, b))) {
                    let c = contrast(fg, bg);
                    assert!(c >= 4.5, "{fg:06x} on {bg:06x}: {c:.2}");
                }
            }
            // Graphical objects on the cards and ground: chart series and
            // marks, the ended legend swatch's outline, and the keyline of
            // the cell that needs you.
            for fg in [
                t.text,
                t.muted,
                t.ended_text,
                t.waiting,
                t.failed,
                t.series_claude,
                t.series_codex,
                t.need,
            ] {
                for bg in [t.card, t.window] {
                    let c = contrast(fg, bg);
                    assert!(c >= 3.0, "{fg:06x} on {bg:06x}: {c:.2}");
                }
            }
        }
    }

    #[test]
    fn roster_ranks_who_needs_you_first() {
        use agentpulse_core::{Reading, TokenUsage};
        let session = |id: &str, status| AgentSession {
            provider: Provider::ClaudeCode,
            session_id: id.into(),
            project_path: None,
            model: None,
            status,
            started_at_ms: None,
            last_activity_at_ms: Some(0),
            ended_at_ms: None,
            active_ms: 0,
            tokens: TokenUsage::default(),
            cost_usd: Reading::Unavailable,
            unpriced_events: 0,
            output_rate: 0.,
            process: None,
            resources: Reading::Unavailable,
            recorded_cost: Reading::Unavailable,
        };
        use SessionStatus::*;
        let sessions = vec![
            session("idle", Idle),
            session("busy", Active),
            session("fast", Active),
            session("asks", Waiting),
            session("done1", Completed),
            session("fail", Failed),
            session("done2", Completed),
            session("done3", Completed),
        ];
        let order = roster(&sessions, |x| x.session_id == "fast");
        let ids: Vec<&str> = order
            .iter()
            .map(|(i, _)| sessions[*i].session_id.as_str())
            .collect();
        // Only the newest three ended sessions; failed ranks before ended.
        assert_eq!(
            ids,
            ["asks", "fast", "busy", "idle", "fail", "done1", "done2"]
        );
        assert_eq!(order[0].1, Ribbon::NeedsYou);
        assert_eq!(order[1].1, Ribbon::Sprinting);
    }

    #[test]
    fn notes_say_what_was_measured() {
        assert_eq!(ago(0), "under 1 min");
        assert_eq!(ago(6 * 60_000 + 59_000), "6 min");
        assert_eq!(ago(125 * 60_000), "2 h 05 min");
        assert_eq!(ago((3 * 24 + 4) * 3_600_000), "3 d 4 h");
        assert_eq!(rate_text(118.4), "118 tok/s");
        assert_eq!(rate_text(4.25), "4.2 tok/s");
    }

    #[test]
    fn roster_fits_whole_cells() {
        assert_eq!(roster_columns(880.), 1);
        assert_eq!(roster_columns(1120.), 2);
        assert_eq!(roster_columns(1440.), 3);
        assert_eq!(roster_columns(3000.), MAX_COLUMNS);
        for w in (880..=3000).step_by(8).map(|w| w as f32) {
            let n = roster_columns(w) as f32;
            let available = w - SIDEBAR_W - 2. * CONTENT_PAD;
            assert!(
                n * CELL_MIN + (n - 1.) * CELL_GAP <= available || n == 1.,
                "{w}"
            );
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
    fn session_table_shows_whole_columns() {
        // The whole Sessions table fits the default window; the minimum window
        // drops columns instead of slicing one at its edge.
        assert_eq!(session_columns(1120.).len(), SESSION_COLUMNS.len());
        let narrow = session_columns(880.);
        assert!(narrow.iter().all(|i| !NARROW_HIDDEN.contains(i)));
        let room = 880. - SIDEBAR_W - 2. * CONTENT_PAD;
        assert!(table_width(&narrow) <= room, "{}", table_width(&narrow));
        // The leading column holds the cat and the widest ribbon.
        assert!(table_cat_w() + sp::S + 72. <= SESSION_COLUMNS[0].width);
    }

    #[test]
    fn contrast_formula() {
        assert!((contrast(0x000000, 0xFFFFFF) - 21.0).abs() < 1e-9);
        assert!((contrast(0x777777, 0xFFFFFF) - 4.48).abs() < 0.01);
    }
}
