// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Recording adapters ONLY, even on Windows. Every request exercises the same
//! Owner and private real-spawn/join wrapper as production; no native effects.

use super::*;
use parking_lot::Mutex;
use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::ThreadId;
use std::time::Duration;
use tessera_system::power::PowerHost;

const TIMEOUT: Duration = Duration::from_secs(5);
const CLEANUP_STALE: u32 = 0xdead_beef;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    Impersonate,
    Open,
    Lookup,
    Revert,
    Close,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Call {
    OuterCreate,
    OuterReturn,
    DriverDrop,
    Create,
    Lock,
    LogOut(u32, u32),
    Impersonate(i32),
    CurrentThread,
    Open(u32, i32),
    Lookup(bool),
    Adjust(i32, u32, u32),
    Shutdown(bool, bool, u32, u32, u32),
    Suspend(bool, bool, bool),
    LastError(u32),
    Revert,
    Close,
    TokenDrop,
    CallsDrop,
    ThreadExit,
    PanicPayloadDrop,
    Completion,
}

#[derive(Clone, Debug)]
struct Event {
    call: Call,
    thread: ThreadId,
}

struct Block {
    entered: Sender<ThreadId>,
    release: Mutex<Receiver<()>>,
}

impl Block {
    fn wait(&self) {
        self.entered.send(std::thread::current().id()).unwrap();
        self.release.lock().recv_timeout(TIMEOUT).unwrap();
    }
}

fn block() -> (Arc<Block>, Receiver<ThreadId>, Sender<()>) {
    let (entered, receiver) = mpsc::channel();
    let (release, released) = mpsc::channel();
    (
        Arc::new(Block {
            entered,
            release: Mutex::new(released),
        }),
        receiver,
        release,
    )
}

struct Script {
    failures: Vec<(Stage, u32)>,
    success_bool: i32,
    action_bool: i32,
    action_error: u32,
    adjust_bool: i32,
    adjust_error: u32,
    shutdown_code: u32,
    suspend_bool: bool,
    revert_retry_succeeds: bool,
    panic_action: bool,
    panic_payload_drop: bool,
    native_block: Option<Arc<Block>>,
    exit_block: Option<Arc<Block>>,
    driver_drop_block: Option<Arc<Block>>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            failures: Vec::new(),
            success_bool: 1,
            action_bool: 1,
            action_error: 0,
            adjust_bool: 1,
            adjust_error: 0,
            shutdown_code: 0,
            suspend_bool: true,
            revert_retry_succeeds: false,
            panic_action: false,
            panic_payload_drop: false,
            native_block: None,
            exit_block: None,
            driver_drop_block: None,
        }
    }
}

struct Record {
    script: Script,
    events: Mutex<Vec<Event>>,
    error: AtomicU32,
}

impl Record {
    fn new(script: Script) -> Arc<Self> {
        Arc::new(Self {
            script,
            events: Mutex::new(Vec::new()),
            error: AtomicU32::new(0),
        })
    }

    fn event(&self, call: Call) {
        self.events.lock().push(Event {
            call,
            thread: std::thread::current().id(),
        });
    }

    fn calls(&self) -> Vec<Call> {
        self.events
            .lock()
            .iter()
            .map(|event| event.call.clone())
            .collect()
    }
}

thread_local! {
    static IMPERSONATING: Cell<bool> = const { Cell::new(false) };
    static EXIT_GUARD: RefCell<Option<ExitGuard>> = const { RefCell::new(None) };
}

struct ExitGuard(Arc<Record>);
impl Drop for ExitGuard {
    fn drop(&mut self) {
        // Teardown marker precedes actual thread termination. This event (or a
        // channel) is NOT retirement proof: production always joins the thread.
        self.0.event(Call::ThreadExit);
        if let Some(block) = &self.0.script.exit_block {
            block.wait();
        }
    }
}

struct RecordingThread(ThreadId);
struct RecordingToken {
    record: Arc<Record>,
    owner: ThreadId,
    _thread: PhantomData<Rc<()>>,
}
impl Drop for RecordingToken {
    fn drop(&mut self) {
        assert_eq!(std::thread::current().id(), self.owner);
        self.record.event(Call::TokenDrop);
    }
}

