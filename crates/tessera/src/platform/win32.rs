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
mod autoplay;
mod backdrop;
mod bin;
mod bluetooth;
mod brightness;
mod clipboard_watch;
mod commands;
mod connections;
mod desktop_folder;
mod desktops;
mod devices;
mod display_config;
mod display_mode;
mod display_names;
mod drag;
mod drop_target;
mod execute;
mod explorer_view;
mod file_jobs;
mod file_op;
mod files;
mod folder_handler;
mod fonts;
mod hdr;
mod hotkeys;
mod hotspot;
mod icon;
mod ime;
mod index;
mod input_names;
mod jump;
mod key_locks;
mod keyboard;
mod lasting;
mod media;
mod mouse_buttons;
mod mouse_hook;
mod move_watch;
mod net_drives;
mod network;
mod night_light;
mod notifications;
mod open_with;
mod overlays;
mod pace;
mod packaged;
mod passage;
mod peek;
mod plans;
mod power;
mod power_mode;
mod power_settings;
mod power_warnings;
mod preview_host;
mod privacy;
mod processes;
mod program_path;
mod properties;
mod quick_access;
mod radios;
mod registry;
mod renaming;
mod run_history;
mod screenshot;
mod send_to;
mod share;
mod shell_host;
mod shell_item;
mod shell_items;
mod shell_menu;
mod shell_mode;
mod shell_new;
mod shell_thread;
mod shell_windows;
mod shortcuts;
mod show_desktop;
mod snap;
mod spell;
mod startup;
mod switching;
mod system;
mod system_wallpaper;
mod taskbar;
mod taskbar_list;
mod terminal;
mod thumbnail;
mod thumbnails;
mod tile_border;
mod touch_keyboard;
mod tray;
mod uninstall;
mod uri_scheme;
mod veil;
mod video;
mod wallpaper_server;
mod wheel_lines;
mod wic;
mod wifi;
mod window_ops;
mod windows_key;
mod winlogon;
mod zones;

