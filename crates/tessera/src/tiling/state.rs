// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the tiling keeps over restarts, in `tiling-state.toml` of the
//! configuration directory: for each workspace of each display, the
//! layout chosen while Tessera ran, the shape and sizes of its tiles, and
//! the windows the user stacked, by their program, class and title, so
//! that the stacks are made again when those windows are found.
//!
//! The file is written by Tessera a moment after a change, so that many
//! changes in a row are written once. A file that cannot be read is
//! reported and taken as empty.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::layouts::LayoutChoice;
use super::tree::Node;
use crate::config::config_dir;

/// The file of the configuration directory the tiling is kept in.
const FILE: &str = "tiling-state.toml";

/// A workspace of a display: the display by its device name or its place
/// (see [`super::displays::DisplayName::key`]), the workspace by what
/// [`super::workspace_slot`] calls it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlaceKey {
    pub display: String,
    pub workspace: String,
}

/// What is kept of a place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kept {
    /// The layout chosen there while Tessera ran.
    pub chosen: Option<LayoutChoice>,
    /// The layout the tree is of, and the tree.
    pub tree: Option<(LayoutChoice, Node)>,
    /// The stacks the user made there, each by its windows in the order
    /// of their tabs.
    pub stacks: Vec<Vec<WindowKey>>,
}

/// A window as it is known again after a restart, as far as it can be:
/// by the file name of its program, its class and its title when it was
/// stacked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowKey {
    #[serde(default)]
    pub exe: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub title: String,
}