struct RecordingCalls {
    record: Arc<Record>,
    owner: ThreadId,
    reverts: usize,
    _thread: PhantomData<Rc<()>>,
}
impl RecordingCalls {
    fn new(record: Arc<Record>) -> Self {
        record.event(Call::Create);
        Self {
            record,
            owner: std::thread::current().id(),
            reverts: 0,
            _thread: PhantomData,
        }
    }

    fn event(&self, call: Call) {
        assert_eq!(std::thread::current().id(), self.owner);
        self.record.event(call);
    }

    fn raw_bool(&self, stage: Stage) -> i32 {
        if let Some((_, code)) = self
            .record
            .script
            .failures
            .iter()
            .find(|(step, _)| *step == stage)
        {
            self.record.error.store(*code, Ordering::SeqCst);
            0
        } else {
            self.record.error.store(CLEANUP_STALE, Ordering::SeqCst);
            self.record.script.success_bool
        }
    }

    fn action(&self) {
        if let Some(block) = &self.record.script.native_block {
            block.wait();
        }
        if self.record.script.panic_action {
            std::panic::panic_any(PanicDrop(self.record.clone()));
        }
        self.record
            .error
            .store(self.record.script.action_error, Ordering::SeqCst);
    }
}

struct PanicDrop(Arc<Record>);
impl Drop for PanicDrop {
    fn drop(&mut self) {
        assert!(
            !IMPERSONATING.with(Cell::get),
            "panic payload must retire on neutral outer"
        );
        self.0.event(Call::PanicPayloadDrop);
        if self.0.script.panic_payload_drop {
            panic!("recorded payload drop");
        }
    }
}

impl Calls for RecordingCalls {
    type Thread = RecordingThread;
    type Token = RecordingToken;
    type Privilege = u32;

    fn lock_work_station(&mut self) -> i32 {
        self.event(Call::Lock);
        self.action();
        self.record.script.action_bool
    }
    fn exit_windows_ex(&mut self, flags: u32, reason: u32) -> i32 {
        self.event(Call::LogOut(flags, reason));
        self.action();
        self.record.script.action_bool
    }
    fn impersonate_self(&mut self, level: i32) -> i32 {
        self.event(Call::Impersonate(level));
        EXIT_GUARD.with(|slot| *slot.borrow_mut() = Some(ExitGuard(self.record.clone())));
        let status = self.raw_bool(Stage::Impersonate);
        if status != 0 {
            IMPERSONATING.with(|value| value.set(true));
        }
        status
    }
    fn current_thread(&mut self) -> RecordingThread {
        self.event(Call::CurrentThread);
        RecordingThread(self.owner)
    }
    fn open_thread_token(
        &mut self,
        thread: &RecordingThread,
        access: u32,
        open_as_self: i32,
    ) -> (i32, Option<RecordingToken>) {
        assert_eq!(thread.0, self.owner);
        self.event(Call::Open(access, open_as_self));
        let status = self.raw_bool(Stage::Open);
        let token = if status != 0 {
            Some(RecordingToken {
                record: self.record.clone(),
                owner: self.owner,
                _thread: PhantomData,
            })
        } else {
            None
        };
        (status, token)
    }
    fn lookup_shutdown_privilege(&mut self, system: *const u16) -> (i32, u32) {
        self.event(Call::Lookup(system.is_null()));
        (self.raw_bool(Stage::Lookup), 17)
    }
    fn adjust_token_privileges(
        &mut self,
        token: &RecordingToken,
        privilege: &u32,
        disable_all: i32,
        count: u32,
        attributes: u32,
    ) -> i32 {
        assert_eq!(token.owner, self.owner);
        assert_eq!(*privilege, 17);
        self.event(Call::Adjust(disable_all, count, attributes));
        self.record
            .error
            .store(self.record.script.adjust_error, Ordering::SeqCst);
        self.record.script.adjust_bool
    }
    fn initiate_shutdown(
        &mut self,
        machine: *const u16,
        message: *const u16,
        grace: u32,
        flags: u32,
        reason: u32,
    ) -> u32 {
        self.event(Call::Shutdown(
            machine.is_null(),
            message.is_null(),
            grace,
            flags,
            reason,
        ));
        self.action();
        self.record.script.shutdown_code
    }
    fn set_suspend_state(&mut self, hibernate: bool, force: bool, disable_wake: bool) -> bool {
        self.event(Call::Suspend(hibernate, force, disable_wake));
        self.action();
        self.record.script.suspend_bool
    }
    fn revert_to_self(&mut self) -> i32 {
        self.event(Call::Revert);
        self.reverts += 1;
        let status = if self.reverts > 1 && self.record.script.revert_retry_succeeds {
            self.record.script.success_bool
        } else {
            self.raw_bool(Stage::Revert)
        };
        if status != 0 {
            IMPERSONATING.with(|value| value.set(false));
        }
        status
    }
    fn close_token(&mut self, token: &RecordingToken) -> i32 {
        assert_eq!(token.owner, self.owner);
        self.event(Call::Close);
        self.raw_bool(Stage::Close)
    }
    fn last_error(&mut self) -> u32 {
        let code = self.record.error.load(Ordering::SeqCst);
        self.event(Call::LastError(code));
        code
    }
}

