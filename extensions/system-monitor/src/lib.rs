// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The meters of the machine, its windows and what it plays, an example of
//! the extension interface 0.4.
//!
//! `meters` is the processor and the memory in use, for a bar; `panel`
//! adds the network and the battery, the media that plays with buttons
//! that control it, and the open windows, which a field filters, each with
//! its icon and a button that brings it in front, for a popup. Both are
//! drawn again when what they watch changes. Typed into the launcher,
//! `sys` lists the meters, and `sys` with a word the windows whose title
//! has it; the shortcut `panel` opens the popup. The setting `windows`
//! says how many windows the panel lists (8, from 1 to 50).

mod model;

use model::{app_name, bytes, matching, percent, rate};
use tessera_extension_kit::{
    self as kit, Answer, AnswerAction, Event, Extension, Id, MediaCommand, Meters, Ui, settings,
    shell,
};

const ICON_CPU: &str = "\u{e9d9}";
const ICON_MEMORY: &str = "\u{e950}";
const ICON_NETWORK: &str = "\u{e774}";
const ICON_BATTERY: &str = "\u{e83f}";
const ICON_NOTE: &str = "\u{ec4f}";
const ICON_PREVIOUS: &str = "\u{e892}";
const ICON_PLAY: &str = "\u{e768}";
const ICON_PAUSE: &str = "\u{e769}";
const ICON_NEXT: &str = "\u{e893}";
const ICON_WINDOW: &str = "\u{e737}";

/// How high the list of windows is at most, in logical pixels.
const LIST_HEIGHT: f32 = 240.;

/// How high an icon of a window is.
const WINDOW_ICON: f32 = 20.;

/// The answers the launcher gets at most.
const ANSWERS: usize = 8;

struct Monitor {
    /// What the field of the panel filters the windows by.
    filter: String,
}

impl Monitor {
    fn meters(&self) -> Ui {
        let mut ui = Ui::bar();
        match shell::meters() {
            Ok(meters) => {
                ui.icon(Ui::ROOT, ICON_CPU);
                ui.text(Ui::ROOT, meters.cpu.map_or("…".to_owned(), percent));
                ui.icon(Ui::ROOT, ICON_MEMORY);
                ui.text(Ui::ROOT, percent(shell::memory_share(&meters)));
            }
            Err(error) => {
                ui.caption(Ui::ROOT, error);
            }
        }
        ui
    }

    fn panel(&self) -> Ui {
        let mut ui = Ui::panel();
        match shell::meters() {
            Ok(meters) => Self::gauges(&mut ui, &meters),
            Err(error) => {
                ui.caption(Ui::ROOT, error);
            }
        }
        Self::media(&mut ui);
        self.windows(&mut ui);
        ui
    }

    /// A card with the meters.
    fn gauges(ui: &mut Ui, meters: &Meters) {
        let card = ui.column(Ui::ROOT);
        let gauge = |ui: &mut Ui, icon: &str, label: String, share: Option<f32>| {
            let line = ui.row(card);
            ui.icon(line, icon);
            ui.text(line, label);
            ui.spacer(line);
            if let Some(share) = share {
                ui.progress(card, share);
            }
        };
        let cpu = meters.cpu.map_or("…".to_owned(), percent);
        gauge(ui, ICON_CPU, format!("Processor {cpu}"), meters.cpu);
        let memory = shell::memory_share(meters);
        let used = format!(
            "Memory {} of {}",
            bytes(meters.memory_used as f64),
            bytes(meters.memory_total as f64)
        );
        gauge(ui, ICON_MEMORY, used, Some(memory));
        if let (Some(received), Some(sent)) = (meters.received, meters.sent) {
            let traffic = format!("Down {} · Up {}", rate(received), rate(sent));
            gauge(ui, ICON_NETWORK, traffic, None);
        }
        if let Some(battery) = &meters.battery {
            let state = if battery.plugged_in {
                ", plugged in"
            } else {
                ""
            };
            let label = format!("Battery {}%{state}", battery.percent);
            gauge(
                ui,
                ICON_BATTERY,
                label,
                Some(f32::from(battery.percent) / 100.),
            );
        }
    }

    /// A card with what plays and buttons that control it.
    fn media(ui: &mut Ui) {
        let playing = match shell::media() {
            Ok(Some(media)) => media,
            Ok(None) => return,
            Err(error) => {
                ui.caption(Ui::ROOT, error);
                return;
            }
        };
        let card = ui.column(Ui::ROOT);
        let line = ui.row(card);
        ui.icon(line, ICON_NOTE);
        ui.text(line, playing.title.clone());
        ui.spacer(line);
        if !playing.artist.is_empty() {
            ui.caption(card, playing.artist.clone());
        }
        if let Some(position) = &playing.position
            && position.length > 0
        {
            ui.progress(card, position.elapsed as f32 / position.length as f32);
        }
        let buttons = ui.row(card);
        ui.button(buttons, ICON_PREVIOUS, "previous");
        let toggle = if playing.playing {
            ICON_PAUSE
        } else {
            ICON_PLAY
        };
        ui.button(buttons, toggle, "toggle");
        ui.button(buttons, ICON_NEXT, "next");
    }