impl WindowKey {
    /// How well a window of program `exe`, class `class` and title
    /// `title` fits the key: 2 for all three, 1 for the program and the
    /// class with another title, as a browser shows another page; `None`
    /// for a window of another program or class.
    pub fn fit(&self, exe: &str, class: &str, title: &str) -> Option<u8> {
        if !self.exe.eq_ignore_ascii_case(exe) || self.class != class {
            return None;
        }
        Some(if self.title == title { 2 } else { 1 })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateFile {
    #[serde(default)]
    tiles: Vec<Entry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    display: String,
    workspace: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    chosen: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tree: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    stacks: Vec<Vec<WindowKey>>,
}

/// Where the file is.
pub fn path() -> Option<PathBuf> {
    Some(config_dir()?.join(FILE))
}

/// `kept` as the file writes it.
pub fn to_text(kept: &BTreeMap<PlaceKey, Kept>) -> String {
    let tiles = { kept.iter() }
        .filter(|(_, kept)| kept.chosen.is_some() || kept.tree.is_some() || !kept.stacks.is_empty())
        .map(|(key, kept)| Entry {
            display: key.display.clone(),
            workspace: key.workspace.clone(),
            chosen: kept.chosen.as_ref().map(LayoutChoice::name),
            layout: kept.tree.as_ref().map(|(layout, _)| layout.name()),
            tree: kept.tree.as_ref().map(|(_, tree)| tree.to_text()),
            stacks: kept.stacks.clone(),
        })
        .collect();
    let body = toml::to_string(&StateFile { tiles }).unwrap_or_default();
    format!(
        "# Kept by Tessera: the tiling layout, sizes and stacks of each workspace of each display.\n\n{body}"
    )
}

/// What the file `text` keeps, or why it cannot be read. An entry whose
/// tree cannot be read keeps its chosen layout.
pub fn from_text(text: &str) -> Result<BTreeMap<PlaceKey, Kept>, String> {
    let file: StateFile = toml::from_str(text).map_err(|error| error.message().to_owned())?;
    let mut kept = BTreeMap::new();
    for entry in file.tiles {
        let tree = match (entry.layout, entry.tree) {
            (Some(layout), Some(tree)) => Node::from_text(&tree)
                .ok()
                .map(|tree| (LayoutChoice::of(&layout), tree)),
            _ => None,
        };
        let key = PlaceKey {
            display: entry.display,
            workspace: entry.workspace,
        };
        let chosen = entry.chosen.as_deref().map(LayoutChoice::of);
        let stacks = { entry.stacks.into_iter() }
            .filter(|stack| stack.len() >= 2)
            .collect();
        kept.insert(
            key,
            Kept {
                chosen,
                tree,
                stacks,
            },
        );
    }
    Ok(kept)
}

/// Reads the file; a missing file keeps nothing, and one that cannot be
/// read is reported. It reads from the disk, so it is not called where it
/// would hold up drawing.
pub fn read() -> BTreeMap<PlaceKey, Kept> {
    let Some(path) = path() else {
        return BTreeMap::new();
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return BTreeMap::new(),
        Err(error) => {
            report!(
                "{} was not read, so the tiles start alike: {error}",
                path.display()
            );
            return BTreeMap::new();
        }
    };
    from_text(&text).unwrap_or_else(|why| {
        report!(
            "{} is not usable, so the tiles start alike: {why}",
            path.display()
        );
        BTreeMap::new()
    })
}

/// Writes `text` as the file. It writes to the disk, so it is not called
/// where it would hold up drawing.
pub fn write(text: &str) {
    let Some(path) = path() else {
        return;
    };
    if let Some(directory) = path.parent() {
        let _ = fs::create_dir_all(directory);
    }
    if let Err(error) = fs::write(&path, text) {
        report!(
            "{} was not written, so the sizes of the tiles are not kept: {error}",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiling::layouts::TileLayout;
    use crate::tiling::tree::Axis;

    #[test]
    fn what_is_kept_is_read_back() {
        let mut kept = BTreeMap::new();
        let tree = Node::Split {
            axis: Axis::Horizontal,
            children: vec![(Node::Leaf, 1.25), (Node::even(Axis::Vertical, 2), 0.75)],
        };
        kept.insert(
            PlaceKey {
                display: r"\\.\DISPLAY1".to_owned(),
                workspace: "0".to_owned(),
            },
            Kept {
                chosen: Some(LayoutChoice::Builtin(TileLayout::Bsp)),
                tree: Some((LayoutChoice::Builtin(TileLayout::Bsp), tree)),
                stacks: Vec::new(),
            },
        );
        kept.insert(
            PlaceKey {
                display: "#2".to_owned(),
                workspace: "3".to_owned(),
            },
            Kept {
                chosen: Some(LayoutChoice::File("tall".to_owned())),
                tree: None,
                stacks: vec![vec![
                    WindowKey {
                        exe: "editor.exe".to_owned(),
                        class: "Main".to_owned(),
                        title: "a \"b\"".to_owned(),
                    },
                    WindowKey::default(),
                ]],
            },
        );
        // A place with nothing to keep is not written.
        kept.insert(
            PlaceKey {
                display: "#3".to_owned(),
                workspace: "0".to_owned(),
            },
            Kept::default(),
        );
        let text = to_text(&kept);
        assert!(text.starts_with("# Kept by Tessera"));
        assert!(text.contains("editor.exe"));
        kept.retain(|_, kept| *kept != Kept::default());
        assert_eq!(from_text(&text), Ok(kept));
    }

    #[test]
    fn a_tree_that_cannot_be_read_leaves_the_layout() {
        let text = "[[tiles]]\ndisplay = \"#1\"\nworkspace = \"0\"\nchosen = \"rows\"\nlayout = \"rows\"\ntree = \"h(\"\n";
        let kept = from_text(text).unwrap();
        let place = kept.values().next().unwrap();
        assert_eq!(place.chosen, Some(LayoutChoice::Builtin(TileLayout::Rows)));
        assert_eq!(place.tree, None);
        assert!(from_text("[[tiles]]\ndisplay = 1").is_err());
        assert!(from_text("other = true").is_err());
        assert_eq!(from_text(""), Ok(BTreeMap::new()));
    }

    #[test]
    fn a_window_is_known_again_by_its_program_class_and_title() {
        let key = WindowKey {
            exe: "Browser.exe".to_owned(),
            class: "Chrome_WidgetWin_1".to_owned(),
            title: "News".to_owned(),
        };
        assert_eq!(
            key.fit("browser.exe", "Chrome_WidgetWin_1", "News"),
            Some(2)
        );
        assert_eq!(
            key.fit("browser.exe", "Chrome_WidgetWin_1", "Mail"),
            Some(1)
        );
        assert_eq!(key.fit("browser.exe", "Other", "News"), None);
        assert_eq!(key.fit("editor.exe", "Chrome_WidgetWin_1", "News"), None);
        // A stack of one window is not kept.
        let text =
            "[[tiles]]\ndisplay = \"#1\"\nworkspace = \"0\"\nstacks = [[{ exe = \"a.exe\" }]]\n";
        let kept = from_text(text).unwrap();
        assert!(kept.values().next().unwrap().stacks.is_empty());
    }
}
