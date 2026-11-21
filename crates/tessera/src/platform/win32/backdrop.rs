// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A window where the wallpaper is: behind the icons of the desktop.
//!
//! Windows documents no way to put one there. Since Windows 11 24H2 the
//! desktop window of Explorer, `Progman`, holds the icons
//! (`SHELLDLL_DefView`) and below them the wallpaper (`WorkerW`) as its
//! children, once it was sent an undocumented message; a window made a
//! child between the two is drawn over the wallpaper and under the icons.
//! Earlier versions lay the desktop out differently, which is not handled
//! here.
//!
//! The window is a plain one that copies a bitmap to the screen itself: a
//! picture then costs its pixels once for each display, and nothing of
//! what a window of the user interface takes for drawing.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr;

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLACK_BRUSH, BLENDFUNCTION, BeginPaint, BitBlt,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint,
    EnumDisplayMonitors, FillRect, GdiAlphaBlend, GetDC, GetStockObject, HBITMAP, HBRUSH, HDC,
    HGDIOBJ, HMONITOR, MapWindowPoints, PAINTSTRUCT, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, FindWindowW, GWLP_USERDATA,
    GetWindowLongPtrW, LWA_ALPHA, RegisterClassW, SMTO_NORMAL, SW_SHOWNA, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SendMessageTimeoutW, SetLayeredWindowAttributes, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, WM_ERASEBKGND, WM_NCDESTROY, WM_PAINT, WNDCLASSW, WS_CHILD,
    WS_EX_LAYERED, WS_EX_NOACTIVATE,
};
use windows::core::{BOOL, w};

use super::ScreenRect;

/// The message that has `Progman` lay the desktop out with the wallpaper
/// as a child of its own, and the parameters it is known to take.
const RAISE_DESKTOP: u32 = 0x052C;
const RAISE_DESKTOP_W: usize = 0xD;
const RAISE_DESKTOP_L: isize = 1;

/// How long `Progman` gets to answer, in milliseconds.
const ANSWER_TIME: u32 = 1000;

/// The bounds of every display, in pixels of the desktop.
pub fn display_frames() -> Vec<ScreenRect> {
    unsafe extern "system" fn add(_: HMONITOR, _: HDC, frame: *mut RECT, frames: LPARAM) -> BOOL {
        // SAFETY: `frames` is the pointer `display_frames` passed, to a
        // list that outlives the enumeration, and `frame` is valid for
        // the duration of the call.
        unsafe {
            let frames = &mut *(frames.0 as *mut Vec<ScreenRect>);
            frames.push((*frame).into());
        }
        true.into()
    }
    let mut frames: Vec<ScreenRect> = Vec::new();
    // SAFETY: the enumeration is synchronous and `frames` outlives it.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&raw mut frames as isize));
    }
    frames
}

/// The pixels of a picture for a [`DesktopBackdrop`]: rows from the top,
/// each [`Self::stride`] bytes long, a pixel being blue, green and red.
pub struct BackdropPicture {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl BackdropPicture {
    /// How many bytes a row of `width` pixels takes: Windows has rows end
    /// on a multiple of four bytes.
    pub fn stride(width: u32) -> usize {
        (width as usize * 3).next_multiple_of(4)
    }

    /// The width and the height in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The bytes of the picture.
    #[cfg(test)]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// A picture of `pixels`, which must hold `height` rows.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        assert_eq!(pixels.len(), Self::stride(width) * height as usize);
        Self {
            width,
            height,
            pixels,
        }
    }
}

/// The bitmap a backdrop window shows, selected into a drawing context
/// of its own.
struct Held {
    context: HDC,
    bitmap: HBITMAP,
    /// What the context held before the bitmap, to give back.
    before: HGDIOBJ,
}

impl Drop for Held {
    fn drop(&mut self) {
        // SAFETY: the handles were made by `DesktopBackdrop::set` and are
        // released once, here.
        unsafe {
            SelectObject(self.context, self.before);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.context);
        }
    }
}

/// A window that fills one display behind the icons of the desktop and
/// over the wallpaper of Windows, and shows one picture. It gets no input
/// and belongs to the thread that opened it.
pub struct DesktopBackdrop {
    hwnd: HWND,
    width: i32,
    height: i32,
    /// Whether the window has come to be seen.
    revealed: Cell<bool>,
}

