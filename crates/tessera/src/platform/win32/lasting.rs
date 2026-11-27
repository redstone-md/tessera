// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What tells a window or a display apart beyond the number Windows gives
//! it, which names another window or display once it is gone: for what
//! Tessera keeps over its own restart, such as which workspace a window
//! was on.

use windows::Win32::Foundation::{CloseHandle, FILETIME};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::app_windows::executable_of;
use super::hdr::device_of;
use super::snap::Display;
use super::{AppWindowId, DisplayKey};

/// What a window is told apart by from a later window that gets the same
/// number: the program of its process and when that process started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowMark {
    /// The path of the program.
    pub executable: String,
    /// When the process started, in the ticks of 100 ns of Windows.
    pub started: u64,
}

impl AppWindowId {
    /// The mark of the window; `None` for a window that is gone and for
    /// one whose process may not be asked, as that of an administrator.
    pub fn mark(self) -> Option<WindowMark> {
        let process = self.process();
        if process == 0 {
            return None;
        }
        let executable = executable_of(process)?;
        Some(WindowMark {
            executable: executable.to_string_lossy().into_owned(),
            started: process_started(process)?,
        })
    }
}

impl DisplayKey {
    /// The name Windows gives the device of the display, as `\\.\DISPLAY1`,
    /// which stays the same over a restart while the displays do.
    pub fn device(self) -> Option<String> {
        device_of(Some(self)).map(|name| String::from_utf16_lossy(&name))
    }

    /// The display whose device is named `device`, if one is connected.
    pub fn of_device(device: &str) -> Option<Self> {
        let displays = Display::all().into_iter().map(|display| display.key);
        displays
            .into_iter()
            .find(|key| key.device().as_deref() == Some(device))
    }
}

/// When the process `process` started.
fn process_started(process: u32) -> Option<u64> {
    let (mut created, mut ended, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the times outlive the call, and the handle is closed before
    // returning.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process).ok()?;
        let read = GetProcessTimes(
            handle,
            &raw mut created,
            &raw mut ended,
            &raw mut kernel,
            &raw mut user,
        );
        let _ = CloseHandle(handle);
        read.ok()?;
    }
    Some(u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
}
