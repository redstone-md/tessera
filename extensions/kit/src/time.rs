// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Calendar and clock arithmetic on the seconds `now` gives, and ways of
//! writing times and spans.
//!
//! Dates are of the Gregorian calendar; days are counted from 1970-01-01.
//! Time zones are fixed offsets from UTC: nothing here knows of daylight
//! saving time.

pub const MINUTE: u64 = 60;
pub const HOUR: u64 = 60 * MINUTE;
pub const DAY: u64 = 24 * HOUR;

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Days from 1970-01-01 to the given date.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let (month, day) = (i64::from(month), i64::from(day));
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The year, month and day `days` after 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_from_march + 2) / 5 + 1) as u32;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    let next = if month == 12 {
        days_from_civil(year + 1, 1, 1)
    } else {
        days_from_civil(year, month + 1, 1)
    };
    (next - days_from_civil(year, month, 1)) as u32
}

/// The seconds since 1970 of a date such as `2027-01-01`, optionally
/// with a time of day as `2027-01-01 09:30` or `2027-01-01T09:30`, in UTC.
pub fn parse_date(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, clock) = match text.split_once([' ', 'T']) {
        Some((date, clock)) => (date, Some(clock.trim())),
        None => (text, None),
    };
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    if day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let seconds_of_day = match clock {
        None => 0,
        Some(clock) => {
            let (hour, minute) = clock.split_once(':')?;
            let (hour, minute): (u64, u64) = (hour.parse().ok()?, minute.parse().ok()?);
            if hour > 23 || minute > 59 {
                return None;
            }
            hour * HOUR + minute * MINUTE
        }
    };
    let seconds = days_from_civil(year, month, day) * DAY as i64 + seconds_of_day as i64;
    u64::try_from(seconds).ok()
}

/// The seconds a UTC offset such as `+9`, `-5`, `+5:30`, `0` or `UTC+2`
/// adds to UTC; offsets beyond 14 hours are refused.
pub fn parse_offset(text: &str) -> Option<i64> {
    let text = text.trim();
    let text = text
        .strip_prefix("UTC")
        .or_else(|| text.strip_prefix("GMT"))
        .unwrap_or(text)
        .trim();
    if text.is_empty() {
        return Some(0);
    }
    let (sign, rest) = match text.as_bytes()[0] {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => (1, text),
    };
    let (hours, minutes) = rest.split_once(':').unwrap_or((rest, "0"));
    let all_digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(hours) || !all_digits(minutes) {
        return None;
    }
    let (hours, minutes): (i64, i64) = (hours.parse().ok()?, minutes.parse().ok()?);
    if minutes > 59 || hours * 60 + minutes > 14 * 60 {
        return None;
    }
    Some(sign * (hours * HOUR as i64 + minutes * MINUTE as i64))
}

/// An offset written as `UTC+9`, `UTC-3:30` or `UTC`.
pub fn format_offset(offset: i64) -> String {
    if offset == 0 {
        return "UTC".to_owned();
    }
    let sign = if offset < 0 { '-' } else { '+' };
    let minutes = offset.unsigned_abs() / MINUTE;
    match minutes % 60 {
        0 => format!("UTC{sign}{}", minutes / 60),
        rest => format!("UTC{sign}{}:{rest:02}", minutes / 60),
    }
}

/// A moment seen from a zone `offset` seconds off UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    /// Days since 1970-01-01 in that zone.
    pub day: i64,
    /// Seconds since midnight in that zone.
    pub seconds: u64,
}

impl Local {
    pub fn at(now: u64, offset: i64) -> Self {
        let shifted = now as i64 + offset;
        Self {
            day: shifted.div_euclid(DAY as i64),
            seconds: shifted.rem_euclid(DAY as i64) as u64,
        }
    }

    /// The time of day as `09:05`.
    pub fn clock(self) -> String {
        format!(
            "{:02}:{:02}",
            self.seconds / HOUR,
            self.seconds % HOUR / MINUTE
        )
    }

    /// The time of day as `9:05 am`.
    pub fn clock_12(self) -> String {
        let hour = self.seconds / HOUR;
        let half = if hour < 12 { "am" } else { "pm" };
        let shown = match hour % 12 {
            0 => 12,
            hour => hour,
        };
        format!("{shown}:{:02} {half}", self.seconds % HOUR / MINUTE)
    }

    pub fn weekday(self) -> &'static str {
        weekday(self.day)
    }
}

