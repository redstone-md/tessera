// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! An answer to `IDesktopWallpaper` for when Explorer is not there to give
//! one: in shell mode Tessera registers a class object for
//! `CLSID_DesktopWallpaper` with COM, so that a program that asks COM for
//! the wallpaper object of the desktop gets one of Tessera.
//!
//! What a program sets through it ([`WallpaperRequest`]) goes to the
//! wallpaper of Tessera, which writes it to the configuration; what it
//! reads ([`WallpaperAnswers`]) is what the wallpaper of Tessera last said
//! it shows. The displays are named as Explorer names them, by the path
//! of the device interface of each monitor.
//!
//! Whether Settings > Personalization > Background asks COM for a running
//! server, and takes this one, has not been tried: COM gives a class
//! object registered by a running process to programs of the same user
//! that ask for a local server, and Settings may instead create the object
//! in its own process, which this does not answer. Where it is not taken,
//! programs still set the wallpaper with `SystemParametersInfoW`, as
//! [`super::SystemWallpaper`] describes.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use anyhow::{Context as _, Result, anyhow};
use windows::Win32::Foundation::{
    CLASS_E_NOAGGREGATION, COLORREF, E_INVALIDARG, E_POINTER, LPARAM, RECT,
};
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
    MONITORINFOEXW,
};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoInitializeEx, CoRegisterClassObject,
    CoRevokeClassObject, CoTaskMemAlloc, CoUninitialize, IClassFactory, IClassFactory_Impl,
    REGCLS_MULTIPLEUSE,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    DESKTOP_SLIDESHOW_DIRECTION, DESKTOP_SLIDESHOW_OPTIONS, DESKTOP_SLIDESHOW_STATE,
    DESKTOP_WALLPAPER_POSITION, DSD_BACKWARD, DSO_SHUFFLEIMAGES, DSS_ENABLED, DSS_SLIDESHOW,
    DesktopWallpaper, IDesktopWallpaper, IDesktopWallpaper_Impl, IShellItemArray,
    SHCreateShellItemArrayFromIDLists,
};
use windows::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;
use windows::core::{BOOL, GUID, IUnknown, Interface, PCWSTR, PWSTR, Ref, implement};

use super::shell_item::{IdList, path_of_item};
use super::system_wallpaper::{colorref, rgb};
use super::{ScreenRect, WallpaperPosition};

/// What a program asked of the wallpaper.
#[derive(Clone, Debug, PartialEq)]
pub enum WallpaperRequest {
    /// The picture of the display at a frame, or of every display where
    /// none is named; `None` for no picture, which leaves the color.
    Picture {
        display: Option<ScreenRect>,
        file: Option<PathBuf>,
    },
    /// How pictures are fitted.
    Position(WallpaperPosition),
    /// The color around a picture, red, green and blue.
    Color([u8; 3]),
    /// Pictures, or folders of them, that take turns.
    Slideshow(Vec<PathBuf>),
    /// Whether the pictures that take turns are shuffled, and how many
    /// milliseconds one stays.
    SlideshowOptions { shuffle: bool, interval_ms: u32 },
    /// The next picture, or with `back` the one before.
    Advance { back: bool },
    /// Pictures are shown, or with `false` the color alone.
    Enable(bool),
}

/// What a program that asks is told of the wallpaper.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WallpaperAnswers {
    /// Each display with its picture, where it shows one.
    pub displays: Vec<(ScreenRect, Option<PathBuf>)>,
    pub position: WallpaperPosition,
    pub color: [u8; 3],
    /// The pictures or folders that take turns.
    pub slideshow: Vec<PathBuf>,
    pub shuffle: bool,
    pub interval_ms: u32,
    /// Whether pictures are shown, and whether they take turns.
    pub enabled: bool,
    pub turning: bool,
}

/// What the class object, the objects it makes and the wallpaper share.
struct Link {
    answers: Arc<Mutex<WallpaperAnswers>>,
    requests: async_channel::Sender<WallpaperRequest>,
}

impl Link {
    fn answers(&self) -> std::sync::MutexGuard<'_, WallpaperAnswers> {
        self.answers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn ask(&self, request: WallpaperRequest) {
        self.requests.try_send(request).ok();
    }
}

