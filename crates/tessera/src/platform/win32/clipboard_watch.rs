// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The clipboard as the history of it sees it: what is copied, as it is
//! copied, and the way back onto the clipboard.
//!
//! A thread of its own owns a message-only window that Windows tells of
//! every change of the clipboard (`AddClipboardFormatListener`). On each
//! change the thread opens the clipboard, copies out what the history
//! keeps (a list of files, a text, or a picture), closes it again and
//! only then decodes, so the clipboard is held for as short a time as the
//! copying takes. Content the copying program marks as not for a history
//! (password managers do) is left alone. The same thread puts an earlier
//! item back, as the owner of the clipboard, so that its own change is not
//! taken for a new copy.
//!
//! [`paste_into`] then presses Ctrl+V for the user in the window that was
//! in front before the history was opened.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::{OsString, c_void};
use std::io::Cursor;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use async_channel::{Receiver, Sender};
use image::codecs::bmp::BmpDecoder;
use image::codecs::png::PngDecoder;
use image::{DynamicImage, ImageDecoder, RgbaImage};
use windows::Win32::Foundation::{
    GlobalFree, HANDLE, HGLOBAL, HMODULE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    RemoveClipboardFormatListener, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL,
};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE,
    MSG, PostMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_CLIPBOARDUPDATE, WM_CLOSE, WM_DESTROY, WNDCLASSW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_FOREGROUND, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};
use windows::core::{PCWSTR, w};

use super::AppWindowId;
use super::hotkeys::Modifiers;
use super::windows_key::MADE_HERE;

/// The clipboard formats Windows numbers itself.
const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;

/// The formats programs register by name: a picture as a PNG file, the
/// HTML and RTF of rich text, whether files were cut, and the marks of
/// content not to be kept in a history.
const PNG: &str = "PNG";
const HTML: &str = "HTML Format";
const RTF: &str = "Rich Text Format";
const DROP_EFFECT: &str = "Preferred DropEffect";
const EXCLUDE: &str = "ExcludeClipboardContentFromMonitorProcessing";
const VIEWER_IGNORE: &str = "Clipboard Viewer Ignore";
const IN_HISTORY: &str = "CanIncludeInClipboardHistory";

/// What `Preferred DropEffect` says of files that were copied, not cut.
const DROPEFFECT_COPY: u32 = 1;

/// The most characters of a text that is kept; a longer text is not.
const TEXT_LIMIT: usize = 1 << 20;

/// The most pixels of a picture that is kept (20 megapixels); a larger
/// picture is not.
const PIXEL_LIMIT: u64 = 20_000_000;

/// The most bytes of a format that are copied out of the clipboard: a
/// picture of [`PIXEL_LIMIT`] pixels in four bytes each, with its header.
const BYTE_LIMIT: usize = PIXEL_LIMIT as usize * 4 + 4096;

/// How often the clipboard is tried, and how long is waited between two
/// tries: another program may hold it open for a moment.
const OPEN_TRIES: u32 = 6;
const OPEN_PAUSE: Duration = Duration::from_millis(20);

/// How long the window to paste into is waited for to come in front, and
/// the keys held by the user to be let go.
const FRONT_PATIENCE: Duration = Duration::from_millis(600);
const KEYS_PATIENCE: Duration = Duration::from_millis(1500);
const PATIENCE_STEP: Duration = Duration::from_millis(15);

/// The message that tells the window that requests wait.
const REQUESTS_WAITING: u32 = WM_APP + 0x43;

/// The class of the window of the watch.
const CLASS: PCWSTR = w!("Tessera.ClipboardWatch");

/// The key with the letter V.
const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);

/// What was copied, as the history keeps it.
#[derive(Clone, Debug, PartialEq)]
pub enum Clip {
    /// A text; rich text (HTML or RTF) without a plain text beside it is
    /// kept as the text it reads as.
    Text(String),
    /// Files and folders copied or cut in Explorer or elsewhere.
    Files(Vec<PathBuf>),
    /// A picture, with its transparency where the copying program gave
    /// one.
    Image(RgbaImage),
}

/// A clipboard format to put an offer in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    /// One Windows numbers itself.
    Standard(u32),
    /// One programs register by name.
    Named(&'static str),
}

impl Format {
    /// The number of the format in this session.
    fn id(self) -> u32 {
        match self {
            Self::Standard(id) => id,
            Self::Named(name) => format_named(name),
        }
    }
}

/// What to put on the clipboard: each format with its bytes, which
/// [`ClipboardWriter::put`] hands to the clipboard as they are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Offer {
    formats: Vec<(Format, Vec<u8>)>,
}

impl Offer {
    /// A text, as `CF_UNICODETEXT`.
    pub fn text(text: &str) -> Self {
        Self {
            formats: vec![(Format::Standard(CF_UNICODETEXT), wide_bytes(text))],
        }
    }

    /// Files to be copied where they are pasted, as Explorer offers them:
    /// `CF_HDROP` with `Preferred DropEffect` saying copy.
    pub fn files(paths: &[PathBuf]) -> Self {
        let effect = DROPEFFECT_COPY.to_le_bytes().to_vec();
        Self {
            formats: vec![
                (Format::Standard(CF_HDROP), drop_files(paths)),
                (Format::Named(DROP_EFFECT), effect),
            ],
        }
    }

