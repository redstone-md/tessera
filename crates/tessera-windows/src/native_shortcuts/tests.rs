// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Production actor tests through a recording backend; no Windows calls, hook
//! installation, input injection, timer scans, or consumer callbacks on owner.

use super::{Backend, BackendEvent, Gate, RawTrigger, Wake, actor};
use std::rc::Rc;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError},
};
use std::thread::{self, ThreadId};
use std::time::Duration;
use tessera_system::shortcuts::{
    KeyChord, KeyModifiers, ShortcutAction, ShortcutBindingState, ShortcutConfig, ShortcutError,
    ShortcutErrorKind, ShortcutEvent, ShortcutHost, ShortcutSnapshot, ShortcutSubscription,
    ShortcutTrigger, TriggerPoint,
};

const DEADLINE: Duration = Duration::from_secs(5);
const INPUT_CAPACITY: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Factory,
    Install,
    Cleanup,
}

struct Pause {
    entered: SyncSender<()>,
    release: Receiver<()>,
}

impl Pause {
    fn block(self) {
        self.entered.try_send(()).expect("barrier admitted once");
        self.release
            .recv_timeout(DEADLINE)
            .expect("barrier released");
    }
}

struct Hold {
    entered: Receiver<()>,
    release: Option<SyncSender<()>>,
}

impl Hold {
    fn entered(&self) {
        self.entered
            .recv_timeout(DEADLINE)
            .expect("owner or callback reached barrier");
    }

    fn release(mut self) {
        self.release
            .take()
            .expect("barrier not released")
            .try_send(())
            .expect("release barrier");
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.try_send(());
        }
    }
}

