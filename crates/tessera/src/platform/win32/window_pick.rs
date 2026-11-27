// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Picking a window with the pointer, as KWin's "Detect window properties"
//! does: every display is covered by a window of Tessera's that is all but
//! invisible, takes one click and shows a crosshair; the window under the
//! pointer is outlined as the border of tiled windows is drawn, and a
//! click picks it, Escape or the right button cancel. On the click the
//! covers go away and the window at the point is read with
//! `WindowFromPoint`, then the window that owns it with
//! `GetAncestor(GA_ROOTOWNER)`: a click on a dialog picks the window of the
//! program.
//!
//! The covers and the outline belong to a thread of their own, which ends
//! once a window is picked or the picking is cancelled, so that the shell
//! never waits for them.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Once;
use std::{ptr, thread};

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, GA_ROOTOWNER,
    GWL_EXSTYLE, GetAncestor, GetCursorPos, GetMessageW, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, HWND_TOPMOST, IDC_CROSS, IsIconic, IsWindowVisible, LWA_ALPHA,
    LoadCursorW, MSG, PostQuitMessage, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SetForegroundWindow, SetLayeredWindowAttributes, SetWindowPos, ShowWindow,
    TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WINDOW_EX_STYLE, WM_ACTIVATEAPP, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_RBUTTONDOWN, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, w};

use super::snap::Display;
use super::tile_border::ring;
use super::{AppWindowId, ScreenRect};

/// How opaque the covers are, of 255: as little as still takes clicks.
const COVER_ALPHA: u8 = 1;

/// How wide the outline is, in logical pixels.
const OUTLINE: i32 = 3;

/// The class of the covers and of the outline, which clicks go through.
const CLASS: PCWSTR = w!("TesseraWindowPick");

/// Picks a window with the pointer.
pub struct WindowPick;

impl WindowPick {
    /// Covers the displays and lets the user click the window to pick,
    /// outlining it in `color` (0xAARRGGBB) while the pointer is over it.
    /// The window picked arrives once on the receiver, or `None` when the
    /// picking was cancelled or could not begin; a window of Tessera's
    /// own is not picked.
    pub fn start(color: u32) -> Receiver<Option<AppWindowId>> {
        let (sender, receiver) = async_channel::bounded(1);
        let given = sender.clone();
        let started = thread::Builder::new()
            .name("tessera window pick".to_owned())
            .spawn(move || run(given, color));
        if let Err(error) = started {
            report!("no window can be picked: {error}");
            let _ = sender.try_send(None);
        }
        receiver
    }
}

/// What the thread of the picking holds.
struct Picking {
    covers: Vec<HWND>,
    outline: Option<HWND>,
    /// The window outlined now.
    outlined: Option<AppWindowId>,
    color: u32,
    answer: Sender<Option<AppWindowId>>,
    done: bool,
}

thread_local! {
    static PICKING: RefCell<Option<Picking>> = const { RefCell::new(None) };
}

/// Registers the class of the covers and of the outline, once.
fn register(module: windows::Win32::Foundation::HINSTANCE) {
    static REGISTERED: Once = Once::new();
    REGISTERED.call_once(|| {
        // SAFETY: the class names a literal and this program's function,
        // which is never unloaded; the cursor is one of Windows' own.
        unsafe {
            let cursor = LoadCursorW(None, IDC_CROSS).unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(respond),
                hInstance: module,
                hCursor: cursor,
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassW(&raw const class);
        }
    });
}

/// Makes a window over `frame`, of this thread.
fn make(
    extended: WINDOW_EX_STYLE,
    frame: ScreenRect,
    module: windows::Win32::Foundation::HINSTANCE,
) -> Option<HWND> {
    // SAFETY: the class was registered by this thread's program and the
    // name is a literal.
    unsafe {
        CreateWindowExW(
            extended,
            CLASS,
            w!(""),
            WS_POPUP,
            frame.left,
            frame.top,
            frame.right - frame.left,
            frame.bottom - frame.top,
            None,
            None,
            Some(module),
            None,
        )
    }
    .ok()
}

