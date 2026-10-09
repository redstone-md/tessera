// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

fn date(year: i32, month: u32, day: u32) -> CivilDate {
    CivilDate::new(year, month, day).expect("test date")
}

fn snapshot(today: CivilDate, start: WeekStart) -> CalendarSnapshot {
    CalendarSnapshot::new(
        today,
        "fixture-locale".into(),
        std::array::from_fn(|index| format!("Month {}", index + 1)),
        std::array::from_fn(|index| format!("Weekday {}", index + 1)),
        std::array::from_fn(|index| format!("W{}", index + 1)),
        start,
    )
    .expect("fixture metadata")
}

const STARTS: [WeekStart; 7] = [
    WeekStart::Monday,
    WeekStart::Tuesday,
    WeekStart::Wednesday,
    WeekStart::Thursday,
    WeekStart::Friday,
    WeekStart::Saturday,
    WeekStart::Sunday,
];

#[test]
fn civil_dates_validate_and_checked_months_clamp_without_integer_overflow() {
    assert!(CivilDate::new(2023, 2, 29).is_none());
    assert!(CivilDate::new(2024, 0, 1).is_none());
    assert!(CivilDate::new(2024, 13, 1).is_none());
    assert!(CivilDate::new(2024, 1, 0).is_none());
    assert!(CivilDate::new(i32::MAX, 1, 1).is_none());
    assert_eq!(
        date(2024, 1, 31).checked_add_months(1),
        Some(date(2024, 2, 29))
    );
    assert_eq!(
        date(2023, 1, 31).checked_add_months(1),
        Some(date(2023, 2, 28))
    );
    assert_eq!(
        date(2024, 3, 31).checked_add_months(-1),
        Some(date(2024, 2, 29))
    );
    assert_eq!(
        date(2024, 2, 29).checked_add_months(12),
        Some(date(2025, 2, 28))
    );
    assert_eq!(
        date(2024, 2, 29).checked_add_months(-12),
        Some(date(2023, 2, 28))
    );
    let ordinary = date(2024, 1, 31);
    assert_eq!(ordinary.checked_add_months(0), Some(ordinary));
    assert_eq!(ordinary.checked_add_months(i32::MIN), None);
    assert_eq!(ordinary.checked_add_months(i32::MAX), None);
    assert_eq!(ordinary.to_string(), "2024-01-31");
    assert_eq!(date(-1, 1, 1).to_string(), "-0001-01-01");
    assert_eq!(date(30827, 1, 1).to_string(), "+30827-01-01");
}

#[test]
fn enclosing_grid_has_four_five_or_six_complete_weeks() {
    for (month, rows) in [(2, 4), (4, 5), (5, 6)] {
        let projection =
            CalendarState::new(snapshot(date(2021, month, 12), WeekStart::Monday)).projection();
        assert_eq!(projection.days.len(), rows * 7);
        assert_eq!(projection.title, format!("Month {month} 2021"));
        assert_eq!(projection.days.iter().filter(|day| day.today).count(), 1);
        assert_eq!(projection.days.iter().filter(|day| day.selected).count(), 1);
        assert_eq!(
            projection.days.iter().filter(|day| !day.off_month).count(),
            if month == 2 {
                28
            } else if month == 4 {
                30
            } else {
                31
            }
        );
    }
}

#[test]
fn all_week_starts_rotate_native_names_and_align_contiguous_days() {
    for (index, start) in STARTS.into_iter().enumerate() {
        let projection = CalendarState::new(snapshot(date(2021, 5, 12), start)).projection();
        assert_eq!(
            projection.weekdays,
            std::array::from_fn(|offset| format!("W{}", (index + offset) % 7 + 1))
        );
        let first = projection.days[0].date.expect("ordinary first date");
        let last = projection
            .days
            .last()
            .expect("weeks")
            .date
            .expect("ordinary last date");
        assert_eq!(first.0.weekday().num_days_from_monday() as usize, index);
        assert_eq!(
            last.0.weekday().num_days_from_monday() as usize,
            (index + 6) % 7
        );
        assert!((28..=42).contains(&projection.days.len()));
        assert_eq!(projection.days.len() % 7, 0);
        for pair in projection.days.windows(2) {
            assert_eq!(
                pair[0].date.expect("day").0.succ_opt(),
                Some(pair[1].date.expect("day").0)
            );
        }
        let today = projection.days.iter().find(|day| day.today).expect("today");
        assert_eq!(today.label, "12");
        assert_eq!(today.description, "Weekday 3 2021-05-12");
    }
}

