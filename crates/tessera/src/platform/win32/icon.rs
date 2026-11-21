// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Executable icons as raw pixels.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, ReleaseDC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetIconInfo, HICON, ICONINFO, PrivateExtractIconsW,
};

/// A square-ish icon as top-down, straight-alpha BGRA pixels.
#[derive(Clone, Debug)]
pub struct IconPixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl IconPixels {
    /// Extracts an icon of `file`, an executable, a library or an icon
    /// file, scaled to `size` pixels. `index` counts the icons in the file
    /// from zero; a negative one is a resource identifier. Returns `None`
    /// when the file has no such icon or cannot be read.
    pub fn of_file(file: &Path, index: i32, size: u32) -> Option<Self> {
        let mut path = [0u16; 260];
        let wide: Vec<u16> = file.as_os_str().encode_wide().collect();
        if wide.len() >= path.len() {
            return None;
        }
        path[..wide.len()].copy_from_slice(&wide);

        let mut icons = [HICON::default()];
        // SAFETY: `path` is NUL-terminated and `icons` has room for the one
        // icon requested.
        let extracted = unsafe {
            PrivateExtractIconsW(
                &path,
                index,
                size as i32,
                size as i32,
                Some(&mut icons),
                None,
                0,
            )
        };
        let icon = icons[0];
        if extracted == 0 || extracted == u32::MAX || icon.is_invalid() {
            return None;
        }
        let pixels = Self::of_icon(icon);
        // SAFETY: `icon` was created above and is not used afterwards.
        unsafe {
            let _ = DestroyIcon(icon);
        }
        pixels
    }

    /// The pixels of `icon`, which stays the caller's to destroy.
    pub(super) fn of_icon(icon: HICON) -> Option<Self> {
        // SAFETY: every structure passed is correctly sized, the pixel buffer
        // matches the dimensions given to GetDIBits, and every GDI object
        // obtained here is released before returning.
        unsafe {
            let mut info = ICONINFO::default();
            GetIconInfo(icon, &raw mut info).ok()?;
            let color = info.hbmColor;
            let mut bitmap = BITMAP::default();
            let described = GetObjectW(
                color.into(),
                size_of::<BITMAP>() as i32,
                Some((&raw mut bitmap).cast()),
            );
            let (width, height) = (bitmap.bmWidth, bitmap.bmHeight);

            let mut bgra = vec![0u8; (width.max(0) * height.max(0) * 4) as usize];
            let mut copied = 0;
            if described != 0 && !bgra.is_empty() {
                let mut header = BITMAPINFO {
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
                let screen = GetDC(None);
                copied = GetDIBits(
                    screen,
                    color,
                    0,
                    height as u32,
                    Some(bgra.as_mut_ptr().cast()),
                    &raw mut header,
                    DIB_RGB_COLORS,
                );
                ReleaseDC(None, screen);
            }
            let _ = DeleteObject(color.into());
            let _ = DeleteObject(info.hbmMask.into());
            if copied == 0 {
                return None;
            }

            // Icons without an alpha channel report every pixel as transparent.
            let pixels = bgra.as_chunks_mut::<4>().0;
            if pixels.iter().all(|pixel| pixel[3] == 0) {
                pixels.iter_mut().for_each(|pixel| pixel[3] = 0xff);
            }
            Some(Self {
                width: width as u32,
                height: height as u32,
                bgra,
            })
        }
    }
}