fn barrier() -> (Pause, Hold) {
    let (entered, notification) = mpsc::sync_channel(1);
    let (release, receiver) = mpsc::sync_channel(1);
    (
        Pause {
            entered,
            release: receiver,
        },
        Hold {
            entered: notification,
            release: Some(release),
        },
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Authority {
    generation: u64,
    listening: bool,
    closed: bool,
}

impl Authority {
    fn read(gate: &Gate) -> Self {
        Self {
            generation: gate.generation.load(Ordering::Acquire),
            listening: gate.listening.load(Ordering::Acquire),
            closed: gate.closed.load(Ordering::Acquire),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Operation {
    Construct,
    Install(u64),
    Register(KeyChord, u64),
    Cleanup {
        launcher: bool,
        settings: bool,
        authority: Authority,
    },
    TriggerRead(u64),
    Cursor,
    WaitFault(ShortcutError),
    RearmRead,
    Drop,
}

struct Scenario {
    launcher_error: Option<ShortcutError>,
    settings_error: Option<ShortcutError>,
    cleanup_error: Option<ShortcutError>,
    cursor: Option<TriggerPoint>,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            launcher_error: None,
            settings_error: None,
            cleanup_error: None,
            cursor: Some(TriggerPoint { x: -480, y: 240 }),
        }
    }
}

struct Record {
    operations: Vec<(ThreadId, Operation)>,
    gate: Option<Arc<Gate>>,
    pause: Option<(Phase, Pause)>,
    scenario: Scenario,
}

#[derive(Debug)]
enum Input {
    Wake,
    Trigger(RawTrigger),
    Fault(ShortcutError),
    Rearm,
    Fence(SyncSender<()>),
}

struct Control {
    record: Mutex<Record>,
    changed: Condvar,
    input: SyncSender<Input>,
}

impl Control {
    fn record(&self, operation: Operation, phase: Option<Phase>) {
        let pause = {
            let mut record = self.record.lock().expect("record lock");
            record.operations.push((thread::current().id(), operation));
            let pause = if record
                .pause
                .as_ref()
                .is_some_and(|(armed, _)| Some(*armed) == phase)
            {
                record.pause.take().map(|(_, pause)| pause)
            } else {
                None
            };
            self.changed.notify_all();
            pause
        };
        if let Some(pause) = pause {
            pause.block();
        }
    }

    fn inject(&self, input: Input) {
        // The same mutex brackets condition checks and notifications, preventing
        // a lost wake between try_recv and the backend's blocking wait.
        let record = self.record.lock().expect("input lock");
        match self.input.try_send(input) {
            Ok(()) => self.changed.notify_all(),
            Err(mpsc::TrySendError::Disconnected(Input::Wake)) => {
                // The Rust command wake can retire the owner before its saved
                // native wake runs. Like the SDK's Arc-owned event, that is safe.
                assert!(
                    record
                        .gate
                        .as_ref()
                        .is_some_and(|gate| gate.closed.load(Ordering::Acquire)),
                    "only a closed owner may retire fixture input"
                );
                assert_eq!(
                    record.operations.last().map(|(_, operation)| operation),
                    Some(&Operation::Drop),
                    "a late wake requires completed backend resource retirement"
                );
            }
            Err(error) => panic!("bounded fixture input has room: {error:?}"),
        }
    }
}

struct Retirement(SyncSender<()>);

impl Drop for Retirement {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

struct Fixture {
    control: Arc<Control>,
    retired: Receiver<()>,
}

impl Fixture {
    fn create(scenario: Scenario) -> (Self, Arc<dyn ShortcutHost>) {
        let (input, receiver) = mpsc::sync_channel(INPUT_CAPACITY);
        let (retired, retirement) = mpsc::sync_channel(1);
        let control = Arc::new(Control {
            record: Mutex::new(Record {
                operations: Vec::new(),
                gate: None,
                pause: None,
                scenario,
            }),
            changed: Condvar::new(),
            input,
        });
        let owner_control = control.clone();
        let notice = Retirement(retired);
        let host = actor::create(move |gate| {
            owner_control.record.lock().expect("factory record").gate = Some(gate.clone());
            owner_control.record(Operation::Construct, Some(Phase::Factory));
            Ok(RecordingBackend {
                control: owner_control,
                input: receiver,
                gate,
                owner: thread::current().id(),
                launcher: false,
                settings: false,
                // Deliberately !Send: construction and retirement belong to the
                // installing owner, not to the thread calling the lazy factory.
                _owner_only: Rc::new(()),
                _retirement: notice,
            })
        })
        .expect("create recording host");
        (
            Self {
                control,
                retired: retirement,
            },
            host,
        )
    }

    fn arm(&self, phase: Phase) -> Hold {
        let (pause, hold) = barrier();
        let mut record = self.control.record.lock().expect("arm record");
        assert!(record.pause.is_none(), "one recording barrier at a time");
        record.pause = Some((phase, pause));
        hold
    }

    fn trigger(&self, generation: u64, action: ShortcutAction) {
        self.control.inject(Input::Trigger(RawTrigger {
            generation,
            action,
            reserved: false,
        }));
    }

    fn fence(&self) {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.control.inject(Input::Fence(sender));
        receive_once(&receiver);
    }

    fn fail_wait(&self, error: ShortcutError) {
        self.control.inject(Input::Fault(error));
    }

    fn request_rearm(&self) {
        self.control.inject(Input::Rearm);
    }

    fn set_cleanup_error(&self, error: Option<ShortcutError>) {
        self.control
            .record
            .lock()
            .expect("cleanup scenario")
            .scenario
            .cleanup_error = error;
    }

    fn set_launcher_error(&self, error: Option<ShortcutError>) {
        self.control
            .record
            .lock()
            .expect("launcher scenario")
            .scenario
            .launcher_error = error;
    }

    fn gate(&self) -> Arc<Gate> {
        self.control
            .record
            .lock()
            .expect("gate record")
            .gate
            .clone()
            .expect("backend initialized")
    }

    fn operations(&self) -> Vec<(ThreadId, Operation)> {
        self.control
            .record
            .lock()
            .expect("operations record")
            .operations
            .clone()
    }

    fn cursor_count(&self) -> usize {
        self.operations()
            .iter()
            .filter(|(_, operation)| *operation == Operation::Cursor)
            .count()
    }

    fn retired(&self) {
        receive_once(&self.retired);
    }

    fn close(&self, host: Arc<dyn ShortcutHost>) {
        drop(host);
        self.retired();
        let operations = self.operations();
        if let Some((owner, _)) = operations.first() {
            assert_ne!(*owner, thread::current().id());
            assert!(
                operations.iter().all(|(thread, _)| thread == owner),
                "all backend work stays on one owner"
            );
            assert_eq!(
                operations.last().map(|(_, operation)| operation),
                Some(&Operation::Drop)
            );
        }
    }
}

struct RecordingBackend {
    control: Arc<Control>,
    input: Receiver<Input>,
    gate: Arc<Gate>,
    owner: ThreadId,
    launcher: bool,
    settings: bool,
    _owner_only: Rc<()>,
    _retirement: Retirement,
}

impl RecordingBackend {
    fn assert_owner(&self) {
        assert_eq!(thread::current().id(), self.owner, "backend owner changed");
    }
}

impl Backend for RecordingBackend {
    fn wake(&self) -> Wake {
        self.assert_owner();
        let control = self.control.clone();
        Arc::new(move || control.inject(Input::Wake))
    }

    fn install_launcher(&mut self, generation: u64) -> Result<(), ShortcutError> {
        self.assert_owner();
        assert!(
            !self.launcher && !self.settings,
            "retire every old registration before a hook"
        );
        self.control
            .record(Operation::Install(generation), Some(Phase::Install));
        if let Some(error) = self
            .control
            .record
            .lock()
            .expect("launcher result")
            .scenario
            .launcher_error
            .clone()
        {
            return Err(error);
        }
        self.launcher = true;
        Ok(())
    }

    fn register_settings(
        &mut self,
        chord: &KeyChord,
        generation: u64,
    ) -> Result<(), ShortcutError> {
        self.assert_owner();
        assert!(
            !self.settings,
            "never retain two registrations for the owned ID"
        );
        self.control
            .record(Operation::Register(*chord, generation), None);
        if let Some(error) = self
            .control
            .record
            .lock()
            .expect("settings result")
            .scenario
            .settings_error
            .clone()
        {
            return Err(error);
        }
        self.settings = true;
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), ShortcutError> {
        self.assert_owner();
        self.control.record(
            Operation::Cleanup {
                launcher: self.launcher,
                settings: self.settings,
                authority: Authority::read(&self.gate),
            },
            Some(Phase::Cleanup),
        );
        if let Some(error) = self
            .control
            .record
            .lock()
            .expect("cleanup result")
            .scenario
            .cleanup_error
            .clone()
        {
            return Err(error);
        }
        self.launcher = false;
        self.settings = false;
        Ok(())
    }

    fn wait(&mut self) -> Result<Option<BackendEvent>, ShortcutError> {
        self.assert_owner();
        let input = {
            let mut record = self.control.record.lock().expect("wait record");
            loop {
                match self.input.try_recv() {
                    Ok(input) => break input,
                    Err(TryRecvError::Empty) => {
                        record = self.control.changed.wait(record).expect("recording wake");
                    }
                    Err(TryRecvError::Disconnected) => return Err(super::unavailable()),
                }
            }
        };
        match input {
            Input::Wake => Ok(None),
            Input::Trigger(trigger) => {
                self.control
                    .record(Operation::TriggerRead(trigger.generation), None);
                Ok(Some(BackendEvent::Triggered(trigger)))
            }
            Input::Fence(sender) => {
                sender.try_send(()).expect("recording fence");
                Ok(None)
            }
            Input::Fault(error) => {
                self.control
                    .record(Operation::WaitFault(error.clone()), None);
                Err(error)
            }
            Input::Rearm => {
                self.control.record(Operation::RearmRead, None);
                Ok(Some(BackendEvent::RearmRequired))
            }
        }
    }

    fn cursor(&mut self) -> Option<TriggerPoint> {
        self.assert_owner();
        self.control.record(Operation::Cursor, None);
        self.control
            .record
            .lock()
            .expect("cursor result")
            .scenario
            .cursor
    }
}

impl Drop for RecordingBackend {
    fn drop(&mut self) {
        self.assert_owner();
        assert!(
            !self.launcher && !self.settings,
            "owner retires native resources before backend drop"
        );
        self.control.record(Operation::Drop, None);
    }
}

fn receive_once<T>(receiver: &Receiver<T>) -> T {
    let value = receiver
        .recv_timeout(DEADLINE)
        .expect("accepted delivery arrived");
    assert!(
        matches!(
            receiver.recv_timeout(DEADLINE),
            Err(RecvTimeoutError::Disconnected)
        ),
        "exactly one delivery"
    );
    value
}

fn configure(
    host: &dyn ShortcutHost,
    config: ShortcutConfig,
) -> Result<ShortcutSnapshot, ShortcutError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    host.configure(
        config,
        Box::new(move |result| sender.try_send(result).expect("one completion")),
    )
    .expect("configuration accepted");
    receive_once(&receiver)
}

fn subscribe(
    host: &dyn ShortcutHost,
) -> (
    Box<dyn ShortcutSubscription>,
    Receiver<(ShortcutEvent, ThreadId)>,
) {
    let (sender, receiver) = mpsc::sync_channel(8);
    let subscription = host
        .subscribe(Arc::new(move |event| {
            sender
                .try_send((event, thread::current().id()))
                .expect("bounded event recording");
        }))
        .expect("subscribe recording consumer");
    (subscription, receiver)
}

#[test]
fn disabled_configuration_is_lazy_and_close_retires_the_uninvoked_factory() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let (subscription, _events) = subscribe(host.as_ref());
    for _ in 0..2 {
        let snapshot = configure(host.as_ref(), ShortcutConfig::default().with_enabled(false))
            .expect("disabled snapshot");
        assert!(!snapshot.enabled);
        assert!(snapshot.generation > 0);
        assert_eq!(snapshot.bindings.len(), 2);
        assert!(snapshot.bindings.iter().all(|binding| binding.state
            == ShortcutBindingState::Paused
            && binding.error.is_none()));
        assert!(
            fixture.operations().is_empty(),
            "disabled configuration never initializes a backend"
        );
    }
    drop(subscription);
    fixture.close(host);
    assert!(fixture.operations().is_empty());
}

#[test]
fn bounded_admission_completes_every_ok_once_and_busy_accepts_no_callback() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let (pause, hold) = barrier();
    let (first_sender, first_receiver) = mpsc::sync_channel(1);
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |result| {
            pause.block();
            first_sender.try_send(result).expect("first completion");
        }),
    )
    .expect("first accepted");
    hold.entered();

    let mut accepted = Vec::new();
    for id in 0..16 {
        let (sender, receiver) = mpsc::sync_channel(1);
        host.configure(
            ShortcutConfig::default().with_enabled(false),
            Box::new(move |result| {
                sender.try_send((id, result)).expect("accepted completion");
            }),
        )
        .expect("bounded slot accepted");
        accepted.push(receiver);
        fixture.fence();
    }
    let (rejected_sender, rejected_receiver) = mpsc::sync_channel(1);
    let error = host
        .configure(
            ShortcutConfig::default().with_enabled(false),
            Box::new(move |result| {
                rejected_sender
                    .try_send(result)
                    .expect("rejected callback must not run");
            }),
        )
        .expect_err("seventeenth pending request rejects promptly");
    assert_eq!(error.kind(), ShortcutErrorKind::Busy);
    assert!(
        matches!(
            rejected_receiver.try_recv(),
            Err(TryRecvError::Disconnected)
        ),
        "Err owns no completion"
    );

    hold.release();
    receive_once(&first_receiver).expect("initial request completed");
    for (expected, receiver) in accepted.iter().enumerate() {
        let (id, result) = receive_once(receiver);
        assert_eq!(id, expected);
        let snapshot =
            result.expect("owner processed each accepted request before its recording fence");
        assert!(!snapshot.enabled);
    }
    assert_eq!(fixture.cursor_count(), 0);
    assert_eq!(
        fixture
            .operations()
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Construct))
            .count(),
        1
    );
    fixture.close(host);
}

