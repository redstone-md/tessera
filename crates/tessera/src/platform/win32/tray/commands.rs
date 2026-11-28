// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The commands scripts and `Shell.Application` post to the taskbar as
//! `WM_COMMAND` with a number, such as 401 for the Run dialog or 415 to
//! minimize every window.
//!
//! Microsoft documents none of the numbers. Those answered here are kept
//! in one table with where each was taken from: the explorer of ReactOS
//! (`TRAYCMD_*` in its sources) and the documentation and scripts of
//! AutoHotkey. Beside Explorer the commands go on to Explorer's taskbar,
//! as they always went there. Where the shell serves them, the tray thread
//! hands each one to the thread of the interface over a channel, posting
//! and never waiting for it, and a number not in the table is reported.
//!
//! Cascading and tiling are done here, on a thread of their own, with
//! `CascadeWindows` and `TileWindows` over the windows of applications on
//! the primary display: they move windows of other programs and wait for
//! each.

use std::sync::OnceLock;
use std::thread;

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CascadeWindows, IsHungAppWindow, IsIconic, MDITILE_HORIZONTAL, MDITILE_SKIPDISABLED,
    MDITILE_VERTICAL, MDITILE_ZORDER, TileWindows,
};

use super::super::AppWindow;

/// What a command posted to the taskbar asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    /// The Run dialog.
    Run,
    /// The dialog that shuts Windows down, signs out or restarts.
    ShutDown,
    /// The windows of the primary display one over the other, each a
    /// little lower and further right.
    Cascade,
    /// The windows of the primary display one above the other.
    TileHorizontally,
    /// The windows of the primary display side by side.
    TileVertically,
    /// Shows the desktop, or brings the windows back.
    ShowDesktop,
    /// The calendar with the date and the time.
    DateAndTime,
    /// The settings of the taskbar.
    TaskbarProperties,
    /// Minimizes every window.
    MinimizeAll,
    /// Brings back the windows minimized so.
    UndoMinimizeAll,
    TaskManager,
    /// The settings of the notification icons.
    NotificationIcons,
}

/// Every number answered: the command, and where the number was taken
/// from. None of them is documented by Microsoft or was checked against
/// the taskbar of Windows 11.
pub const COMMANDS: &[(u16, TrayCommand, &str)] = &[
    (401, TrayCommand::Run, "ReactOS TRAYCMD_RUN_DIALOG"),
    (
        402,
        TrayCommand::ShutDown,
        "ReactOS TRAYCMD_LOGOFF_DIALOG; AutoHotkey scripts use it for the shut down dialog",
    ),
    (403, TrayCommand::Cascade, "ReactOS TRAYCMD_CASCADE"),
    (404, TrayCommand::TileHorizontally, "ReactOS TRAYCMD_TILE_H"),
    (405, TrayCommand::TileVertically, "ReactOS TRAYCMD_TILE_V"),
    (
        407,
        TrayCommand::ShowDesktop,
        "ReactOS TRAYCMD_TOGGLE_DESKTOP; AutoHotkey's documentation",
    ),
    (
        408,
        TrayCommand::DateAndTime,
        "ReactOS TRAYCMD_DATE_AND_TIME",
    ),
    (
        413,
        TrayCommand::TaskbarProperties,
        "ReactOS TRAYCMD_TASKBAR_PROPERTIES",
    ),
    (
        415,
        TrayCommand::MinimizeAll,
        "ReactOS TRAYCMD_MINIMIZE_ALL; AutoHotkey's documentation (WinMinimizeAll)",
    ),
    (
        416,
        TrayCommand::UndoMinimizeAll,
        "ReactOS TRAYCMD_RESTORE_ALL; AutoHotkey's documentation (WinMinimizeAllUndo)",
    ),
    (
        419,
        TrayCommand::MinimizeAll,
        "ReactOS TRAYCMD_SHOW_DESKTOP; AutoHotkey's documentation (419, minimize all)",
    ),
    (
        420,
        TrayCommand::TaskManager,
        "ReactOS TRAYCMD_SHOW_TASK_MGR",
    ),
    (
        505,
        TrayCommand::NotificationIcons,
        "AutoHotkey scripts (the notification icons of the taskbar's settings)",
    ),
];

