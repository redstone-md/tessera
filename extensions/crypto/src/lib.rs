// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Prices of coins from CoinGecko.
//!
//! It asks CoinGecko, which needs no account, for the prices of the coins
//! of the setting `coins` on a timer. `price` shows the first, for a bar;
//! `list` shows them all, for a popup.

mod model;

use model::{Query, Quote, read};
use tessera_extension_kit::net::{self, Feed};
use tessera_extension_kit::time::{self, MINUTE};
use tessera_extension_kit::{self as kit, Extension, Response, Ui, format, settings};

const SERVICE: &str = "CoinGecko";

struct Prices {
    query: Result<Query, String>,
    feed: Feed<Vec<Quote>>,
}

impl Prices {
    fn change(quote: &Quote) -> String {
        quote.change.map(format::change).unwrap_or_default()
    }

    fn price(&self) -> Ui {
        let (Ok(query), Some((quotes, _))) = (&self.query, self.feed.latest()) else {
            return Ui::bar_caption(match &self.query {
                Err(_) => "Crypto: see the settings".to_owned(),
                Ok(_) => self.feed.excuse(),
            });
        };
        let quote = &quotes[0];
        let mut ui = Ui::bar();
        ui.text(
            Ui::ROOT,
            format!("{} {}", quote.ticker(), query.money(quote.price)),
        );
        ui.caption(Ui::ROOT, Self::change(quote));
        ui
    }

    fn list(&self, now: u64) -> Ui {
        let mut ui = Ui::panel();
        let query = match &self.query {
            Ok(query) => query,
            Err(error) => {
                ui.caption(Ui::ROOT, error.clone());
                return ui;
            }
        };
        let Some((quotes, came)) = self.feed.latest() else {
            ui.caption(Ui::ROOT, self.feed.excuse());
            let buttons = ui.row(Ui::ROOT);
            ui.button(buttons, "Refresh", "refresh");
            return ui;
        };
        for quote in quotes {
            let line = ui.row(Ui::ROOT);
            ui.text(line, quote.name());
            ui.text(line, query.money(quote.price));
            ui.caption(line, Self::change(quote));
        }
        let missing: Vec<&str> = query
            .coins
            .iter()
            .filter(|coin| !quotes.iter().any(|quote| &quote.coin == *coin))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            ui.caption(
                Ui::ROOT,
                format!("CoinGecko does not know {}.", missing.join(", ")),
            );
        }
        if let Some(error) = self.feed.error() {
            ui.caption(Ui::ROOT, error);
        }
        let footer = ui.row(Ui::ROOT);
        ui.caption(
            footer,
            format!("CoinGecko, {} · change over 24 h", time::ago(now, came)),
        );
        ui.button(footer, "Refresh", "refresh");
        ui
    }
}

impl Extension for Prices {
    fn start(_now: u64) -> Self {
        let coins = settings::text("coins").unwrap_or_else(|| "bitcoin, ethereum".to_owned());
        let currency = settings::text("currency").unwrap_or_else(|| "usd".to_owned());
        let query = Query::parse(&coins, &currency);
        if let Err(error) = &query {
            kit::log(error);
        }
        let minutes = settings::number("interval_minutes", 10, 2..=24 * 60);
        Self {
            query,
            feed: Feed::new(minutes * MINUTE, MINUTE),
        }
    }

    fn render(&mut self, widget: &str, now: u64) -> Ui {
        if let Ok(query) = &self.query {
            self.feed.ask_due(now, || net::get(query.address(), &[]));
        }
        match widget {
            "price" => self.price(),
            _ => self.list(now),
        }
    }

    fn activate(&mut self, _widget: &str, action: &str, _now: u64) {
        if action == "refresh" {
            self.feed.hurry();
        }
    }

    fn fetched(&mut self, request: u32, outcome: Result<Response, String>, now: u64) {
        let Ok(query) = &self.query else {
            return;
        };
        self.feed
            .answered(request, outcome, now, SERVICE, |body| read(body, query));
    }
}

kit::extension!(Prices);
