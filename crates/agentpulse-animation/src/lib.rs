//! Original pixel-art cats and the animation state machine that drives them
//! from agent telemetry. No UI framework dependency.

mod sprites;

use std::time::Duration;

use agentpulse_core::{Provider, SessionStatus};

/// Smoothed output tokens/s above which a running cat starts sprinting.
/// Initial values: calibrate against real sessions.
pub const SPRINT_ON: f32 = 30.0;
/// Smoothed output tokens/s below which a sprinting cat drops back to running.
/// Initial values: calibrate against real sessions.
pub const SPRINT_OFF: f32 = 20.0;

/// Time constant of the activity moving average.
const SMOOTHING_SECS: f32 = 2.0;
/// Poll interval while animation is off but a sprint threshold is still pending.
/// Frame interval of the wind-up and skid one-shots.
const ONESHOT_MS: f32 = 70.0;
const SETTLE_POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Clear,
    Outline,
    Body,
    Shade,
    Belly,
    Eye,
    Accent,
    /// Dust puffs and speed lines: a neutral grey that reads on light and dark.
    Dust,
}

impl Ink {
    fn from_char(c: u8) -> Option<Ink> {
        Some(match c {
            b'.' => Ink::Clear,
            b'o' => Ink::Outline,
            b'b' => Ink::Body,
            b's' => Ink::Shade,
            b'w' => Ink::Belly,
            b'e' => Ink::Eye,
            b'p' => Ink::Accent,
            b'd' => Ink::Dust,
            _ => return None,
        })
    }
}

/// An ASCII pixel grid; see `sprites.rs` for the character legend.
pub struct Sprite {
    rows: &'static [&'static str],
}

impl Sprite {
    pub(crate) const fn new(rows: &'static [&'static str]) -> Self {
        Sprite { rows }
    }

    pub fn width(&self) -> usize {
        self.rows.first().map_or(0, |r| r.len())
    }

    pub fn height(&self) -> usize {
        self.rows.len()
    }

    /// The ASCII rows, one char per pixel; [`Sprite::ink`] gives each char's meaning.
    pub fn rows(&self) -> &'static [&'static str] {
        self.rows
    }

    /// Out-of-range coordinates and unknown characters read as `Clear`.
    pub fn ink(&self, x: usize, y: usize) -> Ink {
        self.rows
            .get(y)
            .and_then(|r| r.as_bytes().get(x))
            .and_then(|&c| Ink::from_char(c))
            .unwrap_or(Ink::Clear)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub outline: [u8; 4],
    pub body: [u8; 4],
    pub shade: [u8; 4],
    pub belly: [u8; 4],
    pub eye: [u8; 4],
    pub accent: [u8; 4],
    pub dust: [u8; 4],
}

impl Palette {
    pub fn color(&self, ink: Ink) -> Option<[u8; 4]> {
        match ink {
            Ink::Clear => None,
            Ink::Outline => Some(self.outline),
            Ink::Body => Some(self.body),
            Ink::Shade => Some(self.shade),
            Ink::Belly => Some(self.belly),
            Ink::Eye => Some(self.eye),
            Ink::Accent => Some(self.accent),
            Ink::Dust => Some(self.dust),
        }
    }
}

const PINK: [u8; 4] = [240, 120, 140, 255];
const DUST: [u8; 4] = [128, 130, 138, 255];

pub fn palette(provider: Provider) -> Palette {
    match provider {
        Provider::ClaudeCode => Palette {
            outline: [150, 78, 30, 255],
            body: [232, 140, 56, 255],
            shade: [176, 88, 28, 255],
            belly: [250, 222, 176, 255],
            eye: [30, 24, 20, 255],
            accent: PINK,
            dust: DUST,
        },
        // Dark grey body with a light outline so it reads on a dark background too.
        Provider::Codex => Palette {
            outline: [150, 160, 180, 255],
            body: [52, 54, 64, 255],
            shade: [78, 82, 96, 255],
            belly: [90, 94, 108, 255],
            eye: [120, 230, 120, 255],
            accent: PINK,
            dust: DUST,
        },
        Provider::OpenCode => Palette {
            outline: [110, 112, 120, 255],
            body: [226, 228, 232, 255],
            shade: [176, 180, 190, 255],
            belly: [255, 255, 255, 255],
            eye: [36, 40, 48, 255],
            accent: PINK,
            dust: DUST,
        },
    }
}