impl DesktopBackdrop {
    /// Opens the window over the display at `frame`, not yet to be seen.
    /// Fails where the desktop is not laid out as Windows 11 24H2 does.
    pub fn open(frame: ScreenRect) -> Result<Self> {
        let width = frame.right - frame.left;
        let height = frame.bottom - frame.top;
        // SAFETY: the calls look windows up by class, send one message
        // with a time limit, and make and place a window of this thread;
        // `corner` outlives the call that maps it.
        unsafe {
            let desktop = FindWindowW(w!("Progman"), None).context("there is no desktop window")?;
            let parts = |desktop: HWND| {
                let icons =
                    FindWindowExW(Some(desktop), None, w!("SHELLDLL_DefView"), None).ok()?;
                FindWindowExW(Some(desktop), None, w!("WorkerW"), None).ok()?;
                Some(icons)
            };
            let icons = match parts(desktop) {
                Some(icons) => icons,
                None => {
                    SendMessageTimeoutW(
                        desktop,
                        RAISE_DESKTOP,
                        WPARAM(RAISE_DESKTOP_W),
                        LPARAM(RAISE_DESKTOP_L),
                        SMTO_NORMAL,
                        ANSWER_TIME,
                        None,
                    );
                    let Some(icons) = parts(desktop) else {
                        bail!("the desktop of this Windows is not laid out in a way Tessera knows");
                    };
                    icons
                }
            };

            let module = GetModuleHandleW(None).context("the program has no module")?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(respond),
                hInstance: module.into(),
                lpszClassName: w!("TesseraBackdrop"),
                ..Default::default()
            };
            // Fails when the class is there already, which is as good.
            RegisterClassW(&raw const class);

            // A child is placed in the coordinates of its parent.
            let mut corner = [POINT {
                x: frame.left,
                y: frame.top,
            }];
            MapWindowPoints(None, Some(desktop), &mut corner);
            // Windows makes and draws a layered child only for a program
            // whose manifest says it is written for Windows 8 or later,
            // as that of Tessera does.
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE,
                class.lpszClassName,
                w!(""),
                WS_CHILD,
                corner[0].x,
                corner[0].y,
                width,
                height,
                Some(desktop),
                None,
                Some(module.into()),
                None,
            )
            .context("the wallpaper window could not be made")?;
            let backdrop = Self {
                hwnd,
                width,
                height,
                revealed: Cell::new(false),
            };
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA)
                .context("the window could not be made a layer of the desktop")?;
            // After the icons in the order of the children is below them,
            // and the wallpaper stays the last.
            SetWindowPos(
                hwnd,
                Some(icons),
                corner[0].x,
                corner[0].y,
                width,
                height,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("the window could not be put behind the icons")?;
            Ok(backdrop)
        }
    }

    /// The size of the window, which a picture for it must have.
    pub fn size(&self) -> (u32, u32) {
        (self.width.unsigned_abs(), self.height.unsigned_abs())
    }

    fn held(&self) -> *mut Held {
        // SAFETY: the call only reads a value kept with the window.
        unsafe { GetWindowLongPtrW(self.hwnd, GWLP_USERDATA) as *mut Held }
    }

    /// Takes `picture` as the one to show, and forgets the one before.
    /// What is on the screen stays until [`Self::advance`] changes it.
    pub fn set(&self, picture: &BackdropPicture) -> Result<()> {
        if (picture.width, picture.height) != self.size() {
            bail!("the picture is not of the size of the display");
        }
        // First the old bitmap goes, so that there are never two.
        // SAFETY: the pointer is one `set` stored and nothing else frees
        // while the window lives; the window procedure runs on this
        // thread, so it does not use the bitmap meanwhile.
        unsafe {
            let before = self.held();
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            if !before.is_null() {
                drop(Box::from_raw(before));
            }
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: self.width,
                // A negative height has the rows start at the top.
                biHeight: -self.height,
                biPlanes: 1,
                biBitCount: 24,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: `info` describes a bitmap of exactly the bytes of
        // `picture`, which are copied into the memory Windows made for
        // it; the handles are kept in `Held`, which releases them.
        unsafe {
            let mut bits: *mut c_void = ptr::null_mut();
            let bitmap = CreateDIBSection(
                None,
                &raw const info,
                DIB_RGB_COLORS,
                &raw mut bits,
                None,
                0,
            )
            .context("there is no memory for the wallpaper")?;
            if bits.is_null() {
                let _ = DeleteObject(bitmap.into());
                bail!("there is no memory for the wallpaper");
            }
            ptr::copy_nonoverlapping(
                picture.pixels.as_ptr(),
                bits.cast::<u8>(),
                picture.pixels.len(),
            );
            let context = CreateCompatibleDC(None);
            let before = SelectObject(context, bitmap.into());
            let held = Box::new(Held {
                context,
                bitmap,
                before,
            });
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, Box::into_raw(held) as isize);
        }
        Ok(())
    }

    /// Moves what is on the screen towards the picture: `from` and `to`
    /// say how far the change was and is to be now, from 0 to 1, where 1
    /// shows the picture alone. The first picture of the window fades in
    /// over the wallpaper of Windows, a later one over the one before.
    pub fn advance(&self, from: f32, to: f32) {
        let held = self.held();
        if held.is_null() {
            return;
        }
        let to = to.clamp(0., 1.);
        let from = from.clamp(0., to);
        let byte = |share: f32| (share * 255.).round().clamp(0., 255.) as u8;
        // SAFETY: `held` is alive as in `set`; the drawing context of the
        // window is released after use.
        unsafe {
            let source = (*held).context;
            if !self.revealed.get() {
                let _ = SetLayeredWindowAttributes(self.hwnd, COLORREF(0), byte(to), LWA_ALPHA);
                // Shown, the window is asked to paint the picture.
                let _ = ShowWindow(self.hwnd, SW_SHOWNA);
                self.revealed.set(to >= 1.);
                return;
            }
            let screen = GetDC(Some(self.hwnd));
            if to >= 1. {
                let _ = BitBlt(
                    screen,
                    0,
                    0,
                    self.width,
                    self.height,
                    Some(source),
                    0,
                    0,
                    SRCCOPY,
                );
            } else if to > from {
                // Laid over what is shown, which already holds `from` of
                // the picture, this much of it makes `to`.
                let blend = BLENDFUNCTION {
                    SourceConstantAlpha: byte((to - from) / (1. - from)),
                    ..Default::default()
                };
                let _ = GdiAlphaBlend(
                    screen,
                    0,
                    0,
                    self.width,
                    self.height,
                    source,
                    0,
                    0,
                    self.width,
                    self.height,
                    blend,
                );
            }
            ReleaseDC(Some(self.hwnd), screen);
        }
    }
}

