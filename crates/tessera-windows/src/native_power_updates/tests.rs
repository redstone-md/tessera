// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Raw recordings exercise the production Owner and worker. No OS API calls.

use super::*;
use crate::power_updates::worker::{self, Job, Spawner};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, ThreadId};
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Event {
    Create,
    Open {
        path: String,
        options: u32,
        access: u32,
    },
    Close,
    KeyDrop,
    CallsDrop,
}

type Events = Arc<Mutex<Vec<(ThreadId, Event)>>>;

fn record(events: &Events, event: Event) {
    events.lock().push((thread::current().id(), event));
}

fn operations(events: &Events) -> Vec<Event> {
    events
        .lock()
        .iter()
        .map(|(_, event)| event.clone())
        .collect()
}

fn open_event(index: usize) -> Event {
    Event::Open {
        path: PATHS[index].into(),
        options: OPEN_OPTIONS,
        access: READ_ACCESS,
    }
}

#[derive(Clone, Copy, Default)]
enum Failure {
    #[default]
    None,
    FactoryError,
    FactoryPanic,
    OpenPanic,
    ClosePanic,
    KeyDropPanic,
    CallsDropPanic,
}

struct Block {
    reached: Sender<()>,
    release: Mutex<Receiver<()>>,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum BlockAt {
    #[default]
    Open,
    Close,
    CallsDrop,
}

#[derive(Clone)]
struct Scenario {
    opens: Vec<Result<(), u32>>,
    close: Result<(), u32>,
    failure: Failure,
    block: Option<Arc<Block>>,
    block_at: BlockAt,
}

impl Scenario {
    fn new(opens: impl Into<Vec<Result<(), u32>>>) -> Self {
        Self {
            opens: opens.into(),
            close: Ok(()),
            failure: Failure::None,
            block: None,
            block_at: BlockAt::Open,
        }
    }

    fn wait_if_blocked(&mut self, phase: BlockAt) {
        if self.block_at == phase
            && let Some(block) = self.block.take()
        {
            block.reached.send(()).unwrap();
            block
                .release
                .lock()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
    }
}

struct RecordingKey {
    events: Events,
    owner: ThreadId,
    panic_on_drop: bool,
    _not_send: Rc<()>,
}

impl Drop for RecordingKey {
    fn drop(&mut self) {
        assert_eq!(thread::current().id(), self.owner);
        record(&self.events, Event::KeyDrop);
        assert!(!self.panic_on_drop, "recorded key drop panic");
    }
}

struct RecordingCalls {
    events: Events,
    owner: ThreadId,
    opens: VecDeque<Result<(), u32>>,
    scenario: Scenario,
    _not_send: Rc<()>,
}

impl Calls for RecordingCalls {
    type Key = RecordingKey;

    fn open_hklm(&mut self, path: &str, options: u32, access: u32) -> Result<Self::Key, u32> {
        assert_eq!(thread::current().id(), self.owner);
        record(
            &self.events,
            Event::Open {
                path: path.into(),
                options,
                access,
            },
        );
        self.scenario.wait_if_blocked(BlockAt::Open);
        assert!(
            !matches!(self.scenario.failure, Failure::OpenPanic),
            "recorded open panic"
        );
        self.opens.pop_front().expect("unexpected registry call")?;
        Ok(RecordingKey {
            events: self.events.clone(),
            owner: self.owner,
            panic_on_drop: matches!(self.scenario.failure, Failure::KeyDropPanic),
            _not_send: Rc::new(()),
        })
    }

