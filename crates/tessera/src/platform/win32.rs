// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Win32 window behavior that GPUI does not expose.

#![allow(unsafe_code)]

mod accent;
mod account;
mod app_commands;
mod app_windows;
mod appbar;
mod audio;
mod backdrop;
mod bin;
mod bluetooth;
mod brightness;
mod clipboard_watch;
mod commands;
mod connections;
mod desktops;
mod devices;
mod display_config;
mod display_mode;
mod drag;
mod drop_target;
mod execute;
mod explorer_view;
mod file_op;
mod files;
mod hdr;
mod hotkeys;
mod hotspot;
mod icon;
mod index;
mod input_names;
mod jump;
mod key_locks;
mod keyboard;
mod media;
mod move_watch;
mod network;
mod night_light;
mod notifications;
mod open_with;
mod pace;
mod packaged;
mod passage;
mod plans;
mod power;
mod power_mode;
mod power_settings;
mod power_warnings;
mod privacy;
mod processes;
mod program_path;
mod quick_access;
mod radios;
mod registry;
mod renaming;
mod run_history;
mod screenshot;
mod send_to;
mod shell_host;
mod shell_item;
mod shell_items;
mod shell_menu;
mod shell_mode;
mod shell_new;
mod shell_thread;
mod shortcuts;
mod show_desktop;
mod snap;
mod startup;
mod switching;
mod system;
mod system_wallpaper;
mod taskbar;
mod terminal;
mod thumbnail;
mod tray;
mod veil;
mod video;
mod wic;
mod wifi;
mod window_ops;
mod windows_key;
mod winlogon;

