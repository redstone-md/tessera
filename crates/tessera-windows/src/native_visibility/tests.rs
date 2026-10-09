// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

// These adapters never register a hook, signal a Windows event, or synthesize
// input. Opaque tokens exercise the SAME Owner<Calls> used by the native SDK.

use parking_lot::Mutex;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, ThreadId};
use std::time::Duration;

use super::environment::{
    DEVICE_CAP, EXTERNAL_PEN, EnvironmentCalls, INTEGRATED_PEN, TOUCH, TOUCH_PAD,
    pointer_environment,
};
use super::*;

const EVENT_TOKEN: usize = 0xE771;
const HOOK_TOKEN: usize = 0xA001;
const DPI_TOKEN: usize = 0xD001;
const POINT_TOKEN: isize = 0xCAFE;
const CHAIN_RESULT: isize = -91;
const DEADLINE: Duration = Duration::from_secs(5);
const SEED: PhysicalPoint = PhysicalPoint { x: -1700, y: 777 };

type GuardSlot = Arc<Mutex<Option<Box<dyn PointerWatchGuard>>>>;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Record {
    CreateEvent(usize),
    SignalEvent(usize),
    SignalError(u32),
    CloseEvent(usize),
    CreateCalls,
    BeginContext,
    InstallHook,
    RemoveHook(usize),
    SetDpi,
    ReadCursor,
    RestoreDpi(usize),
    DigitizerBits,
    DeviceCount,
    DeviceList(usize),
    RetireContext,
    EndContext,
    DropCalls,
    Wait(usize, u32),
    NextMessage,
    Dispatch,
    Copy(PhysicalPoint),
    CopyPanic,
    Chain(i32, usize, isize),
    Ready(Result<(), PointerWatchError>),
    Environment(PointerEnvironment),
    Position(PhysicalPoint),
    Failed(PointerWatchError),
}

#[derive(Default)]
struct Audit {
    records: Mutex<Vec<(ThreadId, Record)>>,
    in_ffi: AtomicBool,
}

#[derive(Clone, Default)]
struct Log(Arc<Audit>);

impl Log {
    fn push(&self, record: Record) {
        self.0.records.lock().push((thread::current().id(), record));
    }

    fn records(&self) -> Vec<Record> {
        self.0
            .records
            .lock()
            .iter()
            .map(|(_, record)| record.clone())
            .collect()
    }

    fn count(&self, record: &Record) -> usize {
        self.records().iter().filter(|item| *item == record).count()
    }

    fn positions(&self) -> Vec<PhysicalPoint> {
        self.records()
            .into_iter()
            .filter_map(|record| {
                if let Record::Position(point) = record {
                    Some(point)
                } else {
                    None
                }
            })
            .collect()
    }

    fn callbacks(&self) -> (PointerWatchCallback, PointerWatchCompletion) {
        let events = self.clone();
        let ready = self.clone();
        (
            Arc::new(move |event| {
                assert!(!events.0.in_ffi.load(Ordering::Acquire));
                match event {
                    PointerWatchEvent::Position(point) => events.push(Record::Position(point)),
                    PointerWatchEvent::Environment(environment) => {
                        events.push(Record::Environment(environment))
                    }
                    PointerWatchEvent::Failed(error) => events.push(Record::Failed(error)),
                }
            }),
            Box::new(move |result| {
                assert!(!ready.0.in_ffi.load(Ordering::Acquire));
                ready.push(Record::Ready(result));
            }),
        )
    }

    fn before(&self, first: Record, second: Record) {
        let records = self.records();
        let first = records.iter().position(|record| record == &first).unwrap();
        let second = records.iter().position(|record| record == &second).unwrap();
        assert!(first < second, "unexpected ownership order: {records:?}");
    }

    fn owner_thread_only(&self) {
        let records = self.0.records.lock();
        let owner = records
            .iter()
            .find(|(_, record)| *record == Record::CreateCalls)
            .unwrap()
            .0;
        for (thread, record) in records.iter() {
            if matches!(
                record,
                Record::BeginContext
                    | Record::InstallHook
                    | Record::RemoveHook(_)
                    | Record::SetDpi
                    | Record::ReadCursor
                    | Record::RestoreDpi(_)
                    | Record::RetireContext
                    | Record::EndContext
                    | Record::DropCalls
                    | Record::DigitizerBits
                    | Record::DeviceCount
                    | Record::DeviceList(_)
            ) {
                assert_eq!(*thread, owner, "resource escaped the owner: {record:?}");
            }
        }
    }
}

struct Stop {
    log: Log,
    signal_error: Option<u32>,
}

impl StopSignal for Stop {
    fn signal(&self) -> Result<(), u32> {
        self.log.push(Record::SignalEvent(EVENT_TOKEN));
        match self.signal_error {
            Some(code) => {
                self.log.push(Record::SignalError(code));
                Err(code)
            }
            None => Ok(()),
        }
    }
}

impl Drop for Stop {
    fn drop(&mut self) {
        self.log.push(Record::CloseEvent(EVENT_TOKEN));
    }
}

struct Hook(usize);
struct Dpi(usize);

#[derive(Default)]
struct Local {
    active: Cell<bool>,
    latest: Cell<Option<PhysicalPoint>>,
    failed: Cell<bool>,
}

struct RecordedHook<'a> {
    log: &'a Log,
    local: &'a Local,
    point: PhysicalPoint,
    panic_copy: bool,
}

impl HookCalls for RecordedHook<'_> {
    fn copy_move(&self, token: isize) {
        assert_eq!(token, POINT_TOKEN);
        if self.panic_copy {
            self.log.push(Record::CopyPanic);
            panic!("recorded copy panic");
        }
        if self.local.active.get() {
            self.log.push(Record::Copy(self.point));
            self.local.latest.set(Some(self.point));
        }
    }

    fn callback_failed(&self) {
        self.local.failed.set(true);
    }

    fn chain(&self, code: i32, wparam: usize, lparam: isize) -> isize {
        self.log.push(Record::Chain(code, wparam, lparam));
        CHAIN_RESULT
    }
}

