// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One passive WH_MOUSE_LL owner with a copy-only FFI path. The owner and its
//! recording adapter execute the same startup, bounded pump and cleanup.
//! Windows can silently remove a timed-out hook; no idle polling or automatic
//! reinstallation pretends to detect that. Consumers must be cheap enqueuers:
//! a blocking consumer can delay worker cleanup, but guard Drop never joins it.
//! Touch environment is a startup-only system capability snapshot, not a
//! primary-monitor classification. Device/session changes require a fresh watch;
//! this owner has no device invalidation registration or idle observation loop.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use tessera_system::visibility::{
    PhysicalPoint, PointerEnvironment, PointerHost, PointerNativeOperation, PointerWatchCallback,
    PointerWatchCompletion, PointerWatchError, PointerWatchEvent, PointerWatchGuard,
};

use crate::single_flight::{Flight, FlightGate};

const PUMP_BUDGET: usize = 32;
const SAFETY_WAIT_MS: u32 = 250;
const HC_ACTION: i32 = 0;
const WM_MOUSEMOVE: usize = 0x0200;
const LOCAL_FAILURE: u32 = 1;
const UNEXPECTED_PUMP: u32 = 13;
const QUIT_PUMP: u32 = 1223;

mod environment;

struct Startup {
    seed: PhysicalPoint,
    environment: PointerEnvironment,
}

#[cfg(windows)]
mod sdk;

#[cfg(windows)]
pub(crate) fn host() -> Arc<dyn PointerHost> {
    // A Rust-only gate: no handle, thread or hook is created by the factory.
    static GATE: std::sync::LazyLock<FlightGate> = std::sync::LazyLock::new(FlightGate::default);
    host_with(sdk::Factory, ThreadSpawner, GATE.clone())
}

trait StopSignal: Send + Sync + 'static {
    fn signal(&self) -> Result<(), u32>;
}

struct State<S: StopSignal> {
    live: AtomicBool,
    stop: Arc<S>,
    // Drop has no error return and retired consumers must not be re-entered.
    // Preserve a failed signal locally; the safety wake only checks retirement.
    signal_error: AtomicU32,
}

impl<S: StopSignal> State<S> {
    fn admits(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }

    fn retire(&self) {
        self.live.store(false, Ordering::Release);
    }

    fn check_live(&self) -> Result<(), PointerWatchError> {
        if self.admits() {
            Ok(())
        } else {
            Err(PointerWatchError::Stopped)
        }
    }
}

struct Guard<S: StopSignal>(Arc<State<S>>);

impl<S: StopSignal> PointerWatchGuard for Guard<S> {}

impl<S: StopSignal> Drop for Guard<S> {
    fn drop(&mut self) {
        // No join, native teardown, service lock or callback on this thread.
        self.0.retire();
        if let Err(PointerWatchError::Native { code, .. }) =
            native_call(PointerNativeOperation::SignalStopEvent, || {
                self.0.stop.signal()
            })
        {
            self.0.signal_error.store(code, Ordering::Release);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Wait {
    Stop,
    Messages,
    Safety,
}

#[derive(Default)]
struct Hints {
    latest: Option<PhysicalPoint>,
    failed: bool,
}

/// Only these operations are reachable from HOOKPROC. Neither adapter can
/// invoke a consumer through this seam; chaining is outside the panic boundary.
trait HookCalls {
    fn copy_move(&self, lparam: isize);
    fn callback_failed(&self);
    fn chain(&self, code: i32, wparam: usize, lparam: isize) -> isize;
}

fn hook_callback(calls: &impl HookCalls, code: i32, wparam: usize, lparam: isize) -> isize {
    if code < 0 {
        return calls.chain(code, wparam, lparam);
    }
    if code == HC_ACTION
        && wparam == WM_MOUSEMOVE
        && let Err(payload) = catch_unwind(AssertUnwindSafe(|| calls.copy_move(lparam)))
    {
        // A panic payload may itself have a panicking destructor. Never let
        // it unwind through FFI, and still call the next hook exactly once.
        std::mem::forget(payload);
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| calls.callback_failed())) {
            std::mem::forget(payload);
        }
    }
    calls.chain(code, wparam, lparam)
}

/// Native tokens and all thread-local context stay on the installing thread.
/// The stop token is the sole shared native resource; waits borrow its Arc.
trait Calls: 'static {
    type Stop: StopSignal;
    type Hook;
    type DpiContext;
    type Message;

