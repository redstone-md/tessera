// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One thread of the process for the work the Shell of Windows wants done
//! in a single-threaded COM apartment with a message pump: starting files
//! with `ShellExecuteEx`, context menus (`IContextMenu`, `TrackPopupMenuEx`),
//! drag and drop, file operations with their dialogs, preview handlers.
//!
//! The thread of GPUI must not do that work: the Shell pumps window
//! messages while it waits, which GPUI cannot take in the middle of an
//! event, and some calls wait for seconds. The shell thread initializes
//! OLE (and with it COM, apartment-threaded), owns a hidden window that
//! menus and dialogs can name as their owner, and runs the work handed to
//! it in order. Work arrives as a message to that window, so work handed
//! over while earlier work waits in a menu or a dialog with a message loop
//! of its own is done in the meantime, from inside that loop. Work that
//! keeps a dialog open for long, such as a file operation with its
//! progress, therefore holds nothing up, but it is better given a thread
//! of its own, initialized the same way.
//!
//! ```ignore
//! // From the thread of GPUI: hand the work over, await the answer.
//! let name = ShellThread::call(move || display_name_of(&path));
//! cx.spawn(async move |cx| { let name = name.recv().await; ... }).detach();
//! ```

use std::cell::RefCell;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{OnceLock, mpsc};
use std::thread;

use anyhow::{Context as _, Result, anyhow};
use async_channel::Receiver;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, PostMessageW,
    RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_POPUP,
};
use windows::core::{PCWSTR, w};

/// The class of the hidden window of the shell thread.
const CLASS: PCWSTR = w!("Tessera.ShellThread");

/// The message that tells the window that work is waiting.
const WORK_WAITING: u32 = WM_APP + 0x51;

/// A piece of work for the shell thread.
type Work = Box<dyn FnOnce() + Send>;

/// A handler of the messages of the hidden window; see
/// [`ShellThread::intercept`].
type Handler = Box<dyn FnMut(u32, WPARAM, LPARAM) -> Option<LRESULT>>;

/// The running shell thread: where work is queued and the window that is
/// told of it.
struct Worker {
    queue: mpsc::Sender<Work>,
    /// The hidden window, as a number, since a handle cannot be shared
    /// between threads.
    window: isize,
}

/// The thread, once started; why it could not start otherwise.
static WORKER: OnceLock<Result<Worker, String>> = OnceLock::new();

/// The number of the shell thread, or zero before it runs.
static THREAD: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// The work queued for this thread, on the shell thread only.
    static WAITING: RefCell<Option<mpsc::Receiver<Work>>> = const { RefCell::new(None) };
    /// The handlers of messages to the hidden window, with their numbers.
    static HANDLERS: RefCell<Vec<(u64, Handler)>> = const { RefCell::new(Vec::new()) };
}

/// The process-wide thread for the Shell's apartment-threaded work.
pub struct ShellThread;

impl ShellThread {
    /// Has `work` done on the shell thread, after what was handed over
    /// before. It returns at once. A panic in `work` is reported and the
    /// thread goes on.
    ///
    /// Where the thread could not be started, which is reported once, the
    /// work is done on a new thread with OLE initialized instead, without
    /// the hidden window.
    pub fn run(work: impl FnOnce() + Send + 'static) {
        let work: Work = Box::new(work);
        let worker = match WORKER.get_or_init(Worker::start) {
            Ok(worker) => worker,
            Err(_) => return Self::run_elsewhere(work),
        };
        if worker.queue.send(work).is_err() {
            return report!("the shell thread takes no more work");
        }
        // SAFETY: posting to a window that has been destroyed fails without
        // side effects.
        let posted = unsafe {
            PostMessageW(
                Some(handle(worker.window)),
                WORK_WAITING,
                WPARAM(0),
                LPARAM(0),
            )
        };
        if let Err(error) = posted {
            report!("the shell thread was not told of new work: {error}");
        }
    }

