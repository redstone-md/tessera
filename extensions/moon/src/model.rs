// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The phase of the moon from the mean length of a lunar month.

use std::f64::consts::TAU;

/// The mean time from one new moon to the next, in days.
const SYNODIC_MONTH: f64 = 29.530_588_853;

/// A new moon: 2000-01-06 18:14 UTC, in seconds since 1970.
const KNOWN_NEW_MOON: f64 = 947_182_440.;

const DAY: f64 = 86_400.;

const NAMES: [&str; 8] = [
    "New moon",
    "Waxing crescent",
    "First quarter",
    "Waxing gibbous",
    "Full moon",
    "Waning gibbous",
    "Last quarter",
    "Waning crescent",
];

const GLYPHS: [&str; 8] = ["🌑", "🌒", "🌓", "🌔", "🌕", "🌖", "🌗", "🌘"];

/// The moon at a moment.
#[derive(Clone, Copy, Debug)]
pub struct Moon {
    /// Days since the last new moon.
    age: f64,
}

impl Moon {
    pub fn at(now: u64) -> Self {
        let days = (now as f64 - KNOWN_NEW_MOON) / DAY;
        Self {
            age: days.rem_euclid(SYNODIC_MONTH),
        }
    }

    /// Days since the last new moon.
    pub fn age(self) -> f64 {
        self.age
    }

    /// How far the moon is through its month, from 0 to 1.
    pub fn share(self) -> f64 {
        self.age / SYNODIC_MONTH
    }

    /// The lit part of the disc, from 0 to 1.
    pub fn illumination(self) -> f64 {
        (1. - (TAU * self.share()).cos()) / 2.
    }

    pub fn waxing(self) -> bool {
        self.share() < 0.5
    }

    /// Which of the eight phases it is in, each centred on its moment.
    fn octant(self) -> usize {
        (self.share() * 8. + 0.5) as usize % 8
    }

    pub fn name(self) -> &'static str {
        NAMES[self.octant()]
    }

    pub fn glyph(self) -> &'static str {
        GLYPHS[self.octant()]
    }

    /// Seconds to the next full moon.
    pub fn until_full(self) -> u64 {
        let half = SYNODIC_MONTH / 2.;
        let days = (half - self.age).rem_euclid(SYNODIC_MONTH);
        (days * DAY) as u64
    }

    /// Seconds to the next new moon.
    pub fn until_new(self) -> u64 {
        ((SYNODIC_MONTH - self.age) * DAY) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2024-01-11 11:57 UTC, a new moon.
    const NEW: u64 = 1_704_974_220;
    /// 2024-01-25 17:54 UTC, a full moon.
    const FULL: u64 = 1_706_205_240;

    #[test]
    fn known_new_and_full_moons_are_recognised() {
        let new = Moon::at(NEW);
        assert!(new.illumination() < 0.02, "{}", new.illumination());
        assert_eq!(new.name(), "New moon");
        let full = Moon::at(FULL);
        assert!(full.illumination() > 0.98, "{}", full.illumination());
        assert_eq!((full.name(), full.glyph()), ("Full moon", "🌕"));
    }

    #[test]
    fn the_phases_come_in_order() {
        let week = 7 * 86_400 + 9 * 3600;
        let names: Vec<&str> = (0..4).map(|at| Moon::at(NEW + at * week).name()).collect();
        assert_eq!(
            names,
            ["New moon", "First quarter", "Full moon", "Last quarter"]
        );
        assert!(Moon::at(NEW + 3 * 86_400).waxing());
        assert!(!Moon::at(FULL + 3 * 86_400).waxing());
    }

    #[test]
    fn the_next_full_and_new_moons_are_within_a_month() {
        let moon = Moon::at(NEW + 86_400);
        let hours = |seconds: u64| seconds as f64 / 3600.;
        // Off by at most half a day from the real full moon.
        assert!((hours(moon.until_full()) - hours(FULL - NEW - 86_400)).abs() < 12.);
        assert!(moon.until_new() > 27 * 86_400);
        let after_full = Moon::at(FULL + 86_400);
        assert!(after_full.until_full() > 27 * 86_400);
        assert!(after_full.until_new() < 15 * 86_400);
    }
}
