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
//! of its own, initialized the same way, with [`ShellThread::run_alone`];
//! so is work that may wait without handling messages, such as a start
//! that waits on the network.
//!
//! The thread is made the first time work is handed over, and that call
//! returns at once: the work waits until the thread has initialized OLE
//! and made its window.
//!
//! ```ignore
//! // From the thread of GPUI: hand the work over, await the answer.
//! let name = ShellThread::call(move || display_name_of(&path));
//! cx.spawn(async move |cx| { let name = name.recv().await; ... }).detach();
//! ```

use std::cell::RefCell;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, AtomicU32, AtomicU64, Ordering};
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
/// [`ShellThread::intercept`]. It is shared so that it can be called
/// while the list of handlers is changed, and borrowed while it runs.
type Handler = Rc<RefCell<dyn FnMut(u32, WPARAM, LPARAM) -> Option<LRESULT>>>;

/// The shell thread: where work is queued.
struct Worker {
    queue: mpsc::Sender<Work>,
}

/// The thread, once made; why it could not be made otherwise. It is made
/// the first time work is handed over, without waiting for it to be
/// ready: the work waits in the queue until it is.
static WORKER: OnceLock<Result<Worker, String>> = OnceLock::new();

/// The hidden window of the shell thread, as a number since a handle
/// cannot be shared between threads; zero until the thread has made it,
/// and for good where it could not.
static WINDOW: AtomicIsize = AtomicIsize::new(0);

/// The number of the shell thread, or zero before it runs.
static THREAD: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// The work queued for this thread, on the shell thread only.
    static WAITING: RefCell<Option<mpsc::Receiver<Work>>> = const { RefCell::new(None) };
    /// The handlers of messages to the hidden window, with their numbers,
    /// the oldest first.
    static HANDLERS: RefCell<Vec<(u64, Handler)>> = const { RefCell::new(Vec::new()) };
}

/// The process-wide thread for the Shell's apartment-threaded work.
pub struct ShellThread;

impl ShellThread {
    /// Has `work` done on the shell thread, after what was handed over
    /// before. It returns at once, also the first time, while the thread
    /// is still starting. A panic in `work` is reported and the thread
    /// goes on.
    ///
    /// Where the thread could not be started, which is reported once, the
    /// work is done on a new thread with OLE initialized instead, without
    /// the hidden window.
    pub fn run(work: impl FnOnce() + Send + 'static) {
        let work: Work = Box::new(work);
        let worker = match WORKER.get_or_init(Worker::start) {
            Ok(worker) => worker,
            Err(_) => return Self::run_alone("shell work", work),
        };
        // The thread holds the queue for as long as the process lives, so
        // this fails only if it ended with a panic.
        if let Err(mpsc::SendError(work)) = worker.queue.send(work) {
            return Self::run_alone("shell work", work);
        }
        // Before the window exists the thread is still starting, and does
        // the waiting work once it has made it.
        let window = WINDOW.load(Ordering::SeqCst);
        if window == 0 {
            return;
        }
        // SAFETY: posting to a window that has been destroyed fails without
        // side effects.
        let posted =
            unsafe { PostMessageW(Some(handle(window)), WORK_WAITING, WPARAM(0), LPARAM(0)) };
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
    /// thread could not start, or has not yet.
    /// Asked for on the shell thread itself, it is there once the work
    /// runs.
    pub(super) fn owner() -> Option<HWND> {
        let window = WINDOW.load(Ordering::SeqCst);
        (window != 0).then(|| handle(window))
    }

    /// Has `handler` see every message to the hidden window before the
    /// default handling, until the returned [`Interception`] is dropped;
    /// an answer of `Some` is what the window answers. This is how a
    /// context menu of the Shell gets the messages it asks for
    /// (`IContextMenu3::HandleMenuMsg2` for `WM_INITMENUPOPUP`,
    /// `WM_DRAWITEM`, `WM_MEASUREITEM` and `WM_MENUCHAR`) while
    /// `TrackPopupMenuEx` runs.
    ///
    /// The newest handler is asked first, so the menu of work done inside
    /// the menu loop of earlier work gets its own messages. A handler that
    /// is running is not asked again for a message that arrives while it
    /// runs; the others are. A handler that panics is reported and removed.
    /// It is to be called on the shell thread, and the interception dropped
    /// there too; elsewhere it does nothing.
    #[allow(dead_code, reason = "for the context menus of the Shell")]
    pub(super) fn intercept(
        handler: impl FnMut(u32, WPARAM, LPARAM) -> Option<LRESULT> + 'static,
    ) -> Interception {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        if Self::is_current() {
            let handler: Handler = Rc::new(RefCell::new(handler));
            HANDLERS.with(|handlers| handlers.borrow_mut().push((number, handler)));
        } else {
            report!("a handler of the shell thread was added on another thread");
        }
        Interception(number)
    }

    /// Does `work` on a new thread called `name`, with COM initialized
    /// apartment-threaded and OLE, as on the shell thread but without its
    /// hidden window, and returns at once. This is for work that may keep
    /// the thread for long without handling messages, which would hold up
    /// the work after it on the shell thread: a file operation with its
    /// progress, a pool of thumbnails, a start that waits on the network
    /// or for the consent of the user. A panic in `work` is reported.
    pub fn run_alone(name: &str, work: impl FnOnce() + Send + 'static) {
        let work: Work = Box::new(work);
        let started = thread::Builder::new().name(name.to_owned()).spawn(move || {
            if let Err(error) = enter_apartment() {
                report!("{error}, for the work of the Shell");
            }
            do_work(work);
        });
        if let Err(error) = started {
            report!("no thread starts for the work of the Shell: {error}");
        }
    }

    /// Has `work` done as [`Self::run_alone`] does, and gives its answer
    /// as [`Self::call`] does.
    #[allow(dead_code, reason = "for the packages that ask the Shell for answers")]
    pub fn call_alone<R: Send + 'static>(
        name: &str,
        work: impl FnOnce() -> R + Send + 'static,
    ) -> Receiver<R> {
        let (answer, answered) = async_channel::bounded(1);
        Self::run_alone(name, move || {
            answer.try_send(work()).ok();
        });
        answered
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
        remove_handler(number);
    }
}

