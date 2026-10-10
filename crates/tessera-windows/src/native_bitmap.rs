// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Bounded GDI copies shared by native icons and actual Shell thumbnails.

use std::mem::size_of;

use windows_sys::Win32::Graphics::Gdi::{
    BITMAP, CreateCompatibleDC as sys_CreateCompatibleDC, DeleteDC as sys_DeleteDC,
    GetDIBits as sys_GetDIBits, GetObjectW as sys_GetObjectW, HBITMAP as SYS_HBITMAP,
};

#[derive(Clone, Copy)]
pub(crate) enum BitmapAlpha {
    Premultiplied,
    Opaque,
}

pub(crate) struct BitmapPixels {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

/// Converts a GDI bitmap into top-down premultiplied RGBA bytes.
///
/// One-pass: `GetObjectW` supplies dimensions, then `GetDIBits` fills a
/// forced 32-bpp top-down buffer, avoiding the header round-trip.
pub(crate) fn copy_bitmap(bitmap: SYS_HBITMAP, alpha: BitmapAlpha) -> Option<BitmapPixels> {
    if bitmap.is_null() {
        return None;
    }
    let mut header = BITMAP::default();
    // SAFETY: header is a valid writable BITMAP for the queried handle.
    if unsafe {
        sys_GetObjectW(
            bitmap.cast(),
            size_of::<BITMAP>() as i32,
            (&mut header as *mut BITMAP).cast(),
        )
    } == 0
    {
        return None;
    }
    let (width, height) = (header.bmWidth, header.bmHeight);
    const MAX_EDGE: i32 = 128;
    if width <= 0 || height == 0 || width > MAX_EDGE || height.unsigned_abs() > MAX_EDGE as u32 {
        return None;
    }
    let rows = height.unsigned_abs() as usize;
    // SAFETY: a memory DC without a referencing window; deleted on all paths.
    let context = unsafe { sys_CreateCompatibleDC(std::ptr::null_mut()) };
    if context.is_null() {
        return None;
    }
    let result = (|| {
        let stride = (width as usize * 4).next_multiple_of(4);
        let mut pixels = vec![0u8; stride * rows];
        let mut info = windows_sys::Win32::Graphics::Gdi::BITMAPINFO {
            bmiHeader: windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER {
                biSize: size_of::<windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: request top-down rows regardless of source order.
                biHeight: -(rows as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: windows_sys::Win32::Graphics::Gdi::BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: pixels is exactly `stride * rows`; GetDIBits writes at most
        // that much for a full-height 32-bpp top-down copy.
        let copied = unsafe {
            sys_GetDIBits(
                context,
                bitmap,
                0,
                rows as u32,
                pixels.as_mut_ptr().cast(),
                &mut info,
                windows_sys::Win32::Graphics::Gdi::DIB_RGB_COLORS,
            )
        };
        if copied != rows as i32 {
            return None;
        }
        let mut rgba = Vec::with_capacity(width as usize * rows * 4);
        for row in 0..rows {
            // GetDIBits already converted to requested top-down row order.
            let base = row * stride;
            for pixel in 0..width as usize {
                // 32-bpp BI_RGB memory order is B, G, R, (A).
                let offset = base + pixel * 4;
                rgba.push(pixels[offset + 2]);
                rgba.push(pixels[offset + 1]);
                rgba.push(pixels[offset]);
                rgba.push(match alpha {
                    BitmapAlpha::Premultiplied => pixels[offset + 3],
                    BitmapAlpha::Opaque => 255,
                });
            }
        }
        if rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0) {
            return None;
        }
        Some(BitmapPixels {
            width: width as u32,
            height: rows as u32,
            rgba,
        })
    })();
    // SAFETY: this thread created the memory DC above.
    unsafe { sys_DeleteDC(context) };
    result
}
