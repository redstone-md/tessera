// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A border Tessera draws around the tiled window in front: a window of
//! its own that clicks go through, that never takes the focus and that
//! has no button in the dock, laid just above the window it surrounds.
//! Its corners are rounded as Windows rounds those of the window.
//!
//! The window belongs to a thread of its own, which does nothing else, so
//! that drawing the border never holds up the shell; what it is to show
//! is handed over and the thread told to look.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, Once, OnceLock, PoisonError};
use std::{ptr, thread};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    DWMWCP_ROUNDSMALL, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GW_HWNDPREV, GetMessageW, GetWindow,
    HWND_TOP, MSG, PostMessageW, RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos, ShowWindow, TranslateMessage, ULW_ALPHA,
    UpdateLayeredWindow, WM_APP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::w;

use super::registry::registry_text;
use super::{AppWindowId, ScreenRect};

/// What the thread of the border is told to look at.
const LOOK: u32 = WM_APP + 1;

/// How round Windows makes the corners of windows, in pixels at 96 dots
/// per inch, and of those it rounds less.
const ROUND: i32 = 8;
const ROUND_SMALL: i32 = 4;

/// The first build of Windows 11, which rounds the corners of windows.
const ROUNDING_BUILD: u32 = 22000;

/// A border around a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Wanted {
    around: AppWindowId,
    /// What is seen of the window.
    frame: ScreenRect,
    width: i32,
    offset: i32,
    /// As 0xAARRGGBB.
    color: u32,
}

/// What the window of the border shows now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Drawn {
    width: i32,
    height: i32,
    thickness: i32,
    radius: i32,
    color: u32,
}

static WANTED: Mutex<Option<Wanted>> = Mutex::new(None);
/// The window of the border, once its thread made it.
static WINDOW: AtomicIsize = AtomicIsize::new(0);

/// The border Tessera draws around the tiled window in front.
pub struct TileBorder;

impl TileBorder {
    /// Draws a border `width` pixels wide, `offset` pixels out of `frame`,
    /// what is seen of the window `around`, in `color` as 0xAARRGGBB.
    pub fn show(around: AppWindowId, frame: ScreenRect, width: i32, offset: i32, color: u32) {
        let wanted = Wanted {
            around,
            frame,
            width: width.max(1),
            offset: offset.max(0),
            color,
        };
        *WANTED.lock().unwrap_or_else(PoisonError::into_inner) = Some(wanted);
        start();
        tell();
    }

    /// Takes the border away.
    pub fn hide() {
        *WANTED.lock().unwrap_or_else(PoisonError::into_inner) = None;
        tell();
    }
}

/// Tells the thread of the border to look at what it is to show.
fn tell() {
    let window = WINDOW.load(Ordering::Acquire);
    if window != 0 {
        // SAFETY: posting to a window that has since been destroyed fails
        // without side effects.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(window as *mut c_void)),
                LOOK,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

/// Starts the thread of the border, once.
fn start() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let started = thread::Builder::new()
            .name("tessera tile border".to_owned())
            .spawn(run);
        if let Err(error) = started {
            report!("the border of the tiled window in front is not drawn: {error}");
        }
    });
}

/// Makes the window of the border and answers it, for as long as the
/// process runs.
fn run() {
    // SAFETY: the calls register a class and make a window of this
    // thread; the names are literals.
    let window = unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            return;
        };
        let class = WNDCLASSW {
            lpfnWndProc: Some(respond),
            hInstance: module.into(),
            lpszClassName: w!("TesseraTileBorder"),
            ..Default::default()
        };
        RegisterClassW(&raw const class);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class.lpszClassName,
            w!(""),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(module.into()),
            None,
        )
    };
    let window = match window {
        Ok(window) => window,
        Err(error) => {
            report!("the border of the tiled window in front could not be made: {error}");
            return;
        }
    };
    WINDOW.store(window.0 as isize, Ordering::Release);
    look(window);
    let mut message = MSG::default();
    // SAFETY: `message` outlives every call; the loop only hands the
    // messages of this thread on to where they belong.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

