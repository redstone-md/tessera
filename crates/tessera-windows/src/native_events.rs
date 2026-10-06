// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Hooks and their blocking message loop live on the same owned thread.
//! Out-of-context callbacks never inject code and only notify the caller;
//! UI-side coalescing decides when to collect a fresh desktop snapshot.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};

use windows_sys::Win32::Foundation::{GetLastError, HWND};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, DispatchMessageW, EVENT_OBJECT_CREATE, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE,
    EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_SHOW,
    EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, GetMessageW,
    MSG, OBJID_WINDOW, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, TranslateMessage,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_QUIT,
};

use crate::apps::ApplicationError;
use crate::events::DesktopEventCallback;

thread_local! {
    static CALLBACK: RefCell<Option<DesktopEventCallback>> = const { RefCell::new(None) };
}

const EVENTS: [u32; 9] = [
    EVENT_SYSTEM_FOREGROUND,
    EVENT_OBJECT_CREATE,
    EVENT_OBJECT_DESTROY,
    EVENT_OBJECT_SHOW,
    EVENT_OBJECT_HIDE,
    EVENT_SYSTEM_MINIMIZESTART,
    EVENT_SYSTEM_MINIMIZEEND,
    EVENT_OBJECT_NAMECHANGE,
    EVENT_OBJECT_LOCATIONCHANGE,
];

struct Hook(HWINEVENTHOOK);

impl Drop for Hook {
    fn drop(&mut self) {
        // SAFETY: each hook is owned and released on its registration thread.
        unsafe { UnhookWinEvent(self.0) };
    }
}

pub(crate) struct NativeWatcher {
    thread_id: u32,
    worker: Option<JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
}

impl Drop for NativeWatcher {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        // SAFETY: startup publishes the id only after this thread's queue exists.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(worker) = self.worker.take() {
            // A callback may release its own guard: let that thread finish after
            // the callback returns rather than attempting to join itself.
            if worker.thread().id() != thread::current().id() {
                let _ = worker.join();
            }
        }
    }
}

pub(crate) fn start(callback: DesktopEventCallback) -> Result<NativeWatcher, ApplicationError> {
    let (ready, received) = mpsc::sync_channel(1);
    let stopping = Arc::new(AtomicBool::new(false));
    let worker_stopping = Arc::clone(&stopping);
    let worker = thread::Builder::new()
        .name("tessera-desktop-watch".into())
        .spawn(move || pump(callback, ready, worker_stopping))
        .map_err(|error| ApplicationError::Windows {
            operation: "start desktop watcher",
            code: error.raw_os_error().unwrap_or_default() as u32,
        })?;
    match received.recv() {
        Ok(Ok(thread_id)) => Ok(NativeWatcher {
            thread_id,
            worker: Some(worker),
            stopping,
        }),
        result => {
            let _ = worker.join();
            Err(match result {
                Ok(Err(error)) => error,
                _ => ApplicationError::Windows {
                    operation: "initialize desktop watcher",
                    code: 0,
                },
            })
        }
    }
}

fn register_hooks() -> Result<Vec<Hook>, ApplicationError> {
    let ranges = [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, false),
        (EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE, true),
        (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND, true),
        (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE, true),
        (
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            true,
        ),
    ];
    let mut hooks = Vec::with_capacity(ranges.len());
    for (first, last, skip_own) in ranges {
        // SAFETY: static callback, no DLL, and only notifications from this desktop.
        let handle = unsafe {
            SetWinEventHook(
                first,
                last,
                std::ptr::null_mut(),
                Some(on_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | if skip_own { WINEVENT_SKIPOWNPROCESS } else { 0 },
            )
        };
        if handle.is_null() {
            return Err(ApplicationError::Windows {
                operation: "SetWinEventHook",
                code: unsafe { GetLastError() },
            });
        }
        hooks.push(Hook(handle));
    }
    Ok(hooks)
}

fn pump(
    callback: DesktopEventCallback,
    ready: mpsc::SyncSender<Result<u32, ApplicationError>>,
    stopping: Arc<AtomicBool>,
) {
    let mut message = MSG::default();
    // SAFETY: creates this thread's queue before publishing it to the guard.
    unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
    CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
    let hooks = match register_hooks() {
        Ok(hooks) => hooks,
        Err(error) => {
            let _ = ready.send(Err(error));
            CALLBACK.with(|slot| slot.borrow_mut().take());
            return;
        }
    };
    if ready.send(Ok(unsafe { GetCurrentThreadId() })).is_ok() {
        // GetMessage blocks without idle wakeups and delivers out-of-context
        // WinEvents on the very thread that registered these hooks.
        // If posting WM_QUIT ever fails because the queue is full, a queued
        // message still wakes us and this flag avoids an unbounded drain.
        while !stopping.load(Ordering::Acquire)
            && unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0
        {
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    drop(hooks);
    CALLBACK.with(|slot| slot.borrow_mut().take());
}

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if hwnd.is_null()
        || object != OBJID_WINDOW
        || child != CHILDID_SELF as i32
        || !EVENTS.contains(&event)
    {
        return;
    }
    // WinEvent callbacks can be reentrant. Release the RefCell borrow before
    // invoking user code and never unwind into user32.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let callback = CALLBACK.with(|slot| slot.borrow().clone());
        if let Some(callback) = callback {
            callback();
        }
    }));
}