#[test]
fn partial_registration_results_keep_both_binding_observations_truthful() {
    let conflict = ShortcutError::new(ShortcutErrorKind::Conflict, Some(1409), "recorded conflict");
    let denied = ShortcutError::new(ShortcutErrorKind::AccessDenied, Some(5), "recorded denial");
    for (scenario, launcher_state, settings_state, launcher_error, settings_error) in [
        (
            Scenario {
                settings_error: Some(conflict.clone()),
                ..Scenario::default()
            },
            ShortcutBindingState::Registered,
            ShortcutBindingState::Conflict,
            None,
            Some(conflict),
        ),
        (
            Scenario {
                launcher_error: Some(denied.clone()),
                ..Scenario::default()
            },
            ShortcutBindingState::Denied,
            ShortcutBindingState::Registered,
            Some(denied),
            None,
        ),
    ] {
        let (fixture, host) = Fixture::create(scenario);
        let config = ShortcutConfig::default();
        let snapshot = configure(host.as_ref(), config.clone())
            .expect("partial native registration is a snapshot, not all-or-nothing success");
        assert!(snapshot.enabled);
        assert_eq!(snapshot.bindings.len(), 2);
        let launcher = snapshot.binding(ShortcutAction::ToggleLauncher);
        assert_eq!(launcher.action, ShortcutAction::ToggleLauncher);
        assert_eq!(launcher.chord, KeyChord::BareWin);
        assert_eq!(launcher.state, launcher_state);
        assert_eq!(launcher.error, launcher_error);
        let settings = snapshot.binding(ShortcutAction::OpenSettings);
        assert_eq!(settings.action, ShortcutAction::OpenSettings);
        assert_eq!(
            settings.chord,
            config.effective_chord(ShortcutAction::OpenSettings)
        );
        assert_eq!(settings.state, settings_state);
        assert_eq!(settings.error, settings_error);
        fixture.close(host);
    }
}