enum Message {
    Move(PhysicalPoint),
    Call(RefCell<Option<Job>>),
    Quit,
}

enum Action {
    Messages(Vec<Message>),
    Burst(Vec<PhysicalPoint>),
    Safety,
    Call(Job),
    Block {
        entered: Sender<()>,
        release: Receiver<()>,
    },
    Fail(u32),
    CallbackPanic,
    Stop,
}

struct EnvironmentPlan {
    bits: u32,
    count: Result<u32, u32>,
    kinds: Vec<i32>,
    list_error: Option<u32>,
    actual: Option<u32>,
}

impl Default for EnvironmentPlan {
    fn default() -> Self {
        Self {
            bits: 0,
            count: Ok(0),
            kinds: Vec::new(),
            list_error: None,
            actual: None,
        }
    }
}

#[derive(Default)]
struct Plan {
    failure: Option<(PointerNativeOperation, u32)>,
    signal_error: Option<u32>,
    install_action: Option<Job>,
    read_action: Option<Job>,
    reenter_remove: bool,
    environment: EnvironmentPlan,
    actions: VecDeque<Action>,
}

struct RecordingCalls {
    log: Log,
    // This Rc is intentionally !Send: factory creates it on the owner thread.
    local: Rc<Local>,
    plan: RefCell<Plan>,
    messages: RefCell<VecDeque<Message>>,
}

impl RecordingCalls {
    fn fails(&self, operation: PointerNativeOperation) -> Result<(), u32> {
        match self.plan.borrow().failure {
            Some((failed, code)) if failed == operation => Err(code),
            _ => Ok(()),
        }
    }

    fn movement(&self, point: PhysicalPoint) {
        self.log.0.in_ffi.store(true, Ordering::Release);
        let calls = RecordedHook {
            log: &self.log,
            local: &self.local,
            point,
            panic_copy: false,
        };
        assert_eq!(
            hook_callback(&calls, HC_ACTION, WM_MOUSEMOVE, POINT_TOKEN),
            CHAIN_RESULT
        );
        self.log.0.in_ffi.store(false, Ordering::Release);
    }
}

impl Calls for RecordingCalls {
    type Stop = Stop;
    type Hook = Hook;
    type DpiContext = Dpi;
    type Message = Message;

    fn begin_context(&self) -> Result<(), u32> {
        self.log.push(Record::BeginContext);
        self.local.active.set(true);
        self.fails(PointerNativeOperation::Callback)
    }

    fn retire_context(&self) {
        self.log.push(Record::RetireContext);
        self.local.active.set(false);
    }

    fn end_context(&self) {
        self.log.push(Record::EndContext);
        self.local.latest.set(None);
        self.local.failed.set(false);
    }

    fn install_hook(&self) -> Result<Hook, u32> {
        self.log.push(Record::InstallHook);
        let action = self.plan.borrow_mut().install_action.take();
        if let Some(action) = action {
            action();
        }
        self.fails(PointerNativeOperation::InstallHook)?;
        Ok(Hook(HOOK_TOKEN))
    }

    fn remove_hook(&self, hook: Hook) -> Result<(), u32> {
        assert!(!self.local.active.get());
        self.log.push(Record::RemoveHook(hook.0));
        if self.plan.borrow().reenter_remove {
            self.movement(PhysicalPoint { x: 999, y: 999 });
        }
        self.fails(PointerNativeOperation::RemoveHook)
    }

    fn set_dpi_context(&self) -> Result<Dpi, u32> {
        self.log.push(Record::SetDpi);
        self.fails(PointerNativeOperation::SetDpiContext)?;
        Ok(Dpi(DPI_TOKEN))
    }

    fn restore_dpi_context(&self, previous: Dpi) -> Result<(), u32> {
        self.log.push(Record::RestoreDpi(previous.0));
        self.fails(PointerNativeOperation::RestoreDpiContext)
    }

    fn read_cursor(&self) -> Result<PhysicalPoint, u32> {
        self.log.push(Record::ReadCursor);
        let action = self.plan.borrow_mut().read_action.take();
        if let Some(action) = action {
            action();
        }
        self.fails(PointerNativeOperation::ReadCursor)?;
        Ok(SEED)
    }

    fn environment(&self) -> PointerEnvironment {
        pointer_environment(self)
    }

    fn take_hints(&self) -> Hints {
        Hints {
            latest: self.local.latest.replace(None),
            failed: self.local.failed.replace(false),
        }
    }

    fn wait(&self, _: &Stop, safety_ms: u32) -> Result<Wait, u32> {
        self.log.push(Record::Wait(EVENT_TOKEN, safety_ms));
        if !self.messages.borrow().is_empty() {
            return Ok(Wait::Messages);
        }
        let action = self
            .plan
            .borrow_mut()
            .actions
            .pop_front()
            .unwrap_or(Action::Fail(999));
        match action {
            Action::Messages(messages) => {
                self.messages.borrow_mut().extend(messages);
                Ok(Wait::Messages)
            }
            Action::Burst(points) => {
                for point in points {
                    self.movement(point);
                }
                Ok(Wait::Messages)
            }
            Action::Safety => Ok(Wait::Safety),
            Action::Call(action) => {
                action();
                Ok(Wait::Safety)
            }
            Action::Block { entered, release } => {
                entered.send(()).unwrap();
                release.recv_timeout(DEADLINE).unwrap();
                Ok(Wait::Safety)
            }
            Action::Fail(code) => Err(code),
            Action::CallbackPanic => {
                self.log.0.in_ffi.store(true, Ordering::Release);
                let calls = RecordedHook {
                    log: &self.log,
                    local: &self.local,
                    point: SEED,
                    panic_copy: true,
                };
                assert_eq!(
                    hook_callback(&calls, HC_ACTION, WM_MOUSEMOVE, POINT_TOKEN),
                    CHAIN_RESULT
                );
                self.log.0.in_ffi.store(false, Ordering::Release);
                Ok(Wait::Messages)
            }
            Action::Stop => Ok(Wait::Stop),
        }
    }

    fn next_message(&self) -> Option<Message> {
        self.log.push(Record::NextMessage);
        self.messages.borrow_mut().pop_front()
    }