impl Drop for RecordingCalls {
    fn drop(&mut self) {
        self.event(Call::CallsDrop);
        self.record.error.store(CLEANUP_STALE, Ordering::SeqCst);
    }
}

struct RecordingDriver<F> {
    inner: PowerDriver<F>,
    record: Arc<Record>,
}
impl<C, F> Driver for RecordingDriver<F>
where
    C: Calls + 'static,
    F: Fn() -> C + Clone + Send + 'static,
{
    fn perform(&mut self, action: PowerAction) -> Outcome {
        let result = self.inner.perform(action);
        self.record.event(Call::OuterReturn);
        result
    }
}
impl<F> Drop for RecordingDriver<F> {
    fn drop(&mut self) {
        assert!(!IMPERSONATING.with(Cell::get));
        self.record.event(Call::DriverDrop);
        if let Some(block) = &self.record.script.driver_drop_block {
            block.wait();
        }
    }
}

fn host(record: Arc<Record>, spawn: operation::Spawn) -> Arc<dyn PowerHost> {
    crate::power::worker::host(move || {
        record.event(Call::OuterCreate);
        let create = {
            let record = record.clone();
            move || RecordingCalls::new(record.clone())
        };
        let mut inner = PowerDriver::new(create);
        inner.spawn = spawn;
        Ok(RecordingDriver {
            inner,
            record: record.clone(),
        })
    })
}

fn perform(record: Arc<Record>, action: PowerAction) -> Outcome {
    let create = {
        let record = record.clone();
        move || RecordingCalls::new(record.clone())
    };
    PowerDriver::new(create).perform(action)
}

fn is_effect(call: &Call) -> bool {
    matches!(
        call,
        Call::Lock | Call::LogOut(..) | Call::Shutdown(..) | Call::Suspend(..)
    )
}

fn privileged_prefix() -> Vec<Call> {
    vec![
        Call::Create,
        Call::Impersonate(2),
        Call::CurrentThread,
        Call::Open(40, 1),
        Call::Lookup(true),
        Call::Adjust(0, 1, 2),
        Call::LastError(0),
    ]
}

#[test]
fn recording_power_factory_and_clones_make_zero_native_calls_before_perform() {
    let record = Record::new(Script::default());
    let host = host(record.clone(), operation::spawn);
    let clone = host.clone();
    assert!(record.events.lock().is_empty());
    drop(host);
    drop(clone);
    assert!(record.events.lock().is_empty());
}