#[test]
fn only_visible_month_days_can_select_and_off_month_selection_moves_display() {
    let mut state = CalendarState::new(snapshot(date(2024, 5, 15), WeekStart::Monday));
    assert!(!state.select_day(date(2024, 7, 1)));
    assert_eq!(state.selected(), date(2024, 5, 15));
    assert!(state.select_day(date(2024, 4, 29)));
    assert_eq!(state.displayed(), date(2024, 4, 29));
    assert_eq!(state.selected(), date(2024, 4, 29));
    assert_eq!(state.projection().title, "Month 4 2024");
    state.toggle_view();
    assert!(!state.select_day(date(2024, 4, 29)));
}

#[test]
fn navigation_and_year_month_selection_do_not_change_selection() {
    let original = date(2024, 2, 29);
    let mut state = CalendarState::new(snapshot(original, WeekStart::Sunday));
    assert!(!state.select_month(1));
    state.toggle_view();
    let year = state.projection();
    assert_eq!(year.title, "2024");
    assert!(year.days.is_empty());
    assert_eq!(year.months.len(), 12);
    assert_eq!(
        year.months
            .iter()
            .filter(|month| month.current)
            .map(|month| month.month)
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert!(state.navigate(CalendarDirection::Next));
    assert_eq!(state.displayed(), date(2025, 2, 28));
    assert_eq!(state.selected(), original);
    assert!(state.projection().months.iter().all(|month| !month.current));
    assert!(!state.select_month(0));
    assert!(!state.select_month(13));
    assert_eq!(state.view(), CalendarView::Year);
    assert!(state.select_month(12));
    assert_eq!(state.displayed(), date(2025, 12, 1));
    assert_eq!(state.selected(), original);
    assert_eq!(state.view(), CalendarView::Month);
    assert!(state.navigate(CalendarDirection::Previous));
    assert_eq!(state.displayed(), date(2025, 11, 1));
    assert_eq!(state.selected(), original);
}

#[test]
fn fresh_day_preserves_browsing_and_today_resets_both_dates_without_changing_mode() {
    let mut state = CalendarState::new(snapshot(date(2024, 5, 31), WeekStart::Monday));
    assert!(state.select_day(date(2024, 5, 30)));
    state.navigate(CalendarDirection::Previous);
    state.refresh(snapshot(date(2024, 6, 1), WeekStart::Monday));
    assert_eq!(state.displayed(), date(2024, 4, 30));
    assert_eq!(state.selected(), date(2024, 5, 30));
    assert!(state.projection().days.iter().all(|day| !day.today));
    state.toggle_view();
    state.today();
    assert_eq!(state.view(), CalendarView::Year);
    assert_eq!(state.displayed(), date(2024, 6, 1));
    assert_eq!(state.selected(), date(2024, 6, 1));
    state.toggle_view();
    state.navigate(CalendarDirection::Next);
    state.today();
    assert_eq!(state.view(), CalendarView::Month);
    assert_eq!(state.displayed(), date(2024, 6, 1));
}

#[test]
fn genuine_locale_metadata_changes_reset_dates_and_preserve_view() {
    let today = date(2024, 5, 15);
    for change in 0..5 {
        let mut state = CalendarState::new(snapshot(today, WeekStart::Monday));
        state.navigate(CalendarDirection::Next);
        state.toggle_view();
        let mut changed = snapshot(date(2024, 5, 16), WeekStart::Monday);
        match change {
            0 => changed.locale_name = "other-locale".into(),
            1 => changed.week_start = WeekStart::Thursday,
            2 => changed.months[0] = "Different full month".into(),
            3 => changed.weekdays_full[0] = "Different full weekday".into(),
            _ => changed.weekdays_abbreviated[0] = "Different abbreviation".into(),
        }
        state.refresh(changed);
        assert_eq!(state.displayed(), date(2024, 5, 16));
        assert_eq!(state.selected(), date(2024, 5, 16));
        assert_eq!(state.view(), CalendarView::Year);
    }
}

#[test]
fn full_chrono_bounds_have_disabled_enclosing_placeholders_and_checked_navigation() {
    for bound in [CivilDate(NaiveDate::MIN), CivilDate(NaiveDate::MAX)] {
        let forbidden = if bound.0 == NaiveDate::MIN {
            CalendarDirection::Previous
        } else {
            CalendarDirection::Next
        };
        let mut found_placeholder = false;
        for start in STARTS {
            let mut state = CalendarState::new(snapshot(bound, start));
            let projection = state.projection();
            assert!((28..=42).contains(&projection.days.len()));
            assert_eq!(projection.days.len() % 7, 0);
            assert_eq!(projection.days.iter().filter(|day| day.today).count(), 1);
            for day in &projection.days {
                if day.date.is_none() {
                    found_placeholder = true;
                    assert!(day.label.is_empty());
                    assert!(day.description.is_empty());
                    assert!(!day.off_month && !day.today && !day.selected);
                }
            }
            assert!(!state.navigate(forbidden));
            assert_eq!(state.displayed(), bound);
            assert_eq!(projection.can_previous, bound.0 != NaiveDate::MIN);
            assert_eq!(projection.can_next, bound.0 != NaiveDate::MAX);
            state.toggle_view();
            assert!(!state.navigate(forbidden));
            for month in 1..=12 {
                assert!(state.select_month(month));
                assert_eq!(state.displayed().day(), 1);
                assert_eq!(state.selected(), bound);
                state.toggle_view();
            }
        }
        assert!(found_placeholder);
    }
}

#[test]
fn metadata_validation_is_utf16_bounded_rejects_controls_and_preserves_unicode_rtl() {
    let original = snapshot(date(2024, 1, 1), WeekStart::Monday);
    let construct = |value: &CalendarSnapshot| {
        CalendarSnapshot::new(
            value.today,
            value.locale_name.clone(),
            value.months.clone(),
            value.weekdays_full.clone(),
            value.weekdays_abbreviated.clone(),
            value.week_start,
        )
    };
    for invalid in [
        "",
        "   ",
        "line\nbreak",
        "embedded\0nul",
        "\u{7f}",
        "\u{85}",
    ] {
        let mut value = original.clone();
        value.months[0] = invalid.into();
        assert_eq!(
            construct(&value).expect_err("bad label").kind,
            CalendarErrorKind::InvalidData
        );
        value = original.clone();
        value.locale_name = invalid.into();
        assert_eq!(
            construct(&value).expect_err("bad locale").kind,
            CalendarErrorKind::InvalidData
        );
    }
    let mut value = original.clone();
    value.locale_name = "a".repeat(84);
    value.months[0] = "𐀀".repeat(128);
    value.weekdays_full[0] = "\u{200f}الاثنين".into();
    value.weekdays_abbreviated[0] = "月".into();
    let valid = construct(&value).expect("bounded Unicode metadata");
    assert_eq!(valid.today(), original.today());
    assert_eq!(valid.locale_name(), value.locale_name);
    assert_eq!(valid.months(), &value.months);
    assert_eq!(valid.weekdays_full(), &value.weekdays_full);
    assert_eq!(valid.weekdays_abbreviated(), &value.weekdays_abbreviated);
    assert_eq!(valid.week_start(), WeekStart::Monday);
    value.locale_name.push('a');
    assert!(construct(&value).is_err());
    value.locale_name = original.locale_name.clone();
    value.months[0].push('a');
    assert!(construct(&value).is_err());
    value = original.clone();
    value.weekdays_full[6] = "x".repeat(257);
    assert!(construct(&value).is_err());
    value = original.clone();
    value.weekdays_abbreviated[6] = "x\ty".into();
    assert!(construct(&value).is_err());
}

#[test]
fn week_start_setter_changes_only_alignment_preserving_browse_selection_and_view() {
    for view in [CalendarView::Month, CalendarView::Year] {
        let mut state = CalendarState::new(snapshot(date(2024, 2, 29), WeekStart::Monday));
        assert!(state.select_day(date(2024, 2, 28)));
        assert!(state.navigate(CalendarDirection::Next));
        if view == CalendarView::Year {
            state.toggle_view();
        }
        let displayed = state.displayed();
        let selected = state.selected();
        let original = state.snapshot.clone();
        for (index, start) in STARTS.into_iter().enumerate() {
            state.set_week_start(start);
            let mut expected = original.clone();
            expected.week_start = start;
            assert_eq!(
                state.snapshot, expected,
                "all other native metadata is retained"
            );
            assert_eq!(state.displayed(), displayed);
            assert_eq!(state.selected(), selected);
            assert_eq!(state.view(), view);
            assert_eq!(
                state.projection().weekdays,
                std::array::from_fn(|offset| format!("W{}", (index + offset) % 7 + 1)),
            );
            let projected = state.projection();
            state.set_week_start(start);
            assert_eq!(
                state.projection(),
                projected,
                "same alignment is idempotent"
            );
        }
    }
}