pub use accent::{accent_color, light_mode, set_light_mode};
pub use account::{Account, Place};
pub use app_commands::AppCommand;
pub use app_windows::{AppWindow, AppWindowId, WindowEvents, pointer_at, pointer_held};
pub use audio::spatial_sound;
pub use audio::{AppSound, AudioDevice, AudioOutput};
pub use autoplay::{AutoPlay, AutoPlayHandler, AutoPlayOffer, Chosen, Invoke, Medium};
pub use backdrop::display_at;
pub use backdrop::{
    BackdropPicture, DesktopBackdrop, desktop_look, desktop_pixels, display_frames,
    read_desktop_if_requested,
};
pub use bin::RecycleBin;
pub use bin::{BinUsage, RecycledItem};
pub use bluetooth::{Bluetooth, PairedDevice};
pub use bluetooth::{BluetoothScan, FoundDevice, Pairing, PairingFailure, PairingPrompt};
pub use brightness::BrightnessLevel;
pub use brightness::ScreenBrightness;
pub use clipboard_watch::{Clip, ClipboardWatch, Offer, paste_into};
pub use commands::Commands;
pub use connections::{Carrier, Connection, Vpn, VpnState, Wireless};
pub use desktop_folder::DesktopOptions;
pub use desktop_folder::{
    DesktopFolders, DesktopLayer, ExplorerIcons, FolderWatch, LayerKind, VirtualKind, layer_frame,
    virtual_items,
};
pub use desktops::VirtualDesktops;
pub use desktops::{DesktopSurvey, DesktopWindow};
pub use devices::{
    DeviceChanges, EjectRefusal, Ejection, RemovableDevice, RemovableDrive, Removables,
};
pub use display_config::{DisplayOutput, DisplaySetup, Rotation, VideoMode, hertz_text};
pub use display_mode::DisplayMode;
pub use display_names::{DisplayName, display_device, display_names};
pub use drag::DragOut;
pub use drop_target::{DropEvent, DropTarget, DropZone, DropZones};
pub use execute::Process;
pub use execute::{Launch, Show};
pub use explorer_view::{
    Attributes, ExplorerSettings, NameRules, localized_candidates, shell_names,
};
pub use file_jobs::{FileJobEvent, FileJobState, FileJobs};
pub use file_op::{FileDeed, FileOperation, LinkTarget, Made, Outcome, UndoJournal, unique_name};
pub use files::{
    Drive, DriveKind, FileClipboard, FileWork, disk_space, is_hidden, reveal_in_explorer,
    show_properties,
};
pub use files::{reveal, reveal_with};
pub use folder_handler::{FilesStart, start_detached};
pub use folder_handler::{
    FolderHandler, known_folder_path, start_explorer, started_by_files_fallback, tell_problem,
};
pub use fonts::font_families;
pub use hdr::Hdr;
pub use hotkeys::{Hotkey, Hotkeys, Modifiers};
pub use hotspot::{Hotspot, HotspotBand, HotspotInfo, HotspotState};
pub use icon::IconPixels;
pub use ime::{ImeMode, ImeState};
pub use index::FileSearch;
pub use index::SearchIndex;
pub use input_names::InputName;
pub use input_names::OwnInput;
pub use jump::EXPLORER_APP_ID;
pub use jump::app_id_of_program;
pub use jump::{JumpItem, JumpList, JumpTarget, KnownCategory};
pub use key_locks::{KeyLocks, window_in_front};
pub use keyboard::{InputLanguage, Typing};
pub use lasting::WindowMark;
pub use media::SessionKey;
pub use media::{Media, MediaCommand, NowPlaying, Position};
pub use media::{Repeat, SessionInfo};
pub use mouse_buttons::{MouseBinding, MouseButtonName, MouseButtons};
pub use mouse_hook::{ActiveTracking, GrabKey, MouseHook, PointerButton, PointerEvent};
pub use move_watch::{MoveEvent, MoveWatch};
pub use net_drives::{DriveDialog, NetworkDrives};
pub use network::{Link, Network};
pub use night_light::{COLDEST, GammaNight, WARMEST, WindowsNight, WindowsNightLight};
pub use notifications::Occupation;
pub use notifications::{Notification, Notifications};
pub use open_with::{OpenWithHandler, choose_app};
pub use overlays::{CloudState, Overlays};
pub use pace::Pace;
pub use packaged::{app_id_in, app_path};
pub use passage::Passage;
pub use passage::Still;
pub use peek::paint_desktop;
pub use plans::PowerPlan;
pub use power::PowerAction;
pub use power::PowerChoices;
pub use power_mode::{PowerMode, PowerModeState};
pub use power_settings::{
    ButtonAction, PowerChange, PowerSetting, PowerSettings, PowerState, SleepBlock, Supply,
};
pub use power_warnings::{BatteryChanges, BatteryLevels, CriticalAction};
pub use preview_host::{PreviewColors, PreviewHost, PreviewShown};
pub use privacy::{Capability, CapabilityUse, PrivacyWatch};
pub use processes::RunningProcess;
pub use program_path::{expand_variables, find_program, recent_folder};
pub use properties::{FileProperty, Properties, PropertyEdit, PropertyValue, split_items};
pub use quick_access::QuickFolder;
pub use quick_access::{QuickAccess, ShellPlace, onedrive, recent_files, wsl_distributions};
pub use radios::{RadioId, RadioInfo, RadioType, Radios, airplane_mode};
pub use renaming::rename_new;
pub use run_history::RunHistory;
pub use screenshot::save_screenshot;
pub use send_to::SendToTarget;
pub use share::SharePackage;
pub use shell_host::{HostOptions, ShellEvent, ShellHost};
pub use shell_item::ShellId;
pub use shell_menu::{Intercept, MenuOutcome, ShellMenu, ShellVerb};
pub use shell_mode::started_as_shell;
pub use shell_mode::{explorer_running, shell_mode};
pub use shell_new::NewTemplate;
pub use shell_thread::ShellThread;
pub use shell_windows::ShellWindows;
pub use shortcuts::Shortcut;
pub use show_desktop::ShowDesktop;
pub use snap::on_resizing_border;
pub use snap::{Display, MaximizeHover, WindowArranging, front_fills, shift_held};
pub use spell::SpellCheck;
pub use startup::{SessionStartup, StartupEntry, StartupSource, SwitchRefusal};
pub use system::{Battery, MemoryLoad, NetworkTraffic, ProcessorLoad, memory_held};
pub use system::{GpuLoad, GpuUsage};
pub use system_wallpaper::{SystemWallpaper, WallpaperPosition, user_locale};
pub use taskbar::Taskbar;
pub use taskbar_list::{
    GivenIcon, Progress, ProgressState, TaskbarList, TaskbarState, ThumbButton,
};
pub use terminal::{terminal, windows_terminal};
pub use thumbnail::Thumbnail;
pub use thumbnails::{LARGEST_THUMBNAIL, ThumbnailKind, Thumbnails};
pub use tile_border::TileBorder;
pub use touch_keyboard::TouchKeyboard;
pub use tray::BalloonKind;
pub use tray::TrayPromotion;
pub use tray::{Balloon, BalloonEvent};
pub use tray::{Tray, TrayButton, TrayIcon};
pub use uninstall::{Removal, app_settings, put_on_desktop};
pub use uri_scheme::UriScheme;
pub use veil::Veil;
pub use video::{DesktopVideo, VideoState};
pub use video::{Poster, VideoLook, VideoOverlay};
pub use wallpaper_server::{WallpaperAnswers, WallpaperRequest, WallpaperServer};
pub use wheel_lines::wheel_lines;
pub use wic::decode_with_wic;
pub use wifi::{Wifi, WifiNetwork, WifiReach, WifiSecurity, password_fits};
pub use windows_key::DesktopClose;
pub use windows_key::{ChoiceKey, TakenKeys, WindowsKey};
pub use winlogon::{
    ExplorerShell, NextShell, SHELL_FLAG, ShellRegistration, attach_console, is_safe_mode,
    own_program_name, recovery_keys_held,
};
pub use zones::SystemZone;