    fn close(&mut self, key: Self::Key) -> Result<(), u32> {
        assert_eq!(thread::current().id(), self.owner);
        assert_eq!(key.owner, self.owner);
        record(&self.events, Event::Close);
        self.scenario.wait_if_blocked(BlockAt::Close);
        assert!(
            !matches!(self.scenario.failure, Failure::ClosePanic),
            "recorded close panic"
        );
        self.scenario.close
    }
}

impl Drop for RecordingCalls {
    fn drop(&mut self) {
        assert_eq!(thread::current().id(), self.owner);
        record(&self.events, Event::CallsDrop);
        self.scenario.wait_if_blocked(BlockAt::CallsDrop);
        assert!(
            !matches!(self.scenario.failure, Failure::CallsDropPanic),
            "recorded calls drop panic"
        );
    }
}

fn factory(
    events: Events,
    scenario: Scenario,
) -> impl Fn() -> Result<Owner<RecordingCalls>, PowerUpdatesError> + Send + Sync {
    move || {
        record(&events, Event::Create);
        match scenario.failure {
            Failure::FactoryError => return Err(PowerUpdatesError::AccessDenied),
            Failure::FactoryPanic => panic!("recorded read-owner factory panic"),
            _ => {}
        }
        Ok(Owner::new(RecordingCalls {
            events: events.clone(),
            owner: thread::current().id(),
            opens: scenario.opens.iter().copied().collect(),
            scenario: scenario.clone(),
            _not_send: Rc::new(()),
        }))
    }
}

struct Inline;

impl Spawner for Inline {
    fn spawn(&self, job: Job) -> std::io::Result<()> {
        job();
        Ok(())
    }
}

#[derive(Clone, Default)]
struct Deferred(Arc<Mutex<Option<Job>>>);

impl Deferred {
    fn run(&self) {
        let job = self.0.lock().take().unwrap();
        job();
    }
}

impl Spawner for Deferred {
    fn spawn(&self, job: Job) -> std::io::Result<()> {
        let mut pending = self.0.lock();
        assert!(pending.is_none(), "no accepted read queue");
        *pending = Some(job);
        Ok(())
    }
}

fn run(scenario: Scenario) -> (Result<PowerUpdateHint, PowerUpdatesError>, Vec<Event>) {
    let events = Events::default();
    let host = worker::host_with_spawner(factory(events.clone(), scenario), Inline);
    let (sender, receiver) = mpsc::channel();
    host.read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    let result = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    (result, operations(&events))
}

#[test]
fn ordered_hklm_paths_use_key_read_zero_options_and_ordinary_process_view() {
    assert_eq!(
        PATHS,
        [
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending",
        ]
    );
    assert_eq!(READ_ACCESS, 0x0002_0019);
    assert_eq!(READ_ACCESS & (0x0100 | 0x0200), 0); // No 64/32-bit view override.
    assert_eq!(OPEN_OPTIONS, 0);
    let (result, events) = run(Scenario::new([Err(2), Err(3)]));
    assert_eq!(result, Ok(PowerUpdateHint::NotDetected));
    assert_eq!(
        events,
        [
            Event::Create,
            open_event(0),
            open_event(1),
            Event::CallsDrop
        ]
    );
}

#[test]
fn first_success_shortcircuits_and_closes_exactly_once_before_owner_drop() {
    let (result, events) = run(Scenario::new([Ok(())]));
    assert_eq!(result, Ok(PowerUpdateHint::Pending));
    assert_eq!(
        events,
        [
            Event::Create,
            open_event(0),
            Event::Close,
            Event::KeyDrop,
            Event::CallsDrop
        ]
    );
}

#[test]
fn second_known_pending_wins_after_first_missing_or_unexpected_open_error() {
    for first in [2, 3, 5, 87, u32::MAX] {
        let (result, events) = run(Scenario::new([Err(first), Ok(())]));
        assert_eq!(result, Ok(PowerUpdateHint::Pending));
        assert_eq!(
            events,
            [
                Event::Create,
                open_event(0),
                open_event(1),
                Event::Close,
                Event::KeyDrop,
                Event::CallsDrop
            ]
        );
    }
}

#[test]
fn both_absence_codes_are_not_detected_but_access_denied_and_other_statuses_are_errors() {
    for first in [2, 3] {
        for second in [2, 3] {
            assert_eq!(
                run(Scenario::new([Err(first), Err(second)])).0,
                Ok(PowerUpdateHint::NotDetected)
            );
        }
    }
    for (first, second, expected) in [
        (5, 2, PowerUpdatesError::AccessDenied),
        (3, 5, PowerUpdatesError::AccessDenied),
        (87, 3, PowerUpdatesError::Native { code: 87 }),
        (2, u32::MAX, PowerUpdatesError::Native { code: u32::MAX }),
        (87, 5, PowerUpdatesError::Native { code: 87 }),
        (5, 87, PowerUpdatesError::AccessDenied),
    ] {
        let (result, events) = run(Scenario::new([Err(first), Err(second)]));
        assert_eq!(result, Err(expected));
        assert_eq!(
            events,
            [
                Event::Create,
                open_event(0),
                open_event(1),
                Event::CallsDrop
            ]
        );
    }
}

#[test]
fn authoritative_returned_codes_need_no_last_error_call_in_the_raw_seam() {
    // Calls deliberately has no last_error capability. Every code, including
    // unusual high bits, is supplied directly by the recording open/close.
    for code in [0, 6, 1300, 0x8000_0000, u32::MAX] {
        assert_eq!(
            run(Scenario::new([Err(code), Err(2)])).0,
            Err(PowerUpdatesError::Native { code }),
        );
    }
}

#[test]
fn cleanup_failure_is_checked_once_never_pending_and_retains_the_first_fault() {
    for code in [5, 6, u32::MAX] {
        let mut scenario = Scenario::new([Ok(())]);
        scenario.close = Err(code);
        let (result, events) = run(scenario);
        assert_eq!(result, Err(native_error(code)));
        assert_eq!(
            events,
            [
                Event::Create,
                open_event(0),
                Event::Close,
                Event::KeyDrop,
                Event::CallsDrop
            ]
        );
    }
    let mut scenario = Scenario::new([Err(87), Ok(())]);
    scenario.close = Err(5);
    let (result, events) = run(scenario);
    assert_eq!(result, Err(PowerUpdatesError::Native { code: 87 }));
    assert_eq!(
        events
            .iter()
            .filter(|event| **event == Event::Close)
            .count(),
        1
    );
}

#[test]
fn same_owner_retirement_takes_the_resource_before_close_and_drop_never_retries() {
    let events = Events::default();
    let mut scenario = Scenario::new([]);
    scenario.close = Err(6);
    let mut owner = factory(events.clone(), scenario)().unwrap();
    owner.key = Some(RecordingKey {
        events: events.clone(),
        owner: thread::current().id(),
        panic_on_drop: false,
        _not_send: Rc::new(()),
    });
    assert_eq!(owner.retire(), Err(PowerUpdatesError::Native { code: 6 }));
    assert_eq!(owner.retire(), Ok(()));
    drop(owner);
    assert_eq!(
        operations(&events),
        [
            Event::Create,
            Event::Close,
            Event::KeyDrop,
            Event::CallsDrop
        ]
    );
}

#[test]
fn factory_open_close_and_drop_panics_complete_once_after_same_owner_cleanup_attempt() {
    for failure in [
        Failure::FactoryError,
        Failure::FactoryPanic,
        Failure::OpenPanic,
        Failure::ClosePanic,
        Failure::KeyDropPanic,
        Failure::CallsDropPanic,
    ] {
        let mut scenario = Scenario::new([Ok(())]);
        scenario.failure = failure;
        let (result, events) = run(scenario);
        assert_eq!(
            result,
            Err(if matches!(failure, Failure::FactoryError) {
                PowerUpdatesError::AccessDenied
            } else {
                PowerUpdatesError::Unavailable
            })
        );
        let expected = match failure {
            Failure::FactoryError | Failure::FactoryPanic => vec![Event::Create],
            Failure::OpenPanic => vec![Event::Create, open_event(0), Event::CallsDrop],
            _ => vec![
                Event::Create,
                open_event(0),
                Event::Close,
                Event::KeyDrop,
                Event::CallsDrop,
            ],
        };
        assert_eq!(events, expected);
    }
}

#[test]
fn lazy_factory_busy_clone_and_accepted_host_drop_have_one_callback_and_no_queue() {
    let events = Events::default();
    let scheduler = Deferred::default();
    let host = worker::host_with_spawner(
        factory(events.clone(), Scenario::new([Ok(())])),
        scheduler.clone(),
    );
    let clone = host.clone();
    let callbacks = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();
    assert!(operations(&events).is_empty());
    host.read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    assert!(operations(&events).is_empty());
    assert_eq!(
        clone.read(Box::new({
            let callbacks = callbacks.clone();
            move |_| {
                callbacks.fetch_add(1, Ordering::SeqCst);
            }
        })),
        Err(PowerUpdatesError::Busy)
    );
    drop(host);
    drop(clone);
    scheduler.run();
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
        Ok(PowerUpdateHint::Pending)
    );
    assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    assert_eq!(
        operations(&events),
        [
            Event::Create,
            open_event(0),
            Event::Close,
            Event::KeyDrop,
            Event::CallsDrop
        ]
    );
}

#[test]
fn callback_reentry_observes_resource_and_owner_drop_before_gate_release() {
    let events = Events::default();
    let host = worker::host_with_spawner(factory(events.clone(), Scenario::new([Ok(())])), Inline);
    let (sender, receiver) = mpsc::channel();
    host.read(Box::new({
        let host = host.clone();
        let events = events.clone();
        move |result| {
            let retired = operations(&events);
            let next = host.read(Box::new({
                let sender = sender.clone();
                move |result| sender.send((result, Vec::new(), None)).unwrap()
            }));
            sender.send((result, retired, Some(next))).unwrap();
        }
    }))
    .unwrap();
    let nested = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(nested.0, Ok(PowerUpdateHint::Pending));
    let first = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(first.0, Ok(PowerUpdateHint::Pending));
    assert_eq!(
        first.1,
        [
            Event::Create,
            open_event(0),
            Event::Close,
            Event::KeyDrop,
            Event::CallsDrop
        ]
    );
    assert_eq!(first.2, Some(Ok(())));
    assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected));
}

