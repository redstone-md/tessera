// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The phases of the timer and how it moves through them.

use tessera_extension_kit::store::Record;
use tessera_extension_kit::time::{HOUR, MINUTE};

/// A phase that ended longer ago than this when the timer was looked at
/// is not counted: Tessera was likely not running, nor anyone working.
const STALE_AFTER: u64 = HOUR;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Work,
    Break,
    LongBreak,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Work => "Focus",
            Self::Break => "Break",
            Self::LongBreak => "Long break",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Break => "break",
            Self::LongBreak => "long",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        [Self::Work, Self::Break, Self::LongBreak]
            .into_iter()
            .find(|phase| phase.key() == key)
    }
}

/// The lengths of the phases, in seconds, and how many rounds of work
/// come before a long break.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub work: u64,
    pub short: u64,
    pub long: u64,
    pub rounds: u32,
    /// Whether a phase starts by itself when the one before ends.
    pub auto_start: bool,
}

impl Plan {
    pub fn length(&self, phase: Phase) -> u64 {
        match phase {
            Phase::Work => self.work,
            Phase::Break => self.short,
            Phase::LongBreak => self.long,
        }
        .max(MINUTE)
    }
}

/// Where the timer is. Times are seconds since 1970, so that the timer
/// reads the same after a restart.
#[derive(Clone, Debug, PartialEq)]
pub struct Timer {
    pub phase: Phase,
    /// Rounds of work done since the last long break.
    pub round: u32,
    /// When the timer was last started, while it runs.
    pub since: Option<u64>,
    /// Seconds of the phase that passed before `since`.
    pub banked: u64,
    /// The day `done` counts rounds of work on.
    pub day: i64,
    pub done: u32,
}

impl Default for Timer {
    fn default() -> Self {
        Self {
            phase: Phase::Work,
            round: 0,
            since: None,
            banked: 0,
            day: 0,
            done: 0,
        }
    }
}

impl Timer {
    pub fn read(record: &Record) -> Self {
        let phase = record.text("phase").and_then(Phase::from_key);
        Self {
            phase: phase.unwrap_or(Phase::Work),
            round: record.get("round").unwrap_or(0),
            since: record.get("since"),
            banked: record.get("banked").unwrap_or(0),
            day: record.get("day").unwrap_or(0),
            done: record.get("done").unwrap_or(0),
        }
    }

    pub fn write(&self) -> Record {
        let mut record = Record::default();
        record.set("phase", self.phase.key());
        record.set("round", self.round);
        if let Some(since) = self.since {
            record.set("since", since);
        }
        record.set("banked", self.banked);
        record.set("day", self.day);
        record.set("done", self.done);
        record
    }

    pub fn running(&self) -> bool {
        self.since.is_some()
    }

    fn elapsed(&self, now: u64) -> u64 {
        let running = self.since.map_or(0, |since| now.saturating_sub(since));
        self.banked + running
    }

    pub fn remaining(&self, plan: &Plan, now: u64) -> u64 {
        plan.length(self.phase).saturating_sub(self.elapsed(now))
    }

    /// How much of the phase has passed, from 0 to 1.
    pub fn share(&self, plan: &Plan, now: u64) -> f32 {
        self.elapsed(now) as f32 / plan.length(self.phase) as f32
    }

    /// The rounds of work done on `day`.
    pub fn done_on(&self, day: i64) -> u32 {
        if self.day == day { self.done } else { 0 }
    }

    /// Ends the phases that ran out by `now`, counting the rounds of work
    /// on the day `day_of` gives for when they ended. Whether anything
    /// changed.
    pub fn catch_up(&mut self, plan: &Plan, now: u64, day_of: impl Fn(u64) -> i64) -> bool {
        let mut changed = false;
        while let Some(since) = self.since {
            let left = plan.length(self.phase).saturating_sub(self.banked);
            let end = since + left;
            if now < end {
                break;
            }
            changed = true;
            if now - end > STALE_AFTER {
                *self = Self {
                    day: self.day,
                    done: self.done,
                    ..Self::default()
                };
                break;
            }
            self.advance(plan, Some(day_of(end)));
            self.since = plan.auto_start.then_some(end);
        }
        changed
    }

    /// Moves to the next phase. A round of work is counted on `counted`,
    /// when given.
    fn advance(&mut self, plan: &Plan, counted: Option<i64>) {
        self.phase = match self.phase {
            Phase::Work => {
                if let Some(day) = counted {
                    self.done = self.done_on(day) + 1;
                    self.day = day;
                }
                self.round += 1;
                if self.round >= plan.rounds.max(1) {
                    Phase::LongBreak
                } else {
                    Phase::Break
                }
            }
            Phase::Break => Phase::Work,
            Phase::LongBreak => {
                self.round = 0;
                Phase::Work
            }
        };
        self.banked = 0;
    }

