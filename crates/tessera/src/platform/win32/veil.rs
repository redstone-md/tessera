// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A dark sheet over one display, under a window that stands in front.
//!
//! It is a plain window that Windows fills with black and blends in itself,
//! so that dimming a display costs nothing of what a window of the user
//! interface takes for drawing.

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_HWNDPARENT, IsWindow, LWA_ALPHA,
    RegisterClassW, SW_SHOWNA, SetLayeredWindowAttributes, SetWindowLongPtrW, ShowWindow,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::w;

use super::{NativeSurface, ScreenRect};

/// A black window over a display, as dark as [`Self::darken`] says, which
/// stays right under the window it was opened for. A click on it takes
/// keyboard focus from that window. It belongs to the thread that opened
/// it and goes when dropped.
pub struct Veil {
    hwnd: HWND,
    /// The window in front, which Windows keeps above this one.
    front: HWND,
}

impl Veil {
    /// Opens the sheet over `frame` under `front`, not yet to be seen.
    pub fn open(frame: ScreenRect, front: NativeSurface) -> Result<Self> {
        // SAFETY: the calls register a class and make, show and order windows of this thread.
        unsafe {
            let module = GetModuleHandleW(None).context("the program has no module")?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(respond),
                hInstance: module.into(),
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                lpszClassName: w!("TesseraVeil"),
                ..Default::default()
            };
            // Fails when the class is there already, which is as good.
            RegisterClassW(&raw const class);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class.lpszClassName,
                w!(""),
                WS_POPUP,
                frame.left,
                frame.top,
                frame.right - frame.left,
                frame.bottom - frame.top,
                None,
                None,
                Some(module.into()),
                None,
            )
            .context("the veil could not be made")?;
            let veil = Self {
                hwnd,
                front: front.hwnd,
            };
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA)
                .context("the veil could not be made see-through")?;
            // A window stays above the one that owns it, also when the
            // owner is clicked.
            SetWindowLongPtrW(front.hwnd, GWLP_HWNDPARENT, hwnd.0 as isize);
            let _ = ShowWindow(hwnd, SW_SHOWNA);
            Ok(veil)
        }
    }

    /// Makes the sheet `share` of the way to black, from 0 to 1.
    pub fn darken(&self, share: f32) {
        let alpha = (share.clamp(0., 1.) * 255.).round() as u8;
        // SAFETY: the window belongs to this thread.
        unsafe {
            let _ = SetLayeredWindowAttributes(self.hwnd, COLORREF(0), alpha, LWA_ALPHA);
        }
    }
}

/// The window procedure of a veil, which leaves everything to Windows.
unsafe extern "system" fn respond(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: the arguments are those Windows called with.
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}

impl Drop for Veil {
    fn drop(&mut self) {
        // SAFETY: the windows belong to this thread. Windows destroys what
        // a destroyed window owns, so the window in front is let go first
        // if it is still there.
        unsafe {
            if IsWindow(Some(self.front)).as_bool() {
                SetWindowLongPtrW(self.front, GWLP_HWNDPARENT, 0);
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
