//! App-level state that outlives the window: the snapshot stream, the motion
//! settings and the menu bar item with its cat. The window observes it.

use std::time::{Duration, Instant};

use agentpulse_animation::{CatAnimator, Motion};
use agentpulse_platform_macos::{MenuLine, StatusItem, reduce_motion};
use gpui::{Context, Task};

use crate::collector::Snapshot;
use crate::menubar::{self, POINTS_HIGH, Pose};

/// Shortest time between menu bar icon frames.
/// 5 fps: at 10 fps AppKit's per-frame work put a sprinting cat at 3-3.7% CPU.
const MENU_BAR_FRAME: Duration = Duration::from_millis(200);

pub struct Hub {
    pub snapshot: Option<Snapshot>,
    /// The snapshot channel closed; the last snapshot is no longer current.
    pub stopped: bool,
    pub motion: Motion,
    cat: CatAnimator,
    pose: Pose,
    last_tick: Instant,
    item: StatusItem,
    title: String,
    lines: Vec<MenuLine>,
    timer: Option<Task<()>>,
    _snapshots: Task<()>,
}

impl Hub {
    pub fn new(
        rx: async_channel::Receiver<Snapshot>,
        item: StatusItem,
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
        let motion = motion();
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
            pose: Pose::of(&cat),
            cat,
            last_tick: Instant::now(),
            item,
            title: String::new(),
            lines: Vec::new(),
            timer: None,
            _snapshots: snapshots,
        };
        hub.refresh_icon();
        hub.refresh_text();
        hub
    }

    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        // Read on every snapshot, so a change applies even with no window open.
        let motion = motion();
        if motion != self.motion {
            tracing::info!(
                enabled = motion.enabled,
                speed = motion.speed,
                "animation settings changed"
            );
            self.motion = motion;
        }
        self.snapshot = Some(snap);
        self.follow(cx);
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
        let icon = menubar::icon(pose.sprite(&self.cat));
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
        let lines = menubar::lines(self.snapshot.as_ref(), self.stopped);
        if lines != self.lines {
            let rows: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
            tracing::debug!(?rows, "menu bar rows");
            self.item.set_lines(&lines);
            self.lines = lines;
        }
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

pub fn motion() -> Motion {
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
