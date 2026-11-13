//! What the menu bar item shows, derived from a snapshot. The AppKit side is
//! `agentpulse_platform_macos::StatusItem`; this module is pure and tested.

use agentpulse_animation::{
    CatAnimator, CatState, Palette, Sprite, frames, rasterize, skid_frames, windup_frames,
};
use agentpulse_core::SessionStatus;
use agentpulse_platform_macos::{MenuLine, WIDEST_TITLE};

use crate::collector::Snapshot;
use crate::format::{self, DASH};

/// Integer scale of the 32x16 sprites: 64x32 pixels shown 16 points high,
/// one sprite pixel per Retina point pair.
pub const SCALE: usize = 2;
pub const POINTS_HIGH: f64 = 16.0;
const MAX_SESSION_ROWS: usize = 5;

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
/// overall output rate; else any Waiting looks around; else any Idle sits;
/// with no live session, no snapshot yet, or collection stopped it sleeps.
pub fn cat_input(s: Option<&Snapshot>, stopped: bool) -> (Option<SessionStatus>, f32) {
    let Some(s) = s.filter(|_| !stopped) else {
        return (None, 0.0);
    };
    let has = |st| s.sessions.iter().any(|x| x.status == st);
    if has(SessionStatus::Active) {
        (Some(SessionStatus::Active), s.output_rate)
    } else {
        let status = [SessionStatus::Waiting, SessionStatus::Idle]
            .into_iter()
            .find(|&st| has(st));
        (status, 0.0)
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

/// The informational rows of the menu.
pub fn lines(s: Option<&Snapshot>, stopped: bool) -> Vec<MenuLine> {
    let mut out = Vec::new();
    if stopped {
        out.push("Data collection stopped".to_string());
    }
    match s {
        None if !stopped => out.push("Reading agent data…".into()),
        None => {}
        Some(s) => {
            let (active, waiting, idle) = (
                count(s, SessionStatus::Active),
                count(s, SessionStatus::Waiting),
                count(s, SessionStatus::Idle),
            );
            out.push(if active + waiting + idle == 0 {
                "No agents running".into()
            } else {
                format!("{active} active · {waiting} waiting · {idle} idle")
            });
            out.push(format!(
                "Tokens today  {}",
                format::tokens(s.today_tokens.total())
            ));
            out.push(format!(
                "Est. cost today  {}",
                format::approx_cost(s.today_cost_usd).unwrap_or(DASH.into())
            ));
            out.push(format!("Output  {:.1} tok/s", s.output_rate));
            for x in s
                .sessions
                .iter()
                .filter(|x| x.status.is_live())
                .take(MAX_SESSION_ROWS)
            {
                out.push(format!(
                    "{} — {} — {}",
                    format::project_name(x.project_path.as_deref()),
                    x.status.label(),
                    x.wall_ms(s.taken_at_ms)
                        .map_or(DASH.into(), format::duration)
                ));
            }
        }
    }
    out.into_iter().map(|text| MenuLine { text }).collect()
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
/// move when the pose changes.
pub fn icon(sprite: &Sprite) -> Icon {
    Icon {
        rgba: rasterize(sprite, &MASK, SCALE),
        width: sprite.width() * SCALE,
        height: sprite.height() * SCALE,
    }
}

/// Writes the exact bitmaps handed to the status item, tinted the way macOS
/// tints a template image: black on a light menu bar, white on a dark one.
#[cfg(feature = "screenshot")]
pub fn preview(dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut shots: Vec<(String, Pose, &Sprite)> = [
        CatState::Sleeping,
        CatState::Sitting,
        CatState::LookingAround,
    ]
    .into_iter()
    .map(|s| (format!("{s:?}"), Pose::Still(s), &frames(s)[0]))
    .collect();
    shots.push((
        "Running".into(),
        Pose::Loop(CatState::Running),
        &frames(CatState::Running)[0],
    ));
    for (name, pose, set) in [
        (
            "Sprinting",
            Pose::Loop(CatState::Sprinting),
            frames(CatState::Sprinting),
        ),
        ("Windup", Pose::Windup, windup_frames()),
        ("Skid", Pose::Skid, skid_frames()),
    ] {
        shots.extend(
            set.iter()
                .enumerate()
                .map(|(i, f)| (format!("{name}-{i}"), pose, f)),
        );
    }
    const PAD: usize = 4;
    for (name, _, sprite) in shots {
        let icon = icon(sprite);
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
    use agentpulse_core::{AgentSession, Provider, Reading, TokenUsage};

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
            indexing: false,
            errors: vec![],
        }
    }

    #[test]
    fn overall_cat_follows_the_busiest_agent() {
        use SessionStatus::*;
        let input = |st: &[SessionStatus]| cat_input(Some(&snapshot(st)), false);
        assert_eq!(cat_input(None, false), (None, 0.0));
        assert_eq!(input(&[]), (None, 0.0));
        assert_eq!(input(&[Completed, Failed]), (None, 0.0));
        assert_eq!(input(&[Idle, Completed]), (Some(Idle), 0.0));
        assert_eq!(input(&[Idle, Waiting]), (Some(Waiting), 0.0));
        assert_eq!(input(&[Idle, Active, Waiting]), (Some(Active), 28.8));
        // A stopped collector's last snapshot no longer drives the cat.
        assert_eq!(cat_input(Some(&snapshot(&[Active])), true), (None, 0.0));
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

    #[test]
    fn menu_rows() {
        use SessionStatus::*;
        let text = |l: Vec<MenuLine>| l.into_iter().map(|l| l.text).collect::<Vec<_>>();
        let s = snapshot(&[Active, Waiting, Idle, Idle, Idle, Idle, Completed]);
        assert_eq!(
            text(lines(Some(&s), false)),
            [
                "1 active · 1 waiting · 4 idle",
                "Tokens today  104M",
                "Est. cost today  ≈ $55.55",
                "Output  28.8 tok/s",
                "p0 — Active — 46m 00s",
                "p1 — Waiting — 46m 00s",
                "p2 — Idle — 46m 00s",
                "p3 — Idle — 46m 00s",
                "p4 — Idle — 46m 00s",
            ]
        );
        let mut s = snapshot(&[Completed]);
        s.today_cost_usd = Reading::Unavailable;
        let rows = text(lines(Some(&s), true));
        assert_eq!(rows[..2], ["Data collection stopped", "No agents running"]);
        assert_eq!(rows[3], "Est. cost today  —");
        assert_eq!(text(lines(None, false)), ["Reading agent data…"]);
        assert_eq!(text(lines(None, true)), ["Data collection stopped"]);
    }

    #[test]
    fn icon_is_a_full_canvas_template_mask() {
        let still = icon(&frames(CatState::Sitting)[0]);
        assert_eq!((still.width, still.height), (64, 32));
        assert_eq!(still.rgba.len(), still.width * still.height * 4);
        // Only black, with the eye cut out and dust faint.
        let px = |i: &Icon| i.rgba.as_chunks::<4>().0.to_vec();
        assert!(px(&still).iter().all(|p| p[..3] == [0, 0, 0]));
        assert!(px(&still).iter().any(|p| p[3] == 255));
        let sprint = icon(&frames(CatState::Sprinting)[0]);
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
        cat.set_input(Some(SessionStatus::Active), 100.0);
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
        cat.set_input(Some(SessionStatus::Idle), 0.0);
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
