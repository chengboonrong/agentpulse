//! The Usage screen and a session's detail, drawn by hand like the rate chart:
//! bars as divs (so each can carry a tooltip), lines and dense series on a
//! canvas. Unknown is never drawn as zero: days before an agent's first stored
//! event are "no data", unpriced requests are counted beside the estimates.

use std::collections::{BTreeSet, HashMap};

use agentpulse_core::{Provider, RateLimits, RateWindow};
use agentpulse_storage::UsageGroup;
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

pub(super) fn scroll_page(id: &'static str, scroll: &ScrollHandle) -> gpui::Stateful<Div> {
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

fn legend(items: Vec<(Div, String)>, t: Theme) -> Div {
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
                .child(secondary(&label, t))
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
        days.get(i).map_or(String::new(), |d| {
            date_label(d.start_ms, &rust_i18n::t!("usage.date_short"))
        })
    };
    x_axis(
        vec![
            label(0),
            label(days.len() / 2),
            rust_i18n::t!("usage.today").to_string(),
        ],
        t,
    )
}

// ------------------------------------------------------------------- usage

pub(super) fn usage(
    s: &Snapshot,
    hidden: &BTreeSet<Provider>,
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
                .child(heading(rust_i18n::t!("usage.usage").to_string()))
                .child(secondary(
                    rust_i18n::t!("usage.period_note", n = USAGE_DAYS).as_ref(),
                    t,
                )),
        )
        .children(notes_gap(notes))
        .child(today_tokens(s, t).mt(px(sp::L)));
    match &s.usage {
        None => page.child(secondary(rust_i18n::t!("usage.indexed").as_ref(), t).mt(px(sp::GROUP))),
        Some(u) => page
            .child(card(t).mt(px(sp::XL)).child(daily_cost(u, t)))
            .child(card(t).mt(px(sp::L)).child(cache(u, t)))
            .child(card(t).mt(px(sp::L)).child(timeline(u, s.taken_at_ms, t)))
            .children(limits(s, hidden, t).map(|l| card(t).mt(px(sp::L)).child(l)))
            .children(u.breakdown.as_ref().map(|b| {
                div()
                    .child(
                        card(t).mt(px(sp::L)).child(breakdown_list(
                            rust_i18n::t!("usage.models").to_string(),
                            &b.models,
                            b.models
                                .iter()
                                .map(|r| r.key.clone().unwrap_or_default())
                                .collect(),
                            t,
                        )),
                    )
                    .child(card(t).mt(px(sp::L)).child(breakdown_list(
                        rust_i18n::t!("usage.projects").to_string(),
                        &b.projects,
                        project_labels(&b.projects),
                        t,
                    )))
            })),
    }
}

