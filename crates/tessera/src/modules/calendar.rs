// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A month to page through. Widget: `calendar`. Under the month stand
//! the time zones the clocks name ([`ClockZones`]), each with its time
//! and its offset.
//!
//! `week_numbers = true` puts the week of the year (ISO 8601) before each
//! row. `calendars = ["C:/path/file.ics", "https://host/feed.ics"]` reads
//! the events of those iCalendar files and feeds ([`crate::calendar_events`])
//! every half hour, on the background executor: a dot under a day marks
//! one with events, and a click on a day lists them under the month. Any
//! feed of holidays in iCalendar form can be named the same way; Tessera
//! has no holidays of its own.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Days, Local, Months, NaiveDate, Weekday};
use gpui::{
    AnyElement, App, AppContext, AsyncApp, Context, Entity, FontWeight, Global, InteractiveElement,
    IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled, Subscription,
    WeakEntity, Window, div, px,
};

use crate::calendar_events::{Event, Occurrence, local_times, occurrences, parse};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::modules::clock::{ClockZones, zone_line};
use crate::motion::Interaction;
use crate::theme::Theme;
use crate::ui::{one_line, state_layer};

/// Weeks shown, enough for any month.
const WEEKS: u64 = 6;

/// How often the files and feeds of events are read again.
const REFRESH: Duration = Duration::from_secs(30 * 60);

/// How often the calendar is drawn again, for the day of today and the
/// times of the zones.
const MINUTE: Duration = Duration::from_secs(60);

/// The most of a file or feed of events that is read.
const FEED_LIMIT: u64 = 16 << 20;

/// The size of the dot under a day with events.
const DOT: f32 = 4.;

/// How wide the number of the week is.
const WEEK_WIDTH: f32 = 24.;

/// The size of the numbers of the weeks and the times of events.
const SMALL_TEXT: f32 = 11.;

pub struct CalendarModule;

impl Module for CalendarModule {
    fn name(&self) -> &'static str {
        "calendar"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("calendar", |context, cx| {
            let week_start = match context.text("week_start") {
                None | Some("monday") => Weekday::Mon,
                Some("sunday") => Weekday::Sun,
                Some(other) => {
                    report!("a week starts on monday or sunday, not `{other}`");
                    Weekday::Mon
                }
            };
            let week_numbers = context.flag("week_numbers").unwrap_or(false);
            let sources = context.texts("calendars").unwrap_or_default();
            let feeds = (!sources.is_empty()).then(|| Feeds::shared(sources, cx));
            cx.new(|cx| Calendar::new(week_start, week_numbers, feeds, cx))
                .into()
        })
    }
}

fn first_of_month(date: NaiveDate) -> NaiveDate {
    date.with_day(1).unwrap_or(date)
}

/// The first day shown for `month`: the start of the week its first day is in.
fn first_shown(month: NaiveDate, week_start: Weekday) -> NaiveDate {
    let into_week = month.weekday().days_since(week_start);
    month - Days::new(u64::from(into_week))
}

/// The week of the year, as ISO 8601 counts, of the row of seven days
/// that starts on `first`: the week its Thursday is in.
fn week_number(first: NaiveDate) -> u32 {
    let to_thursday = Weekday::Thu.days_since(first.weekday());
    (first + Days::new(u64::from(to_thursday)))
        .iso_week()
        .week()
}

/// The address `source` is read from: a feed of `webcal:` is read over
/// HTTPS, as the programs that offer such links mean.
fn address(source: &str) -> String {
    match source.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => source.to_owned(),
    }
}

/// Reads the events of the file or feed `source`.
fn read_source(source: &str) -> Result<Vec<Event>, String> {
    let address = address(source);
    let bytes = if address.starts_with("https://") || address.starts_with("http://") {
        crate::extension::http::download(&address, FEED_LIMIT)?
    } else {
        std::fs::read(Path::new(&address)).map_err(|error| error.to_string())?
    };
    Ok(parse(&String::from_utf8_lossy(&bytes)))
}