    fn is_quit(&self, message: &Message) -> bool {
        matches!(message, Message::Quit)
    }

    fn dispatch(&self, message: &Message) {
        self.log.push(Record::Dispatch);
        match message {
            Message::Move(point) => self.movement(*point),
            Message::Call(action) => {
                let action = action.borrow_mut().take();
                if let Some(action) = action {
                    action();
                }
            }
            Message::Quit => unreachable!(),
        }
    }
}

impl EnvironmentCalls for RecordingCalls {
    fn digitizer_bits(&self) -> u32 {
        self.log.push(Record::DigitizerBits);
        self.plan.borrow().environment.bits
    }

    fn device_count(&self) -> Result<u32, u32> {
        self.log.push(Record::DeviceCount);
        self.plan.borrow().environment.count
    }

    fn device_types(&self, kinds: &mut [i32]) -> Result<u32, u32> {
        self.log.push(Record::DeviceList(kinds.len()));
        let plan = self.plan.borrow();
        let environment = &plan.environment;
        if let Some(code) = environment.list_error {
            return Err(code);
        }
        for (output, kind) in kinds.iter_mut().zip(environment.kinds.iter()) {
            *output = *kind;
        }
        Ok(environment.actual.unwrap_or(environment.kinds.len() as u32))
    }
}

impl Drop for RecordingCalls {
    fn drop(&mut self) {
        self.log.push(Record::DropCalls);
    }
}

struct RecordingFactory {
    log: Log,
    plan: Mutex<Option<Plan>>,
    stop_error: Option<u32>,
    calls_error: Option<u32>,
}

impl Factory for RecordingFactory {
    type Calls = RecordingCalls;

    fn create_stop(&self) -> Result<Arc<Stop>, PointerWatchError> {
        if let Some(code) = self.stop_error {
            return Err(native_error(PointerNativeOperation::CreateStopEvent, code));
        }
        self.log.push(Record::CreateEvent(EVENT_TOKEN));
        let signal_error = self.plan.lock().as_ref().and_then(|plan| plan.signal_error);
        Ok(Arc::new(Stop {
            log: self.log.clone(),
            signal_error,
        }))
    }

    fn create_calls(&self) -> Result<RecordingCalls, PointerWatchError> {
        self.log.push(Record::CreateCalls);
        if let Some(code) = self.calls_error {
            return Err(native_error(PointerNativeOperation::Callback, code));
        }
        Ok(RecordingCalls {
            log: self.log.clone(),
            local: Rc::new(Local::default()),
            plan: RefCell::new(self.plan.lock().take().unwrap_or_default()),
            messages: RefCell::new(VecDeque::new()),
        })
    }
}

#[derive(Clone, Default)]
struct Pending {
    job: Arc<Mutex<Option<Job>>>,
    reject: bool,
}

impl Spawner for Pending {
    fn spawn(&self, job: Job) -> std::io::Result<()> {
        if self.reject {
            return Err(std::io::Error::from_raw_os_error(77));
        }
        assert!(self.job.lock().replace(job).is_none());
        Ok(())
    }
}

impl Pending {
    fn run(&self) {
        let job = self.job.lock().take().unwrap();
        job();
    }
}

struct Harness {
    host: Arc<dyn PointerHost>,
    pending: Pending,
    log: Log,
    gate: FlightGate,
}

fn harness(plan: Plan) -> Harness {
    let log = Log::default();
    let pending = Pending::default();
    let gate = FlightGate::default();
    let factory = RecordingFactory {
        log: log.clone(),
        plan: Mutex::new(Some(plan)),
        stop_error: None,
        calls_error: None,
    };
    let host = host_with(factory, pending.clone(), gate.clone());
    Harness {
        host,
        pending,
        log,
        gate,
    }
}

fn retire(slot: &GuardSlot) {
    let guard = slot.lock().take();
    drop(guard);
}

fn retiring(slot: &GuardSlot) -> Job {
    let slot = slot.clone();
    Box::new(move || retire(&slot))
}

fn ordinary(harness: &Harness) -> Box<dyn PointerWatchGuard> {
    let (events, ready) = harness.log.callbacks();
    harness.host.watch(events, ready).unwrap()
}

fn failure(operation: PointerNativeOperation, code: u32) -> PointerWatchError {
    native_error(operation, code)
}

#[test]
fn submission_is_inert_until_owner_runs_and_errors_accept_no_callback() {
    let harness = harness(Plan::default());
    assert!(harness.log.records().is_empty());
    let guard = ordinary(&harness);
    assert_eq!(
        harness.log.records(),
        vec![Record::CreateEvent(EVENT_TOKEN)]
    );
    drop(guard);
    harness.pending.run();
    assert_eq!(
        harness
            .log
            .count(&Record::Ready(Err(PointerWatchError::Stopped))),
        1
    );
    assert_eq!(harness.log.count(&Record::InstallHook), 0);
    assert_eq!(harness.log.count(&Record::CreateCalls), 0);
    assert_eq!(harness.log.count(&Record::CloseEvent(EVENT_TOKEN)), 1);
    assert!(harness.gate.try_enter().is_some());

    for reject in [false, true] {
        let log = Log::default();
        let gate = FlightGate::default();
        let factory = RecordingFactory {
            log: log.clone(),
            plan: Mutex::new(None),
            stop_error: (!reject).then_some(66),
            calls_error: None,
        };
        let pending = Pending {
            reject,
            ..Pending::default()
        };
        let host = host_with(factory, pending, gate.clone());
        let (events, ready) = log.callbacks();
        let result = host.watch(events, ready);
        let expected = if reject {
            failure(PointerNativeOperation::StartThread, 77)
        } else {
            failure(PointerNativeOperation::CreateStopEvent, 66)
        };
        assert!(matches!(result, Err(error) if error == expected));
        assert!(!log.records().iter().any(|record| matches!(
            record,
            Record::Ready(_) | Record::Position(_) | Record::Failed(_)
        )));
        assert!(gate.try_enter().is_some());
        assert_eq!(
            log.count(&Record::CloseEvent(EVENT_TOKEN)),
            usize::from(reject)
        );
    }
}

