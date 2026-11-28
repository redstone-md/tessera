// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The objects programs register to be started with the shell, hosted by
//! Tessera as the shell of the session.
//!
//! Explorer creates, once its desktop is ready, every class named under
//! `Software\Microsoft\Windows\CurrentVersion\ShellServiceObjects` of
//! the machine and of the user that has an `AutoStart` value, and every
//! class the older `ShellServiceObjectDelayLoad` values name, and sends
//! each `IOleCommandTarget::Exec(CGID_ShellServiceObject,
//! SSOCMDID_OPEN)`; at the end of the session it sends `SSOCMDID_CLOSE`.
//! [`ShellServiceObjects`] does the same on a thread of its own, in a
//! single-threaded apartment with a message loop, as the objects expect.
//! An object that cannot be made or refuses to open is reported and left
//! out. The close is part of the bounded wait of the end of the session
//! (see `ShellHost::close_at_session_end`), and of Tessera's quitting.
//!
//! `SharedTaskScheduler`, the other list Explorer once read, has had no
//! effect since Windows Vista and is not hosted.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::System::Ole::IOleCommandTarget;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentThreadId, SetEvent, WaitForSingleObject,
};
use windows::Win32::UI::Shell::CGID_ShellServiceObject;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    TranslateMessage, WM_APP,
};
use windows::core::{GUID, PCWSTR};

use super::ShellHost;
use super::registry::Key;

/// The command that opens an object (`SSOCMDID_OPEN` of shlobj.h).
const SSOCMDID_OPEN: u32 = 2;
/// The command that closes it (`SSOCMDID_CLOSE`).
const SSOCMDID_CLOSE: u32 = 3;

/// The thread message that has the thread close the objects and end.
const CLOSE_ALL: u32 = WM_APP + 0x31;

/// Where the classes are registered, under the machine and the user.
const OBJECTS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\ShellServiceObjects";
const DELAY_LOAD_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\ShellServiceObjectDelayLoad";

/// A class registered to start with the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Registered {
    pub class: GUID,
    /// What the registration calls it: the name of the value of
    /// `ShellServiceObjectDelayLoad`, or the key of `ShellServiceObjects`.
    pub name: String,
}

/// The thread of the objects while it runs.
struct Running {
    thread: u32,
    /// Set once the thread closed the objects; kept for the life of the
    /// process, as the end of the session may still wait on it.
    closed: HANDLE,
}

// SAFETY: the event handle may be used and waited on from any thread.
unsafe impl Send for Running {}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
static CLOSE_REGISTERED: AtomicBool = AtomicBool::new(false);

/// The objects of `ShellServiceObjects`, hosted by Tessera.
pub struct ShellServiceObjects;

impl ShellServiceObjects {
    /// Starts the thread that makes and opens the objects, where it does
    /// not run; returns at once.
    pub fn start() {
        let mut running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
        if running.is_some() {
            return;
        }
        // SAFETY: an unnamed event without a security descriptor, closed
        // only by the end of the process.
        let closed = match unsafe { CreateEventW(None, true, false, PCWSTR::null()) } {
            Ok(event) => event,
            Err(error) => {
                report!("the objects that start with the shell are not started: {error}");
                return;
            }
        };
        let (ready, thread) = std::sync::mpsc::sync_channel(1);
        let event = closed.0 as isize;
        let started = thread::Builder::new()
            .name("shell service objects".to_owned())
            .spawn(move || host(ready, HANDLE(event as *mut c_void)));
        if let Err(error) = started {
            report!("the objects that start with the shell are not started: {error}");
            // SAFETY: the event was made above and is used by nobody.
            let _ = unsafe { CloseHandle(closed) };
            return;
        }
        // The thread tells its number once it has its queue of messages,
        // which takes no waiting.
        match thread.recv_timeout(Duration::from_secs(2)) {
            Ok(thread) => *running = Some(Running { thread, closed }),
            Err(_) => report!("the thread of the objects that start with the shell did not start"),
        }
        drop(running);
        if !CLOSE_REGISTERED.swap(true, Ordering::SeqCst) {
            ShellHost::close_at_session_end(begin_close);
        }
    }

    /// Closes the objects and waits at most `time` for them; says whether
    /// they closed in time. Nothing runs where nothing was started. It
    /// waits, so it is not called on the thread of GPUI.
    pub fn close(time: Duration) -> bool {
        let Some(closed) = begin_close() else {
            return true;
        };
        let milliseconds = time.as_millis().min(u128::from(u32::MAX)) as u32;
        // SAFETY: the event lives for the life of the process.
        unsafe { WaitForSingleObject(closed, milliseconds) == WAIT_OBJECT_0 }
    }
}

/// Asks the thread to close the objects, and gives the event it sets once
/// it has; `None` where it does not run.
fn begin_close() -> Option<HANDLE> {
    let running = RUNNING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()?;
    // SAFETY: a thread that ended refuses the message without harm.
    let posted = unsafe { PostThreadMessageW(running.thread, CLOSE_ALL, WPARAM(0), LPARAM(0)) };
    posted.is_ok().then_some(running.closed)
}