use std::ffi::c_void;
use std::mem::size_of;

use anyhow::{Context as _, Result, bail};
use gpui::Window;
use gpui::{Axis, Pixels, Point, point, px};
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
use windows::Win32::UI::Shell::{ABE_LEFT, ABE_RIGHT, ABM_SETAUTOHIDEBAREX};
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
/// of Windows, and the window that holds Tessera's icons of the desktop.
const NOT_FILLING: &[&str] = &[
    "Progman",
    "WorkerW",
    "XamlExplorerHostIslandWindow",
    "MultitaskingViewFrame",
    desktop_folder::LAYER_CLASS,
];

/// Message the system posts to a window that reserves a screen edge.
const EDGE_NOTIFICATION: u32 = WM_APP + 1;

/// A screen edge a shell surface can reserve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreenEdge {
    Top,
    Bottom,
    Left,
    Right,
}

impl ScreenEdge {
    /// Every edge, in the order the configuration and the menus name them.
    pub const ALL: [Self; 4] = [Self::Top, Self::Bottom, Self::Left, Self::Right];

    fn as_appbar_edge(self) -> u32 {
        match self {
            Self::Top => ABE_TOP,
            Self::Bottom => ABE_BOTTOM,
            Self::Left => ABE_LEFT,
            Self::Right => ABE_RIGHT,
        }
    }

