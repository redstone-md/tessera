// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A focus timer of work and breaks.
//!
//! `timer` shows the phase and the time left in it, for a bar; `controls`
//! starts, pauses, skips and resets it, for a popup. Where the timer is
//! goes to `save-state` whenever it changes, as the times it was started,
//! so it reads the same after a restart.

mod model;

use model::{Phase, Plan, Timer};
use tessera_extension_kit::store::Record;
use tessera_extension_kit::time::{self, Local, MINUTE};
use tessera_extension_kit::{self as kit, Extension, Ui, settings};

struct Pomodoro {
    plan: Plan,
    /// Seconds the user's day is off UTC.
    offset: i64,
    timer: Timer,
}

impl Pomodoro {
    fn today(&self, now: u64) -> i64 {
        Local::at(now, self.offset).day
    }

    fn catch_up(&mut self, now: u64) {
        let offset = self.offset;
        if self
            .timer
            .catch_up(&self.plan, now, |end| Local::at(end, offset).day)
        {
            self.timer.write().save();
        }
    }

    fn status(&self) -> &'static str {
        match (self.timer.running(), self.timer.banked) {
            (true, _) => "",
            (false, 0) => "Ready",
            (false, _) => "Paused",
        }
    }

    fn timer(&self, now: u64) -> Ui {
        let mut ui = Ui::bar();
        let left = self.timer.remaining(&self.plan, now);
        ui.text(
            Ui::ROOT,
            format!("{} {}", self.timer.phase.label(), time::mmss(left)),
        );
        match self.status() {
            "" => ui.progress(Ui::ROOT, self.timer.share(&self.plan, now)),
            status => ui.caption(Ui::ROOT, status),
        };
        ui
    }

    fn controls(&self, now: u64) -> Ui {
        let mut ui = Ui::panel();
        let heading = ui.row(Ui::ROOT);
        ui.text(heading, self.timer.phase.label());
        let (round, rounds) = (self.timer.round, self.plan.rounds);
        ui.caption(
            heading,
            match self.timer.phase {
                Phase::Work => format!("Round {} of {rounds}", (round + 1).min(rounds)),
                Phase::Break => format!("{round} of {rounds} rounds done"),
                Phase::LongBreak => format!("All {rounds} rounds done"),
            },
        );
        let clock = ui.row(Ui::ROOT);
        ui.text(clock, time::mmss(self.timer.remaining(&self.plan, now)));
        if !self.timer.running() {
            ui.caption(clock, self.status());
        }
        ui.progress(Ui::ROOT, self.timer.share(&self.plan, now));
        let buttons = ui.row(Ui::ROOT);
        match self.timer.running() {
            true => ui.button(buttons, "Pause", "pause"),
            false => ui.button(buttons, "Start", "start"),
        };
        ui.button(buttons, "Skip", "skip");
        ui.button(buttons, "Reset", "reset");
        let done = self.timer.done_on(self.today(now));
        ui.caption(Ui::ROOT, format!("Rounds of work done today: {done}"));
        ui
    }
}

impl Pomodoro {
    /// The lengths and rounds of the settings.
    fn plan() -> Plan {
        let minutes = |key, default, most| settings::number(key, default, 1..=most) * MINUTE;
        Plan {
            work: minutes("work_minutes", 25, 180),
            short: minutes("break_minutes", 5, 60),
            long: minutes("long_break_minutes", 15, 120),
            rounds: settings::number("rounds", 4, 1..=12),
            auto_start: settings::flag("auto_start", true),
        }
    }

    /// Seconds the user's day is off UTC, as the settings say.
    fn offset() -> i64 {
        settings::text("utc_offset").map_or(0, |text| {
            time::parse_offset(&text).unwrap_or_else(|| {
                kit::log(&format!(
                    "the setting `utc_offset` is not an offset such as +1 or -5:30: `{text}`"
                ));
                0
            })
        })
    }
}

impl Extension for Pomodoro {
    fn start(_now: u64) -> Self {
        Self {
            plan: Self::plan(),
            offset: Self::offset(),
            timer: Timer::read(&Record::load()),
        }
    }

    /// The phase, the round and the time of the timer stay; the time
    /// passed until now counts by the plan before.
    fn settings_changed(&mut self, now: u64) {
        self.catch_up(now);
        self.plan = Self::plan();
        self.offset = Self::offset();
    }

    fn render(&mut self, widget: &str, now: u64) -> Ui {
        self.catch_up(now);
        match widget {
            "timer" => self.timer(now),
            _ => self.controls(now),
        }
    }

    fn activate(&mut self, _widget: &str, action: &str, now: u64) {
        self.catch_up(now);
        match action {
            "start" => self.timer.start(now),
            "pause" => self.timer.pause(now),
            "skip" => self.timer.skip(&self.plan, now),
            "reset" => self.timer.reset(),
            _ => return,
        }
        self.timer.write().save();
    }
}

kit::extension!(Pomodoro);
