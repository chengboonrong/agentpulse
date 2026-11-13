//! The Usage screen and a session's detail, drawn by hand like the rate chart:
//! bars as divs (so each can carry a tooltip), lines and dense series on a
//! canvas. Unknown is never drawn as zero: days before an agent's first stored
//! event are "no data", unpriced requests are counted beside the estimates.

use agentpulse_core::{Provider, RateWindow};
use chrono::{DateTime, Local, Offset, TimeZone};
use gpui::{
    Bounds, Div, Pixels, canvas, div, fill, point, prelude::*, px, relative, rgb, rgba, size,
};

use super::{
    AppView, CONTENT_PAD, Theme, Tooltip, axis_label, axis_max, card, heading, secondary,
    section_label, sp, ty,
};
use crate::collector::{Day, Key, Snapshot, USAGE_DAYS, UsageCharts};
use crate::format::{self, DASH};
use gpui::{FontWeight, ScrollHandle, WeakEntity};

fn notes_gap(notes: Vec<Div>) -> impl Iterator<Item = Div> {
    notes.into_iter().map(|n| n.mt(px(sp::M)))
}

const BAR_CHART_H: f32 = 120.;
const DETAIL_CHART_H: f32 = 140.;
const AXIS_W: f32 = 48.;
const LANE_LABEL_W: f32 = 176.;
const LANE_H: f32 = 22.;

/// Series colour per agent, at least 3:1 against the cards in both themes.
fn agent_color(p: Provider, t: Theme) -> u32 {
    match p {
        Provider::ClaudeCode => t.series_claude,
        _ => t.series_codex,
    }
}

fn scroll_page(id: &'static str, scroll: &ScrollHandle) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .track_scroll(scroll)
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px(px(CONTENT_PAD))
        .pt(px(sp::M))
        .pb(px(sp::XL))
        .flex()
        .flex_col()
}

fn swatch(color: u32) -> Div {
    div().size(px(10.)).flex_none().rounded_sm().bg(rgb(color))
}

fn legend(items: Vec<(Div, &'static str)>, t: Theme) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap(px(sp::M))
        .children(items.into_iter().map(|(sw, label)| {
            div()
                .flex()
                .items_center()
                .gap(px(sp::XS))
                .child(sw)
                .child(secondary(label, t))
        }))
}

/// A faint dotted baseline: no data stored, unlike a measured zero (nothing).
fn no_data_mark(t: Theme) -> Div {
    div()
        .flex()
        .justify_center()
        .gap(px(3.))
        .children((0..3).map(move |_| {
            div()
                .size(px(2.))
                .rounded_full()
                .bg(rgba((t.muted << 8) | 0x80))
        }))
}

fn no_data_swatch(t: Theme) -> Div {
    div()
        .size(px(10.))
        .flex_none()
        .flex()
        .flex_col()
        .justify_end()
        .pb(px(1.))
        .child(no_data_mark(t).gap(px(2.)))
}

/// A big figure over a muted explanation.
fn figure(value: String, note: &str, t: Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(ty::PLATE))
                .font_weight(FontWeight::EXTRA_BOLD)
                .child(value),
        )
        .child(secondary(note, t))
}

/// Y-axis labels beside a chart of height `h`: the top value and zero.
fn y_axis(top: String, h: f32, t: Theme) -> Div {
    div()
        .w(px(AXIS_W))
        .h(px(h + sp::L))
        .mt(px(-sp::S))
        .flex_none()
        .flex()
        .flex_col()
        .justify_between()
        .items_end()
        .text_size(px(ty::LABEL))
        .line_height(px(sp::L))
        .text_color(rgb(t.muted))
        .child(top)
        .child("0")
}

/// Labels under a chart, spread from its left edge to its right.
fn x_axis(labels: Vec<String>, t: Theme) -> Div {
    div()
        .ml(px(AXIS_W + sp::S))
        .flex()
        .justify_between()
        .text_size(px(ty::LABEL))
        .text_color(rgb(t.muted))
        .children(labels)
}

fn date_label(ms: i64, fmt: &str) -> String {
    DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&Local).format(fmt).to_string())
        .unwrap_or_else(|| DASH.into())
}

