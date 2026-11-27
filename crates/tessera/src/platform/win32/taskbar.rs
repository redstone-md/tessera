// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Hiding the Explorer taskbar while the shell runs.
//!
//! The taskbar is switched to auto-hide, so it stops reserving screen space,
//! and its windows are hidden. Both changes outlive the process that makes
//! them, so they are undone by a separate watchdog process that waits for the
//! shell to exit for any reason, including a crash or being terminated.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, GetCurrentProcessId, INFINITE, OpenProcess, PROCESS_SYNCHRONIZE,
    WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    ABM_GETSTATE, ABM_SETSTATE, ABS_AUTOHIDE, APPBARDATA, SHAppBarMessage,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, SW_HIDE,
    SW_SHOWNOACTIVATE, ShowWindow, WM_COMMAND,
};
use windows::core::{PCWSTR, w};

use super::shell_mode::shell_mode;
use super::winlogon::SHELL_FLAG;

/// The command of the taskbar that shows the desktop or brings the
/// windows back.
const TOGGLE_DESKTOP: usize = 407;

/// Command-line flag that turns the process into the restore watchdog.
const WATCHDOG_FLAG: &str = "--restore-taskbar-after";

/// Command-line flag of a shell that starts only once the processes
/// whose numbers follow it are gone.
const SUCCESSOR_FLAG: &str = "--start-after";

/// The watchdog this shell started.
static WATCHDOG: OnceLock<u32> = OnceLock::new();

/// Whether this shell started the one that takes over from it.
static SUCCESSOR: AtomicBool = AtomicBool::new(false);

/// Waits until the process with the number `process` is gone.
pub(super) fn wait_for(process: u32) {
    // SAFETY: the handle is only waited on and then closed. A process that
    // is already gone cannot be opened and the wait is skipped.
    unsafe {
        if let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, process) {
            WaitForSingleObject(process, INFINITE);
            let _ = CloseHandle(process);
        }
    }
}

/// The Explorer taskbar on every display.
pub struct Taskbar;

impl Taskbar {
    /// Hides the taskbar until this process exits.
    ///
    /// Nothing is changed unless the watchdog that will undo the change has
    /// been started first.
    pub fn hide_until_exit() -> Result<()> {
        let Some(primary) = Self::windows().into_iter().next() else {
            bail!("the Explorer taskbar was not found");
        };
        let state = Self::state();
        let executable =
            std::env::current_exe().context("the Tessera executable could not be located")?;
        // SAFETY: the call takes no arguments and cannot fail.
        let shell = unsafe { GetCurrentProcessId() };
        let watchdog = Command::new(executable)
            .arg(WATCHDOG_FLAG)
            .arg(shell.to_string())
            .arg(state.to_string())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .context("the taskbar restore watchdog could not be started")?;
        let _ = WATCHDOG.set(watchdog.id());

        Self::set_state(primary, state | ABS_AUTOHIDE);
        Self::keep_hidden();
        Ok(())
    }