#[test]
fn failed_or_panicking_read_thread_spawn_has_zero_callbacks_then_releases_gate() {
    struct FailOnce(AtomicUsize, bool);
    impl Spawner for FailOnce {
        fn spawn(&self, job: Job) -> std::io::Result<()> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                assert!(!self.1, "recorded read spawn panic");
                return Err(std::io::Error::other("recorded read spawn failure"));
            }
            job();
            Ok(())
        }
    }
    for panic in [false, true] {
        let events = Events::default();
        let host = worker::host_with_spawner(
            factory(events.clone(), Scenario::new([Ok(())])),
            FailOnce(AtomicUsize::new(0), panic),
        );
        let callbacks = Arc::new(AtomicUsize::new(0));
        assert_eq!(
            host.read(Box::new({
                let callbacks = callbacks.clone();
                move |_| {
                    callbacks.fetch_add(1, Ordering::SeqCst);
                }
            })),
            Err(PowerUpdatesError::Unavailable)
        );
        assert!(operations(&events).is_empty());
        assert_eq!(callbacks.load(Ordering::SeqCst), 0);
        let (sender, receiver) = mpsc::channel();
        host.read(Box::new(move |result| sender.send(result).unwrap()))
            .unwrap();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            Ok(PowerUpdateHint::Pending)
        );
    }
}

