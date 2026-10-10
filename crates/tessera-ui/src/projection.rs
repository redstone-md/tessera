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

/// Application metadata: cleaned name plus the opaque launch key, whether the
/// key is currently pinned, and retained icon pixels (not presentation images).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppProjection {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) pinned: bool,
    pub(crate) icon: Option<crate::PixelIcon>,
}

/// Maximum number of application rows shown by the eager recovery panel.
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

/// Projects at most [`MAX_APPS`] applications for the eager recovery panel.
/// Matching and metadata are identical to the complete launcher projection.
pub(crate) fn project_apps(
    applications: &[PanelApplication],
    pins: &[String],
    search: &str,
) -> Vec<AppProjection> {
    projected_apps(applications, pins, search)
        .take(MAX_APPS)
        .collect()
}

/// Projects the complete retained catalog in catalog order, filtered
/// case-insensitively over **raw** names (empty search matches everything).
/// Labels are sanitized; opaque keys are copied verbatim, never re-derived.
pub(crate) fn project_launcher_apps(
    applications: &[PanelApplication],
    pins: &[String],
    search: &str,
) -> Vec<AppProjection> {
    projected_apps(applications, pins, search).collect()
}

fn projected_apps<'a>(
    applications: &'a [PanelApplication],
    pins: &'a [String],
    search: &str,
) -> impl Iterator<Item = AppProjection> + 'a {
    let needle = search.trim().to_lowercase();
    applications
        .iter()
        .filter(move |application| {
            needle.is_empty() || application.title().to_lowercase().contains(&needle)
        })
        .map(move |application| project_app(application, pins))
}

/// Resolves exact favorite identities against the full trusted catalog, in
/// preference order. Unavailable identities produce no synthetic launch rows;
/// every resolved favorite is included. Favorites have no query.
pub(crate) fn project_favorite_apps(
    applications: &[PanelApplication],
    favorites: &[String],
    pins: &[String],
) -> Vec<AppProjection> {
    favorites
        .iter()
        .filter_map(|favorite| applications.iter().find(|app| app.key() == favorite))
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

/// Group only proven identities, retaining complete snapshot membership.
/// The representative remains an individual opaque window key.
pub(crate) fn dock_groups(snapshot: &PanelSnapshot) -> Vec<Vec<&PanelWindow>> {
    let mut groups: Vec<Vec<&PanelWindow>> = Vec::new();
    for window in snapshot.windows() {
        let group = window.application_identity().and_then(|identity| {
            groups
                .iter_mut()
                .find(|members| members[0].application_identity() == Some(identity))
        });
        if let Some(group) = group {
            group.push(window);
        } else {
            groups.push(vec![window]);
        }
    }
    groups
}

/// AppsFolder may return either its relative AUMID or the canonical namespace
/// parsing name. Strip only that exact known namespace; never infer from names.
pub(crate) fn catalog_aumid(key: &str) -> &str {
    const APPS_FOLDER: &str = "::{4234d49b-0245-4df3-b780-3893943456e1}\\";
    if key
        .get(..APPS_FOLDER.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(APPS_FOLDER))
    {
        &key[APPS_FOLDER.len()..]
    } else {
        key
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
    fn launcher_projects_complete_catalog_order_and_preserves_metadata() {
        let icon = PixelIcon::new(1, 1, vec![1, 2, 3, 255]).unwrap();
        let mut catalog: Vec<_> = (0..1024)
            .map(|index| app(&format!("opaque:{index}"), &format!("App {index}")))
            .collect();
        catalog[1023] = PanelApplication::new(
            "Exact:Tail/身份".into(),
            "  Tail \u{202e}App  ".into(),
            Some(icon.clone()),
        )
        .unwrap();
        let rows = project_launcher_apps(&catalog, &["Exact:Tail/身份".into()], "");
        assert_eq!(rows.len(), catalog.len());
        assert_eq!(
            rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
            catalog
                .iter()
                .map(PanelApplication::key)
                .collect::<Vec<_>>()
        );
        let tail = rows.last().unwrap();
        assert_eq!(tail.key, "Exact:Tail/身份");
        assert_eq!(tail.label, "Tail App");
        assert!(tail.pinned);
        assert_eq!(tail.icon.as_ref(), Some(&icon));
        assert_eq!(project_apps(&catalog, &[], "").len(), MAX_APPS);
    }

    #[test]
    fn launcher_search_includes_every_raw_title_match_beyond_the_recovery_cap() {
        let mut catalog: Vec<_> = (0..MAX_APPS)
            .map(|index| app(&format!("earlier:{index}"), "Unrelated"))
            .collect();
        let raw_title = format!("{}\u{202e}TailMatch", "x".repeat(140));
        catalog.extend((0..80).map(|index| app(&format!("match:{index}"), &raw_title)));
        let rows = project_launcher_apps(&catalog, &[], " TAILMATCH ");
        assert_eq!(rows.len(), 80);
        assert_eq!(rows.first().unwrap().key, "match:0");
        assert_eq!(rows.last().unwrap().key, "match:79");
        let expected_label = format!("{}…", "x".repeat(128));
        assert!(rows.iter().all(|row| row.label == expected_label));
        assert!(project_launcher_apps(&catalog, &[], "no match").is_empty());
        let recovery = project_apps(&catalog, &[], "tailmatch");
        assert_eq!(recovery, rows[..MAX_APPS]);
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
    fn favorites_resolve_complete_preference_tail_without_mutating_saved_keys() {
        let catalog: Vec<_> = (0..80)
            .map(|index| app(&format!("k{index}"), &format!("App {index}")))
            .collect();
        let mut favorites: Vec<_> = (0..70).map(|index| format!("missing-{index}")).collect();
        favorites.extend((0..80).rev().map(|index| format!("k{index}")));
        let rows = project_favorite_apps(&catalog, &favorites, &[]);
        assert_eq!(rows.len(), catalog.len());
        assert_eq!(
            rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
            catalog
                .iter()
                .rev()
                .map(PanelApplication::key)
                .collect::<Vec<_>>()
        );
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
