// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One recycle-only owner, independent of request queue admission/shutdown.
//! A positive registration cookie does not certify delivery for every volume,
//! removable/mount change or Shell restart covered by the NULL aggregate query.
//! Real Windows all-volume notification coverage remains unverified.

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tessera_system::dock_utilities::{DockUtilityError, DockUtilityErrorKind};
use tessera_system::recycle_bin::{
    RecycleBinWatchCallback, RecycleBinWatchCompletion, RecycleBinWatchEvent, RecycleBinWatchGuard,
};

use crate::recycle_bin::worker::native_error;

const RECYCLE_FOLDER: u128 = 0xb7534046_3ecb_4c18_be4e_64cd4cb7d6ac;
const SOURCES: i32 = 0x8002; // ShellLevel | NewDelivery, not desktop/interrupt scope
const ITEM_MASK: u32 = 0x0002_381F; // rename/create/delete/mkdir/rmdir/attributes/updatedir/updateitem
const NOTIFY_MESSAGE: u32 = 0x8000 + 43; // private WM_APP message
const PUMP_BUDGET: usize = 32;
const SAFETY_WAIT_MS: u32 = 250;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Registration {
    sources: i32,
    mask: u32,
    message: u32,
    entries: i32,
    recursive: bool,
}

const REGISTRATION: Registration = Registration {
    sources: SOURCES,
    mask: ITEM_MASK,
    message: NOTIFY_MESSAGE,
    entries: 1,
    recursive: true,
};

#[cfg(windows)]
mod sdk;

#[cfg(windows)]
pub(crate) fn start_native(
    events: RecycleBinWatchCallback,
    ready: RecycleBinWatchCompletion,
) -> Result<Box<dyn RecycleBinWatchGuard>, DockUtilityError> {
    let stop = Arc::new(sdk::StopEvent::new()?);
    start(stop, || Ok(sdk::WindowsCalls), events, ready)
}

trait StopSignal: Send + Sync + 'static {
    fn signal(&self);
}

struct State<S: StopSignal> {
    live: AtomicBool,
    stop: Arc<S>,
}

impl<S: StopSignal> State<S> {
    fn admits(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }

    fn retire(&self) {
        self.live.store(false, Ordering::Release);
    }
}

struct Guard<S: StopSignal>(Arc<State<S>>);

impl<S: StopSignal> RecycleBinWatchGuard for Guard<S> {}

impl<S: StopSignal> Drop for Guard<S> {
    fn drop(&mut self) {
        // Retirement precedes the wake, with no queue insertion, join or lock.
        self.0.retire();
        let _ = catch_unwind(AssertUnwindSafe(|| self.0.stop.signal()));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Wait {
    Stop,
    Messages,
    Safety,
    Failed,
    Unexpected,
}

fn decode_wait(code: u32) -> Wait {
    match code {
        0 => Wait::Stop,
        1 => Wait::Messages,
        258 => Wait::Safety,
        u32::MAX => Wait::Failed,
        _ => Wait::Unexpected,
    }
}

trait MessageCalls {
    type Message;

    fn next(&self) -> Option<Self::Message>;
    fn is_quit(&self, message: &Self::Message) -> bool;
    fn dispatch(&self, message: &Self::Message);
}

fn pump_messages(calls: &impl MessageCalls, budget: usize) -> bool {
    for _ in 0..budget {
        let Some(message) = calls.next() else {
            return true;
        };
        if calls.is_quit(&message) {
            return false;
        }
        calls.dispatch(&message);
    }
    true
}

trait WindowTeardown {
    fn detach(&self) -> bool;
    fn destroy(&self) -> bool;
}

/// Context must remain alive across both operations and any native reentry.
/// A retired opaque token cannot dereference context even if both calls fail.
fn teardown_window(calls: &impl WindowTeardown) -> bool {
    let detached = calls.detach();
    let destroyed = calls.destroy();
    detached || destroyed
}

#[derive(Clone, Copy, Default)]
struct Hints {
    dirty: bool,
    failed: bool,
}

/// Thread-local bounded state only. WndProc never calls a consumer or transfers
/// notification PIDLs. Cells allow native reentry without outstanding borrows.
#[derive(Default)]
struct Flags {
    dirty: Cell<bool>,
    failed: Cell<bool>,
}

impl Flags {
    fn take(&self) -> Hints {
        Hints {
            dirty: self.dirty.replace(false),
            failed: self.failed.replace(false),
        }
    }
}

struct WindowContext {
    token: usize,
    window: usize,
    flags: Flags,
}

/// One private context per recycle owner thread. Userdata contains an opaque
/// checked, never-reused token, not a pointer. Clones escape the borrow before
/// any native call; retirement removes admission before native cleanup.
#[derive(Default)]
struct Contexts(RefCell<Option<Rc<WindowContext>>>);

impl Contexts {
    fn install(&self, window: usize) -> Result<Rc<WindowContext>, u32> {
        static NEXT_TOKEN: AtomicUsize = AtomicUsize::new(1);
        let mut slot = self.0.borrow_mut();
        if slot.is_some() {
            return Err(0x8000_4005);
        }
        let token = NEXT_TOKEN
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| 0x8000_4005_u32)?;
        let context = Rc::new(WindowContext {
            token,
            window,
            flags: Flags::default(),
        });
        *slot = Some(context.clone());
        Ok(context)
    }

    fn lookup(&self, window: usize, token: usize) -> Option<Rc<WindowContext>> {
        self.0
            .borrow()
            .as_ref()
            .filter(|context| context.window == window && context.token == token)
            .cloned()
    }

    fn retire(&self, window: usize, token: usize) {
        let mut slot = self.0.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|context| context.window == window && context.token == token)
        {
            *slot = None;
        }
    }
}