#[test]
fn reconfigure_and_pause_retire_every_binding_before_reusing_one_owner() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let default = ShortcutConfig::default();
    let edited_chord = KeyChord::Chord {
        modifiers: KeyModifiers {
            control: true,
            shift: true,
            ..KeyModifiers::default()
        },
        key: 0x53,
    };
    let edited = default
        .clone()
        .with_settings_override(Some(edited_chord))
        .expect("valid edited chord");
    let first = configure(host.as_ref(), default.clone()).expect("first registrations");
    let second = configure(host.as_ref(), edited.clone()).expect("replace registrations");
    let paused =
        configure(host.as_ref(), edited.clone().with_enabled(false)).expect("pause registrations");
    let resumed = configure(host.as_ref(), edited).expect("resume registrations");
    assert!(
        first.generation < second.generation
            && second.generation < paused.generation
            && paused.generation < resumed.generation
    );
    assert!(
        paused
            .bindings
            .iter()
            .all(|binding| binding.state == ShortcutBindingState::Paused)
    );
    assert_eq!(
        fixture.cursor_count(),
        0,
        "configuration and idle wake never query the pointer"
    );
    let operations: Vec<_> = fixture
        .operations()
        .into_iter()
        .map(|(_, operation)| operation)
        .collect();
    assert_eq!(
        operations,
        vec![
            Operation::Construct,
            Operation::Install(first.generation),
            Operation::Register(
                default.effective_chord(ShortcutAction::OpenSettings),
                first.generation
            ),
            Operation::Cleanup {
                launcher: true,
                settings: true,
                authority: Authority {
                    generation: second.generation,
                    listening: false,
                    closed: false
                }
            },
            Operation::Install(second.generation),
            Operation::Register(edited_chord, second.generation),
            Operation::Cleanup {
                launcher: true,
                settings: true,
                authority: Authority {
                    generation: paused.generation,
                    listening: false,
                    closed: false
                }
            },
            Operation::Cleanup {
                launcher: false,
                settings: false,
                authority: Authority {
                    generation: resumed.generation,
                    listening: false,
                    closed: false
                }
            },
            Operation::Install(resumed.generation),
            Operation::Register(edited_chord, resumed.generation),
        ]
    );
    fixture.close(host);
}

#[test]
fn owner_rejects_stale_triggers_and_queries_cursor_once_outside_wait_and_callback() {
    for cursor in [Some(TriggerPoint { x: -480, y: 240 }), None] {
        let (fixture, host) = Fixture::create(Scenario {
            cursor,
            ..Scenario::default()
        });
        let (subscription, events) = subscribe(host.as_ref());
        let first = configure(host.as_ref(), ShortcutConfig::default()).expect("first generation");
        let current =
            configure(host.as_ref(), ShortcutConfig::default()).expect("current generation");
        fixture.trigger(first.generation, ShortcutAction::ToggleLauncher);
        fixture.trigger(current.generation, ShortcutAction::ToggleLauncher);
        fixture.fence();
        let (event, callback_thread) = events.recv_timeout(DEADLINE).expect("one current trigger");
        assert_eq!(
            event,
            ShortcutEvent::Triggered(ShortcutTrigger {
                generation: current.generation,
                action: ShortcutAction::ToggleLauncher,
                cursor,
            })
        );
        assert_eq!(fixture.cursor_count(), 1);
        let operations = fixture.operations();
        let (owner, _) = operations.first().expect("backend constructed");
        assert_ne!(
            *owner, callback_thread,
            "consumer never runs in hook or owner"
        );
        let cursor_index = operations
            .iter()
            .position(|(_, operation)| *operation == Operation::Cursor)
            .expect("cursor recorded");
        assert_eq!(
            operations[cursor_index - 1].1,
            Operation::TriggerRead(current.generation),
            "pointer lookup happens after the raw hook event has returned"
        );
        configure(host.as_ref(), ShortcutConfig::default().with_enabled(false))
            .expect("relay barrier");
        assert!(
            matches!(events.try_recv(), Err(TryRecvError::Empty)),
            "stale trigger was never delivered"
        );
        drop(subscription);
        fixture.close(host);
    }
}

#[test]
fn reconfiguration_blocks_an_already_queued_old_generation_at_the_relay() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let (subscription, events) = subscribe(host.as_ref());
    configure(host.as_ref(), ShortcutConfig::default()).expect("initial generation");
    let (pause, hold) = barrier();
    let (blocked_sender, blocked_receiver) = mpsc::sync_channel(1);
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |result| {
            blocked_sender.try_send(result).expect("blocked snapshot");
            pause.block();
        }),
    )
    .expect("blocked callback accepted");
    hold.entered();
    let old = blocked_receiver
        .recv_timeout(DEADLINE)
        .expect("blocked callback result")
        .expect("old generation");
    fixture.trigger(old.generation, ShortcutAction::ToggleLauncher);
    fixture.fence();
    assert_eq!(
        fixture.cursor_count(),
        1,
        "one old event admitted before replacement"
    );

    let (new_sender, new_receiver) = mpsc::sync_channel(1);
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |result| {
            new_sender.try_send(result).expect("replacement completion");
        }),
    )
    .expect("replacement accepted while consumer blocked");
    let current_generation = fixture.gate().generation.load(Ordering::Acquire);
    assert!(current_generation > old.generation);
    fixture.trigger(old.generation, ShortcutAction::OpenSettings);
    fixture.trigger(current_generation, ShortcutAction::OpenSettings);
    fixture.fence();
    assert_eq!(
        fixture.cursor_count(),
        2,
        "stale raw event does not query the pointer"
    );
    hold.release();
    assert!(matches!(
        blocked_receiver.recv_timeout(DEADLINE),
        Err(RecvTimeoutError::Disconnected)
    ));
    let current = receive_once(&new_receiver).expect("replacement completed");
    let (event, _) = events
        .recv_timeout(DEADLINE)
        .expect("current event delivered");
    assert_eq!(
        event,
        ShortcutEvent::Triggered(ShortcutTrigger {
            generation: current.generation,
            action: ShortcutAction::OpenSettings,
            cursor: Some(TriggerPoint { x: -480, y: 240 }),
        })
    );
    configure(host.as_ref(), ShortcutConfig::default().with_enabled(false)).expect("relay barrier");
    assert!(
        matches!(events.try_recv(), Err(TryRecvError::Empty)),
        "queued stale event cannot reach the sink"
    );
    drop(subscription);
    fixture.close(host);
}