    /// What the configuration file calls the edge.
    pub fn name(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    /// Whether a strip at this edge runs from the top of the display to
    /// its bottom: the left and the right edge.
    pub fn is_vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    /// The direction a strip at this edge runs in.
    pub fn along(self) -> Axis {
        if self.is_vertical() {
            Axis::Vertical
        } else {
            Axis::Horizontal
        }
    }

    /// The direction a strip at this edge is thick in.
    pub fn across(self) -> Axis {
        self.along().invert()
    }

    /// Where something lies that is moved `distance` toward this edge, out
    /// of the display: what slides in from the edge starts there.
    pub fn travel(self, distance: Pixels) -> Point<Pixels> {
        match self {
            Self::Top => point(px(0.), -distance),
            Self::Bottom => point(px(0.), distance),
            Self::Left => point(-distance, px(0.)),
            Self::Right => point(distance, px(0.)),
        }
    }

    /// Of `x` and `y`, the one that counts along a strip at this edge.
    pub fn along_of<T>(self, x: T, y: T) -> T {
        if self.is_vertical() { y } else { x }
    }

    /// Shrinks `area` to a strip `thickness` deep along this edge.
    fn clamp(self, area: &mut RECT, thickness: i32) {
        match self {
            Self::Top => area.bottom = area.top + thickness,
            Self::Bottom => area.top = area.bottom - thickness,
            Self::Left => area.right = area.left + thickness,
            Self::Right => area.left = area.right - thickness,
        }
    }

    /// Moves `area`, a strip at this edge, `by` physical pixels inward.
    fn inset(self, area: &mut RECT, by: i32) {
        let (x, y) = match self {
            Self::Top => (0, by),
            Self::Bottom => (0, -by),
            Self::Left => (by, 0),
            Self::Right => (-by, 0),
        };
        (area.left, area.right) = (area.left + x, area.right + x);
        (area.top, area.bottom) = (area.top + y, area.bottom + y);
    }
}

/// The part of the length of an edge a surface takes, as shares of that
/// length counted from its top or left end: 0 to 1 is all of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeSpan {
    pub from: f32,
    pub to: f32,
}

impl EdgeSpan {
    /// The whole length of the edge.
    pub const WHOLE: Self = Self { from: 0., to: 1. };

    /// Whether it is the whole length.
    pub fn is_whole(self) -> bool {
        self.from <= 0. && self.to >= 1.
    }

    /// Cuts `area`, a strip along `edge`, to this part of its length. A
    /// part that would be empty keeps a pixel.
    fn cut(self, area: &mut RECT, edge: ScreenEdge) {
        if self.is_whole() {
            return;
        }
        let (from, to) = (self.from.clamp(0., 1.), self.to.clamp(0., 1.));
        let part = |start: &mut i32, end: &mut i32| {
            let (first, length) = (*start, (*end - *start) as f32);
            *start = first + (length * from).round() as i32;
            *end = (first + (length * to).round() as i32).max(*start + 1);
        };
        match edge.along() {
            Axis::Horizontal => part(&mut area.left, &mut area.right),
            Axis::Vertical => part(&mut area.top, &mut area.bottom),
        }
    }
}

/// Where a strip lies at an edge of a display: how deep it is, how far it
/// stands in from the edge, and which part of the edge's length it takes,
/// in physical pixels and shares.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeBand {
    pub edge: ScreenEdge,
    pub depth: i32,
    /// How far from the edge of the display its outer side is.
    pub inset: i32,
    pub span: EdgeSpan,
}

impl EdgeBand {
    /// The band within `display`, the frame of its display.
    pub fn within(self, display: ScreenRect) -> ScreenRect {
        let mut area = RECT::from(display);
        self.edge.clamp(&mut area, self.depth);
        self.edge.inset(&mut area, self.inset);
        self.span.cut(&mut area, self.edge);
        area.into()
    }
}

impl From<ScreenRect> for RECT {
    fn from(rect: ScreenRect) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
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

    /// Where `band` lies on the surface's display.
    pub fn band(self, band: EdgeBand) -> Result<ScreenRect> {
        Ok(band.within(self.display_frame()?))
    }

