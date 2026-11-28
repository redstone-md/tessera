// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Live pictures of other windows, drawn by Windows inside a surface.

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Dwm::{
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE,
    DWM_TNP_VISIBLE, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
    DwmUpdateThumbnailProperties,
};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, WM_DWMSENDICONICLIVEPREVIEWBITMAP, WM_DWMSENDICONICTHUMBNAIL,
};

use super::{AppWindowId, NativeSurface, ScreenRect};

/// A picture of a window that Windows keeps up to date and draws over the
/// content of a surface, until dropped.
#[derive(Debug)]
pub struct Thumbnail(isize);

impl Thumbnail {
    /// Starts picturing `window` on `surface`. Nothing is drawn before
    /// [`Self::place`] says where.
    pub fn of(window: AppWindowId, surface: NativeSurface) -> Result<Self> {
        // SAFETY: the call tolerates handles whose windows are gone, and
        // fails for them.
        let handle = unsafe { DwmRegisterThumbnail(surface.hwnd, window.hwnd()) }
            .context("Windows gave no picture of the window")?;
        Ok(Self(handle))
    }

    /// The width and height of the pictured window, in physical pixels.
    pub fn source_size(&self) -> Option<(i32, i32)> {
        // SAFETY: the handle is a registered thumbnail.
        let size = unsafe { DwmQueryThumbnailSourceSize(self.0) }.ok()?;
        (size.cx > 0 && size.cy > 0).then_some((size.cx, size.cy))
    }

    /// Draws the picture into `area`, in physical pixels from the top left
    /// corner of the surface, as opaque as `opacity`, from 0 to 1.
    pub fn place(&self, area: ScreenRect, opacity: f32) {
        self.place_part(area, None, opacity);
    }

    /// Draws `source`, a part of the pictured window in its own physical
    /// pixels, or all of it, into `area`, as [`Self::place`] does; the part
    /// an application asked its picture to show with `SetThumbnailClip`.
    pub fn place_part(&self, area: ScreenRect, source: Option<ScreenRect>, opacity: f32) {
        let mut flags = DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY | DWM_TNP_VISIBLE;
        if source.is_some() {
            flags |= DWM_TNP_RECTSOURCE;
        }
        let source = source.unwrap_or(area);
        let properties = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: flags,
            rcSource: RECT {
                left: source.left,
                top: source.top,
                right: source.right,
                bottom: source.bottom,
            },
            rcDestination: RECT {
                left: area.left,
                top: area.top,
                right: area.right,
                bottom: area.bottom,
            },
            opacity: (opacity.clamp(0., 1.) * 255.).round() as u8,
            fVisible: true.into(),
            ..Default::default()
        };
        // SAFETY: `properties` outlives the call and the handle is a
        // registered thumbnail. A failure leaves the picture where it was.
        unsafe {
            let _ = DwmUpdateThumbnailProperties(self.0, &raw const properties);
        }
    }
}

impl Thumbnail {
    /// Asks `window` for the picture it gives of itself where it draws no
    /// live one, as a window that stands for a tab does: with `thumbnail`
    /// for a small picture at most that wide and high, or else for the
    /// picture of the whole window that a peek shows. Windows sends these
    /// messages to the windows that said they give such pictures
    /// (`DWMWA_HAS_ICONIC_BITMAP`), and they answer by handing the picture
    /// to the compositor (`DwmSetIconicThumbnail`,
    /// `DwmSetIconicLivePreviewBitmap`); other windows leave the messages
    /// to Windows, which does nothing with them.
    ///
    /// Whether the compositor then shows such a picture in a thumbnail
    /// another program registered, rather than only in the taskbar of
    /// Explorer, is not documented; the test
    /// `iconic_pictures_reach_registered_thumbnails` finds out on a
    /// machine. The message is posted, so a program that hangs holds
    /// nothing up.
    pub fn ask_for_iconic(window: AppWindowId, thumbnail: Option<(u16, u16)>) {
        let (message, size) = match thumbnail {
            Some((width, height)) => (
                WM_DWMSENDICONICTHUMBNAIL,
                ((u32::from(width) << 16) | u32::from(height)) as isize,
            ),
            None => (WM_DWMSENDICONICLIVEPREVIEWBITMAP, 0),
        };
        // SAFETY: posting to a window that has since been destroyed fails
        // without side effects.
        unsafe {
            let _ = PostMessageW(Some(window.hwnd()), message, WPARAM(0), LPARAM(size));
        }
    }
}

