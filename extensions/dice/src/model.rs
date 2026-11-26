// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Throws and what they showed.

use std::collections::VecDeque;

use tessera_extension_kit::rng::XorShift;

/// How many throws are remembered.
const KEPT: usize = 6;

/// Answers to a question asked in silence.
const ANSWERS: [&str; 12] = [
    "Without a doubt",
    "The stars say yes",
    "Go for it",
    "Most likely",
    "Ask after a coffee",
    "Too foggy to tell",
    "Flip a coin instead",
    "Sleep on it",
    "Not today",
    "Unlikely",
    "The stars say no",
    "Absolutely not",
];

/// What can be thrown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Throw {
    D6,
    D20,
    Coin,
    Answer,
}

impl Throw {
    /// The throw a button's action names.
    pub fn named(action: &str) -> Option<Self> {
        Some(match action {
            "d6" => Self::D6,
            "d20" => Self::D20,
            "coin" => Self::Coin,
            "answer" => Self::Answer,
            _ => return None,
        })
    }

    /// How many outcomes it has.
    fn sides(self) -> u32 {
        match self {
            Self::D6 => 6,
            Self::D20 => 20,
            Self::Coin => 2,
            Self::Answer => ANSWERS.len() as u32,
        }
    }
}

/// A throw and what it showed, from 1 to its number of sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub throw: Throw,
    pub value: u32,
}

impl Outcome {
    pub fn describe(self) -> String {
        match self.throw {
            Throw::D6 => format!("d6: {}", self.value),
            Throw::D20 => format!("d20: {}", self.value),
            Throw::Coin if self.value == 1 => "Heads".to_owned(),
            Throw::Coin => "Tails".to_owned(),
            Throw::Answer => ANSWERS[self.value as usize - 1].to_owned(),
        }
    }
}

/// The generator and the last throws, the latest first.
pub struct Table {
    rng: XorShift,
    history: VecDeque<Outcome>,
}

impl Table {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: XorShift::new(seed),
            history: VecDeque::new(),
        }
    }

    /// Throws `throw` at `now`, which is stirred into the generator.
    pub fn throw(&mut self, throw: Throw, now: u64) -> Outcome {
        self.rng.stir(now);
        let outcome = Outcome {
            throw,
            value: self.rng.roll(throw.sides()),
        };
        self.history.push_front(outcome);
        self.history.truncate(KEPT);
        outcome
    }

    pub fn last(&self) -> Option<Outcome> {
        self.history.front().copied()
    }

    /// The throws before the last one.
    pub fn earlier(&self) -> impl Iterator<Item = Outcome> + '_ {
        self.history.iter().skip(1).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throws_stay_within_their_sides() {
        let mut table = Table::new(7);
        for now in 0..500 {
            for throw in [Throw::D6, Throw::D20, Throw::Coin, Throw::Answer] {
                let outcome = table.throw(throw, now);
                assert!((1..=throw.sides()).contains(&outcome.value));
                assert!(!outcome.describe().is_empty());
            }
        }
    }

    #[test]
    fn only_the_last_throws_are_kept() {
        let mut table = Table::new(1);
        assert_eq!(table.last(), None);
        for now in 0..10 {
            table.throw(Throw::Coin, now);
        }
        let last = table.throw(Throw::D20, 10);
        assert_eq!(table.last(), Some(last));
        assert_eq!(table.earlier().count(), KEPT - 1);
    }

    #[test]
    fn actions_name_throws() {
        assert_eq!(Throw::named("d20"), Some(Throw::D20));
        assert_eq!(Throw::named("d7"), None);
    }
}
