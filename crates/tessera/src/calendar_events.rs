// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The events of iCalendar files (RFC 5545), for the calendar: the
//! `VEVENT`s of a file with their start and end, all day or at a time,
//! floating, in universal time or in a zone named by `TZID`, and the days
//! they come again on by `RRULE` (`DAILY`, `WEEKLY`, `MONTHLY` and `YEARLY`,
//! with `INTERVAL`, `COUNT`, `UNTIL`, `BYDAY`, `BYMONTHDAY` and `BYMONTH`),
//! less those `EXDATE` takes out and with those `RECURRENCE-ID` moves.
//!
//! Parsing and the recurrence are pure. What a time in a zone is in local
//! time is asked of a function the caller gives, so that the tests need
//! no Windows; [`local_times`] asks Windows through [`crate::time_zones`].

use std::cell::RefCell;
use std::collections::HashMap;

use chrono::{
    DateTime, Datelike, Days, Duration, Local, Months, NaiveDate, NaiveDateTime, NaiveTime,
    TimeZone, Utc, Weekday,
};

use crate::time_zones::{self, Zone};

/// How many times a rule is followed at most for one event, so that a
/// rule without an end does not take long to pass over.
const MOST_STEPS: usize = 20_000;

/// How a time of an event is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stamp {
    /// A day, without a time: an event of all day.
    Date,
    /// A time wherever the user is.
    Floating,
    /// A time in universal time.
    Utc,
    /// A time in the zone a `TZID` names.
    Zoned(String),
}

/// A time of an event as it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventTime {
    pub at: NaiveDateTime,
    pub stamp: Stamp,
}

/// How often an event comes again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// When an event comes again, as its `RRULE` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub frequency: Frequency,
    /// Every how many days, weeks, months or years; at least 1.
    pub interval: u32,
    /// How many times it comes, the first included.
    pub count: Option<u32>,
    /// The last time it may come.
    pub until: Option<EventTime>,
    /// The days of the week it comes on, each with the place of the day in
    /// its month or year (1 the first, -1 the last), or none for every one.
    pub by_day: Vec<(Option<i32>, Weekday)>,
    /// The days of the month it comes on, -1 the last.
    pub by_month_day: Vec<i32>,
    /// The months it comes in, 1 to 12.
    pub by_month: Vec<u32>,
}

/// An event of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub start: EventTime,
    /// When it ends: given by `DTEND`, or by `DTSTART` and `DURATION`;
    /// `None` where neither says.
    pub end: Option<EventTime>,
    pub rule: Option<Rule>,
    /// The starts the rule gives that do not come.
    pub except: Vec<EventTime>,
    /// The start the rule gave that this event replaces.
    pub replaces: Option<EventTime>,
    /// Whether it does not take place (`STATUS:CANCELLED`); one that
    /// replaces a start takes that start out.
    pub cancelled: bool,
}

impl Event {
    fn all_day(&self) -> bool {
        self.start.stamp == Stamp::Date
    }

    /// How long it lasts: to its end, or a day for an event of all day and
    /// nothing for one at a time without an end.
    fn length(&self) -> Duration {
        match &self.end {
            Some(end) if end.at >= self.start.at => end.at - self.start.at,
            _ if self.all_day() => Duration::days(1),
            _ => Duration::zero(),
        }
    }
}

/// One time an event comes, in local time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub summary: String,
    pub location: String,
    pub start: NaiveDateTime,
    /// After the start, or at it for an event that takes no time; the day
    /// after the last one for an event of all day.
    pub end: NaiveDateTime,
    pub all_day: bool,
}

impl Occurrence {
    /// Whether it takes place on `day`, or at its start for an event that
    /// takes no time.
    pub fn on(&self, day: NaiveDate) -> bool {
        let (from, to) = (
            day.and_time(NaiveTime::MIN),
            day.and_time(NaiveTime::MIN) + Duration::days(1),
        );
        if self.end <= self.start {
            return self.start >= from && self.start < to;
        }
        self.start < to && self.end > from
    }
}

/// The events of the iCalendar text `text`; what cannot be read of it is
/// left out.
pub fn parse(text: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut current: Option<Draft> = None;
    // How deep inside the event the reading is: an alarm inside it has
    // lines of its own.
    let mut inner = 0usize;
    for line in unfold(text) {
        let Some(line) = Line::read(&line) else {
            continue;
        };
        match (line.name.as_str(), line.value.to_ascii_uppercase().as_str()) {
            ("BEGIN", "VEVENT") if current.is_none() => {
                current = Some(Draft::default());
                inner = 0;
                continue;
            }
            ("BEGIN", _) if current.is_some() => inner += 1,
            ("END", "VEVENT") if inner == 0 => {
                if let Some(event) = current.take().and_then(Draft::finish) {
                    events.push(event);
                }
                continue;
            }
            ("END", _) if current.is_some() => inner = inner.saturating_sub(1),
            _ => {}
        }
        if let Some(draft) = current.as_mut().filter(|_| inner == 0) {
            draft.take(&line);
        }
    }
    events
}

