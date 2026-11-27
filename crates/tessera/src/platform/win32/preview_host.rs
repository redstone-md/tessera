// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The previews of files that programs draw themselves, as in the preview
//! pane of Explorer: the pages of a PDF, a document of Word, a sheet of
//! Excel, a mail, a font. Each kind of file names its preview handler
//! (`IPreviewHandler`) under the key
//! `{8895b1c6-b41f-4c1c-a562-0d564250836f}` of its shell extensions, which
//! `AssocQueryString` finds.
//!
//! GPUI draws a window with DirectComposition above whatever child
//! windows it has, so a handler cannot draw into a child of it. It draws
//! into a plain popup window of Tessera's own instead, owned by the
//! window of the file manager, so that it stays above that window, goes
//! with it when it is minimized, and lies below menus, which are topmost.
//! The file manager keeps the popup over the bounds of its preview pane,
//! and hides it when the pane hides.
//!
//! Handlers can take long and some hang, so the popup lives on a thread
//! of its own with its own message loop, in a single-threaded apartment;
//! the thread of GPUI only posts to it and awaits its answers. Handlers
//! are made in the surrogate process of Windows first
//! (`CLSCTX_LOCAL_SERVER`), as Explorer makes them, so that one that
//! crashes or hangs takes only that process with it; while the thread
//! waits for such a handler, COM keeps handling its messages. Where a
//! handler does not answer in time the file manager abandons its
//! [`PreviewHost`]: the popup is hidden, its thread is left to itself,
//! and the next preview gets a new one.
//!
//! Windows joins the input of the threads of an owned window and its
//! owner, and of a window and the windows of other processes inside it.
//! The thread of the popup lets go of the input of the file manager once
//! the popup is made; where Windows joins them again all the same, a
//! handler that hangs can still hold the keys and the focus of the file
//! manager until it is abandoned, for as long as the timeout of previews.
//! A request for a preview that a newer one followed before it was begun
//! is dropped, so that moving the cursor over many files shows only the
//! last.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock, mpsc};

use anyhow::{Context as _, Result, anyhow, bail};
use async_channel::{Receiver, Sender};
use gpui::{Bounds, Pixels, Window};
use windows::Win32::Foundation::{
    COLORREF, E_NOTIMPL, HWND, LPARAM, LRESULT, POINT, RECT, S_FALSE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateSolidBrush, DeleteObject, FillRect, HBRUSH, HDC,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CLSIDFromString, CoCreateInstance, STGM_READ,
    STGM_SHARE_DENY_NONE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::IObjectWithSite;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{
    ASSOCF_INIT_DEFAULTTOSTAR, ASSOCF_NOTRUNCATE, ASSOCSTR_SHELLEXTENSION, AssocQueryStringW,
    IInitializeWithItem, IPreviewHandler, IPreviewHandlerFrame, IPreviewHandlerFrame_Impl,
    IPreviewHandlerVisuals, IShellItem, PREVIEWHANDLERFRAMEINFO, SHCreateItemFromParsingName,
    SHCreateStreamOnFileEx,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_HWNDPARENT,
    GetClientRect, GetMessageW, GetWindowThreadProcessId, MSG, PostMessageW, PostQuitMessage,
    RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, ShowWindowAsync, TranslateMessage, WM_APP, WM_ERASEBKGND, WNDCLASSW,
    WS_CLIPCHILDREN, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{GUID, HSTRING, Interface as _, PCWSTR, PWSTR, implement, w};

use super::{NativeSurface, ShellThread};

/// The key of the shell extensions of a kind that names its preview
/// handler.
const PREVIEW_HANDLER: &str = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";

/// The class of the popups previews are drawn in.
const CLASS: PCWSTR = w!("Tessera.PreviewHost");

/// The message that tells a popup that commands wait.
const COMMANDS_WAITING: u32 = WM_APP + 0x61;

/// The colors a handler that takes them draws with, as `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewColors {
    pub background: u32,
    pub text: u32,
}

/// What came of showing a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewShown {
    /// Its handler draws it.
    Drawn,
    /// No handler is registered for its kind.
    NoHandler,
    /// The handler failed, for this reason.
    Failed(String),
}

/// What the thread of a popup is asked to do.
enum Command {
    Show {
        path: PathBuf,
        colors: PreviewColors,
        answer: Sender<PreviewShown>,
    },
    /// Puts the popup over this rectangle of the screen, in physical
    /// pixels, and shows it.
    Place(RECT),
    Hide,
    /// Lets go of the handler of the file shown.
    Unload,
    Quit,
}

