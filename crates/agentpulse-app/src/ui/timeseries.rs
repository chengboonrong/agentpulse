//! A time series on a canvas: samples placed by time, decimated to one
//! column per pixel, with a hover readout of the nearest sample.

use gpui::{
    Bounds, IntoElement, PathBuilder, Pixels, Point, Styled, TextAlign, TextRun, Window, canvas,
    fill, font, point, px, rgb, size,
};

use super::{FONT_FAMILY, ty};

/// Colours as 0xRRGGBB, taken from the theme.
#[derive(Clone, Copy)]
pub(super) struct Colors {
    pub line: u32,
    pub base: u32,
    pub grid: u32,
    pub text: u32,
    pub card: u32,
}

/// Samples in `[start, end]` as at most one (column, first, second) per pixel column:
/// the column's min and max, whichever came first leading, so peaks survive whatever
/// the sample count.
fn columns(samples: &[(i64, f32)], start: i64, end: i64, cols: usize) -> Vec<(usize, f32, f32)> {
    let span = (end - start).max(1) as f64;
    // Per column: min and max with the index of the sample that set each.
    type Acc = (usize, (f32, usize), (f32, usize));
    let mut out: Vec<Acc> = Vec::new();
    let in_range = samples.iter().filter(|&&(at, _)| at >= start && at <= end);
    for (i, &(at, v)) in in_range.enumerate() {
        let col = (((at - start) as f64 / span) * (cols - 1) as f64).round() as usize;
        match out.last_mut() {
            Some((c, lo, hi)) if *c == col => {
                if v < lo.0 {
                    *lo = (v, i);
                }
                if v > hi.0 {
                    *hi = (v, i);
                }
            }
            _ => out.push((col, (v, i), (v, i))),
        }
    }
    out.into_iter()
        .map(|(c, lo, hi)| {
            if lo.1 <= hi.1 {
                (c, lo.0, hi.0)
            } else {
                (c, hi.0, lo.0)
            }
        })
        .collect()
}

/// The in-range sample nearest in time to `at`.
fn nearest(samples: &[(i64, f32)], start: i64, end: i64, at: i64) -> Option<(i64, f32)> {
    samples
        .iter()
        .copied()
        .filter(|&(t, _)| t >= start && t <= end)
        .min_by_key(|&(t, _)| (t - at).abs())
}

fn ago_text(ms: i64) -> String {
    if ms < 1_000 {
        "now".into()
    } else {
        format!("{} s ago", ms / 1_000)
    }
}

fn shape(window: &Window, text: &str, color: u32) -> gpui::ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: font(FONT_FAMILY),
        color: rgb(color).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text.to_string().into(), px(ty::LABEL), &[run], None)
}

/// A baseline, a hairline at the maximum when there is one, and the samples as a 2 px
/// line over `[end_ms - span_ms, end_ms]`, scaled to `max`. Hovering shows the nearest sample.
pub(super) fn time_series(
    samples: Vec<(i64, f32)>,
    end_ms: i64,
    span_ms: i64,
    max: Option<f32>,
    hover: Option<Point<Pixels>>,
    c: Colors,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let hairline = |y, color| {
                fill(
                    Bounds::new(point(b.left(), y), size(b.size.width, px(1.))),
                    rgb(color),
                )
            };
            window.paint_quad(hairline(b.bottom() - px(1.), c.base));
            let Some(max) = max else { return };
            window.paint_quad(hairline(b.top(), c.grid));
            let start = end_ms - span_ms;
            let cols = (f32::from(b.size.width) as usize).max(2);
            let cols_v = columns(&samples, start, end_ms, cols);
            if cols_v.len() < 2 {
                return;
            }
            let height = b.size.height - px(1.5);
            let x_of = |col: usize| b.left() + px(col as f32);
            let y_of = |v: f32| b.bottom() - px(1.) - height * (v / max);
            let mut path = PathBuilder::stroke(px(2.));
            for (i, &(col, first, second)) in cols_v.iter().enumerate() {
                let p = point(x_of(col), y_of(first));
                if i == 0 {
                    path.move_to(p);
                } else {
                    path.line_to(p);
                }
                if first != second {
                    path.line_to(point(x_of(col), y_of(second)));
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, rgb(c.line));
            }

            let Some(h) = hover.filter(|h| b.contains(h)) else {
                return;
            };
            let at = start
                + (f32::from(h.x - b.left()) / f32::from(b.size.width) * span_ms as f32) as i64;
            let Some((t, v)) = nearest(&samples, start, end_ms, at) else {
                return;
            };
            let x = b.left() + b.size.width * ((t - start) as f32 / span_ms as f32);
            let y = y_of(v);
            window.paint_quad(fill(
                Bounds::new(point(x, b.top()), size(px(1.), b.size.height)),
                rgb(c.grid),
            ));
            window.paint_quad(
                fill(
                    Bounds::new(point(x - px(4.), y - px(4.)), size(px(8.), px(8.))),
                    rgb(c.line),
                )
                .corner_radii(px(4.)),
            );
            let text = format!("{v:.1} tok/s · {}", ago_text(end_ms - t));
            let line = shape(window, &text, c.text);
            let pad = px(4.);
            let w = line.width + pad * 2.;
            let box_x = if x + px(8.) + w > b.right() {
                x - px(8.) - w
            } else {
                x + px(8.)
            };
            let card = Bounds::new(point(box_x, b.top()), size(w, px(ty::LABEL + 8.)));
            window.paint_quad(fill(card, rgb(c.grid)).corner_radii(px(5.)));
            window.paint_quad(
                fill(
                    Bounds::new(
                        card.origin + point(px(1.), px(1.)),
                        card.size - size(px(2.), px(2.)),
                    ),
                    rgb(c.card),
                )
                .corner_radii(px(4.)),
            );
            line.paint(
                card.origin + point(pad, pad),
                px(ty::LABEL),
                TextAlign::Left,
                None,
                window,
                cx,
            )
            .ok();
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimation_keeps_the_peak_and_one_entry_per_column() {
        let s: Vec<(i64, f32)> = (0..1000)
            .map(|i| (i, if i == 500 { 9. } else { 1. }))
            .collect();
        let c = columns(&s, 0, 999, 100);
        assert!(c.len() <= 100);
        assert!(c.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(c.iter().any(|&(_, a, b)| a == 9. || b == 9.));
    }

    #[test]
    fn decimation_skips_out_of_range_samples() {
        let s = [(-5, 7.), (0, 1.), (10, 2.), (11, 8.)];
        let c = columns(&s, 0, 10, 11);
        assert_eq!(c, [(0, 1., 1.), (10, 2., 2.)]);
        let s = [(0, 5.), (1, 2.), (2, 8.)];
        assert_eq!(columns(&s, 0, 2, 1 + 1), [(0, 5., 5.), (1, 2., 8.)]);
    }

    #[test]
    fn nearest_picks_the_closest_in_range_sample() {
        let s = [(-100, 5.), (0, 1.), (10, 2.), (100, 9.)];
        assert_eq!(nearest(&s, 0, 50, 7), Some((10, 2.)));
        assert_eq!(nearest(&s, 0, 50, 3), Some((0, 1.)));
        assert_eq!(nearest(&s, 200, 300, 250), None);
    }

    #[test]
    fn ago_text_reads_now_then_seconds() {
        assert_eq!(ago_text(999), "now");
        assert_eq!(ago_text(1_000), "1 s ago");
        assert_eq!(ago_text(5_400), "5 s ago");
    }
}
