// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The wallpaper of Windows: the picture each display has, how it is
//! fitted and the color around it, read and written.
//!
//! While Explorer runs it keeps the wallpaper, one picture for each
//! display, and answers for it through `IDesktopWallpaper`. Without
//! Explorer that object cannot be made, and what is left is what Windows
//! itself keeps in the registry (`Control Panel\Desktop`): one picture for
//! all displays and its style, which `SystemParametersInfoW` writes and
//! `PaintDesktop` draws. Settings > Personalization goes through
//! `IDesktopWallpaper`, so without Explorer a picture chosen there may not
//! be taken; a program that calls `SystemParametersInfoW` still works.
//!
//! Both [`SystemWallpaper::read`] and [`SystemWallpaper::write`] ask
//! another process and may wait for it: they are for the shell thread, or
//! another thread with COM, never for the thread of the interface.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::{COLORREF, LPARAM, WPARAM};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::Gdi::{COLOR_DESKTOP, GetSysColor, SetSysColors};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::{
    DESKTOP_WALLPAPER_POSITION, DWPOS_CENTER, DWPOS_FILL, DWPOS_FIT, DWPOS_SPAN, DWPOS_STRETCH,
    DWPOS_TILE, DesktopWallpaper, IDesktopWallpaper,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowThreadProcessId, HWND_BROADCAST, SMTO_ABORTIFHUNG, SPI_SETDESKWALLPAPER,
    SPIF_UPDATEINIFILE, SendMessageTimeoutW, SystemParametersInfoW, WM_SETTINGCHANGE,
};
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

use super::ScreenRect;
use super::app_windows::executable_of;
use super::explorer_running;

/// Where Windows keeps the picture of the desktop and its style.
const DESKTOP: PCWSTR = w!("Control Panel\\Desktop");

/// Where Windows keeps the color of the desktop, as "red green blue".
const COLORS: PCWSTR = w!("Control Panel\\Colors");

/// How long a program gets to take in that the wallpaper changed.
const TOLD_WITHIN_MS: u32 = 200;

/// How the wallpaper of Windows fits a picture to a display, with the
/// names the settings give.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WallpaperPosition {
    /// In its own size in the middle, the color around it.
    Center,
    /// In its own size, repeated from the top left.
    Tile,
    /// Given the shape of the display.
    Stretch,
    /// All of it seen, the color beside it.
    Fit,
    /// Covering the display, cut where it sticks out.
    #[default]
    Fill,
    /// One picture over all displays together.
    Span,
}

impl WallpaperPosition {
    /// The `WallpaperStyle` and `TileWallpaper` values of the registry.
    fn style(self) -> (&'static str, &'static str) {
        match self {
            Self::Center => ("0", "0"),
            Self::Tile => ("0", "1"),
            Self::Stretch => ("2", "0"),
            Self::Fit => ("6", "0"),
            Self::Fill => ("10", "0"),
            Self::Span => ("22", "0"),
        }
    }

    /// The position the registry values `style` and `tile` stand for.
    /// Windows takes an unknown style as centered.
    fn of_style(style: &str, tile: &str) -> Self {
        match style.trim() {
            "2" => Self::Stretch,
            "6" => Self::Fit,
            "10" => Self::Fill,
            "22" => Self::Span,
            _ if tile.trim() == "1" => Self::Tile,
            _ => Self::Center,
        }
    }

    fn of_explorer(position: DESKTOP_WALLPAPER_POSITION) -> Self {
        match position {
            DWPOS_TILE => Self::Tile,
            DWPOS_STRETCH => Self::Stretch,
            DWPOS_FIT => Self::Fit,
            DWPOS_FILL => Self::Fill,
            DWPOS_SPAN => Self::Span,
            _ => Self::Center,
        }
    }

