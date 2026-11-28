// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pictures of the screen, of a display, of a window or of any rectangle
//! of the screen, for screenshots.
//!
//! Windows.Graphics.Capture takes each: a `GraphicsCaptureItem` made for
//! a display or a window through `IGraphicsCaptureItemInterop`, one frame
//! of a free-threaded `Direct3D11CaptureFramePool`, copied to a texture the
//! processor can read. A rectangle is put together of the displays it
//! lies on. Where that capture is not there (before Windows 10 1903, in
//! some remote sessions) or fails, the screen is copied with GDI instead,
//! and a window is printed with `PrintWindow(PW_RENDERFULLCONTENT)`, which
//! also draws what the window draws with the GPU. A window is taken
//! without what lies around or over it.
//!
//! Tessera knows the displays at their own density (per-monitor DPI
//! awareness, version 2), so every picture has the pixels of the displays
//! as they are. The pointer is in the picture on request. Windows draws a
//! yellow border around what is being captured: it is turned off with
//! `IsBorderRequired`, which Windows 11 grants to programs that are not
//! packaged; Windows 10 has no such switch and shows the border for the
//! moment a capture takes.
//!
//! Every call waits for Windows and is to be made away from the thread
//! that draws.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use image::RgbaImage;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HBITMAP, HDC,
    HGDIOBJ, HMONITOR, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, EnumWindows, GA_ROOT, GWL_EXSTYLE,
    GetAncestor, GetCursorInfo, GetIconInfo, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, ICONINFO, IsIconic, IsWindowVisible, PW_RENDERFULLCONTENT,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WS_EX_TOOLWINDOW,
    WindowFromPoint,
};
use windows::core::{BOOL, Interface, factory};

use super::{AppWindowId, Display, DisplayKey, ScreenRect};

/// How long the first frame of a capture is waited for.
const FRAME_PATIENCE: Duration = Duration::from_secs(3);

/// What to take a picture of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureArea {
    /// A rectangle of the screen, in physical pixels; it may lie on
    /// several displays.
    Screen(ScreenRect),
    /// One display, whole.
    Display(DisplayKey),
    /// One window, without what lies around or over it.
    Window(AppWindowId),
}

/// A picture of the screen and where it was.
#[derive(Clone, Debug)]
pub struct Capture {
    pub pixels: RgbaImage,
    /// Where its top left pixel was on the screen, and how large it is.
    pub frame: ScreenRect,
}

/// Takes a picture of `area`, with the pointer in it if `cursor`. It waits
/// for Windows and is to be called away from the thread that draws.
pub fn capture(area: CaptureArea, cursor: bool) -> Result<Capture> {
    // Windows.Graphics.Capture lives in the apartment of many threads; a
    // thread that is in another one already keeps it, which works too.
    // SAFETY: the call only sets up COM for this thread.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    match area {
        CaptureArea::Display(display) => {
            let frame = Display::all()
                .into_iter()
                .find(|known| known.key == display)
                .map(|known| known.frame)
                .context("the display is gone")?;
            capture_rect(frame, cursor)
        }
        CaptureArea::Screen(rect) => capture_rect(rect, cursor),
        CaptureArea::Window(window) => capture_window(window, cursor),
    }
}