pub use accent::{accent_color, light_mode, set_light_mode};
pub use account::{Account, Place};
pub use app_commands::AppCommand;
pub use app_windows::{AppWindow, AppWindowId, WindowEvents, pointer_at, pointer_held};
pub use audio::spatial_sound;
pub use audio::{AppSound, AudioDevice, AudioOutput};
pub use backdrop::display_at;
pub use backdrop::{
    BackdropPicture, DesktopBackdrop, desktop_look, desktop_pixels, display_frames,
    read_desktop_if_requested,
};
pub use bin::RecycleBin;
pub use bin::{BinUsage, RecycledItem};
pub use bluetooth::{Bluetooth, PairedDevice};
pub use brightness::BrightnessLevel;
pub use brightness::ScreenBrightness;
pub use clipboard_watch::{Clip, ClipboardWatch, Offer, paste_into};
pub use commands::Commands;
pub use connections::{Carrier, Connection, Vpn, VpnState, Wireless};
pub use desktops::VirtualDesktops;
pub use devices::{
    DeviceChanges, EjectRefusal, Ejection, RemovableDevice, RemovableDrive, Removables,
};
pub use display_config::{DisplayOutput, DisplaySetup, Rotation, VideoMode, hertz_text};
pub use display_mode::DisplayMode;
pub use drag::DragOut;
pub use drop_target::{DropEvent, DropTarget, DropZone, DropZones};
pub use execute::Process;
pub use execute::{Launch, Show};
pub use explorer_view::{
    Attributes, ExplorerSettings, NameRules, localized_candidates, shell_names,
};
pub use file_op::{FileDeed, FileOperation, LinkTarget, Made, Outcome, UndoJournal, unique_name};
pub use files::{
    Drive, DriveKind, FileClipboard, FileWork, disk_space, is_hidden, reveal_in_explorer,
    show_properties,
};
pub use hdr::Hdr;
pub use hotkeys::{Hotkey, Hotkeys, Modifiers};
pub use hotspot::{Hotspot, HotspotBand, HotspotInfo, HotspotState};
pub use icon::IconPixels;
pub use index::SearchIndex;
pub use input_names::InputName;
pub use input_names::OwnInput;
pub use jump::{JumpTask, RecentDocument, app_id_of_program};
pub use key_locks::{KeyLocks, window_in_front};
pub use keyboard::{InputLanguage, Typing};
pub use media::SessionKey;
pub use media::{Media, MediaCommand, NowPlaying, Position};
pub use media::{Repeat, SessionInfo};
pub use move_watch::{MoveEvent, MoveWatch};
pub use network::{Link, Network};
pub use night_light::{COLDEST, GammaNight, WARMEST, WindowsNight, WindowsNightLight};
pub use notifications::Occupation;
pub use notifications::{Notification, Notifications};
pub use open_with::{OpenWithHandler, choose_app};
pub use pace::Pace;
pub use packaged::{app_id_in, app_path};
pub use passage::Passage;
pub use plans::PowerPlan;
pub use power::PowerAction;
pub use power::PowerChoices;
pub use power_mode::{PowerMode, PowerModeState};
pub use power_settings::{
    ButtonAction, PowerChange, PowerSetting, PowerSettings, PowerState, SleepBlock, Supply,
};
pub use power_warnings::{BatteryChanges, BatteryLevels, CriticalAction};
pub use privacy::{Capability, CapabilityUse, PrivacyWatch};
pub use processes::RunningProcess;
pub use program_path::{expand_variables, find_program, recent_folder};
pub use quick_access::QuickFolder;
pub use quick_access::{QuickAccess, ShellPlace, onedrive, recent_files, wsl_distributions};
pub use radios::{RadioId, RadioInfo, RadioType, Radios, airplane_mode};
pub use renaming::rename_new;
pub use run_history::RunHistory;
pub use screenshot::save_screenshot;
pub use send_to::SendToTarget;
pub use shell_host::{HostOptions, ShellEvent, ShellHost};
pub use shell_item::ShellId;
pub use shell_menu::{Intercept, MenuOutcome, ShellMenu, ShellVerb};
pub use shell_mode::started_as_shell;
pub use shell_mode::{explorer_running, shell_mode};
pub use shell_new::NewTemplate;
pub use shell_thread::ShellThread;
pub use shortcuts::Shortcut;
pub use show_desktop::ShowDesktop;
pub use snap::on_resizing_border;
pub use snap::{Display, MaximizeHover, WindowArranging, front_fills, shift_held};
pub use startup::{SessionStartup, StartupEntry, StartupSource, SwitchRefusal};
pub use system::{Battery, MemoryLoad, NetworkTraffic, ProcessorLoad, memory_held};
pub use system::{GpuLoad, GpuUsage};
pub use system_wallpaper::{SystemWallpaper, WallpaperPosition, user_locale};
pub use taskbar::Taskbar;
pub use terminal::{terminal, windows_terminal};
pub use thumbnail::Thumbnail;
pub use tray::{Balloon, BalloonEvent};
pub use tray::{Tray, TrayButton, TrayIcon};
pub use veil::Veil;
pub use video::{DesktopVideo, VideoState};
pub use wic::decode_with_wic;
pub use wifi::{Wifi, WifiNetwork, WifiReach, WifiSecurity, password_fits};
pub use windows_key::DesktopClose;
pub use windows_key::{ChoiceKey, TakenKeys, WindowsKey};
pub use winlogon::{
    ExplorerShell, NextShell, SHELL_FLAG, ShellRegistration, attach_console, is_safe_mode,
    own_program_name, recovery_keys_held,
};

use std::ffi::c_void;
use std::mem::size_of;

use anyhow::{Context as _, Result, bail};
use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL, MONITORINFO,
    MonitorFromPoint, MonitorFromWindow,
};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Shell::{
    ABE_BOTTOM, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, APPBARDATA, SHAppBarMessage,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CBT_CREATEWNDW, CallNextHookEx, HCBT_CREATEWND, HHOOK, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_CBT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindowLongPtrW,
    GetWindowRect, GetWindowThreadProcessId, HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOPMOST, IsIconic,
    IsWindowVisible, LWA_ALPHA, PostMessageW, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow,
    SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE,
    WM_APP, WM_MOUSEMOVE, WM_SIZE, WS_CAPTION, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE, WindowFromPoint,
};

