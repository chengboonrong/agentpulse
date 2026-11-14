//! The Settings screen: the animation, the update check and the phones. Also the
//! banner that announces a new release on the other screens.

use agentpulse_platform_macos::reduce_motion;
use gpui::ClickEvent;

use super::cats::toggled;
use super::*;
use crate::hub::{self, now_ms};
use crate::phones::{Phone, Qr, Started, State};
use crate::settings::SPEEDS;
use crate::updates::{CURRENT, Check, Release};

const CARD_MAX_W: f32 = 640.;
const AUTO_DETAIL: &str = "Every 2 hours AgentPulse asks GitHub for its latest release. Nothing about you or your agents is sent.";
const DIRECT_DETAIL: &str = "Phones on the same Wi-Fi get updates straight from this Mac.";
const PHONES_PRIVACY: &str =
    "Updates go through your relay sealed for your phone: the relay can't read them.";
/// The QR code's side, about; modules are whole points so none blur.
const QR_SIDE: f32 = 220.;
/// Light modules added around the code: with the generator's own 1-module margin, the
/// 4 the QR standard asks for.
const QR_QUIET: usize = 3;

/// One click handler that calls `f` on the hub, reading its state at click time.
fn on_hub(
    entity: &Entity<Hub>,
    f: impl Fn(&mut Hub, &mut Context<Hub>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let entity = entity.clone();
    move |_, _, cx| entity.update(cx, |h, cx| f(h, cx))
}

pub(super) fn screen(
    hub: &Hub,
    entity: &Entity<Hub>,
    t: Theme,
    scroll: &gpui::ScrollHandle,
) -> impl IntoElement {
    // The whole page scrolls, release notes and all, as Usage does.
    div()
        .id("settings")
        .track_scroll(scroll)
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px(px(CONTENT_PAD))
        .pt(px(sp::M))
        .pb(px(sp::XL))
        .flex()
        .flex_col()
        .child(heading("Settings"))
        .child(animation_card(hub, entity, t).mt(px(sp::XL)))
        .child(updates_card(hub, entity, t).mt(px(sp::L)))
        .child(phones_card(hub, entity, t).mt(px(sp::L)))
}

fn animation_card(hub: &Hub, entity: &Entity<Hub>, t: Theme) -> Div {
    let a = hub.settings.animation();
    // The speed options follow what is in effect, not just the stored choice.
    let live = hub.motion.enabled;
    let shown = SPEEDS.into_iter().min_by(|x, y| {
        (x - hub.motion.speed)
            .abs()
            .total_cmp(&(y - hub.motion.speed).abs())
    });
    let why = if reduce_motion() {
        Some("Reduce Motion is on in System Settings, so the cats stay still.")
    } else if hub::env_overrides_animation() {
        Some("Set by AGENTPULSE_ANIMATION / AGENTPULSE_ANIMATION_SPEED.")
    } else {
        None
    };
    let speeds = SPEEDS.iter().enumerate().map(|(i, &speed)| {
        let on = shown == Some(speed);
        let label = format!("{speed}×");
        div()
            .id(("speed", i))
            .role(Role::RadioButton)
            .aria_label(label.clone())
            .aria_toggled(toggled(on))
            .px(px(sp::M))
            .py(px(sp::XS))
            .rounded(px(8.))
            .when(on, |d| d.bg(rgb(t.card)).font_weight(FontWeight::BOLD))
            .map(|d| {
                if live {
                    d.with_focus_ring(t)
                        .when(!on, |d| d.hover(|s| s.bg(rgb(t.surface))))
                        .on_click(on_hub(entity, move |h, cx| h.set_speed(speed, cx)))
                } else {
                    // Inert while the cats do not move: no focus, no click.
                    d.aria_description("Animation is off")
                        .border_1()
                        .border_color(transparent_black())
                        .text_color(rgb(t.muted))
                }
            })
            .child(label)
    });
    card(t)
        .max_w(px(CARD_MAX_W))
        .gap(px(sp::M))
        .child(card_title("Animation"))
        .child(switch(
            "animate",
            "Animate cats",
            None,
            a.enabled,
            t,
            on_hub(entity, |h, cx| h.toggle_animation(cx)),
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(sp::M))
                .px(px(sp::S))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .when(!live, |d| d.text_color(rgb(t.muted)))
                        .child("Speed"),
                )
                .child(
                    div()
                        .id("speed")
                        .role(Role::RadioGroup)
                        .aria_label("Speed")
                        .flex()
                        .gap(px(sp::XS))
                        .p(px(3.))
                        .rounded(px(10.))
                        .bg(rgb(t.chrome))
                        .border_1()
                        .border_color(rgb(t.border))
                        .children(speeds),
                ),
        )
        .children(why.map(|w| secondary(w, t).px(px(sp::S))))
}

