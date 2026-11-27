// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The time in other zones: `time tokyo` or `time in UTC+3` tells what
//! time it is there, and `15:00 berlin in new york` what time it is in
//! New York when it is three in the afternoon in Berlin. A zone is an
//! offset from UTC (`UTC+3`, `GMT-5:30`, `UTC`), `local` or `here` for
//! the zone of this computer, or a name the shared table of zones knows:
//! an IANA name, a Windows name or a city.
//!
//! The table of zones is `crate::time_zones`, which the clock brings.
//! Until it is there, the stub below stands in for it with the same
//! signature and knows only offsets and the local zone, so a city is not
//! found yet.

use chrono::{Duration, Local, NaiveDateTime, NaiveTime, Offset, TimeZone, Utc};

use super::{Answer, AnswerAction, Query, Runner, fill};

use time_zones::Zone;

const ICON_TIME: &str = "\u{e823}";

/// The scores of the time asked for with `time`, and of a conversion.
const SCORE_ASKED: u16 = 900;
const SCORE_CONVERSION: u16 = 880;

/// The words a question for the time begins with.
const ASKED: [&str; 2] = ["time", "время"];

/// The words between two zones of a conversion, and before the zone
/// whose time is asked.
const BETWEEN: [&str; 4] = ["in", "to", "в", "во"];

/// The names of the zone of this computer.
const LOCAL: [&str; 4] = ["local", "here", "здесь", "local time"];

/// A stand-in for the table of zones that the clock brings
/// (`crate::time_zones`), with the same signature. It knows only offsets
/// from UTC and the zone of this computer.
#[allow(
    dead_code,
    reason = "a stand-in with the signature of the table that replaces it"
)]
mod time_zones {
    use chrono::{Local, NaiveDateTime, Offset, TimeZone};

    /// A zone as the table names it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Zone {
        pub windows_id: String,
        pub display: String,
        pub iana: Option<String>,
    }

    /// What the stand-in calls the zone of this computer.
    const LOCAL_ID: &str = "Local";

    /// The zone `name` names: here only `UTC+3` and the like, and the
    /// local zone.
    pub fn find(name: &str) -> Option<Zone> {
        if name.eq_ignore_ascii_case(LOCAL_ID) {
            return Some(Zone {
                windows_id: LOCAL_ID.to_owned(),
                display: LOCAL_ID.to_owned(),
                iana: None,
            });
        }
        let minutes = super::offset_named(name)?;
        let written = super::offset_written(minutes);
        Some(Zone {
            windows_id: written.clone(),
            display: written,
            iana: None,
        })
    }

    /// Every zone the table knows; the stand-in lists none.
    pub fn all() -> Vec<Zone> {
        Vec::new()
    }

    impl Zone {
        /// What time it is now in the zone.
        pub fn now(&self) -> Option<NaiveDateTime> {
            let now = chrono::Utc::now().naive_utc();
            let offset = self.offset_minutes(now)?;
            Some(now + chrono::Duration::minutes(i64::from(offset)))
        }

        /// How many minutes the zone is ahead of UTC at `at_utc`.
        pub fn offset_minutes(&self, at_utc: NaiveDateTime) -> Option<i32> {
            if self.windows_id == LOCAL_ID {
                let offset = Local.offset_from_utc_datetime(&at_utc).fix();
                return Some(offset.local_minus_utc() / 60);
            }
            super::offset_named(&self.windows_id)
        }

        /// The time in `to` when it is `local` here.
        pub fn convert(&self, local: NaiveDateTime, to: &Zone) -> Option<NaiveDateTime> {
            let guess = local - chrono::Duration::minutes(i64::from(self.offset_minutes(local)?));
            let utc = local - chrono::Duration::minutes(i64::from(self.offset_minutes(guess)?));
            Some(utc + chrono::Duration::minutes(i64::from(to.offset_minutes(utc)?)))
        }
    }
}

/// The offset from UTC in minutes that `name` writes, as `UTC+3`,
/// `utc-5:30`, `GMT+0100` or `UTC`; at most fourteen hours.
fn offset_named(name: &str) -> Option<i32> {
    let name = name.trim().to_ascii_uppercase();
    let rest = name
        .strip_prefix("UTC")
        .or_else(|| name.strip_prefix("GMT"))?
        .trim();
    if rest.is_empty() {
        return Some(0);
    }
    let (sign, digits) = match rest.split_at_checked(1)? {
        ("+", digits) => (1, digits),
        ("-" | "−", digits) => (-1, digits),
        _ => return None,
    };
    let (hours, minutes) = match digits.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if digits.len() == 4 => digits.split_at(2),
        None => (digits, "0"),
    };
    let (hours, minutes): (i32, i32) = (hours.parse().ok()?, minutes.parse().ok()?);
    if hours > 14 || minutes >= 60 || (hours == 14 && minutes > 0) {
        return None;
    }
    Some(sign * (hours * 60 + minutes))
}