/// Classes of windows that cover a display without being an application
/// shown full screen: the desktop, and the task view and window switcher
/// of Windows.
const NOT_FILLING: [&str; 4] = [
    "Progman",
    "WorkerW",
    "XamlExplorerHostIslandWindow",
    "MultitaskingViewFrame",
];

/// Message the system posts to a window that reserves a screen edge.
const EDGE_NOTIFICATION: u32 = WM_APP + 1;

/// A screen edge a shell surface can reserve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreenEdge {
    Top,
    Bottom,
}

impl ScreenEdge {
    fn as_appbar_edge(self) -> u32 {
        match self {
            Self::Top => ABE_TOP,
            Self::Bottom => ABE_BOTTOM,
        }
    }

    /// Shrinks `area` to a strip `thickness` deep along this edge.
    fn clamp(self, area: &mut RECT, thickness: i32) {
        match self {
            Self::Top => area.bottom = area.top + thickness,
            Self::Bottom => area.top = area.bottom - thickness,
        }
    }
}

/// A rectangle on the virtual screen, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl From<RECT> for ScreenRect {
    fn from(rect: RECT) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }
}

impl ScreenRect {
    /// Whether all of `other` lies within this rectangle.
    pub fn covers(self, other: Self) -> bool {
        self.left <= other.left
            && self.top <= other.top
            && self.right >= other.right
            && self.bottom >= other.bottom
    }
}

/// Whether the pointer is on a window this program keeps above the others:
/// a surface, a popup or a menu, but not a card on the desktop.
pub fn pointer_on_own_window() -> bool {
    let Some((x, y)) = pointer_at() else {
        return false;
    };
    let mut process = 0;
    // SAFETY: the calls only read which window is at a point and whose it
    // is; `process` outlives the call that fills it.
    unsafe {
        let window = WindowFromPoint(POINT { x, y });
        GetWindowThreadProcessId(window, Some(&raw mut process));
        let above = GetWindowLongPtrW(window, GWL_EXSTYLE) & WS_EX_TOPMOST.0 as isize != 0;
        above && process == GetCurrentProcessId()
    }
}

/// The native window behind a GPUI shell surface.
#[derive(Clone, Copy, Debug)]
pub struct NativeSurface {
    hwnd: HWND,
}

/// Identifies a display for as long as the display configuration is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DisplayKey(isize);

impl DisplayKey {
    /// The display the pointer is on, or the nearest one to it.
    pub fn under_pointer() -> Option<Self> {
        let (x, y) = pointer_at()?;
        // SAFETY: the call only reads the arrangement of the displays.
        let display = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
        (!display.is_invalid()).then_some(Self(display.0 as isize))
    }

    /// The part of the display that the surfaces at its edges leave to
    /// windows.
    pub fn work_area(self) -> Option<ScreenRect> {
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `info` is correctly sized, and a display that is gone
        // makes the call fail.
        let read = unsafe { GetMonitorInfoW(HMONITOR(self.0 as *mut c_void), &raw mut info) };
        read.as_bool().then(|| info.rcWork.into())
    }

    /// The display that holds most of `hwnd`, or the nearest one.
    fn of_window(hwnd: HWND) -> Self {
        // SAFETY: the call only reads window state and tolerates a handle
        // whose window has since been destroyed.
        Self(unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }.0 as isize)
    }
}

/// The side of a window that is made while a [`SmallBirth`] lives, in
/// pixels.
const BORN_SIDE: i32 = 64;

/// While it lives, the windows this thread makes are made small.
///
/// GPUI has Windows choose how large a window is made, which is most of
/// a display, and what draws the window holds pictures of that size from
/// then: some fifty megabytes the process is charged with until the
/// window has been given its size and they were let go, seconds later.
/// A window of the shell is given its place right after it is made, and
/// so nothing is lost by making it small.
pub struct SmallBirth(HHOOK);

impl SmallBirth {
    /// Begins to have windows made small. `None` where Windows refuses,
    /// and windows are then made as large as before.
    pub fn begin() -> Option<Self> {
        // SAFETY: the hook is for this thread alone and is taken away
        // when the value is dropped, on this thread.
        let hook =
            unsafe { SetWindowsHookExW(WH_CBT, Some(born_small), None, GetCurrentThreadId()) };
        hook.ok().map(Self)
    }
}

