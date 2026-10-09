// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Display-only notifications, independently owned from accepted display reads.
//! Native calls may stall, and failed mandatory SDK acknowledgements may cause
//! Windows to terminate the process. Software coalescing does not bound either.

use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use std::time::Instant;

use tessera_system::display_context::{
    DisplayContextError, DisplayContextWatchCallback, DisplayContextWatchEvent,
    DisplayContextWatchGuard, DisplayContextWatchReady,
};

const FAILURE: u32 = 0x8000_4005;
const PUMP_BUDGET: usize = 32;
const QUIET_MS: u64 = 400;
const BURST_CAP_MS: u64 = 2_000;

#[cfg(windows)]
mod sdk;
#[cfg(all(windows, test))]
mod sdk_tests;
#[cfg(test)]
mod tests;

#[cfg(windows)]
pub(crate) fn watch(
    on_event: DisplayContextWatchCallback,
    on_ready: DisplayContextWatchReady,
) -> Result<Box<dyn DisplayContextWatchGuard>, DisplayContextError> {
    let signals = Signals::create(sdk::Kernel)?;
    start(signals, sdk::WindowsCalls::new, on_event, on_ready)
}

fn native(code: u32) -> DisplayContextError {
    DisplayContextError::Native { code }
}

/// Only SetEvent/ResetEvent/CloseHandle are serialized. The owner never holds
/// this lock across waiting, dispatch, WinRT registration/Stop or a consumer.
/// A failed close leaves ownership intact for a later best-effort attempt.
trait KernelCalls: Send + Sync + 'static {
    type Event: Copy + Send;
    fn create(&self) -> Result<Self::Event, u32>;
    fn set(&self, event: Self::Event) -> Result<(), u32>;
    fn reset(&self, event: Self::Event) -> Result<(), u32>;
    fn close(&self, event: Self::Event) -> Result<(), u32>;
}

struct Handles<E> {
    stop: Option<E>,
    wake: Option<E>,
}

struct Signals<K: KernelCalls> {
    kernel: K,
    handles: Mutex<Handles<K::Event>>,
}

impl<K: KernelCalls> Signals<K> {
    fn create(kernel: K) -> Result<Arc<Self>, DisplayContextError> {
        let stop = kernel.create().map_err(native)?;
        let signals = Arc::new(Self {
            kernel,
            handles: Mutex::new(Handles {
                stop: Some(stop),
                wake: None,
            }),
        });
        let wake = signals.kernel.create().map_err(native)?;
        signals
            .handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .wake = Some(wake);
        Ok(signals)
    }

    fn stop(&self) -> Result<(), u32> {
        let handles = self.handles.lock().unwrap_or_else(|e| e.into_inner());
        handles.stop.map_or(Ok(()), |event| self.kernel.set(event))
    }

    fn wake(&self) -> Result<(), u32> {
        let handles = self.handles.lock().unwrap_or_else(|e| e.into_inner());
        handles.wake.map_or(Ok(()), |event| self.kernel.set(event))
    }

    fn reset_wake(&self) -> Result<(), u32> {
        let handles = self.handles.lock().unwrap_or_else(|e| e.into_inner());
        handles
            .wake
            .map_or(Ok(()), |event| self.kernel.reset(event))
    }

    fn borrowed(&self) -> Option<(K::Event, K::Event)> {
        let handles = self.handles.lock().unwrap_or_else(|e| e.into_inner());
        Some((handles.stop?, handles.wake?))
    }

