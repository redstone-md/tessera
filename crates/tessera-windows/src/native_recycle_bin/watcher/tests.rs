// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::collections::VecDeque;
use std::rc::Weak;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::{ThreadId, current};
use std::time::Duration;

use parking_lot::Mutex;

use super::*;

const DEADLINE: Duration = Duration::from_secs(5);
const WINDOW: usize = 123;
const SHARED_HANDLE: usize = 0xCAFE;
const SENDER_PID: u32 = 77;
const LOCK: u64 = 456;
const COOKIE: u32 = 321;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Event {
    Initialize(i32),
    Resolve(u128),
    Window(u32),
    Register(Registration),
    Lock(usize, u32),
    Unlock(u64),
    ReadyStart,
    ReadyEnd,
    Consumer(RecycleBinWatchEvent),
    Wait(u32),
    Pump(usize),
    SignalStop,
    RetireToken,
    Deregister(u32),
    Detach,
    Destroy,
    ContextFree,
    PidlFree,
    Uninitialize,
    DriverDrop,
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<(ThreadId, Event)>>>);
impl Log {
    fn push(&self, event: Event) {
        self.0.lock().push((current().id(), event));
    }
    fn events(&self) -> Vec<Event> {
        self.0
            .lock()
            .iter()
            .map(|(_, event)| event.clone())
            .collect()
    }
    fn assert_owner(&self) {
        let log = self.0.lock();
        let owner = log
            .iter()
            .find(|(_, event)| *event == Event::Initialize(2))
            .unwrap()
            .0;
        assert_ne!(owner, current().id());
        assert!(
            log.iter()
                .filter(|(_, event)| *event != Event::SignalStop)
                .all(|(thread, _)| *thread == owner)
        );
    }
}

#[derive(Clone, Copy)]
struct Notice {
    event: i32,
    lock_ok: bool,
    unlock_ok: bool,
    panic_lock: bool,
    panic_unlock: bool,
}
impl Default for Notice {
    fn default() -> Self {
        Self {
            event: 0x1000,
            lock_ok: true,
            unlock_ok: true,
            panic_lock: false,
            panic_unlock: false,
        }
    }
}

struct NoticeCalls {
    log: Log,
    notice: Notice,
}
impl NotificationCalls for NoticeCalls {
    type Lock = u64;
    fn lock(&self, handle: usize, process: u32) -> Option<(u64, i32)> {
        self.log.push(Event::Lock(handle, process));
        assert!(!self.notice.panic_lock, "recording lock panic");
        self.notice.lock_ok.then_some((LOCK, self.notice.event))
    }
    fn unlock(&self, lock: &u64) -> bool {
        self.log.push(Event::Unlock(*lock));
        assert!(!self.notice.panic_unlock, "recording unlock panic");
        self.notice.unlock_ok
    }
}

enum Action {
    Notice(Notice),
    Wait(Wait),
    Quit,
    PanicPump,
}

struct Stop {
    commands: Sender<Action>,
    log: Log,
    fail_signal: bool,
}
impl StopSignal for Stop {
    fn signal(&self) {
        self.log.push(Event::SignalStop);
        if !self.fail_signal {
            let _ = self.commands.send(Action::Wait(Wait::Stop));
        }
    }
}

#[derive(Clone, Copy)]
struct Plan {
    initialization: i32,
    resolve_error: Option<u32>,
    null_pidl: bool,
    window_error: Option<u32>,
    cookie: u32,
    initial_dirty: bool,
    fail_signal: bool,
    panic_factory: bool,
    detach_ok: bool,
    destroy_ok: bool,
}
impl Default for Plan {
    fn default() -> Self {
        Self {
            initialization: 0,
            resolve_error: None,
            null_pidl: false,
            window_error: None,
            cookie: COOKIE,
            initial_dirty: false,
            fail_signal: false,
            panic_factory: false,
            detach_ok: true,
            destroy_ok: true,
        }
    }
}