#[test]
fn all_six_actions_have_exact_fixed_arguments_and_update_variants_no_force_or_preflight() {
    let plain = PowerUpdatePolicy::OmitExplicitInstallation;
    let install = PowerUpdatePolicy::RequestInstallation;
    let cases = [
        (PowerAction::LockSession, Call::Lock),
        (PowerAction::LogOut, Call::LogOut(0, 0)),
        (
            PowerAction::PowerOff { updates: plain },
            Call::Shutdown(true, true, 0, 8, 0),
        ),
        (
            PowerAction::Reboot { updates: plain },
            Call::Shutdown(true, true, 0, 4, 0),
        ),
        (
            PowerAction::PowerOff { updates: install },
            Call::Shutdown(true, true, 0, 72, 0x8002_0003),
        ),
        (
            PowerAction::Reboot { updates: install },
            Call::Shutdown(true, true, 0, 68, 0x8002_0003),
        ),
        (PowerAction::Suspend, Call::Suspend(false, false, false)),
        (PowerAction::Hibernate, Call::Suspend(true, false, false)),
    ];
    let outer = std::thread::current().id();
    for (action, expected_action) in cases {
        let record = Record::new(Script::default());
        assert_eq!(perform(record.clone(), action), Ok(PowerRequestAccepted));
        let expected = if needs_privilege(action) {
            let mut calls = privileged_prefix();
            calls.extend([
                expected_action,
                Call::Revert,
                Call::Close,
                Call::TokenDrop,
                Call::CallsDrop,
                Call::ThreadExit,
            ]);
            calls
        } else {
            vec![Call::Create, expected_action, Call::CallsDrop]
        };
        assert_eq!(record.calls(), expected);
        let events = record.events.lock();
        let native_owner = events[0].thread;
        assert!(events.iter().all(|event| event.thread == native_owner));
        assert_eq!(native_owner != outer, needs_privilege(action));
    }
}

#[test]
fn raw_bool_nonzero_and_boolean_true_do_not_consult_stale_action_error() {
    for status in [i32::MIN, -42, -1, 1, 2, i32::MAX] {
        for action in [
            PowerAction::LockSession,
            PowerAction::LogOut,
            PowerAction::Suspend,
        ] {
            let record = Record::new(Script {
                success_bool: status,
                action_bool: status,
                adjust_bool: status,
                action_error: ACCESS_DENIED,
                ..Script::default()
            });
            assert_eq!(perform(record.clone(), action), Ok(PowerRequestAccepted));
            let reads = record
                .calls()
                .into_iter()
                .filter(|call| matches!(call, Call::LastError(_)))
                .collect::<Vec<_>>();
            assert_eq!(
                reads,
                if needs_privilege(action) {
                    vec![Call::LastError(0)]
                } else {
                    vec![]
                }
            );
        }
    }
}

#[test]
fn bool_and_boolean_failure_capture_exact_error_before_any_cleanup_no_replay() {
    for code in [0, 1, ACCESS_DENIED, 1300, 1314, 0x8007_0005, u32::MAX] {
        for action in [
            PowerAction::LockSession,
            PowerAction::LogOut,
            PowerAction::Suspend,
            PowerAction::Hibernate,
        ] {
            let record = Record::new(Script {
                action_bool: 0,
                suspend_bool: false,
                action_error: code,
                failures: vec![(Stage::Revert, 55), (Stage::Close, 66)],
                ..Script::default()
            });
            assert_eq!(perform(record.clone(), action), Err(native_error(code)));
            let calls = record.calls();
            let effect = calls.iter().position(is_effect).unwrap();
            assert_eq!(calls[effect + 1], Call::LastError(code));
            assert_eq!(calls.iter().filter(|call| is_effect(call)).count(), 1);
            assert_eq!(record.error.load(Ordering::SeqCst), CLEANUP_STALE);
        }
    }
}

#[test]
fn shutdown_dword_is_authoritative_never_get_last_error_even_with_cleanup_failure() {
    for code in [1, ACCESS_DENIED, 1300, 1314, 0x8007_0005, u32::MAX] {
        for action in [
            PowerAction::PowerOff {
                updates: PowerUpdatePolicy::OmitExplicitInstallation,
            },
            PowerAction::Reboot {
                updates: PowerUpdatePolicy::RequestInstallation,
            },
        ] {
            let record = Record::new(Script {
                shutdown_code: code,
                action_error: 999,
                failures: vec![(Stage::Close, 77)],
                ..Script::default()
            });
            assert_eq!(perform(record.clone(), action), Err(native_error(code)));
            let calls = record.calls();
            let effect = calls.iter().position(is_effect).unwrap();
            assert_eq!(calls[effect + 1], Call::Revert);
            assert!(!calls.contains(&Call::LastError(999)));
            assert_eq!(calls.iter().filter(|call| is_effect(call)).count(), 1);
        }
    }
}