    pub fn start(&mut self, now: u64) {
        self.since.get_or_insert(now);
    }

    pub fn pause(&mut self, now: u64) {
        self.banked = self.elapsed(now);
        self.since = None;
    }

    /// Ends the phase without counting it; the next one runs if this one
    /// did.
    pub fn skip(&mut self, plan: &Plan, now: u64) {
        let running = self.running();
        match self.phase {
            // Skipped work is no round: a short break follows.
            Phase::Work => (self.phase, self.banked) = (Phase::Break, 0),
            _ => self.advance(plan, None),
        }
        self.since = running.then_some(now);
    }

    /// Back to the start of the first round of work, keeping what was done
    /// today.
    pub fn reset(&mut self) {
        *self = Self {
            day: self.day,
            done: self.done,
            ..Self::default()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: Plan = Plan {
        work: 25 * MINUTE,
        short: 5 * MINUTE,
        long: 15 * MINUTE,
        rounds: 2,
        auto_start: true,
    };

    fn day(seconds: u64) -> i64 {
        (seconds / 86_400) as i64
    }

    #[test]
    fn a_running_phase_counts_down_and_pauses() {
        let mut timer = Timer::default();
        timer.start(1000);
        assert_eq!(timer.remaining(&PLAN, 1060), 24 * MINUTE);
        timer.pause(1060);
        assert_eq!(timer.remaining(&PLAN, 5000), 24 * MINUTE);
        timer.start(5000);
        assert_eq!(timer.remaining(&PLAN, 5060), 23 * MINUTE);
        assert!((timer.share(&PLAN, 5060) - 2. / 25.).abs() < 1e-6);
    }

    #[test]
    fn phases_follow_each_other_and_rounds_are_counted() {
        let mut timer = Timer::default();
        timer.start(0);
        // Work, break, work, long break, and into the next work.
        let now = 25 * MINUTE + 5 * MINUTE + 25 * MINUTE + 15 * MINUTE + 60;
        assert!(timer.catch_up(&PLAN, now, day));
        assert_eq!(timer.phase, Phase::Work);
        assert_eq!(timer.round, 0);
        assert_eq!(timer.done_on(0), 2);
        assert_eq!(timer.remaining(&PLAN, now), 24 * MINUTE);
        assert!(!timer.catch_up(&PLAN, now, day));
    }

    #[test]
    fn without_auto_start_the_next_phase_waits() {
        let plan = Plan {
            auto_start: false,
            ..PLAN
        };
        let mut timer = Timer::default();
        timer.start(0);
        timer.catch_up(&plan, 40 * MINUTE, day);
        assert_eq!((timer.phase, timer.running()), (Phase::Break, false));
        assert_eq!(timer.remaining(&plan, 50 * MINUTE), 5 * MINUTE);
    }

    #[test]
    fn a_phase_long_over_is_not_counted() {
        let mut timer = Timer::default();
        timer.start(0);
        timer.catch_up(&PLAN, 25 * MINUTE + 2 * HOUR, day);
        assert_eq!(timer, Timer::default());
    }

    #[test]
    fn rounds_are_counted_per_day() {
        let mut timer = Timer {
            day: 3,
            done: 5,
            ..Timer::default()
        };
        assert_eq!(timer.done_on(4), 0);
        let start = 4 * 86_400;
        timer.start(start);
        timer.catch_up(&PLAN, start + 26 * MINUTE, day);
        assert_eq!((timer.day, timer.done), (4, 1));
    }

    #[test]
    fn skipping_does_not_count() {
        let mut timer = Timer::default();
        timer.start(0);
        timer.skip(&PLAN, 60);
        assert_eq!((timer.phase, timer.round, timer.done), (Phase::Break, 0, 0));
        assert_eq!(timer.remaining(&PLAN, 60), 5 * MINUTE);
        timer.skip(&PLAN, 70);
        assert_eq!(timer.phase, Phase::Work);
        timer.reset();
        assert_eq!(timer, Timer::default());
    }

    #[test]
    fn the_timer_survives_a_restart() {
        let mut timer = Timer::default();
        timer.start(100);
        timer.catch_up(&PLAN, 26 * MINUTE + 100, day);
        let back = Timer::read(&Record::parse(&timer.write().to_text()));
        assert_eq!(back, timer);
    }
}
