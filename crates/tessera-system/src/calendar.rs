// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Read-only local calendar metadata and pure browsing/selection state.

use std::fmt;

use chrono::{Datelike, Days, Months, NaiveDate};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivilDate(NaiveDate);

impl CivilDate {
    pub fn new(year: i32, month: u32, day: u32) -> Option<Self> {
        NaiveDate::from_ymd_opt(year, month, day).map(Self)
    }

    pub fn year(self) -> i32 {
        self.0.year()
    }

    pub fn month(self) -> u32 {
        self.0.month()
    }

    pub fn day(self) -> u32 {
        self.0.day()
    }

    /// Checked month arithmetic retains the day, clamping at month end.
    pub fn checked_add_months(self, delta: i32) -> Option<Self> {
        let months = Months::new(delta.unsigned_abs());
        if delta < 0 {
            self.0.checked_sub_months(months)
        } else {
            self.0.checked_add_months(months)
        }
        .map(Self)
    }
}

impl fmt::Display for CivilDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0.format("%Y-%m-%d"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeekStart {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl WeekStart {
    fn index(self) -> usize {
        match self {
            Self::Monday => 0,
            Self::Tuesday => 1,
            Self::Wednesday => 2,
            Self::Thursday => 3,
            Self::Friday => 4,
            Self::Saturday => 5,
            Self::Sunday => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarErrorKind {
    Unsupported,
    Unavailable,
    InvalidData,
    Busy,
    Stopped,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarError {
    pub kind: CalendarErrorKind,
    pub message: String,
}

impl CalendarError {
    pub fn new(kind: CalendarErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for CalendarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CalendarError {}

/// Immutable native metadata. Weekday arrays are always Monday-first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarSnapshot {
    today: CivilDate,
    locale_name: String,
    months: [String; 12],
    weekdays_full: [String; 7],
    weekdays_abbreviated: [String; 7],
    week_start: WeekStart,
}

impl CalendarSnapshot {
    pub fn new(
        today: CivilDate,
        locale_name: String,
        months: [String; 12],
        weekdays_full: [String; 7],
        weekdays_abbreviated: [String; 7],
        week_start: WeekStart,
    ) -> Result<Self, CalendarError> {
        // Native locale capacity is 85 UTF-16 units, including its terminator.
        if !valid_text(&locale_name, 84)
            || months
                .iter()
                .chain(&weekdays_full)
                .chain(&weekdays_abbreviated)
                .any(|label| !valid_text(label, 256))
        {
            return Err(CalendarError::new(
                CalendarErrorKind::InvalidData,
                "Invalid calendar locale metadata",
            ));
        }
        Ok(Self {
            today,
            locale_name,
            months,
            weekdays_full,
            weekdays_abbreviated,
            week_start,
        })
    }

    pub fn today(&self) -> CivilDate {
        self.today
    }

    pub fn locale_name(&self) -> &str {
        &self.locale_name
    }

    pub fn months(&self) -> &[String; 12] {
        &self.months
    }

    pub fn weekdays_full(&self) -> &[String; 7] {
        &self.weekdays_full
    }

    pub fn weekdays_abbreviated(&self) -> &[String; 7] {
        &self.weekdays_abbreviated
    }

    pub fn week_start(&self) -> WeekStart {
        self.week_start
    }

    fn same_locale(&self, other: &Self) -> bool {
        self.locale_name == other.locale_name
            && self.months == other.months
            && self.weekdays_full == other.weekdays_full
            && self.weekdays_abbreviated == other.weekdays_abbreviated
            && self.week_start == other.week_start
    }
}

fn valid_text(text: &str, max_utf16: usize) -> bool {
    !text.trim().is_empty()
        && !text.chars().any(char::is_control)
        && text.encode_utf16().take(max_utf16 + 1).count() <= max_utf16
}

pub type CalendarReadCompletion =
    Box<dyn FnOnce(Result<CalendarSnapshot, CalendarError>) + Send + 'static>;

/// Prompt read acceptance: `Ok` guarantees exactly one completion; `Err` none.
/// Completions may run inline or on a worker. No calendar mutation is exposed.
pub trait CalendarHost: Send + Sync {
    fn read(&self, completion: CalendarReadCompletion) -> Result<(), CalendarError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarView {
    Month,
    Year,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarDirection {
    Previous,
    Next,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarDay {
    pub date: Option<CivilDate>,
    pub label: String,
    pub description: String,
    pub off_month: bool,
    pub today: bool,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarMonth {
    pub month: u32,
    pub label: String,
    pub current: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarProjection {
    pub view: CalendarView,
    pub title: String,
    pub weekdays: [String; 7],
    pub days: Vec<CalendarDay>,
    pub months: [CalendarMonth; 12],
    pub can_previous: bool,
    pub can_next: bool,
}

/// Browsing and selection are separate; navigation never selects a new date.
pub struct CalendarState {
    snapshot: CalendarSnapshot,
    displayed: CivilDate,
    selected: CivilDate,
    view: CalendarView,
}

impl CalendarState {
    pub fn new(snapshot: CalendarSnapshot) -> Self {
        Self {
            displayed: snapshot.today,
            selected: snapshot.today,
            snapshot,
            view: CalendarView::Month,
        }
    }

    pub fn view(&self) -> CalendarView {
        self.view
    }

    pub fn displayed(&self) -> CivilDate {
        self.displayed
    }

    pub fn selected(&self) -> CivilDate {
        self.selected
    }

    /// Change grid alignment without changing browsing, selection, or view mode.
    pub fn set_week_start(&mut self, start: WeekStart) {
        self.snapshot.week_start = start;
    }

    pub fn projection(&self) -> CalendarProjection {
        let year = self.displayed.0.format("%Y");
        let title = match self.view {
            CalendarView::Month => format!(
                "{} {year}",
                self.snapshot.months[(self.displayed.month() - 1) as usize]
            ),
            CalendarView::Year => year.to_string(),
        };
        CalendarProjection {
            view: self.view,
            title,
            weekdays: std::array::from_fn(|index| {
                self.snapshot.weekdays_abbreviated[(index + self.snapshot.week_start.index()) % 7]
                    .clone()
            }),
            days: if self.view == CalendarView::Month {
                self.month_days()
            } else {
                Vec::new()
            },
            months: std::array::from_fn(|index| CalendarMonth {
                month: index as u32 + 1,
                label: self.snapshot.months[index].clone(),
                current: self.displayed.year() == self.snapshot.today.year()
                    && index as u32 + 1 == self.snapshot.today.month(),
            }),
            can_previous: self
                .navigation_target(CalendarDirection::Previous)
                .is_some(),
            can_next: self.navigation_target(CalendarDirection::Next).is_some(),
        }
    }

    pub fn navigate(&mut self, direction: CalendarDirection) -> bool {
        let Some(date) = self.navigation_target(direction) else {
            return false;
        };
        self.displayed = date;
        true
    }

    pub fn toggle_view(&mut self) {
        self.view = match self.view {
            CalendarView::Month => CalendarView::Year,
            CalendarView::Year => CalendarView::Month,
        };
    }

    pub fn today(&mut self) {
        self.displayed = self.snapshot.today;
        self.selected = self.snapshot.today;
    }

    pub fn select_day(&mut self, date: CivilDate) -> bool {
        if self.view != CalendarView::Month
            || !self.month_days().iter().any(|day| day.date == Some(date))
        {
            return false;
        }
        self.displayed = date;
        self.selected = date;
        true
    }

    pub fn select_month(&mut self, month: u32) -> bool {
        if self.view != CalendarView::Year {
            return false;
        }
        let Some(date) = CivilDate::new(self.displayed.year(), month, 1) else {
            return false;
        };
        self.displayed = date;
        self.view = CalendarView::Month;
        true
    }

    /// A fresh day alone updates highlighting, not the user's browsing position.
    pub fn refresh(&mut self, snapshot: CalendarSnapshot) {
        if !self.snapshot.same_locale(&snapshot) {
            self.displayed = snapshot.today;
            self.selected = snapshot.today;
        }
        self.snapshot = snapshot;
    }

    fn navigation_target(&self, direction: CalendarDirection) -> Option<CivilDate> {
        let step = if self.view == CalendarView::Month {
            1
        } else {
            12
        };
        self.displayed.checked_add_months(match direction {
            CalendarDirection::Previous => -step,
            CalendarDirection::Next => step,
        })
    }

    fn month_days(&self) -> Vec<CalendarDay> {
        // Every validated month has day 1. Keep construction checked even at
        // chrono's limits; enclosing days outside that range are placeholders.
        let Some(first) = self.displayed.0.with_day(1) else {
            return Vec::new();
        };
        let last = first
            .checked_add_months(Months::new(1))
            .and_then(|next| next.pred_opt())
            .unwrap_or(NaiveDate::MAX);
        let leading = (first.weekday().num_days_from_monday() as usize + 7
            - self.snapshot.week_start.index())
            % 7;
        let count = (leading + last.day() as usize).div_ceil(7) * 7;
        (0..count)
            .map(|index| {
                let date = if index < leading {
                    first.checked_sub_days(Days::new((leading - index) as u64))
                } else {
                    first.checked_add_days(Days::new((index - leading) as u64))
                }
                .map(CivilDate);
                CalendarDay {
                    date,
                    label: date.map_or_else(String::new, |date| date.day().to_string()),
                    description: date.map_or_else(String::new, |date| {
                        format!(
                            "{} {date}",
                            self.snapshot.weekdays_full
                                [date.0.weekday().num_days_from_monday() as usize]
                        )
                    }),
                    off_month: date.is_some_and(|date| date.month() != self.displayed.month()),
                    today: date == Some(self.snapshot.today),
                    selected: date == Some(self.selected),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