    /// A field and the windows it lets through, each with a button.
    fn windows(&self, ui: &mut Ui) {
        let card = ui.column(Ui::ROOT);
        let heading = ui.row(card);
        ui.icon(heading, ICON_WINDOW);
        ui.text(heading, "Windows");
        ui.text_input(card, "filter", "Filter the windows", self.filter.clone());
        let windows = match shell::windows() {
            Ok(windows) => windows,
            Err(error) => {
                ui.caption(card, error);
                return;
            }
        };
        let count = settings::number("windows", 8, 1..=50);
        let shown = matching(&windows, &self.filter, count);
        if shown.is_empty() {
            ui.caption(card, "No window matches.");
            return;
        }
        let list = ui.scroll(card, LIST_HEIGHT);
        for window in shown {
            let line = ui.row(list);
            Self::window_icon(ui, line, &window.app);
            let title = if window.title.is_empty() {
                app_name(&window.app).to_owned()
            } else {
                window.title.clone()
            };
            ui.text(line, title);
            ui.spacer(line);
            if !window.focused {
                ui.button(line, "Show", format!("focus:{}", window.id));
            }
        }
    }

    fn window_icon(ui: &mut Ui, line: Id, app: &str) {
        if app.is_empty() {
            ui.icon(line, ICON_WINDOW);
        } else {
            ui.app_icon(line, app, WINDOW_ICON);
        }
    }

    /// The meters as answers that copy them.
    fn meter_answers() -> Vec<Answer> {
        let Ok(meters) = shell::meters() else {
            return Vec::new();
        };
        let answer = |id: &str, title: String, icon: &str| Answer {
            id: id.to_owned(),
            title: title.clone(),
            detail: "System monitor".to_owned(),
            icon: icon.to_owned(),
            score: 600,
            action: AnswerAction::Copy(title),
        };
        let mut answers = Vec::new();
        if let Some(cpu) = meters.cpu {
            answers.push(answer(
                "cpu",
                format!("Processor {}", percent(cpu)),
                ICON_CPU,
            ));
        }
        let memory = format!(
            "Memory {} of {}",
            bytes(meters.memory_used as f64),
            bytes(meters.memory_total as f64)
        );
        answers.push(answer("memory", memory, ICON_MEMORY));
        answers
    }
}

impl Extension for Monitor {
    fn start(_now: u64) -> Self {
        Self {
            filter: String::new(),
        }
    }

    fn render(&mut self, widget: &str, _now: u64) -> Ui {
        match widget {
            "meters" => self.meters(),
            _ => self.panel(),
        }
    }

    fn input(&mut self, _widget: &str, event: Event<'_>, _now: u64) {
        let done = match event {
            Event::InputChanged { id: "filter", text }
            | Event::Submitted { id: "filter", text } => {
                self.filter = text.to_owned();
                Ok(())
            }
            Event::Pressed("previous") => shell::media_control(MediaCommand::Previous),
            Event::Pressed("toggle") => shell::media_control(MediaCommand::Toggle),
            Event::Pressed("next") => shell::media_control(MediaCommand::Next),
            Event::Pressed(action) => match action.strip_prefix("focus:").map(str::parse) {
                Some(Ok(id)) => shell::focus(id),
                _ => Ok(()),
            },
            _ => Ok(()),
        };
        if let Err(error) = done {
            kit::log(&error);
        }
    }

    fn answer(&mut self, query: &str, _now: u64) -> Vec<Answer> {
        let query = query.trim();
        if query.is_empty() {
            return Self::meter_answers();
        }
        let Ok(windows) = shell::windows() else {
            return Vec::new();
        };
        let found = matching(&windows, query, ANSWERS).into_iter();
        let answers = found.map(|window| Answer {
            id: window.id.to_string(),
            title: window.title.clone(),
            detail: app_name(&window.app).to_owned(),
            icon: ICON_WINDOW.to_owned(),
            score: 650,
            action: AnswerAction::Run,
        });
        answers.collect()
    }

    fn run_answer(&mut self, id: &str, _now: u64) {
        if let Ok(id) = id.parse()
            && let Err(error) = shell::focus(id)
        {
            kit::log(&error);
        }
    }

    fn shortcut(&mut self, id: &str, _now: u64) {
        if id == "panel"
            && let Err(error) = shell::open_popup("panel")
        {
            kit::log(&error);
        }
    }
}

kit::extension!(Monitor);