    fn begin_context(&self) -> Result<(), u32>;
    fn retire_context(&self);
    fn end_context(&self);
    fn install_hook(&self) -> Result<Self::Hook, u32>;
    fn remove_hook(&self, hook: Self::Hook) -> Result<(), u32>;
    fn set_dpi_context(&self) -> Result<Self::DpiContext, u32>;
    fn restore_dpi_context(&self, previous: Self::DpiContext) -> Result<(), u32>;
    fn read_cursor(&self) -> Result<PhysicalPoint, u32>;
    fn environment(&self) -> PointerEnvironment;
    fn take_hints(&self) -> Hints;
    fn wait(&self, stop: &Self::Stop, safety_ms: u32) -> Result<Wait, u32>;
    fn next_message(&self) -> Option<Self::Message>;
    fn is_quit(&self, message: &Self::Message) -> bool;
    fn dispatch(&self, message: &Self::Message);
}

trait Factory: Send + Sync + 'static {
    type Calls: Calls;

    fn create_stop(&self) -> Result<Arc<<Self::Calls as Calls>::Stop>, PointerWatchError>;
    fn create_calls(&self) -> Result<Self::Calls, PointerWatchError>;
}

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Failure must drop the job without running or retaining it.
trait Spawner: Send + Sync + 'static {
    fn spawn(&self, job: Job) -> std::io::Result<()>;
}

#[cfg(windows)]
struct ThreadSpawner;

#[cfg(windows)]
impl Spawner for ThreadSpawner {
    fn spawn(&self, job: Job) -> std::io::Result<()> {
        std::thread::Builder::new()
            .name("tessera-pointer-watch".into())
            .spawn(job)
            .map(drop)
    }
}

struct Host<F, S> {
    factory: Arc<F>,
    spawner: S,
    gate: FlightGate,
}

fn host_with<F: Factory, S: Spawner>(
    factory: F,
    spawner: S,
    gate: FlightGate,
) -> Arc<dyn PointerHost> {
    Arc::new(Host {
        factory: Arc::new(factory),
        spawner,
        gate,
    })
}

impl<F: Factory, S: Spawner> PointerHost for Host<F, S> {
    fn watch(
        &self,
        events: PointerWatchCallback,
        ready: PointerWatchCompletion,
    ) -> Result<Box<dyn PointerWatchGuard>, PointerWatchError> {
        let flight = self.gate.try_enter().ok_or(PointerWatchError::Busy)?;
        let stop =
            catch_unwind(AssertUnwindSafe(|| self.factory.create_stop())).unwrap_or_else(|_| {
                Err(native_error(
                    PointerNativeOperation::CreateStopEvent,
                    LOCAL_FAILURE,
                ))
            })?;
        let state = Arc::new(State {
            live: AtomicBool::new(true),
            stop,
            signal_error: AtomicU32::new(0),
        });
        let worker_state = state.clone();
        let factory = self.factory.clone();
        let job = Box::new(move || run(factory.as_ref(), worker_state, flight, events, ready));
        let submission = catch_unwind(AssertUnwindSafe(|| self.spawner.spawn(job)))
            .unwrap_or_else(|_| Err(std::io::Error::other("pointer owner spawn failed")));
        submission.map_err(|error| {
            let code = error
                .raw_os_error()
                .and_then(|code| u32::try_from(code).ok())
                .filter(|code| *code != 0)
                .unwrap_or(LOCAL_FAILURE);
            native_error(PointerNativeOperation::StartThread, code)
        })?;
        Ok(Box::new(Guard(state)))
    }
}

struct Owner<C: Calls> {
    calls: C,
    state: Arc<State<C::Stop>>,
    hook: Option<C::Hook>,
    dpi: Option<C::DpiContext>,
    context: bool,
    // Last field: admission outlives native cleanup, adapter and worker handle
    // references, including startup failure and panics.
    _flight: Flight,
}

