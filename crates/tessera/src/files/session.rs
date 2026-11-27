// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the file manager keeps between its windows: the tabs of the last
//! window closed, to open again when `[files] restore` asks, the shares
//! of other computers visited, and, when `[files] per_folder` asks, how
//! each folder was last shown. They are kept in `files-session.toml` and
//! `files-views.toml` of the local data directory of Tessera, which are
//! no configuration and are written by the file manager alone.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui::{App, Global, Task};
use serde::{Deserialize, Serialize};

use super::model::{Column, Grouping, Location, Look};
use super::tabs::EVEN_SPLIT;

/// The file the tabs are kept in.
const SESSION_FILE: &str = "files-session.toml";

/// The file the looks of folders are kept in.
const VIEWS_FILE: &str = "files-views.toml";

/// How many folders keep their look; those not seen longest are
/// forgotten first.
const VIEWS_KEPT: usize = 500;

/// How long after a change of a look it is written, so that a few
/// changes in a row are written once.
const VIEWS_DELAY: Duration = Duration::from_secs(2);

/// The directory Tessera keeps what it learns in, apart from the
/// configuration: `%LOCALAPPDATA%\tessera`.
pub fn state_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("tessera"))
}

/// Counts the files written beside the kept ones, so that two writes at
/// once never share one.
static FRESH: AtomicU64 = AtomicU64::new(0);

/// Held while the session file is read, changed and written again, so
/// that two windows changing it at once do not lose a change.
static SESSION_LOCK: Mutex<()> = Mutex::new(());

/// Writes `text` to the file `name` of the local data directory, through
/// a file beside it of its own, so that a crash leaves the old file whole
/// and two writes at once do not mix. Blocks; it is done away from the
/// user interface.
pub fn write_state(name: &str, text: &str) {
    let Some(directory) = state_dir() else {
        return;
    };
    let fresh = directory.join(format!(
        "{name}.{}.{}.new",
        std::process::id(),
        FRESH.fetch_add(1, Ordering::Relaxed)
    ));
    let written = fs::create_dir_all(&directory).and_then(|()| {
        fs::write(&fresh, text)?;
        fs::rename(&fresh, directory.join(name))
    });
    if let Err(error) = written {
        fs::remove_file(&fresh).ok();
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

    /// Reads the session kept, lets `change` change it, and keeps it again
    /// where `change` says it changed, one window at a time. Blocks.
    pub fn change(change: impl FnOnce(&mut Self) -> bool) {
        let _held = SESSION_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut session = Self::load();
        if change(&mut session) {
            session.save();
        }
    }

    /// Keeps the session. Blocks.
    fn save(&self) {
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

/// How a folder was last shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderView {
    pub look: Look,
    pub zoom: usize,
    pub column: Column,
    pub descending: bool,
    pub grouping: Grouping,
}

/// A look of a folder as the file keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct SavedView {
    path: String,
    #[serde(flatten)]
    view: FolderView,
    /// When it was used last, counted in uses.
    used: u64,
}

/// What the file of looks holds.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct SavedViews {
    views: Vec<SavedView>,
}

/// How each folder was last shown, for the configuration that keeps a
/// look per folder.
#[derive(Default)]
pub struct FolderViews {
    /// By the path in small letters, with when each was used last.
    views: HashMap<String, (FolderView, u64)>,
    /// The count of uses, which tells the latest.
    clock: u64,
    /// Whether the file was asked for, and whether it was read.
    asked: bool,
    loaded: bool,
    writing: Option<Task<()>>,
}

impl Global for FolderViews {}

/// The key of a folder among the looks, alike in any case.
fn view_key(path: &Path) -> String {
    path.to_string_lossy().trim_end_matches('\\').to_lowercase()
}

impl FolderViews {
    /// Reads the file of looks, once, away from the user interface; until
    /// it is read, folders have no look of their own.
    pub fn load(cx: &mut App) {
        let views = cx.default_global::<Self>();
        if views.asked {
            return;
        }
        views.asked = true;
        cx.spawn(async move |cx| {
            let saved = cx
                .background_executor()
                .spawn(async {
                    read_state(VIEWS_FILE)
                        .and_then(|text| toml::from_str::<SavedViews>(&text).ok())
                        .unwrap_or_default()
                })
                .await;
            cx.update(|cx| {
                let views = cx.default_global::<Self>();
                views.loaded = true;
                for saved in saved.views {
                    views.clock = views.clock.max(saved.used);
                    // A look changed while the file was read is newer.
                    { views.views.entry(saved.path) }.or_insert((saved.view, saved.used));
                }
            })
            .ok();
        })
        .detach();
    }

    /// Whether the file of looks was read, so that folders have the looks
    /// kept.
    pub fn loaded(cx: &App) -> bool {
        cx.try_global::<Self>().is_some_and(|views| views.loaded)
    }

    /// How the folder at `path` was last shown, if it is kept.
    pub fn get(path: &Path, cx: &App) -> Option<FolderView> {
        let views = cx.try_global::<Self>()?;
        views.views.get(&view_key(path)).map(|(view, _)| *view)
    }

    /// Keeps `view` as how the folder at `path` is shown, and writes the
    /// looks a moment later.
    pub fn put(path: &Path, view: FolderView, cx: &mut App) {
        let views = cx.default_global::<Self>();
        views.clock += 1;
        let used = views.clock;
        views.views.insert(view_key(path), (view, used));
        if views.views.len() > VIEWS_KEPT {
            let oldest = { views.views.iter() }
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                views.views.remove(&oldest);
            }
        }
        let saved = SavedViews {
            views: { views.views.iter() }
                .map(|(path, (view, used))| SavedView {
                    path: path.clone(),
                    view: *view,
                    used: *used,
                })
                .collect(),
        };
        let text = match toml::to_string(&saved) {
            Ok(text) => text,
            Err(error) => {
                report!("the file manager cannot write the looks of folders: {error}");
                return;
            }
        };
        let executor = cx.background_executor().clone();
        let writing = cx.background_executor().spawn(async move {
            executor.timer(VIEWS_DELAY).await;
            write_state(VIEWS_FILE, &text);
        });
        // A write that waits is replaced by this one, which drops it.
        cx.global_mut::<Self>().writing = Some(writing);
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
    fn the_look_of_a_folder_is_kept_in_its_file() {
        let saved = SavedViews {
            views: vec![SavedView {
                path: "c:\\users\\me".to_owned(),
                view: FolderView {
                    look: Look::Icons,
                    zoom: 3,
                    column: Column::Modified,
                    descending: true,
                    grouping: Grouping::Modified,
                },
                used: 7,
            }],
        };
        let text = toml::to_string(&saved).expect("looks can be written");
        let read: SavedViews = toml::from_str(&text).expect("and read");
        assert_eq!(read.views, saved.views);
        assert_eq!(view_key(Path::new("C:\\Users\\Me\\")), "c:\\users\\me");
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