    /// A picture: as a device-independent bitmap, which every program
    /// reads and Windows turns into the other bitmap formats, and as the
    /// PNG file `png`, which keeps its transparency for the programs that
    /// read that format.
    pub fn image(pixels: &RgbaImage, png: Vec<u8>) -> Self {
        Self {
            formats: vec![
                (Format::Standard(CF_DIB), dib_of(pixels)),
                (Format::Named(PNG), png),
            ],
        }
    }
}

/// What the thread of the watch and those who ask it share.
struct Shared {
    /// The window of the watch, as a number; zero until it is made.
    window: AtomicIsize,
    /// Whether the watch is to end.
    stop: AtomicBool,
    /// Whether pictures are kept.
    images: AtomicBool,
    /// The window of another program that came in front last, as a
    /// number; zero before one did.
    front: AtomicIsize,
}

impl Shared {
    /// Tells the thread that requests wait. Before its window exists the
    /// thread answers them once it has made it.
    fn wake(&self) {
        let window = self.window.load(Ordering::SeqCst);
        if window == 0 {
            return;
        }
        // SAFETY: posting to a window that has been destroyed fails without
        // side effects.
        let _ =
            unsafe { PostMessageW(Some(handle(window)), REQUESTS_WAITING, WPARAM(0), LPARAM(0)) };
    }
}

/// A request to put an offer on the clipboard, with where the answer goes.
type Request = (Offer, Sender<Result<(), String>>);

/// The watch of the clipboard, for as long as it exists: dropping it ends
/// the thread and lets go of the clipboard.
pub struct ClipboardWatch {
    shared: Arc<Shared>,
    requests: mpsc::Sender<Request>,
}

impl ClipboardWatch {
    /// Starts watching the clipboard, keeping pictures if `images`, and
    /// returns at once; what is copied from then on arrives on the
    /// receiver. A thread that cannot be started is the error; a window
    /// the thread cannot make is reported, and the receiver closes, as it
    /// does whenever the watch ends.
    pub fn start(images: bool) -> Result<(Self, Receiver<Clip>), String> {
        let shared = Arc::new(Shared {
            window: AtomicIsize::new(0),
            stop: AtomicBool::new(false),
            images: AtomicBool::new(images),
            front: AtomicIsize::new(0),
        });
        let (requests, waiting) = mpsc::channel();
        let (clips, copied) = async_channel::unbounded();
        let thread_shared = shared.clone();
        thread::Builder::new()
            .name("clipboard".to_owned())
            .spawn(move || watch(thread_shared, waiting, clips))
            .map_err(|error| error.to_string())?;
        Ok((Self { shared, requests }, copied))
    }

    /// Keeps pictures from now on if `images`, or leaves them out.
    pub fn keep_images(&self, images: bool) {
        self.shared.images.store(images, Ordering::Relaxed);
    }

    /// The window of another program that was in front last while the
    /// clipboard was watched, if it is still there: where the user was
    /// before a window of Tessera came in front.
    pub fn window_before(&self) -> Option<AppWindowId> {
        let raw = self.shared.front.load(Ordering::Relaxed);
        let window = AppWindowId::from_raw(raw);
        (raw != 0 && window.exists()).then_some(window)
    }

    /// What puts offers on the clipboard through this watch, from any
    /// thread.
    pub fn writer(&self) -> ClipboardWriter {
        ClipboardWriter {
            shared: self.shared.clone(),
            requests: self.requests.clone(),
        }
    }
}

impl Drop for ClipboardWatch {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let window = self.shared.window.load(Ordering::SeqCst);
        if window != 0 {
            // SAFETY: posting to a window that has been destroyed fails
            // without side effects.
            let _ = unsafe { PostMessageW(Some(handle(window)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
    }
}

/// Puts offers on the clipboard through the thread of a watch.
#[derive(Clone)]
pub struct ClipboardWriter {
    shared: Arc<Shared>,
    requests: mpsc::Sender<Request>,
}

impl ClipboardWriter {
    /// Has `offer` put on the clipboard in place of what is there, and
    /// gives the answer once it is: awaited with `recv().await` on the
    /// thread of GPUI, or with `recv_blocking()` on a thread of its own.
    /// Where the watch has ended, the answer says so.
    pub fn put(&self, offer: Offer) -> Receiver<Result<(), String>> {
        let (answer, answered) = async_channel::bounded(1);
        match self.requests.send((offer, answer)) {
            Ok(()) => self.shared.wake(),
            Err(mpsc::SendError((_, answer))) => {
                let ended = "the history of the clipboard has ended".to_owned();
                answer.try_send(Err(ended)).ok();
            }
        }
        answered
    }
}

/// Brings `window` in front and presses Ctrl+V in it, once the keys the
/// user holds are let go, so that what is on the clipboard is pasted
/// there. It waits for both, for a moment, and is therefore to be called
/// on a thread of its own. Windows does not let the keys reach a window of
/// a program that runs elevated when Tessera does not, and says nothing
/// of it.
pub fn paste_into(window: AppWindowId) -> Result<(), String> {
    if !window.exists() {
        return Err("the window to paste into is gone".to_owned());
    }
    window.activate();
    let front = wait_for(FRONT_PATIENCE, || AppWindowId::foreground() == Some(window));
    if !front {
        return Err("the window to paste into did not come in front".to_owned());
    }
    let free = wait_for(KEYS_PATIENCE, || {
        Modifiers::down_now() == Modifiers::default()
    });
    if !free {
        return Err("nothing was pasted, keys were held down".to_owned());
    }
    let keys = [
        key(VK_CONTROL, true),
        key(VK_V, true),
        key(VK_V, false),
        key(VK_CONTROL, false),
    ];
    // SAFETY: the events outlive the call, which copies them.
    let sent = unsafe { SendInput(&keys, size_of::<INPUT>() as i32) };
    match sent as usize == keys.len() {
        true => Ok(()),
        false => Err("Windows refused the keys that paste".to_owned()),
    }
}

/// Whether `done` came true within `patience`, asked again and again.
fn wait_for(patience: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if done() {
            return true;
        }
        if start.elapsed() >= patience {
            return false;
        }
        thread::sleep(PATIENCE_STEP);
    }
}

/// A key going down if `down`, and up otherwise, marked as made here so
/// that the keyboard hook of Tessera leaves it alone.
fn key(key: VIRTUAL_KEY, down: bool) -> INPUT {
    let flags = match down {
        true => KEYBD_EVENT_FLAGS(0),
        false => KEYEVENTF_KEYUP,
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: flags,
                dwExtraInfo: MADE_HERE,
                ..Default::default()
            },
        },
    }
}

