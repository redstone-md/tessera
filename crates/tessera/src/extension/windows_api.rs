// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The windows as extensions are told of them, and what an extension may
//! do to them, as a script of KWin may: bring one in front, minimize it,
//! close it, or move it to another workspace.
//!
//! The list is read on the thread of the extension; what is done to a
//! window is done afterwards by the thread that draws, which keeps the
//! workspaces.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use gpui::App;

use super::nodes::is_packaged;
use super::system::ShellState;
use crate::platform::{AppWindow, AppWindowId, DisplayKey};
use crate::workspaces::Workspaces;

/// A window as an extension is told of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowRow {
    pub id: u64,
    pub title: String,
    /// The AppUserModelID of a packaged application, or else the program
    /// file; see [`app_of`].
    pub app: String,
    pub display: u32,
    pub workspace: Option<u32>,
    pub minimized: bool,
    pub focused: bool,
}

/// The windows a task switcher shows, in the order of Windows, the one in
/// front first; `shell` tells the workspaces they live on.
pub fn list(shell: Option<&ShellState>) -> Vec<WindowRow> {
    let front = AppWindowId::foreground();
    let windows = AppWindow::enumerate().into_iter();
    let rows = windows.map(|window| {
        let app = app_of(window.app_id.as_deref(), window.executable.as_deref());
        let raw = window.id.raw();
        WindowRow {
            id: raw as u64,
            title: window.id.title(),
            app,
            display: display_number(window.display),
            workspace: shell.and_then(|shell| shell.homes.get(&raw).copied()),
            minimized: window.id.is_minimized(),
            focused: Some(window.id) == front,
        }
    });
    let mut rows: Vec<WindowRow> = rows.collect();
    // The one in front first, the rest in the order of Windows.
    if let Some(at) = rows.iter().position(|row| row.focused) {
        let focused = rows.remove(at);
        rows.insert(0, focused);
    }
    rows
}

/// What names the application of a window to an extension, so that the
/// icon of an image node can be had of it: the AppUserModelID `app_id`
/// where it is one of a packaged application, or else the program file.
/// Many desktop programs set an AppUserModelID of their own, such as
/// `Chrome`, of which no icon can be had; that one is told only where
/// Windows does not tell the program file.
fn app_of(app_id: Option<&str>, executable: Option<&Path>) -> String {
    match (app_id, executable) {
        (Some(id), _) if is_packaged(id) => id.to_owned(),
        (_, Some(file)) => file.to_string_lossy().into_owned(),
        (Some(id), None) => id.to_owned(),
        (None, None) => String::new(),
    }
}

/// The same number for every window of a display.
fn display_number(display: DisplayKey) -> u32 {
    let mut hasher = DefaultHasher::new();
    display.hash(&mut hasher);
    hasher.finish() as u32
}

/// The window numbered `id`, if it is one a task switcher shows: an
/// extension reaches no other, nor those of Tessera.
pub fn find(id: u64) -> Option<AppWindowId> {
    let raw = isize::try_from(id).ok()?;
    let window = AppWindowId::from_raw(raw);
    let windows = AppWindow::enumerate();
    windows
        .iter()
        .any(|listed| listed.id == window)
        .then_some(window)
}

/// What an extension does to a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowDeed {
    Focus,
    Minimize,
    Close,
    /// To the workspace of Tessera at this place.
    Move(u32),
}

impl WindowDeed {
    /// Fails, saying why, where the deed cannot be done with the
    /// workspaces `shell` tells of.
    pub fn check(self, shell: Option<&ShellState>) -> Result<(), String> {
        let Self::Move(place) = self else {
            return Ok(());
        };
        let workspaces = shell.map(|shell| &shell.workspaces);
        match workspaces {
            Some(workspaces) if workspaces.tessera && place < workspaces.count => Ok(()),
            Some(workspaces) if workspaces.tessera => Err(format!(
                "there are {} workspaces, counted from 0; {place} is none of them",
                workspaces.count
            )),
            _ => Err("Tessera keeps no workspaces of its own to move windows between".to_owned()),
        }
    }

    /// Does the deed to `window`, as the thread that draws.
    pub fn perform(self, window: AppWindowId, cx: &mut App) {
        match self {
            Self::Focus => match Workspaces::residents(None, cx) {
                // A window on another workspace is hidden until it is gone
                // to.
                Some(_) => Workspaces::show_window(window, cx),
                None => window.activate(),
            },
            Self::Minimize => window.minimize(),
            Self::Close => window.close(),
            Self::Move(place) => Workspaces::move_window(window, place as usize, cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::system::WorkspaceState;

    #[test]
    fn a_window_is_named_by_its_package_or_else_its_program_file() {
        let chrome = Path::new(r"C:\Program Files\Google\Chrome\chrome.exe");
        let calculator = "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App";
        assert_eq!(app_of(Some(calculator), Some(chrome)), calculator);
        assert_eq!(
            app_of(Some("Chrome"), Some(chrome)),
            r"C:\Program Files\Google\Chrome\chrome.exe"
        );
        assert_eq!(app_of(None, Some(chrome)), chrome.to_string_lossy());
        assert_eq!(app_of(Some("Chrome"), None), "Chrome");
        assert_eq!(app_of(None, None), "");
    }

    #[test]
    fn windows_are_moved_only_between_workspaces_of_tessera() {
        let shell = |count, tessera| ShellState {
            workspaces: WorkspaceState {
                count,
                current: 0,
                names: Vec::new(),
                tessera,
            },
            ..ShellState::default()
        };
        assert_eq!(WindowDeed::Move(2).check(Some(&shell(3, true))), Ok(()));
        assert!(WindowDeed::Move(3).check(Some(&shell(3, true))).is_err());
        assert!(WindowDeed::Move(0).check(Some(&shell(4, false))).is_err());
        assert!(WindowDeed::Move(0).check(None).is_err());
        assert_eq!(WindowDeed::Close.check(None), Ok(()));
    }
}
