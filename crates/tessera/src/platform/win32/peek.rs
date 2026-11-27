// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What a peek through the windows needs of Windows: where the picture of
//! a window belongs on the screen, a layer above the windows and under
//! what is kept above them, and the picture of the desktop.

#![allow(unsafe_code)]

use std::ffi::c_void;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GdiFlush, HGDIOBJ, PaintDesktop, SelectObject, SetWindowOrgEx,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, HWND_NOTOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
};

use super::{AppWindowId, NativeSurface, ScreenRect};

impl AppWindowId {
    /// Where a picture of the window `source` physical pixels large, as
    /// [`Thumbnail::source_size`](super::Thumbnail::source_size) gives it,
    /// lies on the screen: the whole window with its invisible border, or
    /// for a minimized window that size around the middle of where it is
    /// restored to. `None` for a window that is gone.
    pub fn pictured_frame(self, (width, height): (i32, i32)) -> Option<ScreenRect> {
        if !self.is_minimized() {
            let mut whole = RECT::default();
            // SAFETY: `whole` outlives the call, which fails for a window
            // that has since been destroyed.
            unsafe { GetWindowRect(self.hwnd(), &raw mut whole) }.ok()?;
            return Some(whole.into());
        }
        let restored = self.restored_frame()?;
        Some(centered_on(restored, width, height))
    }
}

/// A rectangle `width` by `height` around the middle of `frame`.
fn centered_on(frame: ScreenRect, width: i32, height: i32) -> ScreenRect {
    let left = (frame.left + frame.right - width) / 2;
    let top = (frame.top + frame.bottom - height) / 2;
    ScreenRect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

impl NativeSurface {
    /// Puts the window above every ordinary window and under those kept
    /// above the others, such as Tessera's bars, popups and previews, which
    /// stay in sight over it. It is no longer kept above itself.
    pub fn lay_over_windows(self) {
        // SAFETY: `hwnd` belongs to a live window owned by this thread, and
        // the call only changes its place in the order of windows.
        unsafe {
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
            let _ = SetWindowPos(self.hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
        }
    }
}

/// The picture of the desktop within `frame`, which is in pixels of the
/// desktop, as Windows paints it behind the windows (`PaintDesktop`): the
/// wallpaper of Windows or its color, without icons and without the
/// wallpaper Tessera shows itself. Its width, its height and its pixels,
/// top row first, each blue, green, red and opaque. `None` where Windows
/// paints nothing.
///
/// The memory of the picture is the size of the display: it takes a
/// moment and is not for the thread of the interface.
pub fn paint_desktop(frame: ScreenRect) -> Option<(u32, u32, Vec<u8>)> {
    let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
    if width <= 0 || height <= 0 {
        return None;
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // A height below zero has the rows run from the top.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    // SAFETY: the context and the bitmap are made, used and deleted here;
    // `bits` points at the memory of the bitmap, `width * height * 4`
    // bytes as `info` describes it, which is copied out before the bitmap
    // is deleted.
    unsafe {
        let context = CreateCompatibleDC(None);
        if context.is_invalid() {
            return None;
        }
        let bitmap = CreateDIBSection(
            Some(context),
            &raw const info,
            DIB_RGB_COLORS,
            &raw mut bits,
            None,
            0,
        );
        let Ok(bitmap) = bitmap else {
            let _ = DeleteDC(context);
            return None;
        };
        let before = SelectObject(context, HGDIOBJ(bitmap.0));
        // The desktop is painted in its own coordinates: the corner of the
        // frame comes to the corner of the bitmap.
        let _ = SetWindowOrgEx(context, frame.left, frame.top, None);
        let painted = PaintDesktop(context).as_bool();
        let _ = GdiFlush();
        let length = width as usize * height as usize * 4;
        let pixels = (painted && !bits.is_null())
            .then(|| std::slice::from_raw_parts(bits.cast::<u8>(), length).to_vec());
        SelectObject(context, before);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(context);
        let mut pixels = pixels?;
        // GDI leaves the fourth byte at zero.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = u8::MAX;
        }
        Some((width as u32, height as u32, pixels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimized_window_is_pictured_around_where_it_comes_back() {
        let frame = ScreenRect {
            left: 100,
            top: 200,
            right: 500,
            bottom: 400,
        };
        let pictured = centered_on(frame, 420, 210);
        assert_eq!(
            pictured,
            ScreenRect {
                left: 90,
                top: 195,
                right: 510,
                bottom: 405,
            }
        );
    }
}