/// `minutes` from UTC as an offset is written: `UTC+3`, `UTC-5:30`.
fn offset_written(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let (hours, rest) = (minutes.abs() / 60, minutes.abs() % 60);
    match (minutes, rest) {
        (0, _) => "UTC".to_owned(),
        (_, 0) => format!("UTC{sign}{hours}"),
        _ => format!("UTC{sign}{hours}:{rest:02}"),
    }
}

/// A clock time as it is typed: `15:00`, `9:30`, `3pm`, `3:15 am`, `15h`.
fn clock_time(text: &str) -> Option<NaiveTime> {
    let text = text.trim().to_lowercase();
    let (text, half) = match (text.strip_suffix("pm"), text.strip_suffix("am")) {
        (Some(text), _) => (text.trim(), Some(12)),
        (_, Some(text)) => (text.trim(), Some(0)),
        _ => (text.as_str(), None),
    };
    let text = text.strip_suffix('h').unwrap_or(text);
    let (hours, minutes) = text.split_once([':', '.']).unwrap_or((text, "0"));
    let (mut hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    if let Some(half) = half {
        if !(1..=12).contains(&hours) {
            return None;
        }
        hours = hours % 12 + half;
    }
    NaiveTime::from_hms_opt(hours, minutes, 0)
}

/// A zone a question names.
#[derive(Clone, Debug, PartialEq)]
enum Place {
    /// So many minutes ahead of UTC.
    Offset(i32),
    /// The zone of this computer.
    Local,
    /// A zone of the table.
    Zone(Zone),
}

impl Place {
    /// The zone `name` names, if it names one.
    fn named(name: &str) -> Option<Self> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        if let Some(minutes) = offset_named(name) {
            return Some(Self::Offset(minutes));
        }
        if LOCAL.iter().any(|local| name.eq_ignore_ascii_case(local)) {
            return Some(Self::Local);
        }
        time_zones::find(name).map(Self::Zone)
    }

    /// How many minutes the zone is ahead of UTC at `at_utc`.
    fn offset_minutes(&self, at_utc: NaiveDateTime) -> Option<i32> {
        match self {
            Self::Offset(minutes) => Some(*minutes),
            Self::Local => Some(
                Local
                    .offset_from_utc_datetime(&at_utc)
                    .fix()
                    .local_minus_utc()
                    / 60,
            ),
            Self::Zone(zone) => zone.offset_minutes(at_utc),
        }
    }

    /// The time in UTC when it is `local` in the zone.
    fn to_utc(&self, local: NaiveDateTime) -> Option<NaiveDateTime> {
        // The offset at the guess is right but within an hour of a change
        // of the clocks, which a second look settles.
        let guess = local - minutes(self.offset_minutes(local)?);
        Some(local - minutes(self.offset_minutes(guess)?))
    }

    /// The time in the zone at `utc`.
    fn at(&self, utc: NaiveDateTime) -> Option<NaiveDateTime> {
        Some(utc + minutes(self.offset_minutes(utc)?))
    }

    /// What the zone is called in an answer.
    fn name(&self, query: &Query) -> String {
        match self {
            Self::Offset(minutes) => offset_written(*minutes),
            Self::Local => query.say("Local time").to_owned(),
            Self::Zone(zone) => zone.display.clone(),
        }
    }
}

fn minutes(minutes: i32) -> Duration {
    Duration::minutes(i64::from(minutes))
}

/// What a question for the time asks.
#[derive(Clone, Debug, PartialEq)]
enum Asked {
    /// The time now in a zone; in the local zone and in UTC for none.
    Now(Option<Place>),
    /// The time in the second zone when it is this time in the first.
    Conversion(NaiveTime, Place, Place),
}