    fn to_explorer(self) -> DESKTOP_WALLPAPER_POSITION {
        match self {
            Self::Center => DWPOS_CENTER,
            Self::Tile => DWPOS_TILE,
            Self::Stretch => DWPOS_STRETCH,
            Self::Fit => DWPOS_FIT,
            Self::Fill => DWPOS_FILL,
            Self::Span => DWPOS_SPAN,
        }
    }
}

/// The wallpaper of Windows at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemWallpaper {
    /// Each display with its picture; `None` where it has none and shows
    /// the color.
    pub pictures: Vec<(ScreenRect, Option<PathBuf>)>,
    pub position: WallpaperPosition,
    /// The color around a picture that does not cover a display, or of
    /// the whole desktop without one, as red, green and blue.
    pub color: [u8; 3],
}

impl SystemWallpaper {
    /// What Windows shows on the displays at `frames`: from Explorer
    /// where it runs, otherwise from the registry, where every display
    /// has the same picture. For a thread with COM; see the module.
    pub fn read(frames: &[ScreenRect]) -> Self {
        Self::from_explorer(frames).unwrap_or_else(|| Self::from_registry(frames))
    }

    /// The picture of the display at `frame`, or of the first display
    /// where none was read for that one.
    pub fn picture_of(&self, frame: ScreenRect) -> Option<&Path> {
        let own = self.pictures.iter().find(|(at, _)| *at == frame);
        own.or_else(|| self.pictures.first())
            .and_then(|(_, picture)| picture.as_deref())
    }