#[test]
fn accepted_real_read_owner_keeps_non_send_resources_on_worker_through_close_and_drop() {
    let events = Events::default();
    let (reached, waiting) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let mut scenario = Scenario::new([Ok(())]);
    scenario.block = Some(Arc::new(Block {
        reached,
        release: Mutex::new(blocked),
    }));
    let host = worker::host(factory(events.clone(), scenario));
    let caller = thread::current().id();
    let (sender, receiver) = mpsc::channel();
    host.read(Box::new({
        let events = events.clone();
        move |result| {
            sender
                .send((result, thread::current().id(), events.lock().clone()))
                .unwrap()
        }
    }))
    .unwrap();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        host.read(Box::new(|_| panic!("busy read accepts no callback"))),
        Err(PowerUpdatesError::Busy)
    );
    assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
    drop(host); // Never joins/cancels the blocked accepted read.
    release.send(()).unwrap();
    let (result, consumer, trace) = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(result, Ok(PowerUpdateHint::Pending));
    assert_ne!(caller, consumer);
    assert!(trace.iter().all(|(owner, _)| *owner == consumer));
    assert_eq!(
        trace
            .into_iter()
            .map(|(_, event)| event)
            .collect::<Vec<_>>(),
        [
            Event::Create,
            open_event(0),
            Event::Close,
            Event::KeyDrop,
            Event::CallsDrop
        ]
    );
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(5)),
        Err(mpsc::RecvTimeoutError::Disconnected),
    );
}

