//! The Cats screen: each agent's coat and eye colour. A click applies the
//! choice everywhere the cat is drawn and saves it.

use agentpulse_animation::{COATS, Coat, EYES, GROUPS};
use gpui::{ClickEvent, Stateful, Toggled, WeakEntity};

use super::*;

/// Agents AgentPulse reads today, in the order of the switch.
const AGENTS: [Provider; 2] = [Provider::ClaudeCode, Provider::Codex];
const PREVIEW_SCALE: f32 = 4.;
const SWATCH_SCALE: f32 = 2.;
/// The 64-point cat plus padding, and room for a two-line name.
const SWATCH_W: f32 = 104.;
const EYE_DOT: f32 = 12.;

impl AppView {
    pub(super) fn cats_screen(&self, t: Theme, this: WeakEntity<Self>) -> impl IntoElement {
        let agent = self.cats_agent;
        let look = self.settings.looks().get(agent);
        let eyes_text = look
            .eye
            .map_or("Coat's own eyes".into(), |e| format!("{} eyes", e.name));
        div()
            .id("cats")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(CONTENT_PAD))
            .pt(px(sp::M))
            .pb(px(sp::XL))
            .flex()
            .flex_col()
            .child(heading("Cats"))
            .child(
                secondary(
                    "Each agent's cat on the Overview and in the widget. A choice applies at once and is saved.",
                    t,
                )
                .mt(px(sp::XS)),
            )
            .child(div().mt(px(sp::XL)).flex().child(agent_switch(agent, t, &this)))
            .child(
                div()
                    .id("preview")
                    .role(Role::Image)
                    .aria_label(format!(
                        "{}'s cat, {}, {}, sitting and running",
                        agent.label(),
                        look.coat.name,
                        eyes_text.to_lowercase()
                    ))
                    .mt(px(sp::L))
                    .p(px(sp::L))
                    .rounded(px(CELL_RADIUS))
                    .bg(rgb(t.card))
                    .border_2()
                    .border_color(rgb(t.keyline))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(sp::XL))
                    .child(
                        // The agent's profile stage, sitting and running.
                        div()
                            .flex()
                            .items_end()
                            .gap(px(sp::L))
                            .px(px(sp::XL))
                            .pt(px(sp::L))
                            .pb(px(sp::M))
                            .rounded(px(14.))
                            .bg(rgb(t.stage(agent)))
                            .children(self.preview.iter().map(|a| {
                                div()
                                    .ml(px(-cat_overhang(PREVIEW_SCALE)))
                                    .child(cat(a.sprite(), look, PREVIEW_SCALE))
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(sp::XS))
                            .child(
                                div()
                                    .text_size(px(ty::FIGURE))
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .child(look.coat.name),
                            )
                            .child(div().text_color(rgb(t.muted)).child(format!(
                                "{}'s cat · {} · {}",
                                agent.label(),
                                look.coat.group,
                                eyes_text
                            ))),
                    ),
            )
            .child(section_label("Eyes", t).mt(px(sp::GROUP)))
            .child(eyes(agent, look, t, &this).mt(px(sp::S)))
            .children(GROUPS.iter().enumerate().map(|(i, &group)| {
                div()
                    .mt(px(if i == 0 { sp::GROUP } else { sp::M }))
                    .flex()
                    .flex_col()
                    .gap(px(sp::S))
                    .child(div().font_weight(FontWeight::BOLD).child(group))
                    .child(
                        div()
                            .id(group)
                            .role(Role::RadioGroup)
                            .aria_label(group)
                            .flex()
                            .flex_wrap()
                            .gap(px(sp::S))
                            .children(
                                COATS
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, c)| c.group == group)
                                    .map(|(i, c)| swatch(i, c, agent, look, t, &this)),
                            ),
                    )
            }))
    }
}

/// Applies `look` to `agent`'s cat and saves it.
fn choose(
    this: &WeakEntity<AppView>,
    agent: Provider,
    look: Look,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let this = this.clone();
    move |_, _, cx| {
        this.update(cx, |v, cx| {
            v.settings.set(agent, look);
            cx.notify();
        })
        .ok();
    }
}

pub(super) fn toggled(on: bool) -> Toggled {
    if on { Toggled::True } else { Toggled::False }
}