/// The times the events come between `from` and `to`, both included, in
/// local time, by their start. `to_local` turns a time of an event into
/// local time, `None` where it cannot.
pub fn occurrences(
    events: &[Event],
    from: NaiveDate,
    to: NaiveDate,
    to_local: &dyn Fn(&EventTime) -> Option<NaiveDateTime>,
) -> Vec<Occurrence> {
    let window_start = from.and_time(NaiveTime::MIN);
    let window_end = to.and_time(NaiveTime::MIN) + Duration::days(1);
    // The starts other events take the place of, by event.
    let moved: Vec<(&str, &EventTime)> = { events.iter() }
        .filter_map(|event| Some((event.uid.as_str(), event.replaces.as_ref()?)))
        .collect();
    let mut found = Vec::new();
    for event in events.iter().filter(|event| !event.cancelled) {
        let length = event.length();
        let all_day = event.all_day();
        let gone = |start: &NaiveDateTime| {
            let same =
                |time: &EventTime| time.at == *start || (all_day && time.at.date() == start.date());
            event.except.iter().any(same)
                || (event.replaces.is_none()
                    && moved
                        .iter()
                        .any(|(uid, time)| *uid == event.uid && same(time)))
        };
        let starts = match &event.rule {
            Some(rule) => expand(&event.start, rule, (window_start, window_end), to_local),
            None => vec![event.start.at],
        };
        for start in starts.into_iter().filter(|start| !gone(start)) {
            let written = EventTime {
                at: start,
                stamp: event.start.stamp.clone(),
            };
            let Some(local) = to_local(&written) else {
                continue;
            };
            let occurrence = Occurrence {
                summary: event.summary.clone(),
                location: event.location.clone(),
                start: local,
                end: local + length,
                all_day,
            };
            let ends = occurrence.end.max(occurrence.start + Duration::seconds(1));
            if occurrence.start < window_end && ends > window_start {
                found.push(occurrence);
            }
        }
    }
    found.sort_by(|one, other| {
        { other.all_day.cmp(&one.all_day) }
            .then(one.start.cmp(&other.start))
            .then(one.summary.cmp(&other.summary))
    });
    found
}

/// What turns a time of an event into local time: as it is for a day and
/// a floating time, from universal time by the zone of the machine, and
/// from the zone `TZID` names as Windows has it, or as a floating time
/// where no zone of Tessera has that name. Each zone is looked up once.
pub fn local_times() -> impl Fn(&EventTime) -> Option<NaiveDateTime> {
    let zones: RefCell<HashMap<String, Option<Zone>>> = RefCell::default();
    move |time: &EventTime| {
        let from_utc = |utc: NaiveDateTime| {
            let utc: DateTime<Utc> = Utc.from_utc_datetime(&utc);
            utc.with_timezone(&Local).naive_local()
        };
        match &time.stamp {
            Stamp::Date | Stamp::Floating => Some(time.at),
            Stamp::Utc => Some(from_utc(time.at)),
            Stamp::Zoned(name) => {
                let mut zones = zones.borrow_mut();
                let zone = { zones.entry(name.clone()) }.or_insert_with(|| time_zones::find(name));
                match zone {
                    Some(zone) => zone.utc_of(time.at).map(from_utc),
                    None => Some(time.at),
                }
            }
        }
    }
}