/// A popup that previews files, on a thread of its own, until dropped.
pub struct PreviewHost {
    commands: mpsc::Sender<Command>,
    /// The popup, as a number, once its thread made it.
    window: Arc<AtomicIsize>,
    /// Where the popup was last put, so that it is moved only when that
    /// changes.
    placed: Option<RECT>,
}

impl PreviewHost {
    /// Starts a popup owned by `owner`, the window of the file manager.
    /// It returns at once; the popup is made on its thread.
    pub fn new(owner: &Window) -> Result<Self> {
        let owner = NativeSurface::of(owner)?.hwnd.0 as isize;
        let (commands, taken) = mpsc::channel();
        let window = Arc::new(AtomicIsize::new(0));
        let made = window.clone();
        ShellThread::run_alone("preview", move || host(owner, taken, made));
        Ok(Self {
            commands,
            window,
            placed: None,
        })
    }

    /// Has the handler of the kind of the file at `path` draw it in the
    /// popup, in place of what it drew before. The receiver yields once.
    pub fn show(&mut self, path: PathBuf, colors: PreviewColors) -> Receiver<PreviewShown> {
        let (answer, answered) = async_channel::bounded(1);
        self.send(Command::Show {
            path,
            colors,
            answer,
        });
        answered
    }

    /// Puts the popup over `bounds` of `window`, in its logical pixels as
    /// GPUI lays it out, and shows it.
    pub fn place(&mut self, window: &Window, bounds: Bounds<Pixels>) {
        let Ok(surface) = NativeSurface::of(window) else {
            return;
        };
        let scale = window.scale_factor();
        let mut origin = POINT::default();
        // SAFETY: the window belongs to this thread; the point outlives the
        // call.
        if !unsafe { ClientToScreen(surface.hwnd, &raw mut origin) }.as_bool() {
            return;
        }
        let at = |value: Pixels| (value / gpui::px(1.) * scale).round() as i32;
        let rect = RECT {
            left: origin.x + at(bounds.left()),
            top: origin.y + at(bounds.top()),
            right: origin.x + at(bounds.right()),
            bottom: origin.y + at(bounds.bottom()),
        };
        if self.placed != Some(rect) {
            self.placed = Some(rect);
            self.send(Command::Place(rect));
        }
    }

    /// Hides the popup, keeping what it shows.
    pub fn hide(&mut self) {
        if self.placed.take().is_some() {
            self.send(Command::Hide);
        }
    }

    /// Lets go of the handler of the file shown.
    pub fn unload(&mut self) {
        self.send(Command::Unload);
    }

    /// Gives the popup up while its handler does not answer: it is hidden
    /// without waiting for its thread, which is left to itself and ends
    /// once the handler answers.
    pub fn abandon(self) {
        let window = self.window.load(Ordering::SeqCst);
        if window != 0 {
            let window = HWND(window as *mut c_void);
            // SAFETY: both calls only post to the window, which may belong
            // to a thread that waits, and fail without harm where it is
            // gone; without an owner it no longer follows the file manager.
            unsafe {
                let _ = ShowWindowAsync(window, SW_HIDE);
                SetWindowLongPtrW(window, GWLP_HWNDPARENT, 0);
            }
        }
        // Dropping sends the thread away once it answers again.
    }

    fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            return;
        }
        let window = self.window.load(Ordering::SeqCst);
        if window != 0 {
            // SAFETY: posting to a window that is gone fails without harm.
            let _ = unsafe {
                PostMessageW(
                    Some(HWND(window as *mut c_void)),
                    COMMANDS_WAITING,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
        }
    }
}

impl Drop for PreviewHost {
    fn drop(&mut self) {
        self.send(Command::Quit);
    }
}

/// What the thread of a popup holds.
struct Host {
    window: HWND,
    commands: mpsc::Receiver<Command>,
    /// The commands taken from the channel and not yet done.
    waiting: VecDeque<Command>,
    handler: Option<IPreviewHandler>,
    frame: IPreviewHandlerFrame,
    background: Option<HBRUSH>,
    /// Whether a command is being done, inside which COM may hand the
    /// popup messages: then only placements are done at once.
    busy: bool,
    quitting: bool,
}

impl Host {
    /// Whether `command`, just taken from those waiting, is a preview
    /// nobody waits for any more or one a newer preview follows: then it
    /// is not begun.
    fn superseded(&self, command: &Command) -> bool {
        let Command::Show { answer, .. } = command else {
            return false;
        };
        answer.is_closed() || { self.waiting.iter() }
            .any(|waiting| matches!(waiting, Command::Show { .. }))
    }
}