fn agent_switch(agent: Provider, t: Theme, this: &WeakEntity<AppView>) -> Stateful<Div> {
    div()
        .id("agents")
        .role(Role::TabList)
        .aria_label("Agent")
        .flex()
        .gap(px(sp::XS))
        .p(px(3.))
        .rounded(px(10.))
        .bg(rgb(t.chrome))
        .border_1()
        .border_color(rgb(t.border))
        .children(AGENTS.map(|a| {
            let this = this.clone();
            div()
                .id(a.as_str())
                .role(Role::Tab)
                .aria_label(a.label())
                .aria_selected(a == agent)
                .px(px(sp::M))
                .py(px(sp::XS))
                .rounded(px(8.))
                .with_focus_ring(t)
                .when(a == agent, |d| {
                    d.bg(rgb(t.card)).font_weight(FontWeight::BOLD)
                })
                .child(a.label())
                .on_click(move |_, _, cx| {
                    this.update(cx, |v, cx| {
                        v.cats_agent = a;
                        cx.notify();
                    })
                    .ok();
                })
        }))
}

/// "Coat's own" and the seven eye colours.
fn eyes(agent: Provider, look: Look, t: Theme, this: &WeakEntity<AppView>) -> Stateful<Div> {
    let own = (None, "Coat's own", look.coat.palette.eye);
    let options = std::iter::once(own).chain(EYES.iter().map(|e| (Some(e), e.name, e.color)));
    div()
        .id("eyes")
        .role(Role::RadioGroup)
        .aria_label("Eye colour")
        .flex()
        .flex_wrap()
        .gap(px(sp::S))
        .children(options.enumerate().map(|(i, (eye, name, color))| {
            let on = look.eye == eye;
            div()
                .id(("eye", i))
                .role(Role::RadioButton)
                .aria_label(name)
                .aria_toggled(toggled(on))
                .flex()
                .items_center()
                .gap(px(sp::XS))
                .px(px(sp::S))
                .py(px(sp::XS))
                .rounded_full()
                .with_focus_ring(t)
                .border_2()
                .border_color(rgb(if on { t.text } else { t.keyline }))
                .bg(rgb(t.card))
                .when(on, |d| d.font_weight(FontWeight::BOLD))
                .hover(|s| s.bg(rgb(t.surface)))
                .text_size(px(ty::LABEL))
                .child(
                    div()
                        .size(px(EYE_DOT))
                        .rounded_full()
                        .bg(rgba(color))
                        .border_1()
                        .border_color(rgb(t.border)),
                )
                .child(name)
                .on_click(choose(this, agent, Look { eye, ..look }))
        }))
}

/// One coat as a small roster cell: its sitting cat, in the eye colour
/// currently chosen, on the agent's stage.
fn swatch(
    i: usize,
    coat: &'static Coat,
    agent: Provider,
    look: Look,
    t: Theme,
    this: &WeakEntity<AppView>,
) -> Stateful<Div> {
    let on = std::ptr::eq(look.coat, coat);
    let shown = Look { coat, ..look };
    div()
        .id(("coat", i))
        .role(Role::RadioButton)
        .aria_label(coat.name)
        .aria_toggled(toggled(on))
        .w(px(SWATCH_W))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(sp::XS))
        .p(px(6.))
        .rounded(px(14.))
        .with_focus_ring(t)
        .border_2()
        .bg(rgb(t.card))
        .border_color(rgb(if on { t.text } else { t.keyline }))
        .hover(|s| s.bg(rgb(t.surface)))
        .child(
            div()
                .w_full()
                .h(px(48.))
                .rounded(px(10.))
                .bg(rgb(t.stage(agent)))
                .flex()
                .items_end()
                .justify_center()
                .pb(px(6.))
                .child(div().ml(px(-cat_overhang(SWATCH_SCALE))).child(cat(
                    &frames(CatState::Sitting)[0],
                    shown,
                    SWATCH_SCALE,
                ))),
        )
        .child(
            div()
                .w_full()
                .text_center()
                .text_size(px(ty::LABEL))
                .when(on, |d| d.font_weight(FontWeight::BOLD))
                .child(coat.name),
        )
        .on_click(choose(this, agent, shown))
}