impl Drop for DesktopBackdrop {
    fn drop(&mut self) {
        // SAFETY: the window belongs to this thread; its bitmap is freed
        // by the window procedure as the window goes.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// The window procedure of a backdrop: paints the bitmap, or black
/// before there is one, and frees the bitmap with the window.
unsafe extern "system" fn respond(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: the value kept with the window is null or a `Held` that
    // `DesktopBackdrop::set` boxed, freed here or there and nowhere else;
    // the paint structure outlives the calls that use it.
    unsafe {
        let held = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Held;
        match message {
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let screen = BeginPaint(hwnd, &raw mut paint);
                let RECT {
                    left,
                    top,
                    right,
                    bottom,
                } = paint.rcPaint;
                if held.is_null() {
                    FillRect(
                        screen,
                        &raw const paint.rcPaint,
                        HBRUSH(GetStockObject(BLACK_BRUSH).0),
                    );
                } else {
                    let source = Some((*held).context);
                    let _ = BitBlt(
                        screen,
                        left,
                        top,
                        right - left,
                        bottom - top,
                        source,
                        left,
                        top,
                        SRCCOPY,
                    );
                }
                let _ = EndPaint(hwnd, &raw const paint);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if !held.is_null() {
                    drop(Box::from_raw(held));
                }
                DefWindowProcW(hwnd, message, w, l)
            }
            _ => DefWindowProcW(hwnd, message, w, l),
        }
    }
}
