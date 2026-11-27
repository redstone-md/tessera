// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The top stories of Hacker News.
//!
//! It asks the Hacker News API for the list of top stories on a timer and
//! then for each of the first few, a few requests at a time. `top` shows
//! the title of the first, for a bar; `stories` shows them all, for a
//! popup.

mod model;

use model::{Story, TOP_STORIES, item_address, read_top};
use tessera_extension_kit::net::{self, Fetcher, Schedule};
use tessera_extension_kit::time::{self, MINUTE};
use tessera_extension_kit::{self as kit, Extension, Response, Ui, format, settings};

const SERVICE: &str = "Hacker News";

/// Requests under way at a time; Tessera allows a few.
const AT_A_TIME: usize = 3;

/// The longest title the bar shows, in characters.
const BAR_TITLE: usize = 48;

/// What a request asks for.
#[derive(Clone, Copy, Debug)]
enum Ask {
    List,
    /// The story at this place of the list.
    Story(usize),
}

struct News {
    count: usize,
    schedule: Schedule,
    fetcher: Fetcher<Ask>,
    /// The stories shown and when they came.
    stories: Vec<Story>,
    came: Option<u64>,
    /// The stories of the refresh under way, in their places.
    incoming: Vec<Option<Story>>,
    error: Option<String>,
}

impl News {
    fn ask_due(&mut self, now: u64) {
        if !self.fetcher.busy() && self.schedule.due(now) {
            self.schedule.asked(now);
            self.fetcher.queue(net::get(TOP_STORIES, &[]), Ask::List);
        }
        for (ask, error) in self.fetcher.pump() {
            self.failed(ask, error, now);
        }
    }

    fn failed(&mut self, ask: Ask, error: String, now: u64) {
        kit::log(&error);
        match ask {
            Ask::List => {
                self.error = Some(error);
                self.schedule.settle(false);
            }
            // A story that cannot be had is left out.
            Ask::Story(_) => self.complete(now),
        }
    }

    fn answered(&mut self, ask: Ask, outcome: Result<Response, String>, now: u64) {
        let body = match net::body(outcome, SERVICE) {
            Ok(body) => body,
            Err(error) => return self.failed(ask, error, now),
        };
        match ask {
            Ask::List => match read_top(&body, self.count) {
                Ok(ids) => {
                    self.incoming = vec![None; ids.len()];
                    for (at, id) in ids.into_iter().enumerate() {
                        self.fetcher
                            .queue(net::get(item_address(id), &[]), Ask::Story(at));
                    }
                }
                Err(error) => self.failed(ask, error, now),
            },
            Ask::Story(at) => match Story::read(&body) {
                Ok(story) => {
                    self.incoming[at] = Some(story);
                    self.complete(now);
                }
                Err(error) => self.failed(ask, error, now),
            },
        }
    }

    /// Shows the stories of the refresh once none is awaited.
    fn complete(&mut self, now: u64) {
        if self.fetcher.busy() {
            return;
        }
        let stories: Vec<Story> = self.incoming.drain(..).flatten().collect();
        if stories.is_empty() {
            self.error = Some("None of the top stories could be read.".to_owned());
            self.schedule.settle(false);
        } else {
            (self.stories, self.came, self.error) = (stories, Some(now), None);
            self.schedule.settle(true);
        }
    }

    fn excuse(&self) -> String {
        self.error
            .clone()
            .unwrap_or_else(|| "Asking Hacker News…".to_owned())
    }

    fn top(&self) -> Ui {
        match self.stories.first() {
            Some(story) => {
                let mut ui = Ui::bar();
                ui.text(Ui::ROOT, format::shorten(&story.title, BAR_TITLE));
                ui
            }
            None => Ui::bar_caption(self.excuse()),
        }
    }

    fn stories(&self, now: u64) -> Ui {
        let mut ui = Ui::panel();
        if self.stories.is_empty() {
            ui.caption(Ui::ROOT, self.excuse());
        }
        for story in &self.stories {
            let card = ui.column(Ui::ROOT);
            ui.text(card, story.title.clone());
            ui.caption(card, story.about());
        }
        if let (Some(error), false) = (&self.error, self.stories.is_empty()) {
            ui.caption(Ui::ROOT, error.clone());
        }
        let footer = ui.row(Ui::ROOT);
        let when = self
            .came
            .map_or(String::new(), |came| format!(", {}", time::ago(now, came)));
        ui.caption(footer, format!("Hacker News{when}"));
        ui.button(footer, "Refresh", "refresh");
        ui
    }
}

impl News {
    /// How many stories the settings ask for.
    fn count() -> usize {
        settings::number("count", 6, 1..=15)
    }

    /// Seconds between questions, as the settings say.
    fn interval() -> u64 {
        settings::number("interval_minutes", 15, 5..=24 * 60) * MINUTE
    }
}

impl Extension for News {
    fn start(_now: u64) -> Self {
        Self {
            count: Self::count(),
            schedule: Schedule::new(Self::interval(), MINUTE),
            fetcher: Fetcher::new(AT_A_TIME),
            stories: Vec::new(),
            came: None,
            incoming: Vec::new(),
            error: None,
        }
    }

    fn settings_changed(&mut self, _now: u64) {
        // The stories shown stay until those of the new count come.
        self.count = Self::count();
        self.schedule.retune(Self::interval());
    }

    fn render(&mut self, widget: &str, now: u64) -> Ui {
        self.ask_due(now);
        match widget {
            "top" => self.top(),
            _ => self.stories(now),
        }
    }

    fn activate(&mut self, _widget: &str, action: &str, _now: u64) {
        if action == "refresh" {
            self.schedule.hurry();
        }
    }

    fn fetched(&mut self, request: u32, outcome: Result<Response, String>, now: u64) {
        if let Some(ask) = self.fetcher.finish(request) {
            self.answered(ask, outcome, now);
        }
        // Send what waits without waiting for the next render.
        self.ask_due(now);
    }
}

kit::extension!(News);
