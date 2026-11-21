// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Windows that leave the screen sideways instead of vanishing.
//!
//! A window of another program cannot be moved smoothly or faded from
//! outside, and Windows stops picturing it the moment it is hidden. What
//! moves is a still picture of it, taken before it is hidden and shown in
//! a window of its own right over it. The pictures have a thread of their
//! own and live for one motion.

use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_TRANSITIONS_FORCEDISABLED, DwmFlush, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC,
    HGDIOBJ, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowRect, IsIconic, IsZoomed,
    RegisterClassW, SW_SHOWNA, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, w};

use super::{AppWindowId, ScreenRect};

/// How long a window takes to leave.
const DURATION: Duration = Duration::from_millis(260);

/// How far a window travels while it leaves, in physical pixels.
const TRAVEL: f32 = 280.;

/// The radius Windows 11 rounds the corners of a window with, in logical
/// pixels.
const CORNER: f32 = 8.;

/// Asks for a picture of all of a window, also of what it draws with the
/// graphics card.
const WHOLE_CONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);

/// The motion of windows that leave the screen.
pub struct Departure;

impl Departure {
    /// Hides `windows`, which are seen leaving to the left or, with
    /// `rightward`, to the right. A window no picture can be taken of,
    /// and a minimized one, is hidden without the motion.
    pub fn play(windows: Vec<AppWindowId>, rightward: bool) {
        if windows.is_empty() {
            return;
        }
        let leaving = windows.clone();
        let started = thread::Builder::new()
            .name("departure".to_owned())
            .spawn(move || leave(&leaving, rightward));
        if started.is_err() {
            for window in windows {
                window.hide();
            }
        }
    }
}

/// The share of the way covered after `share` of the time: fast at first,
/// slow at the end.
fn eased(share: f32) -> f32 {
    1. - (1. - share.clamp(0., 1.)).powi(3)
}

/// Whether the place `x`, `y` of a picture `width` by `height` lies in
/// one of its corners outside the rounding of `radius`.
fn outside_rounding(x: i32, y: i32, width: i32, height: i32, radius: i32) -> bool {
    let near = |place: i32, length: i32| {
        if place < radius {
            radius - place
        } else if place >= length - radius {
            place - (length - radius - 1)
        } else {
            0
        }
    };
    let (across, down) = (near(x, width), near(y, height));
    across > 0 && down > 0 && across * across + down * down > radius * radius
}

/// Shows pictures of `windows` over them, hides the windows and moves the
/// pictures away.
fn leave(windows: &[AppWindowId], rightward: bool) {
    let pictures: Vec<Picture> = windows
        .iter()
        .filter_map(|&window| Picture::take(window))
        .collect();
    // SAFETY: the calls wait for the screen to be drawn and fail without
    // side effects.
    unsafe {
        // The pictures are on the screen before the windows go.
        if !pictures.is_empty() {
            let _ = DwmFlush();
        }
    }
    // Windows fades a window that is hidden where it stands, which would
    // be seen next to its picture.
    for &window in windows {
        fade_in_place(window, false);
        window.hide();
    }
    let direction = if rightward { 1. } else { -1. };
    let start = Instant::now();
    while !pictures.is_empty() {
        let share = start.elapsed().as_secs_f32() / DURATION.as_secs_f32();
        if share >= 1. {
            break;
        }
        let shift = (eased(share) * TRAVEL * direction).round() as i32;
        for picture in &pictures {
            picture.show(shift, 1. - eased(share));
        }
        // SAFETY: as above. Waits for the next picture of the screen.
        if unsafe { DwmFlush() }.is_err() {
            thread::sleep(Duration::from_millis(8));
        }
    }
    drop(pictures);
    for &window in windows {
        fade_in_place(window, true);
    }
}

/// Says whether Windows may animate `window` itself when it is shown or
/// hidden, which it does unless told otherwise.
fn fade_in_place(window: AppWindowId, allowed: bool) {
    let disabled = BOOL::from(!allowed);
    // SAFETY: `disabled` is as large as the call is told, and a window
    // that is gone makes the call fail without side effects.
    unsafe {
        let _ = DwmSetWindowAttribute(
            window.hwnd(),
            DWMWA_TRANSITIONS_FORCEDISABLED,
            (&raw const disabled).cast(),
            size_of::<BOOL>() as u32,
        );
    }
}

/// A still picture of a window in a window of its own, until dropped.
struct Picture {
    /// The window that shows the picture, once there is one.
    window: Option<HWND>,
    dc: HDC,
    bitmap: HBITMAP,
    /// What the device context held before the bitmap.
    before: HGDIOBJ,
    /// Where the pictured window is seen on the screen.
    frame: ScreenRect,
}