impl Drop for Thumbnail {
    fn drop(&mut self) {
        // SAFETY: the handle was registered by `of` and is not used again.
        unsafe {
            let _ = DwmUnregisterThumbnail(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{COLORREF, HWND, LRESULT};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_FORCE_ICONIC_REPRESENTATION, DWMWA_HAS_ICONIC_BITMAP, DwmInvalidateIconicBitmaps,
        DwmSetIconicLivePreviewBitmap, DwmSetIconicThumbnail, DwmSetWindowAttribute,
    };
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, CreateSolidBrush, DIB_RGB_COLORS,
        DeleteObject, GetDC, GetPixel, HBITMAP, HGDIOBJ, ReleaseDC,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE,
        PeekMessageW, RegisterClassW, SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage, WNDCLASSW,
        WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
    };
    use windows::core::{BOOL, w};

    use super::*;

    /// Whether the compositor asked the test window for each kind of
    /// picture by itself.
    static ASKED_THUMBNAIL: AtomicBool = AtomicBool::new(false);
    static ASKED_PREVIEW: AtomicBool = AtomicBool::new(false);

    /// A red bitmap `width` by `height`, which its caller deletes.
    fn red(width: i32, height: i32) -> HBITMAP {
        let (width, height) = (width.max(1), height.max(1));
        let info = BITMAPINFO {
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
        };
        let mut bits = std::ptr::null_mut();
        // SAFETY: the bitmap is made with the size `info` gives, and its
        // memory is filled within that size.
        unsafe {
            let bitmap = CreateDIBSection(
                None,
                &raw const info,
                DIB_RGB_COLORS,
                &raw mut bits,
                None,
                0,
            )
            .unwrap();
            let pixels =
                std::slice::from_raw_parts_mut(bits.cast::<u32>(), (width * height) as usize);
            pixels.fill(0xFFFF_0000);
            bitmap
        }
    }

    /// The window procedure of the test windows: the source gives a red
    /// picture of itself whenever the compositor asks for one.
    unsafe extern "system" fn respond(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        // SAFETY: the arguments are those Windows called with; each bitmap
        // is deleted once the compositor has copied it.
        unsafe {
            match message {
                WM_DWMSENDICONICTHUMBNAIL => {
                    ASKED_THUMBNAIL.store(true, Ordering::Relaxed);
                    let (width, height) = ((l.0 >> 16) as i32 & 0xFFFF, l.0 as i32 & 0xFFFF);
                    let bitmap = red(width, height);
                    let _ = DwmSetIconicThumbnail(hwnd, bitmap, 0);
                    let _ = DeleteObject(HGDIOBJ(bitmap.0));
                    LRESULT(0)
                }
                WM_DWMSENDICONICLIVEPREVIEWBITMAP => {
                    ASKED_PREVIEW.store(true, Ordering::Relaxed);
                    let bitmap = red(300, 200);
                    let _ = DwmSetIconicLivePreviewBitmap(hwnd, bitmap, None, 0);
                    let _ = DeleteObject(HGDIOBJ(bitmap.0));
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, message, w, l),
            }
        }
    }

    /// Handles the messages of this thread for `time`.
    fn pump(time: Duration) {
        let until = Instant::now() + time;
        let mut message = MSG::default();
        while Instant::now() < until {
            // SAFETY: `message` outlives the calls, which hand the messages
            // of this thread to its windows.
            unsafe {
                while PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&raw const message);
                    DispatchMessageW(&raw const message);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The spike of iconic pictures: a green window that says it gives
    /// pictures of itself and gives red ones, pictured by a thumbnail
    /// registered in another window. Prints whether the compositor asked
    /// for a picture by itself, and whether the thumbnail shows green (the
    /// window as it is drawn: iconic pictures reach only the taskbar of
    /// Explorer) or red (they reach any registered thumbnail, so previews
    /// can ask for them with `Thumbnail::ask_for_iconic`). Run with
    /// `--ignored --nocapture` on a desktop with nothing over the top left
    /// corner of the primary display.
    #[test]
    #[ignore = "needs the compositor of Windows and a display to look at"]
    fn iconic_pictures_reach_registered_thumbnails() {
        // SAFETY: the windows are made, shown and destroyed on this thread,
        // and every attribute is passed with its size.
        unsafe {
            let module = GetModuleHandleW(None).unwrap();
            let class = WNDCLASSW {
                lpfnWndProc: Some(respond),
                hInstance: module.into(),
                hbrBackground: CreateSolidBrush(COLORREF(0x0000_FF00)),
                lpszClassName: w!("TesseraIconicSpike"),
                ..Default::default()
            };
            RegisterClassW(&raw const class);
            let window = |extended, style, left| {
                CreateWindowExW(
                    extended,
                    class.lpszClassName,
                    w!("iconic"),
                    style,
                    left,
                    0,
                    300,
                    200,
                    None,
                    None,
                    Some(module.into()),
                    None,
                )
                .unwrap()
            };
            let source = window(Default::default(), WS_OVERLAPPEDWINDOW | WS_VISIBLE, 400);
            let yes = BOOL::from(true);
            for attribute in [DWMWA_FORCE_ICONIC_REPRESENTATION, DWMWA_HAS_ICONIC_BITMAP] {
                let set = DwmSetWindowAttribute(
                    source,
                    attribute,
                    (&raw const yes).cast(),
                    size_of::<BOOL>() as u32,
                );
                println!("attribute {}: {set:?}", attribute.0);
            }
            let target = window(WS_EX_TOPMOST, WS_POPUP, 0);
            let _ = ShowWindow(target, SW_SHOWNOACTIVATE);
            let surface = NativeSurface { hwnd: target };
            let id = AppWindowId::from_raw(source.0 as isize);
            let thumbnail = Thumbnail::of(id, surface).unwrap();
            let area = ScreenRect {
                left: 0,
                top: 0,
                right: 300,
                bottom: 200,
            };
            thumbnail.place(area, 1.);
            let _ = DwmInvalidateIconicBitmaps(source);
            pump(Duration::from_millis(500));
            println!(
                "asked by the compositor: thumbnail {}, preview {}",
                ASKED_THUMBNAIL.load(Ordering::Relaxed),
                ASKED_PREVIEW.load(Ordering::Relaxed)
            );
            Thumbnail::ask_for_iconic(id, Some((300, 200)));
            Thumbnail::ask_for_iconic(id, None);
            pump(Duration::from_millis(500));
            let screen = GetDC(None);
            let color = GetPixel(screen, 150, 100).0;
            ReleaseDC(None, screen);
            let seen = match color & 0x00FF_FFFF {
                0x0000_00FF => "red: iconic pictures reach registered thumbnails",
                0x0000_FF00 => "green: registered thumbnails show the window as drawn",
                _ => "neither: something covers the thumbnail, or it shows nothing",
            };
            println!("the thumbnail shows {color:06x}, {seen}");
            drop(thumbnail);
            let _ = DestroyWindow(target);
            let _ = DestroyWindow(source);
        }
    }
}
