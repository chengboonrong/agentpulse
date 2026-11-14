//! What the menu bar item shows, derived from a snapshot. The AppKit side is
//! `agentpulse_platform_macos::StatusItem`; this module is pure and tested.

use agentpulse_animation::{
    CatAnimator, CatState, Palette, Sprite, frames, rasterize, skid_frames, windup_frames,
};
use agentpulse_core::{AgentSession, RateLimits, RateWindow, SessionStatus};
use agentpulse_platform_macos::{MenuAction, MenuImage, MenuLine, WIDEST_TITLE};

use crate::collector::Snapshot;
use crate::format::{self, DASH};
use crate::ui::timeseries::columns;
use crate::ui::{RATE_GAP_MS, RATE_SPAN_MS};

/// Integer scale of the 32x16 sprites: 64x32 pixels shown 16 points high,
/// one sprite pixel per Retina point pair.
pub const SCALE: usize = 2;
pub const POINTS_HIGH: f64 = 16.0;
const MAX_SESSION_ROWS: usize = 5;
/// Longer project names are cut to this many characters, so the longest row
/// ends before the duration column (measured in agentpulse-platform-macos's tests).
const MAX_PROJECT_CHARS: usize = 16;
/// The icon fades once the latest snapshot is older than this.
pub const STALE_AFTER_MS: i64 = 60_000;
/// "Updated just now" below this age.
const JUST_NOW_MS: i64 = 15_000;
/// A plan-limit reading older than this says when it was taken.
const LIMIT_AS_OF_AFTER_MS: i64 = 3_600_000;
/// A faded icon keeps this share of its alpha.
const STALE_ALPHA: f32 = 0.45;

/// Template mask: macOS uses only the alpha. The eye is a cut-out and dust is faint.
const MASK: Palette = Palette {
    outline: [0, 0, 0, 255],
    body: [0, 0, 0, 255],
    shade: [0, 0, 0, 255],
    belly: [0, 0, 0, 255],
    eye: [0, 0, 0, 0],
    accent: [0, 0, 0, 255],
    dust: [0, 0, 0, 115],
};

/// Animator input for the one menu bar cat: any Active agent runs at the
/// overall output rate, with the CPU of the Active agents' process trees
/// summed (None when none of them has a CPU reading); else any Waiting looks
/// around; else any Idle sits; with no live session, no snapshot yet, or
/// collection stopped it sleeps.
pub fn cat_input(s: Option<&Snapshot>, stopped: bool) -> (Option<SessionStatus>, f32, Option<f32>) {
    let Some(s) = s.filter(|_| !stopped) else {
        return (None, 0.0, None);
    };
    let has = |st| s.sessions.iter().any(|x| x.status == st);
    if has(SessionStatus::Active) {
        let cpu = s
            .sessions
            .iter()
            .filter(|x| x.status == SessionStatus::Active)
            .filter_map(|x| x.resources.value()?.cpu_percent)
            .reduce(|a, b| a + b);
        (Some(SessionStatus::Active), s.output_rate, cpu)
    } else {
        let status = [SessionStatus::Waiting, SessionStatus::Idle]
            .into_iter()
            .find(|&st| has(st));
        (status, 0.0, None)
    }
}

/// Number of Active agents, capped at "99+"; empty (hidden) when there are
/// none or the counts are stale because collection stopped.
pub fn title(s: Option<&Snapshot>, stopped: bool) -> String {
    let active = s
        .filter(|_| !stopped)
        .map_or(0, |s| count(s, SessionStatus::Active));
    match active {
        0 => String::new(),
        1..=99 => active.to_string(),
        _ => WIDEST_TITLE.into(),
    }
}