impl Picture {
    /// Pictures `window` and shows the picture where the window is.
    fn take(window: AppWindowId) -> Option<Self> {
        let hwnd = window.hwnd();
        let mut whole = RECT::default();
        // SAFETY: the calls read the state of a window that may be gone,
        // which makes them fail, and make objects of this thread that the
        // picture frees when dropped. The bits of the bitmap are as many
        // as its width and height say and are not used after it.
        unsafe {
            if IsIconic(hwnd).as_bool() || GetWindowRect(hwnd, &raw mut whole).is_err() {
                return None;
            }
            let (width, height) = (whole.right - whole.left, whole.bottom - whole.top);
            if width <= 0 || height <= 0 {
                return None;
            }
            let frame = window.frame().unwrap_or(whole.into());
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // From the top down.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(bitmap) = CreateDIBSection(
                Some(dc),
                &raw const info,
                DIB_RGB_COLORS,
                &raw mut bits,
                None,
                0,
            ) else {
                let _ = DeleteDC(dc);
                return None;
            };
            let before = SelectObject(dc, bitmap.into());
            let mut picture = Self {
                window: None,
                dc,
                bitmap,
                before,
                frame,
            };
            if bits.is_null() || !PrintWindow(hwnd, dc, WHOLE_CONTENT).as_bool() {
                return None;
            }
            // What is seen of the window starts here in the picture.
            let (left, top) = (frame.left - whole.left, frame.top - whole.top);
            let (seen_width, seen_height) = (frame.right - frame.left, frame.bottom - frame.top);
            let radius = if IsZoomed(hwnd).as_bool() {
                0
            } else {
                (CORNER * GetDpiForWindow(hwnd) as f32 / 96.).round() as i32
            };
            let pixels =
                std::slice::from_raw_parts_mut(bits.cast::<u32>(), (width * height) as usize);
            for (row, line) in pixels.chunks_exact_mut(width as usize).enumerate() {
                for (column, pixel) in line.iter_mut().enumerate() {
                    let (x, y) = (column as i32 - left, row as i32 - top);
                    // The picture says nothing of how see-through it is.
                    *pixel = match outside_rounding(x, y, seen_width, seen_height, radius) {
                        true => 0,
                        false => *pixel | 0xFF00_0000,
                    };
                }
            }
            let module = GetModuleHandleW(None).ok()?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(respond),
                hInstance: module.into(),
                lpszClassName: w!("TesseraDeparture"),
                ..Default::default()
            };
            // Fails when the class is there already, which is as good.
            RegisterClassW(&raw const class);
            // Clicks go through the picture.
            let shown = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST
                    | WS_EX_NOACTIVATE,
                class.lpszClassName,
                w!(""),
                WS_POPUP,
                frame.left,
                frame.top,
                seen_width,
                seen_height,
                None,
                None,
                Some(module.into()),
                None,
            )
            .ok()?;
            picture.window = Some(shown);
            let place = POINT {
                x: frame.left,
                y: frame.top,
            };
            let size = SIZE {
                cx: seen_width,
                cy: seen_height,
            };
            let from = POINT { x: left, y: top };
            let blend = blend(1.);
            UpdateLayeredWindow(
                shown,
                None,
                Some(&raw const place),
                Some(&raw const size),
                Some(dc),
                Some(&raw const from),
                COLORREF(0),
                Some(&raw const blend),
                ULW_ALPHA,
            )
            .ok()?;
            let _ = ShowWindow(shown, SW_SHOWNA);
            Some(picture)
        }
    }

    /// Shows the picture `shift` physical pixels to the right of where the
    /// window was, as opaque as `opacity`, from 0 to 1.
    fn show(&self, shift: i32, opacity: f32) {
        let Some(window) = self.window else {
            return;
        };
        let place = POINT {
            x: self.frame.left + shift,
            y: self.frame.top,
        };
        let blend = blend(opacity);
        // SAFETY: the window belongs to this thread; what it shows stays.
        unsafe {
            let _ = UpdateLayeredWindow(
                window,
                None,
                Some(&raw const place),
                None,
                None,
                None,
                COLORREF(0),
                Some(&raw const blend),
                ULW_ALPHA,
            );
        }
    }
}

/// How a picture as opaque as `opacity` is blended with what is under it.
fn blend(opacity: f32) -> BLENDFUNCTION {
    BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: (opacity.clamp(0., 1.) * 255.).round() as u8,
        AlphaFormat: AC_SRC_ALPHA as u8,
    }
}

impl Drop for Picture {
    fn drop(&mut self) {
        // SAFETY: the window and the objects belong to this thread and
        // are not used again.
        unsafe {
            if let Some(window) = self.window {
                let _ = DestroyWindow(window);
            }
            SelectObject(self.dc, self.before);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// The window procedure of a picture, which leaves everything to Windows.
unsafe extern "system" fn respond(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: the arguments are those Windows called with.
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_way_is_covered_fast_at_first() {
        assert_eq!(eased(0.), 0.);
        assert_eq!(eased(1.), 1.);
        assert_eq!(eased(2.), 1.);
        assert!(eased(0.5) > 0.8);
    }

    #[test]
    fn only_the_tips_of_the_corners_are_cut() {
        let outside = |x, y| outside_rounding(x, y, 100, 50, 8);
        assert!(outside(0, 0));
        assert!(outside(99, 0));
        assert!(outside(0, 49));
        assert!(outside(99, 49));
        assert!(!outside(8, 0));
        assert!(!outside(0, 8));
        assert!(!outside(3, 3));
        assert!(!outside(50, 25));
        assert!(!outside_rounding(0, 0, 100, 50, 0));
    }
}
