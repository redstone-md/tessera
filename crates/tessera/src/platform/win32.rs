// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Win32 window behavior that GPUI does not expose.

#![allow(unsafe_code)]

mod app_windows;
mod icon;
mod taskbar;

pub use app_windows::{AppWindow, AppWindowId};
pub use icon::IconPixels;
pub use taskbar::Taskbar;

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
    GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WM_APP,
    WS_EX_NOACTIVATE, WS_POPUP, WS_VISIBLE,
};

/// Message the system posts to a window that reserves a screen edge.
const EDGE_NOTIFICATION: u32 = WM_APP + 1;

/// A screen edge a shell surface can reserve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// The native window behind a GPUI shell surface.
#[derive(Clone, Copy, Debug)]
pub struct NativeSurface {
    hwnd: HWND,
}

impl NativeSurface {
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
        // SAFETY: `hwnd` belongs to a live window owned by this thread, and
        // the calls only change its styles and z-order.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as isize);
            let extended = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(
                self.hwnd,
                GWL_EXSTYLE,
                extended | WS_EX_NOACTIVATE.0 as isize,
            );
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
        .context("the surface styles could not be applied")
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
        data.uEdge = edge.as_appbar_edge();

        // SAFETY: `data` and `monitor` are correctly sized structures that
        // outlive every call, and `hwnd` is a live window owned by this thread.
        unsafe {
            let mut monitor = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let display = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            if !GetMonitorInfoW(display, &mut monitor).as_bool() {
                bail!("the surface's display could not be read");
            }
            if SHAppBarMessage(ABM_NEW, &mut data) == 0 {
                bail!("the system refused to register the surface as an edge bar");
            }
            let reservation = EdgeReservation { surface: self };

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
                self.hwnd,
                None,
                left,
                top,
                right - left,
                bottom - top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
            .context("the surface could not be moved into its reserved area")?;
            Ok(reservation)
        }
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
