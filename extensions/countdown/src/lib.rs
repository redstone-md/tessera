// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The time left until named dates of the configuration.
//!
//! `next` shows the nearest date ahead, for a bar; `events` shows every
//! date, for a popup. When each date was first seen goes to `save-state`,
//! for the bars that fill on the way to it.

mod model;

use model::{Countdown, parse_events};
use tessera_extension_kit::store::Record;
use tessera_extension_kit::time::{self, DAY};
use tessera_extension_kit::{self as kit, Extension, Ui, settings};

/// What to show without dates.
const NO_EVENTS: &str = "Name dates in the setting `events`, as \"New Year=2027-01-01\".";

struct Dates {
    countdown: Result<Countdown, String>,
}

impl Dates {
    fn next(&self, now: u64) -> Ui {
        let countdown = match &self.countdown {
            Ok(countdown) => countdown,
            Err(_) => return Ui::bar_caption("Countdown: see the settings"),
        };
        match countdown.next(now) {
            Some(event) => {
                let mut ui = Ui::bar();
                let left = time::span(event.at - now);
                ui.text(Ui::ROOT, format!("{} · {left}", event.name));
                ui
            }
            None if countdown.events.is_empty() => Ui::bar_caption("Countdown: no dates"),
            None => Ui::bar_caption("No dates ahead"),
        }
    }

    fn events(&self, now: u64) -> Ui {
        let mut ui = Ui::panel();
        let countdown = match &self.countdown {
            Ok(countdown) if !countdown.events.is_empty() => countdown,
            Ok(_) => {
                ui.caption(Ui::ROOT, NO_EVENTS);
                return ui;
            }
            Err(error) => {
                ui.caption(Ui::ROOT, error.clone());
                return ui;
            }
        };
        let (ahead, past): (Vec<_>, Vec<_>) =
            countdown.events.iter().partition(|event| event.at > now);
        for event in ahead {
            let line = ui.row(Ui::ROOT);
            ui.text(line, event.name.clone());
            ui.caption(line, time::span(event.at - now));
            ui.progress(Ui::ROOT, countdown.progress(event, now));
            let day = (event.at / DAY) as i64;
            ui.caption(
                Ui::ROOT,
                format!("{} {}", time::weekday(day), time::date(day)),
            );
        }
        for event in past.into_iter().rev() {
            let line = ui.row(Ui::ROOT);
            ui.text(line, event.name.clone());
            ui.caption(line, format!("passed {}", time::ago(now, event.at)));
        }
        ui
    }
}

impl Dates {
    /// The dates of the settings, with what was seen of them.
    fn countdown(now: u64) -> Result<Countdown, String> {
        let events = settings::text("events").map_or(Ok(Vec::new()), |text| parse_events(&text));
        let countdown = events.map(|events| {
            let (countdown, changed) = Countdown::new(events, Record::load(), now);
            if changed {
                countdown.seen().save();
            }
            countdown
        });
        if let Err(error) = &countdown {
            kit::log(&format!("the setting `events` cannot be read: {error}"));
        }
        countdown
    }
}

impl Extension for Dates {
    fn start(now: u64) -> Self {
        Self {
            countdown: Self::countdown(now),
        }
    }

    fn settings_changed(&mut self, now: u64) {
        self.countdown = Self::countdown(now);
    }

    fn render(&mut self, widget: &str, now: u64) -> Ui {
        match widget {
            "next" => self.next(now),
            _ => self.events(now),
        }
    }
}

kit::extension!(Dates);
