// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What went wrong while the shell ran. A shell started from the desktop
//! has no console, so what is reported is kept for the `problems` widgets
//! to show, besides being written to the standard error.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// How many reports are kept; older ones are forgotten.
const KEPT: usize = 50;

/// Reports that something went wrong, with the arguments of `format!`.
macro_rules! report {
    ($($argument:tt)*) => {
        $crate::report::report(format!($($argument)*))
    };
}

/// One thing that went wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub text: String,
    /// The time of day it last happened at, as hours and minutes.
    pub at: String,
    /// How many times it happened.
    pub times: u32,
}

static REPORTS: Mutex<VecDeque<Report>> = Mutex::new(VecDeque::new());

/// Counts every change to the reports, so that a reader can tell cheaply
/// whether it has seen the latest.
static CHANGES: AtomicU64 = AtomicU64::new(0);

/// Keeps `text` as something that went wrong and writes it to the standard
/// error.
pub fn report(text: String) {
    eprintln!("tessera: {text}");
    let at = chrono::Local::now().format("%H:%M").to_string();
    let mut reports = REPORTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    add(&mut reports, text, at);
    drop(reports);
    CHANGES.fetch_add(1, Ordering::Relaxed);
}

/// Adds a report to `reports`: one that says what an earlier one said is
/// counted with it and becomes the latest, and only the latest [`KEPT`]
/// stay.
fn add(reports: &mut VecDeque<Report>, text: String, at: String) {
    let earlier = reports.iter().position(|report| report.text == text);
    let times = match earlier.and_then(|index| reports.remove(index)) {
        Some(earlier) => earlier.times.saturating_add(1),
        None => 1,
    };
    if reports.len() == KEPT {
        reports.pop_front();
    }
    reports.push_back(Report { text, at, times });
}

/// A number that differs after every change to the reports.
pub fn changes() -> u64 {
    CHANGES.load(Ordering::Relaxed)
}

/// What is kept, the oldest first.
pub fn reports() -> Vec<Report> {
    let reports = REPORTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reports.iter().cloned().collect()
}

/// Forgets everything kept.
pub fn clear() {
    let mut reports = REPORTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reports.clear();
    drop(reports);
    CHANGES.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_report_is_counted_and_old_ones_are_forgotten() {
        let mut reports = VecDeque::new();
        add(&mut reports, "one".to_owned(), "10:00".to_owned());
        add(&mut reports, "two".to_owned(), "10:01".to_owned());
        add(&mut reports, "one".to_owned(), "10:05".to_owned());
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].text, "two");
        assert_eq!((reports[1].times, reports[1].at.as_str()), (2, "10:05"));
        for index in 0..KEPT {
            add(&mut reports, format!("other {index}"), "11:00".to_owned());
        }
        assert_eq!(reports.len(), KEPT);
        assert_eq!(reports[0].text, "other 0");
        assert_eq!(reports[KEPT - 1].text, format!("other {}", KEPT - 1));
    }
}