/// One column per day: the bar (or the no-data slot) with its tooltip.
fn day_columns(
    id: &'static str,
    days: &[Day],
    max: Option<f32>,
    t: Theme,
    column: impl Fn(&Day, f32) -> (Vec<(u32, f32)>, String),
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .h(px(BAR_CHART_H))
        .flex()
        .gap(px(2.))
        .border_b_1()
        .border_color(rgb(t.muted))
        .children(days.iter().enumerate().map(|(i, d)| {
            let no_data = d.agents.iter().all(|(_, u)| u.is_none());
            let scale = max.map_or(0., |m| BAR_CHART_H / m);
            let (segments, tip) = column(d, scale);
            let tip: gpui::SharedString = tip.into();
            div()
                .id((id, i))
                .flex_1()
                .h_full()
                .flex()
                .flex_col()
                .justify_end()
                .when(no_data, |c| c.child(no_data_mark(t).mb(px(3.))))
                .children(
                    segments
                        .into_iter()
                        .filter(|(_, h)| *h > 0.)
                        .map(|(color, h)| {
                            // Stacked segments keep a hairline between them.
                            div()
                                .h(px(h.max(1.)))
                                .flex_none()
                                .border_t_1()
                                .border_color(rgb(t.card))
                                .bg(rgb(color))
                        }),
                )
                .tooltip(move |_, cx| {
                    cx.new(|_| Tooltip {
                        text: tip.clone(),
                        t,
                    })
                    .into()
                })
        }))
}

fn day_x_axis(days: &[Day], t: Theme) -> Div {
    let label = |i: usize| {
        days.get(i)
            .map_or(String::new(), |d| date_label(d.start_ms, "%b %-d"))
    };
    x_axis(vec![label(0), label(days.len() / 2), "Today".into()], t)
}

// ------------------------------------------------------------------- usage

pub(super) fn usage(
    s: &Snapshot,
    notes: Vec<Div>,
    t: Theme,
    scroll: &ScrollHandle,
) -> impl IntoElement {
    let page = scroll_page("usage", scroll)
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_baseline()
                .gap(px(sp::M))
                .child(heading("Usage"))
                .child(secondary(
                    &format!("Last {USAGE_DAYS} days. Costs are estimates at API list prices."),
                    t,
                )),
        )
        .children(notes_gap(notes))
        .child(today_tokens(s, t).mt(px(sp::L)));
    match &s.usage {
        None => {
            page.child(secondary("Usage appears once history is indexed.", t).mt(px(sp::GROUP)))
        }
        Some(u) => page
            .child(card(t).mt(px(sp::XL)).child(daily_cost(u, t)))
            .child(card(t).mt(px(sp::L)).child(cache(u, t)))
            .child(card(t).mt(px(sp::L)).child(timeline(u, s.taken_at_ms, t)))
            .child(card(t).mt(px(sp::L)).child(limits(s, t))),
    }
}

/// Today's tokens by kind, all agents: one line under the title.
fn today_tokens(s: &Snapshot, t: Theme) -> Div {
    let tk = s.today_tokens;
    let pair = |label: &'static str, value: u64| {
        div()
            .flex()
            .gap(px(sp::XS))
            .child(secondary(label, t))
            .child(
                div()
                    .text_size(px(ty::LABEL))
                    .font_weight(FontWeight::BOLD)
                    .child(format::tokens(value)),
            )
    };
    div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap_x(px(sp::L))
        .gap_y(px(sp::XS))
        .child(section_label("Today", t))
        .child(pair("Input", tk.input))
        .child(pair("Output", tk.output))
        .child(pair("Cache write", tk.cache_write_5m + tk.cache_write_1h))
        .child(pair("Cache read", tk.cache_read))
        .children((s.today_unpriced_events > 0).then(|| {
            secondary(
                &format!("{} responses unpriced", s.today_unpriced_events),
                t,
            )
        }))
}

/// Sum over agents with data; None when no agent has data that day.
fn day_cost(d: &Day) -> Option<(f64, u64)> {
    d.agents
        .iter()
        .filter_map(|(_, u)| u.as_ref())
        .map(|u| (u.cost_usd, u.unpriced_events))
        .reduce(|a, b| (a.0 + b.0, a.1 + b.1))
}

fn unpriced_note(n: u64) -> String {
    format!(
        "+ {n} unpriced {} (model or tier without a price, not in the figure)",
        if n == 1 { "request" } else { "requests" }
    )
}

