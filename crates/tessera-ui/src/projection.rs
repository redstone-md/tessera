// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pure projection of a full [`PanelSnapshot`] into display rows.
//!
//! Filtering must never change the opaque row key a row activates: the key is
//! carried alongside the caption and re-read on activation, so search can
//! only hide rows, never renumber them.

use crate::{PanelApplication, PanelSnapshot, PanelWindow, sanitize};

/// One display row: cleaned caption plus the opaque activation key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowProjection {
    pub(crate) key: String,
    pub(crate) caption: String,
    pub(crate) minimized: bool,
}

/// One launcher row: cleaned application name plus the opaque launch key,
/// whether the key is currently pinned, and the icon pixels to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppProjection {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) pinned: bool,
    pub(crate) icon: Option<crate::PixelIcon>,
}

/// Maximum number of application rows shown at once.
pub(crate) const MAX_APPS: usize = 64;

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

/// Projects the capped catalog into launcher rows, filtered case-insensitively
/// over **raw** names (empty search matches everything). Display labels are
/// sanitized here; search never sees the sanitized form, and keys are copied
/// verbatim — never re-derived from truncated labels.
pub(crate) fn project_apps(
    applications: &[PanelApplication],
    pins: &[String],
    search: &str,
) -> Vec<AppProjection> {
    let needle = search.trim().to_lowercase();
    applications
        .iter()
        .filter(|application| {
            needle.is_empty() || application.title().to_lowercase().contains(&needle)
        })
        .take(MAX_APPS)
        .map(|application| project_app(application, pins))
        .collect()
}

/// Resolves exact favorite identities against the full trusted catalog, in
/// preference order. Unavailable identities produce no synthetic launch rows;
/// the display cap applies only after resolution. Favorites have no query.
pub(crate) fn project_favorite_apps(
    applications: &[PanelApplication],
    favorites: &[String],
    pins: &[String],
) -> Vec<AppProjection> {
    favorites
        .iter()
        .filter_map(|favorite| applications.iter().find(|app| app.key() == favorite))
        .take(MAX_APPS)
        .map(|application| project_app(application, pins))
        .collect()
}

fn project_app(application: &PanelApplication, pins: &[String]) -> AppProjection {
    AppProjection {
        key: application.key().to_string(),
        label: sanitize::caption(application.title()),
        pinned: pins.iter().any(|pin| pin == application.key()),
        icon: application.icon().cloned(),
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
    use crate::PixelIcon;

    fn app(key: &str, title: &str) -> PanelApplication {
        PanelApplication::new(key.to_string(), title.to_string(), None).unwrap()
    }

    #[test]
    fn apps_search_matches_raw_names_but_labels_are_sanitized() {
        let catalog = vec![app("a", "Rust \u{202e}Editor"), app("b", "Web Browser")];
        let rows = project_apps(&catalog, &[], "EDITOR");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key, "a");
        assert_eq!(rows[0].label, "Rust Editor");
        assert!(project_apps(&catalog, &[], "no match").is_empty());
    }

    #[test]
    fn apps_rows_are_capped_and_pins_are_reported() {
        let catalog: Vec<_> = (0..MAX_APPS + 10)
            .map(|index| app(&format!("k{index}"), &format!("App {index}")))
            .collect();
        let rows = project_apps(&catalog, &["k0".to_string()], "");
        assert_eq!(rows.len(), MAX_APPS);
        assert!(rows[0].pinned);
        assert!(!rows[1].pinned);
    }

    #[test]
    fn favorites_resolve_full_catalog_in_saved_order_and_report_only_dock_pins() {
        let mut catalog: Vec<_> = (0..80)
            .map(|index| app(&format!("k{index}"), &format!("App {index}")))
            .collect();
        let favorites = vec!["missing".into(), "k79".into(), "k2".into()];
        let rows = project_favorite_apps(&catalog, &favorites, &["k2".into()]);
        assert_eq!(
            rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
            ["k79", "k2"]
        );
        assert!(!rows[0].pinned);
        assert!(rows[1].pinned);
        assert_eq!(project_apps(&catalog, &[], "")[0].key, "k0");
        catalog.push(app("missing", "Reinstalled"));
        let restored = project_favorite_apps(&catalog, &favorites, &[]);
        assert_eq!(restored[0].key, "missing");
        assert_eq!(restored[1].key, "k79");
        assert!(project_favorite_apps(&[], &favorites, &[]).is_empty());
        assert_eq!(favorites, ["missing", "k79", "k2"]);
    }

    #[test]
    fn favorite_cap_follows_resolution_without_losing_preference_tail() {
        let catalog: Vec<_> = (0..80)
            .map(|index| app(&format!("k{index}"), &format!("App {index}")))
            .collect();
        let mut favorites: Vec<_> = (0..70).map(|index| format!("missing-{index}")).collect();
        favorites.extend((0..80).rev().map(|index| format!("k{index}")));
        let rows = project_favorite_apps(&catalog, &favorites, &[]);
        assert_eq!(rows.len(), MAX_APPS);
        assert_eq!(rows[0].key, "k79");
        assert_eq!(rows[MAX_APPS - 1].key, "k16");
        assert_eq!(favorites.len(), 150);
        assert_eq!(favorites.last().unwrap(), "k0");
    }

    #[test]
    fn icon_dto_validation_rejects_bad_shapes() {
        assert!(PixelIcon::new(0, 4, vec![0; 16]).is_none());
        assert!(PixelIcon::new(129, 1, vec![0; 129 * 4]).is_none());
        assert!(PixelIcon::new(2, 2, vec![0; 15]).is_none());
        assert!(PixelIcon::new(2, 2, vec![0; 16]).is_some());
        assert!(PanelApplication::new(String::new(), "x".into(), None).is_none());
    }

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