#[test]
fn subscription_drop_revokes_queued_and_late_delivery_before_owner_cleanup() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let (subscription, events) = subscribe(host.as_ref());
    let (pause, hold) = barrier();
    let (snapshot_sender, snapshot_receiver) = mpsc::sync_channel(1);
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |result| {
            snapshot_sender.try_send(result).expect("active snapshot");
            pause.block();
        }),
    )
    .expect("active configuration accepted");
    hold.entered();
    let current = snapshot_receiver
        .recv_timeout(DEADLINE)
        .expect("active completion")
        .expect("active generation");
    fixture.trigger(current.generation, ShortcutAction::ToggleLauncher);
    fixture.fence();
    assert_eq!(
        fixture.cursor_count(),
        1,
        "event queued before the subscription drops"
    );
    let gate = fixture.gate();
    let cleanup = fixture.arm(Phase::Cleanup);
    drop(subscription);
    assert!(
        !gate.listening.load(Ordering::Acquire),
        "drop synchronously revokes listener authority"
    );
    assert_eq!(gate.generation.load(Ordering::Acquire), 0);
    cleanup.entered();
    fixture.trigger(current.generation, ShortcutAction::ToggleLauncher);
    cleanup.release();
    fixture.fence();
    assert_eq!(
        fixture.cursor_count(),
        1,
        "late callback does not query the pointer"
    );
    hold.release();
    assert!(matches!(
        snapshot_receiver.recv_timeout(DEADLINE),
        Err(RecvTimeoutError::Disconnected)
    ));
    configure(host.as_ref(), ShortcutConfig::default().with_enabled(false)).expect("relay barrier");
    assert!(
        matches!(events.try_recv(), Err(TryRecvError::Disconnected)),
        "neither queued nor late event reached the dropped sink"
    );
    assert!(
        fixture.operations().iter().any(|(_, operation)| matches!(
            operation,
            Operation::Cleanup {
                launcher: true,
                settings: true,
                authority: Authority {
                    generation: 0,
                    listening: false,
                    closed: false
                }
            }
        )),
        "cleanup observed authority already revoked"
    );
    fixture.close(host);
}

#[test]
fn completion_reentry_and_panic_do_not_hold_admission_or_strand_later_completions() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let reentrant = host.clone();
    let (outer_sender, outer_receiver) = mpsc::sync_channel(1);
    let (nested_sender, nested_receiver) = mpsc::sync_channel(1);
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |result| {
            outer_sender
                .try_send((result, thread::current().id()))
                .expect("outer completion");
            reentrant
                .configure(
                    ShortcutConfig::default().with_enabled(false),
                    Box::new(move |result| {
                        nested_sender.try_send(result).expect("nested completion");
                    }),
                )
                .expect("completion can reenter configure");
            panic!("recorded consumer completion panic");
        }),
    )
    .expect("outer accepted");
    let (outer, callback_thread) = receive_once(&outer_receiver);
    outer.expect("outer snapshot");
    let nested = receive_once(&nested_receiver).expect("relay survived callback panic");
    assert!(!nested.enabled);
    assert_ne!(fixture.operations()[0].0, callback_thread);
    fixture.close(host);
}

#[test]
fn sink_reentry_and_panic_leave_the_owner_and_next_event_usable() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let weak = Arc::downgrade(&host);
    let first_event = Arc::new(AtomicBool::new(true));
    let (events_sender, events) = mpsc::sync_channel(2);
    let (nested_sender, nested) = mpsc::sync_channel(1);
    let subscription = host
        .subscribe(Arc::new(move |event| {
            events_sender
                .try_send((event, thread::current().id()))
                .expect("sink event");
            if first_event.swap(false, Ordering::AcqRel) {
                let sender = nested_sender.clone();
                weak.upgrade()
                    .expect("live host")
                    .configure(
                        ShortcutConfig::default(),
                        Box::new(move |result| {
                            sender.try_send(result).expect("sink reentry completion");
                        }),
                    )
                    .expect("sink can reenter configure");
                panic!("recorded consumer sink panic");
            }
        }))
        .expect("subscribe reentrant sink");
    let first = configure(host.as_ref(), ShortcutConfig::default()).expect("first generation");
    fixture.trigger(first.generation, ShortcutAction::ToggleLauncher);
    let (first_event, callback_thread) = events.recv_timeout(DEADLINE).expect("first callback");
    assert!(
        matches!(first_event, ShortcutEvent::Triggered(trigger) if trigger.generation == first.generation)
    );
    let second = nested
        .recv_timeout(DEADLINE)
        .expect("sink reentry completed")
        .expect("second generation");
    assert!(second.generation > first.generation);
    fixture.trigger(second.generation, ShortcutAction::OpenSettings);
    fixture.fence();
    let (second_event, second_thread) = events.recv_timeout(DEADLINE).expect("sink survived panic");
    assert!(
        matches!(second_event, ShortcutEvent::Triggered(trigger) if trigger.generation == second.generation && trigger.action == ShortcutAction::OpenSettings)
    );
    assert_eq!(
        callback_thread, second_thread,
        "the same relay survives consumer panic"
    );
    assert_ne!(fixture.operations()[0].0, callback_thread);
    assert_eq!(fixture.cursor_count(), 2);
    drop(subscription);
    fixture.close(host);
}

struct CallbackOwners {
    host: Arc<dyn ShortcutHost>,
    subscription: Box<dyn ShortcutSubscription>,
}

#[test]
fn sink_can_drop_its_subscription_and_last_host_without_owner_join() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let owners = Arc::new(Mutex::new(None::<CallbackOwners>));
    let callback_owners = owners.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    let subscription = host
        .subscribe(Arc::new(move |event| {
            let owned = callback_owners
                .lock()
                .expect("callback owners")
                .take()
                .expect("callback owns resources");
            drop(owned.subscription);
            drop(owned.host);
            sender
                .try_send((event, thread::current().id()))
                .expect("drop callback returned promptly");
        }))
        .expect("subscribe dropping sink");
    let current = configure(host.as_ref(), ShortcutConfig::default()).expect("active generation");
    let gate = fixture.gate();
    *owners.lock().expect("transfer owners") = Some(CallbackOwners { host, subscription });
    fixture.trigger(current.generation, ShortcutAction::ToggleLauncher);
    let (event, callback_thread) = receiver
        .recv_timeout(DEADLINE)
        .expect("callback returned after dropping last host");
    assert!(
        matches!(event, ShortcutEvent::Triggered(trigger) if trigger.generation == current.generation)
    );
    assert_eq!(
        Authority::read(&gate),
        Authority {
            generation: 0,
            listening: false,
            closed: true
        }
    );
    fixture.retired();
    assert!(owners.lock().expect("retired owners").is_none());
    assert_ne!(fixture.operations()[0].0, callback_thread);
    assert_eq!(
        fixture.operations().last().map(|(_, operation)| operation),
        Some(&Operation::Drop)
    );
}