struct Pidl {
    log: Log,
    _thread_bound: Rc<()>,
}
impl Drop for Pidl {
    fn drop(&mut self) {
        self.log.push(Event::PidlFree);
    }
}

struct Window {
    context: Option<Rc<WindowContext>>,
    contexts: Rc<Contexts>,
    log: Log,
    detach_ok: bool,
    destroy_ok: bool,
}
struct Teardown<'a> {
    window: &'a Window,
    context: Weak<WindowContext>,
    token: usize,
}
impl WindowTeardown for Teardown<'_> {
    fn detach(&self) -> bool {
        assert!(self.context.upgrade().is_some());
        assert!(self.window.contexts.lookup(WINDOW, self.token).is_none());
        self.window.log.push(Event::Detach);
        self.window.detach_ok
    }
    fn destroy(&self) -> bool {
        // Synchronous native reentry can no longer look up the retired token,
        // but the owner's context is still alive throughout destruction.
        assert!(self.context.upgrade().is_some());
        assert!(self.window.contexts.lookup(WINDOW, self.token).is_none());
        self.window.log.push(Event::Destroy);
        self.window.destroy_ok
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        let context = self.context.as_ref().unwrap();
        let weak = Rc::downgrade(context);
        let token = context.token;
        self.contexts.retire(WINDOW, token);
        let _ = teardown_window(&Teardown {
            window: self,
            context: weak.clone(),
            token,
        });
        drop(self.context.take());
        assert!(weak.upgrade().is_none());
        assert!(self.contexts.lookup(WINDOW, token).is_none());
        self.log.push(Event::ContextFree);
    }
}

struct RecordingCalls {
    plan: Plan,
    log: Log,
    contexts: Rc<Contexts>,
    token: Cell<usize>,
    actions: Receiver<Action>,
    pending: RefCell<Option<Action>>,
    cleanup: Sender<()>,
    waiting: Sender<()>,
}
impl Drop for RecordingCalls {
    fn drop(&mut self) {
        self.log.push(Event::DriverDrop);
        let _ = self.cleanup.send(());
    }
}
impl Calls<Stop> for RecordingCalls {
    type Pidl = Pidl;
    type Window = Window;
    fn initialize(&self, model: i32) -> i32 {
        self.log.push(Event::Initialize(model));
        self.plan.initialization
    }
    fn resolve(&self, folder: u128) -> Result<Option<Pidl>, u32> {
        self.log.push(Event::Resolve(folder));
        if let Some(error) = self.plan.resolve_error {
            return Err(error);
        }
        Ok((!self.plan.null_pidl).then(|| Pidl {
            log: self.log.clone(),
            _thread_bound: Rc::new(()),
        }))
    }
    fn create_window(&self, message: u32) -> Result<Window, u32> {
        self.log.push(Event::Window(message));
        if let Some(error) = self.plan.window_error {
            return Err(error);
        }
        let context = self.contexts.install(WINDOW)?;
        self.token.set(context.token);
        Ok(Window {
            context: Some(context),
            contexts: self.contexts.clone(),
            log: self.log.clone(),
            detach_ok: self.plan.detach_ok,
            destroy_ok: self.plan.destroy_ok,
        })
    }
    fn register(&self, window: &Window, _: &Pidl, registration: Registration) -> u32 {
        self.log.push(Event::Register(registration));
        assert_eq!(
            (
                registration.sources,
                registration.mask,
                registration.message,
                registration.entries,
                registration.recursive
            ),
            (0x8002, 0x2381F, NOTIFY_MESSAGE, 1, true)
        );
        if self.plan.initial_dirty {
            for _ in 0..3 {
                notification(
                    &NoticeCalls {
                        log: self.log.clone(),
                        notice: Notice::default(),
                    },
                    &window.context.as_ref().unwrap().flags,
                    SHARED_HANDLE,
                    SENDER_PID,
                );
            }
        }
        self.plan.cookie
    }
    fn hints(&self, window: &Window) -> Hints {
        window.context.as_ref().unwrap().flags.take()
    }
    fn wait(&self, _: &Stop, safety_ms: u32) -> Wait {
        self.log.push(Event::Wait(safety_ms));
        let _ = self.waiting.send(());
        match self
            .actions
            .recv_timeout(Duration::from_millis(u64::from(safety_ms)))
        {
            Ok(Action::Wait(wait)) => wait,
            Ok(action) => {
                *self.pending.borrow_mut() = Some(action);
                Wait::Messages
            }
            Err(RecvTimeoutError::Timeout) => Wait::Safety,
            Err(RecvTimeoutError::Disconnected) => Wait::Failed,
        }
    }
    fn pump(&self, window: &Window, budget: usize) -> bool {
        self.log.push(Event::Pump(budget));
        let action = self.pending.borrow_mut().take();
        match action {
            Some(Action::Notice(notice)) => {
                notification(
                    &NoticeCalls {
                        log: self.log.clone(),
                        notice,
                    },
                    &window.context.as_ref().unwrap().flags,
                    SHARED_HANDLE,
                    SENDER_PID,
                );
                true
            }
            Some(Action::Quit) => false,
            Some(Action::PanicPump) => panic!("recording pump panic"),
            _ => true,
        }
    }
    fn retire(&self, window: &Window) {
        self.log.push(Event::RetireToken);
        self.contexts
            .retire(WINDOW, window.context.as_ref().unwrap().token);
    }
    fn deregister(&self, cookie: u32) {
        assert!(self.contexts.lookup(WINDOW, self.token.get()).is_none());
        self.log.push(Event::Deregister(cookie));
    }
    fn uninitialize(&self) {
        self.log.push(Event::Uninitialize);
    }
}