fn daily_cost(u: &UsageCharts, t: Theme) -> Div {
    let costs: Vec<f32> = u
        .days
        .iter()
        .map(|d| day_cost(d).map_or(0., |c| c.0 as f32))
        .collect();
    let max = axis_max(&costs);
    let total: f64 = u.days.iter().filter_map(day_cost).map(|c| c.0).sum();
    let unpriced: u64 = u.days.iter().filter_map(day_cost).map(|c| c.1).sum();
    let any_data = u.days.iter().any(|d| day_cost(d).is_some());
    let today = u.days.last();
    let today_line = today.map(|d| {
        d.agents
            .iter()
            .map(|(p, x)| {
                let v = x.as_ref().map_or(DASH.into(), |x| approx(x.cost_usd));
                format!("{} {v}", p.label())
            })
            .collect::<Vec<_>>()
            .join(" · ")
    });
    let mut legend_items: Vec<(Div, &'static str)> = u
        .days
        .first()
        .map(|d| {
            d.agents
                .iter()
                .map(|(p, _)| (swatch(agent_color(*p, t)), p.label()))
                .collect()
        })
        .unwrap_or_default();
    legend_items.push((no_data_swatch(t), "No data stored"));
    div()
        .flex()
        .flex_col()
        .gap(px(sp::S))
        .child(section_label("Daily estimated cost", t))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(sp::XL))
                .child(figure(
                    if any_data { approx(total) } else { DASH.into() },
                    &format!("{USAGE_DAYS} days, estimate at API list prices"),
                    t,
                ))
                .children(today_line.map(|l| figure(l, "today", t))),
        )
        .child(legend(legend_items, t))
        .child(
            div()
                .mt(px(sp::S))
                .flex()
                .gap(px(sp::S))
                .child(y_axis(
                    max.map_or(String::new(), |m| format!("${}", axis_label(m))),
                    BAR_CHART_H,
                    t,
                ))
                .child(day_columns("cost-day", &u.days, max, t, |d, scale| {
                    let date = date_label(d.start_ms, "%a %b %-d");
                    let Some((usd, unpriced)) = day_cost(d) else {
                        return (vec![], format!("{date} · no data stored"));
                    };
                    let parts: Vec<String> = d
                        .agents
                        .iter()
                        .map(|(p, x)| {
                            let v = x.as_ref().map_or("no data".into(), |x| approx(x.cost_usd));
                            format!("{} {v}", p.label())
                        })
                        .collect();
                    let mut tip = format!("{date} · {} ({})", approx(usd), parts.join(", "));
                    if unpriced > 0 {
                        tip += &format!(" · + {unpriced} unpriced");
                    }
                    // Drawn bottom up: the first agent sits on the axis.
                    let segs = d
                        .agents
                        .iter()
                        .rev()
                        .filter_map(|(p, x)| {
                            x.as_ref()
                                .map(|x| (agent_color(*p, t), x.cost_usd as f32 * scale))
                        })
                        .collect();
                    (segs, tip)
                })),
        )
        .child(day_x_axis(&u.days, t))
        .children((unpriced > 0).then(|| secondary(&unpriced_note(unpriced), t)))
        .child(secondary("Hover a day for its total.", t))
}

/// Estimate with the ≈ sign, as everywhere else.
fn approx(usd: f64) -> String {
    format::approx_cost(agentpulse_core::Reading::Estimated(usd)).unwrap_or_default()
}

/// Cache reads as a share of input-side tokens (input + cache writes + cache
/// reads), over every agent with data that day. None without such tokens.
fn cache_share(d: &Day) -> Option<f64> {
    let (read, all) = d
        .agents
        .iter()
        .filter_map(|(_, u)| u.as_ref())
        .fold((0u64, 0u64), |(r, a), u| {
            (r + u.cache_read, a + u.input + u.cache_write + u.cache_read)
        });
    (all > 0).then(|| read as f64 / all as f64)
}

