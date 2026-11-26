// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Dice, a coin and answers to questions asked in silence, offline.
//!
//! `roll` shows the last throw with a button for a six-sided die, for a
//! bar; `table` has a button for each throw and the last throws, for a
//! popup.

mod model;

use model::{Table, Throw};
use tessera_extension_kit::{self as kit, Extension, Ui};

struct Dice {
    table: Table,
}

impl Dice {
    fn roll(&self) -> Ui {
        let mut ui = Ui::bar();
        match self.table.last() {
            Some(outcome) => ui.text(Ui::ROOT, outcome.describe()),
            None => ui.caption(Ui::ROOT, "Dice"),
        };
        ui.button(Ui::ROOT, "Roll", "d6");
        ui
    }

    fn table(&self) -> Ui {
        let mut ui = Ui::panel();
        match self.table.last() {
            Some(outcome) => ui.text(Ui::ROOT, outcome.describe()),
            None => ui.caption(Ui::ROOT, "Roll a die, flip a coin or ask a question."),
        };
        let buttons = ui.row(Ui::ROOT);
        for (label, action) in [
            ("d6", "d6"),
            ("d20", "d20"),
            ("Coin", "coin"),
            ("Ask", "answer"),
        ] {
            ui.button(buttons, label, action);
        }
        let earlier: Vec<String> = self
            .table
            .earlier()
            .map(|outcome| outcome.describe())
            .collect();
        if !earlier.is_empty() {
            ui.caption(Ui::ROOT, format!("Before: {}", earlier.join(" · ")));
        }
        ui
    }
}

impl Extension for Dice {
    fn start(now: u64) -> Self {
        Self {
            table: Table::new(now),
        }
    }

    fn render(&mut self, widget: &str, _now: u64) -> Ui {
        match widget {
            "roll" => self.roll(),
            _ => self.table(),
        }
    }

    fn activate(&mut self, _widget: &str, action: &str, now: u64) {
        if let Some(throw) = Throw::named(action) {
            self.table.throw(throw, now);
        }
    }
}

kit::extension!(Dice);