/// The menu's rows at wall-clock `now_ms`: freshness, counts, today's figures,
/// the output rate with its sparkline, Codex plan limits, then the live
/// sessions, those waiting for you first, each opening its detail when clicked.
pub fn lines(s: Option<&Snapshot>, stopped: bool, now_ms: i64) -> Vec<MenuLine> {
    let line = |text: String| MenuLine {
        text,
        ..MenuLine::default()
    };
    let mut out = Vec::new();
    if stopped {
        out.push(line("Data collection stopped".into()));
    }
    match s {
        None if !stopped => out.push(line("Reading agent data…".into())),
        None => {}
        Some(s) => {
            out.push(line(updated(now_ms - s.taken_at_ms)));
            let (active, waiting, idle) = (
                count(s, SessionStatus::Active),
                count(s, SessionStatus::Waiting),
                count(s, SessionStatus::Idle),
            );
            out.push(line(if active + waiting + idle == 0 {
                "No agents running".into()
            } else {
                format!("{active} active · {waiting} waiting · {idle} idle")
            }));
            out.push(line(format!(
                "Tokens today  {}",
                format::tokens(s.today_tokens.total())
            )));
            out.push(line(format!(
                "Est. cost today  {}",
                format::approx_cost(s.today_cost_usd).unwrap_or(DASH.into())
            )));
            out.push(MenuLine {
                image: Some(sparkline(&s.rate_history, s.taken_at_ms)),
                ..line(format!("Output  {:.1} tok/s", s.output_rate))
            });
            out.extend(s.codex_limits.iter().flat_map(|l| limit_lines(l, now_ms)));
            let mut live: Vec<&AgentSession> =
                s.sessions.iter().filter(|x| x.status.is_live()).collect();
            // Stable: within each group the snapshot's order, newest activity first.
            live.sort_by_key(|x| x.status != SessionStatus::Waiting);
            out.extend(live.into_iter().take(MAX_SESSION_ROWS).map(|x| {
                MenuLine {
                    // The state word too, so the glyph is not the only cue.
                    text: format!(
                        "{}  {} · {} · {}",
                        glyph(x.status),
                        truncate(format::project_name(x.project_path.as_deref())),
                        x.provider.label(),
                        x.status.label()
                    ),
                    trailing: Some(
                        x.wall_ms(s.taken_at_ms)
                            .map_or(DASH.into(), format::duration),
                    ),
                    image: None,
                    action: Some(MenuAction::OpenSession(session_id(x))),
                }
            }));
        }
    }
    out
}

/// "Updated just now", "Updated 40 s ago", "Updated 3 min ago", "Updated 2 h ago".
fn updated(age_ms: i64) -> String {
    let s = age_ms.max(0) / 1000;
    match s {
        _ if age_ms < JUST_NOW_MS => "Updated just now".into(),
        0..60 => format!("Updated {s} s ago"),
        60..3_600 => format!("Updated {} min ago", s / 60),
        _ => format!("Updated {} h ago", s / 3_600),
    }
}

/// Needs you, working, resting.
fn glyph(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Waiting => "◆",
        SessionStatus::Active => "●",
        _ => "○",
    }
}

fn truncate(name: &str) -> String {
    if name.chars().count() <= MAX_PROJECT_CHARS {
        return name.into();
    }
    name.chars()
        .take(MAX_PROJECT_CHARS - 1)
        .chain(['…'])
        .collect()
}

/// What a session row's [`MenuAction::OpenSession`] carries.
pub fn session_id(x: &AgentSession) -> String {
    format!("{}/{}", x.provider.as_str(), x.session_id)
}

/// One row per Codex limit window, each with its bar.
fn limit_lines(l: &RateLimits, now_ms: i64) -> Vec<MenuLine> {
    l.primary
        .iter()
        .chain(&l.secondary)
        .map(|w| {
            let stale = out_of_date(w, now_ms);
            MenuLine {
                text: limit_text(w, now_ms) + &as_of(l.ts_ms, now_ms).unwrap_or_default(),
                image: Some(limit_bar(w.used_percent, stale)),
                ..MenuLine::default()
            }
        })
        .collect()
}

/// Past its reset time the reading no longer describes the window. The one
/// test the menu, the Overview header and the Usage screen share.
pub fn out_of_date(w: &RateWindow, now_ms: i64) -> bool {
    w.resets_at_s.is_some_and(|r| r * 1000 < now_ms)
}

/// " · as of 14:05" for a plan-limit reading taken more than an hour before `now_ms`.
pub fn as_of(ts_ms: i64, now_ms: i64) -> Option<String> {
    (now_ms - ts_ms > LIMIT_AS_OF_AFTER_MS)
        .then(|| format!(" · as of {}", format::start_time(ts_ms, now_ms)))
}

/// "Codex 5h · 62% used · resets in 1h 12m", or "Codex 5h · Out of date".
fn limit_text(w: &RateWindow, now_ms: i64) -> String {
    let name = format!("Codex {}", short_window(w.window_minutes));
    match w.resets_at_s {
        _ if out_of_date(w, now_ms) => format!("{name} · Out of date"),
        Some(r) => format!(
            "{name} · {:.0}% used · resets in {}",
            w.used_percent,
            format::duration((r * 1000 - now_ms) as u64)
        ),
        None => format!("{name} · {:.0}% used", w.used_percent),
    }
}

/// "5h", "weekly", "daily", "90m": a window's name in a narrow row.
pub fn short_window(minutes: Option<u64>) -> String {
    match minutes {
        Some(10_080) => "weekly".into(),
        Some(1_440) => "daily".into(),
        Some(m) if m > 0 && m % 1_440 == 0 => format!("{}d", m / 1_440),
        Some(m) if m > 0 && m % 60 == 0 => format!("{}h", m / 60),
        Some(m) => format!("{m}m"),
        None => "plan".into(),
    }
}

