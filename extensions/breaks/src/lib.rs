// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Reminders to rest the eyes.
//!
//! `reminder` counts down to the next break and, when it is due, asks to
//! look away, for a bar; `controls` adds buttons to take the break now and
//! to snooze it, for a popup. When the wait began goes to `save-state`.

mod model;

use model::{Reminder, Status};
use tessera_extension_kit::store::Record;
use tessera_extension_kit::time::{self, MINUTE};
use tessera_extension_kit::{self as kit, Extension, Ui, settings};

struct Breaks {
    reminder: Reminder,
    /// Seconds to look away for.
    rest: u64,
}

impl Breaks {
    fn save(&self) {
        let mut record = Record::default();
        record.set("start", self.reminder.start);
        record.save();
    }

    fn look_away(&self) -> String {
        format!("Look away for {} s", self.rest)
    }

    fn reminder(&self, now: u64) -> Ui {
        let mut ui = Ui::bar();
        match self.reminder.status(now) {
            Status::Waiting { left, share } => {
                ui.text(Ui::ROOT, format!("Break in {}", time::mmss(left)));
                ui.progress(Ui::ROOT, share);
            }
            Status::Due => {
                ui.text(Ui::ROOT, self.look_away());
                ui.button(Ui::ROOT, "Done", "done");
            }
        }
        ui
    }

    fn controls(&self, now: u64) -> Ui {
        let mut ui = Ui::panel();
        match self.reminder.status(now) {
            Status::Waiting { left, share } => {
                ui.text(Ui::ROOT, format!("Next break in {}", time::mmss(left)));
                ui.progress(Ui::ROOT, share);
                let buttons = ui.row(Ui::ROOT);
                ui.button(buttons, "Break now", "now");
                ui.button(buttons, "Restart", "done");
            }
            Status::Due => {
                ui.text(Ui::ROOT, self.look_away());
                ui.caption(
                    Ui::ROOT,
                    "Rest your eyes on something about six metres away.",
                );
                let buttons = ui.row(Ui::ROOT);
                ui.button(buttons, "Done", "done");
                let snooze = self.reminder.snooze / MINUTE;
                ui.button(buttons, format!("Snooze {snooze} min"), "snooze");
            }
        }
        ui
    }
}

impl Extension for Breaks {
    fn start(now: u64) -> Self {
        let saved: Option<u64> = Record::load().get("start");
        let breaks = Self {
            reminder: Reminder {
                every: settings::number("every_minutes", 20, 5..=240) * MINUTE,
                snooze: settings::number("snooze_minutes", 5, 1..=60) * MINUTE,
                start: saved.unwrap_or(now).min(now),
            },
            rest: settings::number("break_seconds", 20, 5..=600),
        };
        if saved.is_none() {
            breaks.save();
        }
        breaks
    }

    fn render(&mut self, widget: &str, now: u64) -> Ui {
        if self.reminder.catch_up(now) {
            self.save();
        }
        match widget {
            "reminder" => self.reminder(now),
            _ => self.controls(now),
        }
    }

    fn activate(&mut self, _widget: &str, action: &str, now: u64) {
        match action {
            "done" => self.reminder.done(now),
            "snooze" => self.reminder.snooze(now),
            "now" => self.reminder.now(now),
            _ => return,
        }
        self.save();
    }
}

kit::extension!(Breaks);