#[test]
fn host_close_preserves_every_accepted_completion_and_cleans_up_on_owner() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let install = fixture.arm(Phase::Install);
    let mut completions = Vec::new();
    let (sender, receiver) = mpsc::sync_channel(1);
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |result| {
            sender.try_send(result).expect("active accepted completion");
        }),
    )
    .expect("active accepted");
    completions.push(receiver);
    install.entered();
    for _ in 0..8 {
        let (sender, receiver) = mpsc::sync_channel(1);
        host.configure(
            ShortcutConfig::default(),
            Box::new(move |result| {
                sender.try_send(result).expect("queued accepted completion");
            }),
        )
        .expect("queued accepted before close");
        completions.push(receiver);
    }
    let gate = fixture.gate();
    drop(host);
    assert_eq!(
        Authority::read(&gate),
        Authority {
            generation: 0,
            listening: false,
            closed: true
        }
    );
    install.release();
    for (index, receiver) in completions.iter().enumerate() {
        let result = receive_once(receiver);
        if index != 0 {
            assert_eq!(
                result
                    .expect_err("queued accepted work completes with closed error")
                    .kind(),
                ShortcutErrorKind::Other
            );
        }
    }
    fixture.retired();
    let operations = fixture.operations();
    let owner = operations[0].0;
    assert!(operations.iter().all(|(thread, _)| *thread == owner));
    assert_eq!(
        operations
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Construct))
            .count(),
        1
    );
    assert_eq!(
        operations.last().map(|(_, operation)| operation),
        Some(&Operation::Drop)
    );
    assert!(operations.iter().any(|(_, operation)| matches!(
        operation,
        Operation::Cleanup {
            authority: Authority {
                generation: 0,
                listening: false,
                closed: true
            },
            ..
        }
    )));
}

#[test]
fn cleanup_failure_reports_typed_unavailable_and_late_commands_do_not_reinstall() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let (old_subscription, old_events) = subscribe(host.as_ref());
    let initial =
        configure(host.as_ref(), ShortcutConfig::default()).expect("initial registration");
    let error = ShortcutError::new(
        ShortcutErrorKind::AccessDenied,
        Some(5),
        "recorded cleanup failure",
    );
    fixture.set_cleanup_error(Some(error.clone()));
    let cleanup = fixture.arm(Phase::Cleanup);
    drop(old_subscription);
    cleanup.entered();
    fixture.trigger(initial.generation, ShortcutAction::ToggleLauncher);
    fixture.trigger(initial.generation, ShortcutAction::OpenSettings);
    let (subscription, events) = subscribe(host.as_ref());
    cleanup.release();
    let (event, callback_thread) = events
        .recv_timeout(DEADLINE)
        .expect("cleanup fault delivered");
    assert_eq!(event, ShortcutEvent::Unavailable(error.clone()));
    assert_eq!(fixture.gate().generation.load(Ordering::Acquire), 0);
    assert!(matches!(
        old_events.try_recv(),
        Err(TryRecvError::Disconnected)
    ));
    assert_ne!(fixture.operations()[0].0, callback_thread);
    for enabled in [true, false] {
        assert_eq!(
            configure(
                host.as_ref(),
                ShortcutConfig::default().with_enabled(enabled)
            ),
            Err(error.clone()),
            "terminal cleanup failure still completes accepted requests with its typed error"
        );
    }
    assert_eq!(fixture.cursor_count(), 0);
    assert_eq!(
        fixture
            .operations()
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Construct))
            .count(),
        1
    );
    assert_eq!(
        fixture
            .operations()
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Install(_)))
            .count(),
        1
    );
    assert!(
        matches!(events.try_recv(), Err(TryRecvError::Empty)),
        "one terminal fault, not a repeating unavailable stream"
    );
    fixture.set_cleanup_error(None);
    drop(subscription);
    fixture.close(host);
    assert_eq!(
        Authority::read(&fixture.gate()),
        Authority {
            generation: 0,
            listening: false,
            closed: true
        }
    );
    assert_eq!(
        fixture
            .operations()
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Register(..)))
            .count(),
        1,
        "the one settings registration is retired, never replaced"
    );
    assert!(
        matches!(
            events.recv_timeout(DEADLINE),
            Err(RecvTimeoutError::Disconnected)
        ),
        "close retires the sink without delivering old input or another fault"
    );
    // Deterministically exercise Host::drop's native wake after its Rust command
    // wake has already retired the receiver; the real close can interleave so.
    fixture.control.inject(Input::Wake);
}

