// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Currency exchange rates from ExchangeRate-API.
//!
//! It asks the open access of ExchangeRate-API, which needs no account,
//! for the rates of the setting `base` on a timer. `rate` shows the first
//! currency of `symbols`, for a bar; `rates` shows them all, for a popup.

mod model;

use model::{Query, Rates, read};
use tessera_extension_kit::net::{self, Feed};
use tessera_extension_kit::time::{self, DAY, MINUTE};
use tessera_extension_kit::{self as kit, Extension, Response, Ui, format, settings};

const SERVICE: &str = "ExchangeRate-API";

struct Exchange {
    query: Result<Query, String>,
    feed: Feed<Rates>,
}

impl Exchange {
    fn rate(&self) -> Ui {
        let (Ok(query), Some((rates, _))) = (&self.query, self.feed.latest()) else {
            return Ui::bar_caption(match &self.query {
                Err(_) => "Exchange: see the settings".to_owned(),
                Ok(_) => self.feed.excuse(),
            });
        };
        let mut ui = Ui::bar();
        match &rates.rates[0] {
            (symbol, Some(rate)) => ui.text(
                Ui::ROOT,
                format!("1 {} = {} {symbol}", query.base, format::amount(*rate)),
            ),
            (symbol, None) => ui.caption(Ui::ROOT, format!("No rate for {symbol}")),
        };
        ui
    }

    fn rates(&self) -> Ui {
        let mut ui = Ui::panel();
        let query = match &self.query {
            Ok(query) => query,
            Err(error) => {
                ui.caption(Ui::ROOT, error.clone());
                return ui;
            }
        };
        let Some((rates, _)) = self.feed.latest() else {
            ui.caption(Ui::ROOT, self.feed.excuse());
            let buttons = ui.row(Ui::ROOT);
            ui.button(buttons, "Refresh", "refresh");
            return ui;
        };
        for (symbol, rate) in &rates.rates {
            let line = ui.row(Ui::ROOT);
            ui.text(line, symbol.clone());
            match rate {
                Some(rate) => {
                    ui.text(line, format::amount(*rate));
                    let back = format::amount(1. / rate);
                    ui.caption(line, format!("1 {symbol} = {back} {}", query.base));
                }
                None => {
                    ui.caption(line, "not known");
                }
            }
        }
        if let Some(error) = self.feed.error() {
            ui.caption(Ui::ROOT, error);
        }
        let footer = ui.row(Ui::ROOT);
        let updated = rates.updated.map_or(String::new(), |at| {
            let day = (at / DAY) as i64;
            format!(", updated {} {}", time::weekday(day), time::date(day))
        });
        ui.caption(footer, format!("Rates by ExchangeRate-API{updated}"));
        ui.button(footer, "Refresh", "refresh");
        ui
    }
}

impl Exchange {
    /// The base and the currencies of the settings.
    fn query() -> Result<Query, String> {
        let base = settings::text("base").unwrap_or_else(|| "USD".to_owned());
        let symbols = settings::text("symbols").unwrap_or_else(|| "EUR, GBP, JPY".to_owned());
        let query = Query::parse(&base, &symbols);
        if let Err(error) = &query {
            kit::log(error);
        }
        query
    }

    /// Seconds between questions, as the settings say.
    fn interval() -> u64 {
        settings::number("interval_minutes", 60, 30..=24 * 60) * MINUTE
    }
}

impl Extension for Exchange {
    fn start(_now: u64) -> Self {
        Self {
            query: Self::query(),
            feed: Feed::new(Self::interval(), 5 * MINUTE),
        }
    }

    fn settings_changed(&mut self, _now: u64) {
        let query = Self::query();
        self.feed.retune(Self::interval(), query != self.query);
        self.query = query;
    }

    fn render(&mut self, widget: &str, now: u64) -> Ui {
        if let Ok(query) = &self.query {
            self.feed.ask_due(now, || net::get(query.address(), &[]));
        }
        match widget {
            "rate" => self.rate(),
            _ => self.rates(),
        }
    }

    fn activate(&mut self, _widget: &str, action: &str, _now: u64) {
        if action == "refresh" {
            self.feed.hurry();
        }
    }

    fn fetched(&mut self, request: u32, outcome: Result<Response, String>, now: u64) {
        if let Ok(query) = &self.query {
            self.feed
                .answered(request, outcome, now, SERVICE, |body| read(body, query));
        }
    }
}

kit::extension!(Exchange);