    /// Has `work` done on the shell thread, as [`Self::run`] does, and
    /// gives its answer: the receiver yields it once, and is closed
    /// without an answer if `work` panicked. Await it from a task of GPUI
    /// with `recv().await`; never block the thread of GPUI on it.
    #[allow(dead_code, reason = "for the packages that ask the Shell for answers")]
    pub fn call<R: Send + 'static>(work: impl FnOnce() -> R + Send + 'static) -> Receiver<R> {
        let (answer, answered) = async_channel::bounded(1);
        Self::run(move || {
            answer.try_send(work()).ok();
        });
        answered
    }

    /// Whether this is the shell thread.
    pub fn is_current() -> bool {
        // SAFETY: the call only reads which thread this is.
        let current = unsafe { GetCurrentThreadId() };
        THREAD.load(Ordering::Relaxed) == current
    }

    /// The hidden window of the shell thread, to name as the owner of a
    /// menu (`TrackPopupMenuEx`), of a dialog or of an operation of the
    /// Shell done on that thread. It is never shown. `None` where the
    /// thread could not start.
    #[allow(dead_code, reason = "for menus and dialogs of the Shell")]
    pub(super) fn owner() -> Option<HWND> {
        let worker = WORKER.get_or_init(Worker::start).as_ref().ok()?;
        Some(handle(worker.window))
    }

    /// Has `handler` see every message to the hidden window before the
    /// default handling, until the returned [`Interception`] is dropped;
    /// an answer of `Some` is what the window answers. This is how a
    /// context menu of the Shell gets the messages it asks for
    /// (`IContextMenu3::HandleMenuMsg2` for `WM_INITMENUPOPUP`,
    /// `WM_DRAWITEM`, `WM_MEASUREITEM` and `WM_MENUCHAR`) while
    /// `TrackPopupMenuEx` runs. It is to be called on the shell thread,
    /// and the interception dropped there too; elsewhere it does nothing.
    #[allow(dead_code, reason = "for the context menus of the Shell")]
    pub(super) fn intercept(
        handler: impl FnMut(u32, WPARAM, LPARAM) -> Option<LRESULT> + 'static,
    ) -> Interception {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        if Self::is_current() {
            HANDLERS.with(|handlers| match handlers.try_borrow_mut() {
                Ok(mut handlers) => handlers.push((number, Box::new(handler))),
                Err(_) => report!("a handler of the shell thread was added while one ran"),
            });
        } else {
            report!("a handler of the shell thread was added on another thread");
        }
        Interception(number)
    }

    /// Does `work` on a thread of its own with OLE initialized, for when
    /// the shell thread could not be started.
    fn run_elsewhere(work: Work) {
        let started = thread::Builder::new()
            .name("shell work".to_owned())
            .spawn(move || {
                if let Err(error) = enter_apartment() {
                    report!("{error}, for the work of the Shell");
                }
                do_work(work);
            });
        if let Err(error) = started {
            report!("no thread starts for the work of the Shell: {error}");
        }
    }
}

/// While it exists, a handler sees the messages of the hidden window; see
/// [`ShellThread::intercept`].
#[must_use = "the handler is removed when this is dropped"]
#[allow(dead_code, reason = "for the context menus of the Shell")]
pub(super) struct Interception(u64);

impl Drop for Interception {
    fn drop(&mut self) {
        let number = self.0;
        HANDLERS.with(|handlers| {
            if let Ok(mut handlers) = handlers.try_borrow_mut() {
                handlers.retain(|(other, _)| *other != number);
            }
        });
    }
}

impl Worker {
    /// Starts the thread and waits until its window exists. A failure is
    /// reported here, once.
    fn start() -> Result<Self, String> {
        let started = Self::try_start();
        if let Err(error) = &started {
            report!(
                "the shell thread did not start, work of the Shell gets threads of its own: {error:#}"
            );
        }
        started.map_err(|error| format!("{error:#}"))
    }