impl Drop for SmallBirth {
    fn drop(&mut self) {
        // SAFETY: the hook is the one `begin` set.
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
    }
}

/// The class of the windows GPUI makes for the application.
const GPUI_WINDOW: &str = "Zed::Window";

/// Has a window of GPUI that is about to be made be made [`BORN_SIDE`]
/// wide and high. Other windows, as the one that dims a display, are
/// made as they ask.
unsafe extern "system" fn born_small(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HCBT_CREATEWND as i32 {
        // SAFETY: for this code Windows gives what the window is made
        // with, which may be changed, for as long as the call lasts.
        unsafe {
            let mut name = [0u16; 64];
            let window = HWND(wparam.0 as *mut c_void);
            let length = GetClassNameW(window, &mut name).max(0) as usize;
            let made = (lparam.0 as *const CBT_CREATEWNDW).as_ref();
            if String::from_utf16_lossy(&name[..length]) == GPUI_WINDOW
                && let Some(how) = made.and_then(|made| made.lpcs.as_mut())
            {
                (how.cx, how.cy) = (BORN_SIDE, BORN_SIDE);
            }
        }
    }
    // SAFETY: what was given is passed on as it is.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

impl NativeSurface {
    /// The display the surface is on.
    pub fn display(self) -> DisplayKey {
        DisplayKey::of_window(self.hwnd)
    }

    /// Resolves the native window of a GPUI window.
    pub fn of(window: &Window) -> Result<Self> {
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("the window has no native handle: {error}"))?;
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => Ok(Self {
                hwnd: HWND(handle.hwnd.get() as *mut c_void),
            }),
            other => bail!("unsupported native window handle: {other:?}"),
        }
    }

    /// Turns the window into a shell surface: no caption or frame, above
    /// ordinary windows, and clicks on it do not take keyboard focus away
    /// from the active application.
    pub fn adopt_as_shell_surface(self) -> Result<()> {
        self.adopt(WS_EX_NOACTIVATE)
    }

    /// Turns the window into something the shell lays over the screen: no
    /// caption or frame, above ordinary windows, never in front, and the
    /// pointer goes through it to what is beneath.
    pub fn adopt_as_overlay(self) -> Result<()> {
        self.adopt(WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT)?;
        // SAFETY: `hwnd` belongs to a live window owned by this thread. A
        // layered window is drawn once it is told how opaque it is.
        unsafe { SetLayeredWindowAttributes(self.hwnd, COLORREF(0), 255, LWA_ALPHA) }
            .context("the window could not be made to let the pointer through")
    }

    /// Turns the window into a popup of the shell: no caption or frame,
    /// above ordinary windows, and not listed among application windows.
    /// Unlike a surface it can take keyboard focus.
    pub fn adopt_as_popup(self) -> Result<()> {
        self.adopt(WS_EX_TOOLWINDOW)
    }

    /// Turns the window into a card on the desktop: no caption or frame,
    /// below every other window, not listed among application windows, and
    /// clicks on it neither take keyboard focus nor raise it.
    pub fn adopt_as_card(self) -> Result<()> {
        self.adopt(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE)?;
        self.keep_above(false);
        Ok(())
    }

    /// The window's bounds on the screen.
    pub fn frame(self) -> Result<ScreenRect> {
        let mut rect = RECT::default();
        // SAFETY: `rect` outlives the call, which only reads window state.
        unsafe { GetWindowRect(self.hwnd, &mut rect) }
            .context("the window bounds could not be read")?;
        Ok(rect.into())
    }

    /// The bounds of the display the window is on.
    pub fn display_frame(self) -> Result<ScreenRect> {
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `monitor` is a correctly sized structure that outlives
        // the calls, which only read window and display state.
        let read = unsafe {
            let display = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            GetMonitorInfoW(display, &mut monitor).as_bool()
        };
        if !read {
            bail!("the window's display could not be read");
        }
        Ok(monitor.rcMonitor.into())
    }

    /// The strip `thickness` physical pixels deep at `edge` of the
    /// surface's display.
    pub fn strip(self, edge: ScreenEdge, thickness: i32) -> Result<ScreenRect> {
        let display = self.display_frame()?;
        let mut strip = RECT {
            left: display.left,
            top: display.top,
            right: display.right,
            bottom: display.bottom,
        };
        edge.clamp(&mut strip, thickness);
        Ok(strip.into())
    }

    /// Whether the pointer is in the strip `thickness` physical pixels deep
    /// at `edge` of the surface's display.
    pub fn pointer_in_strip(self, edge: ScreenEdge, thickness: i32) -> bool {
        let (Ok(strip), Some((x, y))) = (self.strip(edge, thickness), pointer_at()) else {
            return false;
        };
        (strip.left..strip.right).contains(&x) && (strip.top..strip.bottom).contains(&y)
    }

    /// Moves and resizes the window without activating it.
    ///
    /// The window is told its new size once more through its queue. GPUI
    /// drops the size it is told while the application is in use, as it is
    /// whenever a view places its window, and then goes on looking for
    /// what the pointer is on within the size it knew before.
    pub fn place(self, frame: ScreenRect) -> Result<()> {
        let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
        let before = self.frame().ok();
        // SAFETY: `hwnd` belongs to a live window owned by this thread.
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                frame.left,
                frame.top,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
        }
        .context("the window could not be moved")?;
        let same = before.is_some_and(|before| {
            (before.right - before.left, before.bottom - before.top) == (width, height)
        });
        if !same && (1..=0xffff).contains(&width) && (1..=0xffff).contains(&height) {
            // The two sides of the client area, as `WM_SIZE` carries them:
            // a window of Tessera has no frame around it.
            let size = (height as isize) << 16 | width as isize;
            // SAFETY: posting to a window that has since been destroyed
            // fails without side effects.
            let _ = unsafe { PostMessageW(Some(self.hwnd), WM_SIZE, WPARAM(0), LPARAM(size)) };
        }
        Ok(())
    }

    /// Shows the window, which was opened hidden so that it is first
    /// seen without a frame and where it belongs. With `focus` it also
    /// asks for the keyboard, as [`Self::activate`] does.
    pub fn reveal(self, focus: bool) {
        let how = if focus { SW_SHOW } else { SW_SHOWNOACTIVATE };
        // SAFETY: `hwnd` belongs to a live window owned by this thread.
        unsafe {
            let _ = ShowWindow(self.hwnd, how);
        }
        if focus {
            self.activate();
        }
    }

    /// Hides the window, which stays what it is.
    pub fn conceal(self) {
        // SAFETY: `hwnd` belongs to a live window owned by this thread.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Asks for keyboard input to go to the window. The system grants that
    /// to the process that received the last input, as it has after a
    /// click on one of its windows.
    pub fn activate(self) {
        // SAFETY: `hwnd` belongs to a live window owned by this thread.
        unsafe {
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    /// Whether the window in front fills the whole display of this one, as
    /// a game or a video played full screen does. The desktop and the
    /// screens of Windows that show all windows do not count, nor does a
    /// window of Tessera. A window without a caption counts when it fills
    /// the room left to windows: Windows moves a game out of an edge that
    /// gets reserved while it runs, and it stays there.
    pub fn display_is_filled(self) -> bool {
        let windows = AppWindow::enumerate().into_iter();
        self.filled_among(windows.map(|window| HWND(window.id.raw() as *mut c_void)))
    }

    /// Whether an application fills the display of this window, as
    /// [`Self::display_is_filled`] says, for a thread that must never wait
    /// for another thread of the shell, as the tray thread: it reads no
    /// list shared with them and asks none of their windows anything.
    fn display_is_filled_apart(self) -> bool {
        let windows = AppWindow::handles().into_iter();
        self.filled_among(windows.map(|(hwnd, _)| hwnd))
    }

    /// Whether the window in front, or the first of `windows`, application
    /// windows topmost first, that lies on top on this display fills it.
    fn filled_among(self, mut windows: impl Iterator<Item = HWND>) -> bool {
        // SAFETY: the call only reads which window is in front.
        let front = unsafe { GetForegroundWindow() };
        if self.is_filled_by(front) {
            return true;
        }
        // An application shown full screen keeps its display while the
        // user works on another one: the window that lies on top of the
        // others on this display says.
        // SAFETY: the calls only read the state of windows.
        let display = unsafe { MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST) };
        let on_top = windows.find(|&hwnd| {
            // SAFETY: as above; a window that went away is on no display,
            // and a minimized one covers nothing.
            let (there, pinned) = unsafe {
                let there = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL) == display;
                let extended = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                (
                    there && !IsIconic(hwnd).as_bool(),
                    extended & WS_EX_TOPMOST.0 != 0,
                )
            };
            // A window kept above the others covers no application.
            there && !pinned
        });
        on_top.is_some_and(|window| self.is_filled_by(window))
    }

    /// Whether `window` is one of an application that fills the display of
    /// this surface.
    fn is_filled_by(self, window: HWND) -> bool {
        let front = window;
        // SAFETY: the calls only read the state of windows; a window that
        // went away meanwhile makes them fail, which counts as not filled.
        unsafe {
            if front.is_invalid() || front == self.hwnd || !IsWindowVisible(front).as_bool() {
                return false;
            }
            let mut process = 0;
            GetWindowThreadProcessId(front, Some(&raw mut process));
            if process == std::process::id() {
                return false;
            }
            let mut name = [0u16; 64];
            let length = GetClassNameW(front, &mut name).max(0) as usize;
            let class = String::from_utf16_lossy(&name[..length]);
            if NOT_FILLING.contains(&class.as_str()) {
                return false;
            }
            let display = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            if MonitorFromWindow(front, MONITOR_DEFAULTTONEAREST) != display {
                return false;
            }
            let mut monitor = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let mut frame = RECT::default();
            if !GetMonitorInfoW(display, &mut monitor).as_bool()
                || GetWindowRect(front, &mut frame).is_err()
            {
                return false;
            }
            let captioned =
                GetWindowLongPtrW(front, GWL_STYLE) as u32 & WS_CAPTION.0 == WS_CAPTION.0;
            let frame = ScreenRect::from(frame);
            frame.covers(monitor.rcMonitor.into())
                || (!captioned && frame.covers(monitor.rcWork.into()))
        }
    }

    /// Puts the window above ordinary windows, or below all of them.
    pub fn keep_above(self, above: bool) {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        // SAFETY: `hwnd` belongs to a live window owned by this thread, and
        // the calls only change its place in the order of windows.
        unsafe {
            if above {
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
            } else {
                let _ = SetWindowPos(self.hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
                let _ = SetWindowPos(self.hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, flags);
            }
        }
    }

    /// Tells the window that the pointer is just beyond its top left
    /// corner, unless the pointer is on the window.
    pub fn forget_pointer(self) {
        /// The position -1, -1 as a mouse message carries it.
        const OUTSIDE: isize = 0xffff_ffff;
        let mut pointer = POINT::default();
        // SAFETY: `pointer` outlives the call that fills it, and posting to
        // a window that has since been destroyed fails without side effects.
        unsafe {
            if GetCursorPos(&raw mut pointer).is_ok() && WindowFromPoint(pointer) == self.hwnd {
                return;
            }
            let _ = PostMessageW(Some(self.hwnd), WM_MOUSEMOVE, WPARAM(0), LPARAM(OUTSIDE));
        }
    }

    /// Removes the caption and frame, has `extended_style` be what the
    /// window has of the styles the shell gives, and raises the window above
    /// ordinary ones. A window that was adopted as something else before is
    /// what it is adopted as now.
    fn adopt(self, extended_style: WINDOW_EX_STYLE) -> Result<()> {
        // SAFETY: `hwnd` belongs to a live window owned by this thread, and
        // the calls only change its styles and z-order.
        unsafe {
            // A window that is still hidden stays so: one that only says
            // it is visible would not be shown when asked to.
            let shown = GetWindowLongPtrW(self.hwnd, GWL_STYLE) & WS_VISIBLE.0 as isize;
            SetWindowLongPtrW(self.hwnd, GWL_STYLE, WS_POPUP.0 as isize | shown);
            let given = WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT;
            let extended = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) & !(given.0 as isize);
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, extended | extended_style.0 as isize);
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
        }
        .context("the window styles could not be applied")
    }

    /// Reserves a strip `thickness` physical pixels deep at `edge` of the
    /// surface's display and moves the surface into it, so maximized windows
    /// stop at the surface instead of sliding under it.
    ///
    /// The reservation ends when the returned guard is dropped. Windows also
    /// ends it when the window is destroyed, including after a crash.
    pub fn reserve_edge(self, edge: ScreenEdge, thickness: i32) -> Result<EdgeReservation> {
        let mut data = self.appbar_data();
        data.uCallbackMessage = EDGE_NOTIFICATION;
        // SAFETY: `data` is a correctly sized APPBARDATA that outlives the
        // call, and `hwnd` is a live window owned by this thread.
        if unsafe { SHAppBarMessage(ABM_NEW, &mut data) } == 0 {
            bail!("the system refused to register the surface as an edge bar");
        }
        let reservation = EdgeReservation { surface: self };
        reservation.occupy(edge, thickness)?;
        Ok(reservation)
    }

    fn appbar_data(self) -> APPBARDATA {
        APPBARDATA {
            cbSize: size_of::<APPBARDATA>() as u32,
            hWnd: self.hwnd,
            ..Default::default()
        }
    }
}