#[test]
fn adjust_requires_raw_bool_and_immediate_success_code_positive_1300_has_zero_effects() {
    for (status, code) in [
        (0, 0),
        (0, 5),
        (0, 1314),
        (1, 1300),
        (-1, 1300),
        (1, 1314),
        (1, u32::MAX),
    ] {
        let record = Record::new(Script {
            adjust_bool: status,
            adjust_error: code,
            ..Script::default()
        });
        assert_eq!(
            perform(record.clone(), PowerAction::Suspend),
            Err(native_error(code))
        );
        let calls = record.calls();
        let adjust = calls
            .iter()
            .position(|call| matches!(call, Call::Adjust(..)))
            .unwrap();
        assert_eq!(calls[adjust + 1], Call::LastError(code));
        assert_eq!(calls.iter().filter(|call| is_effect(call)).count(), 0);
        assert_eq!(calls.iter().filter(|call| **call == Call::Close).count(), 1);
    }
}

#[test]
fn partial_acquisition_uses_same_owner_and_retires_only_acquired_resources() {
    for stage in [Stage::Impersonate, Stage::Open, Stage::Lookup] {
        let record = Record::new(Script {
            failures: vec![(stage, 1314)],
            ..Script::default()
        });
        assert_eq!(
            perform(record.clone(), PowerAction::Hibernate),
            Err(PowerError::Native { code: 1314 })
        );
        let calls = record.calls();
        assert_eq!(calls.iter().filter(|call| is_effect(call)).count(), 0);
        assert_eq!(
            calls.iter().filter(|call| **call == Call::Revert).count(),
            usize::from(stage != Stage::Impersonate)
        );
        assert_eq!(
            calls.iter().filter(|call| **call == Call::Close).count(),
            usize::from(stage == Stage::Lookup)
        );
        assert_eq!(
            calls
                .iter()
                .filter(|call| **call == Call::TokenDrop)
                .count(),
            usize::from(stage == Stage::Lookup)
        );
        assert_eq!(
            calls[calls.len() - 2..],
            [Call::CallsDrop, Call::ThreadExit]
        );
    }
}

#[test]
fn first_checked_cleanup_failure_is_retained_close_once_no_action_replay_even_if_revert_retry_succeeds()
 {
    for retry in [false, true] {
        let record = Record::new(Script {
            failures: vec![(Stage::Revert, 61), (Stage::Close, 62)],
            revert_retry_succeeds: retry,
            ..Script::default()
        });
        assert_eq!(
            perform(record.clone(), PowerAction::Suspend),
            Err(PowerError::Native { code: 61 })
        );
        let calls = record.calls();
        assert_eq!(
            calls.iter().filter(|call| **call == Call::Revert).count(),
            2
        );
        assert_eq!(calls.iter().filter(|call| **call == Call::Close).count(), 1);
        assert_eq!(calls.iter().filter(|call| is_effect(call)).count(), 1);
        let close = calls.iter().position(|call| *call == Call::Close).unwrap();
        assert_eq!(calls[close + 1], Call::LastError(62));
        assert_eq!(calls[close + 2], Call::TokenDrop);
    }
    let record = Record::new(Script {
        failures: vec![(Stage::Close, ACCESS_DENIED)],
        ..Script::default()
    });
    assert_eq!(
        perform(record.clone(), PowerAction::Hibernate),
        Err(PowerError::AccessDenied)
    );
    assert_eq!(
        record
            .calls()
            .iter()
            .filter(|call| **call == Call::Close)
            .count(),
        1
    );
}