thread_local! {
    /// The popup of this thread, for its window procedure.
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// The body of the thread of a popup: makes it, does what it is asked,
/// and ends when asked to or when its [`PreviewHost`] is gone.
fn host(owner: isize, commands: mpsc::Receiver<Command>, made: Arc<AtomicIsize>) {
    let window = match create_window(HWND(owner as *mut c_void)) {
        Ok(window) => window,
        Err(error) => {
            report!("previews cannot be shown: {error:#}");
            // Every request is answered, so that nobody waits for long.
            for command in commands {
                if let Command::Show { answer, .. } = command {
                    answer
                        .try_send(PreviewShown::Failed(format!("{error:#}")))
                        .ok();
                }
            }
            return;
        }
    };
    HOST.with(|host| {
        *host.borrow_mut() = Some(Host {
            window,
            commands,
            waiting: VecDeque::new(),
            handler: None,
            frame: Frame.into(),
            background: None,
            busy: false,
            quitting: false,
        })
    });
    made.store(window.0 as isize, Ordering::SeqCst);
    // What was asked before the popup was made.
    work();
    let mut message = MSG::default();
    // SAFETY: `message` outlives the calls that fill and pass it on.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
    HOST.with(|host| host.borrow_mut().take());
}

/// Does what waits: every command where none is being done, or else only
/// the placements, which COM may let through while a handler is asked.
fn work() {
    loop {
        let command = HOST.with(|host| {
            let mut host = host.try_borrow_mut().ok()?;
            let host = host.as_mut()?;
            loop {
                match host.commands.try_recv() {
                    Ok(command) => host.waiting.push_back(command),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        host.waiting.push_back(Command::Quit);
                        break;
                    }
                }
            }
            if host.busy {
                let at = { host.waiting.iter() }
                    .position(|command| matches!(command, Command::Place(_) | Command::Hide))?;
                return host.waiting.remove(at);
            }
            let command = loop {
                let command = host.waiting.pop_front()?;
                if !host.superseded(&command) {
                    break command;
                }
                if let Command::Show { answer, .. } = command {
                    let newer = "a newer preview was asked for".to_owned();
                    answer.try_send(PreviewShown::Failed(newer)).ok();
                }
            };
            host.busy = !matches!(command, Command::Place(_) | Command::Hide);
            Some(command)
        });
        let Some(command) = command else {
            return;
        };
        let lasting = !matches!(command, Command::Place(_) | Command::Hide);
        perform(command);
        if lasting {
            let quitting = HOST.with(|host| {
                let mut host = host.borrow_mut();
                let host = host.as_mut()?;
                host.busy = false;
                Some(host.quitting)
            });
            if quitting != Some(false) {
                return;
            }
        }
    }
}

/// Does one command. The host is not borrowed while a handler is asked,
/// as COM may call the window procedure meanwhile.
fn perform(command: Command) {
    let held = HOST.with(|host| {
        let host = host.borrow();
        host.as_ref().map(|host| (host.window, host.frame.clone()))
    });
    let Some((window, frame)) = held else {
        return;
    };
    match command {
        Command::Show {
            path,
            colors,
            answer,
        } => {
            unload();
            set_background(window, colors.background);
            let shown = match preview(window, &frame, &path, colors) {
                Ok(Some(handler)) => {
                    HOST.with(|host| {
                        if let Some(host) = host.borrow_mut().as_mut() {
                            host.handler = Some(handler);
                        }
                    });
                    PreviewShown::Drawn
                }
                Ok(None) => PreviewShown::NoHandler,
                Err(error) => PreviewShown::Failed(format!("{error:#}")),
            };
            answer.try_send(shown).ok();
        }
        Command::Place(rect) => {
            // SAFETY: the popup belongs to this thread.
            let _ = unsafe {
                SetWindowPos(
                    window,
                    None,
                    rect.left,
                    rect.top,
                    (rect.right - rect.left).max(1),
                    (rect.bottom - rect.top).max(1),
                    SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
                )
            };
            // While the handler is told, COM may hand the popup messages:
            // only placements are done then, which move the popup alone.
            let handler = HOST.with(|host| {
                let mut host = host.borrow_mut();
                let host = host.as_mut()?;
                let handler = host.handler.clone().filter(|_| !host.busy)?;
                host.busy = true;
                Some(handler)
            });
            if let Some(handler) = handler {
                let mut client = client_rect(window);
                loop {
                    // SAFETY: the handler was made on this thread; the
                    // rectangle outlives the call.
                    let _ = unsafe { handler.SetRect(&raw const client) };
                    // A placement done meanwhile is told too.
                    let now = client_rect(window);
                    if now == client {
                        break;
                    }
                    client = now;
                }
                HOST.with(|host| {
                    if let Some(host) = host.borrow_mut().as_mut() {
                        host.busy = false;
                    }
                });
            }
        }
        Command::Hide => {
            // SAFETY: the popup belongs to this thread.
            let _ = unsafe { ShowWindow(window, SW_HIDE) };
        }
        Command::Unload => unload(),
        Command::Quit => {
            unload();
            HOST.with(|host| {
                if let Some(host) = host.borrow_mut().as_mut() {
                    host.quitting = true;
                    if let Some(brush) = host.background.take() {
                        // SAFETY: the brush was made by this thread and is
                        // no longer selected anywhere.
                        let _ = unsafe { DeleteObject(brush.into()) };
                    }
                }
            });
            // SAFETY: the popup belongs to this thread; quitting ends its
            // message loop.
            unsafe {
                let _ = DestroyWindow(window);
                PostQuitMessage(0);
            }
        }
    }
}

