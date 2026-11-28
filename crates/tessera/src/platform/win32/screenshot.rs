// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Where screenshots are saved and how a file of one is written, as
//! Win+PrtScn does in Windows: in the Screenshots folder of Pictures, or
//! another folder, never over a file that is there, and with the number
//! Windows counts on with where the name asks for it.
//!
//! [`render_text`] draws a line of text the way the annotations of a
//! screenshot need it: as how much each pixel is covered, to be put on the
//! picture in any colour.
//!
//! Writing a picture takes a moment; it is to be done away from the thread
//! that draws.

use std::fs::OpenOptions;
use std::io::{BufWriter, ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use image::{GrayImage, ImageFormat, RgbaImage};
use windows::Win32::Foundation::{COLORREF, ERROR_SUCCESS, RECT};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DT_CALCRECT, DT_NOPREFIX, DeleteDC, DeleteObject, DrawTextW, FW_NORMAL,
    FW_SEMIBOLD, OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_DWORD, RegSetKeyValueW};
use windows::Win32::UI::Shell::{FOLDERID_Screenshots, KF_FLAG_CREATE, SHGetKnownFolderPath};
use windows::Win32::UI::WindowsAndMessaging::{
    LWA_ALPHA, SetLayeredWindowAttributes, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT,
};
use windows::core::{PCWSTR, w};

use super::files::wide;
use super::registry::registry_dword;

/// Where Explorer keeps the number of the next screenshot.
const EXPLORER: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer";

/// How many numbers are tried past the first, where files of those names
/// exist already.
const TRIES: u32 = 10_000;

/// The largest side of a text drawn by [`render_text`], in pixels.
const TEXT_SIDE: i32 = 8192;

/// Writes `picture` as `format` into `folder`, made where it is missing,
/// under the first name `names` gives for 1, 2, 3 and on that no file has.
/// With `counted` the numbers start at the one Windows counts its own
/// screenshots with, and Windows counts on from the one taken, as
/// Win+PrtScn does; the name then holds the number. The answer is the
/// file.
pub fn save_screenshot(
    picture: &RgbaImage,
    folder: &Path,
    format: ImageFormat,
    counted: bool,
    names: impl Fn(u32) -> String,
) -> Result<PathBuf> {
    std::fs::create_dir_all(folder)
        .with_context(|| format!("{} could not be made", folder.display()))?;
    let first = match counted {
        true => next_index().unwrap_or(1).max(1),
        false => 1,
    };
    let bytes = encode_picture(picture, format)?;
    let mut last = None;
    for number in first..first.saturating_add(TRIES) {
        let name = names(number);
        // The same name for every number is tried once.
        if last.as_ref() == Some(&name) {
            break;
        }
        let path = folder.join(&name);
        last = Some(name);
        // A file that exists is never overwritten.
        let file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => bail!("{} could not be written: {error}", path.display()),
        };
        let mut writer = BufWriter::new(file);
        let written = writer.write_all(&bytes).and_then(|()| writer.flush());
        if let Err(error) = written {
            drop(writer);
            std::fs::remove_file(&path).ok();
            bail!("{} could not be written: {error}", path.display());
        }
        if counted {
            store_next_index(number + 1);
        }
        return Ok(path);
    }
    bail!(
        "every name for a screenshot in {} is taken",
        folder.display()
    )
}

/// The bytes of a file of `picture` as `format`: a JPEG without its
/// transparency, which JPEG has none of; PNG and WebP keep it, WebP
/// without loss.
pub fn encode_picture(picture: &RgbaImage, format: ImageFormat) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut bytes);
    let written = match format {
        ImageFormat::Jpeg => {
            let opaque = image::DynamicImage::ImageRgba8(picture.clone()).into_rgb8();
            opaque.write_to(&mut cursor, format)
        }
        _ => picture.write_to(&mut cursor, format),
    };
    written.with_context(|| format!("the picture could not be written as {format:?}"))?;
    Ok(bytes)
}

/// The Screenshots folder of the user, made where it is missing.
pub fn screenshots_folder() -> Option<PathBuf> {
    // SAFETY: the identifier outlives the call; the text Windows
    // allocates is copied and then freed exactly once.
    unsafe {
        let text = SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_CREATE, None).ok()?;
        let path = text.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(text.0.cast()));
        path
    }
}

/// The number Windows gives the next screenshot.
fn next_index() -> Option<u32> {
    registry_dword(HKEY_CURRENT_USER, EXPLORER, "ScreenshotIndex")
}

