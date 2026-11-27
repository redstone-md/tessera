// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A wall clock that repaints once per minute. Widget: `clock`.
//!
//! With `zones = ["Europe/Berlin", "Tokyo"]` the tip of the clock, while
//! the pointer rests on it, gives the date and the time in each of those
//! zones with its offset, and the calendar shows them under the month
//! ([`ClockZones`]). A zone is named as [`crate::time_zones::find`] takes
//! it: an IANA identifier, a zone of Windows or a city.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use chrono::format::{Item, StrftimeItems};
use chrono::{Local, NaiveDateTime, Timelike};
use gpui::{
    App, AppContext, AsyncApp, Bounds, Context, FontWeight, Global, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, StatefulInteractiveElement, Styled, WeakEntity,
    Window, canvas,
};

use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::popup::PopupAnchor;
use crate::theme::Theme;
use crate::time_zones::{self, Zone};
use crate::tip::Tips;
use crate::ui::pill;

/// What tells the tip of a clock from other tips.
const TIP_KIND: &str = "clock";

/// How the date is written at the head of the tip.
const TIP_DATE: &str = "%A %-d %B %Y";

/// How a time in another zone is written.
const ZONE_TIME: &str = "%H:%M";

/// Between two zones in the tip.
const SEPARATOR: &str = "  ·  ";

pub struct ClockModule;

impl Module for ClockModule {
    fn name(&self) -> &'static str {
        "clock"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("clock", |context, cx| {
            let format = context
                .text("format")
                .filter(|format| {
                    let valid = is_valid_format(format);
                    if !valid {
                        report!("`{format}` is not a clock format, the default is used");
                    }
                    valid
                })
                .unwrap_or(DEFAULT_FORMAT)
                .to_owned();
            let zones = context.texts("zones").unwrap_or_default();
            cx.new(|cx| Clock::new(format, zones, cx)).into()
        })
    }
}

const DEFAULT_FORMAT: &str = "%a %-d %b  %H:%M";

/// Whether `format` is a `strftime` pattern that can be displayed.
fn is_valid_format(format: &str) -> bool {
    !StrftimeItems::new(format).any(|item| item == Item::Error)
}

/// The zones the clocks that are shown name, for the calendar, in the
/// order they are named; each once. A clock that is gone, as after the
/// configuration was read again, names none.
#[derive(Default)]
pub struct ClockZones(Vec<(WeakEntity<Clock>, Vec<Zone>)>);

impl Global for ClockZones {}

impl ClockZones {
    /// The zones the clocks name.
    pub fn named(cx: &App) -> Vec<Zone> {
        let Some(this) = cx.try_global::<Self>() else {
            return Vec::new();
        };
        let mut named: Vec<Zone> = Vec::new();
        let shown = { this.0.iter() }.filter(|(clock, _)| clock.upgrade().is_some());
        for zone in shown.flat_map(|(_, zones)| zones) {
            if !named.contains(zone) {
                named.push(zone.clone());
            }
        }
        named
    }

    /// Has `clock` name `zones` from now on, while it is shown.
    fn add(clock: WeakEntity<Clock>, zones: Vec<Zone>, cx: &mut App) {
        let this = &mut cx.default_global::<Self>().0;
        this.retain(|(clock, _)| clock.upgrade().is_some());
        if !zones.is_empty() {
            this.push((clock, zones));
        }
    }
}

/// Finds the zones `names` names, on a thread that may wait: Windows is
/// asked for its zones the first time. A name that names no zone is
/// reported and left out.
pub async fn resolve(names: Vec<String>, cx: &mut AsyncApp) -> Vec<Zone> {
    if names.is_empty() {
        return Vec::new();
    }
    let found = cx.background_executor().spawn(async move {
        { names.into_iter() }
            .filter_map(|name| {
                let zone = time_zones::find(&name);
                if zone.is_none() {
                    report!("`{name}` names no time zone Tessera knows");
                }
                zone
            })
            .collect::<Vec<Zone>>()
    });
    found.await
}

