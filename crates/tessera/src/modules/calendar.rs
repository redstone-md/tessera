// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A month to page through. Widget: `calendar`.

use chrono::{Datelike, Days, Local, Months, NaiveDate, Weekday};
use gpui::{
    AnyElement, AppContext, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Window, div,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::Interaction;
use crate::theme::Theme;
use crate::ui::state_layer;

/// Weeks shown, enough for any month.
const WEEKS: u64 = 6;

pub struct CalendarModule;

impl Module for CalendarModule {
    fn name(&self) -> &'static str {
        "calendar"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("calendar", |context, cx| {
            let week_start = match context.text("week_start") {
                None | Some("monday") => Weekday::Mon,
                Some("sunday") => Weekday::Sun,
                Some(other) => {
                    report!("a week starts on monday or sunday, not `{other}`");
                    Weekday::Mon
                }
            };
            let motion = cx.global::<Theme>().motion;
            cx.new(|_| Calendar {
                month: first_of_month(Local::now().date_naive()),
                week_start,
                steps: [Interaction::new(&motion); 2],
            })
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

/// Shows one month, with today marked, and steps to the months around it.
struct Calendar {
    /// The first day of the month shown.
    month: NaiveDate,
    week_start: Weekday,
    /// Pointer feedback of the previous and next buttons.
    steps: [Interaction; 2],
}

impl Calendar {
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

    fn day(&self, date: NaiveDate, today: NaiveDate, theme: &Theme) -> AnyElement {
        let cell = div()
            .size(theme.calendar.cell_size)
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .child(date.day().to_string());
        if date == today {
            cell.bg(theme.colors.primary)
                .text_color(theme.colors.surface_container)
                .font_weight(FontWeight::BOLD)
        } else if date.month() == self.month.month() {
            cell
        } else {
            cell.text_color(theme.colors.outline)
        }
        .into_any_element()
    }
}

impl Render for Calendar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let today = Local::now().date_naive();
        let first = first_shown(self.month, self.week_start);
        let row = || div().flex().justify_between();

        let header = row()
            .items_center()
            .child(self.step_button(0, "‹", -1, &theme, window, cx))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.month.format("%B %Y").to_string()),
            )
            .child(self.step_button(1, "›", 1, &theme, window, cx));
        let weekdays = row()
            .text_color(theme.colors.on_surface_variant)
            .children((0..7).map(|day| {
                div()
                    .w(theme.calendar.cell_size)
                    .flex()
                    .justify_center()
                    .child((first + Days::new(day)).format("%a").to_string())
            }));
        let weeks = (0..WEEKS).map(|week| {
            row().children(
                (0..7).map(|day| self.day(first + Days::new(week * 7 + day), today, &theme)),
            )
        });

        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(header)
            .child(weekdays)
            .children(weeks)
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
            steps: [Interaction::new(&motion); 2],
        };
        calendar.step(1);
        assert_eq!(calendar.month, date(2027, 1, 1));
        calendar.step(-2);
        assert_eq!(calendar.month, date(2026, 11, 1));
    }
}