/// A 40×10 pt bar: a faint track, filled to `used_percent`, the fill faint
/// too when the reading is out of date.
pub fn limit_bar(used_percent: f64, stale: bool) -> MenuImage {
    const W: usize = 80;
    const H: usize = 20;
    // 4 pt tall, centred.
    const TOP: usize = 6;
    const BAR: usize = 8;
    let filled = ((used_percent.clamp(0.0, 100.0) / 100.0) * W as f64).round() as usize;
    let mut rgba = vec![0u8; W * H * 4];
    for y in TOP..TOP + BAR {
        for x in 0..W {
            // Round ends: cut the corners of a half-height radius.
            let r = BAR / 2;
            let dy = (2 * (y - TOP) + 1).abs_diff(BAR);
            let edge = x.min(W - 1 - x);
            if edge < r && (2 * (r - edge) - 1).pow(2) + dy.pow(2) > (2 * r).pow(2) {
                continue;
            }
            rgba[(y * W + x) * 4 + 3] = match (x < filled, stale) {
                (true, false) => 255,
                (true, true) => 120,
                (false, _) => 60,
            };
        }
    }
    MenuImage {
        rgba,
        width: W,
        height: H,
    }
}

/// A 34×12 pt sparkline of the output rate over the chart's span ending at
/// `end_ms`: one bar per point, the column's peak, scaled to the span's peak.
/// A measured zero is a 1 px baseline. A column with no sample stays blank
/// where the samples around it are more than the chart's gap apart; between
/// joined samples it takes the lower of the two, as the chart's line passes there.
pub fn sparkline(history: &[(i64, f32)], end_ms: i64) -> MenuImage {
    const COLS: usize = 34;
    const H: usize = 24;
    let w = COLS * 2;
    let samples: Vec<(i64, f32)> = history
        .iter()
        .copied()
        .filter(|(_, v)| v.is_finite())
        .collect();
    let cols = columns(&samples, end_ms - RATE_SPAN_MS, end_ms, COLS, RATE_GAP_MS);
    let mut peaks: [Option<f32>; COLS] = [None; COLS];
    let mut prev: Option<(usize, f32)> = None;
    for &(c, a, b, breaks) in &cols {
        let v = a.max(b).max(0.0);
        if let Some((pc, pv)) = prev.filter(|&(pc, _)| !breaks && c > pc + 1) {
            for p in &mut peaks[pc + 1..c] {
                *p = Some(pv.min(v));
            }
        }
        peaks[c] = Some(v);
        prev = Some((c, v));
    }
    let top = peaks.iter().flatten().fold(0f32, |m, &v| m.max(v));
    let mut rgba = vec![0u8; w * H * 4];
    for (c, v) in peaks.iter().enumerate() {
        let Some(v) = v else { continue };
        let h = if top > 0.0 {
            ((v / top) * H as f32).round().max(1.0) as usize
        } else {
            1
        };
        for y in H - h..H {
            for x in 2 * c..2 * c + 2 {
                rgba[(y * w + x) * 4 + 3] = 255;
            }
        }
    }
    MenuImage {
        rgba,
        width: w,
        height: H,
    }
}

/// The latest snapshot is too old to trust, or collection stopped.
pub fn stale(s: Option<&Snapshot>, stopped: bool, now_ms: i64) -> bool {
    stopped || s.is_some_and(|s| now_ms - s.taken_at_ms > STALE_AFTER_MS)
}

/// What VoiceOver says for the menu bar item.
pub fn accessibility_label(s: Option<&Snapshot>, stopped: bool, now_ms: i64) -> String {
    if stale(s, stopped, now_ms) {
        return "AgentPulse, not updating".into();
    }
    let Some(s) = s else {
        return "AgentPulse".into();
    };
    let parts: Vec<String> = [
        (SessionStatus::Waiting, "waiting"),
        (SessionStatus::Active, "active"),
        (SessionStatus::Idle, "idle"),
    ]
    .into_iter()
    .map(|(st, word)| (count(s, st), word))
    .filter(|&(n, _)| n > 0)
    .map(|(n, word)| format!("{n} {word}"))
    .collect();
    if parts.is_empty() {
        "AgentPulse, no agents running".into()
    } else {
        format!("AgentPulse, {}", parts.join(", "))
    }
}

/// Any session is waiting for you, while collection runs.
pub fn waiting(s: Option<&Snapshot>, stopped: bool) -> bool {
    !stopped && s.is_some_and(|s| count(s, SessionStatus::Waiting) > 0)
}

