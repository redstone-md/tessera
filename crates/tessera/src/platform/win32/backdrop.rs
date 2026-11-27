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
//! what a window of the user interface takes for drawing. The pixels lie
//! in a temporary file that Windows maps into memory, where it can be
//! had: they are needed only when the window is asked to paint, which is
//! seldom, and so they need not stay in the memory of the shell.

use std::cell::Cell;
use std::ffi::c_void;
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{
    COLORREF, CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLACK_BRUSH, BLENDFUNCTION, BeginPaint, BitBlt,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint,
    EnumDisplayMonitors, FillRect, GdiAlphaBlend, GetDC, GetStockObject, HBITMAP, HBRUSH, HDC,
    HGDIOBJ, HMONITOR, MONITOR_DEFAULTTONEAREST, MapWindowPoints, MonitorFromRect, PAINTSTRUCT,
    ReleaseDC, SRCCOPY, SelectObject, SetViewportOrgEx,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{CreateFileMappingW, PAGE_READWRITE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, FindWindowW, GWLP_USERDATA,
    GetWindowLongPtrW, GetWindowRect, LWA_ALPHA, RegisterClassW, SMTO_NORMAL, SPI_GETDESKWALLPAPER,
    SW_SHOWNA, SWP_FRAMECHANGED, SWP_NOACTIVATE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SendMessageTimeoutW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    SystemParametersInfoW, WM_ERASEBKGND, WM_NCDESTROY, WM_PAINT, WNDCLASSW, WS_CHILD,
    WS_EX_LAYERED, WS_EX_NOACTIVATE,
};
use windows::core::{BOOL, PCWSTR, w};

use super::{DisplayKey, ScreenRect};

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

/// The display at `frame`, one of those [`display_frames`] gives, or the
/// one nearest to it.
pub fn display_at(frame: ScreenRect) -> DisplayKey {
    let rect = RECT {
        left: frame.left,
        top: frame.top,
        right: frame.right,
        bottom: frame.bottom,
    };
    // SAFETY: the call only reads the arrangement of the displays, and
    // `rect` outlives it.
    let display = unsafe { MonitorFromRect(&raw const rect, MONITOR_DEFAULTTONEAREST) };
    DisplayKey(display.0 as isize)
}

/// How many times a wallpaper of Tessera began to come, came to be shown
/// or went.
static BACKDROPS_CHANGED: AtomicU64 = AtomicU64::new(0);

/// A number that is another one when the wallpaper has changed: that of
/// Windows, by the name and the time of its file, or one of Tessera. A
/// wallpaper that moves is not seen to change.
pub fn desktop_look() -> u64 {
    let mut name = [0_u16; 520];
    // SAFETY: the buffer is of the length given.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            name.len() as u32,
            Some(name.as_mut_ptr().cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    let length = name.iter().position(|&unit| unit == 0).unwrap_or(0);
    let name = String::from_utf16_lossy(&name[..length]);
    let changed = std::fs::metadata(&name)
        .and_then(|file| file.modified())
        .ok();
    let mut look = DefaultHasher::new();
    (read.is_ok(), name, changed).hash(&mut look);
    BACKDROPS_CHANGED.load(Ordering::Relaxed).hash(&mut look);
    look.finish()
}

/// Has a window drawn with all that the compositor shows of it.
const WHOLE_CONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);

/// What the desktop window shows within `frame`, which is in pixels of
/// the desktop: the wallpaper, what is laid over it there and the icons,
/// and no window that is not a part of the desktop. The width of the
/// picture and its rows from the top, a pixel being blue, green, red and
/// a byte that says nothing. `None` where the desktop cannot be read.
///
/// The desktop window draws itself into a bitmap of the size of `frame`
/// for this, which takes a moment: not for the thread of the interface.
/// The process that asks is charged with memory for the whole desktop
/// window, tens of megabytes on several displays, and keeps being charged
/// for seconds after. Another process of this program is therefore
/// started to read, which gives the pixels and ends, and what it was
/// charged with goes with it. The desktop is read by one caller at a
/// time, and asked for seldom: see [`desktop_look`].
pub fn desktop_pixels(frame: ScreenRect) -> Option<(usize, Vec<u8>)> {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    desktop_pixels_by_reader(frame).or_else(|| desktop_pixels_here(frame))
}

/// The argument that has the program read the desktop and end. The
/// sides of the frame follow it: left, top, right and bottom.
const READER_FLAG: &str = "--read-desktop";

/// Has a started process create no console window.
const NO_WINDOW: u32 = 0x0800_0000;

/// Reads the desktop within `frame` in another process of this program.
/// `None` where that process cannot be started or gives something else
/// than the pixels of `frame`.
fn desktop_pixels_by_reader(frame: ScreenRect) -> Option<(usize, Vec<u8>)> {
    let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
    if width <= 0 || height <= 0 {
        return None;
    }
    let sides = [frame.left, frame.top, frame.right, frame.bottom];
    let read = Command::new(std::env::current_exe().ok()?)
        .arg(READER_FLAG)
        .args(sides.map(|side| side.to_string()))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(NO_WINDOW)
        .output()
        .ok()?;
    let length = width as usize * height as usize * 4;
    (read.status.success() && read.stdout.len() == length).then_some((width as usize, read.stdout))
}

/// When the program was started to read the desktop, does so, writes the
/// pixels [`desktop_pixels`] gives to its output and returns `true`:
/// the program is to end then.
pub fn read_desktop_if_requested(mut arguments: impl Iterator<Item = String>) -> bool {
    if arguments.next().as_deref() != Some(READER_FLAG) {
        return false;
    }
    let mut side = || arguments.next().and_then(|value| value.parse::<i32>().ok());
    let read = (|| {
        let frame = ScreenRect {
            left: side()?,
            top: side()?,
            right: side()?,
            bottom: side()?,
        };
        let (_, pixels) = desktop_pixels_here(frame)?;
        let mut output = std::io::stdout().lock();
        output.write_all(&pixels).ok()?;
        output.flush().ok()
    })();
    if read.is_none() {
        std::process::exit(1);
    }
    true
}

/// Reads the desktop within `frame` in this process.
fn desktop_pixels_here(frame: ScreenRect) -> Option<(usize, Vec<u8>)> {
    let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
    if width <= 0 || height <= 0 {
        return None;
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // A negative height has the rows start at the top.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: the bitmap is made for `info` and read as the bytes `info`
    // gives it while `held` keeps it; `held` releases the handles.
    unsafe {
        let desktop = FindWindowW(w!("Progman"), None).ok()?;
        let mut whole = RECT::default();
        GetWindowRect(desktop, &raw mut whole).ok()?;
        let mut bits: *mut c_void = ptr::null_mut();
        let bitmap = CreateDIBSection(
            None,
            &raw const info,
            DIB_RGB_COLORS,
            &raw mut bits,
            None,
            0,
        )
        .ok()?;
        let context = CreateCompatibleDC(None);
        let before = SelectObject(context, bitmap.into());
        let held = Held {
            context,
            bitmap,
            before,
            _backing: None,
        };
        // The window draws itself from its own corner: the corner is put
        // where it is seen from `frame`.
        let moved = SetViewportOrgEx(
            held.context,
            whole.left - frame.left,
            whole.top - frame.top,
            None,
        );
        if bits.is_null()
            || !moved.as_bool()
            || !PrintWindow(desktop, held.context, WHOLE_CONTENT).as_bool()
        {
            return None;
        }
        let length = width as usize * height as usize * 4;
        let pixels = std::slice::from_raw_parts(bits.cast::<u8>(), length).to_vec();
        Some((width as usize, pixels))
    }
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
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The bytes of the picture, to change.
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
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
    /// The file the pixels lie in, where there is one. Declared last: it
    /// goes after the bitmap that uses it.
    _backing: Option<Backing>,
}

/// A temporary file of the size of a picture, mapped for a bitmap to lie
/// in. The file is gone when it is closed.
struct Backing {
    section: HANDLE,
    _file: File,
}

impl Backing {
    /// A file of `length` bytes. `None` where it cannot be made: the
    /// bitmap then lies in the memory of the shell.
    fn new(length: usize) -> Option<Self> {
        /// Windows deletes the file when its last handle is closed, and
        /// is in no hurry to write it out.
        const DELETE_ON_CLOSE: u32 = 0x0400_0000;
        const TEMPORARY: u32 = 0x100;
        static MADE: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "tessera-wallpaper-{}-{}.bin",
            std::process::id(),
            MADE.fetch_add(1, Ordering::Relaxed)
        );
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .share_mode(0)
            .custom_flags(DELETE_ON_CLOSE)
            .attributes(TEMPORARY)
            .open(std::env::temp_dir().join(name))
            .ok()?;
        file.set_len(length as u64).ok()?;
        // SAFETY: the handle is that of `file`, which is kept for as long
        // as the mapping.
        let section = unsafe {
            CreateFileMappingW(
                HANDLE(file.as_raw_handle()),
                None,
                PAGE_READWRITE,
                0,
                0,
                None,
            )
        }
        .ok()?;
        Some(Self {
            section,
            _file: file,
        })
    }
}

impl Drop for Backing {
    fn drop(&mut self) {
        // SAFETY: the handle was made by `new` and is closed once.
        unsafe {
            let _ = CloseHandle(self.section);
        }
    }
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
        Ok(Self {
            hwnd: open_behind_icons(frame, w!("TesseraBackdrop"), respond)?,
            width: frame.right - frame.left,
            height: frame.bottom - frame.top,
            revealed: Cell::new(false),
        })
    }

    /// Whether there is a desktop window to open one under: without one,
    /// as in shell mode before Tessera's own is there, none can be opened
    /// and nothing is to be made for it.
    pub fn possible() -> bool {
        // SAFETY: the call only looks a window up by its class.
        unsafe { FindWindowW(w!("Progman"), None) }.is_ok()
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
            let mut backing = Backing::new(picture.pixels.len());
            let mut made = backing.as_ref().and_then(|backing| {
                CreateDIBSection(
                    None,
                    &raw const info,
                    DIB_RGB_COLORS,
                    &raw mut bits,
                    Some(backing.section),
                    0,
                )
                .ok()
                .filter(|_| !bits.is_null())
            });
            if made.is_none() {
                backing = None;
                bits = ptr::null_mut();
                let bitmap = CreateDIBSection(
                    None,
                    &raw const info,
                    DIB_RGB_COLORS,
                    &raw mut bits,
                    None,
                    0,
                )
                .context("there is no memory for the wallpaper")?;
                made = Some(bitmap);
            }
            let Some(bitmap) = made else {
                bail!("there is no memory for the wallpaper");
            };
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
                _backing: backing,
            });
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, Box::into_raw(held) as isize);
        }
        // The picture begins to come: the desktop is about to change.
        BACKDROPS_CHANGED.fetch_add(1, Ordering::Relaxed);
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
        if to >= 1. {
            BACKDROPS_CHANGED.fetch_add(1, Ordering::Relaxed);
        }
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