/// The rectangle every display together covers.
pub fn virtual_screen() -> ScreenRect {
    // SAFETY: the calls only read the arrangement of the displays.
    unsafe {
        let (left, top) = (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        );
        ScreenRect {
            left,
            top,
            right: left + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: top + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// What is seen of each window of the screen that is shown, from the one
/// in front to the one at the back: those of other programs only, not
/// the windows of Tessera, which a picture of the screen lies under.
pub fn window_frames() -> Vec<(AppWindowId, ScreenRect)> {
    unsafe extern "system" fn add(window: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` is the pointer `window_frames` passed, to a list
        // that outlives the enumeration.
        let found = unsafe { &mut *(found.0 as *mut Vec<HWND>) };
        found.push(window);
        true.into()
    }
    let mut found: Vec<HWND> = Vec::new();
    // SAFETY: the enumeration is synchronous and `found` outlives it.
    unsafe {
        let _ = EnumWindows(Some(add), LPARAM(&raw mut found as isize));
    }
    let own = std::process::id();
    { found.into_iter() }
        .filter(|&hwnd| seen(hwnd) && process_of(hwnd) != own)
        .filter_map(|hwnd| {
            let id = AppWindowId::from_raw(hwnd.0 as isize);
            let frame = id.frame()?;
            let has_area = frame.right > frame.left && frame.bottom > frame.top;
            has_area.then_some((id, frame))
        })
        .collect()
}

/// The window of another program the pointer is on, and what is seen of
/// it; the windows of Tessera are looked through.
pub fn window_under_pointer() -> Option<AppWindowId> {
    let (x, y) = super::pointer_at()?;
    // SAFETY: the calls only read which window is at a point and its
    // top-level window.
    let root = unsafe {
        let window = WindowFromPoint(POINT { x, y });
        GetAncestor(window, GA_ROOT)
    };
    if !root.is_invalid() && process_of(root) != std::process::id() && seen(root) {
        return Some(AppWindowId::from_raw(root.0 as isize));
    }
    let inside = |rect: &ScreenRect| {
        (rect.left..rect.right).contains(&x) && (rect.top..rect.bottom).contains(&y)
    };
    window_frames()
        .into_iter()
        .find(|(_, frame)| inside(frame))
        .map(|(id, _)| id)
}

/// The window the user works in: the one in front, or where a window of
/// Tessera is in front, the window of another program nearest the front.
pub fn front_window() -> Option<AppWindowId> {
    let front = AppWindowId::foreground().filter(|window| window.process() != std::process::id());
    front.or_else(|| {
        window_frames()
            .into_iter()
            .map(|(id, _)| id)
            // A tool window, as a menu or a tip, is no window to work in.
            .find(|id| !is_tool_window(id.hwnd()))
    })
}

/// Whether a window is shown: visible, not minimized and not hidden by
/// the compositor, as a window of another virtual desktop is.
fn seen(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: the calls only read the state of a window that may be gone,
    // which makes them fail; `cloaked` is as large as the call is told.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return false;
        }
        let read = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        );
        read.is_err() || cloaked == 0
    }
}

fn is_tool_window(hwnd: HWND) -> bool {
    // SAFETY: the call only reads the styles of a window.
    let extended = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    extended & WS_EX_TOOLWINDOW.0 as isize != 0
}

fn process_of(hwnd: HWND) -> u32 {
    let mut process = 0;
    // SAFETY: `process` outlives the call, which only reads whose the
    // window is.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut process)) };
    process
}

/// A picture of `rect` of the screen: each display it lies on taken with
/// Windows.Graphics.Capture and put where it is, or else all of it copied
/// with GDI.
fn capture_rect(rect: ScreenRect, cursor: bool) -> Result<Capture> {
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    if width <= 0 || height <= 0 {
        bail!("the rectangle to take has no area");
    }
    match compose(rect, cursor) {
        Ok(pixels) => Ok(Capture {
            pixels,
            frame: rect,
        }),
        Err(error) => {
            report!("the screen is copied with GDI: {error:#}");
            Ok(Capture {
                pixels: gdi_screen(rect, cursor)?,
                frame: rect,
            })
        }
    }
}

/// `rect` put together of the pictures of the displays it lies on; what
/// no display shows stays transparent.
fn compose(rect: ScreenRect, cursor: bool) -> Result<RgbaImage> {
    let (width, height) = (
        (rect.right - rect.left) as u32,
        (rect.bottom - rect.top) as u32,
    );
    let mut picture = RgbaImage::new(width, height);
    let mut any = false;
    for display in Display::all() {
        let frame = display.frame;
        let meets = frame.left < rect.right
            && rect.left < frame.right
            && frame.top < rect.bottom
            && rect.top < frame.bottom;
        if !meets {
            continue;
        }
        let item = item_for_monitor(HMONITOR(display.key.0 as *mut c_void))?;
        let mut taken = take_frame(&item, cursor)?;
        // A display shows everything it covers.
        for pixel in taken.pixels_mut() {
            pixel[3] = u8::MAX;
        }
        let (x, y) = (
            i64::from(frame.left - rect.left),
            i64::from(frame.top - rect.top),
        );
        image::imageops::replace(&mut picture, &taken, x, y);
        any = true;
    }
    match any {
        true => Ok(picture),
        false => Err(anyhow!("no display shows the rectangle")),
    }
}

/// A picture of `window` with Windows.Graphics.Capture, or else printed by
/// the window itself, cut to what is seen of it.
fn capture_window(window: AppWindowId, cursor: bool) -> Result<Capture> {
    if !window.exists() {
        bail!("the window is gone");
    }
    let hwnd = window.hwnd();
    // SAFETY: the call only reads the state of a window.
    if unsafe { IsIconic(hwnd) }.as_bool() {
        bail!("the window is minimized");
    }
    let seen = window
        .frame()
        .context("where the window is could not be read")?;
    let taken = item_for_window(hwnd).and_then(|item| take_frame(&item, cursor));
    match taken {
        Ok(pixels) => {
            let frame = ScreenRect {
                left: seen.left,
                top: seen.top,
                right: seen.left + pixels.width() as i32,
                bottom: seen.top + pixels.height() as i32,
            };
            Ok(Capture { pixels, frame })
        }
        Err(error) => {
            report!("the window is printed instead of captured: {error:#}");
            let pixels = print_window(hwnd, seen, cursor)?;
            Ok(Capture {
                pixels,
                frame: seen,
            })
        }
    }
}

fn item_for_monitor(monitor: HMONITOR) -> Result<GraphicsCaptureItem> {
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
        .context("Windows.Graphics.Capture is not there")?;
    // SAFETY: the display is one Windows enumerated a moment ago; one that
    // is gone makes the call fail.
    unsafe { interop.CreateForMonitor(monitor) }.context("the display cannot be captured")
}

fn item_for_window(window: HWND) -> Result<GraphicsCaptureItem> {
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
        .context("Windows.Graphics.Capture is not there")?;
    // SAFETY: a window that is gone makes the call fail.
    unsafe { interop.CreateForWindow(window) }.context("the window cannot be captured")
}

/// A device of Direct3D 11 and its context, and the same device as
/// Windows.Graphics.Capture takes it.
fn device() -> Result<(ID3D11Device, ID3D11DeviceContext, IDirect3DDevice)> {
    let (mut device, mut context) = (None, None);
    // SAFETY: the out parameters outlive the call, which fills them.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            None,
            Some(&raw mut context),
        )
    }
    .context("no Direct3D device could be made")?;
    let (device, context) = device.zip(context).context("no Direct3D device was made")?;
    let dxgi: IDXGIDevice = device.cast()?;
    // SAFETY: the device of DXGI is a live one made above.
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }?;
    Ok((device, context, inspectable.cast()?))
}

/// One frame of `item`, with the pointer if `cursor`, as RGBA.
fn take_frame(item: &GraphicsCaptureItem, cursor: bool) -> Result<RgbaImage> {
    if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
        bail!("Windows.Graphics.Capture is not supported here");
    }
    let (device, context, direct) = device()?;
    let size = item.Size()?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &direct,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        1,
        size,
    )?;
    let (arrived, frames) = mpsc::channel();
    pool.FrameArrived(&TypedEventHandler::new(
        move |pool: windows::core::Ref<'_, Direct3D11CaptureFramePool>, _| {
            if let Some(pool) = pool.as_ref() {
                arrived.send(pool.TryGetNextFrame()).ok();
            }
            Ok(())
        },
    ))?;
    let session = pool.CreateCaptureSession(item)?;
    session.SetIsCursorCaptureEnabled(cursor).ok();
    // Windows 10 has no such switch, and Windows 11 may refuse it; the
    // yellow border is then seen while the frame is taken.
    if let Err(error) = session.SetIsBorderRequired(false) {
        report!("Windows keeps the border of a capture: {error}");
    }
    session.StartCapture()?;
    let frame = frames.recv_timeout(FRAME_PATIENCE);
    let pixels = match frame {
        Ok(Ok(frame)) => {
            let surface = frame.Surface()?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
            // SAFETY: the surface of a frame is a texture of Direct3D 11.
            let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
            let read = read_texture(&device, &context, &texture);
            frame.Close().ok();
            read
        }
        Ok(Err(error)) => Err(anyhow!("the frame could not be read: {error}")),
        Err(_) => Err(anyhow!("no frame came")),
    };
    session.Close().ok();
    pool.Close().ok();
    pixels
}

/// The pixels of `texture`, copied to one the processor can read.
fn read_texture(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    texture: &ID3D11Texture2D,
) -> Result<RgbaImage> {
    let mut description = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `description` outlives the call that fills it.
    unsafe { texture.GetDesc(&raw mut description) };
    let staging_description = D3D11_TEXTURE2D_DESC {
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
        ..description
    };
    let mut staging = None;
    // SAFETY: the description outlives the call, which fills `staging`.
    unsafe { device.CreateTexture2D(&raw const staging_description, None, Some(&raw mut staging)) }
        .context("no texture to read could be made")?;
    let staging = staging.context("no texture to read was made")?;
    let (width, height) = (description.Width, description.Height);
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    // SAFETY: both textures are of one device, of the same size and
    // format; the mapped memory is read only while it is mapped, row by
    // row within its pitch.
    unsafe {
        context.CopyResource(&staging, texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped))
            .context("the texture could not be read")?;
        let row = width as usize * 4;
        for y in 0..height as usize {
            let from = mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize);
            let line = std::slice::from_raw_parts(from, row);
            pixels[y * row..(y + 1) * row].copy_from_slice(line);
        }
        context.Unmap(&staging, 0);
    }
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    RgbaImage::from_raw(width, height, pixels).context("the frame has the wrong size")
}

/// A bitmap of GDI on a device context of its own, freed when dropped.
struct Canvas {
    context: HDC,
    bitmap: HBITMAP,
    before: HGDIOBJ,
    width: i32,
    height: i32,
}

impl Canvas {
    /// A canvas of `width` by `height` like the screen.
    fn new(width: i32, height: i32) -> Result<Self> {
        // SAFETY: the screen's context is released before returning; what
        // is made is freed by `drop`.
        unsafe {
            let screen = GetDC(None);
            let context = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            ReleaseDC(None, screen);
            if context.is_invalid() || bitmap.is_invalid() {
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(context);
                bail!("no bitmap of {width} by {height} could be made");
            }
            let before = SelectObject(context, bitmap.into());
            Ok(Self {
                context,
                bitmap,
                before,
                width,
                height,
            })
        }
    }

    /// Draws the pointer where it is, if it is shown, `origin` being where
    /// the canvas lies on the screen.
    fn draw_pointer(&self, origin: (i32, i32)) {
        let mut info = CURSORINFO {
            cbSize: size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: the structures outlive the calls that fill them, and
        // the bitmaps of the icon's information are deleted again.
        unsafe {
            if GetCursorInfo(&raw mut info).is_err() || info.flags.0 & CURSOR_SHOWING.0 == 0 {
                return;
            }
            let mut icon = ICONINFO::default();
            if GetIconInfo(info.hCursor.into(), &raw mut icon).is_err() {
                return;
            }
            let x = info.ptScreenPos.x - origin.0 - icon.xHotspot as i32;
            let y = info.ptScreenPos.y - origin.1 - icon.yHotspot as i32;
            let _ = DrawIconEx(
                self.context,
                x,
                y,
                info.hCursor.into(),
                0,
                0,
                0,
                None,
                DI_NORMAL,
            );
            let _ = DeleteObject(icon.hbmColor.into());
            let _ = DeleteObject(icon.hbmMask.into());
        }
    }

    /// The pixels of the canvas, opaque.
    fn pixels(&self) -> Result<RgbaImage> {
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: self.width,
                // Negative for rows from the top down.
                biHeight: -self.height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; self.width as usize * self.height as usize * 4];
        // SAFETY: the bitmap is selected out for the call, as GetDIBits
        // asks, and back in after; the buffer is as large as described.
        let rows = unsafe {
            SelectObject(self.context, self.before);
            let rows = GetDIBits(
                self.context,
                self.bitmap,
                0,
                self.height as u32,
                Some(bgra.as_mut_ptr().cast()),
                &raw mut header,
                DIB_RGB_COLORS,
            );
            SelectObject(self.context, self.bitmap.into());
            rows
        };
        if rows != self.height {
            bail!("the bitmap could not be read");
        }
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = u8::MAX;
        }
        RgbaImage::from_raw(self.width as u32, self.height as u32, bgra)
            .context("the bitmap has the wrong size")
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: the objects were made by `new` and are freed once.
        unsafe {
            SelectObject(self.context, self.before);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.context);
        }
    }
}

/// `rect` of the screen copied with GDI, with the pointer if `cursor`.
fn gdi_screen(rect: ScreenRect, cursor: bool) -> Result<RgbaImage> {
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let canvas = Canvas::new(width, height)?;
    // SAFETY: the screen's context is released after the copy; layered
    // windows, such as Tessera's own, are copied as well.
    let copied = unsafe {
        let screen = GetDC(None);
        let copied = BitBlt(
            canvas.context,
            0,
            0,
            width,
            height,
            Some(screen),
            rect.left,
            rect.top,
            SRCCOPY | CAPTUREBLT,
        );
        ReleaseDC(None, screen);
        copied
    };
    copied.context("the screen could not be copied")?;
    if cursor {
        canvas.draw_pointer((rect.left, rect.top));
    }
    canvas.pixels()
}

/// `window` as it prints itself, cut to `seen`, what is seen of it.
fn print_window(window: HWND, seen: ScreenRect, cursor: bool) -> Result<RgbaImage> {
    let mut whole = RECT::default();
    // SAFETY: `whole` outlives the call that fills it.
    unsafe { GetWindowRect(window, &raw mut whole) }.context("the window is gone")?;
    let whole = ScreenRect::from(whole);
    let (width, height) = (whole.right - whole.left, whole.bottom - whole.top);
    if width <= 0 || height <= 0 {
        bail!("the window has no area");
    }
    let canvas = Canvas::new(width, height)?;
    // SAFETY: the window draws into the canvas, which outlives the call.
    let printed = unsafe {
        PrintWindow(
            window,
            canvas.context,
            PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT),
        )
    };
    if !printed.as_bool() {
        bail!("the window did not print itself");
    }
    if cursor {
        canvas.draw_pointer((whole.left, whole.top));
    }
    let pixels = canvas.pixels()?;
    let (x, y) = (seen.left - whole.left, seen.top - whole.top);
    let (x, y) = (x.clamp(0, width) as u32, y.clamp(0, height) as u32);
    let cut_width = ((seen.right - seen.left).max(1) as u32).min(width as u32 - x);
    let cut_height = ((seen.bottom - seen.top).max(1) as u32).min(height as u32 - y);
    Ok(image::imageops::crop_imm(&pixels, x, y, cut_width, cut_height).to_image())
}
