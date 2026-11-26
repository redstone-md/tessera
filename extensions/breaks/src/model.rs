// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! When the next break is due.

/// A break due for longer than this is forgotten and the wait starts over.
const STALE_AFTER: u64 = 30 * 60;

/// Where the wait for the next break stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    /// Seconds left, and how much of the wait has passed, from 0 to 1.
    Waiting {
        left: u64,
        share: f32,
    },
    Due,
}

/// Breaks every `every` seconds; times are seconds since 1970.
#[derive(Clone, Debug, PartialEq)]
pub struct Reminder {
    pub every: u64,
    pub snooze: u64,
    /// When the wait for the next break began.
    pub start: u64,
}

impl Reminder {
    pub fn status(&self, now: u64) -> Status {
        let waited = now.saturating_sub(self.start);
        match self.every.checked_sub(waited) {
            Some(left) if left > 0 => Status::Waiting {
                left,
                share: waited as f32 / self.every as f32,
            },
            _ => Status::Due,
        }
    }

    /// Starts over when a break has been due for long. Whether it did.
    pub fn catch_up(&mut self, now: u64) -> bool {
        let stale = now > self.start + self.every + STALE_AFTER;
        if stale {
            self.start = now;
        }
        stale
    }

    /// The break was taken: the wait for the next one begins.
    pub fn done(&mut self, now: u64) {
        self.start = now;
    }

    /// Asks again `snooze` seconds from now.
    pub fn snooze(&mut self, now: u64) {
        self.start = (now + self.snooze).saturating_sub(self.every);
    }

    /// Makes the break due at once.
    pub fn now(&mut self, now: u64) {
        self.start = now.saturating_sub(self.every);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reminder() -> Reminder {
        Reminder {
            every: 1200,
            snooze: 300,
            start: 10_000,
        }
    }

    #[test]
    fn a_break_falls_due_after_the_wait() {
        let reminder = reminder();
        assert_eq!(
            reminder.status(10_300),
            Status::Waiting {
                left: 900,
                share: 0.25
            }
        );
        assert_eq!(reminder.status(11_200), Status::Due);
    }

    #[test]
    fn snoozing_asks_again_later_and_done_starts_over() {
        let mut reminder = reminder();
        reminder.snooze(11_200);
        assert!(matches!(
            reminder.status(11_200),
            Status::Waiting { left: 300, .. }
        ));
        assert_eq!(reminder.status(11_500), Status::Due);
        reminder.done(11_510);
        assert!(matches!(
            reminder.status(11_510),
            Status::Waiting { left: 1200, .. }
        ));
        reminder.now(11_600);
        assert_eq!(reminder.status(11_600), Status::Due);
    }

    #[test]
    fn a_break_due_for_long_is_forgotten() {
        let mut reminder = reminder();
        assert!(!reminder.catch_up(11_200 + 60));
        assert!(reminder.catch_up(11_200 + 3600));
        assert!(matches!(
            reminder.status(14_800),
            Status::Waiting { left: 1200, .. }
        ));
    }
}