/// Shortest distinguishing folder suffix, stopping at the home-folder alias.
fn project_labels(rows: &[UsageGroup]) -> Vec<String> {
    let mut parents: Vec<_> = rows
        .iter()
        .map(|r| r.key.as_deref().map(std::path::Path::new))
        .collect();
    let mut labels: Vec<_> = rows
        .iter()
        .map(|r| match r.key.as_deref() {
            Some(path) => format::project_name(Some(path)).to_string(),
            None => rust_i18n::t!("usage.no_project").to_string(),
        })
        .collect();
    loop {
        let mut counts = HashMap::new();
        for label in &labels {
            *counts.entry(label.clone()).or_insert(0) += 1;
        }
        let mut changed = false;
        for (i, label) in labels.iter_mut().enumerate() {
            if counts[label] <= 1 {
                continue;
            }
            if let Some(path) = parents[i] {
                parents[i] = if format::project_name(path.to_str()) == "~" {
                    None
                } else {
                    path.parent()
                        .filter(|p| !p.as_os_str().is_empty() && p.parent().is_some())
                };
                if let Some(parent) = parents[i] {
                    *label = format!("{}/{label}", format::project_name(parent.to_str()));
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    // Reserve natural labels so generated numbers cannot collide with them.
    let mut used: BTreeSet<_> = labels.iter().cloned().collect();
    let mut counts = HashMap::new();
    for label in &labels {
        *counts.entry(label.clone()).or_insert(0) += 1;
    }
    let mut order: Vec<_> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| rows[a].key.cmp(&rows[b].key));
    let mut numbers = HashMap::new();
    for i in order {
        if counts[&labels[i]] > 1 {
            let n = numbers.entry(labels[i].clone()).or_insert(0);
            loop {
                *n += 1;
                let candidate = format!("{} ({n})", labels[i]);
                if used.insert(candidate.clone()) {
                    labels[i] = candidate;
                    break;
                }
            }
        }
    }
    labels
}

fn ranked_rows(rows: &[UsageGroup], labels: Vec<String>) -> Vec<(String, UsageGroup)> {
    let mut shown: Vec<_> = labels
        .into_iter()
        .zip(rows.iter().cloned())
        .take(8)
        .collect();
    if rows.len() > 8 {
        let mut other = UsageGroup::default();
        for row in &rows[8..] {
            other.tokens += row.tokens;
            other.cost_usd += row.cost_usd;
            other.unpriced_events += row.unpriced_events;
        }
        shown.push((rust_i18n::t!("usage.other").to_string(), other));
    }
    shown
}

fn breakdown_list(title: String, rows: &[UsageGroup], labels: Vec<String>, t: Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(sp::S))
        .child(section_label(title, t))
        .children(
            rows.is_empty()
                .then(|| secondary(rust_i18n::t!("usage.no_data_stored").as_ref(), t)),
        )
        .children(ranked_rows(rows, labels).into_iter().map(|(label, row)| {
            let mut value = format!(
                "{} · {}",
                rust_i18n::t!("usage.tokens", n = format::tokens(row.tokens)),
                approx(row.cost_usd)
            );
            if row.unpriced_events > 0 {
                value.push_str(&rust_i18n::t!(
                    "usage.unpriced_suffix",
                    n = row.unpriced_events
                ));
            }
            div()
                .flex()
                .items_center()
                .gap(px(sp::S))
                .text_size(px(ty::LABEL))
                .child(div().flex_1().min_w_0().truncate().child(label))
                .child(secondary(&value, t).flex_shrink_0())
        }))
}

/// Today's tokens by kind, all agents: one line under the title.
fn today_tokens(s: &Snapshot, t: Theme) -> Div {
    let tk = s.today_tokens;
    let pair = |label: &str, value: u64| {
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
        .child(section_label(rust_i18n::t!("usage.today").to_string(), t))
        .child(pair(rust_i18n::t!("usage.input").as_ref(), tk.input))
        .child(pair(rust_i18n::t!("usage.output").as_ref(), tk.output))
        .child(pair(
            rust_i18n::t!("usage.cache_write").as_ref(),
            tk.cache_write_5m + tk.cache_write_1h,
        ))
        .child(pair(
            rust_i18n::t!("usage.cache_read").as_ref(),
            tk.cache_read,
        ))
        .children((s.today_unpriced_events > 0).then(|| {
            secondary(
                rust_i18n::t!("usage.responses_unpriced", n = s.today_unpriced_events).as_ref(),
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
    rust_i18n::t!(
        if n == 1 {
            "usage.unpriced_one"
        } else {
            "usage.unpriced_many"
        },
        n = n
    )
    .to_string()
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
    let mut legend_items: Vec<(Div, String)> = u
        .days
        .first()
        .map(|d| {
            d.agents
                .iter()
                .map(|(p, _)| (swatch(agent_color(*p, t)), p.label().to_string()))
                .collect()
        })
        .unwrap_or_default();
    legend_items.push((
        no_data_swatch(t),
        rust_i18n::t!("usage.no_data_stored").to_string(),
    ));
    div()
        .flex()
        .flex_col()
        .gap(px(sp::S))
        .child(section_label(
            rust_i18n::t!("usage.daily_cost").to_string(),
            t,
        ))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(sp::XL))
                .child(figure(
                    if any_data { approx(total) } else { DASH.into() },
                    rust_i18n::t!("usage.days_estimate", n = USAGE_DAYS).as_ref(),
                    t,
                ))
                .children(
                    today_line.map(|l| figure(l, rust_i18n::t!("usage.today_lower").as_ref(), t)),
                ),
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
                    let date = date_label(d.start_ms, &rust_i18n::t!("usage.date_day"));
                    let Some((usd, unpriced)) = day_cost(d) else {
                        return (
                            vec![],
                            rust_i18n::t!("usage.date_no_data", date = date).to_string(),
                        );
                    };
                    let parts: Vec<String> = d
                        .agents
                        .iter()
                        .map(|(p, x)| {
                            let v = x
                                .as_ref()
                                .map_or(rust_i18n::t!("usage.no_data").to_string(), |x| {
                                    approx(x.cost_usd)
                                });
                            format!("{} {v}", p.label())
                        })
                        .collect();
                    let mut tip = format!("{date} · {} ({})", approx(usd), parts.join(", "));
                    if unpriced > 0 {
                        tip += rust_i18n::t!("usage.unpriced_suffix", n = unpriced).as_ref();
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
        .child(secondary(rust_i18n::t!("usage.hover_day").as_ref(), t))
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
        .child(section_label(
            rust_i18n::t!("usage.cache_efficiency").to_string(),
            t,
        ))
        .child(figure(
            match u
                .cache_saved_usd
                .filter(|_| u.days.iter().any(|d| day_cost(d).is_some()))
            {
                // A net loss when cache writes cost more than reads saved.
                Some(usd) if usd < 0. => rust_i18n::t!(
                    "usage.net_saved",
                    value = format!("≈ −{}", format::cost(-usd))
                )
                .to_string(),
                Some(usd) => rust_i18n::t!("usage.net_saved", value = approx(usd)).to_string(),
                None => rust_i18n::t!("usage.net_saved", value = DASH).to_string(),
            },
            rust_i18n::t!("usage.cache_estimate", n = USAGE_DAYS).as_ref(),
            t,
        ))
        .children((u.cache_saved_unpriced > 0).then(|| {
            secondary(
                rust_i18n::t!(
                    if u.cache_saved_unpriced == 1 {
                        "usage.cache_unpriced_one"
                    } else {
                        "usage.cache_unpriced_many"
                    },
                    n = u.cache_saved_unpriced
                )
                .as_ref(),
                t,
            )
        }))
        .child(secondary(rust_i18n::t!("usage.cache_bars").as_ref(), t))
        .child(
            div()
                .mt(px(sp::S))
                .flex()
                .gap(px(sp::S))
                .child(y_axis("100%".into(), BAR_CHART_H, t))
                .child(day_columns(
                    "cache-day",
                    &u.days,
                    Some(1.),
                    t,
                    |d, scale| {
                        let date = date_label(d.start_ms, &rust_i18n::t!("usage.date_day"));
                        if d.agents.iter().all(|(_, x)| x.is_none()) {
                            return (
                                vec![],
                                rust_i18n::t!("usage.date_no_data", date = date).to_string(),
                            );
                        }
                        match cache_share(d) {
                            Some(share) => (
                                vec![(t.muted, share as f32 * scale)],
                                rust_i18n::t!(
                                    "usage.cache_share",
                                    date = date,
                                    percent = format!("{:.0}", share * 100.)
                                )
                                .to_string(),
                            ),
                            None => (
                                vec![],
                                rust_i18n::t!("usage.date_no_requests", date = date).to_string(),
                            ),
                        }
                    },
                )),
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
        .child(section_label(rust_i18n::t!("usage.today").to_string(), t))
        .child(secondary(rust_i18n::t!("usage.lanes").as_ref(), t));
    let Some(first) = u.today.iter().map(|(_, l)| l.runs[0].0).min() else {
        return section.child(secondary(
            rust_i18n::t!("usage.no_requests_today").as_ref(),
            t,
        ));
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
        .child(
            div()
                .absolute()
                .right(px(0.))
                .top(px(2.))
                .child(rust_i18n::t!("usage.now").to_string()),
        );
    section
        .child(div().flex().flex_col().children(lanes))
        .child(axis)
        .child(secondary(rust_i18n::t!("usage.local_time").as_ref(), t))
}

// ------------------------------------------------------------- plan limits

/// "weekly", "5-hour", "daily": the window as people name it.
pub(super) fn window_name(minutes: Option<u64>) -> String {
    match minutes {
        Some(10_080) => rust_i18n::t!("usage.weekly").to_string(),
        Some(1_440) => rust_i18n::t!("usage.daily").to_string(),
        Some(m) if m > 0 && m % 1_440 == 0 => {
            rust_i18n::t!("usage.window_day", n = m / 1_440).to_string()
        }
        Some(m) if m > 0 && m % 60 == 0 => {
            rust_i18n::t!("usage.window_hour", n = m / 60).to_string()
        }
        Some(m) => rust_i18n::t!("usage.window_minute", n = m).to_string(),
        None => rust_i18n::t!("usage.plan").to_string(),
    }
}

/// "Fri 14:00" within a week, "Nov 21 14:00" beyond.
pub(super) fn reset_label<Tz: TimeZone>(at_s: i64, now_ms: i64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(at) = DateTime::from_timestamp(at_s, 0) else {
        return DASH.into();
    };
    let fmt = if (at_s * 1000 - now_ms).abs() < 6 * 86_400_000 {
        rust_i18n::t!("usage.reset_week")
    } else {
        rust_i18n::t!("usage.reset_date")
    };
    at.with_timezone(tz).format(&fmt).to_string()
}

/// A limit bar's fill: the agent's colour, warning colours as the window
/// fills, muted once out of date.
pub(super) fn limit_color(p: Provider, pct: f64, stale: bool, t: Theme) -> u32 {
    if stale {
        t.muted
    } else if pct >= 90. {
        t.failed
    } else if pct >= 75. {
        t.waiting
    } else {
        agent_color(p, t)
    }
}

fn limit_row(p: Provider, w: &RateWindow, l: &RateLimits, now: i64, t: Theme) -> Div {
    let (plan, as_of) = (l.plan_type.as_deref(), l.ts_ms);
    let stale = crate::menubar::out_of_date(w, now);
    let pct = w.used_percent.clamp(0., 100.);
    let color = limit_color(p, pct, stale, t);
    let mut detail = vec![];
    if let Some(r) = w.resets_at_s {
        detail.push(rust_i18n::t!("usage.resets", time = reset_label(r, now, &Local)).to_string());
    }
    if let Some(p) = plan {
        detail.push(rust_i18n::t!("usage.plan_name", name = p).to_string());
    }
    detail.push(rust_i18n::t!("usage.as_of", time = format::start_time(as_of, now)).to_string());
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
                .child(
                    rust_i18n::t!("usage.limit_used", window = window_name(w.window_minutes))
                        .to_string(),
                ),
        )
        .child(
            div()
                .h(px(8.))
                .w_full()
                .rounded_full()
                .bg(rgb(t.border))
                .child(
                    div()
                        .h_full()
                        .w(relative(pct as f32 / 100.))
                        .rounded_full()
                        .bg(rgb(color)),
                ),
        )
        .child(secondary(&detail.join(" · "), t))
        .children(
            stale.then(|| secondary(rust_i18n::t!("usage.stale", agent = p.label()).as_ref(), t)),
        )
}

/// None when every agent with plan limits is hidden.
fn limits(s: &Snapshot, hidden: &BTreeSet<Provider>, t: Theme) -> Option<Div> {
    let shown: Vec<Provider> = [Provider::ClaudeCode, Provider::Codex]
        .into_iter()
        .filter(|p| !hidden.contains(p))
        .collect();
    if shown.is_empty() {
        return None;
    }
    let section = div().flex().flex_col().gap(px(sp::M)).child(section_label(
        rust_i18n::t!("usage.plan_limits").to_string(),
        t,
    ));
    Some(section.children(shown.into_iter().map(|p| {
        let body = match s.limits.get(&p) {
            None if p == Provider::ClaudeCode => {
                secondary(rust_i18n::t!("usage.no_claude_limits").as_ref(), t)
            }
            None => secondary(rust_i18n::t!("usage.no_codex_limits").as_ref(), t),
            Some(l) => {
                let windows: Vec<&RateWindow> = l.primary.iter().chain(&l.secondary).collect();
                if windows.is_empty() {
                    secondary(
                        rust_i18n::t!("usage.no_windows", agent = p.label()).as_ref(),
                        t,
                    )
                } else {
                    div().flex().flex_col().gap(px(sp::M)).children(
                        windows
                            .into_iter()
                            .map(|w| limit_row(p, w, l, s.taken_at_ms, t)),
                    )
                }
            }
        };
        div()
            .max_w(px(480.))
            .flex()
            .flex_col()
            .gap(px(sp::S))
            .child(div().font_weight(FontWeight::BOLD).child(p.label()))
            .child(body)
    })))
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
        .child(rust_i18n::t!("usage.sessions").to_string())
        .on_click(move |_, _, cx| {
            view.update(cx, |v, cx| v.show_detail(None, cx)).ok();
        });
    let page = scroll_page("detail", scroll)
        .child(back)
        .child(profile.mt(px(sp::M)))
        .children(notes_gap(notes));
    let detail = s.detail.as_ref().filter(|d| &d.key == key);
    let Some(d) = detail else {
        return page.child(secondary(rust_i18n::t!("usage.loading").as_ref(), t).mt(px(sp::XL)));
    };
    if d.requests.is_empty() {
        return page
            .child(secondary(rust_i18n::t!("usage.no_requests").as_ref(), t).mt(px(sp::XL)));
    }
    let n = d.requests.len();
    let color = agent_color(key.0, t);
    let first = d.requests[0].ts_ms;
    let last = d.requests[n - 1].ts_ms;
    let x_labels = vec![
        rust_i18n::t!(
            "usage.request_time",
            n = 1,
            time = format::start_time(first, s.taken_at_ms)
        )
        .to_string(),
        rust_i18n::t!(
            "usage.request_time",
            n = n,
            time = format::start_time(last, s.taken_at_ms)
        )
        .to_string(),
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
        .child(section_label(
            rust_i18n::t!("usage.context_size").to_string(),
            t,
        ))
        .child(secondary(rust_i18n::t!("usage.context_note").as_ref(), t))
        .children(d.long_context.iter().map(|&x| {
            div()
                .flex()
                .items_center()
                .gap(px(sp::XS))
                .child(div().w(px(16.)).h(px(2.)).bg(rgb(t.waiting)))
                .child(secondary(
                    rust_i18n::t!("usage.long_context", n = format::tokens(x)).as_ref(),
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
        .child(section_label(
            rust_i18n::t!("usage.cumulative_cost").to_string(),
            t,
        ))
        .child(figure(
            if unpriced as usize == n {
                DASH.into()
            } else {
                approx(sum)
            },
            rust_i18n::t!("usage.requests_estimate", n = n).as_ref(),
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
    fn project_labels_disambiguate_and_other_preserves_totals() {
        use agentpulse_storage::UsageGroup;
        let rows: Vec<_> = (0..11)
            .map(|i| UsageGroup {
                key: Some(format!("/parent{i}/app")),
                tokens: 100 - i,
                cost_usd: i as f64,
                unpriced_events: i,
            })
            .collect();
        let labels = project_labels(&rows);
        assert_eq!(labels[0], "parent0/app");
        assert_eq!(labels.iter().collect::<BTreeSet<_>>().len(), 11);
        assert!(labels.iter().all(|s| !s.starts_with('/')));
        let shown = ranked_rows(&rows, labels);
        assert_eq!(shown.len(), 9);
        assert_eq!(shown[8].1.tokens, 92 + 91 + 90);
        assert_eq!(shown[8].1.cost_usd, 8. + 9. + 10.);
        assert_eq!(shown[8].1.unpriced_events, 27);
        assert_eq!(
            shown.iter().map(|r| r.1.tokens).sum::<u64>(),
            rows.iter().map(|r| r.tokens).sum::<u64>()
        );
        let missing = vec![UsageGroup {
            key: None,
            ..Default::default()
        }];
        assert!(!project_labels(&missing)[0].is_empty());
        assert_eq!(ranked_rows(&rows[..8], project_labels(&rows[..8])).len(), 8);
    }

    #[test]
    fn project_labels_handle_path_edges_and_collisions() {
        let home = std::env::var("HOME").unwrap();
        let keys = [
            "/a/x/app (1)".to_string(),
            "/b/y/app (1)".into(),
            "/c/x/app".into(),
            "/d/x/app".into(),
            "/".into(),
            home.clone(),
            "/trailing/slash/".into(),
            format!("{home}/app"),
            "/elsewhere/app".into(),
        ];
        let mut rows: Vec<_> = keys
            .into_iter()
            .map(|key| UsageGroup {
                key: Some(key),
                tokens: 1,
                ..Default::default()
            })
            .collect();
        let labels = project_labels(&rows);
        assert_eq!(labels.iter().collect::<BTreeSet<_>>().len(), rows.len());
        assert_eq!(&labels[2..4], &["c/x/app", "d/x/app"]);
        assert_eq!(labels[4], format::project_name(Some("/")));
        assert_eq!(labels[5], "~");
        assert_eq!(labels[6], "slash");
        assert_eq!(labels[7], "~/app");
        let shown = ranked_rows(&rows, labels.clone());
        assert_eq!(shown.len(), 9);
        assert_eq!(shown[8].1.tokens, 1);
        rows.reverse();
        let mut reversed = project_labels(&rows);
        reversed.reverse();
        assert_eq!(labels, reversed);

        // Different full keys can normalize to the same folder suffix.
        let mut aliases: Vec<_> = ["/same/app", "/same/app/", "/same/app (1)"]
            .into_iter()
            .map(|key| UsageGroup {
                key: Some(key.into()),
                ..Default::default()
            })
            .collect();
        let labels = project_labels(&aliases);
        assert_eq!(labels.iter().collect::<BTreeSet<_>>().len(), 3);
        aliases.reverse();
        let mut reversed = project_labels(&aliases);
        reversed.reverse();
        assert_eq!(labels, reversed);
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