struct Fixture {
    guard: Box<dyn RecycleBinWatchGuard>,
    commands: Sender<Action>,
    cleanup: Receiver<()>,
    waiting: Receiver<()>,
    log: Log,
}
fn fixture(
    plan: Plan,
    events: RecycleBinWatchCallback,
    ready: RecycleBinWatchCompletion,
) -> Fixture {
    let log = Log::default();
    let (commands, actions) = channel();
    let (cleanup_done, cleanup) = channel();
    let (waiting_done, waiting) = channel();
    let stop = Arc::new(Stop {
        commands: commands.clone(),
        log: log.clone(),
        fail_signal: plan.fail_signal,
    });
    let factory_log = log.clone();
    let event_log = log.clone();
    let events: RecycleBinWatchCallback = Arc::new(move |event| {
        event_log.push(Event::Consumer(event));
        events(event);
    });
    let ready_log = log.clone();
    let ready: RecycleBinWatchCompletion = Box::new(move |result| {
        ready_log.push(Event::ReadyStart);
        ready(result);
        ready_log.push(Event::ReadyEnd);
    });
    let guard = start(
        stop,
        move || {
            assert!(!plan.panic_factory, "recording factory panic");
            Ok(RecordingCalls {
                plan,
                log: factory_log,
                contexts: Rc::new(Contexts::default()),
                token: Cell::new(0),
                actions,
                pending: RefCell::new(None),
                cleanup: cleanup_done,
                waiting: waiting_done,
            })
        },
        events,
        ready,
    )
    .unwrap();
    Fixture {
        guard,
        commands,
        cleanup,
        waiting,
        log,
    }
}

fn ordinary(
    plan: Plan,
) -> (
    Fixture,
    Receiver<Result<(), DockUtilityError>>,
    Receiver<RecycleBinWatchEvent>,
) {
    let (ready_done, ready) = channel();
    let (event_done, events) = channel();
    let fixture = fixture(
        plan,
        Arc::new(move |event| {
            event_done.send(event).unwrap();
        }),
        Box::new(move |result| {
            ready_done.send(result).unwrap();
        }),
    );
    (fixture, ready, events)
}