/// Lets go of the handler of the file shown, if there is one.
fn unload() {
    let handler = HOST.with(|host| host.borrow_mut().as_mut()?.handler.take());
    if let Some(handler) = handler {
        // SAFETY: the handler was made on this thread.
        unsafe {
            let _ = handler.Unload();
            if let Ok(site) = handler.cast::<IObjectWithSite>() {
                let _ = site.SetSite(None);
            }
        }
    }
}

/// Fills the popup with `color` where the handler draws nothing.
fn set_background(window: HWND, color: u32) {
    // SAFETY: the brush is made and deleted on this thread; the old one
    // is no longer used once replaced.
    unsafe {
        let brush = CreateSolidBrush(colorref(color));
        let old = HOST.with(|host| {
            let mut host = host.borrow_mut();
            host.as_mut()?.background.replace(brush)
        });
        if let Some(old) = old {
            let _ = DeleteObject(old.into());
        }
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(window), None, true);
    }
}

/// `0xRRGGBB` as Windows takes a color: `0x00BBGGRR`.
fn colorref(rgb: u32) -> COLORREF {
    let (red, green, blue) = ((rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff);
    COLORREF(red | (green << 8) | (blue << 16))
}

/// Has the handler of the kind of `path` draw it in `window`; `None`
/// where its kind has no handler.
fn preview(
    window: HWND,
    frame: &IPreviewHandlerFrame,
    path: &Path,
    colors: PreviewColors,
) -> Result<Option<IPreviewHandler>> {
    let Some(class) = handler_of(path) else {
        return Ok(None);
    };
    // SAFETY: COM is initialized on this thread (`run_alone`); everything
    // made here stays on it, and the rectangle outlives the calls.
    unsafe {
        let handler: IPreviewHandler = CoCreateInstance(&class, None, CLSCTX_LOCAL_SERVER)
            .or_else(|_| CoCreateInstance(&class, None, CLSCTX_INPROC_SERVER))
            .context("the preview handler of the kind did not start")?;
        initialize(&handler, path)?;
        if let Ok(site) = handler.cast::<IObjectWithSite>() {
            let _ = site.SetSite(frame);
        }
        if let Ok(visuals) = handler.cast::<IPreviewHandlerVisuals>() {
            let _ = visuals.SetBackgroundColor(colorref(colors.background));
            let _ = visuals.SetTextColor(colorref(colors.text));
        }
        let client = client_rect(window);
        handler
            .SetWindow(window, &raw const client)
            .context("the preview handler takes no window")?;
        handler
            .DoPreview()
            .context("the preview handler did not draw the file")?;
        Ok(Some(handler))
    }
}

/// Hands `path` to `handler` in the way it takes files: as a stream, as
/// an item of the Shell, or by its path.
///
/// # Safety
///
/// COM is initialized on this thread, which made `handler`.
unsafe fn initialize(handler: &IPreviewHandler, path: &Path) -> Result<()> {
    let name = HSTRING::from(path.to_string_lossy().replace('/', "\\"));
    // Others may write the file while it is shown, as the details pane
    // writes its properties.
    let mode = (STGM_READ | STGM_SHARE_DENY_NONE).0;
    // SAFETY: as the caller promises; the name outlives the calls.
    unsafe {
        if let Ok(taking) = handler.cast::<IInitializeWithStream>() {
            let stream = SHCreateStreamOnFileEx(&name, mode, 0, false, None)
                .context("the file cannot be opened")?;
            return { taking.Initialize(&stream, STGM_READ.0) }
                .context("the preview handler did not take the file");
        }
        if let Ok(taking) = handler.cast::<IInitializeWithItem>() {
            let item: IShellItem = SHCreateItemFromParsingName(&name, None)
                .context("the Shell does not know the file")?;
            return { taking.Initialize(&item, STGM_READ.0) }
                .context("the preview handler did not take the file");
        }
        if let Ok(taking) = handler.cast::<IInitializeWithFile>() {
            return { taking.Initialize(&name, STGM_READ.0) }
                .context("the preview handler did not take the file");
        }
    }
    bail!("the preview handler takes files in no way Tessera knows")
}

/// The preview handler registered for the kind of `path`.
fn handler_of(path: &Path) -> Option<GUID> {
    let extension = path.extension()?.to_string_lossy();
    let kind = HSTRING::from(format!(".{extension}"));
    let key = HSTRING::from(PREVIEW_HANDLER);
    let mut text = [0u16; 64];
    let mut length = text.len() as u32;
    // SAFETY: the buffer holds `length` characters and outlives the call;
    // the texts are NUL-terminated.
    unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_DEFAULTTOSTAR | ASSOCF_NOTRUNCATE,
            ASSOCSTR_SHELLEXTENSION,
            &kind,
            &key,
            Some(PWSTR(text.as_mut_ptr())),
            &raw mut length,
        )
        .ok()
        .ok()?;
        CLSIDFromString(PCWSTR(text.as_ptr())).ok()
    }
}