#[test]
fn wait_fault_survives_full_event_delivery_and_faulted_close_drains_accepted_work() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let (subscription, events) = subscribe(host.as_ref());
    configure(host.as_ref(), ShortcutConfig::default()).expect("initial registration");
    let (pause, hold) = barrier();
    let (snapshot_sender, snapshot_receiver) = mpsc::sync_channel(1);
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |result| {
            snapshot_sender.try_send(result).expect("blocked snapshot");
            pause.block();
        }),
    )
    .expect("blocked callback accepted");
    hold.entered();
    let current = snapshot_receiver
        .recv_timeout(DEADLINE)
        .expect("blocked completion")
        .expect("current registrations");
    for _ in 0..33 {
        fixture.trigger(current.generation, ShortcutAction::ToggleLauncher);
    }
    fixture.fence();
    assert_eq!(
        fixture.cursor_count(),
        32,
        "the overflow event does not reserve a cursor query"
    );
    let error = ShortcutError::new(
        ShortcutErrorKind::Other,
        Some(6),
        "recorded owner wait failure",
    );
    let cleanup = fixture.arm(Phase::Cleanup);
    fixture.fail_wait(error.clone());
    cleanup.entered();
    assert_eq!(
        fixture.gate().generation.load(Ordering::Acquire),
        0,
        "fault revokes old generations before cleanup"
    );
    cleanup.release();
    hold.release();
    assert!(matches!(
        snapshot_receiver.recv_timeout(DEADLINE),
        Err(RecvTimeoutError::Disconnected)
    ));
    let (event, callback_thread) = events
        .recv_timeout(DEADLINE)
        .expect("fault has a slot even behind 32 queued triggers");
    assert_eq!(event, ShortcutEvent::Unavailable(error.clone()));
    assert_ne!(fixture.operations()[0].0, callback_thread);
    assert_eq!(
        configure(host.as_ref(), ShortcutConfig::default()),
        Err(error.clone()),
        "faulted owner answers late accepted work without another backend"
    );
    assert!(
        matches!(events.try_recv(), Err(TryRecvError::Empty)),
        "queued old-generation triggers and duplicate faults are suppressed"
    );
    assert_eq!(fixture.cursor_count(), 32);

    let cleanup = fixture.arm(Phase::Cleanup);
    let (active_sender, active_receiver) = mpsc::sync_channel(1);
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |result| {
            active_sender
                .try_send(result)
                .expect("faulted accepted completion");
        }),
    )
    .expect("faulted owner accepts a request");
    cleanup.entered();
    let mut queued = Vec::new();
    for _ in 0..6 {
        let (sender, receiver) = mpsc::sync_channel(1);
        host.configure(
            ShortcutConfig::default(),
            Box::new(move |result| {
                sender
                    .try_send(result)
                    .expect("accepted before faulted close");
            }),
        )
        .expect("queued request accepted");
        queued.push(receiver);
    }
    let gate = fixture.gate();
    drop(host);
    assert_eq!(
        Authority::read(&gate),
        Authority {
            generation: 0,
            listening: false,
            closed: true
        }
    );
    cleanup.release();
    assert_eq!(receive_once(&active_receiver), Err(error));
    for receiver in queued {
        let error = receive_once(&receiver)
            .expect_err("closed faulted owner completes queued work with an error");
        assert_eq!(error.kind(), ShortcutErrorKind::Other);
        assert_eq!(
            error.native_code(),
            None,
            "closed completion is not a fabricated native result"
        );
    }
    drop(subscription);
    fixture.retired();
    let operations = fixture.operations();
    let owner = operations[0].0;
    assert!(operations.iter().all(|(thread, _)| *thread == owner));
    assert_eq!(
        operations
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Construct))
            .count(),
        1
    );
    assert_eq!(
        operations
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Install(_)))
            .count(),
        2
    );
    assert_eq!(
        operations.last().map(|(_, operation)| operation),
        Some(&Operation::Drop)
    );
}

#[test]
fn unavailable_launcher_keeps_settings_registered_and_explicit_reconfigure_recovers() {
    let error = ShortcutError::new(
        ShortcutErrorKind::Other,
        None,
        "recorded nonneutral launcher seed",
    );
    let (fixture, host) = Fixture::create(Scenario {
        launcher_error: Some(error.clone()),
        ..Scenario::default()
    });
    let config = ShortcutConfig::default();
    let unavailable =
        configure(host.as_ref(), config.clone()).expect("independent registration snapshot");
    assert!(unavailable.enabled);
    let launcher = unavailable.binding(ShortcutAction::ToggleLauncher);
    assert_eq!(launcher.state, ShortcutBindingState::Unavailable);
    assert_eq!(launcher.error, Some(error));
    let settings = unavailable.binding(ShortcutAction::OpenSettings);
    assert_eq!(settings.state, ShortcutBindingState::Registered);
    assert_eq!(settings.error, None);
    assert_eq!(
        settings.chord,
        config.effective_chord(ShortcutAction::OpenSettings)
    );
    fixture.fence();
    assert_eq!(
        fixture.cursor_count(),
        0,
        "registration failure and idle wake do not observe the cursor"
    );
    assert_eq!(
        fixture
            .operations()
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Install(_)))
            .count(),
        1,
        "an unavailable launcher is not automatically retried"
    );

    fixture.set_launcher_error(None);
    fixture.fence();
    assert_eq!(
        fixture
            .operations()
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Install(_)))
            .count(),
        1,
        "a newly neutral seed requires an explicit configure, not idle replay"
    );
    let recovered =
        configure(host.as_ref(), config).expect("explicit configuration recovers registration");
    assert!(recovered.generation > unavailable.generation);
    assert!(recovered.bindings.iter().all(|binding| binding.state
        == ShortcutBindingState::Registered
        && binding.error.is_none()));
    assert_eq!(fixture.cursor_count(), 0);
    let operations = fixture.operations();
    assert_eq!(
        operations
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Construct))
            .count(),
        1
    );
    assert_eq!(
        operations
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Install(_)))
            .count(),
        2
    );
    assert!(
        operations.iter().any(|(_, operation)| matches!(
            operation,
            Operation::Cleanup {
                launcher: false,
                settings: true,
                ..
            }
        )),
        "explicit retry retires the genuinely registered Settings binding first"
    );
    assert!(
        operations.iter().all(|(_, operation)| matches!(
            operation,
            Operation::Construct
                | Operation::Install(_)
                | Operation::Register(_, _)
                | Operation::Cleanup { .. }
        )),
        "configuration performs no trigger processing or cursor work"
    );
    fixture.close(host);
}