/// The class object of Tessera for `CLSID_DesktopWallpaper`, registered
/// with COM for as long as it lives.
pub struct WallpaperServer {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl WallpaperServer {
    /// Registers the class object on a thread of its own, in the
    /// multithreaded apartment, where COM calls it from threads of its
    /// own: `answers` is read and `requests` given what programs set.
    /// Fails where COM does not take the registration.
    pub fn start(
        answers: Arc<Mutex<WallpaperAnswers>>,
        requests: async_channel::Sender<WallpaperRequest>,
    ) -> Result<Self> {
        let (stop, stopped) = mpsc::channel::<()>();
        let (began, beginning) = mpsc::channel::<Result<()>>();
        let link = Arc::new(Link { answers, requests });
        let thread = std::thread::Builder::new().name("wallpaper-server".into());
        let thread = thread.spawn(move || {
            // SAFETY: the thread enters the apartment once and leaves it
            // after the class object was revoked.
            let entered = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            let factory: IClassFactory = Factory(link).into();
            // SAFETY: the class object is alive while it is registered:
            // COM holds a reference until it is revoked.
            let cookie = unsafe {
                CoRegisterClassObject(
                    &DesktopWallpaper,
                    &factory,
                    CLSCTX_LOCAL_SERVER,
                    REGCLS_MULTIPLEUSE,
                )
            };
            match cookie {
                Ok(cookie) => {
                    began.send(Ok(())).ok();
                    // Until told to stop, or until the server is let go.
                    let _ = stopped.recv();
                    // SAFETY: the cookie is the one the registration gave.
                    let _ = unsafe { CoRevokeClassObject(cookie) };
                }
                Err(error) => {
                    let error = anyhow!(error).context("COM did not take the wallpaper server");
                    began.send(Err(error)).ok();
                }
            }
            drop(factory);
            if entered {
                // SAFETY: as above.
                unsafe { CoUninitialize() };
            }
        });
        let thread = thread.context("no thread could be started for the wallpaper server")?;
        beginning
            .recv()
            .context("the wallpaper server ended before it began")??;
        Ok(Self {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

impl Drop for WallpaperServer {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The path of the device interface of each display, as Explorer names
/// displays to `IDesktopWallpaper`, with its bounds.
pub fn monitor_paths() -> Vec<(String, ScreenRect)> {
    unsafe extern "system" fn add(monitor: HMONITOR, _: HDC, _: *mut RECT, found: LPARAM) -> BOOL {
        // SAFETY: `found` is the list `monitor_paths` passed, which
        // outlives the enumeration.
        let found = unsafe { &mut *(found.0 as *mut Vec<HMONITOR>) };
        found.push(monitor);
        true.into()
    }
    let mut monitors: Vec<HMONITOR> = Vec::new();
    // SAFETY: the enumeration is synchronous and `monitors` outlives it.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&raw mut monitors as isize));
    }
    let mut paths = Vec::new();
    for monitor in monitors {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: `info` is of the size it says, and the name of the
        // device it gives is NUL-terminated within it.
        let path = unsafe {
            if !GetMonitorInfoW(monitor, (&raw mut info).cast()).as_bool() {
                continue;
            }
            let mut device = DISPLAY_DEVICEW {
                cb: size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            let named = EnumDisplayDevicesW(
                PCWSTR(info.szDevice.as_ptr()),
                0,
                &raw mut device,
                EDD_GET_DEVICE_INTERFACE_NAME,
            );
            if !named.as_bool() {
                continue;
            }
            text_of(&device.DeviceID)
        };
        if !path.is_empty() {
            paths.push((path, info.monitorInfo.rcMonitor.into()));
        }
    }
    paths
}

/// The text of a buffer that ends with a NUL.
fn text_of(units: &[u16]) -> String {
    let length = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..length])
}

/// `text` in memory of COM, for a caller to free.
fn co_text(text: &str) -> windows::core::Result<PWSTR> {
    let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
    // SAFETY: the memory is of the size of the text and its NUL, which
    // are copied into it.
    unsafe {
        let memory = CoTaskMemAlloc(units.len() * 2).cast::<u16>();
        if memory.is_null() {
            return Err(windows::Win32::Foundation::E_OUTOFMEMORY.into());
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), memory, units.len());
        Ok(PWSTR(memory))
    }
}

/// The text `text` points to; empty for none.
fn text_at(text: &PCWSTR) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: a text COM passes is NUL-terminated.
    unsafe { text.to_string() }.unwrap_or_default()
}

/// The display named `id`, by its bounds; `None` for no name, which
/// stands for every display, and an error for a name no display has.
fn display_named(id: &PCWSTR) -> windows::core::Result<Option<ScreenRect>> {
    let id = text_at(id);
    if id.is_empty() {
        return Ok(None);
    }
    { monitor_paths().into_iter() }
        .find(|(path, _)| path.eq_ignore_ascii_case(&id))
        .map(|(_, frame)| Some(frame))
        .ok_or_else(|| E_INVALIDARG.into())
}

#[implement(IClassFactory)]
struct Factory(Arc<Link>);

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        id: *const GUID,
        object: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        if object.is_null() || id.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: `object` is where the caller wants the object.
        unsafe { *object = std::ptr::null_mut() };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let wallpaper: IDesktopWallpaper = Served(self.0.clone()).into();
        // SAFETY: `id` and `object` are those the caller gave.
        unsafe { wallpaper.query(id, object) }.ok()
    }

