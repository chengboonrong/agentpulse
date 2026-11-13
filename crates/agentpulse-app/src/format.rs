//! Display formatting for numbers, durations and times.

use std::fmt::Display;

use chrono::{DateTime, Local, TimeZone};

/// Shown in place of a value that is not available.
pub const DASH: &str = "—";

/// `950`, `1.2K`, `45K`, `1.2M`.
pub fn tokens(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    let units = [("K", 1e3), ("M", 1e6), ("B", 1e9), ("T", 1e12)];
    for (i, (suffix, div)) in units.iter().enumerate() {
        let v = n as f64 / div;
        let s = if v < 9.95 {
            format!("{v:.1}")
        } else {
            format!("{v:.0}")
        };
        // 999.6K would round to "1000K": move up a unit instead.
        if v < 999.5 || i == units.len() - 1 {
            return format!("{s}{suffix}");
        }
    }
    unreachable!("the last unit always returns")
}

/// `45s`, `12m 05s`, `1h 02m`, `2d 03h`.
pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3_600 => format!("{}m {:02}s", s / 60, s % 60),
        3_600..86_400 => format!("{}h {:02}m", s / 3_600, s % 3_600 / 60),
        _ => format!("{}d {:02}h", s / 86_400, s % 86_400 / 3_600),
    }
}

/// `$1.23`, `<$0.01`, `$0.00`.
pub fn cost(usd: f64) -> String {
    if usd > 0.0 && usd < 0.005 {
        "<$0.01".into()
    } else {
        format!("${usd:.2}")
    }
}

/// `512 B`, `9.5 KB`, `1.4 GB` (binary units, as Activity Monitor shows them).
pub fn bytes(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    match i {
        0 => format!("{n} B"),
        _ if v < 9.95 => format!("{v:.1} {}", units[i]),
        _ => format!("{v:.0} {}", units[i]),
    }
}

/// Local start time: `14:05` today, `Nov 12 14:05` on another day.
pub fn start_time(ms: i64, now_ms: i64) -> String {
    start_time_in(ms, now_ms, &Local)
}

fn start_time_in<Tz: TimeZone>(ms: i64, now_ms: i64, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    let (Some(t), Some(now)) = (
        DateTime::from_timestamp_millis(ms),
        DateTime::from_timestamp_millis(now_ms),
    ) else {
        return DASH.into();
    };
    let (t, now) = (t.with_timezone(tz), now.with_timezone(tz));
    if t.date_naive() == now.date_naive() {
        t.format("%H:%M").to_string()
    } else {
        t.format("%b %-d %H:%M").to_string()
    }
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    #[test]
    fn token_counts() {
        assert_eq!(tokens(0), "0");
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(1_000), "1.0K");
        assert_eq!(tokens(1_234), "1.2K");
        assert_eq!(tokens(45_600), "46K");
        assert_eq!(tokens(999_600), "1.0M");
        assert_eq!(tokens(1_234_567), "1.2M");
        assert_eq!(tokens(250_000_000), "250M");
        assert_eq!(tokens(3_000_000_000), "3.0B");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(45_999), "45s");
        assert_eq!(duration(65_000), "1m 05s");
        assert_eq!(duration(3_720_000), "1h 02m");
        assert_eq!(duration(97_200_000), "1d 03h");
    }

    #[test]
    fn costs() {
        assert_eq!(cost(0.0), "$0.00");
        assert_eq!(cost(0.004), "<$0.01");
        assert_eq!(cost(0.005), "$0.01");
        assert_eq!(cost(1.234), "$1.23");
        assert_eq!(cost(1234.5), "$1234.50");
    }

    #[test]
    fn byte_sizes() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(9_728), "9.5 KB");
        assert_eq!(bytes(300 * 1024 * 1024), "300 MB");
        assert_eq!(bytes(1_503_238_554), "1.4 GB");
    }

    #[test]
    fn start_times() {
        let tz = FixedOffset::east_opt(8 * 3600).unwrap();
        // 2026-11-13 02:00 UTC = 10:00 at +08:00.
        let t = 1_794_535_200_000;
        assert_eq!(start_time_in(t, t + 3_600_000, &tz), "10:00");
        assert_eq!(start_time_in(t, t + 86_400_000, &tz), "Nov 13 10:00");
        assert_eq!(start_time_in(i64::MAX, t, &tz), DASH);
    }
}
