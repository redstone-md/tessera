// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! More of what can be done to the window of another program: maximize,
//! minimize and restore it, keep it above the others, make it see-through,
//! hide its border, and read and set where it goes back to when it is
//! restored.
//!
//! These are kept apart from the listing of windows in app_windows.rs,
//! which the dock extends on its own.
//!
//! Windows does none of this to a window of a process that runs with more
//! rights than Tessera, and says nothing about it. A request that sends
//! the window a message is posted, or made from a short thread of its own
//! and not at all to a window whose program hangs, so that such a program
//! never holds the shell up.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};
use std::thread;

use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Dwm::DWMWA_COLOR_NONE;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetLayeredWindowAttributes, GetWindowLongPtrW, GetWindowPlacement,
    GetWindowRect, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, IsHungAppWindow, IsIconic, IsZoomed,
    LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA, SHOW_WINDOW_CMD, SM_CXBORDER, SM_CXFIXEDFRAME,
    SM_CXPADDEDBORDER, SM_CXSIZEFRAME, SM_CYBORDER, SM_CYFIXEDFRAME, SM_CYSIZEFRAME, SW_MAXIMIZE,
    SW_MINIMIZE, SW_RESTORE, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SYSTEM_METRICS_INDEX, SetLayeredWindowAttributes, SetWindowLongPtrW,
    SetWindowPlacement, SetWindowPos, ShowWindowAsync, WINDOWPLACEMENT, WS_DLGFRAME, WS_EX_LAYERED,
    WS_EX_TOPMOST, WS_THICKFRAME,
};

use super::{AppWindowId, ScreenRect};

/// The least a window is made opaque, so that it can still be found.
const LEAST_OPACITY: f32 = 0.1;

/// The windows Tessera made layered to make them see-through, so that it
/// takes that away again when they are opaque again, and only from them.
static LAYERED: Mutex<Option<HashSet<isize>>> = Mutex::new(None);

/// The invisible border of windows as it was last measured while they
/// were neither maximized nor minimized, when it is wider or narrower
/// then, by window.
static MARGINS: Mutex<Option<HashMap<isize, [i32; 4]>>> = Mutex::new(None);

/// How many windows the invisible border is remembered for before those
/// that are gone are forgotten.
const MARGINS_KEPT: usize = 256;

/// How much the invisible border of a window reaches beyond what is seen
/// of it, on the left, top, right and bottom: `whole` is the window as
/// Windows places it, `seen` what is drawn of it.
fn margins(seen: ScreenRect, whole: ScreenRect) -> [i32; 4] {
    [
        seen.left - whole.left,
        seen.top - whole.top,
        whole.right - seen.right,
        whole.bottom - seen.bottom,
    ]
}

/// `rect` grown by `margins`, or shrunk by them with `inward`.
fn widened(rect: ScreenRect, [left, top, right, bottom]: [i32; 4], inward: bool) -> ScreenRect {
    let sign = if inward { -1 } else { 1 };
    ScreenRect {
        left: rect.left - sign * left,
        top: rect.top - sign * top,
        right: rect.right + sign * right,
        bottom: rect.bottom + sign * bottom,
    }
}

/// `rect` moved by `(x, y)`.
fn shifted(rect: ScreenRect, (x, y): (i32, i32)) -> ScreenRect {
    ScreenRect {
        left: rect.left + x,
        top: rect.top + y,
        right: rect.right + x,
        bottom: rect.bottom + y,
    }
}