/// The short name of the weekday `day` days after 1970-01-01, a Thursday.
pub fn weekday(day: i64) -> &'static str {
    WEEKDAYS[(day + 3).rem_euclid(7) as usize]
}

/// A day written as `26 Nov 2026`.
pub fn date(day: i64) -> String {
    let (year, month, day) = civil_from_days(day);
    format!("{day} {} {year}", MONTHS[month as usize - 1])
}

/// Minutes and seconds as `04:59`; the minutes may pass 59.
pub fn mmss(seconds: u64) -> String {
    format!("{:02}:{:02}", seconds / MINUTE, seconds % MINUTE)
}

/// A span in its two largest units, as `12d 4h`, `3h 5m`, `5m` or `30s`.
pub fn span(seconds: u64) -> String {
    let (days, hours) = (seconds / DAY, seconds % DAY / HOUR);
    let (minutes, rest) = (seconds % HOUR / MINUTE, seconds % MINUTE);
    match () {
        _ if days > 0 => format!("{days}d {hours}h"),
        _ if hours > 0 => format!("{hours}h {minutes}m"),
        _ if minutes > 0 => format!("{minutes}m"),
        _ => format!("{rest}s"),
    }
}

/// How long ago `then` was at `now`, as `just now` or `5 min ago`.
pub fn ago(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / MINUTE),
        3600..86_400 => format!("{} h ago", seconds / HOUR),
        _ => match seconds / DAY {
            1 => "a day ago".to_owned(),
            days => format!("{days} days ago"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_go_both_ways() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        for day in [-800_000, -1, 0, 59, 11_016, 20_000, 2_000_000] {
            let (year, month, date) = civil_from_days(day);
            assert_eq!(days_from_civil(year, month, date), day);
        }
        assert_eq!(civil_from_days(20_783), (2026, 11, 26));
    }

    #[test]
    fn dates_are_read_with_or_without_a_time() {
        assert_eq!(parse_date("2027-01-01"), Some(1_798_761_600));
        assert_eq!(
            parse_date("2027-01-01 09:30"),
            Some(1_798_761_600 + 9 * 3600 + 1800)
        );
        assert_eq!(
            parse_date("2027-01-01T09:30"),
            parse_date("2027-01-01 09:30")
        );
        assert_eq!(parse_date("2028-02-29").map(|s| s % DAY), Some(0));
        for wrong in [
            "2027-02-29",
            "2027-13-01",
            "2027-1",
            "soon",
            "2027-01-01 25:00",
        ] {
            assert_eq!(parse_date(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn offsets_are_read_and_written() {
        assert_eq!(parse_offset("+9"), Some(9 * 3600));
        assert_eq!(parse_offset("-5"), Some(-5 * 3600));
        assert_eq!(parse_offset("+5:30"), Some(5 * 3600 + 1800));
        assert_eq!(parse_offset("UTC-3:30"), Some(-(3 * 3600 + 1800)));
        assert_eq!(parse_offset("0"), Some(0));
        assert_eq!(parse_offset("+15"), None);
        assert_eq!(parse_offset("+5:75"), None);
        assert_eq!(parse_offset("east"), None);
        assert_eq!(format_offset(19_800), "UTC+5:30");
        assert_eq!(format_offset(-5 * 3600), "UTC-5");
        assert_eq!(format_offset(0), "UTC");
    }

    #[test]
    fn local_times_cross_midnight() {
        // 2026-11-26 23:30 UTC.
        let now = 20_783 * DAY + 23 * HOUR + 30 * MINUTE;
        let tokyo = Local::at(now, 9 * 3600);
        assert_eq!((tokyo.day, tokyo.clock()), (20_784, "08:30".to_owned()));
        assert_eq!(tokyo.clock_12(), "8:30 am");
        assert_eq!(Local::at(now, -5 * 3600).clock_12(), "6:30 pm");
        assert_eq!(Local::at(now, 0).weekday(), "Thu");
        assert_eq!(date(20_783), "26 Nov 2026");
    }

    #[test]
    fn spans_are_short() {
        assert_eq!(mmss(299), "04:59");
        assert_eq!(span(12 * DAY + 4 * HOUR + 59), "12d 4h");
        assert_eq!(span(3 * HOUR + 5 * MINUTE), "3h 5m");
        assert_eq!(span(5 * MINUTE + 3), "5m");
        assert_eq!(span(30), "30s");
        assert_eq!(ago(1000, 990), "just now");
        assert_eq!(ago(10_000, 10_000 - 7200), "2 h ago");
        assert_eq!(ago(10 * DAY, 7 * DAY), "3 days ago");
    }
}
