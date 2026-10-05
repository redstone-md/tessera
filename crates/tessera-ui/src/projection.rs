// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pure projection of a full [`PanelSnapshot`] into display rows.
//!
//! Filtering must never change the opaque row key a row activates: the key is
//! carried alongside the caption and re-read on activation, so search can
//! only hide rows, never renumber them.

use crate::{PanelSnapshot, PanelWindow, sanitize};

/// One display row: cleaned caption plus the opaque activation key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowProjection {
    pub(crate) key: String,
    pub(crate) caption: String,
    pub(crate) minimized: bool,
}

/// What the panel renders for a snapshot under the current search filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PanelProjection {
    pub(crate) rows: Vec<RowProjection>,
    pub(crate) summary: String,
    pub(crate) status: String,
    pub(crate) shown: usize,
    pub(crate) total: usize,
    pub(crate) truncated: bool,
}

/// Maximum number of rows shown at once.
pub(crate) const MAX_ROWS: usize = 128;

/// Projects `snapshot` for display, filtered by a case-insensitive `search`
/// over titles (empty search matches everything).
pub(crate) fn project(snapshot: &PanelSnapshot, search: &str) -> PanelProjection {
    let total = snapshot.windows().len();
    let needle = search.trim().to_lowercase();
    let mut truncated = false;
    let rows: Vec<RowProjection> = snapshot
        .windows()
        .iter()
        .filter(|window| needle.is_empty() || window.title().to_lowercase().contains(&needle))
        .take(MAX_ROWS)
        .map(project_row)
        .collect();
    if total > rows.len() {
        truncated = true;
    }
    let shown = rows.len();
    PanelProjection {
        summary: format!(
            "Monitors: {} | Windows: {} | Warnings: {}",
            snapshot.monitor_count(),
            total,
            snapshot.warning_count()
        ),
        status: if total == 0 {
            "No visible windows observed".to_string()
        } else if truncated {
            format!("Showing {shown} of {total} matching windows (limit {MAX_ROWS})")
        } else if needle.is_empty() {
            format!("Showing all {total} windows; click one to activate, refresh to update")
        } else {
            format!("Showing {shown} of {total} windows matching the search")
        },
        rows,
        shown,
        total,
        truncated,
    }
}

fn project_row(window: &PanelWindow) -> RowProjection {
    RowProjection {
        key: window.key().to_string(),
        caption: caption_with_state(window),
        minimized: window.minimized(),
    }
}

/// Cleaned title, never empty, with a minimized marker for clarity.
fn caption_with_state(window: &PanelWindow) -> String {
    let caption = sanitize::caption(window.title());
    if window.minimized() {
        format!("{caption} (minimized)")
    } else {
        caption
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(titles: &[(&str, &str, bool)]) -> PanelSnapshot {
        PanelSnapshot::new(
            2,
            titles
                .iter()
                .map(|(key, title, minimized)| {
                    PanelWindow::new((*key).into(), (*title).into(), *minimized)
                })
                .collect(),
            0,
        )
    }

    #[test]
    fn search_filters_by_case_insensitive_title_but_keeps_opaque_keys() {
        let snapshot = snapshot(&[("k2", "Rust Editor", false), ("k1", "Browser", false)]);
        let filtered = project(&snapshot, "RUST");
        assert_eq!(filtered.rows.len(), 1);
        // The surviving row still activates its own key, not row 0's identity.
        assert_eq!(filtered.rows[0].key, "k2");
        assert_eq!(filtered.rows[0].caption, "Rust Editor");

        let unordered = project(&snapshot, "browser");
        assert_eq!(unordered.rows[0].key, "k1");

        let none = project(&snapshot, "no match anywhere");
        assert!(none.rows.is_empty());
        assert!(none.status.contains("0 of 2"));
        assert_eq!(none.total, 2);
    }

    #[test]
    fn rows_are_capped_and_counts_stay_explicit() {
        let many = PanelSnapshot::new(
            2,
            (0..MAX_ROWS + 30)
                .map(|index| PanelWindow::new(index.to_string(), String::new(), false))
                .collect(),
            0,
        );
        let projection = project(&many, "");
        assert_eq!(projection.rows.len(), MAX_ROWS);
        assert_eq!(projection.shown, MAX_ROWS);
        assert_eq!(projection.total, MAX_ROWS + 30);
        assert!(projection.truncated);
        assert!(projection.status.contains("128 of 158"));
    }

    #[test]
    fn minimized_rows_are_marked_and_empty_titles_fallback() {
        let projection = project(&snapshot(&[("k", "", true)]), "");
        assert_eq!(projection.rows[0].caption, "(untitled window) (minimized)");
        assert!(projection.rows[0].minimized);
        assert!(!projection.truncated);
    }
}
