// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The built-in rows of the settings, written as TOML, one file per part of
//! Tessera, so that every part declares the settings of its own keys and
//! the titles of its own actions.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::options::{Label, OptionDecl};

/// One `[[card]]` of a rows file.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RowCard {
    /// The English title of the section it stands in (see [`SECTIONS`]).
    pub section: String,
    /// The English title of the card. Cards with one title in one section
    /// are one card.
    pub title: String,
    /// The table its rows write keys of, as `tiling` or `snap`; none for
    /// keys at the top of the file.
    #[serde(default)]
    pub table: Option<String>,
    /// Instead of a table: its rows set an option of every widget of this
    /// name.
    #[serde(default)]
    pub widget: Option<String>,
    /// Settings whose `key` is a dotted path below `table`, as
    /// `border.width`.
    #[serde(default, rename = "row")]
    pub rows: Vec<OptionDecl>,
    /// Rows that run an action instead of writing a key.
    #[serde(default, rename = "link")]
    pub links: Vec<LinkRow>,
    /// The name of a widget drawn inside the card as a page of the
    /// settings, shown only where that widget is registered.
    #[serde(default)]
    pub page: Option<String>,
    /// More English words search finds the card by.
    #[serde(default)]
    pub search: Vec<String>,
}

/// A row of a card that runs an action when it is clicked.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LinkRow {
    pub label: Label,
    #[serde(default)]
    pub description: Option<Label>,
    /// The name of the action it runs.
    pub action: String,
    /// The argument it runs the action with.
    #[serde(default)]
    pub arg: Option<String>,
}

/// One `[[action]]` of a rows file: what the Shortcuts page and the
/// choosers of actions call an action.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "read by the Shortcuts page of shortcuts-editor")]
pub(crate) struct ActionTitle {
    /// The name the action is registered under.
    pub name: String,
    /// Its English title, a text of the RUSSIAN table.
    pub title: String,
    /// What its argument is, in English, where it takes one.
    #[serde(default)]
    pub arg: Option<String>,
}

/// What one rows file holds.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RowFile {
    #[serde(default)]
    pub card: Vec<RowCard>,
    #[serde(default)]
    pub action: Vec<ActionTitle>,
}

/// The sections of the settings, by English title, in the order of the
/// rail.
pub(super) const SECTIONS: &[&str] = &[
    "General",
    "Bars",
    "Desktop",
    "Wallpaper",
    "Widgets",
    "Dock",
    "Tray",
    "Notifications",
    "Windows",
    "Window rules",
    "Screen edges",
    "Workspaces",
    "Keys",
    "Search",
    "Files",
    "Clipboard and screenshots",
    "Displays and power",
    "Session",
    "Themes",
    "Extensions",
    "About",
];

/// Every rows file, by the package that fills it. Lines are only added.
pub(super) const ROW_FILES: &[(&str, &str)] = &[
    ("core", include_str!("rows/core.toml")),
    ("widget-options", include_str!("rows/widget-options.toml")),
    (
        "shortcuts-editor",
        include_str!("rows/shortcuts-editor.toml"),
    ),
    (
        "window-rules-editor",
        include_str!("rows/window-rules-editor.toml"),
    ),
    ("tiling-stacks", include_str!("rows/tiling-stacks.toml")),
    ("edges-snap", include_str!("rows/edges-snap.toml")),
    ("dock-pins", include_str!("rows/dock-pins.toml")),
    ("files-namespace", include_str!("rows/files-namespace.toml")),
    (
        "files-search-actions",
        include_str!("rows/files-search-actions.toml"),
    ),
    ("files-terminal", include_str!("rows/files-terminal.toml")),
    ("shell-services", include_str!("rows/shell-services.toml")),
    (
        "taskbar-fidelity",
        include_str!("rows/taskbar-fidelity.toml"),
    ),
    (
        "spectacle-klipper",
        include_str!("rows/spectacle-klipper.toml"),
    ),
    (
        "notifications-rich",
        include_str!("rows/notifications-rich.toml"),
    ),
    ("theme-styles", include_str!("rows/theme-styles.toml")),
    (
        "desktop-fidelity",
        include_str!("rows/desktop-fidelity.toml"),
    ),
    ("explorer-shim", include_str!("rows/explorer-shim.toml")),
    (
        "windows-selftest",
        include_str!("rows/windows-selftest.toml"),
    ),
];

/// The rows files that parse, in the order of [`ROW_FILES`]; a file that
/// does not parse is reported, once, and left out.
fn files() -> &'static [RowFile] {
    static FILES: OnceLock<Vec<RowFile>> = OnceLock::new();
    FILES.get_or_init(|| {
        { ROW_FILES.iter() }
            .filter_map(|(package, text)| match toml::from_str::<RowFile>(text) {
                Ok(file) => Some(file),
                Err(error) => {
                    report!("the rows of the settings of {package} are not read: {error}");
                    None
                }
            })
            .collect()
    })
}

/// The cards of every rows file, in file order; a file that does not parse
/// is reported and skipped.
pub(super) fn cards() -> Vec<RowCard> {
    { files().iter() }
        .flat_map(|file| file.card.iter().cloned())
        .collect()
}

/// The `[[action]]` titles of every rows file, parsed once and kept.
#[allow(dead_code, reason = "read by the Shortcuts page of shortcuts-editor")]
pub(crate) fn action_titles() -> &'static [ActionTitle] {
    static TITLES: OnceLock<Vec<ActionTitle>> = OnceLock::new();
    TITLES.get_or_init(|| {
        { files().iter() }
            .flat_map(|file| file.action.iter().cloned())
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_rows_file_parses() {
        let mut actions = HashSet::new();
        for (package, text) in ROW_FILES {
            let file: RowFile = toml::from_str(text)
                .unwrap_or_else(|error| panic!("the rows of {package} do not parse: {error}"));
            for card in &file.card {
                assert!(
                    SECTIONS.contains(&card.section.as_str()),
                    "{package}: the card `{}` stands in `{}`, which is no section",
                    card.title,
                    card.section
                );
                assert!(
                    card.table.is_none() || card.widget.is_none(),
                    "{package}: the card `{}` names both a table and a widget",
                    card.title
                );
                for row in &card.rows {
                    row.check()
                        .unwrap_or_else(|error| panic!("{package}: `{}` {error}", row.key));
                }
            }
            for action in &file.action {
                assert!(
                    actions.insert(action.name.clone()),
                    "{package}: the action `{}` has a second [[action]] title",
                    action.name
                );
            }
        }
    }

    #[test]
    fn every_section_is_named_once() {
        let named: HashSet<&str> = SECTIONS.iter().copied().collect();
        assert_eq!(named.len(), SECTIONS.len());
    }
}