    fn try_start() -> Result<Self> {
        let (queue, waiting) = mpsc::channel::<Work>();
        let (ready, started) = mpsc::channel::<Result<isize, String>>();
        thread::Builder::new()
            .name("shell".to_owned())
            .spawn(move || pump(waiting, &ready))
            .context("the thread could not be made")?;
        let window = started
            .recv()
            .map_err(|_| anyhow!("the thread ended before it was ready"))?
            .map_err(|error| anyhow!(error))?;
        Ok(Self { queue, window })
    }
}

/// The window a number names that was a handle.
fn handle(window: isize) -> HWND {
    HWND(window as *mut c_void)
}

/// Does `work`, reporting a panic instead of ending the thread with it.
fn do_work(work: Work) {
    if catch_unwind(AssertUnwindSafe(work)).is_err() {
        report!("work done for the Shell failed");
    }
}

/// Initializes COM, apartment-threaded and without the old DDE that the
/// Shell does not need when it starts files, and OLE for this thread, for
/// as long as it lives.
fn enter_apartment() -> Result<(), String> {
    // SAFETY: COM and OLE are initialized once for the thread and never
    // uninitialized while it uses them; COM initialized first makes OLE
    // keep its flags.
    unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        com.ok()
            .map_err(|error| format!("COM was not initialized: {error}"))?;
        OleInitialize(None).map_err(|error| format!("OLE was not initialized: {error}"))
    }
}

/// The body of the shell thread: initializes OLE, makes the hidden
/// window, tells `ready` of it, and handles messages for as long as the
/// process lives.
fn pump(waiting: mpsc::Receiver<Work>, ready: &mpsc::Sender<Result<isize, String>>) {
    if let Err(error) = enter_apartment() {
        ready.send(Err(error)).ok();
        return;
    }
    let window = match create_window() {
        Ok(window) => window,
        Err(error) => {
            ready.send(Err(format!("{error:#}"))).ok();
            return;
        }
    };
    WAITING.with(|queue| *queue.borrow_mut() = Some(waiting));
    // SAFETY: the call only reads which thread this is.
    THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    ready.send(Ok(window.0 as isize)).ok();
    let mut message = MSG::default();
    // SAFETY: `message` outlives the calls that fill and pass it on; the
    // window procedure is called from inside them.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

fn create_window() -> Result<HWND> {
    // SAFETY: the class structure and the strings outlive the calls, and
    // `receive` has the signature of a window procedure.
    unsafe {
        let module = GetModuleHandleW(None).context("the module handle could not be read")?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(receive),
            hInstance: module.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassW(&raw const class) == 0 {
            return Err(anyhow!("the window class could not be registered"));
        }
        // A top-level window, never shown: menus need an owner that can
        // come in front, which a message-only window cannot.
        CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
            CLASS,
            w!("Tessera"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(module.into()),
            None,
        )
        .context("the hidden window could not be made")
    }
}

/// Does the work that is waiting, in the order it was handed over.
fn work_waiting() {
    loop {
        let work = WAITING.with(|queue| {
            let queue = queue.try_borrow().ok()?;
            queue.as_ref()?.try_recv().ok()
        });
        match work {
            Some(work) => do_work(work),
            None => break,
        }
    }
}

/// The window procedure of the hidden window.
unsafe extern "system" fn receive(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WORK_WAITING {
        work_waiting();
        return LRESULT(0);
    }
    let handled = HANDLERS.with(|handlers| {
        let mut handlers = handlers.try_borrow_mut().ok()?;
        handlers
            .iter_mut()
            .find_map(|(_, handler)| handler(message, w, l))
    });
    if let Some(answer) = handled {
        return answer;
    }
    // SAFETY: the arguments are passed on as they came.
    unsafe { DefWindowProcW(window, message, w, l) }
}