/// The starts `rule` gives an event that first starts at `start`, up to
/// the end of `window`, in the time the event is written in. A rule
/// without a count starts near the window rather than at the first start.
fn expand(
    start: &EventTime,
    rule: &Rule,
    window: (NaiveDateTime, NaiveDateTime),
    to_local: &dyn Fn(&EventTime) -> Option<NaiveDateTime>,
) -> Vec<NaiveDateTime> {
    let (after, before) = window;
    // `UNTIL` may be in universal time while the event is in a zone: both
    // are compared in local time.
    let until = rule.until.as_ref().and_then(|until| {
        let local = to_local(until)?;
        match until.stamp {
            // A day as the end takes in the whole of it.
            Stamp::Date => Some(local + Duration::days(1) - Duration::seconds(1)),
            _ => Some(local),
        }
    });
    let local_of = |at: NaiveDateTime| {
        to_local(&EventTime {
            at,
            stamp: start.stamp.clone(),
        })
        .unwrap_or(at)
    };
    // The margin covers the difference between the zone of the event and
    // the local one.
    let before = before + Duration::days(2);
    let interval = rule.interval.max(1);
    let time = start.at.time();
    let first = start.at.date();
    let mut starts = Vec::new();
    let mut counted = 0u32;
    // The first start always counts, even where the rule would not give it.
    let mut take = |at: NaiveDateTime, starts: &mut Vec<NaiveDateTime>| -> bool {
        if at < start.at || starts.last().is_some_and(|last| at <= *last) {
            return true;
        }
        if rule.count.is_some_and(|count| counted >= count)
            || until.is_some_and(|until| local_of(at) > until)
            || at >= before
        {
            return false;
        }
        counted += 1;
        starts.push(at);
        true
    };
    if !take(start.at, &mut starts) {
        return starts;
    }
    let skipped = match rule.count {
        Some(_) => 0,
        None => steps_before(start.at.date(), after.date() - Days::new(2), rule),
    };
    for step in skipped..skipped.saturating_add(MOST_STEPS as u32) {
        let days: Vec<NaiveDate> = match rule.frequency {
            Frequency::Daily => {
                let Some(day) = first.checked_add_days(Days::new(u64::from(step * interval)))
                else {
                    break;
                };
                vec![day]
                    .into_iter()
                    .filter(|day| rule.keeps_day(*day))
                    .collect()
            }
            Frequency::Weekly => {
                let monday = first - Days::new(u64::from(first.weekday().num_days_from_monday()));
                let Some(week) = monday.checked_add_days(Days::new(u64::from(step * interval) * 7))
                else {
                    break;
                };
                let weekdays: Vec<Weekday> = match rule.by_day.is_empty() {
                    true => vec![first.weekday()],
                    false => rule.by_day.iter().map(|(_, day)| *day).collect(),
                };
                let mut days: Vec<NaiveDate> = { weekdays.into_iter() }
                    .map(|weekday| week + Days::new(u64::from(weekday.num_days_from_monday())))
                    .filter(|day| rule.by_month.is_empty() || rule.by_month.contains(&day.month()))
                    .collect();
                days.sort();
                days
            }
            Frequency::Monthly => {
                let Some(month) =
                    month_start(first).checked_add_months(Months::new(step * interval))
                else {
                    break;
                };
                if !rule.by_month.is_empty() && !rule.by_month.contains(&month.month()) {
                    Vec::new()
                } else {
                    rule.days_in_month(month, first)
                }
            }
            Frequency::Yearly => {
                let Some(year) = first.year().checked_add((step * interval) as i32) else {
                    break;
                };
                let months: Vec<u32> = match rule.by_month.is_empty() {
                    true => vec![first.month()],
                    false => rule.by_month.clone(),
                };
                let mut days: Vec<NaiveDate> = { months.into_iter() }
                    .filter_map(|month| NaiveDate::from_ymd_opt(year, month, 1))
                    .flat_map(|month| rule.days_in_month(month, first))
                    .collect();
                // BYDAY with a place and no month counts in the year.
                if rule.by_month.is_empty() && rule.by_day.iter().any(|(place, _)| place.is_some())
                {
                    days = rule.days_in_year(year);
                }
                days.sort();
                days
            }
        };
        let mut going = true;
        for day in days {
            if !take(day.and_time(time), &mut starts) {
                going = false;
                break;
            }
        }
        // Past the window, or done: nothing later comes.
        let passed = match rule.frequency {
            Frequency::Daily => first + Days::new(u64::from(step * interval)),
            Frequency::Weekly => first + Days::new(u64::from(step * interval) * 7),
            Frequency::Monthly => month_start(first)
                .checked_add_months(Months::new(step * interval))
                .unwrap_or(NaiveDate::MAX),
            Frequency::Yearly => {
                NaiveDate::from_ymd_opt(first.year() + (step * interval) as i32, 1, 1)
                    .unwrap_or(NaiveDate::MAX)
            }
        };
        if !going || passed.and_time(time) >= before {
            break;
        }
    }
    starts
}

/// How many periods of `rule` lie wholly between `first` and `day`, which
/// a rule without a count need not go through.
fn steps_before(first: NaiveDate, day: NaiveDate, rule: &Rule) -> u32 {
    if day <= first {
        return 0;
    }
    let interval = rule.interval.max(1);
    let periods = match rule.frequency {
        Frequency::Daily => (day - first).num_days(),
        Frequency::Weekly => (day - first).num_days() / 7,
        Frequency::Monthly => {
            i64::from(day.year() - first.year()) * 12 + i64::from(day.month())
                - i64::from(first.month())
        }
        Frequency::Yearly => i64::from(day.year() - first.year()),
    };
    // One period less, so that the one the day is in is gone through.
    let steps = periods / i64::from(interval) - 1;
    u32::try_from(steps.max(0)).unwrap_or(u32::MAX)
}

