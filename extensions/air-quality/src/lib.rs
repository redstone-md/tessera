// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The air quality at a place, from Open-Meteo.
//!
//! It asks the air quality forecast of Open-Meteo, which needs no account,
//! about the `latitude` and `longitude` of the settings on a timer.
//! `index` shows the European Air Quality Index, for a bar; `details` adds
//! the particles and ozone, for a popup. Without a place nothing is asked.

mod model;

use model::{Air, Place, verdict};
use tessera_extension_kit::net::{self, Feed};
use tessera_extension_kit::time::{self, MINUTE};
use tessera_extension_kit::{self as kit, Extension, Response, Ui, settings};

const SERVICE: &str = "Open-Meteo";

struct AirQuality {
    place: Result<Place, String>,
    feed: Feed<Air>,
}

impl AirQuality {
    fn index(&self) -> Ui {
        let Some((air, _)) = self.feed.latest().filter(|_| self.place.is_ok()) else {
            return Ui::bar_caption(match &self.place {
                Err(_) => "Air: no place".to_owned(),
                Ok(_) => self.feed.excuse(),
            });
        };
        match air.index {
            Some(index) => {
                let mut ui = Ui::bar();
                ui.text(Ui::ROOT, format!("AQI {index:.0} {}", verdict(index)));
                ui
            }
            None => Ui::bar_caption("AQI unknown"),
        }
    }

    fn details(&self, now: u64) -> Ui {
        let mut ui = Ui::panel();
        if let Err(error) = &self.place {
            ui.caption(Ui::ROOT, error.clone());
            return ui;
        }
        let Some((air, came)) = self.feed.latest() else {
            ui.caption(Ui::ROOT, self.feed.excuse());
            let buttons = ui.row(Ui::ROOT);
            ui.button(buttons, "Refresh", "refresh");
            return ui;
        };
        match air.index {
            Some(index) => {
                let heading = ui.row(Ui::ROOT);
                ui.text(heading, format!("European AQI {index:.0}"));
                ui.caption(heading, verdict(index));
                ui.progress(Ui::ROOT, index as f32 / 100.);
            }
            None => {
                ui.caption(Ui::ROOT, "Open-Meteo gave no index for the place.");
            }
        }
        for (name, value) in [
            ("PM2.5", air.pm2_5),
            ("PM10", air.pm10),
            ("Ozone", air.ozone),
        ] {
            let line = ui.row(Ui::ROOT);
            ui.text(line, name);
            ui.caption(
                line,
                value.map_or("unknown".to_owned(), |value| format!("{value:.1} µg/m³")),
            );
        }
        if let Some(error) = self.feed.error() {
            ui.caption(Ui::ROOT, error);
        }
        let footer = ui.row(Ui::ROOT);
        let at = air.time.split_once('T').map_or("", |(_, clock)| clock);
        ui.caption(
            footer,
            format!("Open-Meteo.com, {at} UTC, {}", time::ago(now, came)),
        );
        ui.button(footer, "Refresh", "refresh");
        ui
    }
}

impl Extension for AirQuality {
    fn start(_now: u64) -> Self {
        let place = Place::of(
            settings::text("latitude").as_deref(),
            settings::text("longitude").as_deref(),
        );
        if let Err(error) = &place {
            kit::log(error);
        }
        let minutes = settings::number("interval_minutes", 30, 10..=24 * 60);
        Self {
            place,
            feed: Feed::new(minutes * MINUTE, MINUTE),
        }
    }

    fn render(&mut self, widget: &str, now: u64) -> Ui {
        if let Ok(place) = self.place {
            self.feed.ask_due(now, || net::get(place.address(), &[]));
        }
        match widget {
            "index" => self.index(),
            _ => self.details(now),
        }
    }

    fn activate(&mut self, _widget: &str, action: &str, _now: u64) {
        if action == "refresh" {
            self.feed.hurry();
        }
    }

    fn fetched(&mut self, request: u32, outcome: Result<Response, String>, now: u64) {
        self.feed
            .answered(request, outcome, now, SERVICE, Air::read);
    }
}

kit::extension!(AirQuality);