fn cache(u: &UsageCharts, t: Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(sp::S))
        .child(section_label("Cache efficiency", t))
        .child(figure(
            match u
                .cache_saved_usd
                .filter(|_| u.days.iter().any(|d| day_cost(d).is_some()))
            {
                // A net loss when cache writes cost more than reads saved.
                Some(usd) if usd < 0. => {
                    format!("Net saved by caching ≈ −{}", format::cost(-usd))
                }
                Some(usd) => format!("Net saved by caching {}", approx(usd)),
                None => format!("Net saved by caching {DASH}"),
            },
            &format!(
                "estimate over {USAGE_DAYS} days: what cache reads saved against input, minus what cache writes cost above input, at standard rates"
            ),
            t,
        ))
        .children((u.cache_saved_unpriced > 0).then(|| {
            secondary(
                &format!(
                    "{} cache-using {} without a price (model or tier) are not counted",
                    u.cache_saved_unpriced,
                    if u.cache_saved_unpriced == 1 { "request" } else { "requests" }
                ),
                t,
            )
        }))
        .child(secondary(
            "Bars: cache reads as a share of input-side tokens (input, cache writes and cache reads), all agents.",
            t,
        ))
        .child(
            div()
                .mt(px(sp::S))
                .flex()
                .gap(px(sp::S))
                .child(y_axis("100%".into(), BAR_CHART_H, t))
                .child(day_columns("cache-day", &u.days, Some(1.), t, |d, scale| {
                    let date = date_label(d.start_ms, "%a %b %-d");
                    if d.agents.iter().all(|(_, x)| x.is_none()) {
                        return (vec![], format!("{date} · no data stored"));
                    }
                    match cache_share(d) {
                        Some(share) => (
                            vec![(t.muted, share as f32 * scale)],
                            format!("{date} · {:.0}% of input-side tokens read from cache", share * 100.),
                        ),
                        None => (vec![], format!("{date} · no requests")),
                    }
                })),
        )
        .child(day_x_axis(&u.days, t))
}

/// Hour ticks for a local-time axis from `start` to `end`: at most about six.
fn hour_ticks(start: i64, end: i64) -> Vec<i64> {
    const HOUR: i64 = 3_600_000;
    let span_h = ((end - start) / HOUR).max(1);
    let step = [1, 2, 3, 4, 6, 12]
        .into_iter()
        .find(|s| span_h / s <= 6)
        .unwrap_or(24);
    // Ticks on whole local hours (time zones with half-hour offsets included).
    let offset = DateTime::from_timestamp_millis(start).map_or(0, |t| {
        Local
            .offset_from_utc_datetime(&t.naive_utc())
            .local_minus_utc() as i64
            * 1000
    });
    let first = (start + offset + HOUR * step - 1).div_euclid(HOUR * step) * HOUR * step - offset;
    (0..)
        .map(|i| first + i * HOUR * step)
        .take_while(|&x| x < end)
        .collect()
}

/// The start of the local hour (in `tz`) holding `ms`; whole UTC hours are
/// not local hours in zones with half-hour offsets.
fn local_hour_start<Tz: TimeZone>(ms: i64, tz: &Tz) -> i64 {
    let offset = DateTime::from_timestamp_millis(ms).map_or(0, |t| {
        tz.offset_from_utc_datetime(&t.naive_utc())
            .fix()
            .local_minus_utc() as i64
            * 1000
    });
    (ms + offset).div_euclid(3_600_000) * 3_600_000 - offset
}