    fn from_explorer(frames: &[ScreenRect]) -> Option<Self> {
        let wallpaper = explorer_wallpaper()?;
        // SAFETY: the strings Explorer gives are read once and freed with
        // the allocator of COM, as its documentation asks.
        unsafe {
            let mut monitors = Vec::new();
            for index in 0..wallpaper.GetMonitorDevicePathCount().ok()? {
                let Ok(id) = wallpaper.GetMonitorDevicePathAt(index) else {
                    continue;
                };
                let monitor = Owned(id);
                let rect = wallpaper.GetMonitorRECT(monitor.as_pcwstr()).ok();
                let picture = wallpaper
                    .GetWallpaper(monitor.as_pcwstr())
                    .ok()
                    .map(Owned)
                    .and_then(|path| path.text())
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from);
                if let Some(rect) = rect {
                    monitors.push((ScreenRect::from(rect), picture));
                }
            }
            // The displays as Tessera sees them, in its order; one that
            // Explorer does not list gets the picture of the first.
            let fallback = monitors.first().and_then(|(_, picture)| picture.clone());
            let pictures = frames
                .iter()
                .map(|frame| {
                    let own = monitors.iter().find(|(rect, _)| rect == frame);
                    let picture = own.map_or_else(|| fallback.clone(), |(_, path)| path.clone());
                    (*frame, picture)
                })
                .collect();
            let position = wallpaper
                .GetPosition()
                .map_or(WallpaperPosition::Fill, |position| {
                    WallpaperPosition::of_explorer(position)
                });
            let color = wallpaper
                .GetBackgroundColor()
                .map_or_else(|_| desktop_color(), rgb);
            Some(Self {
                pictures,
                position,
                color,
            })
        }
    }

    fn from_registry(frames: &[ScreenRect]) -> Self {
        let picture = registry_text(DESKTOP, w!("WallPaper"))
            .filter(|path| !path.trim().is_empty())
            .map(PathBuf::from)
            .map(|path| {
                // Windows keeps a copy of what it shows, which stays when
                // the file it was made from is gone.
                if path.is_file() {
                    return path;
                }
                transcoded().filter(|copy| copy.is_file()).unwrap_or(path)
            });
        let style = registry_text(DESKTOP, w!("WallpaperStyle")).unwrap_or_default();
        let tile = registry_text(DESKTOP, w!("TileWallpaper")).unwrap_or_default();
        Self {
            pictures: frames
                .iter()
                .map(|frame| (*frame, picture.clone()))
                .collect(),
            position: WallpaperPosition::of_style(&style, &tile),
            color: desktop_color(),
        }
    }

    /// Makes this the wallpaper of Windows: through Explorer where it
    /// runs, each display with its own picture and a display without one
    /// left as it is, and otherwise the picture of the primary display for
    /// all, the way programs set it without Explorer. Programs are told
    /// with [`Self::announce`]. For a thread with COM; see the module.
    pub fn write(&self) -> Result<()> {
        self.write_shown(false)
    }

    /// Makes this the wallpaper of Windows again, as it was read: as
    /// [`Self::write`] does, but a display without a picture is given
    /// none, so that it shows the color.
    pub fn put_back(&self) -> Result<()> {
        self.write_shown(true)
    }

    fn write_shown(&self, clear: bool) -> Result<()> {
        if let Some(wallpaper) = explorer_wallpaper() {
            return self.write_to_explorer(&wallpaper, clear);
        }
        self.write_to_registry()
    }

    /// Tells the programs that run that the wallpaper changed, where
    /// Explorer does not, after [`Self::write`] or [`Self::put_back`].
    /// Each window gets a moment, so this can take a while; where nobody
    /// waits for it, it is best done last.
    pub fn announce() {
        if explorer_running() {
            return;
        }
        // SAFETY: the name of the section is a constant that outlives the
        // call; a program that hangs is not waited for.
        unsafe {
            let _ = SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(SPI_SETDESKWALLPAPER.0 as usize),
                LPARAM(DESKTOP.as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                TOLD_WITHIN_MS,
                None,
            );
        }
    }

    /// Whether Windows writes over the file `path` when the wallpaper
    /// changes: it is one of the copies Windows keeps of the pictures it
    /// shows (`TranscodedWallpaper` and the others in its `Themes`).
    pub fn overwritten_by_windows(path: &Path) -> bool {
        let Some(themes) = transcoded().as_deref().and_then(Path::parent).map(text_of) else {
            return false;
        };
        let path = text_of(path);
        path.len() > themes.len()
            && path.starts_with(&themes)
            && path[themes.len()..].starts_with('\\')
    }

    fn write_to_explorer(&self, wallpaper: &IDesktopWallpaper, clear: bool) -> Result<()> {
        let none = self.pictures.iter().all(|(_, picture)| picture.is_none());
        // SAFETY: as in `from_explorer`; the strings given are kept alive
        // across the calls.
        unsafe {
            wallpaper.SetPosition(self.position.to_explorer())?;
            wallpaper.SetBackgroundColor(colorref(self.color))?;
            if clear && none {
                // No picture anywhere: the color alone.
                return wallpaper
                    .Enable(false)
                    .context("Explorer did not leave the pictures out");
            }
            if !none {
                // A wallpaper turned off before has its pictures shown.
                let _ = wallpaper.Enable(true);
            }
            let count = wallpaper.GetMonitorDevicePathCount()?;
            for index in 0..count {
                let Ok(id) = wallpaper.GetMonitorDevicePathAt(index) else {
                    continue;
                };
                let monitor = Owned(id);
                let rect = wallpaper.GetMonitorRECT(monitor.as_pcwstr()).ok();
                let frame = rect.map(ScreenRect::from).unwrap_or(ScreenRect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                });
                let picture = self.picture_of(frame);
                if picture.is_none() && !clear {
                    continue;
                }
                // An empty name leaves the display to the color.
                let path = picture.map_or_else(HSTRING::new, |picture| picture.as_os_str().into());
                wallpaper
                    .SetWallpaper(monitor.as_pcwstr(), &path)
                    .with_context(|| match picture {
                        Some(picture) => format!("Explorer did not take {}", picture.display()),
                        None => "Explorer did not leave a picture out".to_owned(),
                    })?;
            }
        }
        Ok(())
    }

    fn write_to_registry(&self) -> Result<()> {
        let (style, tile) = self.position.style();
        set_registry_text(DESKTOP, w!("WallpaperStyle"), style)?;
        set_registry_text(DESKTOP, w!("TileWallpaper"), tile)?;
        let [red, green, blue] = self.color;
        set_registry_text(COLORS, w!("Background"), &format!("{red} {green} {blue}"))?;
        // SAFETY: the arrays are of one element each and outlive the call.
        unsafe {
            let index = [COLOR_DESKTOP.0];
            let color = [colorref(self.color)];
            let _ = SetSysColors(1, index.as_ptr(), color.as_ptr());
        }
        // The picture of the primary display, whose corner is where the
        // desktop begins; of the first where none is.
        let at_origin =
            |(frame, _): &&(ScreenRect, Option<PathBuf>)| frame.left == 0 && frame.top == 0;
        let primary = self
            .pictures
            .iter()
            .find(at_origin)
            .or(self.pictures.first());
        let mut path: Vec<u16> = primary
            .and_then(|(_, picture)| picture.as_deref())
            .map(|path| path.as_os_str().to_string_lossy().encode_utf16().collect())
            .unwrap_or_default();
        path.push(0);
        // SAFETY: the name is NUL-terminated and outlives the call. The
        // change is written without the broadcast `SPIF_SENDCHANGE` makes,
        // which can wait for a program that hangs; `announce` tells it
        // with a limit instead.
        unsafe {
            SystemParametersInfoW(
                SPI_SETDESKWALLPAPER,
                0,
                Some(path.as_mut_ptr().cast()),
                SPIF_UPDATEINIFILE,
            )
            .context("Windows did not take the wallpaper")?;
        }
        Ok(())
    }

    /// Whether the desktop window draws the wallpaper of Windows itself:
    /// it is that of Explorer, or Tessera's own desktop window in shell
    /// mode, which paints with `PaintDesktop`. Where it does, Tessera
    /// need not draw that wallpaper again over it.
    pub fn painted_by_desktop() -> bool {
        // SAFETY: the calls look a window up by class and read which
        // process it belongs to; `process` outlives the call.
        unsafe {
            let Ok(desktop) = FindWindowW(w!("Progman"), None) else {
                return false;
            };
            let mut process = 0_u32;
            GetWindowThreadProcessId(desktop, Some(&raw mut process));
            if process == GetCurrentProcessId() {
                return true;
            }
            executable_of(process)
                .as_deref()
                .and_then(Path::file_name)
                .is_some_and(|name| name.eq_ignore_ascii_case("explorer.exe"))
        }
    }
}