#[test]
fn recoverable_rearm_retires_both_bindings_without_idle_queries_or_automatic_replay() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let (subscription, events) = subscribe(host.as_ref());
    let config = ShortcutConfig::default();
    let initial = configure(host.as_ref(), config.clone()).expect("initial registrations");
    assert!(
        initial
            .bindings
            .iter()
            .all(|binding| binding.state == ShortcutBindingState::Registered)
    );
    fixture.request_rearm();
    fixture.fence();
    let (event, callback_thread) = events
        .recv_timeout(DEADLINE)
        .expect("recoverable owner observation");
    let ShortcutEvent::Unavailable(error) = event else {
        panic!("rearm must report unavailable, not a fabricated trigger");
    };
    assert_eq!(error.kind(), ShortcutErrorKind::Other);
    assert_eq!(error.native_code(), None);
    assert_eq!(
        Authority::read(&fixture.gate()),
        Authority {
            generation: 0,
            listening: true,
            closed: false
        }
    );
    assert_ne!(fixture.operations()[0].0, callback_thread);
    let retired = fixture.operations();
    assert!(
        retired.iter().any(|(_, operation)| matches!(
            operation,
            Operation::Cleanup {
                launcher: true,
                settings: true,
                authority: Authority { generation: 0, .. }
            }
        )),
        "rearm invalidates the owning generation before retiring every binding"
    );
    assert_eq!(
        retired
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Install(_)))
            .count(),
        1
    );
    assert_eq!(
        retired
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Register(_, _)))
            .count(),
        1
    );
    assert_eq!(fixture.cursor_count(), 0);
    fixture.fence();
    fixture.trigger(initial.generation, ShortcutAction::ToggleLauncher);
    fixture.trigger(initial.generation, ShortcutAction::OpenSettings);
    fixture.fence();
    assert_eq!(
        fixture.cursor_count(),
        0,
        "rearm and stale events do not query the cursor"
    );
    assert_eq!(
        fixture
            .operations()
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Install(_)))
            .count(),
        1,
        "the owner stays alive without automatic reinstall or replay"
    );
    assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));

    let recovered =
        configure(host.as_ref(), config).expect("explicit configure reuses the recoverable owner");
    assert!(recovered.generation > initial.generation);
    assert!(recovered.bindings.iter().all(|binding| binding.state
        == ShortcutBindingState::Registered
        && binding.error.is_none()));
    assert_eq!(
        fixture
            .operations()
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Construct))
            .count(),
        1
    );
    assert_eq!(
        fixture
            .operations()
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Install(_)))
            .count(),
        2
    );
    assert_eq!(
        fixture
            .operations()
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Register(_, _)))
            .count(),
        2
    );
    fixture.trigger(initial.generation, ShortcutAction::ToggleLauncher);
    fixture.trigger(recovered.generation, ShortcutAction::OpenSettings);
    fixture.fence();
    let (event, _) = events
        .recv_timeout(DEADLINE)
        .expect("fresh generation works");
    assert_eq!(
        event,
        ShortcutEvent::Triggered(ShortcutTrigger {
            generation: recovered.generation,
            action: ShortcutAction::OpenSettings,
            cursor: Some(TriggerPoint { x: -480, y: 240 }),
        })
    );
    assert_eq!(
        fixture.cursor_count(),
        1,
        "only the new admitted trigger queries the cursor"
    );
    configure(host.as_ref(), ShortcutConfig::default().with_enabled(false)).expect("relay barrier");
    assert!(
        matches!(events.try_recv(), Err(TryRecvError::Empty)),
        "the retired sequence cannot replay"
    );
    drop(subscription);
    fixture.close(host);
}

#[test]
fn rearm_preserves_newly_accepted_generation_and_close_completes_accepted_requests() {
    let (fixture, host) = Fixture::create(Scenario::default());
    let (subscription, events) = subscribe(host.as_ref());
    let initial =
        configure(host.as_ref(), ShortcutConfig::default()).expect("initial registrations");
    let cleanup = fixture.arm(Phase::Cleanup);
    fixture.request_rearm();
    cleanup.entered();
    assert_eq!(fixture.gate().generation.load(Ordering::Acquire), 0);
    let (sender, receiver) = mpsc::sync_channel(1);
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |result| {
            sender
                .try_send(result)
                .expect("replacement accepted completion");
        }),
    )
    .expect("explicit replacement accepted during rearm cleanup");
    let accepted_generation = fixture.gate().generation.load(Ordering::Acquire);
    assert!(accepted_generation > initial.generation);
    cleanup.release();
    let (event, _) = events
        .recv_timeout(DEADLINE)
        .expect("recoverable unavailable observation");
    assert!(
        matches!(event, ShortcutEvent::Unavailable(error) if error.kind() == ShortcutErrorKind::Other)
    );
    let recovered =
        receive_once(&receiver).expect("rearm must not cancel a newer accepted generation");
    assert_eq!(recovered.generation, accepted_generation);
    assert!(
        recovered
            .bindings
            .iter()
            .all(|binding| binding.state == ShortcutBindingState::Registered)
    );
    assert_eq!(
        fixture.gate().generation.load(Ordering::Acquire),
        recovered.generation
    );
    fixture.trigger(initial.generation, ShortcutAction::ToggleLauncher);
    fixture.trigger(recovered.generation, ShortcutAction::OpenSettings);
    fixture.fence();
    let (event, _) = events
        .recv_timeout(DEADLINE)
        .expect("replacement remains live");
    assert!(
        matches!(event, ShortcutEvent::Triggered(trigger) if trigger.generation == recovered.generation)
    );
    assert_eq!(fixture.cursor_count(), 1);

    let cleanup = fixture.arm(Phase::Cleanup);
    fixture.request_rearm();
    cleanup.entered();
    let mut accepted = Vec::new();
    for _ in 0..6 {
        let (sender, receiver) = mpsc::sync_channel(1);
        host.configure(
            ShortcutConfig::default(),
            Box::new(move |result| {
                sender
                    .try_send(result)
                    .expect("accepted before recoverable close");
            }),
        )
        .expect("queued configuration accepted during rearm");
        accepted.push(receiver);
    }
    let gate = fixture.gate();
    drop(host);
    assert_eq!(
        Authority::read(&gate),
        Authority {
            generation: 0,
            listening: false,
            closed: true
        }
    );
    cleanup.release();
    for receiver in accepted {
        let error = receive_once(&receiver)
            .expect_err("closed owner still completes every accepted request");
        assert_eq!(error.kind(), ShortcutErrorKind::Other);
        assert_eq!(error.native_code(), None);
    }
    drop(subscription);
    fixture.retired();
    let operations = fixture.operations();
    let owner = operations[0].0;
    assert!(operations.iter().all(|(thread, _)| *thread == owner));
    assert_eq!(
        operations
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Construct))
            .count(),
        1
    );
    assert_eq!(
        operations
            .iter()
            .filter(|(_, operation)| matches!(operation, Operation::Install(_)))
            .count(),
        2,
        "close does not replay queued configuration into another hook"
    );
    assert_eq!(
        operations.last().map(|(_, operation)| operation),
        Some(&Operation::Drop)
    );
}
