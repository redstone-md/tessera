// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Windows that leave the screen sideways and come back the same way,
//! instead of vanishing and appearing.
//!
//! A window of another program cannot be moved smoothly or faded from
//! outside, and Windows stops picturing it the moment it is hidden. What
//! moves is a still picture of it, taken before it is hidden and shown in
//! a window of its own right over it. A smaller copy of the picture is
//! kept while the window is hidden, and is what is seen coming back before
//! the window itself is shown. Each motion has a thread of its own. The
//! same copy is what [`Passage::still`] gives of a hidden window, for the
//! views that show the windows of other workspaces: Windows has no live
//! picture of a hidden window to give them.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    DWMWCP_ROUND, DwmFlush, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HALFTONE,
    HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject, SetStretchBltMode, StretchDIBits,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowRect, IsIconic, IsZoomed, LWA_ALPHA,
    RegisterClassW, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    SetLayeredWindowAttributes, SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow,
    WINDOW_EX_STYLE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, w};

use super::{AppWindowId, ScreenRect};

/// How long a window takes to leave or to come.
const DURATION: Duration = Duration::from_millis(260);

/// How long the picture of a window that came takes to fade from over the
/// window itself.
const HANDOVER: Duration = Duration::from_millis(90);

/// How long a window that is shown is waited for before its picture goes.
const SHOWN_WAIT: Duration = Duration::from_millis(150);

/// How far a window travels while it leaves or comes, in physical pixels.
const TRAVEL: f32 = 280.;

/// The radius Windows 11 rounds the corners of a window with, in logical
/// pixels.
const CORNER: f32 = 8.;

/// How many pixels the pictures kept of hidden windows may have together:
/// 16 MB of them. They are kept at half the width and height, so that is
/// as much as three windows that fill a 3440 by 1440 display.
const KEPT_PIXELS: usize = 4 << 20;

/// Asks for a picture of all of a window, also of what it draws with the
/// graphics card.
const WHOLE_CONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);

/// The extended styles of a window that shows a picture: above the other
/// windows, not one of them, and clicks go through it.
const OVERLAID: WINDOW_EX_STYLE = WINDOW_EX_STYLE(
    WS_EX_LAYERED.0
        | WS_EX_TRANSPARENT.0
        | WS_EX_TOOLWINDOW.0
        | WS_EX_TOPMOST.0
        | WS_EX_NOACTIVATE.0,
);

/// The pictures kept of hidden windows, the oldest first.
static KEPT: Mutex<Vec<Kept>> = Mutex::new(Vec::new());

/// The motion of windows that are hidden and shown.
pub struct Passage;

/// The picture kept of a hidden window: `width` by `height` pixels of
/// four bytes, blue, green, red and alpha, from the top row down, the
/// colors premultiplied by the alpha. The cut corners of a rounded window
/// are transparent.
#[derive(Clone, Debug)]
pub struct Still {
    pub width: u32,
    pub height: u32,
    pub bgra: Arc<[u8]>,
}

impl Passage {
    /// Hides `windows`, which are seen leaving to the left or, with
    /// `rightward`, to the right. A window no picture can be taken of,
    /// and a minimized one, is hidden without the motion.
    pub fn leave(windows: Vec<AppWindowId>, rightward: bool) {
        if windows.is_empty() {
            return;
        }
        let leaving = windows.clone();
        let started = thread::Builder::new()
            .name("passage".to_owned())
            .spawn(move || leave(&leaving, rightward));
        if started.is_err() {
            for window in windows {
                window.hide();
            }
        }
    }

    /// Shows `windows` again and brings `front` forward. A window that
    /// was seen leaving, and has the place and size it had then, is seen
    /// coming from the left or, with `from_right`, from the right; the
    /// others appear as Windows shows them.
    pub fn enter(windows: Vec<AppWindowId>, front: Option<AppWindowId>, from_right: bool) {
        if windows.is_empty() {
            if let Some(front) = front {
                front.activate();
            }
            return;
        }
        let entering = windows.clone();
        let started = thread::Builder::new()
            .name("passage".to_owned())
            .spawn(move || enter(&entering, front, from_right));
        if started.is_err() {
            for window in windows {
                window.unhide();
            }
            if let Some(front) = front {
                front.activate();
            }
        }
    }