    /// Whether the pointer is in `band` of the surface's display.
    pub fn pointer_in_band(self, band: EdgeBand) -> bool {
        let (Ok(area), Some((x, y))) = (self.band(band), pointer_at()) else {
            return false;
        };
        (area.left..area.right).contains(&x) && (area.top..area.bottom).contains(&y)
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

    /// Reserves a strip `band.depth` physical pixels deep at the edge of
    /// `band` of the surface's display and moves the surface into the part
    /// of it `band` takes, so maximized windows stop at the surface instead
    /// of sliding under it.
    ///
    /// The reservation ends when the returned guard is dropped. Windows also
    /// ends it when the window is destroyed, including after a crash.
    pub fn reserve_band(self, band: EdgeBand) -> Result<EdgeReservation> {
        let mut data = self.appbar_data();
        data.uCallbackMessage = EDGE_NOTIFICATION;
        // SAFETY: `data` is a correctly sized APPBARDATA that outlives the
        // call, and `hwnd` is a live window owned by this thread.
        if unsafe { SHAppBarMessage(ABM_NEW, &mut data) } == 0 {
            bail!("the system refused to register the surface as an edge bar");
        }
        let reservation = EdgeReservation { surface: self };
        reservation.occupy_band(band)?;
        Ok(reservation)
    }

    /// Registers the surface as the bar that hides at `edge` of its
    /// display, so that programs that ask, as browsers shown full screen
    /// do, leave the pointer a way to it. `None` where that edge of that
    /// display has such a bar already, which keeps it. The registration
    /// ends when the returned guard is dropped.
    pub fn claim_hiding_edge(self, edge: ScreenEdge) -> Option<HidingClaim> {
        let display = self.display_frame().ok()?;
        HidingClaim::register(self, edge, display, true).then_some(HidingClaim {
            surface: self,
            edge,
            display,
        })
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
    /// again with [`Self::occupy_band`]: another edge bar moved, or what
    /// keeps the bars changed, as when Explorer's taskbar went or came back.
    pub fn generation() -> u64 {
        appbar::places()
    }

    /// Makes the reserved strip `band.depth` physical pixels deep at the
    /// edge of `band` and moves the surface into the part of it `band`
    /// takes of the edge's length. The surface registers again first, in
    /// case what keeps the bars changed since; one that is registered
    /// already is refused, which changes nothing. The whole length is
    /// reserved, as
    /// Windows takes the depth of a bar off the room of windows all along
    /// its edge; a bar stands further in where other bars are at the same
    /// edge already, as whoever keeps the bars says, so the inset of
    /// `band` is not used.
    pub fn occupy_band(&self, band: EdgeBand) -> Result<()> {
        let hwnd = self.surface.hwnd;
        let mut data = self.surface.appbar_data();
        data.uCallbackMessage = EDGE_NOTIFICATION;
        // SAFETY: `data` is a correctly sized APPBARDATA that outlives the
        // call, and `hwnd` is a live window owned by this thread.
        unsafe { SHAppBarMessage(ABM_NEW, &mut data) };
        let edge = band.edge;
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
            edge.clamp(&mut data.rc, band.depth);
            SHAppBarMessage(ABM_SETPOS, &mut data);

            let mut placed = data.rc;
            band.span.cut(&mut placed, edge);
            let RECT {
                left,
                top,
                right,
                bottom,
            } = placed;
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

/// Keeps a surface registered as the bar that hides at an edge of its
/// display until dropped.
#[derive(Debug)]
pub struct HidingClaim {
    surface: NativeSurface,
    edge: ScreenEdge,
    /// The frame of the display it is registered on.
    display: ScreenRect,
}

impl HidingClaim {
    /// Registers the bar at its edge of `display`, or ends that
    /// registration; whether whoever keeps the bars agreed.
    fn register(surface: NativeSurface, edge: ScreenEdge, display: ScreenRect, on: bool) -> bool {
        let mut data = surface.appbar_data();
        data.uEdge = edge.as_appbar_edge();
        data.rc = display.into();
        data.lParam = LPARAM(on.into());
        // SAFETY: `data` is a correctly sized APPBARDATA that outlives the
        // call, and `hwnd` is a live window owned by this thread.
        unsafe { SHAppBarMessage(ABM_SETAUTOHIDEBAREX, &mut data) != 0 }
    }

    /// Registers again, at the display the surface is on now, after that
    /// display or whoever keeps the bars changed; whether it is
    /// registered now.
    pub fn renew(&mut self) -> bool {
        Self::register(self.surface, self.edge, self.display, false);
        let Ok(display) = self.surface.display_frame() else {
            return false;
        };
        self.display = display;
        Self::register(self.surface, self.edge, display, true)
    }
}

impl Drop for HidingClaim {
    fn drop(&mut self) {
        Self::register(self.surface, self.edge, self.display, false);
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    const DISPLAY: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    fn sides(rect: ScreenRect) -> (i32, i32, i32, i32) {
        (rect.left, rect.top, rect.right, rect.bottom)
    }

    /// The whole strip `depth` deep right at `edge`.
    fn whole(edge: ScreenEdge, depth: i32) -> EdgeBand {
        EdgeBand {
            edge,
            depth,
            inset: 0,
            span: EdgeSpan::WHOLE,
        }
    }

    #[test]
    fn a_band_lies_at_its_edge_of_the_display() {
        let at = |edge| sides(whole(edge, 48).within(DISPLAY));
        assert_eq!(at(ScreenEdge::Top), (0, 0, 1920, 48));
        assert_eq!(at(ScreenEdge::Bottom), (0, 1032, 1920, 1080));
        assert_eq!(at(ScreenEdge::Left), (0, 0, 48, 1080));
        assert_eq!(at(ScreenEdge::Right), (1872, 0, 1920, 1080));
    }

    #[test]
    fn a_band_stands_in_and_takes_a_part_of_the_length() {
        let band = EdgeBand {
            edge: ScreenEdge::Left,
            depth: 40,
            inset: 48,
            span: EdgeSpan {
                from: 0.25,
                to: 0.75,
            },
        };
        assert_eq!(sides(band.within(DISPLAY)), (48, 270, 88, 810));
        let band = EdgeBand {
            edge: ScreenEdge::Bottom,
            depth: 40,
            inset: 10,
            span: EdgeSpan { from: 0.5, to: 1. },
        };
        assert_eq!(sides(band.within(DISPLAY)), (960, 1030, 1920, 1070));
        // An empty part keeps a pixel.
        let band = EdgeBand {
            span: EdgeSpan { from: 0.5, to: 0.5 },
            ..whole(ScreenEdge::Top, 10)
        };
        assert_eq!(sides(band.within(DISPLAY)), (960, 0, 961, 10));
    }

    #[test]
    fn the_edges_say_which_way_they_run_and_slide() {
        assert!(ScreenEdge::Left.is_vertical() && ScreenEdge::Right.is_vertical());
        assert!(!ScreenEdge::Top.is_vertical() && !ScreenEdge::Bottom.is_vertical());
        assert_eq!(ScreenEdge::Left.along(), Axis::Vertical);
        assert_eq!(ScreenEdge::Top.across(), Axis::Vertical);
        assert_eq!(ScreenEdge::Left.travel(px(5.)), point(px(-5.), px(0.)));
        assert_eq!(ScreenEdge::Bottom.travel(px(5.)), point(px(0.), px(5.)));
        assert_eq!(ScreenEdge::Right.along_of(1, 2), 2);
        let names: Vec<_> = ScreenEdge::ALL.iter().map(|edge| edge.name()).collect();
        assert_eq!(names, ["top", "bottom", "left", "right"]);
    }
}