/// How far the work area of the display that holds most of `rect` lies
/// from the corner of that display. Windows gives and takes the place a
/// window is restored to in coordinates that start at that corner of the
/// work area rather than of the screen.
fn workspace_offset(rect: ScreenRect) -> (i32, i32) {
    let rect = RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `rect` and `info` outlive the calls, and `info` is sized as
    // the call is told; the calls only read the arrangement of displays.
    let read = unsafe {
        let display = MonitorFromRect(&raw const rect, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(display, &raw mut info).as_bool()
    };
    match read {
        true => (
            info.rcWork.left - info.rcMonitor.left,
            info.rcWork.top - info.rcMonitor.top,
        ),
        false => (0, 0),
    }
}

impl AppWindowId {
    /// Whether the window is maximized.
    pub fn is_maximized(self) -> bool {
        // SAFETY: the call only reads window state and tolerates a handle
        // whose window has since been destroyed.
        unsafe { IsZoomed(self.hwnd()).as_bool() }
    }

    /// Whether the window is minimized.
    pub fn is_minimized(self) -> bool {
        // SAFETY: as in `is_maximized`.
        unsafe { IsIconic(self.hwnd()).as_bool() }
    }

    /// Whether the window has stopped taking its messages, as a program
    /// that hangs does.
    pub fn is_hung(self) -> bool {
        // SAFETY: as in `is_maximized`.
        unsafe { IsHungAppWindow(self.hwnd()).as_bool() }
    }

    /// Asks the window to be maximized, as its button does.
    pub fn maximize(self) {
        self.show_async(SW_MAXIMIZE);
    }

    /// Asks the window to be minimized.
    pub fn minimize(self) {
        self.show_async(SW_MINIMIZE);
    }

    /// Asks a maximized or minimized window to take the size it had
    /// before.
    pub fn restore(self) {
        self.show_async(SW_RESTORE);
    }

    fn show_async(self, how: SHOW_WINDOW_CMD) {
        // SAFETY: the call is posted, tolerates a handle whose window has
        // since been destroyed and fails without side effects.
        unsafe {
            let _ = ShowWindowAsync(self.hwnd(), how);
        }
    }

    /// Whether the window is kept above the windows that are not.
    pub fn is_topmost(self) -> bool {
        // SAFETY: reading the style of a window, valid or not, has no
        // preconditions.
        let extended = unsafe { GetWindowLongPtrW(self.hwnd(), GWL_EXSTYLE) } as u32;
        extended & WS_EX_TOPMOST.0 != 0
    }

    /// Keeps the window above the windows that are not, or no longer.
    /// Whether Windows took the request is returned.
    pub fn set_topmost(self, above: bool) -> bool {
        let after = if above { HWND_TOPMOST } else { HWND_NOTOPMOST };
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS;
        // SAFETY: the request is posted to the window's thread and fails
        // without side effects for a window that is gone.
        unsafe { SetWindowPos(self.hwnd(), Some(after), 0, 0, 0, 0, flags) }.is_ok()
    }

    /// Puts the window above the other windows that are not kept above,
    /// without bringing it in front.
    pub fn raise(self) -> bool {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS;
        // SAFETY: as in `set_topmost`.
        unsafe { SetWindowPos(self.hwnd(), Some(HWND_TOP), 0, 0, 0, 0, flags) }.is_ok()
    }

    /// How opaque the window is drawn, from 0 to 1.
    pub fn opacity(self) -> f32 {
        let hwnd = self.hwnd();
        // SAFETY: reading the style of a window has no preconditions.
        let extended = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
        if extended & WS_EX_LAYERED.0 == 0 {
            return 1.;
        }
        let mut alpha = 255u8;
        let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
        // SAFETY: both values outlive the call, which only reads them out.
        let read = unsafe {
            GetLayeredWindowAttributes(hwnd, None, Some(&raw mut alpha), Some(&raw mut flags))
        };
        match read.is_ok() && flags.0 & LWA_ALPHA.0 != 0 {
            true => f32::from(alpha) / 255.,
            false => 1.,
        }
    }

    /// Draws the whole window `opacity` opaque, from 0.1 to 1. The window
    /// is made layered for that, which some programs do not draw well in;
    /// it stops being layered once it is opaque again, unless it was
    /// layered before.
    ///
    /// Windows sends the window messages to change its style, so it is
    /// done from a short thread of its own; a window whose program hangs
    /// is left as it is.
    pub fn set_opacity(self, opacity: f32) {
        let raw = self.raw();
        let opacity = opacity.clamp(LEAST_OPACITY, 1.);
        let started = thread::Builder::new()
            .name("tessera window opacity".to_owned())
            .spawn(move || {
                let id = AppWindowId::from_raw(raw);
                if !id.is_hung() {
                    id.set_opacity_here(opacity);
                }
            });
        if let Err(error) = started {
            report!("a window is not made see-through: {error}");
        }
    }

    /// What [`Self::set_opacity`] does, on the thread it is called on.
    fn set_opacity_here(self, opacity: f32) {
        let hwnd = self.hwnd();
        let mut layered = LAYERED.lock().unwrap_or_else(PoisonError::into_inner);
        let ours = layered.get_or_insert_with(HashSet::new);
        // SAFETY: the calls change the extended style and the opacity of a
        // window and fail without side effects for one that is gone.
        unsafe {
            let extended = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let is_layered = extended as u32 & WS_EX_LAYERED.0 != 0;
            if opacity >= 1. {
                if !is_layered {
                    return;
                }
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
                if ours.remove(&self.raw()) {
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, extended & !(WS_EX_LAYERED.0 as isize));
                }
                return;
            }
            if !is_layered {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, extended | WS_EX_LAYERED.0 as isize);
                ours.insert(self.raw());
            }
            let alpha = (opacity * 255.).round() as u8;
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
        }
    }

    /// Has Windows draw no thin border around the window. Whether Windows
    /// took the request is returned; [`Self::set_border`] with `None`
    /// gives the window its border back.
    pub fn hide_border(self) -> bool {
        self.set_border(Some(DWMWA_COLOR_NONE))
    }

    /// The invisible border the window has while it is neither maximized
    /// nor minimized, as [`margins`] gives it. A maximized window reaches
    /// beyond the display on every side, and a minimized one has no
    /// border to measure, so for them it is the border measured before,
    /// or else the one Windows gives a window of its kind.
    fn margins(self) -> Option<[i32; 4]> {
        let mut remembered = MARGINS.lock().unwrap_or_else(PoisonError::into_inner);
        let remembered = remembered.get_or_insert_with(HashMap::new);
        if self.is_maximized() || self.is_minimized() {
            return remembered
                .get(&self.raw())
                .copied()
                .or_else(|| self.usual_margins());
        }
        let seen = self.frame()?;
        let mut whole = RECT::default();
        // SAFETY: `whole` outlives the call, which tolerates a handle whose
        // window has since been destroyed.
        unsafe { GetWindowRect(self.hwnd(), &raw mut whole) }.ok()?;
        let measured = margins(seen, whole.into());
        if remembered.len() >= MARGINS_KEPT {
            remembered.retain(|&raw, _| AppWindowId::from_raw(raw).exists());
        }
        remembered.insert(self.raw(), measured);
        Some(measured)
    }

    /// The invisible border Windows gives a window with the frame this
    /// one has, at its scale, when it is neither maximized nor minimized:
    /// the frame and its padding beyond the line that is drawn of it, on
    /// every side but the top. A program that draws its own frame may
    /// have another.
    fn usual_margins(self) -> Option<[i32; 4]> {
        let hwnd = self.hwnd();
        // SAFETY: the calls only read the scale and the style of a window,
        // and the sizes Windows gives frames at a scale.
        unsafe {
            let dpi = GetDpiForWindow(hwnd);
            if dpi == 0 {
                return None;
            }
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let (x, y) = if style & WS_THICKFRAME.0 != 0 {
                (SM_CXSIZEFRAME, SM_CYSIZEFRAME)
            } else if style & WS_DLGFRAME.0 != 0 {
                (SM_CXFIXEDFRAME, SM_CYFIXEDFRAME)
            } else {
                return Some([0; 4]);
            };
            let metric = |index: SYSTEM_METRICS_INDEX| GetSystemMetricsForDpi(index, dpi);
            let padding = metric(SM_CXPADDEDBORDER);
            let side = (metric(x) + padding - metric(SM_CXBORDER)).max(0);
            let bottom = (metric(y) + padding - metric(SM_CYBORDER)).max(0);
            Some([side, 0, side, bottom])
        }
    }

    /// What is seen of the window when it is restored, on the screen: for
    /// a maximized or minimized window, where it goes back to.
    pub fn restored_frame(self) -> Option<ScreenRect> {
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: `placement` is sized as the call is told and outlives it;
        // the call only reads window state.
        unsafe { GetWindowPlacement(self.hwnd(), &raw mut placement) }.ok()?;
        let normal = ScreenRect::from(placement.rcNormalPosition);
        let whole = shifted(normal, workspace_offset(normal));
        Some(widened(whole, self.margins()?, true))
    }

    /// Has the window go back to `frame`, what is to be seen of it on the
    /// screen, whenever it is restored, and be shown maximized now with
    /// `maximized` or restored there otherwise. A maximized window goes to
    /// the display that holds most of `frame`.
    ///
    /// Windows sends the window messages for this, so it is done from a
    /// short thread of its own; a window whose program hangs is left as it
    /// is.
    pub fn place_restored(self, frame: ScreenRect, maximized: bool) {
        let Some(margins) = self.margins() else {
            return;
        };
        let whole = widened(frame, margins, false);
        let (x, y) = workspace_offset(whole);
        let normal = shifted(whole, (-x, -y));
        let raw = self.raw();
        let placed = thread::Builder::new()
            .name("tessera window placement".to_owned())
            .spawn(move || {
                let id = AppWindowId::from_raw(raw);
                if id.is_hung() {
                    return;
                }
                let mut placement = WINDOWPLACEMENT {
                    length: size_of::<WINDOWPLACEMENT>() as u32,
                    ..Default::default()
                };
                // SAFETY: `placement` is sized as the calls are told and
                // outlives them; a window that is gone makes them fail.
                unsafe {
                    if GetWindowPlacement(id.hwnd(), &raw mut placement).is_err() {
                        return;
                    }
                    placement.rcNormalPosition = RECT {
                        left: normal.left,
                        top: normal.top,
                        right: normal.right,
                        bottom: normal.bottom,
                    };
                    placement.showCmd = match maximized {
                        true => SW_SHOWMAXIMIZED.0 as u32,
                        false => SW_SHOWNORMAL.0 as u32,
                    };
                    let _ = SetWindowPlacement(id.hwnd(), &raw const placement);
                }
            });
        if let Err(error) = placed {
            report!("a window is not placed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn the_invisible_border_is_taken_off_and_put_back() {
        let whole = rect(93, 100, 1007, 707);
        let seen = rect(100, 100, 1000, 700);
        let margins = margins(seen, whole);
        assert_eq!(margins, [7, 0, 7, 7]);
        assert_eq!(widened(seen, margins, false), whole);
        assert_eq!(widened(whole, margins, true), seen);
    }

    #[test]
    fn a_rect_moves_by_an_offset() {
        let moved = shifted(rect(0, 0, 10, 20), (5, -3));
        assert_eq!(moved, rect(5, -3, 15, 17));
        assert_eq!(shifted(moved, (-5, 3)), rect(0, 0, 10, 20));
    }
}