/// The events of the files and feeds the calendars name, read again every
/// half hour, shared by every calendar that names the same ones.
struct Feeds {
    sources: Vec<String>,
    /// The events of every source, `None` until they were first read.
    events: Option<Arc<Vec<Event>>>,
    /// Counts the readings, so that a calendar knows when to look again.
    generation: u64,
}

#[derive(Default)]
struct SharedFeeds(Option<Entity<Feeds>>);

impl Global for SharedFeeds {}

impl Feeds {
    /// The feeds of `sources`, which are read from now on, every half
    /// hour; the feeds of other sources are dropped.
    fn shared(sources: Vec<String>, cx: &mut App) -> Entity<Self> {
        let known = cx.default_global::<SharedFeeds>().0.clone();
        if let Some(feeds) = known.filter(|feeds| feeds.read(cx).sources == sources) {
            return feeds;
        }
        let feeds = cx.new(|cx: &mut Context<Self>| {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    let Ok(sources) = this.read_with(cx, |this, _| this.sources.clone()) else {
                        break;
                    };
                    let read = cx.background_executor().spawn(async move {
                        let mut events = Vec::new();
                        for source in &sources {
                            match read_source(source) {
                                Ok(read) => events.extend(read),
                                Err(error) => {
                                    report!("the events of `{source}` were not read: {error}")
                                }
                            }
                        }
                        events
                    });
                    let events = read.await;
                    let taken = this.update(cx, |this, cx| {
                        this.events = Some(Arc::new(events));
                        this.generation += 1;
                        cx.notify();
                    });
                    if taken.is_err() {
                        break;
                    }
                    cx.background_executor().timer(REFRESH).await;
                }
            })
            .detach();
            Self {
                sources,
                events: None,
                generation: 0,
            }
        });
        cx.global_mut::<SharedFeeds>().0 = Some(feeds.clone());
        feeds
    }
}

/// The days of the grid from `first`, and the events found in them.
#[derive(Clone, Default)]
struct Found {
    /// The first day shown and the reading of the feeds they come from.
    key: Option<(NaiveDate, u64)>,
    occurrences: Vec<Occurrence>,
}

/// Shows one month, with today marked, and steps to the months around it.
struct Calendar {
    /// The first day of the month shown.
    month: NaiveDate,
    week_start: Weekday,
    week_numbers: bool,
    /// The day whose events are listed.
    chosen: NaiveDate,
    feeds: Option<Entity<Feeds>>,
    /// The events of the days shown, once they were found.
    found: Found,
    /// The days shown and the reading of the feeds whose events are being
    /// found now.
    finding: Option<(NaiveDate, u64)>,
    /// Pointer feedback of the previous and next buttons.
    steps: [Interaction; 2],
    _changes: Option<Subscription>,
}

impl Calendar {
    fn new(
        week_start: Weekday,
        week_numbers: bool,
        feeds: Option<Entity<Feeds>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let motion = cx.global::<Theme>().motion;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(MINUTE).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        let changes = { feeds.as_ref() }.map(|feeds| cx.observe(feeds, |_, _, cx| cx.notify()));
        let today = Local::now().date_naive();
        Self {
            month: first_of_month(today),
            week_start,
            week_numbers,
            chosen: today,
            feeds,
            found: Found::default(),
            finding: None,
            steps: [Interaction::new(&motion); 2],
            _changes: changes,
        }
    }

    /// Shows the month `months` after the current one, or before it when
    /// negative.
    fn step(&mut self, months: i32) {
        let distance = Months::new(months.unsigned_abs());
        let month = if months < 0 {
            self.month.checked_sub_months(distance)
        } else {
            self.month.checked_add_months(distance)
        };
        self.month = month.unwrap_or(self.month);
    }