/// Keeps a screen edge reserved for a surface until dropped.
#[derive(Debug)]
pub struct EdgeReservation {
    surface: NativeSurface,
}

impl EdgeReservation {
    /// A number that grows whenever the surfaces are to take their place
    /// again with [`Self::occupy`]: another edge bar moved, or what keeps
    /// the bars changed, as when Explorer's taskbar went or came back.
    pub fn generation() -> u64 {
        appbar::places()
    }

    /// Makes the reserved strip `thickness` physical pixels deep at `edge`
    /// of the surface's display and moves the surface into it. The surface
    /// registers again first, in case what keeps the bars changed since;
    /// one that is registered already is refused, which changes nothing.
    pub fn occupy(&self, edge: ScreenEdge, thickness: i32) -> Result<()> {
        let hwnd = self.surface.hwnd;
        let mut data = self.surface.appbar_data();
        data.uCallbackMessage = EDGE_NOTIFICATION;
        // SAFETY: `data` is a correctly sized APPBARDATA that outlives the
        // call, and `hwnd` is a live window owned by this thread.
        unsafe { SHAppBarMessage(ABM_NEW, &mut data) };
        data.uEdge = edge.as_appbar_edge();

        // SAFETY: `data` and `monitor` are correctly sized structures that
        // outlive every call, and `hwnd` is a live window owned by this thread.
        unsafe {
            let mut monitor = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let display = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            if !GetMonitorInfoW(display, &mut monitor).as_bool() {
                bail!("the surface's display could not be read");
            }
            data.rc = monitor.rcMonitor;
            SHAppBarMessage(ABM_QUERYPOS, &mut data);
            edge.clamp(&mut data.rc, thickness);
            SHAppBarMessage(ABM_SETPOS, &mut data);

            let RECT {
                left,
                top,
                right,
                bottom,
            } = data.rc;
            SetWindowPos(
                hwnd,
                None,
                left,
                top,
                right - left,
                bottom - top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
            .context("the surface could not be moved into its reserved area")
        }
    }
}

impl Drop for EdgeReservation {
    fn drop(&mut self) {
        let mut data = self.surface.appbar_data();
        // SAFETY: `data` is a correctly sized APPBARDATA; removing a
        // registration for an already destroyed window is a no-op.
        unsafe {
            SHAppBarMessage(ABM_REMOVE, &mut data);
        }
    }
}