    fn LockServer(&self, _: BOOL) -> windows::core::Result<()> {
        // The server lives as long as Tessera does.
        Ok(())
    }
}

#[implement(IDesktopWallpaper)]
struct Served(Arc<Link>);

impl IDesktopWallpaper_Impl for Served_Impl {
    fn SetWallpaper(&self, monitor: &PCWSTR, wallpaper: &PCWSTR) -> windows::core::Result<()> {
        let display = display_named(monitor)?;
        let file = Some(text_at(wallpaper))
            .filter(|file| !file.is_empty())
            .map(PathBuf::from);
        {
            let mut answers = self.0.answers();
            for (frame, picture) in &mut answers.displays {
                if display.is_none_or(|display| display == *frame) {
                    picture.clone_from(&file);
                }
            }
        }
        self.0.ask(WallpaperRequest::Picture { display, file });
        Ok(())
    }

    fn GetWallpaper(&self, monitor: &PCWSTR) -> windows::core::Result<PWSTR> {
        let display = display_named(monitor)?;
        let answers = self.0.answers();
        let picture = match display {
            Some(display) => { answers.displays.iter() }
                .find(|(frame, _)| *frame == display)
                .and_then(|(_, picture)| picture.clone()),
            // For every display, the picture they all have, or none.
            None => {
                let mut pictures = answers.displays.iter().map(|(_, picture)| picture);
                let first = pictures.next().cloned().flatten();
                first.filter(|first| pictures.all(|other| other.as_ref() == Some(first)))
            }
        };
        let text = picture.map(|picture| picture.display().to_string());
        co_text(&text.unwrap_or_default())
    }

    fn GetMonitorDevicePathAt(&self, index: u32) -> windows::core::Result<PWSTR> {
        let paths = monitor_paths();
        let (path, _) = paths.get(index as usize).ok_or(E_INVALIDARG)?;
        co_text(path)
    }

    fn GetMonitorDevicePathCount(&self) -> windows::core::Result<u32> {
        Ok(monitor_paths().len() as u32)
    }

    fn GetMonitorRECT(&self, monitor: &PCWSTR) -> windows::core::Result<RECT> {
        let frame = display_named(monitor)?.ok_or(E_INVALIDARG)?;
        Ok(RECT {
            left: frame.left,
            top: frame.top,
            right: frame.right,
            bottom: frame.bottom,
        })
    }

    fn SetBackgroundColor(&self, color: COLORREF) -> windows::core::Result<()> {
        let color = rgb(color);
        self.0.answers().color = color;
        self.0.ask(WallpaperRequest::Color(color));
        Ok(())
    }

    fn GetBackgroundColor(&self) -> windows::core::Result<COLORREF> {
        Ok(colorref(self.0.answers().color))
    }