#[test]
fn successful_registration_restores_dpi_before_ready_then_delivers_one_seed() {
    let harness = harness(Plan {
        actions: VecDeque::from([Action::Safety, Action::Safety, Action::Fail(55)]),
        ..Plan::default()
    });
    let guard = ordinary(&harness);
    harness.pending.run();
    assert_eq!(harness.log.positions(), vec![SEED]);
    assert_eq!(harness.log.count(&Record::Ready(Ok(()))), 1);
    assert_eq!(harness.log.count(&Record::ReadCursor), 1);
    assert_eq!(harness.log.count(&Record::SetDpi), 1);
    assert_eq!(harness.log.count(&Record::RestoreDpi(DPI_TOKEN)), 1);
    assert_eq!(
        harness
            .log
            .count(&Record::Wait(EVENT_TOKEN, SAFETY_WAIT_MS)),
        3
    );
    let environment = Record::Environment(PointerEnvironment {
        touch_capable: Some(false),
    });
    assert_eq!(harness.log.count(&environment), 1);
    assert_eq!(harness.log.count(&Record::DigitizerBits), 1);
    assert_eq!(harness.log.count(&Record::DeviceCount), 1);
    assert_eq!(harness.log.count(&Record::DeviceList(0)), 1);
    harness.log.before(Record::DeviceList(0), Record::SetDpi);
    harness
        .log
        .before(Record::Ready(Ok(())), environment.clone());
    harness.log.before(environment, Record::Position(SEED));
    harness
        .log
        .before(Record::RestoreDpi(DPI_TOKEN), Record::Ready(Ok(())));
    harness
        .log
        .before(Record::Ready(Ok(())), Record::Position(SEED));
    harness
        .log
        .before(Record::RetireContext, Record::RemoveHook(HOOK_TOKEN));
    harness
        .log
        .before(Record::RemoveHook(HOOK_TOKEN), Record::EndContext);
    harness.log.before(
        Record::DropCalls,
        Record::Failed(failure(PointerNativeOperation::MessagePump, 55)),
    );
    harness.log.owner_thread_only();
    assert!(harness.gate.try_enter().is_some());
    assert_eq!(harness.log.count(&Record::CloseEvent(EVENT_TOKEN)), 0);
    drop(guard);
    assert_eq!(harness.log.count(&Record::CloseEvent(EVENT_TOKEN)), 1);
}

#[test]
fn startup_failures_are_explicit_never_zero_positions_and_cleanup_once() {
    for operation in [
        PointerNativeOperation::Callback,
        PointerNativeOperation::InstallHook,
        PointerNativeOperation::SetDpiContext,
        PointerNativeOperation::ReadCursor,
        PointerNativeOperation::RestoreDpiContext,
    ] {
        let harness = harness(Plan {
            failure: Some((operation, 42)),
            ..Plan::default()
        });
        let guard = ordinary(&harness);
        harness.pending.run();
        assert_eq!(
            harness
                .log
                .count(&Record::Ready(Err(failure(operation, 42)))),
            1
        );
        assert!(harness.log.positions().is_empty());
        assert!(
            !harness
                .log
                .records()
                .iter()
                .any(|record| matches!(record, Record::Failed(_)))
        );
        let hooked = !matches!(
            operation,
            PointerNativeOperation::Callback | PointerNativeOperation::InstallHook
        );
        assert_eq!(
            harness.log.count(&Record::RemoveHook(HOOK_TOKEN)),
            usize::from(hooked)
        );
        assert_eq!(harness.log.count(&Record::EndContext), 1);
        assert_eq!(harness.log.count(&Record::DropCalls), 1);
        let read = matches!(
            operation,
            PointerNativeOperation::ReadCursor | PointerNativeOperation::RestoreDpiContext
        );
        assert_eq!(harness.log.count(&Record::ReadCursor), usize::from(read));
        assert_eq!(
            harness.log.count(&Record::RestoreDpi(DPI_TOKEN)),
            usize::from(read)
        );
        harness.log.owner_thread_only();
        drop(guard);
        assert_eq!(harness.log.count(&Record::CloseEvent(EVENT_TOKEN)), 1);
        assert!(harness.gate.try_enter().is_some());
    }
}

#[test]
fn late_successful_install_is_removed_before_stopped_ready_without_seed() {
    let slot: GuardSlot = Arc::new(Mutex::new(None));
    let harness = harness(Plan {
        install_action: Some(retiring(&slot)),
        ..Plan::default()
    });
    *slot.lock() = Some(ordinary(&harness));
    harness.pending.run();
    assert_eq!(harness.log.count(&Record::InstallHook), 1);
    assert_eq!(harness.log.count(&Record::RemoveHook(HOOK_TOKEN)), 1);
    assert_eq!(
        harness
            .log
            .count(&Record::Ready(Err(PointerWatchError::Stopped))),
        1
    );
    assert_eq!(harness.log.count(&Record::ReadCursor), 0);
    assert!(harness.log.positions().is_empty());
    harness.log.before(
        Record::SignalEvent(EVENT_TOKEN),
        Record::RemoveHook(HOOK_TOKEN),
    );
    harness.log.before(
        Record::EndContext,
        Record::Ready(Err(PointerWatchError::Stopped)),
    );
    assert!(harness.gate.try_enter().is_some());
}

#[test]
fn cancellation_during_seed_restores_context_and_removes_hook_before_ready() {
    let slot: GuardSlot = Arc::new(Mutex::new(None));
    let harness = harness(Plan {
        read_action: Some(retiring(&slot)),
        ..Plan::default()
    });
    *slot.lock() = Some(ordinary(&harness));
    harness.pending.run();
    assert_eq!(
        harness
            .log
            .count(&Record::Ready(Err(PointerWatchError::Stopped))),
        1
    );
    assert_eq!(harness.log.count(&Record::ReadCursor), 1);
    assert_eq!(harness.log.count(&Record::RestoreDpi(DPI_TOKEN)), 1);
    assert!(harness.log.positions().is_empty());
    harness.log.before(
        Record::RestoreDpi(DPI_TOKEN),
        Record::RemoveHook(HOOK_TOKEN),
    );
    harness.log.before(
        Record::DropCalls,
        Record::Ready(Err(PointerWatchError::Stopped)),
    );
}