    /// Has the events of the days shown found, on the background executor,
    /// where they were not yet for these days and this reading.
    fn find(&mut self, first: NaiveDate, cx: &mut Context<Self>) {
        let Some(feeds) = &self.feeds else {
            return;
        };
        let feeds = feeds.read(cx);
        let Some(events) = feeds.events.clone() else {
            return;
        };
        let key = (first, feeds.generation);
        if self.found.key == Some(key) || self.finding == Some(key) {
            return;
        }
        self.finding = Some(key);
        let last = first + Days::new(WEEKS * 7 - 1);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let found = cx
                .background_executor()
                .spawn(async move { occurrences(&events, first, last, &local_times()) });
            let found = found.await;
            this.update(cx, |this, cx| {
                if this.finding == Some(key) {
                    this.finding = None;
                }
                this.found = Found {
                    key: Some(key),
                    occurrences: found,
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The events found on `day`.
    fn on(&self, day: NaiveDate) -> impl Iterator<Item = &Occurrence> {
        { self.found.occurrences.iter() }.filter(move |occurrence| occurrence.on(day))
    }

    fn step_button(
        &mut self,
        index: usize,
        label: &'static str,
        months: i32,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.steps[index].state(window);
        let button = div()
            .id(("calendar-step", index))
            .size(theme.calendar.cell_size)
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(theme, theme.colors.surface_container, state))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.step(months);
                cx.notify();
            }))
            .child(label);
        Interaction::track(button, cx, move |this: &mut Self| this.steps.get_mut(index))
            .into_any_element()
    }

    fn day(
        &self,
        date: NaiveDate,
        today: NaiveDate,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.on(date).next().is_some();
        let colors = theme.colors;
        let (ground, text) = if date == today {
            (Some(colors.primary), colors.surface_container)
        } else if date == self.chosen && self.feeds.is_some() {
            (Some(colors.secondary_container), colors.on_surface)
        } else if date.month() == self.month.month() {
            (None, colors.on_surface)
        } else {
            (None, colors.outline)
        };
        let dot_color = if date == today {
            colors.surface_container
        } else {
            colors.primary
        };
        let cell = div()
            .id(("calendar-day", date.num_days_from_ce() as usize))
            .size(theme.calendar.cell_size)
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .text_color(text)
            .child(date.day().to_string())
            .children(busy.then(|| {
                div()
                    .absolute()
                    .bottom(px(DOT))
                    .size(px(DOT))
                    .rounded(theme.shape.full)
                    .bg(dot_color)
            }));
        let cell = match ground {
            Some(ground) => cell.bg(ground),
            None => cell,
        };
        let cell = if date == today {
            cell.font_weight(FontWeight::BOLD)
        } else {
            cell
        };
        if self.feeds.is_none() {
            return cell.into_any_element();
        }
        cell.cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.chosen = date;
                cx.notify();
            }))
            .into_any_element()
    }

    /// The number of the week before a row, or the room of it before the
    /// names of the days.
    fn week_cell(&self, first: Option<NaiveDate>, theme: &Theme) -> Option<AnyElement> {
        self.week_numbers.then(|| {
            div()
                .w(px(WEEK_WIDTH))
                .h(theme.calendar.cell_size)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(SMALL_TEXT))
                .text_color(theme.colors.on_surface_variant)
                .children(first.map(|first| week_number(first).to_string()))
                .into_any_element()
        })
    }

    /// The events of the chosen day, or a note that it has none.
    fn agenda(&self, theme: &Theme, language: Language) -> Option<AnyElement> {
        self.feeds.as_ref()?;
        let heading = language.moment(self.chosen.and_time(chrono::NaiveTime::MIN), "%A %-d %B");
        let events: Vec<AnyElement> = self
            .on(self.chosen)
            .map(|occurrence| {
                let when = match occurrence.all_day {
                    true => language.say("All day").to_owned(),
                    false => occurrence.start.format("%H:%M").to_string(),
                };
                let what = match occurrence.location.is_empty() {
                    true => occurrence.summary.clone(),
                    false => format!("{} \u{2013} {}", occurrence.summary, occurrence.location),
                };
                div()
                    .flex()
                    .gap(theme.spacing.small)
                    .child(
                        div()
                            .flex_none()
                            .w(px(56.))
                            .text_size(px(SMALL_TEXT))
                            .text_color(theme.colors.on_surface_variant)
                            .child(when),
                    )
                    .child(div().flex_1().min_w_0().child(one_line(what)))
                    .into_any_element()
            })
            .collect();
        let note = match (&self.found.key, events.is_empty()) {
            (None, _) => Some(language.say("Reading…")),
            (_, true) => Some(language.say("No events")),
            _ => None,
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap(theme.spacing.tiny)
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(one_line(heading)),
                )
                .children(events)
                .children(note.map(|note| {
                    div()
                        .text_color(theme.colors.on_surface_variant)
                        .child(one_line(note))
                }))
                .into_any_element(),
        )
    }
}