#[test]
fn fixed_scope_positive_cookie_ready_then_one_preserved_dirty_hint() {
    let (ready_done, ready) = channel();
    let (event_done, events) = channel();
    let ready_events = Arc::new(Mutex::new(events));
    let checking_events = ready_events.clone();
    let fixture = fixture(
        Plan {
            initial_dirty: true,
            ..Default::default()
        },
        Arc::new(move |event| {
            event_done.send(event).unwrap();
        }),
        Box::new(move |result| {
            assert!(checking_events.lock().try_recv().is_err());
            ready_done.send(result).unwrap();
            assert!(checking_events.lock().try_recv().is_err());
        }),
    );
    ready.recv_timeout(DEADLINE).unwrap().unwrap();
    assert!(matches!(
        ready.recv_timeout(DEADLINE),
        Err(RecvTimeoutError::Disconnected)
    ));
    assert_eq!(
        ready_events.lock().recv_timeout(DEADLINE).unwrap(),
        RecycleBinWatchEvent::Invalidated
    );
    assert!(ready_events.lock().try_recv().is_err());
    drop(fixture.guard);
    fixture.cleanup.recv_timeout(DEADLINE).unwrap();
    let log = fixture.log.events();
    assert_eq!(
        &log[..4],
        &[
            Event::Initialize(2),
            Event::Resolve(RECYCLE_FOLDER),
            Event::Window(NOTIFY_MESSAGE),
            Event::Register(REGISTRATION)
        ]
    );
    assert_eq!(
        log.iter()
            .filter(|event| **event == Event::Deregister(COOKIE))
            .count(),
        1
    );
    let retired = log
        .iter()
        .position(|event| *event == Event::RetireToken)
        .unwrap();
    let tail: Vec<_> = log[retired..]
        .iter()
        .filter(|event| **event != Event::SignalStop)
        .cloned()
        .collect();
    assert_eq!(
        tail,
        [
            Event::RetireToken,
            Event::Deregister(COOKIE),
            Event::Detach,
            Event::Destroy,
            Event::ContextFree,
            Event::PidlFree,
            Event::Uninitialize,
            Event::DriverDrop
        ]
    );
    fixture.log.assert_owner();
}

#[test]
fn startup_failures_are_ready_errors_not_events_and_never_broaden_scope() {
    for (plan, expected, expected_balance, registered) in [
        (
            Plan {
                initialization: 0x8001_0106_u32 as i32,
                ..Default::default()
            },
            DockUtilityErrorKind::Other,
            0,
            false,
        ),
        (
            Plan {
                resolve_error: Some(0x8007_0005),
                ..Default::default()
            },
            DockUtilityErrorKind::AccessDenied,
            1,
            false,
        ),
        (
            Plan {
                null_pidl: true,
                ..Default::default()
            },
            DockUtilityErrorKind::Other,
            1,
            false,
        ),
        (
            Plan {
                window_error: Some(0x8007_0005),
                ..Default::default()
            },
            DockUtilityErrorKind::AccessDenied,
            1,
            false,
        ),
        (
            Plan {
                cookie: 0,
                ..Default::default()
            },
            DockUtilityErrorKind::Unavailable,
            1,
            true,
        ),
    ] {
        let (fixture, ready, events) = ordinary(plan);
        assert_eq!(
            ready.recv_timeout(DEADLINE).unwrap().unwrap_err().kind,
            expected
        );
        assert!(matches!(
            ready.recv_timeout(DEADLINE),
            Err(RecvTimeoutError::Disconnected)
        ));
        fixture.cleanup.recv_timeout(DEADLINE).unwrap();
        assert!(events.try_recv().is_err());
        let log = fixture.log.events();
        assert_eq!(
            log.iter()
                .filter(|event| **event == Event::Uninitialize)
                .count(),
            expected_balance
        );
        assert_eq!(
            log.iter().any(|event| matches!(event, Event::Register(_))),
            registered
        );
        assert!(
            !log.iter()
                .any(|event| matches!(event, Event::Deregister(_)))
        );
        drop(fixture.guard);
    }
}

