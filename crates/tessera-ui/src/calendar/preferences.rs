// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use tessera_system::calendar::{CalendarError, CalendarSnapshot, WeekStart};

/// Source-defined settings order; this is not the native Monday-first weekday index.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum StartOfWeek {
    #[default]
    Monday,
    Sunday,
    Saturday,
}

impl StartOfWeek {
    pub fn index(self) -> i32 {
        match self {
            Self::Monday => 0,
            Self::Sunday => 1,
            Self::Saturday => 2,
        }
    }

    /// Synthetic or stale selector values must never silently change a saved policy.
    pub fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Monday),
            1 => Some(Self::Sunday),
            2 => Some(Self::Saturday),
            _ => None,
        }
    }
}

impl From<StartOfWeek> for WeekStart {
    fn from(start: StartOfWeek) -> Self {
        match start {
            StartOfWeek::Monday => Self::Monday,
            StartOfWeek::Sunday => Self::Sunday,
            StartOfWeek::Saturday => Self::Saturday,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GeneralPreferences {
    start_of_week: StartOfWeek,
}

impl GeneralPreferences {
    pub fn start_of_week(&self) -> StartOfWeek {
        self.start_of_week
    }

    pub fn with_start_of_week(self, start_of_week: StartOfWeek) -> Self {
        Self { start_of_week }
    }
}

/// Adapt before refresh: native first-day changes must not masquerade as locale changes.
/// All localized labels and the authoritative native civil date remain untouched.
pub(super) fn adapt_snapshot(
    snapshot: CalendarSnapshot,
    start: StartOfWeek,
) -> Result<CalendarSnapshot, CalendarError> {
    CalendarSnapshot::new(
        snapshot.today(),
        snapshot.locale_name().into(),
        snapshot.months().clone(),
        snapshot.weekdays_full().clone(),
        snapshot.weekdays_abbreviated().clone(),
        start.into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_system::calendar::{CalendarDirection, CalendarState, CalendarView, CivilDate};

    fn native(today: CivilDate, start: WeekStart) -> CalendarSnapshot {
        CalendarSnapshot::new(
            today,
            "fr-FR".into(),
            [
                "janvier",
                "février",
                "mars",
                "avril",
                "mai",
                "juin",
                "juillet",
                "août",
                "septembre",
                "octobre",
                "novembre",
                "décembre",
            ]
            .map(String::from),
            [
                "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
            ]
            .map(String::from),
            ["lun", "mar", "mer", "jeu", "ven", "sam", "dim"].map(String::from),
            start,
        )
        .unwrap()
    }

    #[test]
    fn source_choices_have_fallible_selector_indexes_and_explicit_native_mapping() {
        let original = GeneralPreferences::default();
        assert_eq!(original.start_of_week(), StartOfWeek::Monday);
        for (index, choice, native) in [
            (0, StartOfWeek::Monday, WeekStart::Monday),
            (1, StartOfWeek::Sunday, WeekStart::Sunday),
            (2, StartOfWeek::Saturday, WeekStart::Saturday),
        ] {
            assert_eq!(choice.index(), index);
            assert_eq!(StartOfWeek::from_index(index), Some(choice));
            assert_eq!(WeekStart::from(choice), native);
            assert_eq!(original.with_start_of_week(choice).start_of_week(), choice);
        }
        for invalid in [i32::MIN, -1, 3, 7, i32::MAX] {
            assert_eq!(StartOfWeek::from_index(invalid), None);
        }
        assert_eq!(
            original.start_of_week(),
            StartOfWeek::Monday,
            "builder is immutable"
        );
    }

    #[test]
    fn policy_rotates_native_metadata_and_aligns_leap_and_year_boundary_grids() {
        for today in [
            CivilDate::new(2024, 2, 29).unwrap(),
            CivilDate::new(2023, 12, 31).unwrap(),
            CivilDate::new(2024, 1, 1).unwrap(),
        ] {
            let source = native(today, WeekStart::Sunday);
            for (policy, label, first_date) in [
                (
                    StartOfWeek::Monday,
                    "lun",
                    match today.month() {
                        2 => CivilDate::new(2024, 1, 29).unwrap(),
                        12 => CivilDate::new(2023, 11, 27).unwrap(),
                        _ => CivilDate::new(2024, 1, 1).unwrap(),
                    },
                ),
                (
                    StartOfWeek::Sunday,
                    "dim",
                    match today.month() {
                        2 => CivilDate::new(2024, 1, 28).unwrap(),
                        12 => CivilDate::new(2023, 11, 26).unwrap(),
                        _ => CivilDate::new(2023, 12, 31).unwrap(),
                    },
                ),
                (
                    StartOfWeek::Saturday,
                    "sam",
                    match today.month() {
                        2 => CivilDate::new(2024, 1, 27).unwrap(),
                        12 => CivilDate::new(2023, 11, 25).unwrap(),
                        _ => CivilDate::new(2023, 12, 30).unwrap(),
                    },
                ),
            ] {
                let adapted = adapt_snapshot(source.clone(), policy).unwrap();
                assert_eq!(adapted.today(), source.today());
                assert_eq!(adapted.locale_name(), source.locale_name());
                assert_eq!(adapted.months(), source.months());
                assert_eq!(adapted.weekdays_full(), source.weekdays_full());
                assert_eq!(
                    adapted.weekdays_abbreviated(),
                    source.weekdays_abbreviated()
                );
                let projection = CalendarState::new(adapted).projection();
                assert_eq!(projection.weekdays[0], label);
                assert_eq!(projection.days[0].date, Some(first_date));
                assert_eq!(projection.days.iter().filter(|day| day.today).count(), 1);
                assert!(
                    projection
                        .days
                        .iter()
                        .any(|day| day.description.starts_with("lundi")
                            || day.description.starts_with("dimanche"))
                );
            }
        }
    }

    #[test]
    fn adapting_before_refresh_preserves_browsing_but_real_locale_changes_still_reset() {
        let today = CivilDate::new(2024, 2, 29).unwrap();
        let mut calendar = CalendarState::new(
            adapt_snapshot(native(today, WeekStart::Sunday), StartOfWeek::Monday).unwrap(),
        );
        calendar.select_day(CivilDate::new(2024, 2, 28).unwrap());
        calendar.navigate(CalendarDirection::Next);
        calendar.toggle_view();
        let displayed = calendar.displayed();
        let selected = calendar.selected();
        let tomorrow = CivilDate::new(2024, 3, 1).unwrap();
        for start in [WeekStart::Sunday, WeekStart::Tuesday, WeekStart::Saturday] {
            calendar.refresh(adapt_snapshot(native(tomorrow, start), StartOfWeek::Monday).unwrap());
            assert_eq!(calendar.displayed(), displayed);
            assert_eq!(calendar.selected(), selected);
            assert_eq!(calendar.view(), CalendarView::Year);
        }
        let source = native(tomorrow, WeekStart::Sunday);
        let changed = CalendarSnapshot::new(
            source.today(),
            source.locale_name().into(),
            source.months().clone(),
            source.weekdays_full().clone(),
            std::array::from_fn(|i| format!("nouveau {i}")),
            source.week_start(),
        )
        .unwrap();
        calendar.refresh(adapt_snapshot(changed, StartOfWeek::Monday).unwrap());
        assert_eq!(calendar.displayed(), tomorrow);
        assert_eq!(calendar.selected(), tomorrow);
        assert_eq!(calendar.view(), CalendarView::Year);
    }
}