/// What the thread of a watch holds, for its window procedure.
struct Watch {
    shared: Arc<Shared>,
    requests: mpsc::Receiver<Request>,
    clips: Sender<Clip>,
}

thread_local! {
    /// The watch of this thread, on the thread of a watch only.
    static WATCH: RefCell<Option<Watch>> = const { RefCell::new(None) };
}

/// The body of the thread of a watch: makes the window, has Windows tell
/// it of changes, and handles messages until the watch is dropped.
fn watch(shared: Arc<Shared>, requests: mpsc::Receiver<Request>, clips: Sender<Clip>) {
    let window = match create_window() {
        Ok(window) => window,
        Err(error) => {
            report!("the clipboard is not watched: {error:#}");
            return;
        }
    };
    WATCH.with(|watch| {
        *watch.borrow_mut() = Some(Watch {
            shared: shared.clone(),
            requests,
            clips,
        });
    });
    // SAFETY: the window was made on this thread and lives until it is
    // destroyed below or by the message that ends the watch.
    if let Err(error) = unsafe { AddClipboardFormatListener(window) } {
        report!("the clipboard is not watched: {error}");
        // SAFETY: as above.
        let _ = unsafe { DestroyWindow(window) };
        return;
    }
    shared.window.store(window.0 as isize, Ordering::SeqCst);
    // A watch dropped before the window existed could not tell it to
    // close.
    if shared.stop.load(Ordering::SeqCst) {
        // SAFETY: as above.
        let _ = unsafe { DestroyWindow(window) };
    }
    answer_requests(window);
    // The window in front now, and each window of another program that
    // comes in front from now on, is noted; Windows tells of them through
    // the message loop below.
    note_front(AppWindowId::foreground());
    let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
    let (event, procedure) = (EVENT_SYSTEM_FOREGROUND, Some(front_changed as _));
    // SAFETY: `front_changed` is a function of this program, which is never
    // unloaded; no module is injected anywhere, and the hook is removed
    // before the thread ends.
    let hook = unsafe { SetWinEventHook(event, event, None, procedure, 0, 0, flags) };
    let mut message = MSG::default();
    // SAFETY: `message` outlives the calls that fill and pass it on; the
    // window procedure is called from inside them.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
    if !hook.is_invalid() {
        // SAFETY: the hook was set on this thread above.
        let _ = unsafe { UnhookWinEvent(hook) };
    }
    WATCH.with(|watch| watch.borrow_mut().take());
}

/// Notes `window` as the one that came in front last, unless it is one of
/// Tessera.
fn note_front(window: Option<AppWindowId>) {
    let Some(window) = window.filter(|window| window.process() != std::process::id()) else {
        return;
    };
    WATCH.with(|watch| {
        if let Ok(watch) = watch.try_borrow()
            && let Some(watch) = watch.as_ref()
        {
            watch.shared.front.store(window.raw(), Ordering::Relaxed);
        }
    });
}

/// What Windows calls when another window comes in front. Nothing may
/// unwind out of it.
unsafe extern "system" fn front_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    window: HWND,
    object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if object != OBJID_WINDOW.0 || window.is_invalid() {
        return;
    }
    let raw = window.0 as isize;
    let noted = catch_unwind(|| note_front(Some(AppWindowId::from_raw(raw))));
    if noted.is_err() {
        report!("the window in front was not noted for the clipboard");
    }
}