    fn close(&self) -> Result<(), u32> {
        let mut handles = self.handles.lock().unwrap_or_else(|e| e.into_inner());
        let mut failure = None;
        let Handles { stop, wake } = &mut *handles;
        for slot in [stop, wake] {
            if let Some(event) = *slot {
                match self.kernel.close(event) {
                    Ok(()) => *slot = None,
                    Err(code) => {
                        failure.get_or_insert(code);
                    }
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

impl<K: KernelCalls> Drop for Signals<K> {
    fn drop(&mut self) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.close()));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Source {
    Enabled,
    Disabled,
    Changed,
    PathsFailedOrInvalidated,
    TextScale,
    Window,
}

const MANAGER_SOURCES: [Source; 4] = [
    Source::Enabled,
    Source::Disabled,
    Source::Changed,
    Source::PathsFailedOrInvalidated,
];

#[derive(Default)]
struct Hints {
    dirty: bool,
    disabled: bool,
    failed: bool,
}

/// Shared callbacks capture this bounded state, never an owner, HWND or WinRT
/// object. Late SDK callbacks still acknowledge, but cannot admit notifications.
struct Notifications<K: KernelCalls> {
    live: AtomicBool,
    dirty: AtomicBool,
    disabled: AtomicBool,
    failed: AtomicBool,
    queued: AtomicBool,
    signals: Arc<Signals<K>>,
}

impl<K: KernelCalls> Notifications<K> {
    fn new(signals: Arc<Signals<K>>) -> Arc<Self> {
        Arc::new(Self {
            live: AtomicBool::new(true),
            dirty: AtomicBool::new(false),
            disabled: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            queued: AtomicBool::new(false),
            signals,
        })
    }

    fn admits(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }
    fn retire(&self) {
        self.live.store(false, Ordering::Release);
    }

    fn hint(&self, source: Source) {
        if !self.admits() {
            return;
        }
        if source == Source::Disabled {
            self.disabled.store(true, Ordering::Release);
        }
        if source == Source::Enabled {
            self.disabled.store(false, Ordering::Release);
        }
        self.dirty.store(true, Ordering::Release);
        self.wake();
    }

    fn fault(&self) {
        if !self.admits() {
            return;
        }
        self.failed.store(true, Ordering::Release);
        self.wake();
    }

    fn wake(&self) {
        if !self.queued.swap(true, Ordering::AcqRel) && self.signals.wake().is_err() {
            self.failed.store(true, Ordering::Release);
            // A fault is terminal. Wake failure attempts the independent stop
            // event; if both native signals fail the native wait may stall.
            let _ = self.signals.stop();
        }
    }

    fn take(&self) -> Result<Hints, u32> {
        // Reset before flag extraction: any callback whose wake is suppressed
        // by the old queued bit has its flags included in this extraction.
        self.signals.reset_wake()?;
        self.queued.store(false, Ordering::Release);
        Ok(Hints {
            dirty: self.dirty.swap(false, Ordering::AcqRel),
            disabled: self.disabled.load(Ordering::Acquire),
            failed: self.failed.swap(false, Ordering::AcqRel),
        })
    }
}

/// The same mandatory acknowledgement path is used by all four SDK callbacks
/// and recording adapters. Admission is intentionally checked AFTER the attempt.
fn manager_callback<K: KernelCalls>(
    notifications: &Notifications<K>,
    source: Source,
    acknowledge: impl FnOnce() -> Result<(), u32>,
) {
    let result = catch_unwind(AssertUnwindSafe(acknowledge));
    if matches!(result, Ok(Ok(()))) {
        notification_callback(notifications, source);
    } else {
        let _ = catch_unwind(AssertUnwindSafe(|| notifications.fault()));
    }
}

fn notification_callback<K: KernelCalls>(notifications: &Notifications<K>, source: Source) {
    if catch_unwind(AssertUnwindSafe(|| notifications.hint(source))).is_err() {
        let _ = catch_unwind(AssertUnwindSafe(|| notifications.fault()));
    }
}

struct Guard<K: KernelCalls>(Arc<Notifications<K>>);
impl<K: KernelCalls> DisplayContextWatchGuard for Guard<K> {}
impl<K: KernelCalls> Drop for Guard<K> {
    fn drop(&mut self) {
        self.0.retire();
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if self.0.signals.stop().is_err() {
                let _ = self.0.signals.wake();
            }
        }));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Wait {
    Stop,
    Wake,
    Messages,
    Timeout,
    Failed,
}

fn decode_wait(code: u32) -> Wait {
    match code {
        0 => Wait::Stop,
        1 => Wait::Wake,
        2 => Wait::Messages,
        258 => Wait::Timeout,
        _ => Wait::Failed,
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

/// Every native-owned resource is opaque here. This one owner executes startup,
/// pump and retirement for both the SDK and recording adapters, on one MTA thread.
trait Calls: 'static {
    type Kernel: KernelCalls;
    type Class;
    type Window;
    type Context;
    type Settings;
    type Manager;
    type Token;

    fn initialize(&self) -> Result<(), u32>;
    fn uninitialize(&self);
    fn create_class(&self) -> Result<Self::Class, u32>;
    fn create_window(&self, class: &Self::Class) -> Result<Self::Window, u32>;
    fn attach(
        &self,
        window: &Self::Window,
        notifications: Arc<Notifications<Self::Kernel>>,
    ) -> Result<Self::Context, u32>;
    fn retire_context(&self, context: &Self::Context);
    fn detach(&self, window: &Self::Window) -> Result<(), u32>;
    fn destroy_window(&self, window: &Self::Window) -> Result<(), u32>;
    fn unregister_class(&self, class: &Self::Class) -> Result<(), u32>;
    fn create_settings(&self) -> Result<Self::Settings, u32>;
    fn create_manager(&self) -> Result<Self::Manager, u32>;
    fn register_text(
        &self,
        settings: &Self::Settings,
        notifications: Arc<Notifications<Self::Kernel>>,
    ) -> Result<Self::Token, u32>;
    fn register_manager(
        &self,
        manager: &Self::Manager,
        source: Source,
        notifications: Arc<Notifications<Self::Kernel>>,
    ) -> Result<Self::Token, u32>;
    fn start(&self, manager: &Self::Manager) -> Result<(), u32>;
    fn stop(&self, manager: &Self::Manager) -> Result<(), u32>;
    fn remove_text(&self, settings: &Self::Settings, token: &Self::Token) -> Result<(), u32>;
    fn remove_manager(
        &self,
        manager: &Self::Manager,
        source: Source,
        token: &Self::Token,
    ) -> Result<(), u32>;
    fn close_manager(&self, manager: &Self::Manager) -> Result<(), u32>;
    fn release_settings(&self, settings: Self::Settings);
    fn release_manager(&self, manager: Self::Manager);
    fn now_ms(&self) -> u64;
    fn wait(
        &self,
        stop: <Self::Kernel as KernelCalls>::Event,
        wake: <Self::Kernel as KernelCalls>::Event,
        timeout_ms: Option<u32>,
    ) -> Wait;
    /// Dispatch at most budget messages; false means WM_QUIT.
    fn pump(&self, budget: usize) -> bool;
}

struct Owner<C: Calls> {
    calls: C,
    notifications: Arc<Notifications<C::Kernel>>,
    initialized: bool,
    class: Option<C::Class>,
    window: Option<C::Window>,
    context: Option<C::Context>,
    settings: Option<C::Settings>,
    manager: Option<C::Manager>,
    text_token: Option<C::Token>,
    manager_tokens: Vec<(Source, C::Token)>,
    start_attempted: bool,
    retired: bool,
    _thread_bound: PhantomData<Rc<()>>,
}

impl<C: Calls> Owner<C> {
    fn new(calls: C, notifications: Arc<Notifications<C::Kernel>>) -> Self {
        Self {
            calls,
            notifications,
            initialized: false,
            class: None,
            window: None,
            context: None,
            settings: None,
            manager: None,
            text_token: None,
            manager_tokens: Vec::with_capacity(4),
            start_attempted: false,
            retired: false,
            _thread_bound: PhantomData,
        }
    }

    fn check_live(&self) -> Result<(), DisplayContextError> {
        if self.notifications.admits() {
            Ok(())
        } else {
            Err(DisplayContextError::Stopped)
        }
    }

    fn install(&mut self) -> Result<(), DisplayContextError> {
        self.check_live()?;
        self.calls.initialize().map_err(native)?;
        self.initialized = true;
        self.check_live()?;
        self.class = Some(self.calls.create_class().map_err(native)?);
        self.check_live()?;
        self.window = Some(
            self.calls
                .create_window(self.class.as_ref().unwrap())
                .map_err(native)?,
        );
        self.check_live()?;
        self.context = Some(
            self.calls
                .attach(self.window.as_ref().unwrap(), self.notifications.clone())
                .map_err(native)?,
        );
        self.check_live()?;
        self.settings = Some(self.calls.create_settings().map_err(native)?);
        self.check_live()?;
        self.text_token = Some(
            self.calls
                .register_text(self.settings.as_ref().unwrap(), self.notifications.clone())
                .map_err(native)?,
        );
        self.check_live()?;
        self.manager = Some(self.calls.create_manager().map_err(native)?);
        for source in MANAGER_SOURCES {
            self.check_live()?;
            let token = self
                .calls
                .register_manager(
                    self.manager.as_ref().unwrap(),
                    source,
                    self.notifications.clone(),
                )
                .map_err(native)?;
            self.manager_tokens.push((source, token));
        }
        self.check_live()?;
        // A failing Start can have synchronously enabled part of the stack.
        // Attempted state, not only returned success, therefore owns one Stop.
        self.start_attempted = true;
        self.calls
            .start(self.manager.as_ref().unwrap())
            .map_err(native)?;
        self.check_live()?;
        // Closes the registration/read race, even when Start emitted no hint.
        self.notifications.hint(Source::Changed);
        Ok(())
    }

    fn pump(&self, events: &DisplayContextWatchCallback) -> Option<DisplayContextError> {
        let mut burst = Burst::default();
        loop {
            if !self.notifications.admits() {
                return None;
            }
            let hints = match self.notifications.take() {
                Ok(hints) => hints,
                Err(code) => return Some(native(code)),
            };
            if hints.failed {
                return Some(DisplayContextError::Unavailable);
            }
            if hints.dirty {
                burst.hint(self.calls.now_ms());
            }
            // Disabled is an unavailable-stack invalidation, never an empty
            // snapshot. Keep registrations live so Enabled can recover it.
            let _stack_unavailable = hints.disabled;
            if burst.due(self.calls.now_ms()) {
                burst = Burst::default();
                if self.notifications.admits() {
                    let _ = catch_unwind(AssertUnwindSafe(|| {
                        events(DisplayContextWatchEvent::Changed)
                    }));
                }
            }
            if !self.notifications.admits() {
                return None;
            }
            let Some((stop, wake)) = self.notifications.signals.borrowed() else {
                return Some(DisplayContextError::Unavailable);
            };
            let timeout = burst.timeout(self.calls.now_ms());
            match self.calls.wait(stop, wake, timeout) {
                Wait::Stop if !self.notifications.admits() => return None,
                Wait::Stop | Wait::Failed => return Some(DisplayContextError::Unavailable),
                Wait::Messages => {
                    if !self.calls.pump(PUMP_BUDGET) {
                        return Some(DisplayContextError::Unavailable);
                    }
                }
                Wait::Wake => {}
                Wait::Timeout if timeout.is_some() => {}
                Wait::Timeout => return Some(DisplayContextError::Unavailable),
            }
        }
    }

    /// Retirement is best effort but truthful: failed native closure is a fault,
    /// not a claim that the OS resource closed. Stop can synchronously reenter.
    fn retire(&mut self) -> Result<(), DisplayContextError> {
        if self.retired {
            return Ok(());
        }
        self.retired = true;
        self.notifications.retire();
        let mut failure = None;
        let mut attempt = |operation: &mut dyn FnMut() -> Result<(), u32>| {
            let result = catch_unwind(AssertUnwindSafe(operation)).unwrap_or(Err(FAILURE));
            if let Err(code) = result {
                failure.get_or_insert(code);
            }
            result
        };
        if let Some(context) = self.context.as_ref() {
            let _ = catch_unwind(AssertUnwindSafe(|| self.calls.retire_context(context)));
        }
        if let Some(manager) = self.manager.as_ref() {
            if self.start_attempted {
                let _ = attempt(&mut || self.calls.stop(manager));
            }
            for (source, token) in self.manager_tokens.drain(..).rev() {
                let _ = attempt(&mut || self.calls.remove_manager(manager, source, &token));
            }
        }
        if let (Some(settings), Some(token)) = (self.settings.as_ref(), self.text_token.take()) {
            let _ = attempt(&mut || self.calls.remove_text(settings, &token));
        }
        if let Some(manager) = self.manager.take() {
            let _ = attempt(&mut || self.calls.close_manager(&manager));
            self.calls.release_manager(manager);
        }
        if let Some(settings) = self.settings.take() {
            self.calls.release_settings(settings);
        }
        if let Some(window) = self.window.take() {
            let _ = attempt(&mut || self.calls.detach(&window));
            if attempt(&mut || self.calls.destroy_window(&window)).is_err() {
                // Retry on the same thread, never transfer HWND ownership.
                if attempt(&mut || self.calls.destroy_window(&window)).is_err() {
                    std::mem::forget(window);
                    if let Some(class) = self.class.take() {
                        std::mem::forget(class);
                    }
                }
            }
        }
        drop(self.context.take());
        if let Some(class) = self.class.take()
            && attempt(&mut || self.calls.unregister_class(&class)).is_err()
        {
            std::mem::forget(class);
        }
        if self.initialized {
            self.initialized = false;
            self.calls.uninitialize();
        }
        let _ = attempt(&mut || self.notifications.signals.close());
        // Retry only failed handle slots; successful closes cannot be repeated.
        let _ = attempt(&mut || self.notifications.signals.close());
        failure.map_or(Ok(()), |code| Err(native(code)))
    }
}

impl<C: Calls> Drop for Owner<C> {
    fn drop(&mut self) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.retire()));
    }
}

#[derive(Default)]
struct Burst {
    first: Option<u64>,
    last: u64,
}
impl Burst {
    fn hint(&mut self, now: u64) {
        self.first.get_or_insert(now);
        self.last = now;
    }
    fn deadline(&self) -> Option<u64> {
        self.first.map(|first| {
            self.last
                .saturating_add(QUIET_MS)
                .min(first.saturating_add(BURST_CAP_MS))
        })
    }
    fn due(&self, now: u64) -> bool {
        self.deadline().is_some_and(|deadline| now >= deadline)
    }
    fn timeout(&self, now: u64) -> Option<u32> {
        self.deadline()
            .map(|deadline| deadline.saturating_sub(now).min(u32::MAX as u64 - 1) as u32)
    }
}

fn start<K, C>(
    signals: Arc<Signals<K>>,
    create: impl FnOnce() -> C + Send + 'static,
    events: DisplayContextWatchCallback,
    ready: DisplayContextWatchReady,
) -> Result<Box<dyn DisplayContextWatchGuard>, DisplayContextError>
where
    K: KernelCalls,
    C: Calls<Kernel = K>,
{
    let notifications = Notifications::new(signals);
    let worker = notifications.clone();
    std::thread::Builder::new()
        .name("tessera-display-watch".into())
        .spawn(move || run(create, worker, events, ready))
        .map_err(|_| native(FAILURE))?;
    Ok(Box::new(Guard(notifications)))
}

fn run<C: Calls>(
    create: impl FnOnce() -> C,
    notifications: Arc<Notifications<C::Kernel>>,
    events: DisplayContextWatchCallback,
    ready: DisplayContextWatchReady,
) {
    let created = catch_unwind(AssertUnwindSafe(|| {
        Owner::new(create(), notifications.clone())
    }));
    let mut owner = match created {
        Ok(owner) => owner,
        Err(_) => {
            let error = if notifications.admits() {
                native(FAILURE)
            } else {
                DisplayContextError::Stopped
            };
            notifications.retire();
            let _ = notifications.signals.close();
            let _ = catch_unwind(AssertUnwindSafe(|| ready(Err(error))));
            return;
        }
    };
    let installed =
        catch_unwind(AssertUnwindSafe(|| owner.install())).unwrap_or(Err(native(FAILURE)));
    if let Err(error) = installed {
        let stopped = !notifications.admits();
        let _ = owner.retire();
        let error = if stopped {
            DisplayContextError::Stopped
        } else {
            error
        };
        let _ = catch_unwind(AssertUnwindSafe(|| ready(Err(error))));
        return;
    }
    if !notifications.admits() {
        let _ = owner.retire();
        let _ = catch_unwind(AssertUnwindSafe(|| {
            ready(Err(DisplayContextError::Stopped))
        }));
        return;
    }
    // Successful readiness itself is admitted here; a concurrent Drop can only
    // retract later events, not change a callback already admitted as success.
    if catch_unwind(AssertUnwindSafe(|| ready(Ok(())))).is_err() {
        let _ = owner.retire();
        return;
    }
    let failure =
        catch_unwind(AssertUnwindSafe(|| owner.pump(&events))).unwrap_or(Some(native(FAILURE)));
    let terminal_admitted = notifications.admits();
    let retirement = owner.retire().err();
    drop(owner);
    if terminal_admitted && let Some(error) = failure.or(retirement) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            events(DisplayContextWatchEvent::Unavailable(error))
        }));
    }
}

#[cfg(windows)]
fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}