/// Covers the displays, waits for the click and answers it.
fn run(answer: Sender<Option<AppWindowId>>, color: u32) {
    // SAFETY: the call reads the handle of this program.
    let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
        let _ = answer.try_send(None);
        return;
    };
    let module = module.into();
    register(module);
    let cover_style = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
    let covers: Vec<HWND> = { Display::all().into_iter() }
        .filter_map(|display| make(cover_style, display.frame, module))
        .collect();
    if covers.is_empty() {
        report!("no window can be picked: the displays cannot be covered");
        let _ = answer.try_send(None);
        return;
    }
    let outline_style =
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
    let nowhere = ScreenRect {
        left: 0,
        top: 0,
        right: 1,
        bottom: 1,
    };
    let outline = make(outline_style, nowhere, module);
    // SAFETY: the windows belong to this thread; Windows may refuse the
    // keyboard, which only leaves Escape unheard.
    unsafe {
        for &cover in &covers {
            let _ = SetLayeredWindowAttributes(cover, COLORREF(0), COVER_ALPHA, LWA_ALPHA);
            let _ = ShowWindow(cover, SW_SHOWNOACTIVATE);
        }
        let _ = SetForegroundWindow(covers[0]);
        let _ = SetFocus(Some(covers[0]));
    }
    PICKING.with_borrow_mut(|picking| {
        *picking = Some(Picking {
            covers,
            outline,
            outlined: None,
            color,
            answer,
            done: false,
        });
    });
    follow();
    let mut message = MSG::default();
    // SAFETY: `message` outlives every call; the loop hands the messages
    // of this thread's windows to where they belong until the picking ends.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

unsafe extern "system" fn respond(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_MOUSEMOVE => {
            follow();
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            finish(true);
            return LRESULT(0);
        }
        WM_RBUTTONDOWN => {
            finish(false);
            return LRESULT(0);
        }
        WM_KEYDOWN if wparam.0 == usize::from(VK_ESCAPE.0) => {
            finish(false);
            return LRESULT(0);
        }
        // Another program took the keyboard, as the Windows key does:
        // Escape would no longer be heard, so the picking ends.
        WM_ACTIVATEAPP if wparam.0 == 0 => finish(false),
        _ => {}
    }
    // SAFETY: what was given is passed on as it is.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// Where the pointer is.
fn pointer() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    // SAFETY: `point` outlives the call.
    unsafe { GetCursorPos(&raw mut point) }
        .ok()
        .map(|()| (point.x, point.y))
}

/// The window of another program that would be picked at `point` while the
/// covers lie over the displays: the topmost shown one there, by the order
/// of windows, and the window that owns it.
fn under((x, y): (i32, i32)) -> Option<AppWindowId> {
    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY: `gather` receives the pointer to `handles`, which outlives
    // the synchronous enumeration.
    unsafe {
        let _ = EnumWindows(Some(gather), LPARAM(&raw mut handles as isize));
    }
    let own = std::process::id();
    let found = handles.into_iter().find(|&hwnd| {
        let id = AppWindowId::from_raw(hwnd.0 as isize);
        let mut whole = RECT::default();
        // SAFETY: the calls only read the state, the style and the place of
        // a window, into a structure that outlives them.
        unsafe {
            if id.process() == own || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return false;
            }
            let extended = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if extended & WS_EX_TRANSPARENT.0 != 0 || id.is_cloaked() {
                return false;
            }
            if GetWindowRect(hwnd, &raw mut whole).is_err() {
                return false;
            }
        }
        x >= whole.left && x < whole.right && y >= whole.top && y < whole.bottom
    })?;
    root_owner(found)
}

/// The window that owns `hwnd` at the top, or itself; `None` for a window
/// of Tessera's own.
fn root_owner(hwnd: HWND) -> Option<AppWindowId> {
    // SAFETY: the calls only read a relation of a window and whose it is.
    unsafe {
        let root = GetAncestor(hwnd, GA_ROOTOWNER);
        let root = if root.is_invalid() { hwnd } else { root };
        let mut process = 0u32;
        GetWindowThreadProcessId(root, Some(&raw mut process));
        (process != std::process::id()).then(|| AppWindowId::from_raw(root.0 as isize))
    }
}

unsafe extern "system" fn gather(hwnd: HWND, state: LPARAM) -> BOOL {
    // SAFETY: `state` is the pointer `under` passed in.
    let handles = unsafe { &mut *(state.0 as *mut Vec<HWND>) };
    handles.push(hwnd);
    true.into()
}