/// Has Windows count on from `number`, so that its own next screenshot
/// does not look for a name that is taken.
fn store_next_index(number: u32) {
    let key = wide(EXPLORER);
    // SAFETY: the names are NUL-terminated, and the value is a number of
    // the size that is passed.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            w!("ScreenshotIndex"),
            REG_DWORD.0,
            Some((&raw const number).cast()),
            size_of::<u32>() as u32,
        )
    };
    if status != ERROR_SUCCESS {
        report!("the number of the next screenshot was not kept: {status:?}");
    }
}

/// `text` drawn in Segoe UI, `size` pixels high and bold if `bold`, as how
/// much of each pixel the letters cover, 0 to 255; lines break where the
/// text breaks. `None` for an empty text or one that could not be drawn.
pub fn render_text(text: &str, size: u32, bold: bool) -> Option<GrayImage> {
    if text.trim().is_empty() {
        return None;
    }
    let mut wide_text: Vec<u16> = text.encode_utf16().collect();
    let height = -(size.clamp(4, 512) as i32);
    let weight = if bold { FW_SEMIBOLD } else { FW_NORMAL };
    // SAFETY: every object made here is selected out and deleted before
    // returning; the bits of the section are read only while it lives,
    // and no further than its size.
    unsafe {
        let context = CreateCompatibleDC(None);
        if context.is_invalid() {
            return None;
        }
        let font = CreateFontW(
            height,
            0,
            0,
            0,
            weight.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            // Gray smoothing: ClearType would colour the edges.
            ANTIALIASED_QUALITY,
            DEFAULT_PITCH.0 as u32,
            w!("Segoe UI"),
        );
        let before_font = SelectObject(context, font.into());
        let mut bounds = RECT::default();
        DrawTextW(
            context,
            &mut wide_text,
            &raw mut bounds,
            DT_CALCRECT | DT_NOPREFIX,
        );
        let (width, rows) = (
            (bounds.right - bounds.left).clamp(1, TEXT_SIDE),
            (bounds.bottom - bounds.top).clamp(1, TEXT_SIDE),
        );
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -rows,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let section = CreateDIBSection(
            Some(context),
            &raw const info,
            DIB_RGB_COLORS,
            &raw mut bits,
            None,
            0,
        );
        let mask = match section {
            Ok(bitmap) if !bits.is_null() => {
                let before = SelectObject(context, bitmap.into());
                SetBkMode(context, TRANSPARENT);
                SetTextColor(context, COLORREF(0x00FF_FFFF));
                let mut place = RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: rows,
                };
                DrawTextW(context, &mut wide_text, &raw mut place, DT_NOPREFIX);
                let pixels =
                    std::slice::from_raw_parts(bits.cast::<u8>(), (width * rows * 4) as usize);
                // White on black: any one channel is the cover.
                let cover: Vec<u8> = pixels.chunks_exact(4).map(|pixel| pixel[1]).collect();
                SelectObject(context, before);
                let _ = DeleteObject(bitmap.into());
                GrayImage::from_raw(width as u32, rows as u32, cover)
            }
            Ok(bitmap) => {
                let _ = DeleteObject(bitmap.into());
                None
            }
            Err(_) => None,
        };
        SelectObject(context, before_font);
        let _ = DeleteObject(font.into());
        let _ = DeleteDC(context);
        mask
    }
}

impl super::NativeSurface {
    /// Turns the window into one that shows something over the screen for
    /// a moment, as the seconds before a screenshot: no caption or frame,
    /// above every other window, never in front, not listed among the
    /// windows of applications, and the pointer goes through it.
    pub fn adopt_as_click_through(self) -> Result<()> {
        self.adopt(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT)?;
        // SAFETY: the window is a live one of this thread. A layered
        // window is drawn once it is told how opaque it is.
        unsafe { SetLayeredWindowAttributes(self.hwnd, COLORREF(0), 255, LWA_ALPHA) }
            .context("the window could not be made to let the pointer through")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jpeg_loses_only_the_transparency() {
        let picture = RgbaImage::from_pixel(4, 4, image::Rgba([200, 10, 10, 128]));
        let jpeg = encode_picture(&picture, ImageFormat::Jpeg).unwrap();
        let read = image::load_from_memory(&jpeg).unwrap().into_rgba8();
        assert_eq!(read.dimensions(), (4, 4));
        assert!(read.pixels().all(|pixel| pixel[3] == 255));
        let png = encode_picture(&picture, ImageFormat::Png).unwrap();
        assert_eq!(image::load_from_memory(&png).unwrap().into_rgba8(), picture);
    }
}
