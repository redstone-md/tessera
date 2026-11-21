// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Top-level application windows, as a task switcher would list them.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromRect};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_CLOAKED, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_NAMECHANGE,
    EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, OBJID_WINDOW,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetWindow, GetWindowLongPtrW,
    GetWindowPlacement, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SW_MINIMIZE, SW_RESTORE, SetForegroundWindow, ShowWindowAsync, WINDOWPLACEMENT,
    WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PWSTR};

use super::DisplayKey;

/// Identifies a top-level window for as long as it exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AppWindowId(isize);

impl AppWindowId {
    fn hwnd(self) -> HWND {
        HWND(self.0 as *mut c_void)
    }

    /// The window that currently receives keyboard input, if any.
    pub fn foreground() -> Option<Self> {
        // SAFETY: the call takes no arguments and only reads system state.
        let hwnd = unsafe { GetForegroundWindow() };
        (!hwnd.is_invalid()).then_some(Self(hwnd.0 as isize))
    }

    /// Brings the window to the foreground, restoring it if minimized. A
    /// window that is already in the foreground is minimized instead.
    ///
    /// Windows may refuse either request, for example for a window of an
    /// elevated process; there is no reliable signal to report in that case.
    pub fn activate_or_minimize(self) {
        let hwnd = self.hwnd();
        // SAFETY: every call tolerates a handle whose window has since been
        // destroyed and fails without side effects.
        unsafe {
            if Self::foreground() == Some(self) && !IsIconic(hwnd).as_bool() {
                let _ = ShowWindowAsync(hwnd, SW_MINIMIZE);
                return;
            }
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindowAsync(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// Counts what happened to windows since `WindowEvents::watch`.
static HAPPENED: AtomicU64 = AtomicU64::new(0);

/// Tells when the list of application windows may have changed, so that it
/// is read again only then.
pub struct WindowEvents;

impl WindowEvents {
    /// The ranges of events that can change the list or the foreground
    /// window: from a new foreground window to the end of a move, a resize
    /// or a minimized state; windows destroyed, shown and hidden; renamed;
    /// cloaked and uncloaked.
    const WATCHED: [(u32, u32); 4] = [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND),
        (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE),
        (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
    ];

    /// Starts counting, once. Windows delivers the events through the
    /// message queue of the calling thread, which therefore has to be one
    /// that handles messages, and delivers them for as long as the process
    /// runs. Whether Windows accepted every request is returned.
    pub fn watch() -> bool {
        static STARTED: Once = Once::new();
        static WATCHING: AtomicU64 = AtomicU64::new(0);
        STARTED.call_once(|| {
            let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
            let accepted = Self::WATCHED.iter().all(|&(first, last)| {
                // SAFETY: `happened` is a function of this program, which
                // is never unloaded; no module is injected anywhere.
                let hook =
                    unsafe { SetWinEventHook(first, last, None, Some(happened), 0, 0, flags) };
                !hook.is_invalid()
            });
            WATCHING.store(accepted.into(), Ordering::Relaxed);
        });
        WATCHING.load(Ordering::Relaxed) != 0
    }

    /// A number that changes whenever something happened to a window.
    pub fn generation() -> u64 {
        HAPPENED.load(Ordering::Relaxed)
    }
}

unsafe extern "system" fn happened(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    // Events of the parts of a window, such as its scroll bars, and of the
    // pointer do not change the list.
    if object == OBJID_WINDOW.0 && child == 0 {
        HAPPENED.fetch_add(1, Ordering::Relaxed);
    }
}

/// A window the user would expect to find in a task switcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppWindow {
    pub id: AppWindowId,
    /// The display the window is on, or would return to when restored.
    pub display: DisplayKey,
    /// The image of the owning process, when the system allows reading it.
    pub executable: Option<PathBuf>,
}

impl AppWindow {
    /// Lists application windows in z-order, topmost first.
    pub fn enumerate() -> Vec<Self> {
        let mut handles: Vec<HWND> = Vec::new();
        // SAFETY: `collect` receives the pointer to `handles`, which outlives
        // the synchronous enumeration.
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&raw mut handles as isize));
        }
        handles.into_iter().filter_map(Self::describe).collect()
    }

    fn describe(hwnd: HWND) -> Option<Self> {
        // SAFETY: the calls only read window state and tolerate a handle
        // whose window has since been destroyed.
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || GetWindowTextLengthW(hwnd) <= 0 {
                return None;
            }
            if GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
                return None;
            }
            let extended = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if extended & WS_EX_TOOLWINDOW.0 != 0 {
                return None;
            }
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                (&raw mut cloaked).cast(),
                size_of::<u32>() as u32,
            );
            if cloaked != 0 {
                return None;
            }
            let mut process = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&raw mut process));
            if process == GetCurrentProcessId() {
                return None;
            }
            Some(Self {
                id: AppWindowId(hwnd.0 as isize),
                display: display_of(hwnd),
                executable: executable_of(process),
            })
        }
    }
}

unsafe extern "system" fn collect(hwnd: HWND, state: LPARAM) -> BOOL {
    // SAFETY: `state` is the pointer `AppWindow::enumerate` passed in.
    let handles = unsafe { &mut *(state.0 as *mut Vec<HWND>) };
    handles.push(hwnd);
    true.into()
}

fn display_of(hwnd: HWND) -> DisplayKey {
    // SAFETY: `placement` is correctly sized, and the calls only read window
    // state and tolerate a handle whose window has since been destroyed.
    unsafe {
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // A minimized window is parked off-screen; use where it restores to.
        if IsIconic(hwnd).as_bool() && GetWindowPlacement(hwnd, &raw mut placement).is_ok() {
            let display = MonitorFromRect(
                &raw const placement.rcNormalPosition,
                MONITOR_DEFAULTTONEAREST,
            );
            return DisplayKey(display.0 as isize);
        }
    }
    DisplayKey::of_window(hwnd)
}

fn executable_of(process: u32) -> Option<PathBuf> {
    // SAFETY: the buffer and its length describe the same allocation, and the
    // process handle is closed before returning.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process).ok()?;
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(
            &buffer[..length as usize],
        )))
    }
}