fn month_start(day: NaiveDate) -> NaiveDate {
    day.with_day(1).unwrap_or(day)
}

/// The days of the month that starts on `month`.
fn month_days(month: NaiveDate) -> Vec<NaiveDate> {
    month
        .iter_days()
        .take_while(|day| day.month() == month.month())
        .collect()
}

/// The days of `days` that are the weekday `weekday` at `place` among
/// them, or all of them without a place.
fn weekdays_at(days: &[NaiveDate], weekday: Weekday, place: Option<i32>) -> Vec<NaiveDate> {
    let matching: Vec<NaiveDate> = { days.iter() }
        .copied()
        .filter(|day| day.weekday() == weekday)
        .collect();
    match place {
        None => matching,
        Some(place) if place > 0 => matching
            .get(place as usize - 1)
            .copied()
            .into_iter()
            .collect(),
        Some(place) if place < 0 => {
            let back = place.unsigned_abs() as usize;
            matching
                .len()
                .checked_sub(back)
                .and_then(|index| matching.get(index))
                .copied()
                .into_iter()
                .collect()
        }
        Some(_) => Vec::new(),
    }
}

impl Rule {
    /// Whether a daily rule keeps `day`.
    fn keeps_day(&self, day: NaiveDate) -> bool {
        (self.by_day.is_empty()
            || self
                .by_day
                .iter()
                .any(|(_, weekday)| *weekday == day.weekday()))
            && (self.by_month.is_empty() || self.by_month.contains(&day.month()))
            && (self.by_month_day.is_empty()
                || self
                    .by_month_day
                    .iter()
                    .any(|wanted| month_day_is(day, *wanted)))
    }

    /// The days of the month starting on `month` the rule comes on, for an
    /// event that first started on `first`.
    fn days_in_month(&self, month: NaiveDate, first: NaiveDate) -> Vec<NaiveDate> {
        let days = month_days(month);
        let mut chosen: Vec<NaiveDate> =
            match (self.by_day.is_empty(), self.by_month_day.is_empty()) {
                (true, true) => { days.iter() }
                    .copied()
                    .filter(|day| day.day() == first.day())
                    .collect(),
                (true, false) => { days.iter() }
                    .copied()
                    .filter(|day| {
                        self.by_month_day
                            .iter()
                            .any(|wanted| month_day_is(*day, *wanted))
                    })
                    .collect(),
                (false, by_month_day) => {
                    let mut chosen: Vec<NaiveDate> = { self.by_day.iter() }
                        .flat_map(|(place, weekday)| weekdays_at(&days, *weekday, *place))
                        .collect();
                    if !by_month_day {
                        chosen.retain(|day| {
                            self.by_month_day
                                .iter()
                                .any(|wanted| month_day_is(*day, *wanted))
                        });
                    }
                    chosen
                }
            };
        chosen.sort();
        chosen.dedup();
        chosen
    }

    /// The days of `year` its weekdays with a place in the year give.
    fn days_in_year(&self, year: i32) -> Vec<NaiveDate> {
        let (Some(start), Some(end)) = (
            NaiveDate::from_ymd_opt(year, 1, 1),
            NaiveDate::from_ymd_opt(year, 12, 31),
        ) else {
            return Vec::new();
        };
        let days: Vec<NaiveDate> = start.iter_days().take_while(|day| *day <= end).collect();
        let mut chosen: Vec<NaiveDate> = { self.by_day.iter() }
            .flat_map(|(place, weekday)| weekdays_at(&days, *weekday, *place))
            .collect();
        chosen.sort();
        chosen.dedup();
        chosen
    }
}

/// Whether `day` is the day `wanted` of its month, counted from the end
/// where it is below 0.
fn month_day_is(day: NaiveDate, wanted: i32) -> bool {
    if wanted > 0 {
        return day.day() == wanted as u32;
    }
    let last = month_days(month_start(day)).len() as i32;
    wanted < 0 && day.day() as i32 == last + 1 + wanted
}

/// The lines of `text`, those folded onto more lines joined again.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        match (line.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ => lines.push(line.to_owned()),
        }
    }
    lines
}

/// A line of the content: its name in capitals, its parameters and its
/// value.
struct Line {
    name: String,
    parameters: Vec<(String, String)>,
    value: String,
}