    /// The picture kept of `window` while it is hidden, at half its width
    /// and height: the one taken as it was seen leaving. There is none of
    /// a window that is shown, that was hidden without the motion (as a
    /// minimized one is), or whose picture made room for newer ones.
    pub fn still(window: AppWindowId) -> Option<Still> {
        let all = KEPT.lock().ok()?;
        let kept = all.iter().find(|kept| kept.window == window)?;
        Some(Still {
            width: kept.width as u32,
            height: kept.rows() as u32,
            bgra: kept.pixels.clone(),
        })
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

/// Waits for the next picture of the screen.
fn next_frame() {
    // SAFETY: the call waits for the screen to be drawn and fails without
    // side effects.
    if unsafe { DwmFlush() }.is_err() {
        thread::sleep(Duration::from_millis(8));
    }
}

/// Calls `step` with the share of `duration` that has passed, once for
/// every picture of the screen until all of it has.
fn animate(duration: Duration, mut step: impl FnMut(f32)) {
    let start = Instant::now();
    loop {
        let share = start.elapsed().as_secs_f32() / duration.as_secs_f32();
        if share >= 1. {
            break;
        }
        step(share);
        next_frame();
    }
}

/// Shows pictures of `windows` over them, hides the windows and moves the
/// pictures away.
fn leave(windows: &[AppWindowId], rightward: bool) {
    let pictures: Vec<Picture> = windows
        .iter()
        .filter_map(|&window| Picture::take(window))
        .collect();
    // The pictures are on the screen before the windows go.
    if !pictures.is_empty() {
        next_frame();
    }
    // Windows fades a window that is hidden where it stands, which would
    // be seen next to its picture.
    for &window in windows {
        fade_in_place(window, false);
        window.hide();
    }
    if !pictures.is_empty() {
        let direction = if rightward { 1. } else { -1. };
        animate(DURATION, |share| {
            let shift = (eased(share) * TRAVEL * direction).round() as i32;
            for picture in &pictures {
                picture.show(shift, 1. - eased(share));
            }
        });
    }
    for picture in pictures {
        if let Some(kept) = picture.keep() {
            Kept::store(kept);
        }
    }
    for &window in windows {
        fade_in_place(window, true);
    }
}

/// Moves the pictures kept of `windows` to where the windows are, shows
/// the windows under them, brings `front` forward and takes the pictures
/// away.
fn enter(windows: &[AppWindowId], front: Option<AppWindowId>, from_right: bool) {
    let direction = if from_right { 1. } else { -1. };
    let start = (TRAVEL * direction).round() as i32;
    let mut pictured = Vec::new();
    let mut returns = Vec::new();
    for &window in windows {
        match Kept::take(window).and_then(|kept| Return::of(&kept, start)) {
            Some(back) => {
                pictured.push(window);
                returns.push(back);
            }
            // A window without a picture appears as Windows shows it.
            None => {
                window.unhide();
                if front == Some(window) {
                    window.activate();
                }
            }
        }
    }
    if returns.is_empty() {
        if let Some(front) = front.filter(|front| !windows.contains(front)) {
            front.activate();
        }
        return;
    }
    animate(DURATION, |share| {
        let shift = ((1. - eased(share)) * TRAVEL * direction).round() as i32;
        for back in &returns {
            back.show(shift, eased(share));
        }
    });
    for back in &returns {
        back.show(0, 1.);
    }
    // The windows appear at once under their pictures.
    for &window in &pictured {
        fade_in_place(window, false);
        window.unhide();
    }
    let asked = Instant::now();
    while pictured.iter().any(|window| !window.is_shown()) && asked.elapsed() < SHOWN_WAIT {
        next_frame();
    }
    next_frame();
    if let Some(front) = front.filter(|front| pictured.contains(front) || !windows.contains(front))
    {
        front.activate();
    }
    // The pictures are less sharp than the windows, so they fade from
    // over them.
    animate(HANDOVER, |share| {
        for back in &returns {
            back.show(0, 1. - share);
        }
    });
    drop(returns);
    for &window in &pictured {
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

/// Where a window is, with the invisible border Windows gives it.
fn whole_frame(window: AppWindowId) -> Option<ScreenRect> {
    let mut whole = RECT::default();
    // SAFETY: `whole` outlives the call, which a window that is gone makes
    // fail.
    unsafe { GetWindowRect(window.hwnd(), &raw mut whole) }.ok()?;
    Some(whole.into())
}

/// Makes a window for a picture at `frame`, not shown yet.
fn overlay(frame: ScreenRect) -> Option<HWND> {
    // SAFETY: the class and the window belong to this thread; the window
    // procedure is a function of this program.
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(respond),
            hInstance: module.into(),
            lpszClassName: w!("TesseraPassage"),
            ..Default::default()
        };
        // Fails when the class is there already, which is as good.
        RegisterClassW(&raw const class);
        CreateWindowExW(
            OVERLAID,
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
        .ok()
    }
}

/// A still picture of a window in a window of its own, until dropped.
struct Picture {
    /// The pictured window.
    source: AppWindowId,
    /// The window that shows the picture, once there is one.
    window: Option<HWND>,
    dc: HDC,
    bitmap: HBITMAP,
    /// What the device context held before the bitmap.
    before: HGDIOBJ,
    /// The pixels of the bitmap, `width` in a row, from the top.
    bits: *mut u32,
    width: i32,
    /// Where the pictured window is with its invisible border, which is
    /// what the bitmap covers.
    whole: ScreenRect,
    /// Where the pictured window is seen on the screen.
    frame: ScreenRect,
    /// Whether the corners of the window are rounded.
    rounded: bool,
}

impl Picture {
    /// Pictures `window` and shows the picture where the window is.
    fn take(window: AppWindowId) -> Option<Self> {
        let hwnd = window.hwnd();
        // SAFETY: the calls read the state of a window that may be gone,
        // which makes them fail, and make objects of this thread that the
        // picture frees when dropped. The bits of the bitmap are as many
        // as its width and height say and are not used after it.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                return None;
            }
            let whole = whole_frame(window)?;
            let (width, height) = (whole.right - whole.left, whole.bottom - whole.top);
            if width <= 0 || height <= 0 {
                return None;
            }
            let frame = window.frame().unwrap_or(whole);
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let info = bitmap_info(width, height);
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
            let rounded = !IsZoomed(hwnd).as_bool();
            let mut picture = Self {
                source: window,
                window: None,
                dc,
                bitmap,
                before,
                bits: bits.cast(),
                width,
                whole,
                frame,
                rounded,
            };
            if bits.is_null() || !PrintWindow(hwnd, dc, WHOLE_CONTENT).as_bool() {
                return None;
            }
            // What is seen of the window starts here in the picture.
            let (left, top) = (frame.left - whole.left, frame.top - whole.top);
            let (seen_width, seen_height) = (frame.right - frame.left, frame.bottom - frame.top);
            let radius = match rounded {
                true => (CORNER * GetDpiForWindow(hwnd) as f32 / 96.).round() as i32,
                false => 0,
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
            let shown = overlay(frame)?;
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

    /// A copy of what is seen of the window at half the width and height,
    /// to keep while the window is hidden.
    fn keep(&self) -> Option<Kept> {
        let (left, top) = (
            self.frame.left - self.whole.left,
            self.frame.top - self.whole.top,
        );
        let width = (self.frame.right - self.frame.left) / 2;
        let height = (self.frame.bottom - self.frame.top) / 2;
        let rows = self.whole.bottom - self.whole.top;
        let inside =
            left >= 0 && top >= 0 && left + width * 2 <= self.width && top + height * 2 <= rows;
        if width <= 0 || height <= 0 || !inside {
            return None;
        }
        // SAFETY: the bits are those of the bitmap, which lives as long as
        // the picture and has `rows` rows of `self.width` pixels.
        let pixels = unsafe { std::slice::from_raw_parts(self.bits, (self.width * rows) as usize) };
        let at = |x: i32, y: i32| pixels[((top + y) * self.width + left + x) as usize];
        let mut halved: Vec<u8> = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let four = [
                    at(x * 2, y * 2),
                    at(x * 2 + 1, y * 2),
                    at(x * 2, y * 2 + 1),
                    at(x * 2 + 1, y * 2 + 1),
                ];
                halved.extend(mean(four).to_le_bytes());
            }
        }
        Some(Kept {
            window: self.source,
            pixels: halved.into(),
            width,
            whole: self.whole,
            frame: self.frame,
            rounded: self.rounded,
        })
    }
}

/// The pixel between `four` pixels: the mean of each of their colors.
fn mean(four: [u32; 4]) -> u32 {
    let mut pixel = 0;
    for shift in [0, 8, 16, 24] {
        let sum: u32 = four.iter().map(|pixel| (pixel >> shift) & 0xFF).sum();
        pixel |= ((sum + 2) / 4) << shift;
    }
    pixel
}

/// Describes a bitmap of 32 bits a pixel, `width` by `height`, from the
/// top down.
fn bitmap_info(width: i32, height: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
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

/// What is kept of a hidden window: a small picture of it and where it
/// was.
struct Kept {
    window: AppWindowId,
    /// The picture, `width` pixels of four bytes in a row, from the top;
    /// shared with the stills given out of it.
    pixels: Arc<[u8]>,
    width: i32,
    whole: ScreenRect,
    frame: ScreenRect,
    rounded: bool,
}

impl Kept {
    /// How many rows of pixels the picture has.
    fn rows(&self) -> i32 {
        (self.pixels.len() / 4) as i32 / self.width.max(1)
    }

    /// Keeps `kept` in place of what was kept of its window, and lets go
    /// of the oldest pictures and of those of windows that are gone until
    /// the rest is within what may be kept.
    fn store(kept: Self) {
        let Ok(mut all) = KEPT.lock() else {
            return;
        };
        all.retain(|other| other.window != kept.window && other.window.exists());
        all.push(kept);
        let sizes: Vec<usize> = all.iter().map(|kept| kept.pixels.len() / 4).collect();
        all.drain(..droppable(&sizes, KEPT_PIXELS));
    }

    /// What is kept of `window`, if it still is where it was and as large.
    fn take(window: AppWindowId) -> Option<Self> {
        let mut all = KEPT.lock().ok()?;
        let place = all.iter().position(|kept| kept.window == window)?;
        let kept = all.remove(place);
        (whole_frame(window) == Some(kept.whole)).then_some(kept)
    }
}

/// How many of the first of `sizes` are to go for the rest to sum to
/// `most` or less.
fn droppable(sizes: &[usize], most: usize) -> usize {
    let mut sum: usize = sizes.iter().sum();
    let mut dropped = 0;
    while sum > most && dropped < sizes.len() {
        sum -= sizes[dropped];
        dropped += 1;
    }
    dropped
}

/// The picture kept of a window, in a window of its own where the window
/// will be shown, until dropped.
struct Return {
    window: HWND,
    frame: ScreenRect,
}

impl Return {
    /// Shows what was `kept`, not to be seen yet, `shift` physical pixels
    /// to the right of where its window was.
    fn of(kept: &Kept, shift: i32) -> Option<Self> {
        let frame = kept.frame;
        let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
        let rows = kept.rows();
        let window = overlay(frame)?;
        let back = Self { window, frame };
        let corners = match kept.rounded {
            true => DWMWCP_ROUND,
            false => DWMWCP_DONOTROUND,
        };
        let info = bitmap_info(kept.width, rows);
        // SAFETY: the window belongs to this thread, `corners` is as large
        // as the call is told, and the pixels are as many as `info` says
        // and outlive the call that reads them.
        unsafe {
            let _ = DwmSetWindowAttribute(
                window,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&raw const corners).cast(),
                size_of_val(&corners) as u32,
            );
            SetLayeredWindowAttributes(window, COLORREF(0), 0, LWA_ALPHA).ok()?;
            back.show(shift, 0.);
            let _ = ShowWindow(window, SW_SHOWNA);
            let dc = GetDC(Some(window));
            if dc.is_invalid() {
                return None;
            }
            SetStretchBltMode(dc, HALFTONE);
            let drawn = StretchDIBits(
                dc,
                0,
                0,
                width,
                height,
                0,
                0,
                kept.width,
                rows,
                Some(kept.pixels.as_ptr().cast()),
                &raw const info,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
            ReleaseDC(Some(window), dc);
            (drawn != 0).then_some(back)
        }
    }

    /// Shows the picture `shift` physical pixels to the right of where the
    /// window was, as opaque as `opacity`, from 0 to 1.
    fn show(&self, shift: i32, opacity: f32) {
        let alpha = (opacity.clamp(0., 1.) * 255.).round() as u8;
        // SAFETY: the window belongs to this thread.
        unsafe {
            let _ = SetLayeredWindowAttributes(self.window, COLORREF(0), alpha, LWA_ALPHA);
            let _ = SetWindowPos(
                self.window,
                None,
                self.frame.left + shift,
                self.frame.top,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
    }
}

impl Drop for Return {
    fn drop(&mut self) {
        // SAFETY: the window belongs to this thread and is not used again.
        unsafe {
            let _ = DestroyWindow(self.window);
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

    #[test]
    fn four_pixels_become_the_mean_of_their_colors() {
        assert_eq!(mean([0xFF10_2030; 4]), 0xFF10_2030);
        assert_eq!(mean([0xFF00_0000, 0xFF00_0000, 0, 0]), 0x8000_0000);
        assert_eq!(mean([0x0000_00FF, 0x0000_0001, 0, 0]), 0x0000_0040);
    }

    #[test]
    fn the_oldest_pictures_go_until_the_rest_may_be_kept() {
        assert_eq!(droppable(&[], 10), 0);
        assert_eq!(droppable(&[4, 5], 10), 0);
        assert_eq!(droppable(&[4, 5, 6], 10), 2);
        assert_eq!(droppable(&[4, 5, 6], 11), 1);
        assert_eq!(droppable(&[40], 10), 1);
    }
}