fn count(s: &Snapshot, status: SessionStatus) -> usize {
    s.sessions.iter().filter(|x| x.status == status).count()
}

/// What the icon is doing. Only `Still` poses hold a single frame with no timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    Still(CatState),
    Loop(CatState),
    Windup,
    Skid,
}

impl Pose {
    pub fn of(cat: &CatAnimator) -> Pose {
        let sprite = cat.sprite().rows();
        let playing = |set: &[Sprite]| set.iter().any(|f| f.rows() == sprite);
        if playing(windup_frames()) {
            Pose::Windup
        } else if playing(skid_frames()) {
            Pose::Skid
        } else {
            match cat.state() {
                s @ (CatState::Running | CatState::Sprinting) => Pose::Loop(s),
                s => Pose::Still(s),
            }
        }
    }

    pub fn animated(self) -> bool {
        !matches!(self, Pose::Still(_))
    }

    /// The sprite to show: the animator's frame while animated, else the first frame of the pose.
    pub fn sprite(self, cat: &CatAnimator) -> &'static Sprite {
        match self {
            Pose::Still(s) => &frames(s)[0],
            _ => cat.sprite(),
        }
    }
}

pub struct Icon {
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Template bitmap for a sprite at `SCALE`. Every pose uses the full canvas,
/// where the stationary cat always sits at the same place, so the cat does not
/// move when the pose changes. `stale` fades it; `dot` adds the waiting dot in
/// the top-left corner, clear of the stationary poses and inside the canvas, so
/// the item keeps its width.
pub fn icon(sprite: &Sprite, stale: bool, dot: bool) -> Icon {
    let (width, height) = (sprite.width() * SCALE, sprite.height() * SCALE);
    let mut rgba = rasterize(sprite, &MASK, SCALE);
    if dot {
        // A 7 px disc with a 2 px clear ring, so it stays apart from a running cat's dust.
        let (cx, cy) = (4.0f32, 4.0f32);
        for y in 0..10 {
            for x in 0..10 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let a = &mut rgba[(y * width + x) * 4 + 3];
                if d <= 3.5 {
                    *a = 255;
                } else if d <= 5.5 {
                    *a = 0;
                }
            }
        }
    }
    if stale {
        for a in rgba.iter_mut().skip(3).step_by(4) {
            *a = (*a as f32 * STALE_ALPHA).round() as u8;
        }
    }
    Icon {
        rgba,
        width,
        height,
    }
}