#[test]
fn negative_nonmove_retired_and_panicked_ffi_paths_always_chain_exact_arguments() {
    let log = Log::default();
    let local = Local::default();
    local.active.set(true);
    let point = PhysicalPoint { x: 4, y: -9 };
    let calls = RecordedHook {
        log: &log,
        local: &local,
        point,
        panic_copy: false,
    };
    assert_eq!(hook_callback(&calls, -3, WM_MOUSEMOVE, 0), CHAIN_RESULT);
    assert_eq!(hook_callback(&calls, 1, WM_MOUSEMOVE, 0), CHAIN_RESULT);
    assert_eq!(
        hook_callback(&calls, HC_ACTION, WM_MOUSEMOVE + 1, 0),
        CHAIN_RESULT
    );
    assert!(local.latest.get().is_none());
    assert_eq!(
        hook_callback(&calls, HC_ACTION, WM_MOUSEMOVE, POINT_TOKEN),
        CHAIN_RESULT
    );
    assert_eq!(local.latest.take(), Some(point));
    local.active.set(false);
    assert_eq!(
        hook_callback(&calls, HC_ACTION, WM_MOUSEMOVE, POINT_TOKEN),
        CHAIN_RESULT
    );
    assert!(local.latest.get().is_none());
    local.active.set(true);
    let calls = RecordedHook {
        panic_copy: true,
        ..calls
    };
    assert_eq!(
        hook_callback(&calls, HC_ACTION, WM_MOUSEMOVE, POINT_TOKEN),
        CHAIN_RESULT
    );
    assert!(local.failed.get());
    assert_eq!(log.count(&Record::Copy(point)), 1);
    assert_eq!(log.count(&Record::CopyPanic), 1);
    let records = log.records();
    assert_eq!(
        records.last(),
        Some(&Record::Chain(HC_ACTION, WM_MOUSEMOVE, POINT_TOKEN))
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record, Record::Chain(..)))
            .count(),
        6
    );
    assert!(!records.iter().any(|record| matches!(
        record,
        Record::Ready(_) | Record::Position(_) | Record::Failed(_)
    )));
}

#[test]
fn ffi_burst_copies_only_latest_and_owner_delivers_outside_ffi() {
    let points: Vec<_> = (0..100).map(|x| PhysicalPoint { x, y: -22 }).collect();
    let last = *points.last().unwrap();
    let harness = harness(Plan {
        actions: VecDeque::from([Action::Burst(points), Action::Fail(58)]),
        ..Plan::default()
    });
    let guard = ordinary(&harness);
    harness.pending.run();
    assert_eq!(harness.log.positions(), vec![SEED, last]);
    assert_eq!(
        harness
            .log
            .records()
            .iter()
            .filter(|record| matches!(record, Record::Chain(..)))
            .count(),
        100
    );
    assert_eq!(harness.log.count(&Record::Dispatch), 0);
    harness
        .log
        .before(Record::Copy(last), Record::Position(last));
    drop(guard);
}

#[test]
fn flooded_message_queue_is_coalesced_in_32_message_batches_without_polling() {
    let messages = (1..=80)
        .map(|x| Message::Move(PhysicalPoint { x, y: 123 }))
        .collect();
    let harness = harness(Plan {
        actions: VecDeque::from([Action::Messages(messages), Action::Fail(59)]),
        ..Plan::default()
    });
    let guard = ordinary(&harness);
    harness.pending.run();
    assert_eq!(
        harness.log.positions(),
        vec![
            SEED,
            PhysicalPoint { x: 32, y: 123 },
            PhysicalPoint { x: 64, y: 123 },
            PhysicalPoint { x: 80, y: 123 }
        ]
    );
    assert_eq!(harness.log.count(&Record::Dispatch), 80);
    assert_eq!(harness.log.count(&Record::ReadCursor), 1);
    assert_eq!(
        harness
            .log
            .count(&Record::Wait(EVENT_TOKEN, SAFETY_WAIT_MS)),
        4
    );
    let mut batch = 0;
    for record in harness.log.records() {
        match record {
            Record::Dispatch => {
                batch += 1;
                assert!(batch <= PUMP_BUDGET);
            }
            Record::Wait(..) => batch = 0,
            _ => {}
        }
    }
    drop(guard);
}

#[test]
fn ready_and_position_consumers_can_drop_the_guard_without_joins_or_later_delivery() {
    for in_ready in [true, false] {
        let slot: GuardSlot = Arc::new(Mutex::new(None));
        let harness = harness(Plan {
            actions: VecDeque::from([Action::Burst(vec![PhysicalPoint { x: 1, y: 2 }])]),
            ..Plan::default()
        });
        let log = harness.log.clone();
        let ready_slot = slot.clone();
        let ready_log = log.clone();
        let events_slot = slot.clone();
        let events: PointerWatchCallback = Arc::new(move |event| {
            assert!(!log.0.in_ffi.load(Ordering::Acquire));
            match event {
                PointerWatchEvent::Position(point) => {
                    log.push(Record::Position(point));
                    if !in_ready {
                        retire(&events_slot);
                    }
                }
                PointerWatchEvent::Failed(error) => log.push(Record::Failed(error)),
                PointerWatchEvent::Environment(environment) => {
                    log.push(Record::Environment(environment))
                }
            }
        });
        let ready: PointerWatchCompletion = Box::new(move |result| {
            ready_log.push(Record::Ready(result));
            if in_ready {
                retire(&ready_slot);
            }
        });
        *slot.lock() = Some(harness.host.watch(events, ready).unwrap());
        harness.pending.run();
        assert_eq!(harness.log.count(&Record::Ready(Ok(()))), 1);
        assert_eq!(
            harness.log.positions(),
            if in_ready { vec![] } else { vec![SEED] }
        );
        assert_eq!(
            harness
                .log
                .count(&Record::Wait(EVENT_TOKEN, SAFETY_WAIT_MS)),
            0
        );
        assert_eq!(harness.log.count(&Record::RemoveHook(HOOK_TOKEN)), 1);
        assert_eq!(harness.log.count(&Record::CloseEvent(EVENT_TOKEN)), 1);
        assert!(
            !harness
                .log
                .records()
                .iter()
                .any(|record| matches!(record, Record::Failed(_)))
        );
        assert!(harness.gate.try_enter().is_some());
    }
}

