// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Daily habits to check off, with streaks.
//!
//! `today` shows how many habits are done today, for a bar; `checklist`
//! has a button for each, for a popup. What was checked off goes to
//! `save-state`.

mod model;

use model::Habits;
use tessera_extension_kit::store::Record;
use tessera_extension_kit::time::{self, Local};
use tessera_extension_kit::{self as kit, Extension, Ui, format, settings};

const NO_HABITS: &str = "Name habits in the setting `habits`, as \"Water; Stretch; Read\".";

struct Tracker {
    habits: Habits,
    /// Seconds the user's day is off UTC.
    offset: i64,
}

impl Tracker {
    fn today(&self, now: u64) -> i64 {
        Local::at(now, self.offset).day
    }

    fn bar(&self, now: u64) -> Ui {
        let total = self.habits.list.len();
        if total == 0 {
            return Ui::bar_caption("Habits: none named");
        }
        let done = self.habits.done_on(self.today(now));
        let mut ui = Ui::bar();
        ui.text(Ui::ROOT, format!("Habits {done}/{total}"));
        ui.progress(Ui::ROOT, done as f32 / total as f32);
        ui
    }

    fn checklist(&self, now: u64) -> Ui {
        let mut ui = Ui::panel();
        if self.habits.list.is_empty() {
            ui.caption(Ui::ROOT, NO_HABITS);
            return ui;
        }
        let today = self.today(now);
        for (at, habit) in self.habits.list.iter().enumerate() {
            let line = ui.row(Ui::ROOT);
            ui.text(line, habit.name.clone());
            let streak = habit.streak(today);
            ui.caption(
                line,
                match streak {
                    0 => "no streak".to_owned(),
                    1 => "1 day".to_owned(),
                    days => format!("{days} days in a row"),
                },
            );
            let label = if habit.done_on(today) { "Undo" } else { "Done" };
            ui.button(line, label, format!("toggle:{at}"));
        }
        let best = self.habits.list.iter().max_by_key(|habit| habit.best());
        if let Some(best) = best.filter(|habit| habit.best() > 1) {
            let days = format::plural(best.best().into(), "day", "days");
            ui.caption(Ui::ROOT, format!("Longest streak: {}, {days}", best.name));
        }
        ui
    }
}

impl Extension for Tracker {
    fn start(_now: u64) -> Self {
        let names =
            settings::text("habits").map_or_else(Vec::new, |text| settings::parse_list(&text));
        let offset = settings::text("utc_offset").map_or(0, |text| {
            time::parse_offset(&text).unwrap_or_else(|| {
                kit::log(&format!(
                    "the setting `utc_offset` is not an offset such as +1: `{text}`"
                ));
                0
            })
        });
        Self {
            habits: Habits::read(names, &Record::load()),
            offset,
        }
    }

    fn render(&mut self, widget: &str, now: u64) -> Ui {
        match widget {
            "today" => self.bar(now),
            _ => self.checklist(now),
        }
    }

    fn activate(&mut self, _widget: &str, action: &str, now: u64) {
        let today = self.today(now);
        let habit = action
            .strip_prefix("toggle:")
            .and_then(|at| at.parse::<usize>().ok())
            .and_then(|at| self.habits.list.get_mut(at));
        if let Some(habit) = habit {
            habit.toggle(today);
            self.habits.write().save();
        }
    }
}

kit::extension!(Tracker);
