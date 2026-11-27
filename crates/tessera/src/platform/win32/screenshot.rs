// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A picture of every display saved as a file, as Win+PrtScn does in
//! Windows: in the Screenshots folder of Pictures, named "Screenshot (N)"
//! with the number Windows counts on with.
//!
//! The picture is copied from the screen with GDI and written as PNG,
//! which takes a moment; it is to be done away from the thread that draws.

use std::fs::OpenOptions;
use std::io::{BufWriter, ErrorKind};
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use image::{ImageFormat, RgbaImage};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_DWORD, RegSetKeyValueW};
use windows::Win32::UI::Shell::{FOLDERID_Screenshots, KF_FLAG_CREATE, SHGetKnownFolderPath};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};
use windows::core::w;

use super::files::wide;
use super::registry::registry_dword;

/// Where Explorer keeps the number of the next screenshot.
const EXPLORER: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer";

/// How many numbers are tried past the one Windows counted to, where
/// files of those names exist already.
const TRIES: u32 = 10_000;

/// Takes a picture of all displays and saves it; the answer is the file.
pub fn save_screenshot() -> Result<PathBuf> {
    let picture = capture()?;
    let folder = screenshots_folder().context("there is no folder for screenshots")?;
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("{} could not be made", folder.display()))?;
    let first = next_index().unwrap_or(1).max(1);
    for number in first..first.saturating_add(TRIES) {
        let path = folder.join(file_name(number));
        // A file that exists is never overwritten.
        let file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => bail!("{} could not be written: {error}", path.display()),
        };
        let written = picture.write_to(&mut BufWriter::new(file), ImageFormat::Png);
        if let Err(error) = written {
            std::fs::remove_file(&path).ok();
            bail!("{} could not be written: {error}", path.display());
        }
        store_next_index(number + 1);
        return Ok(path);
    }
    bail!(
        "every name for a screenshot in {} is taken",
        folder.display()
    )
}

/// What the screenshot with the number `number` is called, as Windows
/// calls it.
fn file_name(number: u32) -> String {
    format!("Screenshot ({number}).png")
}

/// Pixels as GDI gives them, blue, green, red and an unused byte, made
/// red, green, blue and opaque.
fn opaque_rgba(mut bgra: Vec<u8>) -> Vec<u8> {
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = u8::MAX;
    }
    bgra
}

/// The Screenshots folder of the user, made where it is missing.
fn screenshots_folder() -> Option<PathBuf> {
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
            windows::core::PCWSTR(key.as_ptr()),
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

/// The pixels of every display, as they are arranged.
fn capture() -> Result<RgbaImage> {
    // SAFETY: the calls only read the arrangement of the displays.
    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if width <= 0 || height <= 0 {
        bail!("there is no display to take a picture of");
    }
    let mut bgra = vec![0_u8; width as usize * height as usize * 4];
    // SAFETY: every object made here is selected out and deleted, and the
    // device context of the screen released, before returning; the
    // buffer has the size of the bitmap described to GetDIBits.
    let copied = unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let before = SelectObject(memory, bitmap.into());
        // Layered windows, such as Tessera's own, are copied as well.
        let blitted = BitBlt(
            memory,
            0,
            0,
            width,
            height,
            Some(screen),
            left,
            top,
            SRCCOPY | CAPTUREBLT,
        );
        SelectObject(memory, before);
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative for rows from the top down.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let rows = match blitted {
            Ok(()) => GetDIBits(
                memory,
                bitmap,
                0,
                height as u32,
                Some(bgra.as_mut_ptr().cast()),
                &raw mut header,
                DIB_RGB_COLORS,
            ),
            Err(_) => 0,
        };
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        rows
    };
    if copied != height {
        bail!("the screen could not be copied");
    }
    RgbaImage::from_raw(width as u32, height as u32, opaque_rgba(bgra))
        .context("the picture of the screen has the wrong size")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screenshot_is_named_with_its_number() {
        assert_eq!(file_name(7), "Screenshot (7).png");
    }

    #[test]
    fn pixels_of_gdi_become_opaque_rgba() {
        let pixels = opaque_rgba(vec![1, 2, 3, 0, 10, 20, 30, 0]);
        assert_eq!(pixels, [3, 2, 1, 255, 30, 20, 10, 255]);
    }
}