#[test]
fn guard_drop_is_nonjoining_keeps_busy_and_handle_open_until_worker_cleanup() {
    let (entered_send, entered) = mpsc::channel();
    let (release, release_recv) = mpsc::channel();
    let harness = harness(Plan {
        actions: VecDeque::from([Action::Block {
            entered: entered_send,
            release: release_recv,
        }]),
        ..Plan::default()
    });
    let guard = ordinary(&harness);
    let pending = harness.pending.clone();
    let worker = thread::spawn(move || pending.run());
    entered.recv_timeout(DEADLINE).unwrap();
    let (dropped_send, dropped) = mpsc::channel();
    let dropper = thread::spawn(move || {
        drop(guard);
        dropped_send.send(()).unwrap();
    });
    dropped.recv_timeout(DEADLINE).unwrap();
    assert_eq!(harness.log.count(&Record::CloseEvent(EVENT_TOKEN)), 0);
    assert_eq!(harness.log.count(&Record::RemoveHook(HOOK_TOKEN)), 0);
    let (events, ready) = harness.log.callbacks();
    assert!(matches!(
        harness.host.watch(events, ready),
        Err(PointerWatchError::Busy)
    ));
    release.send(()).unwrap();
    worker.join().unwrap();
    dropper.join().unwrap();
    assert_eq!(harness.log.count(&Record::CloseEvent(EVENT_TOKEN)), 1);
    assert_eq!(harness.log.count(&Record::RemoveHook(HOOK_TOKEN)), 1);
    assert_eq!(harness.log.count(&Record::ReadCursor), 1);
    harness.log.owner_thread_only();
    assert!(harness.gate.try_enter().is_some());
}

#[test]
fn failed_stop_signal_is_recorded_and_local_safety_retirement_never_polls() {
    let slot: GuardSlot = Arc::new(Mutex::new(None));
    let harness = harness(Plan {
        signal_error: Some(88),
        actions: VecDeque::from([Action::Safety, Action::Call(retiring(&slot))]),
        ..Plan::default()
    });
    *slot.lock() = Some(ordinary(&harness));
    harness.pending.run();
    assert_eq!(harness.log.count(&Record::SignalError(88)), 1);
    assert_eq!(harness.log.count(&Record::Wait(EVENT_TOKEN, 250)), 2);
    assert_eq!(harness.log.count(&Record::ReadCursor), 1);
    assert_eq!(harness.log.count(&Record::InstallHook), 1);
    assert_eq!(harness.log.count(&Record::RemoveHook(HOOK_TOKEN)), 1);
    assert_eq!(harness.log.positions(), vec![SEED]);
    assert!(
        !harness
            .log
            .records()
            .iter()
            .any(|record| matches!(record, Record::Failed(_)))
    );
}

#[test]
fn pump_error_unexpected_stop_and_quit_are_terminal_once_after_cleanup() {
    for (action, code) in [
        (Action::Fail(99), 99),
        (Action::Stop, UNEXPECTED_PUMP),
        (
            Action::Messages(vec![
                Message::Quit,
                Message::Move(PhysicalPoint { x: 1, y: 2 }),
            ]),
            QUIT_PUMP,
        ),
    ] {
        let harness = harness(Plan {
            actions: VecDeque::from([action]),
            ..Plan::default()
        });
        let guard = ordinary(&harness);
        harness.pending.run();
        let failed = Record::Failed(failure(PointerNativeOperation::MessagePump, code));
        assert_eq!(harness.log.count(&failed), 1);
        assert_eq!(harness.log.count(&Record::Ready(Ok(()))), 1);
        assert_eq!(harness.log.count(&Record::RemoveHook(HOOK_TOKEN)), 1);
        assert_eq!(harness.log.count(&Record::EndContext), 1);
        assert_eq!(harness.log.count(&Record::Dispatch), 0);
        harness.log.before(Record::DropCalls, failed);
        drop(guard);
    }
}

#[test]
fn removal_failure_is_explicit_and_token_context_cleanup_is_still_once() {
    let harness = harness(Plan {
        failure: Some((PointerNativeOperation::RemoveHook, 100)),
        reenter_remove: true,
        ..Plan::default()
    });
    let guard = ordinary(&harness);
    harness.pending.run();
    assert_eq!(
        harness.log.count(&Record::Failed(failure(
            PointerNativeOperation::RemoveHook,
            100
        ))),
        1
    );
    assert_eq!(harness.log.count(&Record::RemoveHook(HOOK_TOKEN)), 1);
    assert_eq!(harness.log.count(&Record::EndContext), 1);
    assert_eq!(harness.log.count(&Record::DropCalls), 1);
    assert_eq!(
        harness
            .log
            .count(&Record::Copy(PhysicalPoint { x: 999, y: 999 })),
        0
    );
    assert_eq!(
        harness
            .log
            .count(&Record::Chain(HC_ACTION, WM_MOUSEMOVE, POINT_TOKEN)),
        1
    );
    assert_eq!(harness.log.positions(), vec![SEED]);
    assert!(harness.gate.try_enter().is_some());
    drop(guard);
    assert_eq!(harness.log.count(&Record::CloseEvent(EVENT_TOKEN)), 1);
}