/// RGBA pixels, row-major, `width*scale` by `height*scale`, nearest-neighbour.
pub fn rasterize(sprite: &Sprite, palette: &Palette, scale: usize) -> Vec<u8> {
    let (w, h) = (sprite.width() * scale, sprite.height() * scale);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            if let Some(c) = palette.color(sprite.ink(x / scale, y / scale)) {
                let i = (y * w + x) * 4;
                out[i..i + 4].copy_from_slice(&c);
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CatState {
    Sleeping,
    Sitting,
    LookingAround,
    Running,
    Sprinting,
    Happy,
    Surprised,
}

/// One-shot frames played once when the cat starts sprinting: it scrambles in place.
pub fn windup_frames() -> &'static [Sprite] {
    sprites::WINDUP
}

/// One-shot frames played once when the cat stops sprinting: it skids.
pub fn skid_frames() -> &'static [Sprite] {
    sprites::SKID
}

pub fn frames(state: CatState) -> &'static [Sprite] {
    match state {
        CatState::Sleeping => sprites::SLEEPING,
        CatState::Sitting => sprites::SITTING,
        CatState::LookingAround => sprites::LOOKINGAROUND,
        CatState::Running => sprites::RUNNING,
        CatState::Sprinting => sprites::SPRINTING,
        CatState::Happy => sprites::HAPPY,
        CatState::Surprised => sprites::SURPRISED,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    /// False when the user disabled animation or macOS Reduce Motion is on: the
    /// cat still changes pose with state, but holds the first frame.
    pub enabled: bool,
    /// User speed multiplier, clamped to 0.25..=3.0.
    pub speed: f32,
}

/// Which one-shot sequence is playing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transition {
    Windup,
    Skid,
}

impl Transition {
    fn frames(self) -> &'static [Sprite] {
        match self {
            Transition::Windup => windup_frames(),
            Transition::Skid => skid_frames(),
        }
    }

    /// Entering Sprinting winds up; leaving it for anything but Running skids.
    /// Running <-> Sprinting is just a gear change, so it has no transition.
    fn between(from: CatState, to: CatState) -> Option<Transition> {
        match (from, to) {
            (CatState::Sprinting, CatState::Sprinting | CatState::Running) => None,
            (_, CatState::Sprinting) => Some(Transition::Windup),
            (CatState::Sprinting, _) => Some(Transition::Skid),
            _ => None,
        }
    }
}

pub struct CatAnimator {
    status: Option<SessionStatus>,
    activity: f32,
    smoothed: f32,
    sprinting: bool,
    state: CatState,
    frame: usize,
    /// Time spent on the current frame, in seconds.
    elapsed: f32,
    /// A state change happened since the last `tick`.
    dirty: bool,
    /// Transition to start on the next `tick` (only if animation is enabled).
    pending: Option<Transition>,
    /// Transition currently playing and its frame index.
    playing: Option<(Transition, usize)>,
}

impl Default for CatAnimator {
    fn default() -> Self {
        Self::new()
    }
}

impl CatAnimator {
    pub fn new() -> Self {
        CatAnimator {
            status: None,
            activity: 0.0,
            smoothed: 0.0,
            sprinting: false,
            state: CatState::Sleeping,
            frame: 0,
            elapsed: 0.0,
            dirty: false,
            pending: None,
            playing: None,
        }
    }

    /// Latest telemetry. `status` is None when there is no session to show.
    /// `activity` is output tokens per second over a rolling window.
    pub fn set_input(&mut self, status: Option<SessionStatus>, activity: f32) {
        self.status = status;
        self.activity = if activity.is_finite() {
            activity.max(0.0)
        } else {
            0.0
        };
        self.resolve();
    }