fn updates_card(hub: &Hub, entity: &Entity<Hub>, t: Theme) -> Div {
    let u = hub.settings.updates();
    let now = now_ms();
    let at = |ms: i64| format::start_time(ms, now);
    let status = if hub.checking {
        "Checking…".to_string()
    } else {
        match &hub.last_check {
            Some(Ok(Check::UpToDate)) => format!(
                "AgentPulse {CURRENT} is up to date · checked {}",
                at(hub.checked_at_ms)
            ),
            Some(Ok(Check::Available(_))) => format!("Checked {}", at(hub.checked_at_ms)),
            Some(Err(e)) => format!("Couldn't check: {e} · {}", at(hub.checked_at_ms)),
            // Checked in an earlier run, not yet in this one.
            None if u.last_checked_ms > 0 => format!("Last checked {}", at(u.last_checked_ms)),
            None => "Never checked".to_string(),
        }
    };
    card(t)
        .max_w(px(CARD_MAX_W))
        .gap(px(sp::M))
        .child(card_title("Updates"))
        .child(switch(
            "auto-updates",
            "Check for updates automatically",
            Some(AUTO_DETAIL),
            u.auto,
            t,
            on_hub(entity, |h, cx| h.toggle_auto_updates(cx)),
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(sp::M))
                .px(px(sp::S))
                .child(
                    button("check-now", "Check Now", t)
                        .on_click(on_hub(entity, |h, cx| h.check_now(cx))),
                )
                .child(div().min_w_0().text_color(rgb(t.muted)).child(status)),
        )
        .children(hub.available().map(|r| release(r, t)))
}

fn phones_card(hub: &Hub, entity: &Entity<Hub>, t: Theme) -> Div {
    let (v, p) = hub.phone_state();
    let card = card(t)
        .max_w(px(CARD_MAX_W))
        .gap(px(sp::M))
        .child(card_title("Phones"));
    if !v.available {
        return card.child(
            secondary("Phones need AgentPulse.app (run scripts/bundle.sh).", t).px(px(sp::S)),
        );
    }
    let now = now_ms();
    let errors = hub.phone_error.iter().chain(&v.error);
    let relay_set = p.relay.is_some();
    card.child(
        field("Relay", p.relay.as_deref().unwrap_or("Not set"), t)
            .child(
                button("paste-relay", "Paste Address", t)
                    .on_click(on_hub(entity, |h, cx| h.paste_relay(cx))),
            )
            .when(relay_set, |d| {
                d.child(
                    button("clear-relay", "Clear", t)
                        .aria_label("Clear relay address")
                        .on_click(on_hub(entity, |h, cx| h.clear_relay(cx))),
                )
            }),
    )
    // Known only once a relay is set: with none, nothing touches the Keychain.
    .when(relay_set, |d| {
        let state = match v.relay_key {
            Some(true) => "Key saved",
            Some(false) => "No key",
            None => DASH,
        };
        d.child(
            field("Relay key", state, t)
                .child(
                    button("paste-key", "Paste Key", t)
                        .on_click(on_hub(entity, |h, cx| h.paste_relay_key(cx))),
                )
                .when(v.relay_key == Some(true), |d| {
                    d.child(
                        button("forget-key", "Forget Key", t)
                            .on_click(on_hub(entity, |h, _| h.phones.set_relay_key(None))),
                    )
                }),
        )
    })
    .children(errors.map(|e| secondary(e, t).px(px(sp::S)).text_color(rgb(t.failed))))
    .child(match &v.pending {
        Some(s) => pairing_sheet(s, entity, t, now),
        None => {
            let label = if hub.pairing_starting {
                "Pairing…"
            } else {
                "Pair a Phone"
            };
            let pair = if relay_set && !hub.pairing_starting {
                button("pair", label, t)
                    .on_click(on_hub(entity, |h, cx| h.pair_phone(cx)))
                    .into_any_element()
            } else {
                // Inert, as the Animation card's speeds are: no focus, no click.
                div()
                    .px(px(sp::M))
                    .py(px(sp::XS))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(transparent_black())
                    .bg(rgb(t.card))
                    .text_color(rgb(t.muted))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label)
                    .into_any_element()
            };
            div()
                .px(px(sp::S))
                .flex()
                .child(pair)
                .when(!relay_set, |d| {
                    d.items_center()
                        .gap(px(sp::M))
                        .child(secondary("Set the relay address first.", t))
                })
        }
    })
    .children(
        v.phones
            .iter()
            .enumerate()
            .filter_map(|(i, phone)| phone_row(i, phone, entity, t, now)),
    )
    .child(switch(
        "direct",
        "Direct connection",
        Some(DIRECT_DETAIL),
        p.direct,
        t,
        on_hub(entity, |h, cx| h.toggle_direct(cx)),
    ))
    .child(secondary(PHONES_PRIVACY, t).px(px(sp::S)))
}