/// The client area of `window`.
fn client_rect(window: HWND) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: the rectangle outlives the call.
    let _ = unsafe { GetClientRect(window, &raw mut rect) };
    rect
}

/// Makes the popup, owned by `owner` and hidden.
fn create_window(owner: HWND) -> Result<HWND> {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    // SAFETY: the class structure and the strings outlive the calls, and
    // `receive` has the signature of a window procedure.
    unsafe {
        let module = GetModuleHandleW(None).context("the module handle could not be read")?;
        let registered = *REGISTERED.get_or_init(|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(receive),
                hInstance: module.into(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassW(&raw const class) != 0
        });
        if !registered {
            return Err(anyhow!(
                "the class of the preview window was not registered"
            ));
        }
        let window = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS,
            w!("Preview"),
            WS_POPUP | WS_CLIPCHILDREN,
            0,
            0,
            0,
            0,
            Some(owner),
            None,
            Some(module.into()),
            None,
        )
        .context("the preview window could not be made")?;
        // Windows joined the input of this thread to that of the owner;
        // a handler that hangs here should not hold the file manager.
        let owner_thread = GetWindowThreadProcessId(owner, None);
        if owner_thread != 0 {
            let _ = AttachThreadInput(GetCurrentThreadId(), owner_thread, false);
        }
        Ok(window)
    }
}

/// The window procedure of the popups. Nothing may unwind out of it.
unsafe extern "system" fn receive(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let answered = std::panic::catch_unwind(|| match message {
        COMMANDS_WAITING => {
            work();
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => {
            let brush = HOST.with(|host| host.try_borrow().ok()?.as_ref()?.background);
            let brush = brush?;
            let rect = client_rect(window);
            // SAFETY: the device context is the one Windows handed the
            // message, valid while it is handled.
            unsafe { FillRect(HDC(w.0 as *mut c_void), &raw const rect, brush) };
            Some(LRESULT(1))
        }
        _ => None,
    });
    match answered {
        Ok(Some(answer)) => answer,
        Ok(None) => {
            // SAFETY: the arguments are passed on as they came.
            unsafe { DefWindowProcW(window, message, w, l) }
        }
        Err(_) => {
            report!("a message to the preview window was not handled");
            // SAFETY: as above.
            unsafe { DefWindowProcW(window, message, w, l) }
        }
    }
}

/// What a preview handler is told of the window around it: it has no
/// keys of its own to share.
#[implement(IPreviewHandlerFrame)]
struct Frame;

impl IPreviewHandlerFrame_Impl for Frame_Impl {
    fn GetWindowContext(&self) -> windows::core::Result<PREVIEWHANDLERFRAMEINFO> {
        Err(E_NOTIMPL.into())
    }

    fn TranslateAccelerator(&self, _message: *const MSG) -> windows::core::Result<()> {
        // Not a key of the frame: the handler handles it.
        Err(S_FALSE.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_turned_round_for_windows() {
        assert_eq!(colorref(0x112233), COLORREF(0x332211));
        assert_eq!(colorref(0xff0000), COLORREF(0x0000ff));
    }
}