unsafe extern "system" fn respond(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == LOOK {
        look(hwnd);
        return LRESULT(0);
    }
    // SAFETY: what was given is passed on as it is.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

thread_local! {
    static DRAWN: RefCell<Option<Drawn>> = const { RefCell::new(None) };
}

/// Shows what the border is to be now, on its thread.
fn look(window: HWND) {
    let wanted = *WANTED.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(wanted) = wanted else {
        DRAWN.with_borrow_mut(|drawn| *drawn = None);
        // SAFETY: the window belongs to this thread.
        unsafe {
            let _ = ShowWindow(window, SW_HIDE);
        }
        return;
    };
    let out = wanted.offset + wanted.width;
    let (left, top) = (wanted.frame.left - out, wanted.frame.top - out);
    let width = wanted.frame.right - wanted.frame.left + 2 * out;
    let height = wanted.frame.bottom - wanted.frame.top + 2 * out;
    if width <= 0 || height <= 0 {
        return;
    }
    let corner = corner_radius(wanted.around);
    let drawn = Drawn {
        width,
        height,
        thickness: wanted.width,
        radius: if corner > 0 { corner + out } else { 0 },
        color: wanted.color,
    };
    let same = DRAWN.with_borrow(|before| *before == Some(drawn));
    if !same {
        if !paint(window, drawn, (left, top)) {
            return;
        }
        DRAWN.with_borrow_mut(|before| *before = Some(drawn));
    }
    // Just above the window it surrounds, so that windows above that one
    // stay above the border too.
    // SAFETY: the calls only read the order of windows and move this
    // thread's own window, and tolerate a handle whose window is gone.
    unsafe {
        // Where the border already lies just above it, it keeps its place:
        // raising it to the top would cover the window's own dialogs.
        let (after, kept) = match GetWindow(wanted.around.hwnd(), GW_HWNDPREV) {
            Ok(above) if above == window => (HWND_TOP, true),
            Ok(above) if !above.is_invalid() => (above, false),
            _ => (HWND_TOP, false),
        };
        let flags = SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOSIZE;
        let flags = if same { flags } else { flags | SWP_NOMOVE };
        let flags = if kept { flags | SWP_NOZORDER } else { flags };
        let _ = SetWindowPos(window, Some(after), left, top, 0, 0, flags);
    }
}

/// How round the corners of the window `id` are, in pixels of the screen.
fn corner_radius(id: AppWindowId) -> i32 {
    let mut preference = DWM_WINDOW_CORNER_PREFERENCE::default();
    // SAFETY: `preference` is as large as the call is told and outlives
    // it, and a window that is gone makes the calls fail.
    let (read, dpi) = unsafe {
        let read = DwmGetWindowAttribute(
            id.hwnd(),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw mut preference).cast(),
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
        (read, GetDpiForWindow(id.hwnd()))
    };
    let radius = match read {
        Ok(()) if preference == DWMWCP_DONOTROUND => 0,
        Ok(()) if preference == DWMWCP_ROUNDSMALL => ROUND_SMALL,
        Ok(()) => ROUND,
        // Windows may not tell; Windows 11 rounds unless asked not to.
        Err(_) if build() >= ROUNDING_BUILD => ROUND,
        Err(_) => 0,
    };
    let dpi = if dpi == 0 { 96 } else { dpi };
    (radius as f32 * dpi as f32 / 96.).round() as i32
}

/// The build number of Windows, or 0 where it cannot be read.
fn build() -> u32 {
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(|| {
        let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let text = registry_text(HKEY_LOCAL_MACHINE, key, Some("CurrentBuildNumber"), 16);
        text.and_then(|text| text.trim().parse().ok()).unwrap_or(0)
    })
}

/// Hands the pixels of `drawn` to the window, at `(left, top)` on the
/// screen. Whether Windows took them is returned.
fn paint(window: HWND, drawn: Drawn, (left, top): (i32, i32)) -> bool {
    let pixels = ring(
        drawn.width,
        drawn.height,
        drawn.thickness,
        drawn.radius,
        drawn.color,
    );
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: drawn.width,
            // A negative height has the rows start at the top.
            biHeight: -drawn.height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `info` describes a bitmap of exactly the pixels of
    // `pixels`, which are copied into the memory Windows made for it; the
    // contexts and the bitmap are released before returning.
    unsafe {
        let mut bits: *mut c_void = ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(
            None,
            &raw const info,
            DIB_RGB_COLORS,
            &raw mut bits,
            None,
            0,
        ) else {
            return false;
        };
        if bits.is_null() {
            let _ = DeleteObject(bitmap.into());
            return false;
        }
        ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u32>(), pixels.len());
        let screen = GetDC(None);
        let source = CreateCompatibleDC(Some(screen));
        let before = SelectObject(source, bitmap.into());
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let place = POINT { x: left, y: top };
        let size = SIZE {
            cx: drawn.width,
            cy: drawn.height,
        };
        let from = POINT::default();
        let updated = UpdateLayeredWindow(
            window,
            Some(screen),
            Some(&raw const place),
            Some(&raw const size),
            Some(source),
            Some(&raw const from),
            COLORREF(0),
            Some(&raw const blend),
            ULW_ALPHA,
        );
        SelectObject(source, before);
        let _ = DeleteDC(source);
        let _ = DeleteObject(bitmap.into());
        ReleaseDC(None, screen);
        updated.is_ok()
    }
}