/// The body of the thread: makes and opens the objects, then handles
/// messages until it is told to close them.
fn host(ready: std::sync::mpsc::SyncSender<u32>, closed: HANDLE) {
    let mut message = MSG::default();
    // SAFETY: peeking makes the queue of this thread, so that the message
    // that closes the objects cannot be lost; `message` outlives the call.
    unsafe {
        let _ = PeekMessageW(&raw mut message, None, 0, 0, PM_NOREMOVE);
        let _ = ready.send(GetCurrentThreadId());
    }
    // SAFETY: COM is initialized for this thread and left at its end.
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let objects = open_all(registered());
    // SAFETY: `message` outlives the calls; the objects' windows, if any,
    // get their messages dispatched here.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            if message.hwnd.is_invalid() && message.message == CLOSE_ALL {
                break;
            }
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
    close_all(objects);
    if initialized {
        // SAFETY: matched with the initialization above.
        unsafe { CoUninitialize() };
    }
    // SAFETY: the event lives for the life of the process.
    let _ = unsafe { SetEvent(closed) };
}

/// Makes and opens each class; those that fail are reported and left out.
fn open_all(classes: Vec<Registered>) -> Vec<(Registered, IOleCommandTarget)> {
    let mut opened = Vec::new();
    for registered in classes {
        // SAFETY: the class is a registered one; the object is released
        // when it is dropped, on this thread.
        let made: windows::core::Result<IOleCommandTarget> =
            unsafe { CoCreateInstance(&registered.class, None, CLSCTX_INPROC_SERVER) };
        let target = match made {
            Ok(target) => target,
            Err(error) => {
                report!(
                    "{} ({:?}) was not started with the shell: {error}",
                    registered.name,
                    registered.class
                );
                continue;
            }
        };
        // SAFETY: the group is a constant and the command takes no
        // arguments.
        let open = unsafe {
            target.Exec(
                &CGID_ShellServiceObject,
                SSOCMDID_OPEN,
                0,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        match open {
            Ok(()) => opened.push((registered, target)),
            Err(error) => {
                report!(
                    "{} ({:?}) did not open with the shell: {error}",
                    registered.name,
                    registered.class
                );
            }
        }
    }
    opened
}

/// Closes each object, the last opened first, and lets it go.
fn close_all(objects: Vec<(Registered, IOleCommandTarget)>) {
    for (registered, target) in objects.into_iter().rev() {
        // SAFETY: as for the open.
        let closed = unsafe {
            target.Exec(
                &CGID_ShellServiceObject,
                SSOCMDID_CLOSE,
                0,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if let Err(error) = closed {
            report!("{} did not close: {error}", registered.name);
        }
    }
}

/// The classes registered under the machine, then the user.
fn registered() -> Vec<Registered> {
    let mut found = Vec::new();
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        found.extend(objects_of(root));
        found.extend(delay_loads_of(root));
    }
    unique(found)
}

/// The keys of `ShellServiceObjects` under `root` that have `AutoStart`.
fn objects_of(root: HKEY) -> Vec<Registered> {
    let Some(key) = Key::read(root, OBJECTS_KEY, KEY_WOW64_64KEY) else {
        return Vec::new();
    };
    { key.subkeys().into_iter() }
        .filter(|name| {
            key.child(name).is_some_and(|object| {
                let values = object.values();
                { values.iter() }.any(|(value, _)| value.eq_ignore_ascii_case("AutoStart"))
            })
        })
        .filter_map(|name| {
            Some(Registered {
                class: class_of(&name)?,
                name,
            })
        })
        .collect()
}

/// The values of `ShellServiceObjectDelayLoad` under `root`: a name and
/// the class it starts.
fn delay_loads_of(root: HKEY) -> Vec<Registered> {
    let Some(key) = Key::read(root, DELAY_LOAD_KEY, KEY_WOW64_64KEY) else {
        return Vec::new();
    };
    { key.values().into_iter() }
        .filter_map(|(name, value)| {
            let class = class_of(&value.text()?)?;
            Some(Registered { class, name })
        })
        .collect()
}

/// The class a registry text names, `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}`.
pub(super) fn class_of(text: &str) -> Option<GUID> {
    let inner = text.trim().strip_prefix('{')?.strip_suffix('}')?;
    GUID::try_from(inner).ok()
}

/// The classes, each once, in their first place.
fn unique(classes: Vec<Registered>) -> Vec<Registered> {
    let mut seen = HashSet::new();
    { classes.into_iter() }
        .filter(|registered| seen.insert(registered.class.to_u128()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_class_is_read_in_braces_only() {
        assert_eq!(
            class_of("{35786D3C-B075-49b9-88DD-029876E11C01}"),
            Some(GUID::from_u128(0x35786d3c_b075_49b9_88dd_029876e11c01))
        );
        assert_eq!(
            class_of(" {35786D3C-B075-49B9-88DD-029876E11C01} "),
            Some(GUID::from_u128(0x35786d3c_b075_49b9_88dd_029876e11c01))
        );
        assert_eq!(class_of("35786D3C-B075-49B9-88DD-029876E11C01"), None);
        assert_eq!(class_of("{not a class}"), None);
    }

    #[test]
    fn a_class_registered_twice_starts_once() {
        let one = GUID::from_u128(1);
        let two = GUID::from_u128(2);
        let registered = |class, name: &str| Registered {
            class,
            name: name.to_owned(),
        };
        let classes = vec![
            registered(one, "machine"),
            registered(two, "other"),
            registered(one, "user"),
        ];
        assert_eq!(
            unique(classes),
            [registered(one, "machine"), registered(two, "other")]
        );
    }
}