trait NotificationCalls {
    type Lock;

    fn lock(&self, shared_handle: usize, process_id: u32) -> Option<(Self::Lock, i32)>;
    fn unlock(&self, lock: &Self::Lock) -> bool;
}

struct Unlock<'a, C: NotificationCalls> {
    calls: &'a C,
    lock: C::Lock,
    flags: &'a Flags,
}

impl<C: NotificationCalls> Drop for Unlock<'_, C> {
    fn drop(&mut self) {
        // Even an adapter panic while unlocking must not unwind through FFI or
        // double-panic during another unwind. One successful Lock owns one call.
        if !catch_unwind(AssertUnwindSafe(|| self.calls.unlock(&self.lock))).unwrap_or(false) {
            self.flags.failed.set(true);
        }
    }
}

/// Used by the actual WndProc and recording tests; no FFI consumer callbacks.
fn notification<C: NotificationCalls>(
    calls: &C,
    flags: &Flags,
    shared_handle: usize,
    process_id: u32,
) {
    let handled = catch_unwind(AssertUnwindSafe(|| {
        let Some((lock, event)) = calls.lock(shared_handle, process_id) else {
            flags.failed.set(true);
            return;
        };
        let _unlock = Unlock { calls, lock, flags };
        // Payload pointers are borrowed inside the native lock and never read,
        // freed or transferred. Only scoped membership/update bits are hints.
        if event as u32 & ITEM_MASK != 0 {
            flags.dirty.set(true);
        }
    }));
    if handled.is_err() {
        flags.failed.set(true);
    }
}

trait Calls<S: StopSignal>: 'static {
    type Pidl;
    type Window;

    fn initialize(&self, model: i32) -> i32;
    fn resolve(&self, folder: u128) -> Result<Option<Self::Pidl>, u32>;
    fn create_window(&self, message: u32) -> Result<Self::Window, u32>;
    fn register(&self, window: &Self::Window, pidl: &Self::Pidl, registration: Registration)
    -> u32;
    fn hints(&self, window: &Self::Window) -> Hints;
    fn wait(&self, stop: &S, safety_ms: u32) -> Wait;
    /// Pump all thread messages, bounded per turn. False means WM_QUIT.
    fn pump(&self, window: &Self::Window, budget: usize) -> bool;
    fn retire(&self, window: &Self::Window);
    fn deregister(&self, cookie: u32);
    fn uninitialize(&self);
}

struct Session<C: Calls<S>, S: StopSignal> {
    state: Arc<State<S>>,
    calls: C,
    cookie: Option<u32>,
    window: Option<C::Window>,
    pidl: Option<C::Pidl>,
    initialized: bool,
}

impl<C: Calls<S>, S: StopSignal> Session<C, S> {
    fn install(calls: C, state: Arc<State<S>>) -> Result<Self, DockUtilityError> {
        let mut session = Self {
            state,
            calls,
            cookie: None,
            window: None,
            pidl: None,
            initialized: false,
        };
        session.check_live()?;
        let code = session.calls.initialize(super::STA_MODEL);
        if code < 0 {
            return Err(native_error(
                code as u32,
                "Recycle Bin watch COM initialization",
            ));
        }
        session.initialized = true;
        session.check_live()?;
        session.pidl = session
            .calls
            .resolve(RECYCLE_FOLDER)
            .map_err(|code| native_error(code, "Recycle Bin watch fixed namespace"))?;
        if session.pidl.is_none() {
            return Err(native_error(
                0x8000_4005,
                "Recycle Bin watch null namespace",
            ));
        }
        session.check_live()?;
        session.window = Some(
            session
                .calls
                .create_window(NOTIFY_MESSAGE)
                .map_err(|code| native_error(code, "Recycle Bin watch window"))?,
        );
        session.check_live()?;
        let cookie = session.calls.register(
            session.window.as_ref().unwrap(),
            session.pidl.as_ref().unwrap(),
            REGISTRATION,
        );
        if cookie == 0 {
            // Register has no documented GetLastError contract.
            return Err(DockUtilityError::new(
                DockUtilityErrorKind::Unavailable,
                "Recycle Bin watch registration: 0x80004005",
            ));
        }
        session.cookie = Some(cookie);
        session.check_live()?;
        Ok(session)
    }

    fn check_live(&self) -> Result<(), DockUtilityError> {
        if self.state.admits() {
            Ok(())
        } else {
            Err(stopped())
        }
    }