    fn SetPosition(&self, position: DESKTOP_WALLPAPER_POSITION) -> windows::core::Result<()> {
        let position = WallpaperPosition::of_explorer(position);
        self.0.answers().position = position;
        self.0.ask(WallpaperRequest::Position(position));
        Ok(())
    }

    fn GetPosition(&self) -> windows::core::Result<DESKTOP_WALLPAPER_POSITION> {
        Ok(self.0.answers().position.to_explorer())
    }

    fn SetSlideshow(&self, items: Ref<'_, IShellItemArray>) -> windows::core::Result<()> {
        let items = items.ok()?;
        let mut paths = Vec::new();
        // SAFETY: the array is alive for the call, and each item for as
        // long as it is read.
        unsafe {
            for index in 0..items.GetCount()? {
                if let Some(path) = path_of_item(&items.GetItemAt(index)?) {
                    paths.push(path);
                }
            }
        }
        self.0.answers().slideshow.clone_from(&paths);
        self.0.ask(WallpaperRequest::Slideshow(paths));
        Ok(())
    }

    fn GetSlideshow(&self) -> windows::core::Result<IShellItemArray> {
        let paths = self.0.answers().slideshow.clone();
        let lists: Vec<IdList> = { paths.iter() }
            .filter_map(|path| IdList::of_path(path).ok())
            .collect();
        let pointers: Vec<*const ITEMIDLIST> = lists.iter().map(IdList::as_ptr).collect();
        // SAFETY: the lists outlive the call, which copies them.
        unsafe { SHCreateShellItemArrayFromIDLists(&pointers) }
    }

    fn SetSlideshowOptions(
        &self,
        options: DESKTOP_SLIDESHOW_OPTIONS,
        tick: u32,
    ) -> windows::core::Result<()> {
        let shuffle = options.0 & DSO_SHUFFLEIMAGES.0 != 0;
        {
            let mut answers = self.0.answers();
            answers.shuffle = shuffle;
            answers.interval_ms = tick;
        }
        self.0.ask(WallpaperRequest::SlideshowOptions {
            shuffle,
            interval_ms: tick,
        });
        Ok(())
    }

    fn GetSlideshowOptions(
        &self,
        options: *mut DESKTOP_SLIDESHOW_OPTIONS,
        tick: *mut u32,
    ) -> windows::core::Result<()> {
        if options.is_null() || tick.is_null() {
            return Err(E_POINTER.into());
        }
        let answers = self.0.answers();
        let shuffle = if answers.shuffle {
            DSO_SHUFFLEIMAGES
        } else {
            DESKTOP_SLIDESHOW_OPTIONS(0)
        };
        // SAFETY: both point to where the caller wants the answers.
        unsafe {
            *options = shuffle;
            *tick = answers.interval_ms;
        }
        Ok(())
    }

    fn AdvanceSlideshow(
        &self,
        _monitor: &PCWSTR,
        direction: DESKTOP_SLIDESHOW_DIRECTION,
    ) -> windows::core::Result<()> {
        // Every display goes on together, as the action `wallpaper` has
        // them.
        self.0.ask(WallpaperRequest::Advance {
            back: direction == DSD_BACKWARD,
        });
        Ok(())
    }

    fn GetStatus(&self) -> windows::core::Result<DESKTOP_SLIDESHOW_STATE> {
        let answers = self.0.answers();
        let mut state = 0;
        if answers.enabled {
            state |= DSS_ENABLED.0;
        }
        if answers.turning {
            state |= DSS_SLIDESHOW.0;
        }
        Ok(DESKTOP_SLIDESHOW_STATE(state))
    }

    fn Enable(&self, enable: BOOL) -> windows::core::Result<()> {
        let enable = enable.as_bool();
        self.0.answers().enabled = enable;
        self.0.ask(WallpaperRequest::Enable(enable));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_end_at_their_nul() {
        let units: Vec<u16> = "\\\\?\\DISPLAY#A\0junk".encode_utf16().collect();
        assert_eq!(text_of(&units), "\\\\?\\DISPLAY#A");
        assert_eq!(text_of(&[]), "");
        assert_eq!(text_at(&PCWSTR::null()), "");
    }
}