fn timeline(u: &UsageCharts, now: i64, t: Theme) -> Div {
    let section = div()
        .flex()
        .flex_col()
        .gap(px(sp::S))
        .child(section_label("Today", t))
        .child(secondary(
            "One lane per session with requests today, labelled with its first request's time. Solid where API responses are at most 60 s apart, faint between.",
            t,
        ));
    let Some(first) = u.today.iter().map(|(_, l)| l.runs[0].0).min() else {
        return section.child(secondary("No requests today.", t));
    };
    // From the whole local hour before the first request to now.
    let start = local_hour_start(first, &Local);
    let end = now.max(first + 1);
    let span = (end - start) as f32;
    let ticks = hour_ticks(start, end);
    let lanes = u.today.iter().map(|(p, lane)| {
        let color = agent_color(*p, t);
        let runs = lane.runs.clone();
        let faint = rgb(t.border);
        div()
            .h(px(LANE_H))
            .flex()
            .items_center()
            .gap(px(sp::S))
            .child(
                div()
                    .w(px(LANE_LABEL_W))
                    .flex_none()
                    .flex()
                    .text_size(px(ty::LABEL))
                    // The time stays visible when a long name is cut.
                    .child(div().min_w_0().truncate().child(format!(
                        "{} · {}",
                        p.label(),
                        format::project_name(lane.project_path.as_deref())
                    )))
                    .child(
                        div()
                            .flex_none()
                            .child(format!(" · {}", date_label(lane.runs[0].0, "%H:%M"))),
                    ),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, _| {
                        // Clock skew can put a request after now: keep it on the canvas.
                        let x = |ms: i64| {
                            b.left() + b.size.width * ((ms - start) as f32 / span).clamp(0., 1.)
                        };
                        let mid = b.top() + b.size.height / 2.;
                        let (first, last) = (runs[0].0, runs[runs.len() - 1].1);
                        window.paint_quad(fill(
                            Bounds::new(
                                point(x(first), mid - px(1.)),
                                size(x(last) - x(first), px(2.)),
                            ),
                            faint,
                        ));
                        for (a, z) in &runs {
                            let w = (x(*z) - x(*a)).max(px(2.));
                            window.paint_quad(
                                fill(
                                    Bounds::new(point(x(*a), mid - px(5.)), size(w, px(10.))),
                                    rgb(color),
                                )
                                .corner_radii(px(2.)),
                            );
                        }
                    },
                )
                .flex_1()
                .h_full(),
            )
    });
    let axis = div()
        .ml(px(LANE_LABEL_W + sp::S))
        .relative()
        .h(px(sp::L))
        .border_t_1()
        .border_color(rgb(t.muted))
        .text_size(px(ty::LABEL))
        .text_color(rgb(t.muted))
        .children(
            ticks
                .into_iter()
                .map(|ms| (ms, (ms - start) as f32 / span))
                // The last label needs room to its right.
                .filter(|(_, f)| *f <= 0.9)
                .map(|(ms, f)| {
                    div()
                        .absolute()
                        .left(relative(f))
                        .top(px(2.))
                        .child(date_label(ms, "%H:%M"))
                }),
        )
        .child(div().absolute().right(px(0.)).top(px(2.)).child("now"));
    section
        .child(div().flex().flex_col().children(lanes))
        .child(axis)
        .child(secondary("Local time.", t))
}

// ------------------------------------------------------------- plan limits

/// "weekly", "5-hour", "daily": the window as people name it.
pub(super) fn window_name(minutes: Option<u64>) -> String {
    match minutes {
        Some(10_080) => "weekly".into(),
        Some(1_440) => "daily".into(),
        Some(m) if m > 0 && m % 1_440 == 0 => format!("{}-day", m / 1_440),
        Some(m) if m > 0 && m % 60 == 0 => format!("{}-hour", m / 60),
        Some(m) => format!("{m}-minute"),
        None => "plan".into(),
    }
}

/// "Fri 14:00" within a week, "Nov 21 14:00" beyond.
fn reset_label<Tz: TimeZone>(at_s: i64, now_ms: i64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(at) = DateTime::from_timestamp(at_s, 0) else {
        return DASH.into();
    };
    let fmt = if (at_s * 1000 - now_ms).abs() < 6 * 86_400_000 {
        "%a %H:%M"
    } else {
        "%b %-d %H:%M"
    };
    at.with_timezone(tz).format(fmt).to_string()
}

fn limit_row(w: &RateWindow, plan: Option<&str>, as_of: i64, now: i64, t: Theme) -> Div {
    let stale = w.resets_at_s.is_some_and(|r| r * 1000 < now);
    let pct = w.used_percent.clamp(0., 100.);
    let color = if stale {
        t.muted
    } else if pct >= 90. {
        t.failed
    } else if pct >= 75. {
        t.waiting
    } else {
        t.series_codex
    };
    let mut detail = vec![];
    if let Some(r) = w.resets_at_s {
        detail.push(format!("resets {}", reset_label(r, now, &Local)));
    }
    if let Some(p) = plan {
        detail.push(format!("plan {p}"));
    }
    detail.push(format!("as of {}", format::start_time(as_of, now)));
    div()
        .flex()
        .flex_col()
        .gap(px(sp::XS))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(sp::XS))
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .child(format!("{:.0}%", w.used_percent)),
                )
                .child(format!("of {} limit used", window_name(w.window_minutes))),
        )
        .child(
            div()
                .h(px(8.))
                .w_full()
                .rounded_full()
                .bg(rgb(t.border))
                .child(div().h_full().w(relative(pct as f32 / 100.)).rounded_full().bg(rgb(color))),
        )
        .child(secondary(&detail.join(" · "), t))
        .children(stale.then(|| {
            secondary(
                "Out of date: the reset time has passed. Codex sends a new reading with its next request.",
                t,
            )
        }))
}