/// The name of the language and region the user chose for formats, as
/// `en-US`; `None` where Windows does not tell.
pub fn user_locale() -> Option<String> {
    let mut name = [0_u16; 85];
    // SAFETY: the buffer is of the length the call is given.
    let length = unsafe { GetUserDefaultLocaleName(&mut name) };
    let length = usize::try_from(length).ok()?.checked_sub(1)?;
    Some(String::from_utf16_lossy(&name[..length])).filter(|name| !name.is_empty())
}

/// The wallpaper object of Explorer, while Explorer runs: without it the
/// object may still be made, but has no displays to tell of.
fn explorer_wallpaper() -> Option<IDesktopWallpaper> {
    if !explorer_running() {
        return None;
    }
    // SAFETY: the call makes a COM object on this thread, which has COM.
    let wallpaper: IDesktopWallpaper =
        unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL) }.ok()?;
    // SAFETY: the call only reads a number of the object.
    let displays = unsafe { wallpaper.GetMonitorDevicePathCount() }.unwrap_or(0);
    (displays > 0).then_some(wallpaper)
}

/// `path` as text to compare as Windows does names of files, ignoring
/// case.
fn text_of(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

/// The copy Windows keeps of the picture it shows.
fn transcoded() -> Option<PathBuf> {
    let roaming = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(roaming).join(r"Microsoft\Windows\Themes\TranscodedWallpaper"))
}