#[test]
fn independent_factory_instances_share_busy_until_cancelled_worker_releases_gate() {
    let gate = FlightGate::default();
    let first = harness(Plan::default());
    let second_log = Log::default();
    let second_pending = Pending::default();
    let first_host = host_with(
        RecordingFactory {
            log: first.log.clone(),
            plan: Mutex::new(None),
            stop_error: None,
            calls_error: None,
        },
        first.pending.clone(),
        gate.clone(),
    );
    let second_host = host_with(
        RecordingFactory {
            log: second_log.clone(),
            plan: Mutex::new(None),
            stop_error: None,
            calls_error: None,
        },
        second_pending.clone(),
        gate.clone(),
    );
    let (events, ready) = first.log.callbacks();
    let guard = first_host.watch(events, ready).unwrap();
    for _ in 0..2 {
        let (events, ready) = second_log.callbacks();
        assert!(matches!(
            second_host.watch(events, ready),
            Err(PointerWatchError::Busy)
        ));
    }
    assert!(second_log.records().is_empty());
    drop(guard);
    let (events, ready) = second_log.callbacks();
    assert!(matches!(
        second_host.watch(events, ready),
        Err(PointerWatchError::Busy)
    ));
    first.pending.run();
    let (events, ready) = second_log.callbacks();
    let next = second_host.watch(events, ready).unwrap();
    drop(next);
    second_pending.run();
    assert_eq!(
        first
            .log
            .count(&Record::Ready(Err(PointerWatchError::Stopped))),
        1
    );
    assert_eq!(
        second_log.count(&Record::Ready(Err(PointerWatchError::Stopped))),
        1
    );
}

#[test]
fn consumer_panics_are_contained_after_tls_delivery_and_native_resources_are_retired() {
    for ready_panics in [true, false] {
        let harness = harness(Plan::default());
        let log = harness.log.clone();
        let events: PointerWatchCallback = Arc::new(move |event| {
            assert!(!log.0.in_ffi.load(Ordering::Acquire));
            match event {
                PointerWatchEvent::Failed(error) => log.push(Record::Failed(error)),
                PointerWatchEvent::Environment(environment) => {
                    log.push(Record::Environment(environment))
                }
                PointerWatchEvent::Position(_) => panic!("recorded consumer panic"),
            }
        });
        let ready_log = harness.log.clone();
        let ready: PointerWatchCompletion = Box::new(move |result| {
            ready_log.push(Record::Ready(result));
            if ready_panics {
                panic!("recorded ready panic");
            }
        });
        let guard = harness.host.watch(events, ready).unwrap();
        harness.pending.run();
        assert_eq!(harness.log.count(&Record::Ready(Ok(()))), 1);
        assert_eq!(harness.log.count(&Record::RemoveHook(HOOK_TOKEN)), 1);
        assert_eq!(harness.log.count(&Record::EndContext), 1);
        assert_eq!(
            harness.log.count(&Record::Failed(failure(
                PointerNativeOperation::Callback,
                LOCAL_FAILURE
            ))),
            usize::from(!ready_panics)
        );
        assert!(harness.gate.try_enter().is_some());
        drop(guard);
    }
}

#[cfg(windows)]
#[test]
fn pinned_sdk_constants_match_the_passive_mouse_and_bounded_wait_adapter() {
    use windows_sys::Win32::UI::WindowsAndMessaging;
    assert_eq!(WindowsAndMessaging::HC_ACTION as i32, HC_ACTION);
    assert_eq!(WindowsAndMessaging::WM_MOUSEMOVE as usize, WM_MOUSEMOVE);
    assert_eq!(WindowsAndMessaging::WH_MOUSE_LL, 14);
    assert_eq!(WindowsAndMessaging::MWMO_INPUTAVAILABLE, 4);
    assert_eq!(WindowsAndMessaging::QS_ALLINPUT, 1279);
    use windows_sys::Win32::UI::Controls;
    assert_eq!(Controls::POINTER_DEVICE_TYPE_TOUCH, TOUCH);
    assert_eq!(Controls::POINTER_DEVICE_TYPE_INTEGRATED_PEN, INTEGRATED_PEN);
    assert_eq!(Controls::POINTER_DEVICE_TYPE_EXTERNAL_PEN, EXTERNAL_PEN);
    assert_eq!(Controls::POINTER_DEVICE_TYPE_TOUCH_PAD, TOUCH_PAD);
    assert_eq!(
        WindowsAndMessaging::NID_INTEGRATED_TOUCH | WindowsAndMessaging::NID_EXTERNAL_TOUCH,
        super::environment::TOUCH_BITS
    );
    assert_eq!(PUMP_BUDGET, 32);
    assert_eq!(SAFETY_WAIT_MS, 250);
}

#[test]
fn retirement_during_dispatch_checks_stop_before_the_next_message_and_denies_copied_hints() {
    let slot: GuardSlot = Arc::new(Mutex::new(None));
    let mut messages = vec![
        Message::Move(PhysicalPoint { x: 1, y: 2 }),
        Message::Call(RefCell::new(Some(retiring(&slot)))),
    ];
    messages.extend((3..100).map(|x| Message::Move(PhysicalPoint { x, y: 2 })));
    let harness = harness(Plan {
        actions: VecDeque::from([Action::Messages(messages)]),
        ..Plan::default()
    });
    *slot.lock() = Some(ordinary(&harness));
    harness.pending.run();
    assert_eq!(harness.log.count(&Record::Dispatch), 2);
    assert_eq!(harness.log.count(&Record::NextMessage), 2);
    assert_eq!(harness.log.positions(), vec![SEED]);
    assert_eq!(harness.log.count(&Record::RemoveHook(HOOK_TOKEN)), 1);
    assert_eq!(harness.log.count(&Record::CloseEvent(EVENT_TOKEN)), 1);
    assert!(
        !harness
            .log
            .records()
            .iter()
            .any(|record| matches!(record, Record::Failed(_)))
    );
}

#[test]
fn ffi_copy_fault_becomes_one_terminal_failure_only_after_owner_teardown() {
    let harness = harness(Plan {
        actions: VecDeque::from([Action::CallbackPanic]),
        ..Plan::default()
    });
    let guard = ordinary(&harness);
    harness.pending.run();
    let failed = Record::Failed(failure(PointerNativeOperation::Callback, LOCAL_FAILURE));
    assert_eq!(harness.log.positions(), vec![SEED]);
    assert_eq!(harness.log.count(&Record::CopyPanic), 1);
    assert_eq!(
        harness
            .log
            .count(&Record::Chain(HC_ACTION, WM_MOUSEMOVE, POINT_TOKEN)),
        1
    );
    assert_eq!(harness.log.count(&failed), 1);
    harness.log.before(Record::DropCalls, failed);
    drop(guard);
}