fn limits(s: &Snapshot, t: Theme) -> Div {
    let section = div()
        .flex()
        .flex_col()
        .gap(px(sp::M))
        .child(section_label("Plan limits", t));
    let codex = match &s.codex_limits {
        None => secondary(
            "No Codex limit data yet. Codex reports it with each request.",
            t,
        ),
        Some(l) => {
            let windows: Vec<&RateWindow> = l.primary.iter().chain(&l.secondary).collect();
            if windows.is_empty() {
                secondary("Codex's latest reading had no limit windows.", t)
            } else {
                div().flex().flex_col().gap(px(sp::M)).children(
                    windows
                        .into_iter()
                        .map(|w| limit_row(w, l.plan_type.as_deref(), l.ts_ms, s.taken_at_ms, t)),
                )
            }
        }
    };
    section
        .child(
            div()
                .max_w(px(480.))
                .flex()
                .flex_col()
                .gap(px(sp::S))
                .child(div().font_weight(FontWeight::BOLD).child("Codex"))
                .child(codex),
        )
        .child(secondary(
            "Claude Code writes no plan-limit data to disk, so there is nothing to show for it.",
            t,
        ))
}

// ---------------------------------------------------------- session detail

/// The back link's "‹", drawn so it does not depend on a font's glyph.
fn chevron_left(color: u32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let mut path = gpui::PathBuilder::stroke(px(1.75));
            path.move_to(point(b.right() - px(1.), b.top() + px(1.)));
            path.line_to(point(b.left() + px(1.), b.top() + b.size.height / 2.));
            path.line_to(point(b.right() - px(1.), b.bottom() - px(1.)));
            if let Ok(path) = path.build() {
                window.paint_path(path, rgb(color));
            }
        },
    )
    .w(px(7.))
    .h(px(12.))
}

pub(super) fn session_detail(
    s: &Snapshot,
    key: &Key,
    profile: Div,
    notes: Vec<Div>,
    t: Theme,
    view: WeakEntity<AppView>,
    scroll: &ScrollHandle,
) -> impl IntoElement {
    let back = div()
        .id("back")
        .role(gpui::Role::Link)
        .tab_index(0)
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_color(rgb(t.text))
        .font_weight(FontWeight::MEDIUM)
        .focus_visible(|d| d.underline())
        .child(chevron_left(t.text))
        .child("Sessions")
        .on_click(move |_, _, cx| {
            view.update(cx, |v, cx| v.show_detail(None, cx)).ok();
        });
    let page = scroll_page("detail", scroll)
        .child(back)
        .child(profile.mt(px(sp::M)))
        .children(notes_gap(notes));
    let detail = s.detail.as_ref().filter(|d| &d.key == key);
    let Some(d) = detail else {
        return page.child(secondary("Loading requests…", t).mt(px(sp::XL)));
    };
    if d.requests.is_empty() {
        return page.child(secondary("No requests recorded for this session.", t).mt(px(sp::XL)));
    }
    let n = d.requests.len();
    let color = agent_color(key.0, t);
    let first = d.requests[0].ts_ms;
    let last = d.requests[n - 1].ts_ms;
    let x_labels = vec![
        format!("request 1 · {}", format::start_time(first, s.taken_at_ms)),
        format!("request {n} · {}", format::start_time(last, s.taken_at_ms)),
    ];

    // (a) context size per request.
    let sizes: Vec<f32> = d.requests.iter().map(|r| r.prompt_tokens as f32).collect();
    let peak: Vec<f32> = sizes
        .iter()
        .copied()
        .chain(d.long_context.iter().map(|&x| x as f32))
        .collect();
    let max = axis_max(&peak);
    let thresholds: Vec<f32> = d.long_context.iter().map(|&x| x as f32).collect();
    let context = div()
        .flex()
        .flex_col()
        .gap(px(sp::S))
        .child(section_label("Context size per request", t))
        .child(secondary(
            "Input, cache writes and cache reads sent with each request.",
            t,
        ))
        .children(d.long_context.iter().map(|&x| {
            div()
                .flex()
                .items_center()
                .gap(px(sp::XS))
                .child(div().w(px(16.)).h(px(2.)).bg(rgb(t.waiting)))
                .child(secondary(
                    &format!(
                        "{}: long-context rates apply above this line",
                        format::tokens(x)
                    ),
                    t,
                ))
        }))
        .child(
            div()
                .mt(px(sp::S))
                .flex()
                .gap(px(sp::S))
                .child(y_axis(
                    max.map_or(String::new(), |m| format::tokens(m as u64)),
                    DETAIL_CHART_H,
                    t,
                ))
                .child(series_chart(sizes, thresholds, max, false, color, t)),
        )
        .child(x_axis(x_labels.clone(), t));

    // (b) cumulative estimated cost; unpriced requests add nothing.
    let mut sum = 0.;
    let cumulative: Vec<f32> = d
        .requests
        .iter()
        .map(|r| {
            sum += r.cost_usd.unwrap_or(0.);
            sum as f32
        })
        .collect();
    let unpriced = d.requests.iter().filter(|r| r.cost_usd.is_none()).count() as u64;
    let max = axis_max(&cumulative);
    let cost = div()
        .flex()
        .flex_col()
        .gap(px(sp::S))
        .child(section_label("Cumulative estimated cost", t))
        .child(figure(
            if unpriced as usize == n {
                DASH.into()
            } else {
                approx(sum)
            },
            &format!("{n} requests, estimate at API list prices"),
            t,
        ))
        .children((unpriced > 0).then(|| secondary(&unpriced_note(unpriced), t)))
        .child(
            div()
                .mt(px(sp::S))
                .flex()
                .gap(px(sp::S))
                .child(y_axis(
                    max.map_or(String::new(), |m| format!("${}", axis_label(m))),
                    DETAIL_CHART_H,
                    t,
                ))
                .child(series_chart(cumulative, vec![], max, true, color, t)),
        )
        .child(x_axis(x_labels, t));
    page.child(card(t).mt(px(sp::XL)).child(context))
        .child(card(t).mt(px(sp::L)).child(cost))
}