/// Registers the class of the windows of watches, once per process; a
/// watch started again after one was dropped uses it again.
fn register_class(module: HMODULE) -> Result<()> {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    let registered = REGISTERED.get_or_init(|| {
        let class = WNDCLASSW {
            lpfnWndProc: Some(receive),
            hInstance: module.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        // SAFETY: the class structure and its strings outlive the call,
        // and `receive` has the signature of a window procedure.
        unsafe { RegisterClassW(&raw const class) != 0 }
    });
    match registered {
        true => Ok(()),
        false => Err(anyhow!("the window class could not be registered")),
    }
}

fn create_window() -> Result<HWND> {
    // SAFETY: the call only reads the handle of the program.
    let module =
        unsafe { GetModuleHandleW(None) }.context("the module handle could not be read")?;
    register_class(module)?;
    // SAFETY: the class is registered and the strings are constants; a
    // message-only window is never shown and gets no broadcasts.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("Tessera clipboard"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(module.into()),
            None,
        )
    }
    .context("the window that watches the clipboard could not be made")
}

/// The window procedure of a watch. Nothing may unwind out of it, as that
/// would end the process: a panic is reported, and the message gets the
/// default handling.
unsafe extern "system" fn receive(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let answered = catch_unwind(AssertUnwindSafe(|| answer(window, message)));
    match answered {
        Ok(true) => return LRESULT(0),
        Ok(false) => {}
        Err(_) => report!("a message to the watch of the clipboard was not handled"),
    }
    // SAFETY: the arguments are passed on as they came.
    unsafe { DefWindowProcW(window, message, w, l) }
}

/// Handles `message` to the window of the watch, and says whether it did.
fn answer(window: HWND, message: u32) -> bool {
    match message {
        WM_CLIPBOARDUPDATE => {
            copied(window);
            true
        }
        REQUESTS_WAITING => {
            answer_requests(window);
            true
        }
        WM_CLOSE => {
            // SAFETY: the window belongs to this thread.
            let _ = unsafe { DestroyWindow(window) };
            true
        }
        WM_DESTROY => {
            // SAFETY: the window belongs to this thread; ending its loop is
            // what the watch was dropped for.
            unsafe {
                let _ = RemoveClipboardFormatListener(window);
                PostQuitMessage(0);
            }
            true
        }
        _ => false,
    }
}

/// Reads what was copied and hands it on, unless the watch put it there
/// itself, the copying program asked for it not to be kept, or it is
/// nothing the history keeps.
fn copied(window: HWND) {
    let images = WATCH.with(|watch| {
        let watch = watch.borrow();
        watch
            .as_ref()
            .map(|watch| watch.shared.images.load(Ordering::Relaxed))
    });
    let Some(images) = images else {
        return;
    };
    let Some(raw) = read(window, images) else {
        return;
    };
    let Some(clip) = raw.into_clip() else {
        return;
    };
    WATCH.with(|watch| {
        if let Some(watch) = watch.borrow().as_ref() {
            watch.clips.try_send(clip).ok();
        }
    });
}

/// Puts the offers that wait on the clipboard, in the order they came.
fn answer_requests(window: HWND) {
    loop {
        let request = WATCH.with(|watch| {
            let watch = watch.try_borrow().ok()?;
            watch.as_ref()?.requests.try_recv().ok()
        });
        let Some((offer, answer)) = request else {
            break;
        };
        answer.try_send(put(window, offer)).ok();
    }
}

/// The clipboard, open for this thread until this is dropped.
struct Open;

impl Open {
    /// Opens the clipboard for `window`, trying again for a moment while
    /// another program holds it.
    fn clipboard(window: HWND) -> Option<Self> {
        for attempt in 0..OPEN_TRIES {
            // SAFETY: the window belongs to this thread; the clipboard is
            // closed again when the guard is dropped.
            if unsafe { OpenClipboard(Some(window)) }.is_ok() {
                return Some(Self);
            }
            if attempt + 1 < OPEN_TRIES {
                thread::sleep(OPEN_PAUSE);
            }
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: the clipboard was opened by this thread.
        let _ = unsafe { CloseClipboard() };
    }
}

/// Puts `offer` on the clipboard as its owner `window`, in place of what
/// is there.
fn put(window: HWND, offer: Offer) -> Result<(), String> {
    let open =
        Open::clipboard(window).ok_or_else(|| "another program holds the clipboard".to_owned())?;
    // SAFETY: the clipboard is open; emptying it makes `window` its owner.
    unsafe { EmptyClipboard() }
        .map_err(|error| format!("the clipboard was not emptied: {error}"))?;
    for (format, bytes) in offer.formats {
        set_data(format.id(), &bytes)
            .map_err(|error| format!("the clipboard did not take what was put on it: {error}"))?;
    }
    drop(open);
    Ok(())
}

/// Hands a copy of `bytes` to the open clipboard as `format`.
fn set_data(format: u32, bytes: &[u8]) -> windows::core::Result<()> {
    // SAFETY: the memory is allocated as large as what is written to it,
    // written while it is locked, and owned by the clipboard once it took
    // it; until then it is freed again on failure.
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let start = GlobalLock(memory).cast::<u8>();
        if start.is_null() {
            let _ = GlobalFree(Some(memory));
            return Err(windows::core::Error::from_win32());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), start, bytes.len());
        let _ = GlobalUnlock(memory);
        if let Err(error) = SetClipboardData(format, Some(HANDLE(memory.0))) {
            let _ = GlobalFree(Some(memory));
            return Err(error);
        }
    }
    Ok(())
}

