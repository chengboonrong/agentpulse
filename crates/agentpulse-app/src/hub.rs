//! App-level state that outlives the window: the snapshot stream, the motion
//! settings, the update check, the phone link and the menu bar item with its cat.
//! The window observes it.

use std::time::{Duration, Instant};

use agentpulse_animation::{CatAnimator, Motion};
use agentpulse_platform_macos::{MenuLine, StatusItem, reduce_motion};
use gpui::{Context, Task, prelude::*};

use crate::collector::Snapshot;
use crate::menubar::{self, POINTS_HIGH, Pose};
use crate::phones::{self, PhoneLink};
use crate::settings::{self, Settings};
use crate::updates::{self, Check, Release};

/// Shortest time between menu bar icon frames.
/// 5 fps: at 10 fps AppKit's per-frame work put a sprinting cat at 3-3.7% CPU.
const MENU_BAR_FRAME: Duration = Duration::from_millis(200);
/// Automatic update checks are this far apart.
pub const UPDATE_EVERY_MS: i64 = 2 * 60 * 60 * 1000;

pub struct Hub {
    pub snapshot: Option<Snapshot>,
    /// The snapshot channel closed; the last snapshot is no longer current.
    pub stopped: bool,
    pub motion: Motion,
    pub settings: Settings,
    /// An update check is running.
    pub checking: bool,
    /// The latest check's result, this run.
    pub last_check: Option<Result<Check, String>>,
    /// The newer release found this run; a later failed check keeps it.
    found: Option<Release>,
    pub checked_at_ms: i64,
    /// Automatic checks may run: the normal window, not screenshot mode.
    auto_checks: bool,
    update_timer: Option<Task<()>>,
    check_task: Option<Task<()>>,
    pub phones: PhoneLink,
    /// The Phones card's own error: a bad address or key, or a pairing that did not start.
    pub phone_error: Option<String>,
    /// "Pair a Phone" was clicked and the worker has not answered yet.
    pub pairing_starting: bool,
    /// Repaints each second while a QR code's countdown runs.
    countdown: Option<Task<()>>,
    /// The Phones card's view and settings in screenshot mode, instead of the real ones.
    #[cfg(feature = "screenshot")]
    pub phone_fixture: Option<(phones::View, settings::Phones)>,
    cat: CatAnimator,
    pose: Pose,
    last_tick: Instant,
    item: StatusItem,
    title: String,
    /// What VoiceOver says for the item.
    label: String,
    lines: Vec<MenuLine>,
    timer: Option<Task<()>>,
    /// Fades the icon once the latest snapshot gets too old, if no newer one
    /// comes: at most one wakeup a minute.
    stale_timer: Option<Task<()>>,
    /// Keeps the rows' "Updated …" current while the menu is open.
    menu_timer: Option<Task<()>>,
    _snapshots: Task<()>,
    _phone_changes: Task<()>,
}

