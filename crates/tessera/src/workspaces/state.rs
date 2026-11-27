// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the workspaces of Tessera's own keep over a restart, in
//! `workspaces-state.toml` of the configuration directory: the workspaces
//! of each row with their numbers that stay, the one each row shows, and
//! which window lives on which workspace or on all of them. A window is
//! named by its number together with its program and when its process
//! started, so that another window that got the same number is not taken
//! for it.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::config::config_dir;
use crate::platform::WindowMark;

/// The file of the configuration directory the state is kept in.
const FILE: &str = "workspaces-state.toml";

/// What is kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// The count of workspaces `own` asked for when this was written; a
    /// state written for another count is not taken up.
    pub own: usize,
    /// Whether every display had a row of its own then.
    pub per_display: bool,
    /// The number the next workspace that is made gets.
    pub next_id: u64,
    #[serde(rename = "row")]
    pub rows: Vec<SavedRow>,
    #[serde(rename = "window")]
    pub windows: Vec<SavedWindow>,
}

/// One row of workspaces: the one of all displays, or that of a display.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedRow {
    /// The device of the display, as `\\.\DISPLAY1`; empty for the row
    /// all displays share.
    pub display: String,
    /// The number of each workspace, the first first.
    pub ids: Vec<u64>,
    /// Which workspace is shown, counted from zero.
    pub current: usize,
}

/// Where a window lives.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedWindow {
    /// The number Windows knows the window by.
    pub window: i64,
    pub executable: String,
    /// When its process started, in ticks of 100 ns.
    pub started: u64,
    /// The device of the display of its row, as in [`SavedRow::display`].
    pub display: String,
    /// The number of its workspace; nothing for a window on every
    /// workspace.
    pub workspace: u64,
    /// Whether it is seen on every workspace.
    pub sticky: bool,
}

impl SavedWindow {
    /// Whether `mark` is that of this window.
    pub fn is(&self, mark: &WindowMark) -> bool {
        self.started == mark.started && self.executable.eq_ignore_ascii_case(&mark.executable)
    }
}

impl Saved {
    fn path() -> Option<PathBuf> {
        Some(config_dir()?.join(FILE))
    }

    /// What was kept, if anything was and it can be read.
    pub fn read() -> Option<Self> {
        let text = fs::read_to_string(Self::path()?).ok()?;
        match toml::from_str(&text) {
            Ok(saved) => Some(saved),
            Err(error) => {
                report!(
                    "{FILE} cannot be read, so the windows start on the workspace shown: {error}"
                );
                None
            }
        }
    }

    /// Writes the state, beside the file first and then in its place, so
    /// that a half-written file is never read. `sequence` counts the
    /// states made: one made before the state written last is not written
    /// over it, as can happen when they are written on other threads.
    pub fn write(&self, sequence: u64) {
        static LAST: Mutex<u64> = Mutex::new(0);
        let Ok(mut last) = LAST.lock() else {
            return;
        };
        if sequence < *last {
            return;
        }
        *last = sequence;
        let Some(path) = Self::path() else {
            return;
        };
        let text = match toml::to_string(self) {
            Ok(text) => text,
            Err(error) => {
                report!("the state of the workspaces was not written: {error}");
                return;
            }
        };
        let draft = path.with_extension("toml.new");
        let written = fs::write(&draft, text).and_then(|()| fs::rename(&draft, &path));
        if let Err(error) = written {
            report!(
                "{} was not written, so the windows do not go back to their workspaces after a restart: {error}",
                path.display()
            );
        }
    }

    /// Whether the state was written for workspaces configured as now.
    pub fn fits(&self, own: usize, per_display: bool) -> bool {
        self.own == own && self.per_display == per_display
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> Saved {
        Saved {
            own: 3,
            per_display: false,
            next_id: 5,
            rows: vec![SavedRow {
                display: String::new(),
                ids: vec![1, 4, 3],
                current: 1,
            }],
            windows: vec![SavedWindow {
                window: 1234,
                executable: r"C:\Windows\notepad.exe".to_owned(),
                started: 133_000_000_000_000_000,
                display: String::new(),
                workspace: 4,
                sticky: false,
            }],
        }
    }

    #[test]
    fn the_state_reads_as_it_was_written() {
        let text = toml::to_string(&saved()).unwrap();
        assert_eq!(toml::from_str::<Saved>(&text).unwrap(), saved());
        assert!(text.contains("[[row]]") && text.contains("[[window]]"));
    }

    #[test]
    fn a_state_missing_parts_reads_with_nothing_in_them() {
        let read: Saved = toml::from_str("own = 2").unwrap();
        assert_eq!(read.own, 2);
        assert!(read.rows.is_empty() && read.windows.is_empty());
    }

    #[test]
    fn a_window_is_known_by_its_program_and_the_start_of_its_process() {
        let kept = &saved().windows[0];
        let mark = |executable: &str, started| WindowMark {
            executable: executable.to_owned(),
            started,
        };
        assert!(kept.is(&mark(r"c:\windows\NOTEPAD.EXE", 133_000_000_000_000_000)));
        assert!(!kept.is(&mark(r"C:\Windows\notepad.exe", 133_000_000_000_000_001)));
        assert!(!kept.is(&mark(r"C:\Windows\write.exe", 133_000_000_000_000_000)));
    }

    #[test]
    fn a_state_is_taken_up_only_for_the_same_workspaces() {
        let saved = saved();
        assert!(saved.fits(3, false));
        assert!(!saved.fits(4, false));
        assert!(!saved.fits(3, true));
    }
}