/// A label, its value, then whatever buttons the caller adds.
fn field(label: &'static str, value: &str, t: Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(sp::M))
        .px(px(sp::S))
        .child(
            div()
                .w(px(76.))
                .flex_none()
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(rgb(t.muted))
                .child(value.to_string()),
        )
}

/// The QR code, its countdown and Cancel, while the code is out.
fn pairing_sheet(s: &Started, entity: &Entity<Hub>, t: Theme, now: i64) -> Div {
    let left = (s.expires_at_ms - now).max(0) / 1000;
    div()
        .p(px(sp::L))
        .rounded(px(10.))
        .bg(rgb(t.window))
        .border_1()
        .border_color(rgb(t.keyline))
        .flex()
        .items_center()
        .gap(px(sp::XL))
        .child(qr_code(&s.qr))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(sp::S))
                .child(
                    div()
                        .text_size(px(ty::ROW_TITLE))
                        .font_weight(FontWeight::BOLD)
                        .child("Scan with AgentPulse on your iPhone"),
                )
                .child(secondary(
                    &format!("Expires in {}:{:02}", left / 60, left % 60),
                    t,
                ))
                .child(
                    div().flex().child(
                        button("cancel-pairing", "Cancel", t)
                            .aria_label("Cancel pairing")
                            .on_click(on_hub(entity, |h, _| h.phones.cancel_pairing())),
                    ),
                ),
        )
}

/// Black modules on a white square in both themes, so any camera reads it.
fn qr_code(qr: &Qr) -> Stateful<Div> {
    let qr = qr.clone();
    let n = qr.size + 2 * QR_QUIET;
    let module = (QR_SIDE / n as f32).round().max(1.);
    div()
        .id("pair-qr")
        .role(Role::Image)
        .aria_label("Pairing QR code")
        .flex_none()
        .size(px(module * n as f32))
        .child(
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    window.paint_quad(fill(b, rgb(0xFFFFFF)));
                    for (i, _) in qr.dark.iter().enumerate().filter(|(_, d)| **d) {
                        let (x, y) = (i % qr.size + QR_QUIET, i / qr.size + QR_QUIET);
                        let at = b.origin + point(px(module * x as f32), px(module * y as f32));
                        window.paint_quad(fill(
                            Bounds::new(at, size(px(module), px(module))),
                            rgb(0x000000),
                        ));
                    }
                },
            )
            .size_full(),
        )
}

/// A paired or confirming phone; None for one still waiting (the sheet shows it).
fn phone_row(i: usize, p: &Phone, entity: &Entity<Hub>, t: Theme, now: i64) -> Option<Div> {
    let name = p.name.clone().unwrap_or_else(|| "iPhone".into());
    let row = div().px(px(sp::S)).flex().items_center().gap(px(sp::M));
    let id = p.id.clone();
    if p.state == State::Confirming {
        let code = p.code.as_deref().filter(|c| c.len() == 6 && c.is_ascii());
        let decline = id.clone();
        return Some(
            row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(format!("Pair with {name}?")),
                    )
                    .child(secondary(
                        &code.map_or("Working out the code…".into(), |c| {
                            format!("Check that your phone shows {} {}.", &c[..3], &c[3..])
                        }),
                        t,
                    )),
            )
            // Confirm only once there is a code to compare.
            .when(code.is_some(), |d| {
                d.child(
                    // The primary action: `button` in the accent colour.
                    div()
                        .id(("confirm", i))
                        .role(Role::Button)
                        .aria_label(format!("Confirm pairing with {name}"))
                        .flex_none()
                        .px(px(sp::M))
                        .py(px(sp::XS))
                        .rounded(px(8.))
                        .with_focus_ring(t)
                        .bg(rgb(t.work))
                        .text_color(rgb(ON_RIBBON))
                        .font_weight(FontWeight::MEDIUM)
                        .hover(|s| s.opacity(0.9))
                        .child("Confirm")
                        .on_click(on_hub(entity, move |h, _| h.phones.confirm(&id))),
                )
            })
            .child(
                button(("decline", i), "Decline", t)
                    .aria_label(format!("Decline pairing with {name}"))
                    .on_click(on_hub(entity, move |h, _| h.phones.decline(&decline))),
            ),
        );
    }
    let lines: Vec<String> = match p.state {
        State::Waiting => return None,
        State::Gone => vec!["The relay no longer has this pairing.".into()],
        _ => {
            let seen = match p.last_seen_ms {
                Some(ms) if now - ms < 60_000 => "Last seen just now".to_string(),
                Some(ms) => format!("Last seen {} ago", ago(now - ms)),
                None => "Not seen yet".to_string(),
            };
            let direct = p.direct.then(|| "Connected on this network".to_string());
            [seen].into_iter().chain(direct).collect()
        }
    };
    Some(
        row.child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .truncate()
                        .child(name.clone()),
                )
                .children(lines.iter().map(|l| secondary(l, t))),
        )
        .child(
            button(("remove", i), "Remove", t)
                .aria_label(format!("Remove {name}"))
                .on_click(on_hub(entity, move |h, _| h.phones.remove(&id))),
        ),
    )
}

