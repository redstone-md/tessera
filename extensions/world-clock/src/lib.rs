// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The time in other places, at fixed offsets from UTC.
//!
//! `clock` shows the time at the first place of the setting `zones`, for
//! a bar; `zones` shows every place with its day, for a popup.

mod model;

use model::{Zone, parse_zones, relative_day};
use tessera_extension_kit::time::{self, Local};
use tessera_extension_kit::{self as kit, Extension, Ui, settings};

const NO_ZONES: &str = "Name places in the setting `zones`, as \"Tokyo=+9; London=+0\".";

struct Clocks {
    zones: Result<Vec<Zone>, String>,
    /// Seconds the user's own clock is off UTC.
    home: i64,
    hour12: bool,
}

impl Clocks {
    fn clock(&self, local: Local) -> String {
        if self.hour12 {
            local.clock_12()
        } else {
            local.clock()
        }
    }

    fn bar(&self, now: u64) -> Ui {
        match &self.zones {
            Ok(zones) => match zones.first() {
                Some(zone) => {
                    let mut ui = Ui::bar();
                    ui.text(
                        Ui::ROOT,
                        format!("{} {}", zone.name, self.clock(zone.local(now))),
                    );
                    ui
                }
                None => Ui::bar_caption("World clock: no places"),
            },
            Err(_) => Ui::bar_caption("World clock: see the settings"),
        }
    }

    fn zones(&self, now: u64) -> Ui {
        let mut ui = Ui::panel();
        let zones = match &self.zones {
            Ok(zones) if !zones.is_empty() => zones,
            Ok(_) => {
                ui.caption(Ui::ROOT, NO_ZONES);
                return ui;
            }
            Err(error) => {
                ui.caption(Ui::ROOT, error.clone());
                return ui;
            }
        };
        let home = Local::at(now, self.home).day;
        for zone in zones {
            let local = zone.local(now);
            let line = ui.row(Ui::ROOT);
            ui.text(line, zone.name.clone());
            ui.text(line, self.clock(local));
            ui.caption(
                Ui::ROOT,
                format!(
                    "{}, {} · {}",
                    local.weekday(),
                    relative_day(local.day, home),
                    time::format_offset(zone.offset)
                ),
            );
        }
        ui.caption(
            Ui::ROOT,
            "Fixed offsets; daylight saving time is not applied.",
        );
        ui
    }
}

impl Extension for Clocks {
    fn start(_now: u64) -> Self {
        let zones = settings::text("zones").map_or(Ok(Vec::new()), |text| parse_zones(&text));
        if let Err(error) = &zones {
            kit::log(&format!("the setting `zones` cannot be read: {error}"));
        }
        let home = settings::text("utc_offset").map_or(0, |text| {
            time::parse_offset(&text).unwrap_or_else(|| {
                kit::log(&format!(
                    "the setting `utc_offset` is not an offset such as +1: `{text}`"
                ));
                0
            })
        });
        Self {
            zones,
            home,
            hour12: settings::flag("hour12", false),
        }
    }

    fn render(&mut self, widget: &str, now: u64) -> Ui {
        match widget {
            "clock" => self.bar(now),
            _ => self.zones(now),
        }
    }
}

kit::extension!(Clocks);