    /// Advance time. Returns true when a repaint is needed.
    pub fn tick(&mut self, dt: Duration, motion: Motion) -> bool {
        let dt = dt.as_secs_f32();
        self.smoothed += (self.activity - self.smoothed) * (1.0 - (-dt / SMOOTHING_SECS).exp());
        self.resolve();
        let mut changed = std::mem::take(&mut self.dirty);
        let pending = self.pending.take();
        if !motion.enabled {
            // Reduce Motion: no transitions, hold frame 0 of the target loop.
            changed |= self.frame != 0 || self.playing.is_some();
            self.frame = 0;
            self.elapsed = 0.0;
            self.playing = None;
            return changed;
        }
        if let Some(t) = pending {
            self.playing = Some((t, 0));
            self.elapsed = 0.0;
        }
        let mut left = dt;
        if let Some((t, i)) = self.playing {
            let iv = self.interval(motion);
            let remaining = (t.frames().len() - i) as f32 * iv - self.elapsed;
            if left >= remaining {
                // Finished (possibly long ago): the rest of the time goes to the loop.
                self.playing = None;
                self.frame = 0;
                self.elapsed = 0.0;
                left -= remaining;
                changed = true;
            } else {
                self.elapsed += left;
                left = 0.0;
                let steps = (self.elapsed / iv) as usize;
                self.elapsed -= steps as f32 * iv;
                self.playing = Some((t, i + steps));
                changed |= steps > 0;
            }
        }
        if self.playing.is_none() && left > 0.0 {
            self.elapsed += left;
            let interval = self.interval(motion);
            let steps = (self.elapsed / interval) as usize;
            if steps > 0 {
                self.elapsed -= steps as f32 * interval;
                let next = (self.frame + steps) % frames(self.state).len();
                changed |= next != self.frame;
                self.frame = next;
            }
        }
        changed
    }

    pub fn state(&self) -> CatState {
        self.state
    }