#[test]
fn accepted_guard_drop_during_startup_completes_stopped_once_without_native_start() {
    let (commands, actions) = channel();
    let (entered, entry) = channel();
    let (release, gate) = channel();
    let (ready_done, ready) = channel();
    let (cleanup_done, cleanup) = channel();
    let (waiting, _) = channel();
    let log = Log::default();
    let factory_log = log.clone();
    let guard = start(
        Arc::new(Stop {
            commands,
            log: log.clone(),
            fail_signal: false,
        }),
        move || {
            entered.send(()).unwrap();
            gate.recv_timeout(DEADLINE).unwrap();
            Ok(RecordingCalls {
                plan: Plan::default(),
                log: factory_log,
                contexts: Rc::new(Contexts::default()),
                token: Cell::new(0),
                actions,
                pending: RefCell::new(None),
                cleanup: cleanup_done,
                waiting,
            })
        },
        Arc::new(|_| panic!("event after stopped startup")),
        Box::new(move |result| {
            ready_done.send(result).unwrap();
        }),
    )
    .unwrap();
    entry.recv_timeout(DEADLINE).unwrap();
    drop(guard); // returns before startup gate is released; no native join.
    release.send(()).unwrap();
    assert_eq!(
        ready.recv_timeout(DEADLINE).unwrap().unwrap_err().kind,
        DockUtilityErrorKind::Stopped
    );
    assert!(matches!(
        ready.recv_timeout(DEADLINE),
        Err(RecvTimeoutError::Disconnected)
    ));
    cleanup.recv_timeout(DEADLINE).unwrap();
    assert_eq!(log.events(), [Event::SignalStop, Event::DriverDrop]);
}

#[test]
fn factory_and_ready_panics_are_contained_without_events() {
    let (failed, ready, events) = ordinary(Plan {
        panic_factory: true,
        ..Default::default()
    });
    assert_eq!(
        ready.recv_timeout(DEADLINE).unwrap().unwrap_err().kind,
        DockUtilityErrorKind::Other
    );
    assert!(matches!(
        ready.recv_timeout(DEADLINE),
        Err(RecvTimeoutError::Disconnected)
    ));
    assert!(events.try_recv().is_err());
    drop(failed.guard);
    let fixture = fixture(
        Plan::default(),
        Arc::new(|_| panic!("event after panicked ready")),
        Box::new(|result| {
            result.unwrap();
            panic!("recording ready panic");
        }),
    );
    fixture.cleanup.recv_timeout(DEADLINE).unwrap();
    assert!(fixture.log.events().contains(&Event::Deregister(COOKIE)));
    drop(fixture.guard);
}

#[test]
fn callback_reentry_drops_guard_and_retires_late_queued_events() {
    let guard_slot: Arc<Mutex<Option<Box<dyn RecycleBinWatchGuard>>>> = Arc::new(Mutex::new(None));
    let callback_guard = guard_slot.clone();
    let (done, result) = channel();
    let (ready_done, ready) = channel();
    let fixture = fixture(
        Plan::default(),
        Arc::new(move |event| {
            assert_eq!(event, RecycleBinWatchEvent::Invalidated);
            drop(callback_guard.lock().take());
            done.send(()).unwrap();
        }),
        Box::new(move |result| {
            ready_done.send(result).unwrap();
        }),
    );
    *guard_slot.lock() = Some(fixture.guard);
    ready.recv_timeout(DEADLINE).unwrap().unwrap();
    fixture
        .commands
        .send(Action::Notice(Notice::default()))
        .unwrap();
    for _ in 0..2 {
        let _ = fixture.commands.send(Action::Notice(Notice::default()));
    }
    result.recv_timeout(DEADLINE).unwrap();
    fixture.cleanup.recv_timeout(DEADLINE).unwrap();
    assert!(result.try_recv().is_err());
    let log = fixture.log.events();
    assert_eq!(
        log.iter()
            .filter(|event| matches!(event, Event::Lock(_, _)))
            .count(),
        1
    );
    let unlock = log
        .iter()
        .position(|event| *event == Event::Unlock(LOCK))
        .unwrap();
    let signal = log
        .iter()
        .position(|event| *event == Event::SignalStop)
        .unwrap();
    assert!(unlock < signal);
}

