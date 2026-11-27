// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Example extension: the weather at a place the user names.
//!
//! It asks Open-Meteo, which needs no account, for the weather at the
//! `latitude` and `longitude` of `[extension.weather]` in the shell's
//! configuration, set there or in Settings > Extensions, on a timer, and
//! shows the answer in two widgets: `now` for a bar and `forecast` for a
//! popup. The place is sent to Open-Meteo and to nobody else; without it
//! nothing is asked.

mod model;

use std::cell::RefCell;

use model::{Place, Report, Scale, ago, day_name, degrees, sky};

wit_bindgen::generate!({
    world: "extension",
    path: "../../wit",
});

/// Minutes between questions when the settings give none.
const DEFAULT_INTERVAL_MINUTES: u64 = 20;

/// Seconds before a question that failed is asked again.
const RETRY_SECONDS: u64 = 60;

struct App {
    place: Result<Place, String>,
    scale: Scale,
    /// Seconds between questions.
    interval: u64,
    asked_at: Option<u64>,
    pending: Option<u32>,
    /// The last answer and when it came.
    report: Option<(Report, u64)>,
    error: Option<String>,
}

impl App {
    fn start() -> Self {
        let scale = match setting("unit") {
            None => Scale::Celsius,
            Some(text) => Scale::named(&text).unwrap_or_else(|| {
                log(&format!(
                    "the setting `unit` is celsius or fahrenheit, not `{text}`"
                ));
                Scale::Celsius
            }),
        };
        let minutes = setting("interval_minutes")
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(DEFAULT_INTERVAL_MINUTES);
        Self {
            place: Place::of(
                setting("latitude").as_deref(),
                setting("longitude").as_deref(),
            ),
            scale,
            interval: minutes.clamp(5, 24 * 60) * 60,
            asked_at: None,
            pending: None,
            report: None,
            error: None,
        }
    }

    /// Asks for the weather when it is time to.
    fn ask_due(&mut self, now: u64) {
        let Ok(place) = self.place else {
            return;
        };
        let wait = match self.error {
            Some(_) => RETRY_SECONDS,
            None => self.interval,
        };
        let due = self.asked_at.is_none_or(|asked| now >= asked + wait);
        if self.pending.is_some() || !due {
            return;
        }
        self.asked_at = Some(now);
        let request = Request {
            method: "GET".to_owned(),
            url: place.address(self.scale),
            headers: Vec::new(),
            body: None,
        };
        match fetch(&request) {
            Ok(number) => self.pending = Some(number),
            Err(error) => self.error = Some(error),
        }
    }

    fn answered(&mut self, request: u32, outcome: Result<Response, String>, now: u64) {
        if self.pending != Some(request) {
            return;
        }
        self.pending = None;
        let report = match outcome {
            Ok(response) if (200..300).contains(&response.status) => Report::read(&response.body),
            Ok(response) => Err(format!("Open-Meteo answered {}.", response.status)),
            Err(error) => Err(format!("No answer: {error}")),
        };
        match report {
            Ok(report) => (self.report, self.error) = (Some((report, now)), None),
            Err(error) => self.error = Some(error),
        }
    }

    /// What there is to say when there is no weather to show.
    fn excuse(&self) -> Option<String> {
        match (&self.place, &self.report, &self.error) {
            (Err(error), ..) => Some(error.clone()),
            (_, Some(_), _) => None,
            (_, None, Some(error)) => Some(error.clone()),
            (_, None, None) => Some("Asking for the weather…".to_owned()),
        }
    }

    /// The temperature and the sky, for a bar.
    fn now(&self) -> Tree {
        match (&self.report, self.excuse()) {
            (Some((report, _)), None) => row(vec![text(format!(
                "{} {}",
                degrees(report.temperature),
                sky(report.code)
            ))]),
            (_, excuse) => row(vec![caption(match excuse {
                Some(_) if self.place.is_err() => "Weather: no place".to_owned(),
                Some(excuse) => excuse,
                None => String::new(),
            })]),
        }
    }

    /// The weather now and on the next days, for a popup.
    fn forecast(&self, now: u64) -> Tree {
        let refresh = Tree::leaf(NodeKind::Button(Button {
            label: "Refresh".to_owned(),
            action: "refresh".to_owned(),
        }));
        let Some((report, came)) = &self.report else {
            let excuse = self.excuse().unwrap_or_default();
            return column(vec![caption(excuse), row(vec![refresh])]);
        };
        let mut lines = vec![
            text(format!(
                "{}{}, {}",
                degrees(report.temperature),
                self.scale.letter(),
                sky(report.code)
            )),
            caption(format!(
                "Feels like {}, wind {:.0} {}, humidity {:.0}%",
                degrees(report.feels),
                report.wind,
                report.wind_unit,
                report.humidity
            )),
        ];
        lines.extend(report.days.iter().enumerate().map(|(place, day)| {
            row(vec![
                text(day_name(place, &day.date).to_owned()),
                caption(sky(day.code).to_owned()),
                text(format!("{} / {}", degrees(day.high), degrees(day.low))),
            ])
        }));
        if let Some(error) = &self.error {
            lines.push(caption(error.clone()));
        }
        lines.push(row(vec![
            caption(format!("Open-Meteo.com, {}", ago(now, *came))),
            refresh,
        ]));
        column(lines)
    }
}

/// A node with the nodes inside it, before they are numbered.
struct Tree {
    kind: NodeKind,
    children: Vec<Tree>,
}

impl Tree {
    fn leaf(kind: NodeKind) -> Self {
        Self {
            kind,
            children: Vec::new(),
        }
    }

    /// The nodes in the order Tessera wants them: each before the nodes
    /// inside it.
    fn nodes(self) -> Vec<Node> {
        let mut nodes = Vec::new();
        self.place(&mut nodes);
        nodes
    }

    fn place(self, nodes: &mut Vec<Node>) -> u32 {
        let at = nodes.len();
        nodes.push(Node {
            kind: self.kind,
            children: Vec::new(),
        });
        let children = self
            .children
            .into_iter()
            .map(|child| child.place(nodes))
            .collect();
        nodes[at].children = children;
        at as u32
    }
}

fn row(children: Vec<Tree>) -> Tree {
    Tree {
        kind: NodeKind::Row,
        children,
    }
}

fn column(children: Vec<Tree>) -> Tree {
    Tree {
        kind: NodeKind::Column,
        children,
    }
}

fn text(text: String) -> Tree {
    Tree::leaf(NodeKind::Text(text))
}

fn caption(text: String) -> Tree {
    Tree::leaf(NodeKind::Caption(text))
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<T>(run: impl FnOnce(&mut App) -> T) -> T {
    APP.with_borrow_mut(|app| run(app.get_or_insert_with(App::start)))
}

struct Weather;

impl Guest for Weather {
    fn render(widget: String) -> Vec<Node> {
        let now = now();
        with_app(|app| {
            app.ask_due(now);
            match widget.as_str() {
                "now" => app.now(),
                _ => app.forecast(now),
            }
            .nodes()
        })
    }

    fn activate(_widget: String, action: String) {
        if action == "refresh" {
            with_app(|app| app.asked_at = None);
        }
    }

    fn fetched(request: u32, outcome: Result<Response, String>) {
        let now = now();
        with_app(|app| app.answered(request, outcome, now));
    }
}

export!(Weather);