/// The number of the format programs register as `name`.
fn format_named(name: &str) -> u32 {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: the name is NUL-terminated and outlives the call.
    unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) }
}

/// Whether the clipboard, which is open, holds `format`.
fn holds(format: u32) -> bool {
    // SAFETY: the call only reads the formats of the clipboard.
    unsafe { IsClipboardFormatAvailable(format) }.is_ok()
}

/// What the open clipboard holds as `format`: `None` where it holds none,
/// `Some(None)` where it holds more than `limit` bytes of it.
fn bytes(format: u32, limit: usize) -> Option<Option<Vec<u8>>> {
    if !holds(format) {
        return None;
    }
    // SAFETY: the clipboard is open; the memory it owns is only read,
    // while it is locked, and no further than its size.
    unsafe {
        let memory = HGLOBAL(GetClipboardData(format).ok()?.0);
        let size = GlobalSize(memory);
        if size > limit {
            return Some(None);
        }
        let start = GlobalLock(memory).cast::<u8>();
        if start.is_null() {
            return None;
        }
        let copy = std::slice::from_raw_parts(start, size).to_vec();
        let _ = GlobalUnlock(memory);
        Some(Some(copy))
    }
}

/// Whether the copying program asked that what it copied be left out of
/// a history of the clipboard, as password managers do: with one of the
/// formats that say so, or with `CanIncludeInClipboardHistory` set to 0.
fn excluded() -> bool {
    if [EXCLUDE, VIEWER_IGNORE]
        .into_iter()
        .any(|name| holds(format_named(name)))
    {
        return true;
    }
    match bytes(format_named(IN_HISTORY), 64) {
        Some(Some(value)) => value.get(..4) == Some(&[0, 0, 0, 0][..]),
        _ => false,
    }
}

/// What was copied, as it came off the clipboard, before it is decoded.
#[derive(Debug, PartialEq)]
enum Raw {
    Files(Vec<PathBuf>),
    Text(Vec<u8>),
    Png(Vec<u8>),
    Dib(Vec<u8>),
    Html(Vec<u8>),
    Rtf(Vec<u8>),
}

/// Copies out of the clipboard what the history keeps of it: files first,
/// then a plain text, then a picture (as a PNG where the program gave one,
/// else as a bitmap) if `images`, then HTML and RTF to be read as text.
/// Nothing is copied where the watch put it there itself, the program
/// asked for it not to be kept, the clipboard stays held by another
/// program, or what comes first is larger than is kept.
fn read(window: HWND, images: bool) -> Option<Raw> {
    // SAFETY: the call only reads which window owns the clipboard.
    if unsafe { GetClipboardOwner() }.is_ok_and(|owner| owner == window) {
        return None;
    }
    let _open = Open::clipboard(window)?;
    if excluded() {
        return None;
    }
    if holds(CF_HDROP) {
        return Some(Raw::Files(files()));
    }
    let text_bytes = (TEXT_LIMIT + 1) * size_of::<u16>();
    if let Some(text) = bytes(CF_UNICODETEXT, text_bytes) {
        return text.map(Raw::Text);
    }
    if images {
        if let Some(png) = bytes(format_named(PNG), BYTE_LIMIT) {
            return png.map(Raw::Png);
        }
        // The header of version 5 tells which bits are transparency, which
        // `CF_DIB` loses where Windows makes it of a `CF_DIBV5`.
        for format in [CF_DIBV5, CF_DIB] {
            if let Some(dib) = bytes(format, BYTE_LIMIT) {
                return dib.map(Raw::Dib);
            }
        }
    }
    if let Some(html) = bytes(format_named(HTML), text_bytes) {
        return html.map(Raw::Html);
    }
    bytes(format_named(RTF), text_bytes).flatten().map(Raw::Rtf)
}

