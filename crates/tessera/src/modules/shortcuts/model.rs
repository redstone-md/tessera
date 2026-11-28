// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The rows of the Shortcuts page, worked out from the `[[key]]` entries
//! of the user and the keys the actions bring: one row for each thing a
//! key does, with every key that does it as a chip, what became of each,
//! and the other rows whose keys clash with it.

use crate::config::KeyConfig;
use crate::keys::{Gesture, KeyState, NO_ACTION, written_press};

/// The parts of Tessera the rows are grouped by, in the order they are
/// shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    WindowsKeys,
    Windows,
    Workspaces,
    Dock,
    Launcher,
    Files,
    Screenshots,
    Extensions,
    Yours,
}

/// The actions of each group but the keys of Windows, where the rest go.
const GROUPED: [(Group, &[&str]); 6] = [
    (
        Group::Windows,
        &[
            "snap",
            "snap-display",
            "snap-layouts",
            "window-menu",
            "kill-window",
            "tiling",
            "switcher",
            "taskview",
            "present-windows",
            "activate-window",
            "window-height",
            "window-half",
            "edit-window-rule",
            "edit-zones",
        ],
    ),
    (Group::Workspaces, &["workspace", "send-window", "sticky"]),
    (
        Group::Dock,
        &[
            "dock",
            "dock-new",
            "dock-cycle",
            "dock-menu",
            "dock-focus",
            "pin",
            "unpin",
            "pin-verb",
            "import-taskbar-pins",
        ],
    ),
    (
        Group::Launcher,
        &[
            "popup",
            "runner",
            "run",
            "clipboard",
            "emoji-panel",
            "display-settings",
            "power-settings",
            "wallpaper-settings",
            "bar-settings",
            "install-resource",
        ],
    ),
    (
        Group::Files,
        &[
            "files",
            "files-default",
            "share",
            "map-drive",
            "disconnect-drive",
            "add-network-location",
            "find-computers",
            "trash",
            "eject",
            "terminal-dropdown",
        ],
    ),
    (Group::Screenshots, &["screenshot", "clipboard-actions"]),
];

impl Group {
    pub const ALL: [Self; 9] = [
        Self::WindowsKeys,
        Self::Windows,
        Self::Workspaces,
        Self::Dock,
        Self::Launcher,
        Self::Files,
        Self::Screenshots,
        Self::Extensions,
        Self::Yours,
    ];

    /// What the page calls it, in English.
    pub fn title(self) -> &'static str {
        match self {
            Self::WindowsKeys => "Windows keys",
            Self::Windows => "Windows and tiling",
            Self::Workspaces => "Workspaces",
            Self::Dock => "Dock",
            Self::Launcher => "Launcher and popups",
            Self::Files => "Files",
            Self::Screenshots => "Screenshots",
            Self::Extensions => "Extensions",
            Self::Yours => "Your keys",
        }
    }

    /// The group of the action `name`; one no group names is a key of
    /// Windows, as most actions that bring keys are.
    pub fn of_action(name: &str) -> Self {
        if name.contains('.') || name == "extension-answer" {
            return Self::Extensions;
        }
        let found = GROUPED.iter().find(|(_, names)| names.contains(&name));
        found.map_or(Self::WindowsKeys, |(group, _)| *group)
    }
}

/// What became of a key of a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChipState {
    /// It is in use, and this became of it.
    InUse(KeyState),
    /// A key an action brings that gave way to the user's entry at this
    /// place, which presses the same keys and does something else.
    GivenWay(usize),
    /// A key an action brings, unused because `windows_keys` is off.
    DefaultsOff,
    /// A key an action brings that the user's entry at
    /// [`Chip::entry`] turned off with the action `none`.
    TurnedOff,
}

/// One key of a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    /// The press, as the configuration has it.
    pub press: String,
    /// The `[[key]]` of the user that holds it, or that turns it off.
    pub entry: Option<usize>,
    /// Whether an action brings it.
    pub default: bool,
    pub state: ChipState,
    /// The other rows with a key that clashes with this one.
    pub clashes: Vec<usize>,
}

impl Chip {
    /// Whether the key is pressed for its row: in use and doing something.
    fn pressed(&self) -> bool {
        matches!(&self.state, ChipState::InUse(state) if !matches!(state, KeyState::Off | KeyState::Broken(_)))
    }
}

/// One thing keys do, and the keys that do it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub group: Group,
    /// What the keys do: a `[[key]]` without its press.
    pub what: KeyConfig,
    pub chips: Vec<Chip>,
}

impl Row {
    /// The action the row does, where it does one.
    pub fn action(&self) -> Option<&str> {
        self.what.action.as_deref()
    }

    /// Whether an action brings a key of the row.
    pub fn has_default(&self) -> bool {
        self.chips.iter().any(|chip| chip.default)
    }
}