#[test]
fn touch_environment_uses_real_positive_bits_or_complete_checked_lists_not_ambiguous_zero() {
    let cases = [
        (
            EnvironmentPlan {
                bits: 1,
                count: Err(9),
                ..EnvironmentPlan::default()
            },
            Some(true),
            false,
        ),
        (
            EnvironmentPlan {
                bits: 2,
                count: Err(9),
                ..EnvironmentPlan::default()
            },
            Some(true),
            false,
        ),
        (
            EnvironmentPlan {
                bits: 0x80,
                count: Err(9),
                ..EnvironmentPlan::default()
            },
            None,
            true,
        ),
        (EnvironmentPlan::default(), Some(false), true),
        (
            EnvironmentPlan {
                count: Ok(1),
                kinds: vec![TOUCH],
                ..EnvironmentPlan::default()
            },
            Some(true),
            true,
        ),
        (
            EnvironmentPlan {
                count: Ok(3),
                kinds: vec![INTEGRATED_PEN, EXTERNAL_PEN, TOUCH_PAD],
                ..EnvironmentPlan::default()
            },
            Some(false),
            true,
        ),
        (
            EnvironmentPlan {
                count: Err(9),
                ..EnvironmentPlan::default()
            },
            None,
            true,
        ),
        (
            EnvironmentPlan {
                count: Ok(1),
                list_error: Some(8),
                ..EnvironmentPlan::default()
            },
            None,
            true,
        ),
        (
            EnvironmentPlan {
                count: Ok(1),
                kinds: vec![0],
                ..EnvironmentPlan::default()
            },
            None,
            true,
        ),
        (
            EnvironmentPlan {
                count: Ok(1),
                kinds: vec![1234],
                ..EnvironmentPlan::default()
            },
            None,
            true,
        ),
        (
            EnvironmentPlan {
                count: Ok(0),
                actual: Some(1),
                ..EnvironmentPlan::default()
            },
            None,
            true,
        ),
        (
            EnvironmentPlan {
                count: Ok(2),
                kinds: vec![INTEGRATED_PEN],
                ..EnvironmentPlan::default()
            },
            None,
            true,
        ),
        (
            EnvironmentPlan {
                count: Ok((DEVICE_CAP + 1) as u32),
                ..EnvironmentPlan::default()
            },
            None,
            true,
        ),
    ];
    for (environment, expected, queries_count) in cases {
        let harness = harness(Plan {
            environment,
            actions: VecDeque::from([Action::Safety, Action::Safety, Action::Fail(6)]),
            ..Plan::default()
        });
        let guard = ordinary(&harness);
        harness.pending.run();
        let environment = Record::Environment(PointerEnvironment {
            touch_capable: expected,
        });
        assert_eq!(harness.log.count(&environment), 1);
        assert_eq!(harness.log.count(&Record::Ready(Ok(()))), 1);
        assert_eq!(harness.log.count(&Record::DigitizerBits), 1);
        assert_eq!(
            harness.log.count(&Record::DeviceCount),
            usize::from(queries_count)
        );
        assert_eq!(harness.log.count(&Record::ReadCursor), 1);
        harness
            .log
            .before(Record::Ready(Ok(())), environment.clone());
        harness.log.before(environment, Record::Position(SEED));
        harness.log.owner_thread_only();
        drop(guard);
    }
}

#[test]
fn environment_consumer_self_drop_prevents_seed_and_never_runs_under_ffi_admission() {
    let slot: GuardSlot = Arc::new(Mutex::new(None));
    let harness = harness(Plan::default());
    let log = harness.log.clone();
    let events_slot = slot.clone();
    let events: PointerWatchCallback = Arc::new(move |event| {
        assert!(!log.0.in_ffi.load(Ordering::Acquire));
        match event {
            PointerWatchEvent::Environment(environment) => {
                log.push(Record::Environment(environment));
                retire(&events_slot);
            }
            PointerWatchEvent::Position(point) => log.push(Record::Position(point)),
            PointerWatchEvent::Failed(error) => log.push(Record::Failed(error)),
        }
    });
    let ready_log = harness.log.clone();
    let ready: PointerWatchCompletion =
        Box::new(move |result| ready_log.push(Record::Ready(result)));
    *slot.lock() = Some(harness.host.watch(events, ready).unwrap());
    harness.pending.run();
    assert_eq!(harness.log.count(&Record::Ready(Ok(()))), 1);
    assert_eq!(
        harness.log.count(&Record::Environment(PointerEnvironment {
            touch_capable: Some(false)
        })),
        1
    );
    assert!(harness.log.positions().is_empty());
    assert_eq!(
        harness
            .log
            .count(&Record::Wait(EVENT_TOKEN, SAFETY_WAIT_MS)),
        0
    );
    assert_eq!(harness.log.count(&Record::RemoveHook(HOOK_TOKEN)), 1);
    assert_eq!(harness.log.count(&Record::CloseEvent(EVENT_TOKEN)), 1);
}

#[test]
fn accepted_adapter_creation_failure_completes_once_without_hook_or_event_delivery() {
    let log = Log::default();
    let pending = Pending::default();
    let gate = FlightGate::default();
    let host = host_with(
        RecordingFactory {
            log: log.clone(),
            plan: Mutex::new(None),
            stop_error: None,
            calls_error: Some(73),
        },
        pending.clone(),
        gate.clone(),
    );
    let (events, ready) = log.callbacks();
    let guard = host.watch(events, ready).unwrap();
    pending.run();
    assert_eq!(
        log.count(&Record::Ready(Err(failure(
            PointerNativeOperation::Callback,
            73
        )))),
        1
    );
    assert_eq!(log.count(&Record::BeginContext), 0);
    assert_eq!(log.count(&Record::InstallHook), 0);
    assert!(log.positions().is_empty());
    assert!(
        !log.records()
            .iter()
            .any(|record| matches!(record, Record::Environment(_) | Record::Failed(_)))
    );
    assert!(gate.try_enter().is_some());
    drop(guard);
    assert_eq!(log.count(&Record::CloseEvent(EVENT_TOKEN)), 1);
}
