// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Example extension: the remaining usage limits of AI services.
//!
//! It asks each enabled service for the usage of the user's account, on a
//! timer, and shows the answers in two widgets: `summary` for a bar and
//! `details` for a popup. The settings in `[extension.ai-limits]` of the
//! shell's configuration, which Settings > Extensions also changes, choose
//! the services; the buttons of `details` choose what is shown and are
//! remembered.

mod clock;
mod model;
mod view;

use std::cell::RefCell;

use model::{Provider, refusal};
use view::{Prefs, Slot};

wit_bindgen::generate!({
    world: "extension",
    path: "../../wit",
});

/// The services asked when the settings name none.
const DEFAULT_PROVIDERS: &str = "claude,codex";

/// Minutes between questions to a service when the settings give none.
const DEFAULT_INTERVAL_MINUTES: u64 = 5;

struct App {
    slots: Vec<Slot>,
    /// The services of the `summary` widget, in order.
    in_summary: Vec<Provider>,
    prefs: Prefs,
    /// Seconds between questions to a service.
    interval: u64,
}

impl App {
    fn start() -> Self {
        let listed = |key: &str, default: &str| {
            let text = setting(key).unwrap_or_else(|| default.to_owned());
            let (known, unknown) = view::providers(&text);
            for name in unknown {
                log(&format!(
                    "the setting `{key}` names an unknown service `{name}`"
                ));
            }
            known
        };
        let enabled = listed("providers", DEFAULT_PROVIDERS);
        let in_summary = match setting("summary") {
            Some(_) => listed("summary", ""),
            None => enabled.clone(),
        };
        let minutes = setting("interval_minutes")
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(DEFAULT_INTERVAL_MINUTES);
        Self {
            slots: enabled.into_iter().map(Slot::new).collect(),
            in_summary,
            prefs: Prefs::restore(&load_state()),
            interval: minutes.clamp(1, 24 * 60) * 60,
        }
    }

    /// Asks every service that is due.
    fn ask_due(&mut self, now: u64) {
        let interval = self.interval;
        for slot in &mut self.slots {
            let due = slot.asked_at.is_none_or(|asked| now >= asked + interval);
            if slot.pending.is_none() && due {
                slot.asked_at = Some(now);
                if let Err(error) = ask(slot, now) {
                    slot.report = None;
                    slot.error = Some(error);
                }
            }
        }
    }

    fn answered(&mut self, request: u32, outcome: Result<Response, String>) {
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.pending == Some(request))
        else {
            return;
        };
        slot.pending = None;
        let report = match outcome {
            Ok(response) if (200..300).contains(&response.status) => {
                slot.provider.report(&response.body)
            }
            Ok(response) => Err(refusal(slot.provider, response.status)),
            Err(error) => Err(format!("No answer: {error}")),
        };
        match report {
            Ok(report) => (slot.report, slot.error) = (Some(report), None),
            Err(error) => (slot.report, slot.error) = (None, Some(error)),
        }
    }

    fn pressed(&mut self, action: &str) {
        if action == "refresh" {
            for slot in &mut self.slots {
                slot.asked_at = None;
            }
        } else if self.prefs.toggle(action)
            && let Err(error) = save_state(&self.prefs.store())
        {
            log(&format!("the choice is not remembered: {error}"));
        }
    }
}

/// Starts the request for the usage of the slot's service.
fn ask(slot: &mut Slot, now: u64) -> Result<(), String> {
    let provider = slot.provider;
    let secret = match (provider.credentials_file(), provider.key_setting()) {
        (Some(file), _) => read_file(file).map_err(|error| format!("Cannot read {file}: {error}")),
        (None, Some(key)) => setting(key)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("Set `{key}` in [extension.ai-limits] of config.toml.")),
        (None, None) => Err("There is no way to sign in.".to_owned()),
    }?;
    let call = provider.call(&secret, now)?;
    let headers = call.headers.into_iter();
    let request = Request {
        method: "GET".to_owned(),
        url: call.url.to_owned(),
        headers: headers
            .map(|(name, value)| Header {
                name: name.to_owned(),
                value,
            })
            .collect(),
        body: None,
    };
    slot.pending = Some(fetch(&request)?);
    slot.plan = call.plan;
    Ok(())
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<T>(run: impl FnOnce(&mut App) -> T) -> T {
    APP.with_borrow_mut(|app| run(app.get_or_insert_with(App::start)))
}

struct AiLimits;

impl Guest for AiLimits {
    fn render(widget: String) -> Vec<Node> {
        let now = now();
        with_app(|app| {
            app.ask_due(now);
            match widget.as_str() {
                "summary" => view::summary(&app.slots, &app.in_summary, app.prefs),
                _ => view::details(&app.slots, app.prefs, now),
            }
        })
    }

    fn activate(_widget: String, action: String) {
        with_app(|app| app.pressed(&action));
    }

    fn fetched(request: u32, outcome: Result<Response, String>) {
        with_app(|app| app.answered(request, outcome));
    }

    fn settings_changed() {
        // The settings are read once, when the extension starts; a changed
        // one is used after Tessera starts the extension again.
    }

    fn answer(_query: String) -> Vec<Answer> {
        // This extension answers no launcher queries.
        Vec::new()
    }

    fn run_answer(_id: String) {
        // This extension gives no answers to run.
    }

    fn shortcut(_id: String) {
        // This extension declares no shortcuts.
    }
}

export!(AiLimits);