impl Hub {
    pub fn new(
        rx: async_channel::Receiver<Snapshot>,
        item: StatusItem,
        settings: Settings,
        phones: PhoneLink,
        cx: &mut Context<Self>,
    ) -> Self {
        let snapshots = cx.spawn(async move |this, cx| {
            while let Ok(snap) = rx.recv().await {
                if this.update(cx, |hub, cx| hub.apply(snap, cx)).is_err() {
                    return;
                }
            }
            // The collector thread is gone: say so rather than show its last numbers as live.
            tracing::error!("collector stopped");
            this.update(cx, |hub, cx| {
                hub.stopped = true;
                hub.follow(cx);
            })
            .ok();
        });
        let changes = phones.changes();
        let phone_changes = cx.spawn(async move |this, cx| {
            while changes.recv().await.is_ok() {
                if this.update(cx, |hub, cx| hub.phones_changed(cx)).is_err() {
                    return;
                }
            }
        });
        let motion = motion(&settings);
        tracing::info!(
            animation = motion.enabled,
            speed = motion.speed,
            "animation settings"
        );
        let cat = CatAnimator::new();
        let mut hub = Hub {
            snapshot: None,
            stopped: false,
            motion,
            settings,
            checking: false,
            last_check: None,
            found: None,
            checked_at_ms: 0,
            auto_checks: false,
            update_timer: None,
            check_task: None,
            phones,
            phone_error: None,
            pairing_starting: false,
            countdown: None,
            #[cfg(feature = "screenshot")]
            phone_fixture: None,
            pose: Pose::of(&cat),
            cat,
            last_tick: Instant::now(),
            item,
            title: String::new(),
            label: String::new(),
            lines: Vec::new(),
            timer: None,
            stale_timer: None,
            menu_timer: None,
            _snapshots: snapshots,
            _phone_changes: phone_changes,
        };
        hub.refresh_icon();
        hub.refresh_text();
        hub
    }

    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        // Read on every snapshot, so a Reduce Motion change applies even with no window open.
        self.refresh_motion();
        self.snapshot = Some(snap);
        self.follow(cx);
        if self.stale_timer.is_none() {
            self.arm_stale(cx);
        }
        // Timers stop while the Mac sleeps, so an overdue check starts here.
        if self.check_due() {
            self.check_now(cx);
        }
    }

    fn check_due(&self) -> bool {
        let u = self.settings.updates();
        self.auto_checks
            && u.auto
            && !self.checking
            && now_ms().saturating_sub(u.last_checked_ms) >= UPDATE_EVERY_MS
    }

    fn refresh_motion(&mut self) {
        let motion = motion(&self.settings);
        if motion != self.motion {
            tracing::info!(
                enabled = motion.enabled,
                speed = motion.speed,
                "animation settings changed"
            );
            self.motion = motion;
        }
    }

    /// Turns the stored animation on or off, saves it and applies it at once.
    pub fn toggle_animation(&mut self, cx: &mut Context<Self>) {
        let mut a = self.settings.animation();
        a.enabled = !a.enabled;
        self.settings.set_animation(a);
        self.refresh_motion();
        self.follow(cx);
    }

    /// Saves the animation speed and applies it at once.
    pub fn set_speed(&mut self, speed: f32, cx: &mut Context<Self>) {
        let mut a = self.settings.animation();
        a.speed = speed;
        self.settings.set_animation(a);
        self.refresh_motion();
        self.follow(cx);
    }

    /// The release newer than this build found this run, if any.
    pub fn available(&self) -> Option<&Release> {
        self.found.as_ref()
    }

    /// Starts automatic checks, if they are on. Only the normal window calls this.
    pub fn start_update_checks(&mut self, cx: &mut Context<Self>) {
        self.auto_checks = true;
        self.schedule_check(cx);
    }

    pub fn toggle_auto_updates(&mut self, cx: &mut Context<Self>) {
        let mut u = self.settings.updates();
        u.auto = !u.auto;
        self.settings.set_updates(u);
        self.schedule_check(cx);
        cx.notify();
    }

    /// Hides the banner for this release; Settings and the menu still show it.
    pub fn dismiss(&mut self, version: &str, cx: &mut Context<Self>) {
        let mut u = self.settings.updates();
        u.dismissed = Some(version.to_string());
        self.settings.set_updates(u);
        cx.notify();
    }

    /// Asks GitHub now, unless a check is already running.
    pub fn check_now(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        cx.notify();
        let fetch = cx.background_spawn(async { updates::check() });
        self.check_task = Some(cx.spawn(async move |this, cx| {
            let result = fetch.await;
            this.update(cx, |hub, cx| {
                if let Err(e) = &result {
                    tracing::warn!(error = %e, "update check failed");
                }
                let mut u = hub.settings.updates();
                u.last_checked_ms = now_ms();
                hub.settings.set_updates(u);
                hub.show_check(result, cx);
            })
            .ok();
        }));
    }

    /// Shows a finished check's result and plans the next automatic one.
    pub fn show_check(&mut self, result: Result<Check, String>, cx: &mut Context<Self>) {
        self.checking = false;
        self.checked_at_ms = now_ms();
        match &result {
            Ok(Check::Available(r)) => self.found = Some(r.clone()),
            Ok(Check::UpToDate) => self.found = None,
            Err(_) => {}
        }
        self.last_check = Some(result);
        tracing::info!(
            available = ?self.available().map(|r| r.version.as_str()),
            "update check done"
        );
        self.item
            .set_update(self.available().map(|r| r.version.as_str()));
        self.schedule_check(cx);
        cx.notify();
    }

    /// The next automatic check: [`UPDATE_EVERY_MS`] after the last one,
    /// or now if that has passed. None while automatic checks are off.
    fn schedule_check(&mut self, cx: &mut Context<Self>) {
        self.update_timer = None;
        let u = self.settings.updates();
        if !(self.auto_checks && u.auto) {
            return;
        }
        let wait = u
            .last_checked_ms
            .saturating_add(UPDATE_EVERY_MS)
            .saturating_sub(now_ms())
            .clamp(0, UPDATE_EVERY_MS);
        self.update_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(wait as u64))
                .await;
            this.update(cx, |hub, cx| hub.check_now(cx)).ok();
        }));
    }

    /// What the Phones card shows: the worker's view and the `phones` settings.
    pub fn phone_state(&self) -> (phones::View, settings::Phones) {
        #[cfg(feature = "screenshot")]
        if let Some(fixture) = &self.phone_fixture {
            return fixture.clone();
        }
        (self.phones.view(), self.settings.phones())
    }

    pub fn phones_changed(&mut self, cx: &mut Context<Self>) {
        let counting = self.phone_state().0.pending.is_some();
        self.countdown = counting.then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        return;
                    }
                }
            })
        });
        cx.notify();
    }

    /// Saves the `phones` settings changed by `f` and tells the worker.
    fn change_phones(&mut self, f: impl FnOnce(&mut settings::Phones), cx: &mut Context<Self>) {
        let mut p = self.settings.phones();
        f(&mut p);
        self.settings.set_phones(p);
        self.phones.settings_changed();
        cx.notify();
    }

    /// Saves a typed relay address; false, with the card's error set, when it is not one.
    pub fn save_relay(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let relay = settings::relay_address(text);
        self.phone_error = relay
            .is_none()
            .then(|| "That isn't a relay address: it starts with https://.".to_string());
        match relay {
            Some(relay) => self.change_phones(|p| p.relay = Some(relay), cx),
            None => cx.notify(),
        }
        self.phone_error.is_none()
    }

    pub fn clear_relay(&mut self, cx: &mut Context<Self>) {
        self.phone_error = None;
        self.change_phones(|p| p.relay = None, cx);
    }

    pub fn toggle_direct(&mut self, cx: &mut Context<Self>) {
        self.change_phones(|p| p.direct = !p.direct, cx);
    }

    /// Saves a typed relay key straight to the Keychain; false, with the card's
    /// error set, when it is not one. Never shown or logged.
    pub fn save_relay_key(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let key = Some(text.trim())
            .filter(|k| !k.is_empty() && k.len() <= 256 && !k.contains(char::is_whitespace));
        self.phone_error = key
            .is_none()
            .then(|| "That isn't a relay key: one word, at most 256 characters.".to_string());
        if let Some(key) = key {
            self.phones.set_relay_key(Some(key.to_string()));
        }
        cx.notify();
        self.phone_error.is_none()
    }

    pub fn pair_phone(&mut self, cx: &mut Context<Self>) {
        if self.pairing_starting {
            return;
        }
        self.pairing_starting = true;
        self.phone_error = None;
        let result = self.phones.start_pairing();
        cx.spawn(async move |this, cx| {
            let result = result.recv().await;
            this.update(cx, |hub, cx| {
                hub.pairing_starting = false;
                hub.phone_error = match result {
                    Ok(Ok(_)) => None,
                    Ok(Err(e)) => Some(e),
                    Err(_) => Some("The phone link stopped. Restart AgentPulse.".into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Applies the current snapshot and stopped flag to the cat, title and rows.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let (status, rate, cpu) = menubar::cat_input(self.snapshot.as_ref(), self.stopped);
        self.cat.set_input(status, rate, cpu);
        self.refresh_text();
        // Starts a pending wind-up or skid now rather than at the next timer;
        // a still pose ends the timer.
        self.tick_cat();
        self.schedule(cx);
        cx.notify();
    }

    fn tick_cat(&mut self) {
        let dt = self.last_tick.elapsed();
        self.last_tick = Instant::now();
        self.cat.tick(dt, self.motion);
        self.refresh_icon();
    }

    /// Pushes the current frame to the status item, which skips unchanged images.
    fn refresh_icon(&mut self) {
        let pose = Pose::of(&self.cat);
        if pose != self.pose {
            tracing::debug!(?pose, "menu bar icon");
            self.pose = pose;
        }
        let s = self.snapshot.as_ref();
        let icon = menubar::icon(
            pose.sprite(&self.cat),
            menubar::stale(s, self.stopped, now_ms()),
            menubar::waiting(s, self.stopped),
        );
        self.item
            .set_icon(&icon.rgba, icon.width, icon.height, POINTS_HIGH);
    }

    /// Title and rows follow snapshots, not animation frames.
    fn refresh_text(&mut self) {
        let title = menubar::title(self.snapshot.as_ref(), self.stopped);
        if title != self.title {
            tracing::debug!(title, "menu bar title");
            self.item.set_title(&title);
            self.title = title;
        }
        let now = now_ms();
        let label = menubar::accessibility_label(self.snapshot.as_ref(), self.stopped, now);
        if label != self.label {
            self.item.set_accessibility_label(&label);
            self.label = label;
        }
        let lines = menubar::lines(self.snapshot.as_ref(), self.stopped, now);
        if lines != self.lines {
            let rows: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
            tracing::debug!(?rows, "menu bar rows");
            self.item.set_lines(&lines);
            self.lines = lines;
        }
    }

    /// Wakes when the latest snapshot would turn stale. If a newer one came
    /// meanwhile it waits again for that one; otherwise the icon fades and the
    /// rows' freshness catches up.
    fn arm_stale(&mut self, cx: &mut Context<Self>) {
        let Some(taken) = self.snapshot.as_ref().map(|s| s.taken_at_ms) else {
            return;
        };
        let wait = (taken + menubar::STALE_AFTER_MS + 1 - now_ms()).max(0) as u64;
        self.stale_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(wait))
                .await;
            this.update(cx, |hub, cx| {
                hub.stale_timer = None;
                hub.refresh_icon();
                hub.refresh_text();
                if !menubar::stale(hub.snapshot.as_ref(), hub.stopped, now_ms()) {
                    hub.arm_stale(cx);
                }
            })
            .ok();
        }));
    }

    /// While the menu is open its rows are recomputed each second, so the
    /// freshness row keeps counting even when no snapshot arrives.
    pub fn menu_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.menu_timer = open.then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    let tick = this.update(cx, |hub, _| hub.refresh_text());
                    if tick.is_err() {
                        return;
                    }
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                }
            })
        });
    }

    /// The menu bar is always visible, so this timer does not pause with the
    /// window. It runs only while the cat moves; still poses change on new
    /// snapshots alone.
    fn schedule(&mut self, cx: &mut Context<Self>) {
        self.timer = None;
        if !self.pose.animated() {
            return;
        }
        let Some(next) = self.cat.next_frame_in(self.motion) else {
            return;
        };
        // A 16 pt icon needs no more than 5 fps. Ticking with the real elapsed
        // time keeps the animation's speed; faster loops just skip frames.
        let next = next.max(MENU_BAR_FRAME);
        let wait = next.saturating_sub(self.last_tick.elapsed());
        self.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |hub, cx| {
                hub.tick_cat();
                hub.schedule(cx);
            })
            .ok();
        }));
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// `AGENTPULSE_ANIMATION` (`on` or `off`) and a numeric
/// `AGENTPULSE_ANIMATION_SPEED`, the developer knobs that win over Settings.
/// Other values are ignored, with one warning.
fn env_animation() -> (Option<bool>, Option<f32>) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    let enabled = std::env::var("AGENTPULSE_ANIMATION").ok().and_then(|v| {
        match v.to_ascii_lowercase().as_str() {
            "on" => Some(true),
            "off" => Some(false),
            _ => {
                WARNED.call_once(|| {
                    tracing::warn!(value = %v, "AGENTPULSE_ANIMATION is not on or off, ignored");
                });
                None
            }
        }
    });
    let speed = std::env::var("AGENTPULSE_ANIMATION_SPEED")
        .ok()
        .and_then(|v| v.parse().ok());
    (enabled, speed)
}

/// Whether an environment variable in effect decides the animation instead of Settings.
pub fn env_overrides_animation() -> bool {
    env_animation() != (None, None)
}

/// Settings, overridden by the environment; Reduce Motion always stops it.
pub fn motion(settings: &Settings) -> Motion {
    let a = settings.animation();
    let (enabled, speed) = env_animation();
    Motion {
        enabled: enabled.unwrap_or(a.enabled) && !reduce_motion(),
        speed: speed.unwrap_or(a.speed),
    }
}