/// What `text` asks of the runner.
fn asked(text: &str) -> Option<Asked> {
    let text = text.trim();
    let lower = text.to_lowercase();
    let word = ASKED.iter().find(|word| {
        lower.starts_with(*word)
            && lower[word.len()..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
    });
    let rest = match word {
        Some(word) => text[word.len()..].trim(),
        None => text,
    };
    let words: Vec<&str> = rest.split_whitespace().collect();
    // A conversion: a clock time, a zone, a word between, another zone.
    if let Some(time) = words.first().and_then(|first| clock_time(first)) {
        let between = { words.iter().rposition(|word| is_between(word)) }?;
        let from = match between {
            1 => Place::Local,
            _ => Place::named(&words[1..between].join(" "))?,
        };
        let to = Place::named(&words[between + 1..].join(" "))?;
        return Some(Asked::Conversion(time, from, to));
    }
    // The time now needs the word that asks for it.
    word?;
    let words = match words.first() {
        Some(first) if is_between(first) => &words[1..],
        _ => &words[..],
    };
    match words.is_empty() {
        true => Some(Asked::Now(None)),
        false => Place::named(&words.join(" ")).map(|place| Asked::Now(Some(place))),
    }
}

/// Whether `word` stands between two zones.
fn is_between(word: &str) -> bool {
    BETWEEN.contains(&word.to_lowercase().as_str())
}

/// The time in the zone `place` at `utc`, as an answer says it: the
/// time, and the zone with the day and the offset.
fn said(utc: NaiveDateTime, place: &Place, query: &Query) -> Option<(String, String)> {
    let time = place.at(utc)?;
    let shown = query.language().moment(time, "%H:%M");
    let day = query.language().moment(time, "%a %-d %b");
    let offset = offset_written(place.offset_minutes(utc)?);
    // An offset is named by itself already.
    let detail = match place {
        Place::Offset(_) => fill("{} · {}", &[&offset, &day]),
        _ => fill("{} · {} · {}", &[&place.name(query), &day, &offset]),
    };
    Some((shown, detail))
}

fn answer(title: String, detail: String, score: u16) -> Answer {
    Answer::new(
        title.clone(),
        detail,
        ICON_TIME,
        score,
        AnswerAction::Copy(title.clone()),
    )
    .copying(title)
}

pub struct Time;

impl Runner for Time {
    fn id(&self) -> &'static str {
        "time"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        let now = Utc::now().naive_utc();
        match asked(query.text()) {
            None => Vec::new(),
            Some(Asked::Now(place)) => {
                let places = match place {
                    Some(place) => vec![place],
                    None => vec![Place::Local, Place::Offset(0)],
                };
                { places.iter() }
                    .filter_map(|place| {
                        let (title, detail) = said(now, place, query)?;
                        Some(answer(title, detail, SCORE_ASKED))
                    })
                    .collect()
            }
            Some(Asked::Conversion(time, from, to)) => {
                // The time is taken as one of today in the first zone.
                let today = from.at(now).unwrap_or(now).date();
                let Some(utc) = from.to_utc(today.and_time(time)) else {
                    return Vec::new();
                };
                let Some((shown, detail)) = said(utc, &to, query) else {
                    return Vec::new();
                };
                let typed = fill(
                    "{} {}",
                    &[&time.format("%H:%M").to_string(), &from.name(query)],
                );
                let detail = fill("{} = {}", &[&typed, &detail]);
                vec![answer(shown, detail, SCORE_CONVERSION)]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, Timelike};

    #[test]
    fn offsets_are_read_and_written() {
        assert_eq!(offset_named("UTC+3"), Some(180));
        assert_eq!(offset_named("utc-5:30"), Some(-330));
        assert_eq!(offset_named("GMT+0100"), Some(60));
        assert_eq!(offset_named("UTC"), Some(0));
        assert_eq!(offset_named("UTC+15"), None);
        assert_eq!(offset_named("UTC*3"), None);
        assert_eq!(offset_named("Tokyo"), None);
        assert_eq!(offset_written(180), "UTC+3");
        assert_eq!(offset_written(-330), "UTC-5:30");
        assert_eq!(offset_written(0), "UTC");
    }

    #[test]
    fn clock_times_are_read_as_they_are_typed() {
        let at = |h, m| NaiveTime::from_hms_opt(h, m, 0);
        assert_eq!(clock_time("15:00"), at(15, 0));
        assert_eq!(clock_time("9.30"), at(9, 30));
        assert_eq!(clock_time("3pm"), at(15, 0));
        assert_eq!(clock_time("12 am"), at(0, 0));
        assert_eq!(clock_time("12pm"), at(12, 0));
        assert_eq!(clock_time("15h"), at(15, 0));
        assert_eq!(clock_time("13pm"), None);
        assert_eq!(clock_time("25:00"), None);
        assert_eq!(clock_time("berlin"), None);
    }

    #[test]
    fn questions_are_understood() {
        assert_eq!(asked("time"), Some(Asked::Now(None)));
        assert_eq!(
            asked("time in UTC+3"),
            Some(Asked::Now(Some(Place::Offset(180))))
        );
        assert_eq!(
            asked("Time utc-2"),
            Some(Asked::Now(Some(Place::Offset(-120))))
        );
        assert_eq!(asked("time here"), Some(Asked::Now(Some(Place::Local))));
        assert_eq!(asked("timetable"), None);
        assert_eq!(asked("UTC+3"), None);
        let at = NaiveTime::from_hms_opt(15, 0, 0).unwrap();
        assert_eq!(
            asked("15:00 utc+1 in utc-5"),
            Some(Asked::Conversion(
                at,
                Place::Offset(60),
                Place::Offset(-300)
            ))
        );
        assert_eq!(
            asked("time 3pm in utc"),
            Some(Asked::Conversion(at, Place::Local, Place::Offset(0)))
        );
        assert_eq!(asked("15:00 nowhere in utc"), None);
        assert_eq!(asked("15:00 + 2"), None);
    }

    #[test]
    fn a_time_is_converted_between_offsets() {
        let from = Place::Offset(60);
        let to = Place::Offset(-300);
        let local = NaiveDate::from_ymd_opt(2026, 11, 27)
            .unwrap()
            .and_hms_opt(15, 0, 0)
            .unwrap();
        let utc = from.to_utc(local).unwrap();
        assert_eq!(utc.hour(), 14);
        assert_eq!(to.at(utc).unwrap().hour(), 9);
        let stub = time_zones::find("UTC+9").unwrap();
        let tokyo = stub
            .convert(local, &time_zones::find("UTC").unwrap())
            .unwrap();
        assert_eq!(tokyo.hour(), 6);
    }
}