/// The color of the desktop Windows uses now.
fn desktop_color() -> [u8; 3] {
    // SAFETY: the call takes a number and reads a setting.
    rgb(COLORREF(unsafe { GetSysColor(COLOR_DESKTOP) }))
}

/// `color` as red, green and blue; Windows keeps red in the low byte.
fn rgb(color: COLORREF) -> [u8; 3] {
    let [red, green, blue, _] = color.0.to_le_bytes();
    [red, green, blue]
}

fn colorref([red, green, blue]: [u8; 3]) -> COLORREF {
    COLORREF(u32::from_le_bytes([red, green, blue, 0]))
}

/// A text value under `key` of the user's part of the registry.
fn registry_text(key: PCWSTR, name: PCWSTR) -> Option<String> {
    let mut text = [0_u16; 1024];
    let mut size = size_of_val(&text) as u32;
    // SAFETY: `size` is the size in bytes of the buffer `text`, both
    // outlive the call, and the names are NUL-terminated.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    status.ok().ok()?;
    let length = text.iter().position(|&unit| unit == 0)?;
    Some(String::from_utf16_lossy(&text[..length]))
}

fn set_registry_text(key: PCWSTR, name: PCWSTR, value: &str) -> Result<()> {
    let text: Vec<u16> = value.encode_utf16().chain([0]).collect();
    // SAFETY: the data is the NUL-terminated text, of the size in bytes
    // given, and outlives the call.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            REG_SZ.0,
            Some(text.as_ptr().cast()),
            (text.len() * 2) as u32,
        )
    }
    .ok()
    .context("the registry did not take the wallpaper")
}

/// A string Explorer allocated, freed with the allocator of COM.
struct Owned(PWSTR);

impl Owned {
    fn as_pcwstr(&self) -> PCWSTR {
        PCWSTR(self.0.0)
    }

    fn text(&self) -> Option<String> {
        if self.0.is_null() {
            return None;
        }
        // SAFETY: a string Explorer gives is NUL-terminated.
        unsafe { self.0.to_string() }.ok()
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the string was allocated by COM for the caller, and is
        // freed once, here.
        unsafe { CoTaskMemFree(Some(self.0.0.cast_const().cast())) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_styles_of_the_registry_name_the_positions() {
        for position in [
            WallpaperPosition::Center,
            WallpaperPosition::Tile,
            WallpaperPosition::Stretch,
            WallpaperPosition::Fit,
            WallpaperPosition::Fill,
            WallpaperPosition::Span,
        ] {
            let (style, tile) = position.style();
            assert_eq!(WallpaperPosition::of_style(style, tile), position);
        }
        // Tiling is only for a style of 0; an unknown style is centered.
        assert_eq!(
            WallpaperPosition::of_style("10", "1"),
            WallpaperPosition::Fill
        );
        assert_eq!(
            WallpaperPosition::of_style("7", "0"),
            WallpaperPosition::Center
        );
    }

    #[test]
    fn colors_keep_red_in_the_low_byte() {
        assert_eq!(rgb(COLORREF(0x00_30_20_10)), [0x10, 0x20, 0x30]);
        assert_eq!(colorref([0x10, 0x20, 0x30]), COLORREF(0x00_30_20_10));
    }

    #[test]
    fn a_display_without_a_picture_of_its_own_takes_the_first() {
        let frame = |left| ScreenRect {
            left,
            top: 0,
            right: left + 100,
            bottom: 100,
        };
        let wallpaper = SystemWallpaper {
            pictures: vec![(frame(0), Some(PathBuf::from("a.jpg"))), (frame(100), None)],
            ..SystemWallpaper::default()
        };
        assert_eq!(wallpaper.picture_of(frame(0)), Some(Path::new("a.jpg")));
        assert_eq!(wallpaper.picture_of(frame(100)), None);
        assert_eq!(wallpaper.picture_of(frame(500)), Some(Path::new("a.jpg")));
    }
}
