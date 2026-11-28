// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The windows of the file manager opened again at the next start, as
//! Explorer's "Restore previous folder windows at logon" does, where
//! `[files] restore_windows` asks for it: each window with its place on
//! the screen, whether it was maximized, and its tabs. They are written to
//! `%APPDATA%\tessera\files-windows.toml` when Tessera ends, and every 30
//! seconds while windows change, as the session may end without Tessera
//! being told in time; the tabs are written as `files-session.toml` writes
//! them.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{App, AsyncApp, Global, Task};
use serde::{Deserialize, Serialize};

use super::session::Session;
use super::view::FilesView;
use super::{Files, FilesConfig, WindowsChange};
use crate::config::config_dir;
use crate::platform::{AppWindowId, ScreenRect};

/// The file the windows are kept in, in the directory of the configuration.
const WINDOWS_FILE: &str = "files-windows.toml";

/// How often the windows are written while they change, and how soon after
/// one closes.
const EVERY: Duration = Duration::from_secs(30);
const SOON: Duration = Duration::from_secs(2);

/// What the file holds.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct KeptWindows {
    #[serde(rename = "window")]
    windows: Vec<KeptWindow>,
}

/// A window as the file keeps it.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct KeptWindow {
    /// What is seen of it when it is not maximized: left, top, right and
    /// bottom, in pixels of the screen.
    frame: Option<[i32; 4]>,
    maximized: bool,
    tabs: Session,
}

impl KeptWindow {
    fn frame(&self) -> Option<ScreenRect> {
        let [left, top, right, bottom] = self.frame?;
        (right > left && bottom > top).then_some(ScreenRect {
            left,
            top,
            right,
            bottom,
        })
    }
}

/// Keeps the windows of the file manager for the next start.
#[derive(Default)]
pub struct WindowsRestore {
    /// Whether the configuration asks for the windows to open again.
    on: bool,
    started: bool,
    /// Whether the windows changed since they were written.
    changed: bool,
    /// The write that waits a moment after a window closed.
    soon: Option<Task<()>>,
}

impl Global for WindowsRestore {}

impl WindowsRestore {
    /// Opens the windows kept at the last end, where the configuration
    /// asks for that, and from now on keeps them as they change and when
    /// Tessera ends. Called once, when the shell starts.
    pub fn start(config: &FilesConfig, cx: &mut App) {
        Self::configure(config, cx);
        let this = cx.default_global::<Self>();
        if this.started {
            return;
        }
        this.started = true;
        Files::observe_windows(cx, |change, cx| {
            Self::changed(matches!(change, WindowsChange::Closed(_)), cx);
        });
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(EVERY).await;
                if cx.update(Self::keep_if_changed).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.on_app_quit(|cx| {
            let text = { cx.global::<Self>().on }.then(|| Self::text(cx)).flatten();
            // GPUI waits a moment for this before the process ends.
            cx.background_executor().spawn(async move {
                if let Some(text) = text {
                    write_kept(&text);
                }
            })
        })
        .detach();
        if config.restore_windows {
            Self::reopen(cx);
        }
    }

    /// Takes whether windows are kept from `config`.
    pub fn configure(config: &FilesConfig, cx: &mut App) {
        cx.default_global::<Self>().on = config.restore_windows;
    }