/// What `key` does, without the keys that do it.
pub fn what_of(key: &KeyConfig) -> KeyConfig {
    KeyConfig {
        press: String::new(),
        ..key.clone()
    }
}

/// Whether `key` turns off what is pressed with it.
fn turns_off(key: &KeyConfig) -> bool {
    key.action.as_deref() == Some(NO_ACTION)
}

/// The rows of the entries of the user, `user`, and of the keys the
/// actions bring, `defaults`, which are used where `defaults_on`;
/// `in_use` are the entries in use, each with what became of it.
pub fn rows(
    user: &[KeyConfig],
    defaults: &[KeyConfig],
    defaults_on: bool,
    in_use: &[(KeyConfig, KeyState)],
) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let pressed: Vec<Option<String>> = user.iter().map(|key| written_press(&key.press)).collect();
    // The entries of the user a chip of a default stands for already.
    let mut shown = vec![false; user.len()];
    let state_of = |key: &KeyConfig, from: usize| {
        { in_use.iter().skip(from) }
            .find(|(used, _)| used == key)
            .map_or(KeyState::Held, |(_, state)| state.clone())
    };
    for default in defaults {
        let press = written_press(&default.press);
        let same = {
            pressed
                .iter()
                .position(|other| press.is_some() && *other == press)
        };
        let (entry, state) = match same {
            Some(at) if turns_off(&user[at]) => (Some(at), ChipState::TurnedOff),
            Some(at) if what_of(&user[at]) == what_of(default) => {
                (Some(at), ChipState::InUse(state_of(&user[at], 0)))
            }
            Some(at) => (None, ChipState::GivenWay(at)),
            None if !defaults_on => (None, ChipState::DefaultsOff),
            None => (None, ChipState::InUse(state_of(default, user.len()))),
        };
        if let Some(at) = entry {
            shown[at] = true;
        }
        let chip = Chip {
            press: default.press.clone(),
            entry,
            default: true,
            state,
            clashes: Vec::new(),
        };
        let group = Group::of_action(default.action.as_deref().unwrap_or_default());
        add(&mut rows, group, what_of(default), chip);
    }
    for (at, key) in user.iter().enumerate() {
        if shown[at] {
            continue;
        }
        let chip = Chip {
            press: key.press.clone(),
            entry: Some(at),
            default: false,
            state: ChipState::InUse(state_of(key, 0)),
            clashes: Vec::new(),
        };
        // A key of the user that does what a row of the actions' keys
        // does joins that row; the others are the user's own.
        add(&mut rows, Group::Yours, what_of(key), chip);
    }
    mark_clashes(&mut rows);
    rows
}

/// Adds `chip` to the row that does `what`, made in `group` where there is
/// none yet.
fn add(rows: &mut Vec<Row>, group: Group, what: KeyConfig, chip: Chip) {
    match rows.iter_mut().find(|row| row.what == what) {
        Some(row) => row.chips.push(chip),
        None => rows.push(Row {
            group,
            what,
            chips: vec![chip],
        }),
    }
}

/// Marks each key that is pressed with the other rows whose pressed keys
/// clash with it: the same keys, or a sequence that begins with them.
fn mark_clashes(rows: &mut [Row]) {
    let mut pressed: Vec<(usize, usize, Gesture)> = Vec::new();
    for (row_at, row) in rows.iter().enumerate() {
        for (chip_at, chip) in row.chips.iter().enumerate() {
            if !chip.pressed() {
                continue;
            }
            if let Some(gesture) = Gesture::parse(&chip.press) {
                pressed.push((row_at, chip_at, gesture));
            }
        }
    }
    for (one, (row, chip, gesture)) in pressed.iter().enumerate() {
        let clashes = { pressed.iter().enumerate() }
            .filter(|(other, (other_row, ..))| *other != one && other_row != row)
            .filter(|(_, (.., other))| gesture.clashes(other))
            .map(|(_, (other_row, ..))| *other_row);
        let mut clashes: Vec<usize> = clashes.collect();
        clashes.dedup();
        rows[*row].chips[*chip].clashes = clashes;
    }
}

/// What the page lists, from the top: the title of each group that has a
/// row shown, and its rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Group(Group),
    Row(usize),
}

/// The lines of the rows of `order`, the places of the rows to show in
/// the order to show them, grouped as [`Group::ALL`] orders the groups.
pub fn lines(rows: &[Row], order: &[usize]) -> Vec<Line> {
    let mut lines = Vec::new();
    for group in Group::ALL {
        let mut of_group = { order.iter().copied() }
            .filter(|&at| rows.get(at).is_some_and(|row| row.group == group))
            .peekable();
        if of_group.peek().is_none() {
            continue;
        }
        lines.push(Line::Group(group));
        lines.extend(of_group.map(Line::Row));
    }
    lines
}