    pub fn sprite(&self) -> &'static Sprite {
        match self.playing {
            Some((t, i)) => &t.frames()[i],
            None => &frames(self.state)[self.frame],
        }
    }

    /// Time until the next frame change, or None when nothing will change
    /// without new input (so the caller schedules no timer at all).
    pub fn next_frame_in(&self, motion: Motion) -> Option<Duration> {
        if motion.enabled {
            let left = (self.interval(motion) - self.elapsed).max(0.001);
            return Some(Duration::from_secs_f32(left));
        }
        // Frames are frozen, but smoothing may still push activity across a
        // sprint threshold, which changes the pose.
        let pending = self.status == Some(SessionStatus::Active)
            && if self.sprinting {
                self.activity < SPRINT_OFF
            } else {
                self.activity > SPRINT_ON
            };
        pending.then_some(SETTLE_POLL)
    }

    /// Seconds per frame for what is on screen now, for the given motion settings.
    fn interval(&self, motion: Motion) -> f32 {
        let ms = match self.state {
            _ if self.playing.is_some() => ONESHOT_MS,
            CatState::Running => {
                let t = (self.smoothed / SPRINT_ON).clamp(0.0, 1.0);
                180.0 + (110.0 - 180.0) * t
            }
            CatState::Sprinting => 50.0,
            CatState::Sitting => 600.0,
            CatState::LookingAround => 500.0,
            CatState::Sleeping => 900.0,
            CatState::Happy => 450.0,
            CatState::Surprised => 300.0,
        };
        let speed = if motion.speed.is_finite() {
            motion.speed.clamp(0.25, 3.0)
        } else {
            1.0
        };
        ms / 1000.0 / speed
    }

    /// Recompute the state from the inputs; a change restarts the animation.
    fn resolve(&mut self) {
        if self.sprinting {
            self.sprinting = self.smoothed >= SPRINT_OFF;
        } else {
            self.sprinting = self.smoothed > SPRINT_ON;
        }
        let state = match self.status {
            None => CatState::Sleeping,
            Some(SessionStatus::Idle) => CatState::Sitting,
            Some(SessionStatus::Waiting) => CatState::LookingAround,
            Some(SessionStatus::Completed) => CatState::Happy,
            Some(SessionStatus::Failed) => CatState::Surprised,
            Some(SessionStatus::Active) if self.sprinting => CatState::Sprinting,
            Some(SessionStatus::Active) => CatState::Running,
        };
        if state != self.state {
            // A change during a transition cancels it and is judged from the state we
            // were heading for: stopping mid wind-up skids, stopping mid skid just
            // starts the new loop, and sprinting again mid skid winds up again.
            self.pending = Transition::between(self.state, state);
            self.playing = None;
            self.state = state;
            self.frame = 0;
            self.elapsed = 0.0;
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [CatState; 7] = [
        CatState::Sleeping,
        CatState::Sitting,
        CatState::LookingAround,
        CatState::Running,
        CatState::Sprinting,
        CatState::Happy,
        CatState::Surprised,
    ];
    const ON: Motion = Motion {
        enabled: true,
        speed: 1.0,
    };
    const OFF: Motion = Motion {
        enabled: false,
        speed: 1.0,
    };
    const STEP: Duration = Duration::from_millis(100);

    fn rows(s: &Sprite) -> Vec<String> {
        (0..s.height())
            .map(|y| {
                (0..s.width())
                    .map(|x| format!("{:?}", s.ink(x, y)))
                    .collect()
            })
            .collect()
    }

    /// Feed a constant activity for `secs` seconds in 100 ms steps.
    fn feed(a: &mut CatAnimator, status: SessionStatus, activity: f32, secs: u32, m: Motion) {
        a.set_input(Some(status), activity);
        for _ in 0..secs * 10 {
            a.tick(STEP, m);
        }
    }

    #[test]
    fn status_maps_to_state() {
        let cases = [
            (None, CatState::Sleeping),
            (Some(SessionStatus::Idle), CatState::Sitting),
            (Some(SessionStatus::Waiting), CatState::LookingAround),
            (Some(SessionStatus::Completed), CatState::Happy),
            (Some(SessionStatus::Failed), CatState::Surprised),
            (Some(SessionStatus::Active), CatState::Running),
        ];
        for (status, want) in cases {
            let mut a = CatAnimator::new();
            assert_eq!(a.state(), CatState::Sleeping);
            a.set_input(status, 0.0);
            a.tick(STEP, ON);
            assert_eq!(a.state(), want, "{status:?}");
        }
    }

    fn check_set(label: &str, fs: &[Sprite], min: usize, eyes: bool, size: (usize, usize)) {
        assert!(fs.len() >= min, "{label}");
        let (w, h) = size;
        for f in fs {
            assert_eq!((f.width(), f.height()), (w, h), "{label}");
            for r in f.rows {
                assert_eq!(r.len(), w);
                assert!(r.bytes().all(|c| Ink::from_char(c).is_some()), "{r}");
            }
            let count = |ink| {
                (0..h)
                    .flat_map(|y| (0..w).map(move |x| (x, y)))
                    .filter(|&(x, y)| f.ink(x, y) == ink)
                    .count()
            };
            assert!(w * h - count(Ink::Clear) >= 40, "{label}");
            if eyes {
                assert!(count(Ink::Eye) >= 1, "{label}");
            }
        }
    }

    #[test]
    fn frames_are_well_formed() {
        let min = |s| match s {
            CatState::Sleeping | CatState::Sitting | CatState::Happy | CatState::Surprised => 2,
            CatState::LookingAround => 3,
            CatState::Running | CatState::Sprinting => 4,
        };
        let first = &frames(CatState::Sitting)[0];
        let size = (first.width(), first.height());
        assert!((20..=32).contains(&size.0) && (14..=18).contains(&size.1));
        for state in ALL {
            let eyes = !matches!(state, CatState::Sleeping | CatState::Happy);
            check_set(&format!("{state:?}"), frames(state), min(state), eyes, size);
        }
        check_set("windup", windup_frames(), 4, true, size);
        check_set("skid", skid_frames(), 3, true, size);
    }

    #[test]
    fn frames_differ() {
        for state in ALL {
            let fs = frames(state);
            for i in 0..fs.len() {
                assert_ne!(rows(&fs[i]), rows(&fs[(i + 1) % fs.len()]), "{state:?} {i}");
            }
            for other in ALL.into_iter().filter(|&o| o != state) {
                assert_ne!(rows(&fs[0]), rows(&frames(other)[0]), "{state:?} {other:?}");
            }
        }
        for set in [windup_frames(), skid_frames()] {
            for pair in set.windows(2) {
                assert_ne!(rows(&pair[0]), rows(&pair[1]));
            }
        }
    }

    #[test]
    fn stationary_cat_does_not_jump_sideways() {
        // The front of the cat (right-most non-clear, non-dust column) is shared by
        // the stationary poses, so changing state never shifts it sideways.
        let right = |s: &Sprite| {
            (0..s.width())
                .rev()
                .find(|&x| (0..s.height()).any(|y| !matches!(s.ink(x, y), Ink::Clear | Ink::Dust)))
                .unwrap()
        };
        let fronts: Vec<_> = [CatState::Sitting, CatState::LookingAround, CatState::Happy]
            .into_iter()
            .map(|s| right(&frames(s)[0]))
            .collect();
        assert!(
            fronts.windows(2).all(|p| p[0].abs_diff(p[1]) <= 5),
            "{fronts:?}"
        );
    }

    #[test]
    fn sprint_hysteresis() {
        let mut a = CatAnimator::new();
        feed(&mut a, SessionStatus::Active, 35.0, 10, ON);
        assert_eq!(a.state(), CatState::Sprinting);
        feed(&mut a, SessionStatus::Active, 25.0, 10, ON);
        assert_eq!(a.state(), CatState::Sprinting);
        feed(&mut a, SessionStatus::Active, 15.0, 10, ON);
        assert_eq!(a.state(), CatState::Running);
    }

    const T75: Duration = Duration::from_millis(75);

    fn is_in(s: &Sprite, set: &[Sprite]) -> bool {
        set.iter().any(|f| std::ptr::eq(f, s))
    }

    /// An animator that has settled into Running (no transition pending or playing).
    fn running() -> CatAnimator {
        let mut a = CatAnimator::new();
        a.set_input(Some(SessionStatus::Idle), 0.0);
        a.tick(STEP, ON);
        a.set_input(Some(SessionStatus::Active), 0.0);
        a.tick(STEP, ON);
        assert_eq!(a.state(), CatState::Running);
        a
    }

    /// An animator that has settled into Sprinting, wind-up long finished.
    fn sprinting() -> CatAnimator {
        let mut a = running();
        feed(&mut a, SessionStatus::Active, 60.0, 10, ON);
        assert_eq!(a.state(), CatState::Sprinting);
        assert!(a.playing.is_none());
        a
    }

    /// Ticks of 75 ms (one one-shot frame each) until the sprite is out of `set`.
    fn ticks_in(a: &mut CatAnimator, set: &[Sprite]) -> usize {
        let mut n = 0;
        while is_in(a.sprite(), set) {
            n += 1;
            a.tick(T75, ON);
            assert!(n < 50);
        }
        n
    }

    #[test]
    fn windup_plays_once_when_sprint_starts() {
        let mut a = running();
        // Smoothed activity crosses SPRINT_ON while Running: wind-up, then the loop.
        a.set_input(Some(SessionStatus::Active), 60.0);
        let mut seen = vec![];
        for _ in 0..60 {
            a.tick(Duration::from_millis(50), ON);
            seen.push(is_in(a.sprite(), windup_frames()));
        }
        assert_eq!(a.state(), CatState::Sprinting);
        let first = seen.iter().position(|&w| w).expect("wind-up played");
        let len = seen[first..].iter().take_while(|&&w| w).count();
        assert!((3..=5).contains(&len), "{len}");
        assert!(!seen[first + len..].contains(&true), "wind-up repeated");
        assert!(is_in(a.sprite(), frames(CatState::Sprinting)));
    }

    #[test]
    fn windup_frames_are_shown_in_order() {
        let mut a = running();
        a.set_input(Some(SessionStatus::Active), 60.0);
        feed_until_sprint(&mut a);
        assert!(std::ptr::eq(a.sprite(), &windup_frames()[0]));
        for i in 1..4 {
            a.tick(T75, ON);
            assert!(std::ptr::eq(a.sprite(), &windup_frames()[i]));
        }
        assert_eq!(ticks_in(&mut a, windup_frames()), 1); // one more tick leaves the last frame
        assert!(is_in(a.sprite(), frames(CatState::Sprinting)));
    }

    /// Tick in small steps until the state flips to Sprinting; the wind-up has just begun.
    fn feed_until_sprint(a: &mut CatAnimator) {
        for _ in 0..1000 {
            a.tick(Duration::from_millis(10), ON);
            if a.state() == CatState::Sprinting {
                return;
            }
        }
        panic!("never sprinted");
    }

    #[test]
    fn no_transition_for_entering_running_or_sprinting_to_running() {
        let mut a = CatAnimator::new();
        a.set_input(Some(SessionStatus::Idle), 0.0);
        a.tick(STEP, ON);
        a.set_input(Some(SessionStatus::Active), 0.0);
        a.tick(STEP, ON);
        assert!(is_in(a.sprite(), frames(CatState::Running)));

        let mut a = sprinting();
        a.set_input(Some(SessionStatus::Active), 0.0);
        for _ in 0..200 {
            a.tick(STEP, ON);
            let s = a.sprite();
            assert!(!is_in(s, skid_frames()) && !is_in(s, windup_frames()));
        }
        assert_eq!(a.state(), CatState::Running);
    }

    #[test]
    fn skid_plays_once_when_sprint_ends() {
        let mut a = sprinting();
        a.set_input(Some(SessionStatus::Idle), 0.0);
        a.tick(Duration::from_millis(1), ON);
        assert_eq!(a.state(), CatState::Sitting);
        assert!(std::ptr::eq(a.sprite(), &skid_frames()[0]));
        assert_eq!(ticks_in(&mut a, skid_frames()), 3); // every skid frame shown once
        assert!(is_in(a.sprite(), frames(CatState::Sitting)));
        for _ in 0..20 {
            a.tick(T75, ON);
            assert!(is_in(a.sprite(), frames(CatState::Sitting)));
        }
    }

    #[test]
    fn stopping_mid_windup_skids_and_sprinting_mid_skid_winds_up() {
        let mut a = running();
        a.set_input(Some(SessionStatus::Active), 60.0);
        feed_until_sprint(&mut a);
        assert!(is_in(a.sprite(), windup_frames()));
        a.set_input(Some(SessionStatus::Idle), 0.0);
        a.tick(Duration::from_millis(1), ON);
        assert!(std::ptr::eq(a.sprite(), &skid_frames()[0]));
        // Going straight on to another resting state drops the skid for the new loop.
        a.set_input(Some(SessionStatus::Waiting), 0.0);
        a.tick(Duration::from_millis(1), ON);
        assert!(is_in(a.sprite(), frames(CatState::LookingAround)));
    }

    #[test]
    fn motion_disabled_never_shows_one_shots() {
        let mut a = CatAnimator::new();
        for status in [
            SessionStatus::Idle,
            SessionStatus::Active,
            SessionStatus::Idle,
            SessionStatus::Completed,
        ] {
            a.set_input(Some(status), 100.0);
            for _ in 0..80 {
                a.tick(STEP, OFF);
                let s = a.sprite();
                assert!(!is_in(s, windup_frames()) && !is_in(s, skid_frames()));
                assert!(std::ptr::eq(s, &frames(a.state())[0]));
            }
        }
    }

    #[test]
    fn long_dt_during_windup_lands_in_the_loop() {
        let mut a = running();
        a.set_input(Some(SessionStatus::Active), 60.0);
        feed_until_sprint(&mut a);
        assert!(is_in(a.sprite(), windup_frames()));
        a.tick(Duration::from_secs(600), ON);
        assert!(is_in(a.sprite(), frames(CatState::Sprinting)));
        a.set_input(Some(SessionStatus::Idle), 0.0);
        a.tick(Duration::from_millis(1), ON);
        a.tick(Duration::from_secs(600), ON);
        assert!(is_in(a.sprite(), frames(CatState::Sitting)));
    }

    #[test]
    fn single_spike_does_not_stick() {
        let mut a = CatAnimator::new();
        feed(&mut a, SessionStatus::Active, 500.0, 1, ON);
        feed(&mut a, SessionStatus::Active, 5.0, 15, ON);
        assert_eq!(a.state(), CatState::Running);
    }

    #[test]
    fn smoothing_is_gradual() {
        let mut a = CatAnimator::new();
        a.set_input(Some(SessionStatus::Active), 100.0);
        let rest = a.interval(ON);
        a.tick(STEP, ON);
        assert_eq!(a.state(), CatState::Running);
        let moved = (rest - a.interval(ON)) / (0.180 - 0.110);
        assert!((0.0..0.2).contains(&moved), "{moved}");
    }

    #[test]
    fn disabled_motion_holds_frame_zero() {
        let mut a = CatAnimator::new();
        a.set_input(Some(SessionStatus::Active), 10.0);
        assert!(a.tick(STEP, OFF));
        for _ in 0..200 {
            assert!(!a.tick(STEP, OFF));
        }
        assert!(std::ptr::eq(a.sprite(), &frames(CatState::Running)[0]));
        assert_eq!(a.next_frame_in(OFF), None);
        a.set_input(Some(SessionStatus::Failed), 0.0);
        assert!(a.tick(STEP, OFF));
        assert_eq!(a.state(), CatState::Surprised);
    }

    #[test]
    fn disabled_motion_waits_for_pending_sprint() {
        let mut a = CatAnimator::new();
        a.set_input(Some(SessionStatus::Active), 100.0);
        a.tick(STEP, OFF);
        assert!(a.next_frame_in(OFF).is_some());
        for _ in 0..200 {
            a.tick(STEP, OFF);
        }
        assert_eq!(a.state(), CatState::Sprinting);
        assert_eq!(a.next_frame_in(OFF), None);
        assert!(a.next_frame_in(ON).is_some());
    }

    #[test]
    fn speed_scales_and_clamps() {
        let a = CatAnimator::new(); // Sleeping: 900 ms
        let at = |speed| {
            a.interval(Motion {
                enabled: true,
                speed,
            })
        };
        assert!((at(2.0) - 0.45).abs() < 1e-6);
        assert!((at(100.0) - 0.3).abs() < 1e-6);
        assert!((at(0.0) - 3.6).abs() < 1e-6);
    }

    #[test]
    fn huge_dt_is_cheap_and_valid() {
        let mut a = CatAnimator::new();
        a.set_input(Some(SessionStatus::Idle), 0.0);
        a.tick(STEP, ON);
        a.tick(Duration::from_secs(600), ON);
        assert!(a.frame < frames(a.state()).len());
        let _ = a.sprite();
    }

    #[test]
    fn rasterize_layout() {
        let s = &frames(CatState::Sitting)[0];
        let p = palette(Provider::ClaudeCode);
        let px = rasterize(s, &p, 3);
        assert_eq!(px.len(), s.width() * 3 * s.height() * 3 * 4);
        assert_eq!(&px[0..4], &[0, 0, 0, 0]); // (0,0) is Clear
        // Block check on a coloured pixel: first non-clear pixel, all 9 samples agree.
        let (x, y) = (0..s.height())
            .flat_map(|y| (0..s.width()).map(move |x| (x, y)))
            .find(|&(x, y)| s.ink(x, y) != Ink::Clear)
            .unwrap();
        let want = p.color(s.ink(x, y)).unwrap();
        let w = s.width() * 3;
        for dy in 0..3 {
            for dx in 0..3 {
                let i = ((y * 3 + dy) * w + x * 3 + dx) * 4;
                assert_eq!(px[i..i + 4], want);
            }
        }
    }

    #[test]
    fn clear_is_transparent_and_top_left_block_is_uniform() {
        let p = palette(Provider::Codex);
        for state in ALL {
            for s in frames(state) {
                let px = rasterize(s, &p, 3);
                let w = s.width() * 3;
                let want = p.color(s.ink(0, 0)).unwrap_or([0; 4]);
                for dy in 0..3 {
                    for dx in 0..3 {
                        let i = (dy * w + dx) * 4;
                        assert_eq!(px[i..i + 4], want);
                    }
                }
                for y in 0..s.height() {
                    for x in 0..s.width() {
                        if s.ink(x, y) == Ink::Clear {
                            assert_eq!(px[(y * 3 * w + x * 3) * 4 + 3], 0);
                        }
                    }
                }
            }
        }
    }
}