impl<C: Calls> Owner<C> {
    fn new(calls: C, state: Arc<State<C::Stop>>, flight: Flight) -> Self {
        Self {
            calls,
            state,
            hook: None,
            dpi: None,
            context: false,
            _flight: flight,
        }
    }

    fn start(&mut self) -> Result<Startup, PointerWatchError> {
        self.state.check_live()?;
        self.context = true;
        native_call(PointerNativeOperation::Callback, || {
            self.calls.begin_context()
        })?;
        self.state.check_live()?;
        // Sample system-wide touch once, before installing the time-sensitive
        // hook and outside the cursor DPI override. Failure stays unknown.
        let environment = catch_unwind(AssertUnwindSafe(|| self.calls.environment())).unwrap_or(
            PointerEnvironment {
                touch_capable: None,
            },
        );
        self.state.check_live()?;
        self.hook = Some(native_call(PointerNativeOperation::InstallHook, || {
            self.calls.install_hook()
        })?);
        // A successful late registration is owned before observing cancellation.
        self.state.check_live()?;
        self.dpi = Some(native_call(PointerNativeOperation::SetDpiContext, || {
            self.calls.set_dpi_context()
        })?);
        self.state.check_live()?;
        // Exactly one startup read, never a pump/idle read. Restore is checked
        // even when the read fails; neither failure can publish a default point.
        let seed = native_call(PointerNativeOperation::ReadCursor, || {
            self.calls.read_cursor()
        });
        self.restore_dpi()?;
        let seed = seed?;
        self.state.check_live()?;
        Ok(Startup { seed, environment })
    }

    fn restore_dpi(&mut self) -> Result<(), PointerWatchError> {
        if let Some(previous) = self.dpi.take() {
            native_call(PointerNativeOperation::RestoreDpiContext, || {
                self.calls.restore_dpi_context(previous)
            })?;
        }
        Ok(())
    }

    fn hints(&self) -> Result<Option<PhysicalPoint>, PointerWatchError> {
        let hints = native_call(PointerNativeOperation::Callback, || {
            Ok(self.calls.take_hints())
        })?;
        if hints.failed {
            Err(native_error(
                PointerNativeOperation::Callback,
                LOCAL_FAILURE,
            ))
        } else {
            Ok(hints.latest)
        }
    }

    fn pump(&self) -> Result<(), PointerWatchError> {
        self.state.check_live()?;
        let wait = native_call(PointerNativeOperation::MessagePump, || {
            self.calls.wait(&self.state.stop, SAFETY_WAIT_MS)
        })?;
        self.state.check_live()?;
        match wait {
            Wait::Safety => {}
            Wait::Stop => {
                return Err(native_error(
                    PointerNativeOperation::MessagePump,
                    UNEXPECTED_PUMP,
                ));
            }
            Wait::Messages => {
                for _ in 0..PUMP_BUDGET {
                    self.state.check_live()?;
                    let message = native_call(PointerNativeOperation::MessagePump, || {
                        Ok(self.calls.next_message())
                    })?;
                    self.state.check_live()?;
                    let Some(message) = message else { break };
                    if self.calls.is_quit(&message) {
                        return Err(native_error(PointerNativeOperation::MessagePump, QUIT_PUMP));
                    }
                    native_call(PointerNativeOperation::MessagePump, || {
                        self.calls.dispatch(&message);
                        Ok(())
                    })?;
                    self.state.check_live()?;
                }
            }
        }
        self.state.check_live()
    }

    fn watch(
        &self,
        startup: Startup,
        events: &PointerWatchCallback,
    ) -> Result<(), PointerWatchError> {
        self.state.check_live()?;
        native_call(PointerNativeOperation::Callback, || {
            events(PointerWatchEvent::Environment(startup.environment));
            Ok(())
        })?;
        self.deliver(events, startup.seed)?;
        loop {
            self.state.check_live()?;
            // `take_hints` has returned: no TLS reference/borrow or FFI frame
            // spans consumer invocation. A batch coalesces to its latest point.
            if let Some(point) = self.hints()? {
                self.deliver(events, point)?;
            }
            self.pump()?;
        }
    }