/// Whether every word of `query` is in one of `texts`, in any case; an
/// empty query finds everything.
pub fn found(query: &str, texts: &[&str]) -> bool {
    let texts: Vec<String> = texts.iter().map(|text| text.to_lowercase()).collect();
    let query = query.to_lowercase();
    { query.split_whitespace() }.all(|word| texts.iter().any(|text| text.contains(word)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acting(press: &str, action: &str, arg: Option<&str>) -> KeyConfig {
        KeyConfig {
            press: press.to_owned(),
            action: Some(action.to_owned()),
            arg: arg.map(str::to_owned),
            ..KeyConfig::default()
        }
    }

    fn defaults() -> Vec<KeyConfig> {
        vec![
            acting("win+d", "desktop", None),
            acting("win+m", "desktop", Some("minimize")),
            acting("win+left", "snap", Some("left")),
            acting("win+e", "files", None),
        ]
    }

    #[test]
    fn the_keys_the_actions_bring_are_rows_with_what_became_of_them() {
        let user = [
            acting("Win+D", NO_ACTION, None),
            acting("win+m", "popup", Some("launcher")),
            acting("ctrl+alt+left", "snap", Some("left")),
        ];
        let mut in_use: Vec<(KeyConfig, KeyState)> = user
            .iter()
            .cloned()
            .map(|key| (key, KeyState::Held))
            .collect();
        in_use[0].1 = KeyState::Off;
        in_use.push((defaults()[2].clone(), KeyState::Taken));
        in_use.push((defaults()[3].clone(), KeyState::Refused));
        let rows = rows(&user, &defaults(), true, &in_use);
        let desktop = &rows[0];
        assert_eq!(desktop.group, Group::WindowsKeys);
        assert_eq!(desktop.chips[0].state, ChipState::TurnedOff);
        assert_eq!(desktop.chips[0].entry, Some(0));
        let minimize = &rows[1];
        assert_eq!(minimize.chips[0].state, ChipState::GivenWay(1));
        let snap = &rows[2];
        assert_eq!(snap.group, Group::Windows);
        assert_eq!(snap.chips.len(), 2, "the user's key joins the row");
        assert_eq!(snap.chips[0].state, ChipState::InUse(KeyState::Taken));
        assert!(!snap.chips[1].default);
        assert_eq!(snap.chips[1].entry, Some(2));
        assert_eq!(rows[3].chips[0].state, ChipState::InUse(KeyState::Refused));
        let launcher = &rows[4];
        assert_eq!(launcher.group, Group::Yours);
        assert_eq!(launcher.what.arg.as_deref(), Some("launcher"));
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn without_the_keys_of_windows_the_keys_of_the_actions_are_off() {
        let rows = rows(&[], &defaults(), false, &[]);
        assert!(
            rows.iter()
                .flat_map(|row| &row.chips)
                .all(|chip| chip.state == ChipState::DefaultsOff)
        );
    }

    #[test]
    fn keys_that_clash_are_marked_with_the_other_row() {
        let user = [
            acting("ctrl+alt+k", "popup", Some("launcher")),
            acting("ctrl+alt+k, t", "popup", Some("calendar")),
            acting("ctrl+alt+j", "popup", Some("media")),
        ];
        let in_use: Vec<(KeyConfig, KeyState)> = user
            .iter()
            .cloned()
            .map(|key| (key, KeyState::Held))
            .collect();
        let rows = rows(&user, &[], true, &in_use);
        assert_eq!(rows[0].chips[0].clashes, [1]);
        assert_eq!(rows[1].chips[0].clashes, [0]);
        assert!(rows[2].chips[0].clashes.is_empty());
    }

    #[test]
    fn the_rows_are_listed_by_group() {
        let user = [acting("ctrl+alt+l", "popup", Some("launcher"))];
        let in_use = [(user[0].clone(), KeyState::Held)];
        let rows = rows(&user, &defaults(), true, &in_use);
        let order: Vec<usize> = (0..rows.len()).collect();
        let listed = lines(&rows, &order);
        assert_eq!(listed[0], Line::Group(Group::WindowsKeys));
        assert_eq!(&listed[1..3], [Line::Row(0), Line::Row(1)]);
        assert_eq!(listed[3], Line::Group(Group::Windows));
        assert_eq!(listed.last(), Some(&Line::Row(4)));
        assert_eq!(lines(&rows, &[]), []);
        assert_eq!(Group::of_action("pin"), Group::Dock);
        assert_eq!(Group::of_action("demo.hello"), Group::Extensions);
        assert_eq!(Group::of_action("narrator"), Group::WindowsKeys);
    }

    #[test]
    fn a_search_finds_every_word_somewhere() {
        assert!(found("", &["anything"]));
        assert!(found("snap LEFT", &["Snap the window", "win+left"]));
        assert!(!found("snap right", &["Snap the window", "win+left"]));
    }
}
