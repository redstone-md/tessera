// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Stars, forks and open issues of GitHub repositories.
//!
//! It asks the GitHub API about each repository of the setting `repos` on
//! a timer, a few at a time. `stars` shows the first, for a bar; `repos`
//! shows them all, for a popup.

mod model;

use model::{Stats, address, parse_repos};
use tessera_extension_kit::net::{self, Fetcher, Schedule};
use tessera_extension_kit::time::{self, MINUTE};
use tessera_extension_kit::{self as kit, Extension, Response, Ui, format, settings};

const SERVICE: &str = "GitHub";

/// Requests under way at a time; Tessera allows a few.
const AT_A_TIME: usize = 2;

struct Repository {
    name: String,
    /// The last counts and when they came.
    stats: Option<(Stats, u64)>,
    error: Option<String>,
}

struct GitHub {
    repos: Result<Vec<Repository>, String>,
    token: Option<String>,
    schedule: Schedule,
    /// Each request with the place of its repository.
    fetcher: Fetcher<usize>,
}

impl GitHub {
    fn ask_due(&mut self, now: u64) {
        let Ok(repos) = &mut self.repos else {
            return;
        };
        if !self.fetcher.busy() && self.schedule.due(now) {
            self.schedule.asked(now);
            let authorization = self.token.as_ref().map(|token| format!("Bearer {token}"));
            let mut headers = vec![
                ("User-Agent", "tessera-extension"),
                ("Accept", "application/vnd.github+json"),
                ("X-GitHub-Api-Version", "2022-11-28"),
            ];
            if let Some(authorization) = &authorization {
                headers.push(("Authorization", authorization));
            }
            for (at, repo) in repos.iter().enumerate() {
                self.fetcher
                    .queue(net::get(address(&repo.name), &headers), at);
            }
        }
        for (at, error) in self.fetcher.pump() {
            repos[at].error = Some(error);
            self.schedule.settle(false);
        }
    }

    fn answered(&mut self, request: u32, outcome: Result<Response, String>, now: u64) {
        let (Ok(repos), Some(at)) = (&mut self.repos, self.fetcher.finish(request)) else {
            return;
        };
        let repo = &mut repos[at];
        match net::body(outcome, SERVICE).and_then(|body| Stats::read(&body)) {
            Ok(stats) => (repo.stats, repo.error) = (Some((stats, now)), None),
            Err(error) => {
                kit::log(&format!("{}: {error}", repo.name));
                repo.error = Some(error);
            }
        }
        if !self.fetcher.busy() {
            self.schedule
                .settle(repos.iter().all(|repo| repo.error.is_none()));
        }
    }

    fn stars(&self) -> Ui {
        let Ok(repos) = &self.repos else {
            return Ui::bar_caption("GitHub: see the settings");
        };
        let first = &repos[0];
        let short = first
            .name
            .split_once('/')
            .map_or(&*first.name, |(_, name)| name);
        match (&first.stats, &first.error) {
            (Some((stats, _)), _) => {
                let mut ui = Ui::bar();
                ui.text(
                    Ui::ROOT,
                    format!("★ {} {short}", format::compact(stats.stars)),
                );
                ui
            }
            (None, Some(error)) => Ui::bar_caption(error.clone()),
            (None, None) => Ui::bar_caption("Asking GitHub…"),
        }
    }

    fn repos(&self, now: u64) -> Ui {
        let mut ui = Ui::panel();
        let repos = match &self.repos {
            Ok(repos) => repos,
            Err(error) => {
                ui.caption(Ui::ROOT, error.clone());
                return ui;
            }
        };
        for repo in repos {
            ui.text(Ui::ROOT, repo.name.clone());
            if let Some((stats, _)) = &repo.stats {
                ui.caption(
                    Ui::ROOT,
                    format!(
                        "★ {} · {} {} · {} open issues and PRs",
                        format::compact(stats.stars),
                        format::compact(stats.forks),
                        if stats.forks == 1 { "fork" } else { "forks" },
                        format::compact(stats.issues)
                    ),
                );
            }
            match &repo.error {
                Some(error) if self.token.is_none() && error.contains("(403)") => ui.caption(
                    Ui::ROOT,
                    format!("{error} Too many requests without a token?"),
                ),
                Some(error) => ui.caption(Ui::ROOT, error.clone()),
                None if repo.stats.is_none() => ui.caption(Ui::ROOT, "Asking…"),
                None => continue,
            };
        }
        let footer = ui.row(Ui::ROOT);
        let came = repos
            .iter()
            .filter_map(|repo| repo.stats.as_ref())
            .map(|(_, came)| *came)
            .min();
        let when = came.map_or(String::new(), |came| format!(", {}", time::ago(now, came)));
        ui.caption(footer, format!("GitHub{when}"));
        ui.button(footer, "Refresh", "refresh");
        ui
    }
}

impl GitHub {
    /// The names of the repositories of the settings.
    fn names() -> Result<Vec<String>, String> {
        let names = settings::text("repos").map_or_else(
            || Err("Name repositories in the setting `repos`, as \"rust-lang/rust\".".to_owned()),
            |text| parse_repos(&text),
        );
        if let Err(error) = &names {
            kit::log(error);
        }
        names
    }

    /// The repositories `names`, of which nothing is known yet.
    fn repositories(names: Result<Vec<String>, String>) -> Result<Vec<Repository>, String> {
        names.map(|names| {
            let repos = names.into_iter().map(|name| Repository {
                name,
                stats: None,
                error: None,
            });
            repos.collect()
        })
    }

    /// Whether the repositories shown are `names`.
    fn shows(&self, names: &Result<Vec<String>, String>) -> bool {
        match (&self.repos, names) {
            (Ok(repos), Ok(names)) => repos.iter().map(|repo| &repo.name).eq(names),
            (Err(shown), Err(error)) => shown == error,
            _ => false,
        }
    }

    /// Seconds between questions, as the settings say.
    fn interval() -> u64 {
        settings::number("interval_minutes", 30, 10..=24 * 60) * MINUTE
    }
}

impl Extension for GitHub {
    fn start(_now: u64) -> Self {
        Self {
            repos: Self::repositories(Self::names()),
            token: settings::text("token"),
            schedule: Schedule::new(Self::interval(), 5 * MINUTE),
            fetcher: Fetcher::new(AT_A_TIME),
        }
    }

    fn settings_changed(&mut self, _now: u64) {
        let names = Self::names();
        if !self.shows(&names) {
            // The requests under way name repositories by their place.
            self.fetcher.clear();
            self.repos = Self::repositories(names);
        }
        self.token = settings::text("token");
        self.schedule.retune(Self::interval());
    }

    fn render(&mut self, widget: &str, now: u64) -> Ui {
        self.ask_due(now);
        match widget {
            "stars" => self.stars(),
            _ => self.repos(now),
        }
    }

    fn activate(&mut self, _widget: &str, action: &str, _now: u64) {
        if action == "refresh" {
            self.schedule.hurry();
        }
    }

    fn fetched(&mut self, request: u32, outcome: Result<Response, String>, now: u64) {
        self.answered(request, outcome, now);
        // Send what waits without waiting for the next render.
        self.ask_due(now);
    }
}

kit::extension!(GitHub);