    /// Notes that the windows changed; one that `closed` has them written
    /// soon, so that a window closed is not opened again.
    fn changed(closed: bool, cx: &mut App) {
        let this = cx.default_global::<Self>();
        this.changed = true;
        if !closed || !this.on {
            return;
        }
        let soon = cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor().timer(SOON).await;
            cx.update(Self::keep_if_changed).ok();
        });
        cx.global_mut::<Self>().soon = Some(soon);
    }

    /// Writes the windows where they changed since they were written.
    fn keep_if_changed(cx: &mut App) {
        let this = cx.default_global::<Self>();
        if !this.on || !this.changed {
            return;
        }
        this.changed = false;
        let Some(text) = Self::text(cx) else {
            return;
        };
        cx.background_executor()
            .spawn(async move { write_kept(&text) })
            .detach();
    }

    /// The windows open now, as the file keeps them.
    fn text(cx: &App) -> Option<String> {
        let windows = { Files::windows(cx).into_iter() }
            .filter_map(|window| {
                let view = window.handle.downcast::<FilesView>()?;
                let opening = view.read(cx).ok()?.opening(cx);
                let mut tabs = Session::default();
                tabs.set_tabs(&opening);
                let id = AppWindowId::from_raw(window.hwnd);
                Some(KeptWindow {
                    frame: { id.restored_frame() }
                        .map(|frame| [frame.left, frame.top, frame.right, frame.bottom]),
                    maximized: id.is_maximized(),
                    tabs,
                })
            })
            .collect();
        match toml::to_string(&KeptWindows { windows }) {
            Ok(text) => Some(text),
            Err(error) => {
                report!("the windows of the file manager cannot be kept: {error}");
                None
            }
        }
    }

    /// Opens the windows the file keeps, each where it was, read away from
    /// the user interface.
    fn reopen(cx: &mut App) {
        let read = cx.background_executor().spawn(async {
            let text = fs::read_to_string(kept_path()?).ok()?;
            match toml::from_str::<KeptWindows>(&text) {
                Ok(kept) => Some(kept),
                Err(error) => {
                    report!("{WINDOWS_FILE} is ignored: {error}");
                    None
                }
            }
        });
        cx.spawn(async move |cx: &mut AsyncApp| {
            let Some(kept) = read.await else {
                return;
            };
            cx.update(|cx| {
                for window in kept.windows {
                    let Some(opening) = window.tabs.opening(Files::named) else {
                        continue;
                    };
                    let Some(opened) = Files::open_with(opening, cx) else {
                        continue;
                    };
                    let (Some(frame), Ok(hwnd)) =
                        (window.frame(), opened.read_with(cx, |view, _| view.hwnd()))
                    else {
                        continue;
                    };
                    AppWindowId::from_raw(hwnd).place_restored(frame, window.maximized);
                }
            })
            .ok();
        })
        .detach();
    }
}

/// Where the windows are kept.
fn kept_path() -> Option<PathBuf> {
    Some(config_dir()?.join(WINDOWS_FILE))
}

/// Writes `text` as the windows kept, through a file beside it, so that an
/// end in the middle leaves the old file whole. Blocks; it is done away
/// from the user interface.
fn write_kept(text: &str) {
    let Some(path) = kept_path() else {
        return;
    };
    let fresh = path.with_extension(format!("toml.{}.new", std::process::id()));
    let written = { path.parent() }
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(&fresh, text))
        .and_then(|()| fs::rename(&fresh, &path));
    if let Err(error) = written {
        fs::remove_file(&fresh).ok();
        report!("the windows of the file manager cannot be kept: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::super::Location;
    use super::super::session::{OpenTab, Opening};
    use super::*;

    #[test]
    fn windows_are_kept_with_their_places_and_tabs() {
        let mut tabs = Session::default();
        let opening = Opening {
            tabs: vec![
                OpenTab::at(Location::Computer),
                OpenTab::at(Location::Folder(PathBuf::from("C:\\work"))),
            ],
            active: 1,
        };
        tabs.set_tabs(&opening);
        let kept = KeptWindows {
            windows: vec![KeptWindow {
                frame: Some([10, 20, 1090, 700]),
                maximized: true,
                tabs,
            }],
        };
        let text = toml::to_string(&kept).expect("windows can be written");
        let read: KeptWindows = toml::from_str(&text).expect("and read");
        assert_eq!(read, kept);
        let named = |name: &str| match name {
            "computer" => Location::Computer,
            path => Location::Folder(PathBuf::from(path)),
        };
        assert_eq!(read.windows[0].tabs.opening(named), Some(opening));
        assert_eq!(
            read.windows[0].frame(),
            Some(ScreenRect {
                left: 10,
                top: 20,
                right: 1090,
                bottom: 700
            })
        );
        let empty = KeptWindow {
            frame: Some([5, 5, 5, 9]),
            ..KeptWindow::default()
        };
        assert_eq!(empty.frame(), None);
        assert_eq!(
            toml::from_str::<KeptWindows>("").unwrap(),
            KeptWindows::default()
        );
    }
}
