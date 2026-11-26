// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Habits, the days they were done on and their streaks.

use tessera_extension_kit::store::Record;

/// When a habit was done; days are counted from 1970-01-01.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Days {
    /// The last day it was done.
    last: Option<i64>,
    /// Days in a row it was done, up to `last`.
    streak: u32,
    best: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Habit {
    pub name: String,
    days: Days,
    /// What was before it was checked off today, to take it back.
    before: Option<Days>,
}

impl Habit {
    pub fn done_on(&self, today: i64) -> bool {
        self.days.last == Some(today)
    }

    /// The streak as it stands on `today`: broken when the habit was done
    /// neither today nor yesterday.
    pub fn streak(&self, today: i64) -> u32 {
        match self.days.last {
            Some(last) if last == today || last == today - 1 => self.days.streak,
            _ => 0,
        }
    }

    pub fn best(&self) -> u32 {
        self.days.best
    }

    /// Checks the habit off on `today`, or takes that back when it was.
    pub fn toggle(&mut self, today: i64) {
        if self.done_on(today) {
            self.days = self.before.take().unwrap_or_default();
            return;
        }
        self.before = Some(self.days);
        let streak = self.streak(today) + 1;
        self.days = Days {
            last: Some(today),
            streak,
            best: self.days.best.max(streak),
        };
    }

    fn read(name: String, record: &Record) -> Self {
        let days = |prefix: &str| {
            let key = |field: &str| format!("{prefix}{name}.{field}");
            Days {
                last: record.get(&key("last")),
                streak: record.get(&key("streak")).unwrap_or(0),
                best: record.get(&key("best")).unwrap_or(0),
            }
        };
        let before = record
            .text(&format!("before.{name}.streak"))
            .map(|_| days("before."));
        Self {
            days: days("habit."),
            before,
            name,
        }
    }

    fn write(&self, record: &mut Record) {
        let mut put = |prefix: &str, days: &Days| {
            let key = |field: &str| format!("{prefix}{}.{field}", self.name);
            if let Some(last) = days.last {
                record.set(&key("last"), last);
            }
            record.set(&key("streak"), days.streak);
            record.set(&key("best"), days.best);
        };
        put("habit.", &self.days);
        if let Some(before) = &self.before {
            put("before.", before);
        }
    }
}

/// The habits of the configuration.
pub struct Habits {
    pub list: Vec<Habit>,
}

impl Habits {
    /// The habits `names`, with their days read from `record`.
    pub fn read(names: Vec<String>, record: &Record) -> Self {
        let list = names.into_iter().map(|name| Habit::read(name, record));
        Self {
            list: list.collect(),
        }
    }

    /// The days of the habits, for `save-state`; habits no longer named
    /// are left out.
    pub fn write(&self) -> Record {
        let mut record = Record::default();
        for habit in &self.list {
            habit.write(&mut record);
        }
        record
    }

    pub fn done_on(&self, today: i64) -> usize {
        self.list
            .iter()
            .filter(|habit| habit.done_on(today))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn habits() -> Habits {
        Habits::read(
            vec!["Water".to_owned(), "Read".to_owned()],
            &Record::default(),
        )
    }

    #[test]
    fn streaks_grow_on_days_in_a_row_and_break_after_a_gap() {
        let mut habits = habits();
        let water = &mut habits.list[0];
        for day in 10..13 {
            water.toggle(day);
        }
        assert_eq!(
            (water.streak(12), water.streak(13), water.streak(14)),
            (3, 3, 0)
        );
        water.toggle(14);
        assert_eq!((water.streak(14), water.best()), (1, 3));
        assert_eq!(habits.done_on(14), 1);
    }

    #[test]
    fn checking_off_can_be_taken_back() {
        let mut habits = habits();
        let read = &mut habits.list[1];
        read.toggle(5);
        read.toggle(6);
        read.toggle(6);
        assert!(!read.done_on(6));
        assert_eq!((read.streak(6), read.best()), (1, 1));
        read.toggle(6);
        assert_eq!((read.streak(6), read.best()), (2, 2));
    }

    #[test]
    fn habits_survive_a_restart() {
        let mut habits = habits();
        habits.list[0].toggle(7);
        habits.list[0].toggle(8);
        let record = Record::parse(&habits.write().to_text());
        let back = Habits::read(vec!["Water".to_owned(), "Walk".to_owned()], &record);
        assert_eq!(back.list[0], habits.list[0]);
        assert_eq!(back.list[1].streak(8), 0);
        assert!(!back.write().to_text().contains("Read"));
    }
}