impl DesktopBackdrop {
    /// Moves what is on the screen away from the picture, towards what
    /// lies under the window: `to` goes from 0, the picture as it is, to 1,
    /// where nothing of it is seen. The window is to be dropped then.
    pub fn recede(&self, to: f32) {
        if !self.revealed.get() {
            return;
        }
        let alpha = ((1. - to.clamp(0., 1.)) * 255.).round() as u8;
        // SAFETY: the window belongs to this thread; the call changes how
        // much of it the compositor shows.
        unsafe {
            let _ = SetLayeredWindowAttributes(self.hwnd, COLORREF(0), alpha, LWA_ALPHA);
        }
        if to >= 1. {
            BACKDROPS_CHANGED.fetch_add(1, Ordering::Relaxed);
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
        BACKDROPS_CHANGED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Makes a window of the class `name`, which `respond` is the procedure
/// of, behind the icons of the desktop over the display at `frame`: a
/// layer of the desktop of which nothing is seen yet. Fails where the
/// desktop is not laid out as Windows 11 24H2 does.
pub(super) fn open_behind_icons(
    frame: ScreenRect,
    name: PCWSTR,
    respond: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
) -> Result<HWND> {
    let width = frame.right - frame.left;
    let height = frame.bottom - frame.top;
    // SAFETY: the calls look windows up by class, send one message
    // with a time limit, and make and place a window of this thread;
    // `corner` outlives the call that maps it.
    unsafe {
        let desktop = FindWindowW(w!("Progman"), None).context("there is no desktop window")?;
        let parts = |desktop: HWND| {
            let icons = FindWindowExW(Some(desktop), None, w!("SHELLDLL_DefView"), None).ok()?;
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
            lpszClassName: name,
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
        let placed = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA)
            .context("the window could not be made a layer of the desktop")
            .and_then(|()| {
                // After the icons in the order of the children is
                // below them, and the wallpaper stays the last.
                SetWindowPos(
                    hwnd,
                    Some(icons),
                    corner[0].x,
                    corner[0].y,
                    width,
                    height,
                    SWP_NOACTIVATE | SWP_FRAMECHANGED,
                )
                .context("the window could not be put behind the icons")
            });
        if let Err(error) = placed {
            let _ = DestroyWindow(hwnd);
            return Err(error);
        }
        Ok(hwnd)
    }
}

/// Has [`desktop_look`] give another number: a wallpaper of Tessera came
/// or went.
pub(super) fn backdrops_changed() {
    BACKDROPS_CHANGED.fetch_add(1, Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// Reads the desktop of the machine it runs on.
    #[test]
    #[ignore = "needs a desktop"]
    fn the_desktop_is_read_under_a_bar() {
        let frame = display_frames()[0];
        let bar = ScreenRect {
            bottom: frame.top + 36,
            ..frame
        };
        let began = Instant::now();
        let (width, pixels) = desktop_pixels_here(bar).expect("the desktop is read");
        println!("read in {:?}", began.elapsed());
        assert_eq!(width as i32, bar.right - bar.left);
        assert_eq!(pixels.len(), width * 36 * 4);
        assert!(pixels.iter().any(|&byte| byte != 0));
    }
}
