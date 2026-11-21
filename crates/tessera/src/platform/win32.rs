// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Win32 window behavior that GPUI does not expose.

#![allow(unsafe_code)]

mod accent;
mod app_windows;
mod audio;
mod icon;
mod media;
mod network;
mod notifications;
mod power;
mod shortcuts;
mod system;
mod taskbar;
mod tray;

pub use accent::{accent_color, light_mode};
pub use app_windows::{AppWindow, AppWindowId, WindowEvents};
pub use audio::AudioOutput;
pub use icon::IconPixels;
pub use media::{Media, MediaCommand, NowPlaying};
pub use network::{Link, Network};
pub use notifications::{Notification, Notifications};
pub use power::PowerAction;
pub use shortcuts::Shortcut;
pub use system::{Battery, MemoryLoad, ProcessorLoad};
pub use taskbar::Taskbar;
pub use tray::{Tray, TrayButton, TrayIcon};

use std::ffi::c_void;
use std::mem::size_of;

use anyhow::{Context as _, Result, bail};
use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::Shell::{
    ABE_BOTTOM, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, APPBARDATA, SHAppBarMessage,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, WINDOW_EX_STYLE, WM_APP, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    WS_VISIBLE,
};

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

/// The native window behind a GPUI shell surface.
#[derive(Clone, Copy, Debug)]
pub struct NativeSurface {
    hwnd: HWND,
}

/// Identifies a display for as long as the display configuration is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DisplayKey(isize);

impl DisplayKey {
    /// The display that holds most of `hwnd`, or the nearest one.
    fn of_window(hwnd: HWND) -> Self {
        // SAFETY: the call only reads window state and tolerates a handle
        // whose window has since been destroyed.
        Self(unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }.0 as isize)
    }
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

    /// Turns the window into a popup of the shell: no caption or frame,
    /// above ordinary windows, and not listed among application windows.
    /// Unlike a surface it can take keyboard focus.
    pub fn adopt_as_popup(self) -> Result<()> {
        self.adopt(WS_EX_TOOLWINDOW)
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

    /// Moves and resizes the window without activating it.
    pub fn place(self, frame: ScreenRect) -> Result<()> {
        // SAFETY: `hwnd` belongs to a live window owned by this thread.
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                frame.left,
                frame.top,
                frame.right - frame.left,
                frame.bottom - frame.top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
        }
        .context("the window could not be moved")
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

    /// Removes the caption and frame, adds `extended_style` and raises the
    /// window above ordinary ones.
    fn adopt(self, extended_style: WINDOW_EX_STYLE) -> Result<()> {
        // SAFETY: `hwnd` belongs to a live window owned by this thread, and
        // the calls only change its styles and z-order.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as isize);
            let extended = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
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
    /// Makes the reserved strip `thickness` physical pixels deep at `edge`
    /// of the surface's display and moves the surface into it.
    pub fn occupy(&self, edge: ScreenEdge, thickness: i32) -> Result<()> {
        let hwnd = self.surface.hwnd;
        let mut data = self.surface.appbar_data();
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