#[test]
fn real_child_owner_drop_and_thread_exit_precede_neutral_callback_after_failed_revert() {
    let record = Record::new(Script {
        failures: vec![(Stage::Revert, 61)],
        ..Script::default()
    });
    let host = host(record.clone(), operation::spawn);
    let (sender, receiver) = mpsc::channel();
    host.perform(
        PowerAction::Suspend,
        Box::new({
            let record = record.clone();
            move |result| {
                assert!(!IMPERSONATING.with(Cell::get));
                record.event(Call::Completion);
                sender.send(result).unwrap();
            }
        }),
    )
    .unwrap();
    assert_eq!(
        receiver.recv_timeout(TIMEOUT).unwrap(),
        Err(PowerError::Native { code: 61 })
    );
    let events = record.events.lock();
    let outer = events[0].thread;
    let child = events
        .iter()
        .find(|event| event.call == Call::Create)
        .unwrap()
        .thread;
    assert_ne!(outer, child);
    let exit = events
        .iter()
        .position(|event| event.call == Call::ThreadExit)
        .unwrap();
    let returned = events
        .iter()
        .position(|event| event.call == Call::OuterReturn)
        .unwrap();
    let driver_drop = events
        .iter()
        .position(|event| event.call == Call::DriverDrop)
        .unwrap();
    let completion = events
        .iter()
        .position(|event| event.call == Call::Completion)
        .unwrap();
    assert!(exit < returned && returned < driver_drop && driver_drop < completion);
    for event in events.iter() {
        assert_eq!(
            event.thread,
            if matches!(
                event.call,
                Call::OuterCreate | Call::OuterReturn | Call::DriverDrop | Call::Completion
            ) {
                outer
            } else {
                child
            }
        );
    }
}

fn assert_busy(host: &Arc<dyn PowerHost>, receiver: &Receiver<Outcome>) {
    assert_eq!(
        host.perform(
            PowerAction::LockSession,
            Box::new(|_| panic!("busy cannot accept callback"))
        ),
        Err(PowerError::Busy)
    );
    assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
}