    /// Starts `executable` as the shell that takes over from this one. It
    /// waits for this shell and for its watchdog to be gone, so that it
    /// finds the taskbar as Windows had it and the keys free.
    pub fn start_successor(executable: &Path) -> Result<()> {
        // SAFETY: the call takes no arguments and cannot fail.
        let shell = unsafe { GetCurrentProcessId() };
        let mut command = Command::new(executable);
        command.arg(SUCCESSOR_FLAG).arg(shell.to_string());
        if let Some(watchdog) = WATCHDOG.get() {
            command.arg(watchdog.to_string());
        }
        // The shell of the session hands that on, whatever the value of
        // Winlogon says.
        if shell_mode() {
            command.arg(SHELL_FLAG);
        }
        command
            .spawn()
            .context("the next shell could not be started")?;
        SUCCESSOR.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Whether this shell started the one that takes over from it, so
    /// that it quits without handing the session to anyone else.
    pub fn successor_started() -> bool {
        SUCCESSOR.load(Ordering::Relaxed)
    }

    /// Waits for the shell this one takes over from, if the arguments
    /// name one.
    pub fn wait_for_predecessor(mut arguments: impl Iterator<Item = String>) {
        if arguments.next().as_deref() != Some(SUCCESSOR_FLAG) {
            return;
        }
        for process in arguments.filter_map(|value| value.parse::<u32>().ok()) {
            wait_for(process);
        }
    }

    /// Hides taskbar windows again if Explorer has shown them.
    /// Shows the desktop by putting all windows aside, or brings them
    /// back if that was done last, as the corner of the taskbar does.
    /// Says whether there was a taskbar to ask.
    pub fn toggle_desktop() -> bool {
        let Some(taskbar) = Self::windows_of_class(w!("Shell_TrayWnd"))
            .into_iter()
            .next()
        else {
            return false;
        };
        // SAFETY: posting a message to a window of another program.
        unsafe { PostMessageW(Some(taskbar), WM_COMMAND, WPARAM(TOGGLE_DESKTOP), LPARAM(0)) }
            .is_ok()
    }

    pub fn keep_hidden() {
        for window in Self::windows() {
            // SAFETY: the calls tolerate a window that has been destroyed.
            unsafe {
                if IsWindowVisible(window).as_bool() {
                    let _ = ShowWindow(window, SW_HIDE);
                }
            }
        }
    }

    /// Runs the watchdog if the arguments ask for it, and reports whether
    /// they did. The caller must exit instead of starting the shell then.
    pub fn run_watchdog_if_requested(mut arguments: impl Iterator<Item = String>) -> bool {
        if arguments.next().as_deref() != Some(WATCHDOG_FLAG) {
            return false;
        }
        let shell = arguments.next().and_then(|value| value.parse::<u32>().ok());
        let state = arguments.next().and_then(|value| value.parse::<u32>().ok());
        if let (Some(shell), Some(state)) = (shell, state) {
            wait_for(shell);
            Self::restore(state);
        }
        true
    }

    fn restore(state: u32) {
        let windows = Self::windows();
        if let Some(&primary) = windows.first() {
            Self::set_state(primary, state);
        }
        for window in windows {
            // SAFETY: the call tolerates a window that has been destroyed.
            unsafe {
                let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
            }
        }
    }

    /// The primary taskbar window followed by those of secondary displays.
    fn windows() -> Vec<HWND> {
        let mut windows = Self::windows_of_class(w!("Shell_TrayWnd"));
        windows.extend(Self::windows_of_class(w!("Shell_SecondaryTrayWnd")));
        windows
    }

    /// The top-level windows of `class` that belong to other processes.
    pub(super) fn windows_of_class(class: PCWSTR) -> Vec<HWND> {
        let mut windows = Vec::new();
        let mut previous = None;
        // SAFETY: `class` is a NUL-terminated literal, and the search resumes
        // after the previously returned top-level window.
        while let Ok(window) = unsafe { FindWindowExW(None, previous, class, PCWSTR::null()) } {
            if window.is_invalid() {
                break;
            }
            // The shell's own notification area has the taskbar's class.
            let mut process = 0;
            // SAFETY: `process` outlives the call, which only reads window
            // state; so does the call that names this process.
            let own = unsafe {
                GetWindowThreadProcessId(window, Some(&raw mut process));
                process == GetCurrentProcessId()
            };
            if !own {
                windows.push(window);
            }
            previous = Some(window);
        }
        windows
    }

    fn state() -> u32 {
        let mut data = Self::appbar_data(HWND::default());
        // SAFETY: `data` is a correctly sized APPBARDATA.
        unsafe { SHAppBarMessage(ABM_GETSTATE, &raw mut data) as u32 }
    }

    fn set_state(taskbar: HWND, state: u32) {
        let mut data = Self::appbar_data(taskbar);
        data.lParam = LPARAM(state as isize);
        // SAFETY: `data` is a correctly sized APPBARDATA.
        unsafe {
            SHAppBarMessage(ABM_SETSTATE, &raw mut data);
        }
    }

    fn appbar_data(window: HWND) -> APPBARDATA {
        APPBARDATA {
            cbSize: size_of::<APPBARDATA>() as u32,
            hWnd: window,
            ..Default::default()
        }
    }
}
