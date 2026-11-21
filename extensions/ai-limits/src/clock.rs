// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Reading timestamps and describing time spans.

/// The seconds since 1970-01-01 00:00 UTC of an ISO 8601 timestamp such as
/// `2026-03-01T12:30:00.5+02:00` or `2026-03-01T10:30:00Z`.
pub fn parse_timestamp(text: &str) -> Option<u64> {
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = text.get(range)?;
        digits
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| digits.parse().ok())?
    };
    let separator = |at: usize, expected: &[u8]| -> Option<()> {
        expected.contains(text.as_bytes().get(at)?).then_some(())
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    separator(4, b"-")?;
    separator(7, b"-")?;
    separator(10, b"Tt ")?;
    separator(13, b":")?;
    separator(16, b":")?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }

    let rest = text.get(19..)?;
    let zone = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match zone {
        "Z" | "z" => 0,
        _ => {
            let sign = match zone.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let digits: String = zone[1..].chars().filter(|&c| c != ':').collect();
            if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let (hours, minutes) = digits.split_at(2);
            sign * (hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60)
        }
    };

    let seconds =
        days_from_epoch(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset;
    u64::try_from(seconds).ok()
}

/// Days from 1970-01-01 to the given date of the Gregorian calendar.
fn days_from_epoch(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// A time span in words, by its two largest units: `3 d 4 h`, `2 h 5 min`.
pub fn span(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    if days > 0 {
        format!("{days} d {hours} h")
    } else if hours > 0 {
        format!("{hours} h {minutes} min")
    } else if minutes > 0 {
        format!("{minutes} min")
    } else {
        "less than a minute".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_with_and_without_offsets_are_read() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("2024-02-29T12:00:00Z"), Some(1_709_208_000));
        assert_eq!(
            parse_timestamp("2026-03-01T12:30:00.123456+02:00"),
            parse_timestamp("2026-03-01T10:30:00Z")
        );
        assert_eq!(
            parse_timestamp("2026-03-01T05:30:00-0500"),
            parse_timestamp("2026-03-01T10:30:00Z")
        );
        assert_eq!(parse_timestamp("2026-03-01T10:30:00Z"), Some(1_772_361_000));
    }

    #[test]
    fn text_that_is_not_a_timestamp_is_rejected() {
        for text in [
            "",
            "tomorrow",
            "2026-03-01",
            "2026-03-01T10:30:00",
            "2026-13-01T10:30:00Z",
            "2026-03-01T10:30:00+2",
            "2026/03/01T10:30:00Z",
            "1960-01-01T00:00:00Z",
        ] {
            assert_eq!(parse_timestamp(text), None, "{text:?} was read");
        }
    }

    #[test]
    fn spans_name_their_two_largest_units() {
        assert_eq!(span(0), "less than a minute");
        assert_eq!(span(59), "less than a minute");
        assert_eq!(span(60), "1 min");
        assert_eq!(span(2 * 3600 + 5 * 60 + 30), "2 h 5 min");
        assert_eq!(span(3 * 86_400 + 4 * 3600 + 59 * 60), "3 d 4 h");
    }
}