impl Line {
    fn read(line: &str) -> Option<Self> {
        // The value starts at the first colon outside quotes.
        let mut quoted = false;
        let colon = line.char_indices().find_map(|(index, letter)| {
            match letter {
                '"' => quoted = !quoted,
                ':' if !quoted => return Some(index),
                _ => {}
            }
            None
        })?;
        let (head, value) = (&line[..colon], &line[colon + 1..]);
        let mut parts = head.split(';');
        let name = parts.next()?.trim().to_ascii_uppercase();
        if name.is_empty() {
            return None;
        }
        let parameters = { parts }
            .filter_map(|part| {
                let (key, value) = part.split_once('=')?;
                Some((
                    key.trim().to_ascii_uppercase(),
                    value.trim().trim_matches('"').to_owned(),
                ))
            })
            .collect();
        Some(Self {
            name,
            parameters,
            value: value.to_owned(),
        })
    }

    fn parameter(&self, key: &str) -> Option<&str> {
        { self.parameters.iter() }
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// The times of the value, as `DTSTART` and `EXDATE` write them.
    fn times(&self) -> Vec<EventTime> {
        let date = self
            .parameter("VALUE")
            .is_some_and(|value| value.eq_ignore_ascii_case("DATE"));
        let zone = self.parameter("TZID").map(str::to_owned);
        { self.value.split(',') }
            .filter_map(|value| time_of(value.trim(), date, zone.clone()))
            .collect()
    }
}

/// A time written as `20261127`, `20261127T140000` or `20261127T130000Z`.
fn time_of(value: &str, date: bool, zone: Option<String>) -> Option<EventTime> {
    let day = NaiveDate::parse_from_str(value.get(..8)?, "%Y%m%d").ok()?;
    if date || value.len() == 8 {
        return Some(EventTime {
            at: day.and_time(NaiveTime::MIN),
            stamp: Stamp::Date,
        });
    }
    let rest = value.get(8..)?.strip_prefix(['T', 't'])?;
    let (clock, utc) = match rest.strip_suffix(['Z', 'z']) {
        Some(clock) => (clock, true),
        None => (rest, false),
    };
    let time = NaiveTime::parse_from_str(clock.get(..6)?, "%H%M%S").ok()?;
    let stamp = match (utc, zone) {
        (true, _) => Stamp::Utc,
        (false, Some(zone)) => Stamp::Zoned(zone),
        (false, None) => Stamp::Floating,
    };
    Some(EventTime {
        at: day.and_time(time),
        stamp,
    })
}

/// A length written as `P1D`, `PT1H30M`, `P2W` or `-PT15M`.
fn duration_of(value: &str) -> Option<Duration> {
    let value = value.trim();
    let (negative, value) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value.strip_prefix('+').unwrap_or(value)),
    };
    let value = value.strip_prefix(['P', 'p'])?;
    let mut total = Duration::zero();
    let mut number = String::new();
    let mut timed = false;
    for letter in value.chars() {
        match letter.to_ascii_uppercase() {
            'T' => timed = true,
            digit if digit.is_ascii_digit() => number.push(digit),
            unit => {
                let count: i64 = number.parse().ok()?;
                number.clear();
                total += match (unit, timed) {
                    ('W', _) => Duration::weeks(count),
                    ('D', _) => Duration::days(count),
                    ('H', true) => Duration::hours(count),
                    ('M', true) => Duration::minutes(count),
                    ('S', true) => Duration::seconds(count),
                    _ => return None,
                };
            }
        }
    }
    Some(if negative { -total } else { total })
}

/// The text of a value, without the escapes of iCalendar.
fn text_of(value: &str) -> String {
    let mut text = String::with_capacity(value.len());
    let mut letters = value.chars();
    while let Some(letter) = letters.next() {
        if letter != '\\' {
            text.push(letter);
            continue;
        }
        match letters.next() {
            Some('n' | 'N') => text.push('\n'),
            Some(other) => text.push(other),
            None => {}
        }
    }
    text.trim().to_owned()
}

