// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Top-level application windows, as a task switcher would list them.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
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
    GetWindowPlacement, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, PostMessageW, SW_MINIMIZE, SW_RESTORE, SetForegroundWindow, ShowWindowAsync,
    WINDOWPLACEMENT, WM_CLOSE, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PWSTR};

use super::DisplayKey;

/// Identifies a top-level window for as long as it exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AppWindowId(isize);

impl AppWindowId {
    pub(super) fn hwnd(self) -> HWND {
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
        }
        self.activate();
    }

    /// Brings the window to the foreground, restoring it if minimized.
    /// Windows may refuse, as for [`Self::activate_or_minimize`].
    pub fn activate(self) {
        let hwnd = self.hwnd();
        // SAFETY: every call tolerates a handle whose window has since been
        // destroyed and fails without side effects.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindowAsync(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
    }

    /// Asks the window to close, as its close button does. The application
    /// may ask the user first, or refuse.
    pub fn close(self) {
        // SAFETY: posting to a window that has since been destroyed fails
        // without side effects.
        unsafe {
            let _ = PostMessageW(Some(self.hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    /// What the window's title bar says; empty if Windows does not tell.
    pub fn title(self) -> String {
        let mut buffer = [0u16; 512];
        // SAFETY: the buffer outlives the call, which writes no more than
        // its length.
        let length = unsafe { GetWindowTextW(self.hwnd(), &mut buffer) };
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
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
    /// The identifier of the packaged (Store) application the window
    /// belongs to, if it belongs to one.
    pub app_id: Option<String>,
}

impl AppWindow {
    /// What starts another window of the window's application and names
    /// it when it is pinned: the packaged application, or else the program
    /// file.
    pub fn program(&self) -> Option<PathBuf> {
        let app = self.app_id.as_deref().map(super::packaged::app_path);
        app.or_else(|| self.executable.clone())
    }

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
            let (executable, app_id) = program_of(hwnd, process);
            Some(Self {
                id: AppWindowId(hwnd.0 as isize),
                display: display_of(hwnd),
                executable,
                app_id,
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

/// The image of `process`, when the system allows reading it, and the
/// identifier of the packaged application its window `hwnd` belongs to.
fn program_of(hwnd: HWND, process: u32) -> (Option<PathBuf>, Option<String>) {
    // SAFETY: the buffer and its length describe the same allocation, and the
    // process handle is closed before returning.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process) else {
            return (None, None);
        };
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        );
        let executable = result
            .ok()
            .map(|()| PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize])));
        let app_id = super::packaged::app_id_of(hwnd, handle, executable.as_deref());
        let _ = CloseHandle(handle);
        (executable, app_id)
    }
}
