// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The phase of the moon, worked out from the clock alone.
//!
//! `phase` shows the phase and the lit part of the moon, for a bar;
//! `details` adds its age and the time to the next full and new moon, for
//! a popup.

mod model;

use model::Moon;
use tessera_extension_kit::{self as kit, Extension, Ui, settings, time};

struct Sky {
    /// Whether the bar shows a moon symbol instead of the phase's name.
    glyph: bool,
}

impl Sky {
    fn phase(&self, moon: Moon) -> Ui {
        let mut ui = Ui::bar();
        let name = if self.glyph {
            moon.glyph()
        } else {
            moon.name()
        };
        ui.text(
            Ui::ROOT,
            format!("{name} {:.0}%", moon.illumination() * 100.),
        );
        ui
    }

    fn details(&self, moon: Moon) -> Ui {
        let mut ui = Ui::panel();
        let heading = ui.row(Ui::ROOT);
        ui.text(heading, format!("{} {}", moon.glyph(), moon.name()));
        ui.caption(heading, format!("{:.0}% lit", moon.illumination() * 100.));
        ui.progress(Ui::ROOT, moon.share() as f32);
        let waxing = if moon.waxing() { "waxing" } else { "waning" };
        ui.caption(Ui::ROOT, format!("{:.1} days old, {waxing}", moon.age()));
        for (label, seconds) in [
            ("Full moon", moon.until_full()),
            ("New moon", moon.until_new()),
        ] {
            let line = ui.row(Ui::ROOT);
            ui.text(line, label);
            ui.caption(line, format!("in {}", time::span(seconds)));
        }
        ui.caption(
            Ui::ROOT,
            "From the mean lunar month; within about half a day.",
        );
        ui
    }
}

impl Extension for Sky {
    fn start(_now: u64) -> Self {
        Self {
            glyph: settings::flag("glyph", false),
        }
    }

    fn settings_changed(&mut self, now: u64) {
        *self = Self::start(now);
    }

    fn render(&mut self, widget: &str, now: u64) -> Ui {
        let moon = Moon::at(now);
        match widget {
            "phase" => self.phase(moon),
            _ => self.details(moon),
        }
    }
}

kit::extension!(Sky);
