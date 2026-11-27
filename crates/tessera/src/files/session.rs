// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the file manager keeps between its windows: the tabs of the last
//! window closed, to open again when `[files] restore` asks. It is kept
//! in `files-session.toml` of the local data directory of Tessera, which
//! is no configuration and is written by the file manager alone.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::model::Location;
use super::tabs::EVEN_SPLIT;

/// The file the tabs are kept in.
const SESSION_FILE: &str = "files-session.toml";

/// The directory Tessera keeps what it learns in, apart from the
/// configuration: `%LOCALAPPDATA%\tessera`.
pub fn state_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("tessera"))
}

/// Writes `text` to the file `name` of the local data directory, through
/// a file beside it, so that a crash leaves the old file whole. Blocks;
/// it is done away from the user interface.
pub fn write_state(name: &str, text: &str) {
    let Some(directory) = state_dir() else {
        return;
    };
    let written = fs::create_dir_all(&directory).and_then(|()| {
        let path = directory.join(name);
        let fresh = path.with_extension("toml.new");
        fs::write(&fresh, text)?;
        fs::rename(&fresh, &path)
    });
    if let Err(error) = written {
        report!("the file manager cannot keep {name}: {error}");
    }
}

/// The text of the file `name` of the local data directory, where there
/// is one.
pub fn read_state(name: &str) -> Option<String> {
    fs::read_to_string(state_dir()?.join(name)).ok()
}

/// A tab to open: its locations, the one in front, and how the width is
/// shared between two.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenTab {
    pub panes: Vec<Location>,
    pub active: usize,
    pub split: f32,
}

impl OpenTab {
    pub fn at(location: Location) -> Self {
        Self {
            panes: vec![location],
            active: 0,
            split: EVEN_SPLIT,
        }
    }
}

/// The tabs a window opens with, and the one in front.
#[derive(Clone, Debug, PartialEq)]
pub struct Opening {
    pub tabs: Vec<OpenTab>,
    pub active: usize,
}

impl Opening {
    /// One tab at `location`.
    pub fn at(location: Location) -> Self {
        Self {
            tabs: vec![OpenTab::at(location)],
            active: 0,
        }
    }
}

/// A tab as the session file keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct SavedTab {
    /// The locations as `Files::named` reads them.
    panes: Vec<String>,
    active: usize,
    split: f32,
}

/// What the session file holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    tabs: Vec<SavedTab>,
    active: usize,
    /// The shares of other computers visited, as `\\server\share`.
    pub network: Vec<PathBuf>,
}

impl Session {
    /// The session kept last, or an empty one. Blocks.
    pub fn load() -> Self {
        let Some(text) = read_state(SESSION_FILE) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_else(|error| {
            report!("{SESSION_FILE} is ignored: {error}");
            Self::default()
        })
    }

    /// Keeps the session. Blocks.
    pub fn save(&self) {
        match toml::to_string(self) {
            Ok(text) => write_state(SESSION_FILE, &text),
            Err(error) => report!("the file manager cannot write its session: {error}"),
        }
    }

    /// Keeps `opening` as the tabs to open again.
    pub fn set_tabs(&mut self, opening: &Opening) {
        self.tabs = { opening.tabs.iter() }
            .map(|tab| SavedTab {
                panes: tab.panes.iter().map(Location::name).collect(),
                active: tab.active,
                split: tab.split,
            })
            .collect();
        self.active = opening.active;
    }

    /// Remembers the share `root` of another computer, once; says
    /// whether it was new.
    pub fn visited(&mut self, root: PathBuf) -> bool {
        if self.network.contains(&root) {
            return false;
        }
        self.network.push(root);
        true
    }

    /// The tabs to open again, each location read with `named`; `None`
    /// where none was kept.
    pub fn opening(&self, named: impl Fn(&str) -> Location) -> Option<Opening> {
        let tabs: Vec<OpenTab> = { self.tabs.iter() }
            .filter(|tab| !tab.panes.is_empty())
            .map(|tab| {
                let panes: Vec<Location> =
                    tab.panes.iter().take(2).map(|name| named(name)).collect();
                OpenTab {
                    active: tab.active.min(panes.len() - 1),
                    split: match tab.split {
                        split if split > 0. && split < 1. => split,
                        _ => EVEN_SPLIT,
                    },
                    panes,
                }
            })
            .collect();
        (!tabs.is_empty()).then(|| Opening {
            active: self.active.min(tabs.len() - 1),
            tabs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_are_kept_and_opened_again() {
        let folder = |name: &str| Location::Folder(PathBuf::from(name));
        let opening = Opening {
            tabs: vec![
                OpenTab::at(Location::Computer),
                OpenTab {
                    panes: vec![folder("C:\\a"), folder("D:\\b")],
                    active: 1,
                    split: 0.3,
                },
            ],
            active: 1,
        };
        let mut session = Session::default();
        session.set_tabs(&opening);
        let text = toml::to_string(&session).expect("a session can be written");
        let read: Session = toml::from_str(&text).expect("and read");
        let named = |name: &str| match name {
            "computer" => Location::Computer,
            path => folder(path),
        };
        assert_eq!(read.opening(named), Some(opening));
        assert_eq!(Session::default().opening(named), None);
    }

    #[test]
    fn a_kept_tab_that_makes_no_sense_is_mended() {
        let session: Session = toml::from_str(
            "active = 7\n[[tabs]]\npanes = [\"computer\"]\nactive = 3\nsplit = 4.0\n[[tabs]]\n",
        )
        .expect("a session can be read");
        let opening = session.opening(|_| Location::Computer).expect("one tab");
        assert_eq!(opening.active, 0);
        assert_eq!(opening.tabs, [OpenTab::at(Location::Computer)]);
    }
}