/// Removes the handler with the number `number`, on this thread.
fn remove_handler(number: u64) {
    HANDLERS.with(|handlers| {
        // The list is borrowed only for as long as it is changed or
        // copied, never while a handler runs.
        if let Ok(mut handlers) = handlers.try_borrow_mut() {
            handlers.retain(|(other, _)| *other != number);
        }
    });
}

impl Worker {
    /// Makes the thread and returns at once; the thread gets ready on its
    /// own. A failure to make it is reported here, once.
    fn start() -> Result<Self, String> {
        let (queue, waiting) = mpsc::channel::<Work>();
        let made = thread::Builder::new()
            .name("shell".to_owned())
            .spawn(move || pump(waiting));
        match made {
            Ok(_) => Ok(Self { queue }),
            Err(error) => {
                report!(
                    "the shell thread was not made, work of the Shell gets threads of its own: {error}"
                );
                Err(error.to_string())
            }
        }
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
/// window, does the work that waited meanwhile, and handles messages for
/// as long as the process lives. Where it cannot get ready, it hands each
/// piece of work to a thread of its own instead, the waiting ones first.
fn pump(waiting: mpsc::Receiver<Work>) {
    let window =
        enter_apartment().and_then(|()| create_window().map_err(|error| format!("{error:#}")));
    let window = match window {
        Ok(window) => window,
        Err(error) => {
            report!(
                "the shell thread did not start, work of the Shell gets threads of its own: {error}"
            );
            for work in waiting {
                ShellThread::run_alone("shell work", work);
            }
            return;
        }
    };
    WAITING.with(|queue| *queue.borrow_mut() = Some(waiting));
    // SAFETY: the call only reads which thread this is.
    THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    // Work handed over from now on is told of with a message; the work
    // handed over before is done here.
    WINDOW.store(window.0 as isize, Ordering::SeqCst);
    work_waiting();
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

/// The window procedure of the hidden window. Nothing may unwind out of
/// it, as that would end the process: a panic is reported, and the
/// message gets the default handling.
unsafe extern "system" fn receive(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match catch_unwind(|| answer(message, w, l)) {
        Ok(Some(answer)) => return answer,
        Ok(None) => {}
        Err(_) => report!("a message to the shell thread was not handled"),
    }
    // SAFETY: the arguments are passed on as they came.
    unsafe { DefWindowProcW(window, message, w, l) }
}

/// What the hidden window answers to `message`, if anything other than
/// the default: work that is waiting is done, and the handlers are asked,
/// the newest first.
fn answer(message: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    if message == WORK_WAITING {
        work_waiting();
        return Some(LRESULT(0));
    }
    // A copy, so that a handler can add or remove handlers, and messages
    // that arrive while it runs reach the others.
    let handlers: Vec<(u64, Handler)> =
        HANDLERS.with(|handlers| handlers.borrow().iter().rev().cloned().collect());
    for (number, handler) in handlers {
        let Ok(mut handler) = handler.try_borrow_mut() else {
            continue;
        };
        match catch_unwind(AssertUnwindSafe(|| (*handler)(message, w, l))) {
            Ok(Some(answer)) => return Some(answer),
            Ok(None) => {}
            Err(_) => {
                drop(handler);
                report!("a handler of the shell thread failed and was removed");
                remove_handler(number);
            }
        }
    }
    None
}