#[test]
fn accepted_flight_stays_busy_through_blocking_operation_and_survives_host_and_ui_drop() {
    let (native, entered, release) = block();
    let record = Record::new(Script {
        native_block: Some(native),
        ..Script::default()
    });
    let host = host(record, operation::spawn);
    let ui = Arc::new(());
    let weak_ui = Arc::downgrade(&ui);
    let (sender, receiver) = mpsc::channel();
    host.perform(
        PowerAction::Suspend,
        Box::new(move |result| {
            assert!(weak_ui.upgrade().is_none());
            sender.send(result).unwrap();
        }),
    )
    .unwrap();
    entered.recv_timeout(TIMEOUT).unwrap();
    assert_busy(&host, &receiver);
    drop(host);
    drop(ui);
    assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
    release.send(()).unwrap();
    assert_eq!(
        receiver.recv_timeout(TIMEOUT).unwrap(),
        Ok(PowerRequestAccepted)
    );
    assert_eq!(
        receiver.recv_timeout(TIMEOUT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}

// Blocking a Rust TLS destructor is a portable Linux recording proof, not a
// proposed Windows production pattern (DLL teardown may hold loader lock).
#[cfg(not(windows))]
#[test]
fn accepted_flight_stays_busy_until_real_join_including_tls_then_outer_driver_drop() {
    let (native, native_entered, native_release) = block();
    let (exit, exit_entered, exit_release) = block();
    let (driver, driver_entered, driver_release) = block();
    let record = Record::new(Script {
        native_block: Some(native),
        exit_block: Some(exit),
        driver_drop_block: Some(driver),
        ..Script::default()
    });
    let host = host(record, operation::spawn);
    let (sender, receiver) = mpsc::channel();
    host.perform(
        PowerAction::Hibernate,
        Box::new(move |result| sender.send(result).unwrap()),
    )
    .unwrap();
    let child = native_entered.recv_timeout(TIMEOUT).unwrap();
    assert_busy(&host, &receiver);
    native_release.send(()).unwrap();
    assert_eq!(exit_entered.recv_timeout(TIMEOUT).unwrap(), child);
    assert_busy(&host, &receiver);
    exit_release.send(()).unwrap();
    assert_ne!(driver_entered.recv_timeout(TIMEOUT).unwrap(), child);
    assert_busy(&host, &receiver);
    driver_release.send(()).unwrap();
    assert_eq!(
        receiver.recv_timeout(TIMEOUT).unwrap(),
        Ok(PowerRequestAccepted)
    );
}

#[test]
fn callback_reentry_is_neutral_and_gate_released_after_real_join() {
    let record = Record::new(Script::default());
    let host = host(record.clone(), operation::spawn);
    let (sender, receiver) = mpsc::channel();
    let callback_host = host.clone();
    host.perform(
        PowerAction::Suspend,
        Box::new(move |result| {
            assert_eq!(result, Ok(PowerRequestAccepted));
            assert!(!IMPERSONATING.with(Cell::get));
            assert!(record.calls().contains(&Call::ThreadExit));
            callback_host
                .perform(
                    PowerAction::LockSession,
                    Box::new(move |result| sender.send(result).unwrap()),
                )
                .unwrap();
        }),
    )
    .unwrap();
    assert_eq!(
        receiver.recv_timeout(TIMEOUT).unwrap(),
        Ok(PowerRequestAccepted)
    );
    assert_eq!(
        receiver.recv_timeout(TIMEOUT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}

fn fail_spawn(job: operation::Job) -> std::io::Result<std::thread::JoinHandle<Outcome>> {
    drop(job);
    Err(std::io::Error::other("recorded child spawn failure"))
}
fn panic_spawn(job: operation::Job) -> std::io::Result<std::thread::JoinHandle<Outcome>> {
    drop(job);
    panic!("recorded child spawn panic")
}

#[test]
fn child_spawn_failure_after_host_acceptance_has_one_completion_zero_native_calls() {
    for spawn in [
        fail_spawn as operation::Spawn,
        panic_spawn as operation::Spawn,
    ] {
        let record = Record::new(Script::default());
        let host = host(record.clone(), spawn);
        let (sender, receiver) = mpsc::channel();
        assert_eq!(
            host.perform(
                PowerAction::Suspend,
                Box::new(move |result| sender.send(result).unwrap())
            ),
            Ok(())
        );
        assert_eq!(
            receiver.recv_timeout(TIMEOUT).unwrap(),
            Err(PowerError::Unavailable)
        );
        assert_eq!(
            receiver.recv_timeout(TIMEOUT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        assert_eq!(
            record.calls(),
            [Call::OuterCreate, Call::OuterReturn, Call::DriverDrop]
        );
    }
}

#[test]
fn child_panic_is_retired_and_payload_drop_is_contained_on_neutral_outer() {
    for panic_payload_drop in [false, true] {
        let record = Record::new(Script {
            panic_action: true,
            panic_payload_drop,
            failures: vec![(Stage::Revert, 61)],
            ..Script::default()
        });
        let host = host(record.clone(), operation::spawn);
        let (sender, receiver) = mpsc::channel();
        host.perform(
            PowerAction::Suspend,
            Box::new({
                let record = record.clone();
                move |result| {
                    assert!(!IMPERSONATING.with(Cell::get));
                    record.event(Call::Completion);
                    sender.send(result).unwrap();
                }
            }),
        )
        .unwrap();
        assert_eq!(
            receiver.recv_timeout(TIMEOUT).unwrap(),
            Err(PowerError::Unavailable)
        );
        assert_eq!(
            receiver.recv_timeout(TIMEOUT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        let events = record.events.lock();
        let exit = events
            .iter()
            .position(|event| event.call == Call::ThreadExit)
            .unwrap();
        let payload = events
            .iter()
            .position(|event| event.call == Call::PanicPayloadDrop)
            .unwrap();
        let completion = events
            .iter()
            .position(|event| event.call == Call::Completion)
            .unwrap();
        assert!(exit < payload && payload < completion);
        assert_ne!(events[exit].thread, events[payload].thread);
        assert_eq!(events[payload].thread, events[completion].thread);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.call == Call::Close)
                .count(),
            1
        );
        assert_eq!(
            events.iter().filter(|event| is_effect(&event.call)).count(),
            1
        );
    }
}