#[test]
fn independent_read_hosts_do_not_share_their_admission_gates() {
    let scheduler_a = Deferred::default();
    let scheduler_b = Deferred::default();
    let a = worker::host_with_spawner(
        factory(Events::default(), Scenario::new([Ok(())])),
        scheduler_a.clone(),
    );
    let b = worker::host_with_spawner(
        factory(Events::default(), Scenario::new([Err(2), Err(3)])),
        scheduler_b.clone(),
    );
    let (sender, receiver) = mpsc::channel();
    a.read(Box::new({
        let sender = sender.clone();
        move |result| sender.send(result).unwrap()
    }))
    .unwrap();
    b.read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    scheduler_a.run();
    scheduler_b.run();
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
        Ok(PowerUpdateHint::Pending)
    );
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
        Ok(PowerUpdateHint::NotDetected)
    );
}

#[test]
fn consumer_panic_is_contained_and_does_not_hold_read_admission() {
    let host =
        worker::host_with_spawner(factory(Events::default(), Scenario::new([Ok(())])), Inline);
    assert_eq!(
        host.read(Box::new(|_| panic!("recorded consumer panic"))),
        Ok(())
    );
    let (sender, receiver) = mpsc::channel();
    host.read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
        Ok(PowerUpdateHint::Pending)
    );
}

#[test]
fn busy_persists_through_checked_close_and_read_owner_drop_before_completion() {
    for phase in [BlockAt::Close, BlockAt::CallsDrop] {
        let events = Events::default();
        let (reached, waiting) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let mut scenario = Scenario::new([Ok(())]);
        scenario.block_at = phase;
        scenario.block = Some(Arc::new(Block {
            reached,
            release: Mutex::new(blocked),
        }));
        let host = worker::host(factory(events.clone(), scenario));
        let (sender, receiver) = mpsc::channel();
        host.read(Box::new(move |result| sender.send(result).unwrap()))
            .unwrap();
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            host.read(Box::new(|_| panic!("cleanup still owns admission"))),
            Err(PowerUpdatesError::Busy),
        );
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
        release.send(()).unwrap();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            Ok(PowerUpdateHint::Pending),
        );
        assert_eq!(
            operations(&events),
            [
                Event::Create,
                open_event(0),
                Event::Close,
                Event::KeyDrop,
                Event::CallsDrop
            ],
        );
    }
}

#[test]
fn accepted_read_owner_factory_failure_delivers_one_completion_not_admission_error() {
    for failure in [Failure::FactoryError, Failure::FactoryPanic] {
        let events = Events::default();
        let mut scenario = Scenario::new([]);
        scenario.failure = failure;
        let host = worker::host(factory(events.clone(), scenario));
        let (sender, receiver) = mpsc::channel();
        assert_eq!(
            host.read(Box::new(move |result| sender.send(result).unwrap())),
            Ok(()),
        );
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(if matches!(failure, Failure::FactoryError) {
                PowerUpdatesError::AccessDenied
            } else {
                PowerUpdatesError::Unavailable
            }),
        );
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected),
        );
        assert_eq!(operations(&events), [Event::Create]);
    }
}