impl Render for Calendar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let today = Local::now().date_naive();
        let language = Language::of(cx);
        let first = first_shown(self.month, self.week_start);
        self.find(first, cx);
        let row = || div().flex().justify_between();

        let header = row()
            .items_center()
            .child(self.step_button(0, "‹", -1, &theme, window, cx))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(language.month(self.month)),
            )
            .child(self.step_button(1, "›", 1, &theme, window, cx));
        let weekdays = row()
            .text_color(theme.colors.on_surface_variant)
            .children(self.week_cell(None, &theme))
            .children((0..7).map(|day| {
                div()
                    .w(theme.calendar.cell_size)
                    .flex()
                    .justify_center()
                    .child(language.weekday((first + Days::new(day)).weekday()))
            }));
        let mut weeks = Vec::with_capacity(WEEKS as usize);
        for week in 0..WEEKS {
            let start = first + Days::new(week * 7);
            let days: Vec<AnyElement> = (0..7)
                .map(|day| self.day(start + Days::new(day), today, &theme, cx))
                .collect();
            weeks.push(
                row()
                    .children(self.week_cell(Some(start), &theme))
                    .children(days),
            );
        }
        let zones: Vec<_> = { ClockZones::named(cx).into_iter() }
            .filter_map(|zone| zone_line(&zone, language))
            .map(|line| {
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(one_line(line))
            })
            .collect();
        let agenda = self.agenda(&theme, language);
        let below = (agenda.is_some() || !zones.is_empty()).then(|| {
            div()
                .id("calendar-below")
                .flex_1()
                .min_h(px(0.))
                .pt(theme.spacing.small)
                .flex()
                .flex_col()
                .gap(theme.spacing.small)
                .overflow_y_scroll()
                .children(agenda)
                .children((!zones.is_empty()).then(|| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.spacing.tiny)
                        .children(zones)
                }))
        });
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(header)
            .child(weekdays)
            .children(weeks)
            .children(below)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn the_grid_starts_on_the_configured_weekday() {
        // 1 November 2026 is a Sunday.
        let month = date(2026, 11, 1);
        assert_eq!(first_shown(month, Weekday::Mon), date(2026, 10, 26));
        assert_eq!(first_shown(month, Weekday::Sun), month);
    }

    #[test]
    fn stepping_crosses_year_boundaries() {
        let motion = Theme::default().motion;
        let mut calendar = Calendar {
            month: date(2026, 12, 1),
            week_start: Weekday::Mon,
            week_numbers: false,
            chosen: date(2026, 12, 1),
            feeds: None,
            found: Found::default(),
            finding: None,
            steps: [Interaction::new(&motion); 2],
            _changes: None,
        };
        calendar.step(1);
        assert_eq!(calendar.month, date(2027, 1, 1));
        calendar.step(-2);
        assert_eq!(calendar.month, date(2026, 11, 1));
    }

    #[test]
    fn a_row_is_numbered_by_the_week_of_its_thursday() {
        // 29 December 2025 starts week 1 of 2026, which has its Thursday.
        assert_eq!(week_number(date(2025, 12, 29)), 1);
        // A row from Sunday 28 December 2025 has the same Thursday.
        assert_eq!(week_number(date(2025, 12, 28)), 1);
        // 2026 has 53 weeks: the last starts on Monday 28 December.
        assert_eq!(week_number(date(2026, 12, 28)), 53);
        assert_eq!(week_number(date(2027, 1, 4)), 1);
        assert_eq!(week_number(date(2026, 11, 23)), 48);
    }

    #[test]
    fn a_webcal_feed_is_read_over_https() {
        assert_eq!(
            address("webcal://example.org/holidays.ics"),
            "https://example.org/holidays.ics"
        );
        assert_eq!(address("C:/Users/me/cal.ics"), "C:/Users/me/cal.ics");
    }
}