/// Outlines the window under the pointer, if it is another than the one
/// outlined.
fn follow() {
    let Some(at) = pointer() else {
        return;
    };
    let window = under(at);
    PICKING.with_borrow_mut(|picking| {
        let Some(picking) = picking.as_mut() else {
            return;
        };
        if picking.outlined == window {
            return;
        }
        picking.outlined = window;
        let Some(outline) = picking.outline else {
            return;
        };
        match window.and_then(AppWindowId::frame) {
            Some(frame) => draw_outline(outline, frame, picking.color),
            // SAFETY: the window belongs to this thread.
            None => unsafe {
                let _ = ShowWindow(outline, SW_HIDE);
            },
        }
    });
}

/// Ends the picking: with the window at the pointer where `picked`, else
/// with none.
fn finish(picked: bool) {
    let Some((covers, outline, answer)) = PICKING.with_borrow_mut(|picking| {
        let picking = picking.as_mut()?;
        if picking.done {
            return None;
        }
        picking.done = true;
        let covers = std::mem::take(&mut picking.covers);
        Some((covers, picking.outline.take(), picking.answer.clone()))
    }) else {
        return;
    };
    // SAFETY: the windows belong to this thread; once they are hidden the
    // window at the point is the one under them.
    let chosen = unsafe {
        for &cover in &covers {
            let _ = ShowWindow(cover, SW_HIDE);
        }
        if let Some(outline) = outline {
            let _ = ShowWindow(outline, SW_HIDE);
        }
        let chosen = picked
            .then(pointer)
            .flatten()
            .and_then(|(x, y)| {
                let window =
                    windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(POINT { x, y });
                (!window.is_invalid()).then_some(window)
            })
            .and_then(root_owner);
        for cover in covers {
            let _ = DestroyWindow(cover);
        }
        if let Some(outline) = outline {
            let _ = DestroyWindow(outline);
        }
        PostQuitMessage(0);
        chosen
    };
    let _ = answer.try_send(chosen);
}

/// Shows `outline` as a ring around `frame`, in `color`, just above
/// every other window.
fn draw_outline(outline: HWND, frame: ScreenRect, color: u32) {
    // SAFETY: the call only reads the density of a window of this thread.
    let dpi = match unsafe { GetDpiForWindow(outline) } {
        0 => 96,
        dpi => dpi as i32,
    };
    let thickness = (OUTLINE * dpi / 96).max(1);
    let (left, top) = (frame.left - thickness, frame.top - thickness);
    let width = frame.right - frame.left + 2 * thickness;
    let height = frame.bottom - frame.top + 2 * thickness;
    if width <= 0 || height <= 0 {
        return;
    }
    let pixels = ring(width, height, thickness, 0, color);
    if paint(outline, &pixels, (width, height), (left, top)) {
        // SAFETY: the window belongs to this thread.
        unsafe {
            let flags = SWP_NOACTIVATE | SWP_SHOWWINDOW;
            let _ = SetWindowPos(outline, Some(HWND_TOPMOST), left, top, width, height, flags);
        }
    }
}

/// Hands `pixels`, `size` large, to the layered window `window` at `at` on
/// the screen. Whether Windows took them is returned.
fn paint(window: HWND, pixels: &[u32], size: (i32, i32), at: (i32, i32)) -> bool {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size.0,
            // A negative height has the rows start at the top.
            biHeight: -size.1,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `info` describes a bitmap of exactly the pixels of `pixels`,
    // which are copied into the memory Windows made for it; the contexts
    // and the bitmap are released before returning.
    unsafe {
        let mut bits: *mut c_void = ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(
            None,
            &raw const info,
            DIB_RGB_COLORS,
            &raw mut bits,
            None,
            0,
        ) else {
            return false;
        };
        if bits.is_null() {
            let _ = DeleteObject(bitmap.into());
            return false;
        }
        ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u32>(), pixels.len());
        let screen = GetDC(None);
        let source = CreateCompatibleDC(Some(screen));
        let before = SelectObject(source, bitmap.into());
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let place = POINT { x: at.0, y: at.1 };
        let extent = SIZE {
            cx: size.0,
            cy: size.1,
        };
        let from = POINT::default();
        let updated = UpdateLayeredWindow(
            window,
            Some(screen),
            Some(&raw const place),
            Some(&raw const extent),
            Some(source),
            Some(&raw const from),
            COLORREF(0),
            Some(&raw const blend),
            ULW_ALPHA,
        );
        SelectObject(source, before);
        let _ = DeleteDC(source);
        let _ = DeleteObject(bitmap.into());
        ReleaseDC(None, screen);
        updated.is_ok()
    }
}