    fn hints(&self) -> Hints {
        self.calls.hints(self.window.as_ref().unwrap())
    }

    fn turn(&self) -> Result<(), DockUtilityError> {
        match self.calls.wait(&self.state.stop, SAFETY_WAIT_MS) {
            Wait::Messages => {
                if self.calls.pump(self.window.as_ref().unwrap(), PUMP_BUDGET) {
                    Ok(())
                } else {
                    Err(unavailable("Recycle Bin watch WM_QUIT"))
                }
            }
            Wait::Safety => Ok(()),
            Wait::Stop if !self.state.admits() => Ok(()),
            Wait::Stop | Wait::Failed | Wait::Unexpected => {
                Err(unavailable("Recycle Bin watch wait"))
            }
        }
    }
}

impl<C: Calls<S>, S: StopSignal> Drop for Session<C, S> {
    fn drop(&mut self) {
        // Retire native admission before any teardown can synchronously reenter.
        self.state.retire();
        if let Some(window) = self.window.as_ref() {
            self.calls.retire(window);
        }
        if let Some(cookie) = self.cookie.take() {
            self.calls.deregister(cookie);
        }
        // Window detaches userdata while its context is still alive; then PIDL
        // ILFree and COM balance happen on this same owner thread.
        drop(self.window.take());
        drop(self.pidl.take());
        if self.initialized {
            self.calls.uninitialize();
        }
    }
}

fn start<S, C, F>(
    stop: Arc<S>,
    create: F,
    events: RecycleBinWatchCallback,
    ready: RecycleBinWatchCompletion,
) -> Result<Box<dyn RecycleBinWatchGuard>, DockUtilityError>
where
    S: StopSignal,
    C: Calls<S>,
    F: FnOnce() -> Result<C, DockUtilityError> + Send + 'static,
{
    let state = Arc::new(State {
        live: AtomicBool::new(true),
        stop,
    });
    let worker_state = state.clone();
    std::thread::Builder::new()
        .name("tessera-recycle-watch".into())
        .spawn(move || run(create, worker_state, events, ready))
        .map_err(|_| native_error(0x8000_4005, "Recycle Bin watch start"))?;
    Ok(Box::new(Guard(state)))
}

fn run<S, C>(
    create: impl FnOnce() -> Result<C, DockUtilityError>,
    state: Arc<State<S>>,
    events: RecycleBinWatchCallback,
    ready: RecycleBinWatchCompletion,
) where
    S: StopSignal,
    C: Calls<S>,
{
    let installed = catch_unwind(AssertUnwindSafe(|| {
        if !state.admits() {
            return Err(stopped());
        }
        Session::install(create()?, state.clone())
    }))
    .unwrap_or_else(|_| Err(native_error(0x8000_4005, "Recycle Bin watch startup")));
    let session = match installed {
        Ok(session) => session,
        Err(error) => {
            state.retire();
            let error = if error.kind == DockUtilityErrorKind::Stopped {
                stopped()
            } else {
                error
            };
            let _ = catch_unwind(AssertUnwindSafe(|| ready(Err(error))));
            return;
        }
    };
    if !state.admits() {
        drop(session);
        let _ = catch_unwind(AssertUnwindSafe(|| ready(Err(stopped()))));
        return;
    }
    // No hints are published until successful readiness has returned normally.
    if catch_unwind(AssertUnwindSafe(|| ready(Ok(())))).is_err() {
        state.retire();
        drop(session);
        return;
    }
    let failure = loop {
        if !state.admits() {
            break None;
        }
        let hints = match catch_unwind(AssertUnwindSafe(|| session.hints())) {
            Ok(hints) => hints,
            Err(_) => break Some(unavailable("Recycle Bin watch hints")),
        };
        if hints.failed {
            break Some(unavailable("Recycle Bin watch delivery"));
        }
        if hints.dirty && state.admits() {
            // Admission is the in-flight linearization point. No native lock,
            // FFI frame, state borrow or synchronization lock spans a consumer.
            let _ = catch_unwind(AssertUnwindSafe(|| {
                events(RecycleBinWatchEvent::Invalidated)
            }));
        }
        if !state.admits() {
            break None;
        }
        match catch_unwind(AssertUnwindSafe(|| session.turn())) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => break Some(error),
            Err(_) => break Some(unavailable("Recycle Bin watch pump")),
        }
    };
    // Reserve the single terminal delivery before native retirement. An admitted
    // callback may finish after a concurrent guard drop, as for normal events.
    let terminal_admitted = failure.is_some() && state.admits();
    state.retire();
    drop(session);
    if terminal_admitted && let Some(error) = failure {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            events(RecycleBinWatchEvent::Unavailable(error.kind));
        }));
    }
}

fn stopped() -> DockUtilityError {
    DockUtilityError::new(
        DockUtilityErrorKind::Stopped,
        "Recycle Bin watch: 0x80010108",
    )
}

fn unavailable(operation: &'static str) -> DockUtilityError {
    DockUtilityError::new(
        DockUtilityErrorKind::Unavailable,
        format!("{operation}: 0x80004005"),
    )
}

#[cfg(test)]
mod tests;