fn weekday_of(name: &str) -> Option<Weekday> {
    Some(match name.to_ascii_uppercase().as_str() {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

/// The rule an `RRULE` value writes; `None` for one Tessera cannot follow,
/// as `FREQ=HOURLY`.
fn rule_of(value: &str) -> Option<Rule> {
    let mut rule = Rule {
        frequency: Frequency::Daily,
        interval: 1,
        count: None,
        until: None,
        by_day: Vec::new(),
        by_month_day: Vec::new(),
        by_month: Vec::new(),
    };
    let mut frequency = None;
    for part in value.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let list = || value.split(',').map(str::trim);
        match key.trim().to_ascii_uppercase().as_str() {
            "FREQ" => {
                frequency = Some(match value.trim().to_ascii_uppercase().as_str() {
                    "DAILY" => Frequency::Daily,
                    "WEEKLY" => Frequency::Weekly,
                    "MONTHLY" => Frequency::Monthly,
                    "YEARLY" => Frequency::Yearly,
                    _ => return None,
                });
            }
            "INTERVAL" => {
                rule.interval = value.trim().parse().ok().filter(|interval| *interval > 0)?
            }
            "COUNT" => rule.count = Some(value.trim().parse().ok()?),
            "UNTIL" => rule.until = time_of(value.trim(), false, None),
            "BYDAY" => {
                for day in list() {
                    let split = day.len().checked_sub(2)?;
                    let (place, name) = day.split_at(split);
                    let place = match place.trim() {
                        "" => None,
                        place => Some(place.trim_start_matches('+').parse::<i32>().ok()?),
                    };
                    rule.by_day.push((place, weekday_of(name)?));
                }
            }
            "BYMONTHDAY" => {
                for day in list() {
                    rule.by_month_day.push(day.parse().ok()?);
                }
            }
            "BYMONTH" => {
                for month in list() {
                    rule.by_month.push(
                        month
                            .parse()
                            .ok()
                            .filter(|month| (1..=12).contains(month))?,
                    );
                }
            }
            // What the week starts with does not change the basic rules
            // Tessera follows.
            "WKST" => {}
            // BYSETPOS, BYYEARDAY, BYWEEKNO and the like are not
            // followed: the event is shown at its first start only.
            _ => return None,
        }
    }
    rule.frequency = frequency?;
    Some(rule)
}

/// An event while its lines are read.
#[derive(Default)]
struct Draft {
    uid: String,
    summary: String,
    location: String,
    start: Option<EventTime>,
    end: Option<EventTime>,
    duration: Option<Duration>,
    rule: Option<Rule>,
    except: Vec<EventTime>,
    replaces: Option<EventTime>,
    cancelled: bool,
}

impl Draft {
    fn take(&mut self, line: &Line) {
        match line.name.as_str() {
            "UID" => self.uid = line.value.trim().to_owned(),
            "SUMMARY" => self.summary = text_of(&line.value),
            "LOCATION" => self.location = text_of(&line.value),
            "DTSTART" => self.start = line.times().into_iter().next(),
            "DTEND" => self.end = line.times().into_iter().next(),
            "DURATION" => self.duration = duration_of(&line.value),
            "RRULE" => self.rule = rule_of(&line.value),
            "EXDATE" => self.except.extend(line.times()),
            "RECURRENCE-ID" => self.replaces = line.times().into_iter().next(),
            "STATUS" => self.cancelled = line.value.trim().eq_ignore_ascii_case("CANCELLED"),
            _ => {}
        }
    }

    fn finish(self) -> Option<Event> {
        let start = self.start?;
        let end = self.end.or_else(|| {
            Some(EventTime {
                at: start.at + self.duration?,
                stamp: start.stamp.clone(),
            })
        });
        Some(Event {
            uid: self.uid,
            summary: self.summary,
            location: self.location,
            start,
            end,
            rule: self.rule,
            except: self.except,
            replaces: self.replaces,
            cancelled: self.cancelled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn at(year: i32, month: u32, date: u32, hour: u32, minute: u32) -> NaiveDateTime {
        day(year, month, date).and_hms_opt(hour, minute, 0).unwrap()
    }

    /// Times as they are written: no zone moves them.
    fn same(time: &EventTime) -> Option<NaiveDateTime> {
        Some(time.at)
    }

    fn calendar(events: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}END:VCALENDAR\r\n")
    }

    fn starts(text: &str, from: NaiveDate, to: NaiveDate) -> Vec<NaiveDateTime> {
        let events = parse(&calendar(text));
        occurrences(&events, from, to, &same)
            .into_iter()
            .map(|occurrence| occurrence.start)
            .collect()
    }

    #[test]
    fn an_event_is_read_with_its_times_and_texts() {
        let text = calendar(
            "BEGIN:VEVENT\r\nUID:1\r\nSUMMARY:Lunch\\, with Ann\r\nLOCATION:Caf\r\n \u{e9}\r\n\
             DTSTART;TZID=\"Europe/Berlin\":20261127T120000\r\nDURATION:PT1H30M\r\n\
             BEGIN:VALARM\r\nSUMMARY:not this\r\nEND:VALARM\r\nEND:VEVENT\r\n",
        );
        let events = parse(&text);
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.summary, "Lunch, with Ann");
        assert_eq!(event.location, "Café");
        assert_eq!(event.start.stamp, Stamp::Zoned("Europe/Berlin".to_owned()));
        assert_eq!(event.start.at, at(2026, 11, 27, 12, 0));
        assert_eq!(event.end.as_ref().unwrap().at, at(2026, 11, 27, 13, 30));
    }

    #[test]
    fn times_are_read_in_each_of_their_forms() {
        assert_eq!(
            time_of("20261127T130000Z", false, None).unwrap().stamp,
            Stamp::Utc
        );
        assert_eq!(
            time_of("20261127T130000", false, None).unwrap().stamp,
            Stamp::Floating
        );
        assert_eq!(time_of("20261127", false, None).unwrap().stamp, Stamp::Date);
        assert!(time_of("2026-11-27", false, None).is_none());
        assert_eq!(duration_of("P1D"), Some(Duration::days(1)));
        assert_eq!(duration_of("P1W2DT3H"), Some(Duration::hours(9 * 24 + 3)));
        assert_eq!(duration_of("-PT15M"), Some(Duration::minutes(-15)));
        assert_eq!(duration_of("PT"), Some(Duration::zero()));
        assert_eq!(duration_of("1H"), None);
    }

    #[test]
    fn an_event_of_all_day_lasts_its_days() {
        let text = calendar(
            "BEGIN:VEVENT\r\nUID:h\r\nSUMMARY:Holiday\r\nDTSTART;VALUE=DATE:20261224\r\n\
             DTEND;VALUE=DATE:20261227\r\nEND:VEVENT\r\n",
        );
        let found = occurrences(&parse(&text), day(2026, 12, 1), day(2026, 12, 31), &same);
        assert_eq!(found.len(), 1);
        let holiday = &found[0];
        assert!(holiday.all_day);
        assert!(!holiday.on(day(2026, 12, 23)));
        assert!(holiday.on(day(2026, 12, 24)) && holiday.on(day(2026, 12, 26)));
        assert!(!holiday.on(day(2026, 12, 27)));
        // A day without an end is that day.
        let single =
            calendar("BEGIN:VEVENT\r\nUID:s\r\nDTSTART;VALUE=DATE:20261224\r\nEND:VEVENT\r\n");
        let found = occurrences(&parse(&single), day(2026, 12, 24), day(2026, 12, 24), &same);
        assert!(found[0].on(day(2026, 12, 24)) && !found[0].on(day(2026, 12, 25)));
    }

    #[test]
    fn a_weekly_rule_comes_on_its_days_until_its_count() {
        // 2 November 2026 is a Monday.
        let found = starts(
            "BEGIN:VEVENT\r\nUID:w\r\nDTSTART:20261102T090000\r\n\
             RRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=5\r\nEND:VEVENT\r\n",
            day(2026, 11, 1),
            day(2026, 12, 31),
        );
        assert_eq!(
            found,
            [
                at(2026, 11, 2, 9, 0),
                at(2026, 11, 4, 9, 0),
                at(2026, 11, 9, 9, 0),
                at(2026, 11, 11, 9, 0),
                at(2026, 11, 16, 9, 0)
            ]
        );
        // Every other week, until a day.
        let found = starts(
            "BEGIN:VEVENT\r\nUID:w\r\nDTSTART:20261102T090000\r\n\
             RRULE:FREQ=WEEKLY;INTERVAL=2;UNTIL=20261130\r\nEND:VEVENT\r\n",
            day(2026, 11, 1),
            day(2026, 12, 31),
        );
        assert_eq!(
            found,
            [
                at(2026, 11, 2, 9, 0),
                at(2026, 11, 16, 9, 0),
                at(2026, 11, 30, 9, 0)
            ]
        );
    }

    #[test]
    fn a_daily_rule_leaves_out_what_exdate_takes_out() {
        let found = starts(
            "BEGIN:VEVENT\r\nUID:d\r\nDTSTART:20261127T080000Z\r\nRRULE:FREQ=DAILY\r\n\
             EXDATE:20261128T080000Z,20261130T080000Z\r\nEND:VEVENT\r\n",
            day(2026, 11, 27),
            day(2026, 12, 1),
        );
        assert_eq!(
            found,
            [
                at(2026, 11, 27, 8, 0),
                at(2026, 11, 29, 8, 0),
                at(2026, 12, 1, 8, 0)
            ]
        );
    }

    #[test]
    fn monthly_and_yearly_rules_find_their_days() {
        // The last Friday of each month.
        let found = starts(
            "BEGIN:VEVENT\r\nUID:m\r\nDTSTART:20261030T170000\r\n\
             RRULE:FREQ=MONTHLY;BYDAY=-1FR;COUNT=3\r\nEND:VEVENT\r\n",
            day(2026, 10, 1),
            day(2027, 6, 1),
        );
        assert_eq!(
            found,
            [
                at(2026, 10, 30, 17, 0),
                at(2026, 11, 27, 17, 0),
                at(2026, 12, 25, 17, 0)
            ]
        );
        // The 31st comes only in months that have it.
        let found = starts(
            "BEGIN:VEVENT\r\nUID:m\r\nDTSTART:20261031T100000\r\nRRULE:FREQ=MONTHLY\r\nEND:VEVENT\r\n",
            day(2026, 11, 1),
            day(2027, 1, 31),
        );
        assert_eq!(found, [at(2026, 12, 31, 10, 0), at(2027, 1, 31, 10, 0)]);
        // The fourth Thursday of November, from a file of holidays.
        let found = starts(
            "BEGIN:VEVENT\r\nUID:t\r\nDTSTART;VALUE=DATE:20241128\r\n\
             RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=4TH\r\nEND:VEVENT\r\n",
            day(2026, 1, 1),
            day(2027, 12, 31),
        );
        assert_eq!(found, [at(2026, 11, 26, 0, 0), at(2027, 11, 25, 0, 0)]);
        // A birthday.
        let found = starts(
            "BEGIN:VEVENT\r\nUID:b\r\nDTSTART;VALUE=DATE:20000229\r\nRRULE:FREQ=YEARLY\r\nEND:VEVENT\r\n",
            day(2026, 1, 1),
            day(2028, 12, 31),
        );
        assert_eq!(found, [at(2028, 2, 29, 0, 0)]);
    }

    #[test]
    fn a_moved_time_replaces_the_one_of_the_rule() {
        let text = "BEGIN:VEVENT\r\nUID:x\r\nSUMMARY:Standup\r\nDTSTART:20261123T090000\r\n\
                    RRULE:FREQ=DAILY;COUNT=3\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:x\r\nSUMMARY:Standup\r\nRECURRENCE-ID:20261124T090000\r\n\
                    DTSTART:20261124T110000\r\nEND:VEVENT\r\n";
        assert_eq!(
            starts(text, day(2026, 11, 23), day(2026, 11, 30)),
            [
                at(2026, 11, 23, 9, 0),
                at(2026, 11, 24, 11, 0),
                at(2026, 11, 25, 9, 0)
            ]
        );
    }

    #[test]
    fn a_cancelled_time_is_not_shown() {
        let text = "BEGIN:VEVENT\r\nUID:x\r\nDTSTART:20261123T090000\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:x\r\nRECURRENCE-ID:20261124T090000\r\nSTATUS:CANCELLED\r\n\
                    DTSTART:20261124T090000\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:y\r\nSTATUS:CANCELLED\r\nDTSTART:20261124T120000\r\nEND:VEVENT\r\n";
        assert_eq!(
            starts(text, day(2026, 11, 23), day(2026, 11, 30)),
            [at(2026, 11, 23, 9, 0), at(2026, 11, 25, 9, 0)]
        );
    }

    #[test]
    fn a_rule_without_a_count_starts_near_the_window() {
        let found = starts(
            "BEGIN:VEVENT\r\nUID:old\r\nDTSTART:19700101T070000\r\nRRULE:FREQ=DAILY\r\nEND:VEVENT\r\n",
            day(2026, 11, 27),
            day(2026, 11, 28),
        );
        assert_eq!(found, [at(2026, 11, 27, 7, 0), at(2026, 11, 28, 7, 0)]);
        let rule = rule_of("FREQ=MONTHLY;INTERVAL=2").unwrap();
        assert_eq!(steps_before(day(2026, 1, 15), day(2026, 11, 1), &rule), 4);
        assert_eq!(steps_before(day(2026, 1, 15), day(2025, 11, 1), &rule), 0);
    }

    #[test]
    fn a_rule_tessera_cannot_follow_shows_the_first_time_only() {
        assert!(rule_of("FREQ=HOURLY").is_none());
        assert!(rule_of("FREQ=MONTHLY;BYSETPOS=-1;BYDAY=MO,TU").is_none());
        let rule = rule_of("FREQ=WEEKLY;WKST=SU;BYDAY=+1MO").unwrap();
        assert_eq!(rule.by_day, [(Some(1), Weekday::Mon)]);
    }

    #[test]
    fn occurrences_come_all_day_first_then_by_start() {
        let text = "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Late\r\nDTSTART:20261127T180000\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Early\r\nDTSTART:20261127T080000\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:c\r\nSUMMARY:Day\r\nDTSTART;VALUE=DATE:20261127\r\nEND:VEVENT\r\n";
        let found = occurrences(
            &parse(&calendar(text)),
            day(2026, 11, 27),
            day(2026, 11, 27),
            &same,
        );
        let names: Vec<&str> = found
            .iter()
            .map(|occurrence| occurrence.summary.as_str())
            .collect();
        assert_eq!(names, ["Day", "Early", "Late"]);
    }
}