/// A zone as the tip and the calendar give it: its city, the time there
/// and how far it is from universal time, as "Tokyo 22:05 UTC+09:00".
pub fn zone_line(zone: &Zone, language: Language) -> Option<String> {
    let now = zone.now()?;
    let offset = zone.offset_now().unwrap_or_default();
    Some(format!(
        "{} {} {offset}",
        zone.label(),
        language.moment(now, ZONE_TIME)
    ))
}

/// What the tip of a clock says at `now`: the date, then each zone.
fn tip_text(now: NaiveDateTime, zones: &[String], language: Language) -> String {
    let date = language.moment(now, TIP_DATE);
    std::iter::once(date)
        .chain(zones.iter().cloned())
        .collect::<Vec<String>>()
        .join(SEPARATOR)
}

/// Displays the local date and time to minute precision, in the `strftime`
/// pattern the `format` option gives.
pub struct Clock {
    format: String,
    /// The zones its tip gives, once they were found.
    zones: Vec<Zone>,
    /// Where it was last drawn, for its tip.
    drawn: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Clock {
    fn new(format: String, names: Vec<String>, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let zones = resolve(names, cx).await;
            let taken = this.update(cx, |this, cx| {
                ClockZones::add(cx.weak_entity(), zones.clone(), cx);
                this.zones = zones;
            });
            if taken.is_err() {
                return;
            }
            loop {
                cx.background_executor()
                    .timer(Self::until_next_minute())
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            format,
            zones: Vec::new(),
            drawn: Rc::default(),
        }
    }

    /// Time left until the displayed minute changes.
    fn until_next_minute() -> Duration {
        let now = Local::now();
        let elapsed = u64::from(now.second()) * 1_000 + u64::from(now.timestamp_subsec_millis());
        Duration::from_millis(60_000u64.saturating_sub(elapsed).max(50))
    }

    /// The pointer came onto the clock, or left it: the tip with the date
    /// and the zones comes out of the bar under it, or goes.
    fn tip(&self, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        let key = (TIP_KIND, cx.entity_id().as_u64());
        let Some(drawn) = self.drawn.get().filter(|_| hovered) else {
            Tips::leave(key, cx);
            return;
        };
        let language = Language::of(cx);
        let zones: Vec<String> = { self.zones.iter() }
            .filter_map(|zone| zone_line(zone, language))
            .collect();
        let text = tip_text(Local::now().naive_local(), &zones, language);
        let middle = drawn.origin.x + drawn.size.width / 2.;
        match PopupAnchor::at(window, middle, cx) {
            Ok(anchor) => Tips::request(key, text.into(), anchor, cx),
            Err(error) => report!("a tip has nowhere to open: {error:#}"),
        }
    }
}

impl Render for Clock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let drawn = self.drawn.clone();
        let measure = move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
            drawn.set(Some(bounds));
        };
        pill(cx.global::<Theme>())
            .id("clock")
            .relative()
            .font_weight(FontWeight::MEDIUM)
            .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
            .child(Language::of(cx).moment(Local::now().naive_local(), &self.format))
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                this.tip(*hovered, window, cx);
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_with_unknown_specifiers_are_rejected() {
        assert!(is_valid_format(DEFAULT_FORMAT));
        assert!(is_valid_format("%H:%M:%S"));
        assert!(!is_valid_format("%Q"));
        assert!(is_valid_format(TIP_DATE) && is_valid_format(ZONE_TIME));
    }

    #[test]
    fn the_tip_gives_the_date_then_each_zone() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 27)
            .unwrap()
            .and_hms_opt(14, 5, 0)
            .unwrap();
        let zones = ["Tokyo 22:05 UTC+09:00".to_owned()];
        assert_eq!(
            tip_text(now, &zones, Language::English),
            "Friday 27 November 2026  ·  Tokyo 22:05 UTC+09:00"
        );
        assert_eq!(
            tip_text(now, &[], Language::English),
            "Friday 27 November 2026"
        );
    }
}