    fn deliver(
        &self,
        events: &PointerWatchCallback,
        point: PhysicalPoint,
    ) -> Result<(), PointerWatchError> {
        self.state.check_live()?;
        native_call(PointerNativeOperation::Callback, || {
            events(PointerWatchEvent::Position(point));
            Ok(())
        })
    }

    fn close(&mut self) -> Result<(), PointerWatchError> {
        self.state.retire();
        let mut result = Ok(());
        if self.context {
            result = native_call(PointerNativeOperation::Callback, || {
                self.calls.retire_context();
                Ok(())
            });
        }
        if let Some(hook) = self.hook.take() {
            let removed = native_call(PointerNativeOperation::RemoveHook, || {
                self.calls.remove_hook(hook)
            });
            result = result.and(removed);
        }
        let restored = self.restore_dpi();
        result = result.and(restored);
        if std::mem::take(&mut self.context) {
            let ended = native_call(PointerNativeOperation::Callback, || {
                self.calls.end_context();
                Ok(())
            });
            result = result.and(ended);
        }
        result
    }
}

impl<C: Calls> Drop for Owner<C> {
    fn drop(&mut self) {
        // Explicit close normally reports errors; this idempotent fallback also
        // owns teardown when a consumer/native adapter unwinds unexpectedly.
        let _ = self.close();
    }
}

fn run<F: Factory>(
    factory: &F,
    state: Arc<State<<F::Calls as Calls>::Stop>>,
    flight: Flight,
    events: PointerWatchCallback,
    ready: PointerWatchCompletion,
) {
    let calls = catch_unwind(AssertUnwindSafe(|| {
        state.check_live()?;
        factory.create_calls()
    }))
    .unwrap_or_else(|_| {
        Err(native_error(
            PointerNativeOperation::Callback,
            LOCAL_FAILURE,
        ))
    });
    let calls = match calls {
        Ok(calls) => calls,
        Err(error) => {
            state.retire();
            drop(flight);
            let _ = catch_unwind(AssertUnwindSafe(|| ready(Err(error))));
            return;
        }
    };
    let mut owner = Owner::new(calls, state.clone(), flight);
    let startup = owner
        .start()
        .and_then(|startup| state.check_live().map(|()| startup));
    let startup = match startup {
        Ok(startup) => startup,
        Err(error) => {
            let cleanup = owner.close();
            drop(owner);
            // Cancellation has a fixed readiness result, even if native removal
            // reports an error after the successful late hook was retired.
            let error = if matches!(error, PointerWatchError::Stopped) {
                PointerWatchError::Stopped
            } else {
                cleanup.err().unwrap_or(error)
            };
            let _ = catch_unwind(AssertUnwindSafe(|| ready(Err(error))));
            return;
        }
    };
    if catch_unwind(AssertUnwindSafe(|| ready(Ok(())))).is_err() {
        let _ = owner.close();
        drop(owner);
        return;
    }
    let failure = catch_unwind(AssertUnwindSafe(|| owner.watch(startup, &events)))
        .unwrap_or_else(|_| {
            Err(native_error(
                PointerNativeOperation::MessagePump,
                LOCAL_FAILURE,
            ))
        })
        .err();
    // Reserve terminal admission before retirement; a guard race can leave an
    // already admitted callback in flight, as allowed by the portable contract.
    let terminal_admitted = state.admits();
    let cleanup = owner.close();
    drop(owner);
    let failure = cleanup.err().or(failure);
    if terminal_admitted && let Some(error) = failure {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            events(PointerWatchEvent::Failed(error))
        }));
    }
}

fn native_error(operation: PointerNativeOperation, code: u32) -> PointerWatchError {
    PointerWatchError::Native { operation, code }
}

fn native_call<T>(
    operation: PointerNativeOperation,
    call: impl FnOnce() -> Result<T, u32>,
) -> Result<T, PointerWatchError> {
    catch_unwind(AssertUnwindSafe(call))
        .unwrap_or(Err(LOCAL_FAILURE))
        .map_err(|code| native_error(operation, if code == 0 { LOCAL_FAILURE } else { code }))
}

#[cfg(test)]
mod tests;