/// The newer release: its version, date and notes, and where to get it.
fn release(r: &Release, t: Theme) -> Div {
    let url = r.url.clone();
    div()
        .px(px(sp::S))
        .flex()
        .flex_col()
        .gap(px(sp::S))
        .child(
            div()
                .text_size(px(ty::ROW_TITLE))
                .font_weight(FontWeight::BOLD)
                .child(format!("AgentPulse {} is available", r.version)),
        )
        .children(
            r.published
                .as_ref()
                .map(|d| secondary(&format!("Released {d}"), t)),
        )
        .child(
            div()
                .p(px(sp::M))
                .rounded(px(10.))
                .bg(rgb(t.window))
                .border_1()
                .border_color(rgb(t.keyline))
                .text_size(px(ty::LABEL))
                .child(if r.notes.is_empty() {
                    "No release notes.".to_string()
                } else {
                    r.notes.clone()
                }),
        )
        .child(
            div().flex().child(
                button("download", "Download from GitHub", t)
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            ),
        )
}

/// "AgentPulse 0.0.5 is available." over every screen but Settings, until dismissed.
pub(super) fn banner(
    hub: &Hub,
    entity: &Entity<Hub>,
    t: Theme,
    view: WeakEntity<AppView>,
) -> Option<Div> {
    let r = hub.available()?;
    if hub.settings.updates().dismissed.as_deref() == Some(r.version.as_str()) {
        return None;
    }
    let (version, url, entity) = (r.version.clone(), r.url.clone(), entity.clone());
    Some(
        div()
            .mx(px(CONTENT_PAD))
            .mb(px(sp::XS))
            .px(px(sp::M))
            .py(px(sp::S))
            .rounded(px(10.))
            .bg(rgb(t.banner))
            .border_1()
            .border_color(rgb(t.banner_border))
            .flex()
            .items_center()
            .gap(px(sp::S))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("AgentPulse {version} is available.")),
            )
            .child(
                button("whats-new", "What's New", t).on_click(move |_, _, cx| {
                    view.update(cx, |v, cx| {
                        v.screen = Screen::Settings;
                        v.show_detail(None, cx);
                        v.schedule_animation(cx);
                    })
                    .ok();
                }),
            )
            .child(
                button("banner-download", "Download", t)
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
            .child(
                button("dismiss", "Dismiss", t)
                    .aria_label("Dismiss update notice")
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |h, cx| h.dismiss(&version, cx));
                    }),
            ),
    )
}

fn card_title(text: &'static str) -> Div {
    div()
        .text_size(px(ty::ROW_TITLE))
        .font_weight(FontWeight::BOLD)
        .child(text)
}

pub(super) fn button(id: impl Into<ElementId>, label: &'static str, t: Theme) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .flex_none()
        .px(px(sp::M))
        .py(px(sp::XS))
        .rounded(px(8.))
        .with_focus_ring(t)
        .border_color(rgb(t.keyline))
        .bg(rgb(t.card))
        .font_weight(FontWeight::MEDIUM)
        .hover(|s| s.bg(rgb(t.surface)))
        .child(label)
}

/// A labelled on/off switch; the whole row toggles it.
fn switch(
    id: &'static str,
    label: &'static str,
    detail: Option<&'static str>,
    on: bool,
    t: Theme,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Switch)
        .aria_label(label)
        .aria_toggled(toggled(on))
        .when_some(detail, |d, text| d.aria_description(text))
        .flex()
        .items_center()
        .gap(px(sp::L))
        .p(px(sp::S))
        .rounded(px(10.))
        .with_focus_ring(t)
        .hover(|s| s.bg(rgb(t.surface)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().font_weight(FontWeight::MEDIUM).child(label))
                .children(detail.map(|text| secondary(text, t))),
        )
        .child(
            div()
                .flex_none()
                .w(px(36.))
                .h(px(20.))
                .p(px(2.))
                .rounded_full()
                .bg(rgb(if on { t.work } else { t.idle }))
                .flex()
                .when(on, |d| d.justify_end())
                .child(div().size(px(16.)).rounded_full().bg(rgb(ON_RIBBON))),
        )
        .on_click(click)
}