/// The largest value of each of `columns` equal slices of `values`, so a long
/// series paints one bar per pixel column at most.
fn bucket_max(values: &[f32], columns: usize) -> Vec<f32> {
    if columns == 0 || values.len() <= columns {
        return values.to_vec();
    }
    (0..columns)
        .map(|c| {
            let (a, z) = (c * values.len() / columns, (c + 1) * values.len() / columns);
            values[a..z.max(a + 1)].iter().copied().fold(0., f32::max)
        })
        .collect()
}

/// One value per request, left to right: bars, or a line when `line`. Dashed
/// horizontal lines mark `marks`.
fn series_chart(
    values: Vec<f32>,
    marks: Vec<f32>,
    max: Option<f32>,
    line: bool,
    color: u32,
    t: Theme,
) -> impl IntoElement {
    let (fg, mark, grid) = (rgb(color), rgb(t.waiting), rgb(t.border));
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let hairline = |y: Pixels, color| {
                fill(
                    Bounds::new(point(b.left(), y), size(b.size.width, px(1.))),
                    color,
                )
            };
            window.paint_quad(hairline(b.bottom() - px(1.), rgb(t.muted)));
            let Some(max) = max else { return };
            window.paint_quad(hairline(b.top(), grid));
            let h = b.size.height - px(1.);
            let y = |v: f32| b.bottom() - px(1.) - h * (v / max);
            if line {
                let n = values.len().max(2) - 1;
                let mut path = gpui::PathBuilder::stroke(px(1.5));
                for (i, v) in values.iter().enumerate() {
                    let p = point(b.left() + b.size.width * (i as f32 / n as f32), y(*v));
                    if i == 0 {
                        path.move_to(p);
                    } else {
                        path.line_to(p);
                    }
                }
                if values.len() == 1 {
                    path.line_to(point(b.right(), y(values[0])));
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, fg);
                }
            } else {
                let bars = bucket_max(&values, f32::from(b.size.width) as usize / 2);
                let step = b.size.width / bars.len() as f32;
                let w = (step - px(1.)).max(px(1.));
                for (i, v) in bars.iter().enumerate() {
                    let top = y(*v);
                    window.paint_quad(fill(
                        Bounds::new(
                            point(b.left() + step * i as f32, top),
                            size(w, b.bottom() - px(1.) - top),
                        ),
                        fg,
                    ));
                }
            }
            for m in &marks {
                let my = y(*m);
                let mut x = b.left();
                while x < b.right() {
                    let w = px(6.).min(b.right() - x);
                    window.paint_quad(fill(
                        Bounds::new(point(x, my - px(1.)), size(w, px(2.))),
                        mark,
                    ));
                    x += px(10.);
                }
            }
        },
    )
    .flex_1()
    .min_w_0()
    .h(px(DETAIL_CHART_H))
}