#[test]
fn ready_callback_reentry_can_retire_guard_without_publishing_pending_dirty() {
    let slot: Arc<Mutex<Option<Box<dyn RecycleBinWatchGuard>>>> = Arc::new(Mutex::new(None));
    let callback_slot = slot.clone();
    let (release, gate) = channel();
    let (ready_done, ready) = channel();
    let (event_done, events) = channel();
    let fixture = fixture(
        Plan {
            initial_dirty: true,
            ..Default::default()
        },
        Arc::new(move |event| {
            event_done.send(event).unwrap();
        }),
        Box::new(move |result| {
            result.unwrap();
            gate.recv_timeout(DEADLINE).unwrap();
            drop(callback_slot.lock().take());
            ready_done.send(()).unwrap();
        }),
    );
    *slot.lock() = Some(fixture.guard);
    release.send(()).unwrap();
    ready.recv_timeout(DEADLINE).unwrap();
    fixture.cleanup.recv_timeout(DEADLINE).unwrap();
    assert!(events.try_recv().is_err());
    assert_eq!(
        fixture
            .log
            .events()
            .iter()
            .filter(|event| **event == Event::ReadyStart)
            .count(),
        1
    );
}

#[test]
fn panicked_event_consumer_does_not_unwind_owner_or_strand_later_delivery() {
    let (done, result) = channel();
    let (ready_done, ready) = channel();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_calls = calls.clone();
    let fixture = fixture(
        Plan::default(),
        Arc::new(move |event| {
            assert_eq!(event, RecycleBinWatchEvent::Invalidated);
            let call = callback_calls.fetch_add(1, Ordering::SeqCst);
            done.send(call).unwrap();
            assert_ne!(call, 0, "recording consumer panic");
        }),
        Box::new(move |result| {
            ready_done.send(result).unwrap();
        }),
    );
    ready.recv_timeout(DEADLINE).unwrap().unwrap();
    fixture
        .commands
        .send(Action::Notice(Notice::default()))
        .unwrap();
    assert_eq!(result.recv_timeout(DEADLINE).unwrap(), 0);
    fixture
        .commands
        .send(Action::Notice(Notice::default()))
        .unwrap();
    assert_eq!(result.recv_timeout(DEADLINE).unwrap(), 1);
    drop(fixture.guard);
    fixture.cleanup.recv_timeout(DEADLINE).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn runtime_failures_are_terminal_typed_once_after_native_cleanup() {
    for action in [
        Action::Wait(Wait::Failed),
        Action::Wait(Wait::Unexpected),
        Action::Wait(Wait::Stop),
        Action::Quit,
        Action::PanicPump,
        Action::Notice(Notice {
            lock_ok: false,
            ..Default::default()
        }),
        Action::Notice(Notice {
            unlock_ok: false,
            ..Default::default()
        }),
    ] {
        let (fixture, ready, events) = ordinary(Plan::default());
        ready.recv_timeout(DEADLINE).unwrap().unwrap();
        fixture.commands.send(action).unwrap();
        assert_eq!(
            events.recv_timeout(DEADLINE).unwrap(),
            RecycleBinWatchEvent::Unavailable(DockUtilityErrorKind::Unavailable)
        );
        assert!(matches!(
            events.recv_timeout(DEADLINE),
            Err(RecvTimeoutError::Disconnected)
        ));
        fixture.cleanup.recv_timeout(DEADLINE).unwrap();
        assert_eq!(
            fixture
                .log
                .events()
                .iter()
                .filter(|event| **event == Event::Deregister(COOKIE))
                .count(),
            1
        );
        let log = fixture.log.events();
        let cleanup = log
            .iter()
            .position(|event| *event == Event::DriverDrop)
            .unwrap();
        let unavailable = log
            .iter()
            .position(|event| {
                matches!(event, Event::Consumer(RecycleBinWatchEvent::Unavailable(_)))
            })
            .unwrap();
        assert!(cleanup < unavailable);
        drop(fixture.guard);
    }
}

#[test]
fn failed_shutdown_signal_uses_only_local_250ms_retirement_safety() {
    let (fixture, ready, events) = ordinary(Plan {
        fail_signal: true,
        ..Default::default()
    });
    ready.recv_timeout(DEADLINE).unwrap().unwrap();
    // Wait until owner is in the fake combined wait, not inside native startup.
    fixture.waiting.recv_timeout(DEADLINE).unwrap();
    drop(fixture.guard);
    fixture.cleanup.recv_timeout(DEADLINE).unwrap();
    assert!(events.try_recv().is_err());
    let log = fixture.log.events();
    assert!(
        log.iter()
            .filter_map(|event| if let Event::Wait(ms) = event {
                Some(*ms)
            } else {
                None
            })
            .all(|ms| ms == 250)
    );
    assert!(!log.iter().any(|event| matches!(event, Event::Pump(_))));
}

#[test]
fn shared_message_lock_owns_one_unlock_and_never_uses_registration_cookie() {
    for notice in [
        Notice::default(),
        Notice {
            lock_ok: false,
            ..Default::default()
        },
        Notice {
            unlock_ok: false,
            ..Default::default()
        },
        Notice {
            event: 0x40000,
            ..Default::default()
        },
        Notice {
            panic_lock: true,
            ..Default::default()
        },
        Notice {
            panic_unlock: true,
            ..Default::default()
        },
    ] {
        let log = Log::default();
        let flags = Flags::default();
        notification(
            &NoticeCalls {
                log: log.clone(),
                notice,
            },
            &flags,
            SHARED_HANDLE,
            SENDER_PID,
        );
        let hints = flags.take();
        let locked = notice.lock_ok && !notice.panic_lock;
        assert_eq!(hints.dirty, locked && notice.event as u32 & ITEM_MASK != 0);
        assert_eq!(
            hints.failed,
            !locked || !notice.unlock_ok || notice.panic_unlock
        );
        let expected = if locked {
            vec![Event::Lock(SHARED_HANDLE, SENDER_PID), Event::Unlock(LOCK)]
        } else {
            vec![Event::Lock(SHARED_HANDLE, SENDER_PID)]
        };
        assert_eq!(log.events(), expected);
        assert_ne!(SHARED_HANDLE, COOKIE as usize);
    }
}

#[test]
fn opaque_tokens_reject_queued_forged_retired_and_reused_window_delivery() {
    let contexts = Contexts::default();
    let context = contexts.install(WINDOW).unwrap();
    let token = context.token;
    let weak = Rc::downgrade(&context);
    assert_ne!(token, 0);
    assert!(contexts.lookup(WINDOW, 0).is_none());
    assert!(contexts.lookup(WINDOW + 1, token).is_none());
    assert!(
        contexts
            .lookup(WINDOW, token.checked_add(1).unwrap())
            .is_none()
    );
    let in_flight = contexts.lookup(WINDOW, token).unwrap();
    // Lookup holds no RefCell borrow: native reentry can retire immediately.
    contexts.retire(WINDOW, token);
    assert!(contexts.lookup(WINDOW, token).is_none());
    drop(context);
    assert!(weak.upgrade().is_some());
    in_flight.flags.dirty.set(true);
    assert!(in_flight.flags.take().dirty);
    drop(in_flight);
    assert!(weak.upgrade().is_none());
    let replacement = contexts.install(WINDOW).unwrap();
    assert_ne!(replacement.token, token);
    assert!(contexts.lookup(WINDOW, token).is_none());
    assert!(contexts.install(WINDOW).is_err());
    contexts.retire(WINDOW, token); // stale retirement cannot remove replacement.
    assert!(contexts.lookup(WINDOW, replacement.token).is_some());
    contexts.retire(WINDOW, replacement.token);
}

#[test]
fn context_survives_teardown_reentry_even_failed_detach_and_destroy() {
    for (detach_ok, destroy_ok) in [(true, true), (false, true), (true, false), (false, false)] {
        let (fixture, ready, _) = ordinary(Plan {
            detach_ok,
            destroy_ok,
            ..Default::default()
        });
        ready.recv_timeout(DEADLINE).unwrap().unwrap();
        drop(fixture.guard);
        fixture.cleanup.recv_timeout(DEADLINE).unwrap();
        let log = fixture.log.events();
        let retire = log
            .iter()
            .position(|event| *event == Event::RetireToken)
            .unwrap();
        let tail: Vec<_> = log[retire..]
            .iter()
            .filter(|event| **event != Event::SignalStop)
            .cloned()
            .collect();
        assert_eq!(
            tail,
            [
                Event::RetireToken,
                Event::Deregister(COOKIE),
                Event::Detach,
                Event::Destroy,
                Event::ContextFree,
                Event::PidlFree,
                Event::Uninitialize,
                Event::DriverDrop
            ]
        );
    }
}

struct Queue {
    messages: RefCell<VecDeque<u32>>,
    dispatched: RefCell<Vec<u32>>,
}
impl MessageCalls for Queue {
    type Message = u32;
    fn next(&self) -> Option<u32> {
        self.messages.borrow_mut().pop_front()
    }
    fn is_quit(&self, message: &u32) -> bool {
        *message == 18
    }
    fn dispatch(&self, message: &u32) {
        self.dispatched.borrow_mut().push(*message);
    }
}

#[test]
fn combined_wait_decode_and_bounded_all_message_pump_handle_terminal_quit() {
    assert_eq!(
        [0, 1, 258, u32::MAX, 2, 192].map(decode_wait),
        [
            Wait::Stop,
            Wait::Messages,
            Wait::Safety,
            Wait::Failed,
            Wait::Unexpected,
            Wait::Unexpected
        ]
    );
    let queue = Queue {
        messages: RefCell::new((100..150).collect()),
        dispatched: RefCell::new(Vec::new()),
    };
    assert!(pump_messages(&queue, PUMP_BUDGET));
    assert_eq!(queue.dispatched.borrow().len(), 32);
    assert_eq!(queue.messages.borrow().len(), 18);
    queue.messages.borrow_mut().push_front(18);
    assert!(!pump_messages(&queue, PUMP_BUDGET));
    assert_eq!(queue.dispatched.borrow().len(), 32);
}

#[cfg(windows)]
#[test]
fn pinned_watch_sdk_constants_cover_fixed_namespace_new_delivery_and_updates() {
    use windows::Win32::UI::Shell::{
        FOLDERID_RecycleBinFolder, SHCNE_ATTRIBUTES, SHCNE_CREATE, SHCNE_DELETE, SHCNE_MKDIR,
        SHCNE_RENAMEFOLDER, SHCNE_RENAMEITEM, SHCNE_RMDIR, SHCNE_UPDATEDIR, SHCNE_UPDATEITEM,
        SHCNRF_NewDelivery, SHCNRF_ShellLevel,
    };
    use windows::core::GUID;
    use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_MESSAGE, MWMO_INPUTAVAILABLE, QS_ALLINPUT,
    };
    assert_eq!(FOLDERID_RecycleBinFolder, GUID::from_u128(RECYCLE_FOLDER));
    assert_eq!((SHCNRF_ShellLevel | SHCNRF_NewDelivery).0, SOURCES);
    assert_eq!(
        (SHCNE_RENAMEITEM
            | SHCNE_CREATE
            | SHCNE_DELETE
            | SHCNE_MKDIR
            | SHCNE_RMDIR
            | SHCNE_RENAMEFOLDER
            | SHCNE_ATTRIBUTES
            | SHCNE_UPDATEDIR
            | SHCNE_UPDATEITEM)
            .0,
        ITEM_MASK
    );
    assert_eq!(HWND_MESSAGE as isize, -3);
    assert_eq!((QS_ALLINPUT, MWMO_INPUTAVAILABLE), (0x4FF, 4));
    assert_eq!(
        (WAIT_OBJECT_0, WAIT_TIMEOUT, WAIT_FAILED),
        (0, 258, u32::MAX)
    );
}
