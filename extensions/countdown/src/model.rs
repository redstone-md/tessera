// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Named dates and how far it is to them.

use tessera_extension_kit::settings::parse_pairs;
use tessera_extension_kit::store::Record;
use tessera_extension_kit::time::parse_date;

/// A named date.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub name: String,
    /// Seconds since 1970, UTC.
    pub at: u64,
}

/// Reads dates written as `New Year=2027-01-01; Launch=2027-03-02 14:30`,
/// sorted by when they are.
pub fn parse_events(text: &str) -> Result<Vec<Event>, String> {
    let mut events = parse_pairs(text)?
        .into_iter()
        .map(|(name, date)| match parse_date(&date) {
            Some(at) => Ok(Event { name, at }),
            None => Err(format!(
                "`{date}` of {name} is not a date such as 2027-01-01 or 2027-01-01 09:30"
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    events.sort_by_key(|event| event.at);
    Ok(events)
}

/// The dates and when each was first seen.
pub struct Countdown {
    pub events: Vec<Event>,
    seen: Record,
}

impl Countdown {
    /// The dates, with when they were first seen read from `seen`; those
    /// not seen before are seen at `now`. Whether `seen` changed.
    pub fn new(events: Vec<Event>, mut seen: Record, now: u64) -> (Self, bool) {
        let before = seen.clone();
        let key = |event: &Event| format!("seen.{}", event.name);
        seen.retain(|known| events.iter().any(|event| key(event) == known));
        for event in &events {
            if seen.get::<u64>(&key(event)).is_none() {
                seen.set(&key(event), now);
            }
        }
        let changed = seen != before;
        (Self { events, seen }, changed)
    }

    pub fn seen(&self) -> &Record {
        &self.seen
    }

    /// The nearest date after `now`.
    pub fn next(&self, now: u64) -> Option<&Event> {
        self.events.iter().find(|event| event.at > now)
    }

    /// How far it is from when `event` was first seen to it, from 0 to 1.
    pub fn progress(&self, event: &Event, now: u64) -> f32 {
        let seen = self
            .seen
            .get(&format!("seen.{}", event.name))
            .unwrap_or(now);
        if event.at <= seen || now >= event.at {
            return 1.;
        }
        now.saturating_sub(seen) as f32 / (event.at - seen) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEW_YEAR: u64 = 1_798_761_600;

    #[test]
    fn events_are_read_and_sorted() {
        let events = parse_events("New Year=2027-01-01; Eve = 2026-12-31 18:00").unwrap();
        assert_eq!(events[0].name, "Eve");
        assert_eq!(events[0].at, NEW_YEAR - 6 * 3600);
        assert_eq!(events[1].at, NEW_YEAR);
        assert!(parse_events("Party=soon").unwrap_err().contains("Party"));
        assert!(parse_events("Party").is_err());
    }

    #[test]
    fn the_nearest_date_ahead_is_next() {
        let events = parse_events("A=2027-01-01; B=2027-01-03").unwrap();
        let (countdown, _) = Countdown::new(events, Record::default(), 0);
        assert_eq!(countdown.next(NEW_YEAR - 1).unwrap().name, "A");
        assert_eq!(countdown.next(NEW_YEAR).unwrap().name, "B");
        assert!(countdown.next(NEW_YEAR + 3 * 86_400).is_none());
    }

    #[test]
    fn progress_runs_from_first_sight_to_the_date() {
        let events = parse_events("A=2027-01-01; Gone=2026-01-01").unwrap();
        let start = NEW_YEAR - 1000;
        let stale = Record::parse("seen.Old=5\n");
        let (countdown, changed) = Countdown::new(events.clone(), stale, start);
        assert!(changed);
        assert_eq!(countdown.seen().text("seen.Old"), None);
        let a = &countdown.events[1];
        assert_eq!(countdown.progress(a, start), 0.);
        assert_eq!(countdown.progress(a, start + 250), 0.25);
        assert_eq!(countdown.progress(&countdown.events[0], start), 1.);
        // Seen again after a restart, the dates keep when they were seen.
        let (again, changed) = Countdown::new(events, countdown.seen().clone(), start + 500);
        assert!(!changed);
        assert_eq!(again.progress(&again.events[1], start + 500), 0.5);
    }
}