/// Writes the exact bitmaps handed to the status item, tinted the way macOS
/// tints a template image: black on a light menu bar, white on a dark one.
/// Also the faded and waiting-dot icons and samples of the menu's images.
#[cfg(feature = "screenshot")]
pub fn preview(dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut shots: Vec<(String, &Sprite)> = [
        CatState::Sleeping,
        CatState::Sitting,
        CatState::LookingAround,
    ]
    .into_iter()
    .map(|s| (format!("{s:?}"), &frames(s)[0]))
    .collect();
    shots.push(("Running".into(), &frames(CatState::Running)[0]));
    for (name, set) in [
        ("Sprinting", frames(CatState::Sprinting)),
        ("Windup", windup_frames()),
        ("Skid", skid_frames()),
    ] {
        shots.extend(
            set.iter()
                .enumerate()
                .map(|(i, f)| (format!("{name}-{i}"), f)),
        );
    }
    let mut images: Vec<(String, Icon)> = shots
        .into_iter()
        .map(|(name, sprite)| (name, icon(sprite, false, false)))
        .collect();
    let sitting = &frames(CatState::Sitting)[0];
    let looking = &frames(CatState::LookingAround)[0];
    images.push(("Sitting-stale".into(), icon(sitting, true, false)));
    images.push(("LookingAround-dot".into(), icon(looking, false, true)));
    images.push(("LookingAround-dot-stale".into(), icon(looking, true, true)));
    images.push((
        "Running-dot".into(),
        icon(&frames(CatState::Running)[0], false, true),
    ));
    images.push((
        "Sprinting-dot".into(),
        icon(&frames(CatState::Sprinting)[0], false, true),
    ));
    let menu = |m: MenuImage| Icon {
        rgba: m.rgba,
        width: m.width,
        height: m.height,
    };
    images.push(("limit-62".into(), menu(limit_bar(62.0, false))));
    images.push(("limit-out-of-date".into(), menu(limit_bar(62.0, true))));
    // Two minutes at one sample a second with a 40 s gap (the Mac slept) and a measured zero stretch.
    let history: Vec<(i64, f32)> = (0..120)
        .filter(|i| !(50..90).contains(i))
        .map(|i| {
            (
                i * 1000,
                if i < 20 {
                    0.0
                } else {
                    20.0 + (i % 17) as f32 * 3.0
                },
            )
        })
        .collect();
    images.push(("sparkline".into(), menu(sparkline(&history, 119_000))));
    const PAD: usize = 4;
    for (name, icon) in images {
        let (w, h) = (icon.width + 2 * PAD, 2 * (icon.height + 2 * PAD));
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            // Top half light menu bar, bottom half dark.
            let (bg, ink) = if y < h / 2 {
                (236.0, 0.0)
            } else {
                (40.0, 255.0)
            };
            let iy = (y % (h / 2)).checked_sub(PAD).filter(|&v| v < icon.height);
            for x in 0..w {
                let ix = x.checked_sub(PAD).filter(|&v| v < icon.width);
                let a = match (ix, iy) {
                    (Some(ix), Some(iy)) => {
                        icon.rgba[(iy * icon.width + ix) * 4 + 3] as f32 / 255.0
                    }
                    _ => 0.0,
                };
                let v = (bg + (ink - bg) * a).round() as u8;
                rgb.extend([v, v, v]);
            }
        }
        let file = std::io::BufWriter::new(std::fs::File::create(
            dir.join(format!("{}.png", name.to_lowercase())),
        )?);
        let mut enc = png::Encoder::new(file, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgb)?;
    }
    tracing::info!(dir = %dir.display(), "saved menu bar previews");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agentpulse_animation::Motion;
    use agentpulse_core::{Provider, Reading, ResourceUsage, TokenUsage};

    use super::*;

    fn session(status: SessionStatus, path: &str) -> AgentSession {
        AgentSession {
            provider: Provider::ClaudeCode,
            session_id: path.into(),
            project_path: Some(path.into()),
            model: None,
            status,
            started_at_ms: Some(0),
            last_activity_at_ms: None,
            ended_at_ms: None,
            active_ms: 0,
            tokens: TokenUsage::default(),
            cost_usd: Reading::Unavailable,
            unpriced_events: 0,
            output_rate: 0.0,
            process: None,
            resources: Reading::Unavailable,
            recorded_cost: Reading::Unavailable,
        }
    }

    fn snapshot(statuses: &[SessionStatus]) -> Snapshot {
        Snapshot {
            taken_at_ms: 46 * 60_000,
            sessions: statuses
                .iter()
                .enumerate()
                .map(|(i, &st)| session(st, &format!("/work/p{i}")))
                .collect(),
            today_tokens: TokenUsage {
                output: 104_000_000,
                ..TokenUsage::default()
            },
            today_cost_usd: Reading::Estimated(55.55),
            today_unpriced_events: 0,
            output_rate: 28.8,
            rate_history: vec![],
            agents_cpu_percent: None,
            agents_rss_bytes: None,
            system_gpu_percent: Reading::Unavailable,
            indexing: false,
            errors: vec![],
            usage: None,
            codex_limits: None,
            detail: None,
        }
    }

    #[test]
    fn overall_cat_follows_the_busiest_agent() {
        use SessionStatus::*;
        let input = |st: &[SessionStatus]| cat_input(Some(&snapshot(st)), false);
        assert_eq!(cat_input(None, false), (None, 0.0, None));
        assert_eq!(input(&[]), (None, 0.0, None));
        assert_eq!(input(&[Completed, Failed]), (None, 0.0, None));
        assert_eq!(input(&[Idle, Completed]), (Some(Idle), 0.0, None));
        assert_eq!(input(&[Idle, Waiting]), (Some(Waiting), 0.0, None));
        assert_eq!(input(&[Idle, Active, Waiting]), (Some(Active), 28.8, None));
        // A stopped collector's last snapshot no longer drives the cat.
        assert_eq!(
            cat_input(Some(&snapshot(&[Active])), true),
            (None, 0.0, None)
        );
    }

    #[test]
    fn overall_cat_cpu_is_the_active_agents_sum() {
        use SessionStatus::*;
        let cpu = |x: &mut AgentSession, c: Option<f32>| {
            x.resources = Reading::Measured(ResourceUsage {
                cpu_percent: c,
                rss_bytes: 1,
                process_count: 1,
            });
        };
        let mut s = snapshot(&[Active, Active, Active, Idle]);
        cpu(&mut s.sessions[0], Some(70.0));
        cpu(&mut s.sessions[1], Some(50.0));
        cpu(&mut s.sessions[2], None); // not yet valid: left out, not counted as 0
        cpu(&mut s.sessions[3], Some(500.0)); // idle: a background build does not count
        assert_eq!(cat_input(Some(&s), false).2, Some(120.0));
        // No Active agent with a reading: unavailable.
        for x in s.sessions.iter_mut().take(2) {
            x.resources = Reading::Unavailable;
        }
        assert_eq!(cat_input(Some(&s), false).2, None);
    }

    #[test]
    fn title_counts_active_agents_and_hides_zero() {
        use SessionStatus::*;
        assert_eq!(title(None, false), "");
        assert_eq!(title(Some(&snapshot(&[Idle, Waiting])), false), "");
        assert_eq!(title(Some(&snapshot(&[Active, Idle, Active])), false), "2");
        assert_eq!(title(Some(&snapshot(&[Active; 99])), false), "99");
        assert_eq!(title(Some(&snapshot(&[Active; 100])), false), "99+");
        assert_eq!(title(Some(&snapshot(&[Active, Active])), true), "");
    }

    /// Each row as the menu shows it: text, trailing column, image size in
    /// points, and whether it is clickable.
    fn dump(rows: &[MenuLine]) -> Vec<String> {
        rows.iter()
            .map(|l| {
                let mut d = l.text.clone();
                if let Some(t) = &l.trailing {
                    d += &format!(" [{t}]");
                }
                if let Some(m) = &l.image {
                    d += &format!(" <image {}x{} pt>", m.width / 2, m.height / 2);
                }
                if let Some(a) = &l.action {
                    d += &format!(" -> {a:?}");
                }
                d
            })
            .collect()
    }

    #[test]
    fn menu_rows() {
        use SessionStatus::*;
        let s = snapshot(&[Active, Idle, Waiting, Idle, Idle, Idle, Completed]);
        let now = s.taken_at_ms + 3_000;
        let rows = dump(&lines(Some(&s), false, now));
        println!("{}", rows.join("\n"));
        assert_eq!(
            rows,
            [
                "Updated just now",
                "1 active · 1 waiting · 4 idle",
                "Tokens today  104M",
                "Est. cost today  ≈ $55.55",
                "Output  28.8 tok/s <image 34x12 pt>",
                // Waiting first, then the snapshot's order; capped at five.
                "◆  p2 · Claude Code · Waiting [46m 00s] -> OpenSession(\"claude-code//work/p2\")",
                "●  p0 · Claude Code · Active [46m 00s] -> OpenSession(\"claude-code//work/p0\")",
                "○  p1 · Claude Code · Idle [46m 00s] -> OpenSession(\"claude-code//work/p1\")",
                "○  p3 · Claude Code · Idle [46m 00s] -> OpenSession(\"claude-code//work/p3\")",
                "○  p4 · Claude Code · Idle [46m 00s] -> OpenSession(\"claude-code//work/p4\")",
            ]
        );
        let mut s = snapshot(&[Completed]);
        s.today_cost_usd = Reading::Unavailable;
        let rows = dump(&lines(Some(&s), true, s.taken_at_ms + 125_000));
        assert_eq!(
            rows[..3],
            [
                "Data collection stopped",
                "Updated 2 min ago",
                "No agents running"
            ]
        );
        assert_eq!(rows[4], "Est. cost today  —");
        assert_eq!(dump(&lines(None, false, 0)), ["Reading agent data…"]);
        assert_eq!(dump(&lines(None, true, 0)), ["Data collection stopped"]);
    }

    #[test]
    fn long_project_names_are_cut() {
        let mut s = snapshot(&[SessionStatus::Active]);
        s.sessions[0].project_path = Some("/work/a-very-long-project-name-indeed".into());
        let rows = lines(Some(&s), false, s.taken_at_ms);
        assert_eq!(
            rows.last().unwrap().text,
            "●  a-very-long-pro… · Claude Code · Active"
        );
    }

    #[test]
    fn the_item_tells_voiceover_its_state() {
        use SessionStatus::*;
        let s = snapshot(&[Active, Waiting, Active, Completed]);
        let now = s.taken_at_ms;
        assert_eq!(
            accessibility_label(Some(&s), false, now),
            "AgentPulse, 1 waiting, 2 active"
        );
        assert_eq!(
            accessibility_label(Some(&snapshot(&[Completed])), false, now),
            "AgentPulse, no agents running"
        );
        assert_eq!(accessibility_label(None, false, now), "AgentPulse");
        assert_eq!(
            accessibility_label(Some(&s), true, now),
            "AgentPulse, not updating"
        );
        assert_eq!(
            accessibility_label(Some(&s), false, now + STALE_AFTER_MS + 1),
            "AgentPulse, not updating"
        );
    }

    #[test]
    fn freshness_counts_up_from_the_snapshot() {
        assert_eq!(updated(0), "Updated just now");
        assert_eq!(updated(14_999), "Updated just now");
        assert_eq!(updated(15_000), "Updated 15 s ago");
        assert_eq!(updated(59_999), "Updated 59 s ago");
        assert_eq!(updated(60_000), "Updated 1 min ago");
        assert_eq!(updated(59 * 60_000), "Updated 59 min ago");
        assert_eq!(updated(3 * 3_600_000), "Updated 3 h ago");
        // A clock that went back reads as fresh, not as a negative age.
        assert_eq!(updated(-5_000), "Updated just now");
    }

    #[test]
    fn codex_limit_rows() {
        let now = 1_800_000_000_000;
        let window = |minutes, pct, resets_in_s: Option<i64>| RateWindow {
            used_percent: pct,
            window_minutes: Some(minutes),
            resets_at_s: resets_in_s.map(|d| now / 1000 + d),
        };
        let mut s = snapshot(&[]);
        s.codex_limits = Some(RateLimits {
            ts_ms: now,
            plan_type: Some("plus".into()),
            primary: Some(window(300, 62.4, Some(72 * 60))),
            secondary: Some(window(10_080, 15.0, Some(-1))),
        });
        let rows = dump(&lines(Some(&s), false, now));
        assert_eq!(
            rows[5..],
            [
                "Codex 5h · 62% used · resets in 1h 12m <image 40x10 pt>",
                "Codex weekly · Out of date <image 40x10 pt>",
            ]
        );
        assert_eq!(
            limit_text(&window(90, 3.0, None), now),
            "Codex 90m · 3% used"
        );
        // A reading over an hour old says when it was taken.
        s.codex_limits.as_mut().unwrap().ts_ms = now - 3_600_001;
        let rows = dump(&lines(Some(&s), false, now));
        let at = format::start_time(now - 3_600_001, now);
        assert_eq!(
            rows[5],
            format!("Codex 5h · 62% used · resets in 1h 12m · as of {at} <image 40x10 pt>")
        );
        assert_eq!(
            as_of(now - 3_600_000, now),
            None,
            "an hour exactly is recent"
        );
        // No reading: no rows. No Claude row, which AgentPulse does not collect.
        let rows = dump(&lines(Some(&snapshot(&[])), false, now));
        assert!(
            rows.iter()
                .all(|r| !r.contains("Codex") && !r.contains("Claude"))
        );
    }

    #[test]
    fn limit_bar_fills_to_the_share_used() {
        let alpha = |m: &MenuImage, x: usize| m.rgba[(10 * m.width + x) * 4 + 3];
        let bar = limit_bar(50.0, false);
        assert_eq!((bar.width, bar.height), (80, 20));
        assert_eq!((alpha(&bar, 20), alpha(&bar, 39)), (255, 255));
        assert_eq!((alpha(&bar, 40), alpha(&bar, 60)), (60, 60), "track");
        assert_eq!(bar.rgba[3], 0, "clear above the bar");
        let stale = limit_bar(50.0, true);
        assert_eq!(alpha(&stale, 20), 120, "faint once out of date");
        assert!(
            limit_bar(150.0, false).rgba.chunks(4).all(|p| p[3] != 60),
            "clamped"
        );
    }

    #[test]
    fn sparkline_leaves_gaps_blank_and_draws_zero_as_a_baseline() {
        // One sample a second; nothing from 40 s to 80 s (the Mac slept); zero for the first 10 s.
        let history: Vec<(i64, f32)> = (0..120)
            .filter(|i| !(40..80).contains(i))
            .map(|i| (i * 1000, if i < 10 { 0.0 } else { 10.0 }))
            .chain([(5_500, f32::NAN)])
            .collect();
        let img = sparkline(&history, 119_000);
        assert_eq!((img.width, img.height), (68, 24));
        // Ink per point column, bottom row up.
        let height = |c: usize| {
            (0..img.height)
                .filter(|&y| img.rgba[(y * img.width + 2 * c) * 4 + 3] == 255)
                .count()
        };
        let cols: Vec<usize> = (0..34).map(height).collect();
        assert_eq!(cols[0], 1, "measured zero: a baseline");
        assert_eq!(cols[33], 24, "the peak fills the height");
        let gap: Vec<usize> = (12..22).map(|c| cols[c]).collect();
        assert!(
            gap.iter().all(|&h| h == 0),
            "the gap is blank, not zero: {cols:?}"
        );
        // 1 s samples fill every column outside the gap.
        assert!(
            (0..34)
                .filter(|c| !(12..22).contains(c))
                .all(|c| cols[c] > 0)
        );
        // 5 s samples (no live agent) leave no holes: joined samples carry across.
        let sparse: Vec<(i64, f32)> = (-1..24).map(|i| (i * 5_000, 4.0)).collect();
        let img = sparkline(&sparse, 115_000);
        assert!((0..34).all(|c| img.rgba[(23 * img.width + 2 * c) * 4 + 3] == 255));
        // No history: nothing drawn.
        assert!(sparkline(&[], 0).rgba.iter().all(|&b| b == 0));
    }

    #[test]
    fn icon_fades_when_stale_and_shows_the_waiting_dot() {
        use SessionStatus::*;
        let sprite = &frames(CatState::LookingAround)[0];
        let plain = icon(sprite, false, false);
        let faded = icon(sprite, true, false);
        let max = |i: &Icon| i.rgba.iter().skip(3).step_by(4).copied().max().unwrap();
        assert_eq!((max(&plain), max(&faded)), (255, 115));
        assert_eq!((faded.width, faded.height), (plain.width, plain.height));
        let dot = icon(sprite, false, true);
        let a = |i: &Icon, x: usize, y: usize| i.rgba[(y * i.width + x) * 4 + 3];
        assert_eq!(
            (a(&plain, 4, 4), a(&dot, 4, 4)),
            (0, 255),
            "dot in the corner"
        );
        assert_eq!(a(&dot, 9, 4), 0, "clear ring around it");
        assert_eq!((dot.width, dot.height), (64, 32), "same canvas, same width");
        // Outside the dot's corner the cat is untouched.
        let corner = |i: usize| (i / 4) % 64 < 10 && (i / 4) / 64 < 10;
        assert!(
            plain
                .rgba
                .iter()
                .zip(&dot.rgba)
                .enumerate()
                .all(|(i, (p, d))| corner(i) || p == d)
        );

        let s = snapshot(&[Active, Waiting]);
        let now = s.taken_at_ms;
        assert!(waiting(Some(&s), false));
        assert!(!waiting(Some(&s), true), "stopped: the counts are stale");
        assert!(!waiting(Some(&snapshot(&[Active])), false));
        assert!(!stale(Some(&s), false, now + STALE_AFTER_MS));
        assert!(stale(Some(&s), false, now + STALE_AFTER_MS + 1));
        assert!(stale(Some(&s), true, now));
        assert!(!stale(None, false, now), "no data yet is not stale data");
    }

    #[test]
    fn icon_is_a_full_canvas_template_mask() {
        let still = icon(&frames(CatState::Sitting)[0], false, false);
        assert_eq!((still.width, still.height), (64, 32));
        assert_eq!(still.rgba.len(), still.width * still.height * 4);
        // Only black, with the eye cut out and dust faint.
        let px = |i: &Icon| i.rgba.as_chunks::<4>().0.to_vec();
        assert!(px(&still).iter().all(|p| p[..3] == [0, 0, 0]));
        assert!(px(&still).iter().any(|p| p[3] == 255));
        let sprint = icon(&frames(CatState::Sprinting)[0], false, false);
        assert_eq!((sprint.width, sprint.height), (64, 32));
        assert!(px(&sprint).iter().any(|p| p[3] == 115));
    }

    #[test]
    fn only_moving_poses_animate() {
        let motion = Motion {
            enabled: true,
            speed: 1.0,
        };
        let mut cat = CatAnimator::new();
        assert_eq!(Pose::of(&cat), Pose::Still(CatState::Sleeping));
        cat.set_input(Some(SessionStatus::Active), 100.0, None);
        cat.tick(Duration::from_millis(1), motion);
        assert_eq!(Pose::of(&cat), Pose::Loop(CatState::Running));
        // Step like the timer does until the smoothed rate crosses the sprint threshold.
        for _ in 0..200 {
            if Pose::of(&cat) != Pose::Loop(CatState::Running) {
                break;
            }
            cat.tick(Duration::from_millis(50), motion);
        }
        assert_eq!(Pose::of(&cat), Pose::Windup);
        cat.tick(Duration::from_secs(1), motion);
        assert_eq!(Pose::of(&cat), Pose::Loop(CatState::Sprinting));
        cat.set_input(Some(SessionStatus::Idle), 0.0, None);
        cat.tick(Duration::from_millis(1), motion);
        assert_eq!(Pose::of(&cat), Pose::Skid);
        assert!(Pose::Skid.animated());
        cat.tick(Duration::from_secs(1), motion);
        let pose = Pose::of(&cat);
        assert_eq!(pose, Pose::Still(CatState::Sitting));
        assert!(!pose.animated());
        assert_eq!(
            pose.sprite(&cat).rows(),
            frames(CatState::Sitting)[0].rows()
        );
    }
}