/// The files of the open clipboard, which holds `CF_HDROP`.
fn files() -> Vec<PathBuf> {
    // SAFETY: the clipboard is open; the list it owns is only read, into
    // buffers as long as the calls are told.
    unsafe {
        let Ok(handle) = GetClipboardData(CF_HDROP) else {
            return Vec::new();
        };
        let drop = HDROP(handle.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        (0..count)
            .filter_map(|index| {
                let length = DragQueryFileW(drop, index, None) as usize;
                let mut name = vec![0u16; length + 1];
                let written = DragQueryFileW(drop, index, Some(&mut name)) as usize;
                (written > 0).then(|| PathBuf::from(OsString::from_wide(&name[..written])))
            })
            .collect()
    }
}

impl Raw {
    /// What was copied, decoded; `None` for what is empty, unreadable or
    /// larger than is kept.
    fn into_clip(self) -> Option<Clip> {
        let text = |text: String| (!text.trim().is_empty()).then_some(Clip::Text(text));
        match self {
            Self::Files(files) => (!files.is_empty()).then_some(Clip::Files(files)),
            Self::Text(bytes) => text(wide_text(&bytes)),
            Self::Png(bytes) => decode_png(&bytes).map(Clip::Image),
            Self::Dib(bytes) => decode_dib(&bytes).map(Clip::Image),
            Self::Html(bytes) => text(html_text(html_fragment(&bytes)?)),
            Self::Rtf(bytes) => text(rtf_text(&String::from_utf8_lossy(&bytes))),
        }
    }
}

/// The text of NUL-terminated UTF-16 in `bytes`, little-endian.
fn wide_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// `text` as NUL-terminated UTF-16, little-endian.
fn wide_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// A picture of a PNG file, unless it has more than [`PIXEL_LIMIT`]
/// pixels or cannot be read.
fn decode_png(bytes: &[u8]) -> Option<RgbaImage> {
    let decoder = PngDecoder::new(Cursor::new(bytes)).ok()?;
    decode(decoder)
}

/// A picture of a device-independent bitmap as `CF_DIBV5` or `CF_DIB`
/// holds one: a bitmap file without its file header.
fn decode_dib(bytes: &[u8]) -> Option<RgbaImage> {
    let decoder = BmpDecoder::new_without_file_header(Cursor::new(bytes)).ok()?;
    decode(decoder)
}

/// The picture `decoder` reads, unless it is too large. A picture whose
/// every pixel is fully transparent is taken as opaque: programs write a
/// zero where a bitmap has no transparency.
fn decode(decoder: impl ImageDecoder) -> Option<RgbaImage> {
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > PIXEL_LIMIT {
        return None;
    }
    let mut pixels = DynamicImage::from_decoder(decoder).ok()?.into_rgba8();
    if pixels.pixels().all(|pixel| pixel[3] == 0) {
        pixels.pixels_mut().for_each(|pixel| pixel[3] = u8::MAX);
    }
    Some(pixels)
}

/// The bytes of `CF_DIB` for `pixels`: a `BITMAPINFOHEADER` and the rows
/// from the bottom up, four bytes a pixel in the order blue, green, red
/// and alpha.
fn dib_of(pixels: &RgbaImage) -> Vec<u8> {
    const HEADER: u32 = 40;
    let (width, height) = pixels.dimensions();
    let size = width as usize * height as usize * 4;
    let mut bytes = Vec::with_capacity(HEADER as usize + size);
    bytes.extend(HEADER.to_le_bytes());
    bytes.extend((width as i32).to_le_bytes());
    // A positive height has the rows from the bottom up.
    bytes.extend((height as i32).to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(32u16.to_le_bytes());
    // BI_RGB, then the size of the pixels, the resolutions and the colors
    // of a palette, which it has none of.
    bytes.extend(0u32.to_le_bytes());
    bytes.extend((size as u32).to_le_bytes());
    bytes.extend([0u8; 16]);
    for row in pixels.rows().rev() {
        for pixel in row {
            let [red, green, blue, alpha] = pixel.0;
            bytes.extend([blue, green, red, alpha]);
        }
    }
    bytes
}

/// The bytes of `CF_HDROP` for `paths`: a `DROPFILES` header saying the
/// names are wide, and the names, each ending in a NUL, with one more NUL
/// after the last.
fn drop_files(paths: &[PathBuf]) -> Vec<u8> {
    const HEADER: u32 = 20;
    let mut bytes = Vec::new();
    // Where the names begin, the point of a drop (none), whether it is
    // in the client area, and that the names are wide.
    bytes.extend(HEADER.to_le_bytes());
    bytes.extend([0u8; 8]);
    bytes.extend(0u32.to_le_bytes());
    bytes.extend(1u32.to_le_bytes());
    for path in paths {
        let name = path.as_os_str().encode_wide().chain([0]);
        bytes.extend(name.flat_map(u16::to_le_bytes));
    }
    bytes.extend(0u16.to_le_bytes());
    bytes
}

/// The fragment that the `HTML Format` of the clipboard marks as what was
/// copied: the bytes between the offsets `StartFragment` and `EndFragment`
/// of its header, or the whole of `<body>` where those are missing.
fn html_fragment(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?.trim_end_matches('\0');
    let offset = |key: &str| -> Option<usize> {
        let at = text.find(key)? + key.len();
        let digits: String = text[at..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    };
    let fragment = match (offset("StartFragment:"), offset("EndFragment:")) {
        (Some(start), Some(end)) => text.get(start..end),
        _ => None,
    };
    fragment.or_else(|| Some(&text[text.find("<body").unwrap_or(0)..]))
}

/// What a fragment of HTML reads as: the text without its tags, with a
/// line break for each block that ends, the entities that are written
/// most decoded, and white space run together as a browser shows it.
fn html_text(html: &str) -> String {
    const BLOCKS: [&str; 14] = [
        "p",
        "div",
        "br",
        "li",
        "tr",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "pre",
        "table",
        "blockquote",
    ];
    let mut text = String::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        push_words(&mut text, &decode_entities(&rest[..open]));
        rest = &rest[open..];
        let end = rest.find('>');
        let tag = rest[1..end.unwrap_or(rest.len())].trim_start_matches('/');
        let close = end.map_or(rest.len(), |at| at + 1);
        let name: String = tag
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        // What scripts and styles hold is not read.
        if name == "script" || name == "style" {
            let end = format!("</{name}");
            let after = rest.to_ascii_lowercase().find(&end).unwrap_or(rest.len());
            rest = &rest[after..];
            let close = rest.find('>').map_or(rest.len(), |at| at + 1);
            rest = &rest[close..];
            continue;
        }
        if BLOCKS.contains(&name.as_str()) && !text.is_empty() && !text.ends_with('\n') {
            text.truncate(text.trim_end_matches(' ').len());
            text.push('\n');
        }
        rest = &rest[close..];
    }
    push_words(&mut text, &decode_entities(rest));
    text.trim().to_owned()
}

/// Adds `words` to `text`, every run of white space as one space and none
/// at the start of a line.
fn push_words(text: &mut String, words: &str) {
    for character in words.chars() {
        if character.is_whitespace() {
            if !text.is_empty() && !text.ends_with([' ', '\n']) {
                text.push(' ');
            }
        } else {
            text.push(character);
        }
    }
}

/// `text` with the entities of HTML that are written most, and those by
/// number, as the characters they stand for.
fn decode_entities(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        decoded.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest.find(';').filter(|&end| end <= 10);
        let entity = end.map(|end| &rest[1..end]);
        let character = entity.and_then(|entity| match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => {
                let number = entity.strip_prefix('#')?;
                let code = match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => number.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (character, end) {
            (Some(character), Some(end)) => {
                decoded.push(character);
                rest = &rest[end + 1..];
            }
            _ => {
                decoded.push('&');
                rest = &rest[1..];
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

/// The groups of RTF that hold no text of the document.
const RTF_SKIPPED: [&str; 9] = [
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "header",
    "footer",
    "listtable",
    "listoverridetable",
];

/// What a document in RTF reads as: its text without the control words,
/// with `\par` and `\line` as line breaks, `\tab` as a tab, `\'hh` as a
/// character of Windows-1252 (read as Latin-1, which differs only in a
/// few signs) and `\uN` as the character N, of which the next `\ucN`
/// characters are a stand-in that is left out. Groups that hold no text,
/// such as the table of fonts, pictures and those marked with `\*`, are
/// left out.
fn rtf_text(rtf: &str) -> String {
    let mut text = String::new();
    let characters: Vec<char> = rtf.chars().collect();
    // For each open group, whether it is left out and how many characters
    // stand in for a `\u`.
    let mut groups: Vec<(bool, usize)> = vec![(false, 1)];
    let mut skip_after_unicode = 0usize;
    let mut index = 0;
    let skipping = |groups: &[(bool, usize)]| groups.last().is_some_and(|group| group.0);
    while index < characters.len() {
        let character = characters[index];
        index += 1;
        match character {
            '{' => {
                let (skipped, stand_in) = groups.last().copied().unwrap_or((false, 1));
                groups.push((skipped, stand_in));
            }
            '}' => {
                if groups.len() > 1 {
                    groups.pop();
                }
            }
            '\\' => {
                let Some(&next) = characters.get(index) else {
                    break;
                };
                if !next.is_ascii_alphabetic() {
                    index += 1;
                    let literal = match next {
                        '\\' | '{' | '}' => Some(next),
                        '~' => Some('\u{a0}'),
                        '*' => {
                            if let Some(group) = groups.last_mut() {
                                group.0 = true;
                            }
                            None
                        }
                        '\'' => {
                            let hex: String = characters[index..].iter().take(2).collect();
                            index += hex.len();
                            u8::from_str_radix(&hex, 16).ok().map(char::from)
                        }
                        _ => None,
                    };
                    if let Some(literal) = literal
                        && !skipping(&groups)
                    {
                        if skip_after_unicode > 0 {
                            skip_after_unicode -= 1;
                        } else {
                            text.push(literal);
                        }
                    }
                    continue;
                }
                let word_start = index;
                while characters.get(index).is_some_and(char::is_ascii_alphabetic) {
                    index += 1;
                }
                let word: String = characters[word_start..index].iter().collect();
                let number_start = index;
                if characters.get(index) == Some(&'-') {
                    index += 1;
                }
                while characters.get(index).is_some_and(char::is_ascii_digit) {
                    index += 1;
                }
                let number: Option<i64> = characters[number_start..index]
                    .iter()
                    .collect::<String>()
                    .parse()
                    .ok();
                // A space ends a control word and is part of it.
                if characters.get(index) == Some(&' ') {
                    index += 1;
                }
                if RTF_SKIPPED.contains(&word.as_str()) {
                    if let Some(group) = groups.last_mut() {
                        group.0 = true;
                    }
                    continue;
                }
                if skipping(&groups) {
                    continue;
                }
                match (word.as_str(), number) {
                    ("par" | "line", _) => text.push('\n'),
                    ("tab", _) => text.push('\t'),
                    ("uc", Some(count)) => {
                        if let Some(group) = groups.last_mut() {
                            group.1 = usize::try_from(count).unwrap_or(1);
                        }
                    }
                    ("u", Some(code)) => {
                        // Code points above 32767 are written negative.
                        let code = if code < 0 { code + 65536 } else { code };
                        if let Some(character) = u32::try_from(code).ok().and_then(char::from_u32) {
                            text.push(character);
                        }
                        skip_after_unicode = groups.last().map_or(1, |group| group.1);
                    }
                    _ => {}
                }
            }
            '\r' | '\n' => {}
            _ => {
                if skipping(&groups) {
                    continue;
                }
                if skip_after_unicode > 0 {
                    skip_after_unicode -= 1;
                } else {
                    text.push(character);
                }
            }
        }
    }
    text.trim().to_owned()
}

/// The window a number names that was a handle.
fn handle(window: isize) -> HWND {
    HWND(window as *mut c_void)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_goes_to_utf16_and_back() {
        let text = "Grüße, мир 🌍";
        let bytes = wide_bytes(text);
        assert_eq!(&bytes[bytes.len() - 2..], [0, 0]);
        assert_eq!(wide_text(&bytes), text);
        // What follows the NUL is not read.
        let mut more = bytes.clone();
        more.extend([b'x', 0]);
        assert_eq!(wide_text(&more), text);
    }

    #[test]
    fn a_picture_is_put_as_a_bitmap_that_reads_the_same() {
        let mut pixels = RgbaImage::new(3, 2);
        for (x, y, pixel) in pixels.enumerate_pixels_mut() {
            *pixel = image::Rgba([x as u8 * 80, y as u8 * 120, 7, 255]);
        }
        let dib = dib_of(&pixels);
        assert_eq!(dib.len(), 40 + 3 * 2 * 4);
        // The first row of the bitmap is the bottom row of the picture.
        assert_eq!(&dib[40..44], [7, 120, 0, 255]);
        assert_eq!(decode_dib(&dib), Some(pixels));
    }

    #[test]
    fn a_bitmap_without_transparency_is_opaque() {
        let pixels = RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 0]));
        let decoded = decode_dib(&dib_of(&pixels)).unwrap();
        assert!(decoded.pixels().all(|pixel| pixel.0 == [10, 20, 30, 255]));
    }

    #[test]
    fn a_png_is_read_with_its_transparency() {
        let pixels = RgbaImage::from_pixel(4, 3, image::Rgba([1, 2, 3, 128]));
        let mut png = Vec::new();
        DynamicImage::ImageRgba8(pixels.clone())
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(decode_png(&png), Some(pixels));
        assert_eq!(decode_png(b"no picture"), None);
    }

    #[test]
    fn files_are_listed_after_a_header_and_end_with_two_nuls() {
        let bytes = drop_files(&[PathBuf::from("C:\\a"), PathBuf::from("b")]);
        assert_eq!(&bytes[..4], 20u32.to_le_bytes());
        assert_eq!(&bytes[16..20], 1u32.to_le_bytes());
        let names: Vec<u16> = bytes[20..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let expected: Vec<u16> = "C:\\a\0b\0\0".encode_utf16().collect();
        assert_eq!(names, expected);
    }

    #[test]
    fn the_fragment_of_html_is_found_by_its_offsets() {
        let body = "<html><body><!--StartFragment--><b>Bold</b><!--EndFragment--></body></html>";
        let header_length = 105;
        let start = header_length + body.find("<b>").unwrap();
        let end = header_length + body.find("<!--EndFragment").unwrap();
        let header = format!(
            "Version:0.9\r\nStartHTML:{:010}\r\nEndHTML:{:010}\r\nStartFragment:{start:010}\r\nEndFragment:{end:010}\r\n",
            header_length,
            header_length + body.len()
        );
        assert_eq!(header.len(), header_length);
        let clip = format!("{header}{body}");
        assert_eq!(html_fragment(clip.as_bytes()), Some("<b>Bold</b>"));
        assert_eq!(
            html_fragment(b"<html><body>Plain</body></html>"),
            Some("<body>Plain</body></html>")
        );
    }

    #[test]
    fn html_reads_as_its_text() {
        let html = "<p>One &amp; two</p><p>three&nbsp;<b>four</b>\n  five</p>\
                    <style>p { color: red }</style><ul><li>a&#x41;</li><li>&#66;</li></ul>&bogus";
        assert_eq!(html_text(html), "One & two\nthree four five\naA\nB\n&bogus");
        assert_eq!(html_text("<br><br>x"), "x");
    }

    #[test]
    fn rtf_reads_as_its_text() {
        let rtf = concat!(
            r"{\rtf1\ansi{\fonttbl{\f0 Calibri;}}{\colortbl;\red0\green0\blue0;}",
            "\r\n",
            r"{\*\generator Riched20;}\f0 Hello\tab world\par",
            "\r\n",
            r"caf\'e9 \u1087?\u1088?\uc2\u1080?? end {\b bold}\}}"
        );
        assert_eq!(rtf_text(rtf), "Hello\tworld\ncafé при end bold}");
    }

    #[test]
    fn what_is_empty_is_not_kept() {
        assert_eq!(Raw::Text(wide_bytes("  \n")).into_clip(), None);
        assert_eq!(Raw::Files(Vec::new()).into_clip(), None);
        assert_eq!(
            Raw::Text(wide_bytes("kept")).into_clip(),
            Some(Clip::Text("kept".to_owned()))
        );
        assert_eq!(Raw::Rtf(b"{\\rtf1 }".to_vec()).into_clip(), None);
    }
}