#[cfg(test)]
mod tests {
    use agentpulse_storage::DayUsage;
    use chrono::FixedOffset;

    use super::*;

    fn day(agents: Vec<(Provider, Option<DayUsage>)>) -> Day {
        Day {
            start_ms: 0,
            agents,
        }
    }

    fn used(cost: f64, unpriced: u64, input: u64, write: u64, read: u64) -> Option<DayUsage> {
        Some(DayUsage {
            day: "2026-11-13".into(),
            cost_usd: cost,
            priced_events: 1,
            unpriced_events: unpriced,
            input,
            cache_write: write,
            cache_read: read,
        })
    }

    #[test]
    fn no_data_is_not_zero() {
        let none = day(vec![(Provider::ClaudeCode, None), (Provider::Codex, None)]);
        assert_eq!(day_cost(&none), None);
        assert_eq!(cache_share(&none), None);
        let zero = day(vec![
            (Provider::ClaudeCode, used(0., 0, 0, 0, 0)),
            (Provider::Codex, None),
        ]);
        assert_eq!(day_cost(&zero), Some((0., 0)));
        assert_eq!(cache_share(&zero), None, "no requests: no share either");
        let both = day(vec![
            (Provider::ClaudeCode, used(1.5, 2, 10, 10, 60)),
            (Provider::Codex, used(0.5, 1, 20, 0, 0)),
        ]);
        assert_eq!(day_cost(&both), Some((2.0, 3)));
        assert_eq!(cache_share(&both), Some(0.6));
    }

    #[test]
    fn window_names() {
        assert_eq!(window_name(Some(10_080)), "weekly");
        assert_eq!(window_name(Some(300)), "5-hour");
        assert_eq!(window_name(Some(1_440)), "daily");
        assert_eq!(window_name(Some(2_880)), "2-day");
        assert_eq!(window_name(Some(45)), "45-minute");
        assert_eq!(window_name(None), "plan");
    }

    #[test]
    fn reset_labels() {
        let tz = FixedOffset::east_opt(8 * 3600).unwrap();
        // 2026-11-13 02:00 UTC = Fri 10:00 at +08:00.
        let at = 1_794_535_200;
        assert_eq!(reset_label(at, (at - 3_600) * 1000, &tz), "Fri 10:00");
        assert_eq!(
            reset_label(at, (at - 7 * 86_400) * 1000, &tz),
            "Nov 13 10:00"
        );
    }

    #[test]
    fn buckets_keep_peaks() {
        assert_eq!(bucket_max(&[1., 2.], 4), [1., 2.]);
        assert_eq!(bucket_max(&[1., 5., 2., 3., 9.], 2), [5., 9.]);
        assert_eq!(bucket_max(&[3.; 1000], 7).len(), 7);
    }

    #[test]
    fn lanes_start_on_the_local_hour() {
        let india = FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
        // 2026-11-13 10:20 at +05:30 is 04:50 UTC.
        let at = india
            .with_ymd_and_hms(2026, 11, 13, 10, 20, 0)
            .unwrap()
            .timestamp_millis();
        let ten = india
            .with_ymd_and_hms(2026, 11, 13, 10, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(local_hour_start(at, &india), ten);
        assert_eq!(local_hour_start(ten, &india), ten);
        let utc = FixedOffset::east_opt(0).unwrap();
        assert_eq!(
            local_hour_start(at, &utc),
            at - 20 * 60_000 + 30 * 60_000 - 3_600_000
        );
    }

    #[test]
    fn hour_ticks_stay_inside_and_thin_out() {
        let h = 3_600_000;
        // A whole local hour.
        let start = Local
            .with_ymd_and_hms(2026, 11, 13, 10, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis();
        let ticks = hour_ticks(start, start + 3 * h);
        assert!(ticks.len() <= 3 && ticks.iter().all(|&x| x >= start && x < start + 3 * h));
        // A start on a whole hour gets its own label.
        assert_eq!(ticks[0], start);
        let ticks = hour_ticks(start, start + 23 * h);
        assert!(ticks.len() <= 6, "{}", ticks.len());
        assert!(ticks.windows(2).all(|w| w[1] - w[0] == 4 * h));
    }
}