/// The pixels, row by row from the top, of a ring `thickness` wide along
/// the edges of a rectangle `width` by `height` whose outer corners are
/// rounded by `radius`, in `color` (0xAARRGGBB), with their colors taken
/// by how much of each is covered, as Windows blends them.
pub fn ring(width: i32, height: i32, thickness: i32, radius: i32, color: u32) -> Vec<u32> {
    let (width, height) = (width.max(1), height.max(1));
    let mut pixels = vec![0u32; (width * height) as usize];
    let half = (width as f32 / 2., height as f32 / 2.);
    let thickness = thickness.max(1) as f32;
    let radius = (radius.max(0) as f32).min(half.0).min(half.1);
    let inner_radius = (radius - thickness).max(0.);
    let alpha = (color >> 24) as f32 / 255.;
    let channel = |shift: u32| ((color >> shift) & 0xff) as f32;
    let paint = |x: i32, y: i32| -> u32 {
        let point = (x as f32 + 0.5 - half.0, y as f32 + 0.5 - half.1);
        let outer = covered(point, half, radius);
        let inner = covered(
            point,
            (half.0 - thickness, half.1 - thickness),
            inner_radius,
        );
        let share = (outer - inner).clamp(0., 1.) * alpha;
        if share <= 0. {
            return 0;
        }
        let part = |value: f32| (value * share).round() as u32;
        part(255.) << 24 | part(channel(16)) << 16 | part(channel(8)) << 8 | part(channel(0))
    };
    // Rows away from the corners are covered only near their ends.
    let reach = (thickness + radius).ceil() as i32 + 1;
    let band = (thickness.ceil() as i32 + 1).min(width);
    for y in 0..height {
        let row = &mut pixels[(y * width) as usize..((y + 1) * width) as usize];
        let straight = y >= reach && y < height - reach;
        if straight {
            for x in (0..band).chain(width - band..width) {
                row[x as usize] = paint(x, y);
            }
        } else {
            for x in 0..width {
                row[x as usize] = paint(x, y);
            }
        }
    }
    pixels
}

/// How much of the pixel around `point` lies in the rectangle centred on
/// the origin that reaches `half` out each way, with corners rounded by
/// `radius`: from 0 to 1, by the distance to its edge.
fn covered((x, y): (f32, f32), half: (f32, f32), radius: f32) -> f32 {
    if half.0 <= 0. || half.1 <= 0. {
        return 0.;
    }
    let (qx, qy) = (x.abs() - (half.0 - radius), y.abs() - (half.1 - radius));
    let outside = (qx.max(0.).powi(2) + qy.max(0.).powi(2)).sqrt();
    let distance = outside + qx.max(qy).min(0.) - radius;
    (0.5 - distance).clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_covers_the_edges_and_leaves_the_middle_clear() {
        let red = 0xffff_0000;
        let pixels = ring(20, 10, 2, 0, red);
        let at = |x: usize, y: usize| pixels[y * 20 + x];
        assert_eq!(at(0, 0), red);
        assert_eq!(at(1, 5), red);
        assert_eq!(at(18, 5), red);
        assert_eq!(at(2, 5), 0);
        assert_eq!(at(10, 5), 0);
        assert_eq!(at(10, 9), red);
    }

    #[test]
    fn rounded_corners_are_clear_outside_their_curve() {
        let half_green = 0x8000_ff00;
        let pixels = ring(20, 10, 2, 4, half_green);
        let at = |x: usize, y: usize| pixels[y * 20 + x];
        assert_eq!(at(0, 0), 0);
        // Half the color, as Windows blends it: taken by its alpha.
        assert_eq!(at(10, 0), 0x8000_8000);
        assert_eq!(at(10, 5), 0);
    }
}