impl TrayCommand {
    /// The command the number `number` stands for, if it is in the table.
    pub fn of(number: u16) -> Option<Self> {
        { COMMANDS.iter() }
            .find(|(known, ..)| *known == number)
            .map(|(_, command, _)| *command)
    }

    /// Arranges the windows of applications on the primary display, for
    /// the commands that do; the others do nothing here. It returns at
    /// once: the windows are moved on a thread of their own, as moving
    /// a window of another program waits for that program.
    pub fn arrange(self) {
        if !matches!(
            self,
            Self::Cascade | Self::TileHorizontally | Self::TileVertically
        ) {
            return;
        }
        let started = thread::Builder::new()
            .name("arrange windows".to_owned())
            .spawn(move || arrange(self));
        if let Err(error) = started {
            report!("the windows are not arranged: {error}");
        }
    }
}

/// How many commands wait for the thread of the interface.
const WAITING: usize = 8;

fn channel() -> &'static (Sender<TrayCommand>, Receiver<TrayCommand>) {
    static CHANNEL: OnceLock<(Sender<TrayCommand>, Receiver<TrayCommand>)> = OnceLock::new();
    CHANNEL.get_or_init(|| async_channel::bounded(WAITING))
}

/// The commands that arrived while the shell serves them. One reader is
/// meant to take them; the oldest give way while nobody reads.
pub(super) fn subscribe() -> Receiver<TrayCommand> {
    channel().1.clone()
}

/// Takes the command that `WM_COMMAND` carried in the low word of its
/// `wParam`: hands it to the reader, or reports a number not known.
pub(super) fn received(number: u16) {
    match TrayCommand::of(number) {
        Some(command) => {
            let _ = channel().0.force_send(command);
        }
        None => {
            report!("a program sent the taskbar the command {number}, which Tessera does not know")
        }
    }
}

/// Cascades or tiles the windows of applications on the primary display
/// that are shown and not minimized, within the room it leaves to
/// windows. A window whose program hangs is left where it is.
fn arrange(command: TrayCommand) {
    // SAFETY: the calls only read which display is the primary one and
    // where it leaves room; `info` is correctly sized and outlives them.
    let (primary, area) = unsafe {
        let primary = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(primary, &raw mut info).as_bool() {
            return;
        }
        (primary, info.rcWork)
    };
    let windows: Vec<HWND> = { AppWindow::handles().into_iter() }
        .map(|(hwnd, _)| hwnd)
        .filter(|&hwnd| {
            // SAFETY: the calls only read the state of the window, and
            // tolerate one that has gone.
            unsafe {
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY) == primary
                    && !IsIconic(hwnd).as_bool()
                    && !IsHungAppWindow(hwnd).as_bool()
            }
        })
        .collect();
    if windows.is_empty() {
        return;
    }
    let area: *const RECT = &raw const area;
    // SAFETY: the area and the list outlive the calls, which move the
    // windows they name and skip those that are gone.
    unsafe {
        match command {
            TrayCommand::Cascade => {
                let how = MDITILE_ZORDER | MDITILE_SKIPDISABLED;
                CascadeWindows(None, how, Some(area), Some(&windows));
            }
            TrayCommand::TileHorizontally => {
                TileWindows(None, MDITILE_HORIZONTAL, Some(area), Some(&windows));
            }
            TrayCommand::TileVertically => {
                TileWindows(None, MDITILE_VERTICAL, Some(area), Some(&windows));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_number_stands_for_one_command() {
        assert_eq!(TrayCommand::of(401), Some(TrayCommand::Run));
        assert_eq!(TrayCommand::of(415), Some(TrayCommand::MinimizeAll));
        assert_eq!(TrayCommand::of(419), Some(TrayCommand::MinimizeAll));
        assert_eq!(TrayCommand::of(416), Some(TrayCommand::UndoMinimizeAll));
        assert_eq!(TrayCommand::of(505), Some(TrayCommand::NotificationIcons));
        assert_eq!(TrayCommand::of(406), None);
        let mut numbers: Vec<u16> = COMMANDS.iter().map(|(number, ..)| *number).collect();
        numbers.dedup();
        assert_eq!(numbers.len(), COMMANDS.len());
        assert!(COMMANDS.iter().all(|(_, _, source)| !source.is_empty()));
    }
}
