// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pure recording drivers only: these tests never call any native Power API.

use super::*;
use parking_lot::Mutex;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Default)]
struct Deferred(Arc<Mutex<Option<Job>>>);

impl Deferred {
    fn run(&self) {
        let job = self.0.lock().take().unwrap();
        job(); // The scheduler lock is retired before consumer reentry.
    }
}

impl Spawner for Deferred {
    fn spawn(&self, job: Job) -> std::io::Result<()> {
        let mut pending = self.0.lock();
        assert!(pending.is_none(), "Power has no accepted request queue");
        *pending = Some(job);
        Ok(())
    }
}

struct Inline;

impl Spawner for Inline {
    fn spawn(&self, job: Job) -> std::io::Result<()> {
        job();
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Failure {
    None,
    FactoryError,
    FactoryPanic,
    DriverError,
    DriverPanic,
    DropPanic,
}

struct RecordingDriver {
    events: Arc<Mutex<Vec<&'static str>>>,
    failure: Failure,
    owner: std::thread::ThreadId,
    _not_send: Rc<()>,
}

impl Driver for RecordingDriver {
    fn perform(&mut self, action: PowerAction) -> Result<PowerRequestAccepted, PowerError> {
        assert_eq!(std::thread::current().id(), self.owner);
        assert_eq!(action, PowerAction::LockSession);
        self.events.lock().push("perform");
        match self.failure {
            Failure::DriverError => Err(PowerError::Native { code: 0 }),
            Failure::DriverPanic => panic!("recorded Power driver panic"),
            _ => Ok(PowerRequestAccepted),
        }
    }
}

impl Drop for RecordingDriver {
    fn drop(&mut self) {
        assert_eq!(std::thread::current().id(), self.owner);
        self.events.lock().push("drop");
        if matches!(self.failure, Failure::DropPanic) {
            panic!("recorded Power drop panic");
        }
    }
}

fn factory(
    events: Arc<Mutex<Vec<&'static str>>>,
    failure: Failure,
) -> impl Fn() -> Result<RecordingDriver, PowerError> + Send + Sync {
    move || {
        events.lock().push("create");
        match failure {
            Failure::FactoryError => return Err(PowerError::AccessDenied),
            Failure::FactoryPanic => panic!("recorded Power creation panic"),
            _ => {}
        }
        Ok(RecordingDriver {
            events: Arc::clone(&events),
            failure,
            owner: std::thread::current().id(),
            _not_send: Rc::new(()),
        })
    }
}

#[test]
fn power_factory_is_lazy_and_clone_busy_rejects_without_callback_or_queue() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let scheduler = Deferred::default();
    let host = host_with_spawner(factory(events.clone(), Failure::None), scheduler.clone());
    let clone = Arc::clone(&host);
    let accepted = Arc::new(AtomicUsize::new(0));
    let rejected = Arc::new(AtomicUsize::new(0));
    assert!(events.lock().is_empty());
    host.perform(
        PowerAction::LockSession,
        Box::new({
            let accepted = accepted.clone();
            let events = events.clone();
            move |result| {
                assert_eq!(result, Ok(PowerRequestAccepted));
                assert_eq!(*events.lock(), ["create", "perform", "drop"]);
                accepted.fetch_add(1, Ordering::SeqCst);
            }
        }),
    )
    .unwrap();
    assert!(events.lock().is_empty());
    assert_eq!(
        clone.perform(
            PowerAction::LockSession,
            Box::new({
                let rejected = rejected.clone();
                move |_| {
                    rejected.fetch_add(1, Ordering::SeqCst);
                }
            })
        ),
        Err(PowerError::Busy),
    );
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
    assert_eq!(rejected.load(Ordering::SeqCst), 0);
    scheduler.run();
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    assert_eq!(rejected.load(Ordering::SeqCst), 0);
    clone
        .perform(
            PowerAction::LockSession,
            Box::new(|result| assert!(result.is_ok())),
        )
        .unwrap();
    scheduler.run();
    assert_eq!(
        *events.lock(),
        ["create", "perform", "drop", "create", "perform", "drop"]
    );
}

#[test]
fn accepted_power_survives_all_host_drops_and_completes_exactly_once() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let scheduler = Deferred::default();
    let host = host_with_spawner(factory(events.clone(), Failure::None), scheduler.clone());
    let (sender, receiver) = std::sync::mpsc::channel();
    host.perform(
        PowerAction::LockSession,
        Box::new(move |result| sender.send(result).unwrap()),
    )
    .unwrap();
    drop(host); // No join or cancellation; the accepted request owns its flight.
    assert!(events.lock().is_empty());
    scheduler.run();
    assert_eq!(receiver.recv().unwrap(), Ok(PowerRequestAccepted));
    assert_eq!(
        receiver.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    );
    assert_eq!(*events.lock(), ["create", "perform", "drop"]);
}

#[test]
fn inline_power_completion_reenters_after_driver_drop_and_flight_release() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let host = host_with_spawner(factory(events.clone(), Failure::None), Inline);
    let callbacks = Arc::new(AtomicUsize::new(0));
    host.perform(
        PowerAction::LockSession,
        Box::new({
            let host = Arc::clone(&host);
            let events = events.clone();
            let callbacks = callbacks.clone();
            move |result| {
                assert_eq!(result, Ok(PowerRequestAccepted));
                assert_eq!(*events.lock(), ["create", "perform", "drop"]);
                callbacks.fetch_add(1, Ordering::SeqCst);
                host.perform(
                    PowerAction::LockSession,
                    Box::new(move |result| {
                        assert_eq!(result, Ok(PowerRequestAccepted));
                        callbacks.fetch_add(1, Ordering::SeqCst);
                    }),
                )
                .unwrap();
            }
        }),
    )
    .unwrap();
    assert_eq!(callbacks.load(Ordering::SeqCst), 2);
    assert_eq!(
        *events.lock(),
        ["create", "perform", "drop", "create", "perform", "drop"]
    );
}

#[test]
fn failed_or_panicking_power_spawn_accepts_zero_callbacks_and_releases_flight() {
    struct FailOnce(AtomicUsize, bool);
    impl Spawner for FailOnce {
        fn spawn(&self, job: Job) -> std::io::Result<()> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                if self.1 {
                    panic!("recorded Power spawn panic");
                }
                return Err(std::io::Error::other("recorded Power spawn failure"));
            }
            job();
            Ok(())
        }
    }
    for panic in [false, true] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let host = host_with_spawner(
            factory(events.clone(), Failure::None),
            FailOnce(AtomicUsize::new(0), panic),
        );
        let callbacks = Arc::new(AtomicUsize::new(0));
        assert_eq!(
            host.perform(
                PowerAction::LockSession,
                Box::new({
                    let callbacks = callbacks.clone();
                    move |_| {
                        callbacks.fetch_add(1, Ordering::SeqCst);
                    }
                })
            ),
            Err(PowerError::Unavailable),
        );
        assert_eq!(callbacks.load(Ordering::SeqCst), 0);
        assert!(events.lock().is_empty());
        host.perform(
            PowerAction::LockSession,
            Box::new({
                let callbacks = callbacks.clone();
                move |result| {
                    assert_eq!(result, Ok(PowerRequestAccepted));
                    callbacks.fetch_add(1, Ordering::SeqCst);
                }
            }),
        )
        .unwrap();
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn power_creation_driver_and_drop_failures_complete_once_after_cleanup_attempt() {
    for failure in [
        Failure::FactoryError,
        Failure::FactoryPanic,
        Failure::DriverError,
        Failure::DriverPanic,
        Failure::DropPanic,
    ] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let host = host_with_spawner(factory(events.clone(), failure), Inline);
        let callbacks = Arc::new(AtomicUsize::new(0));
        let expected_error = match failure {
            Failure::FactoryError => PowerError::AccessDenied,
            Failure::DriverError => PowerError::Native { code: 0 },
            _ => PowerError::Unavailable,
        };
        host.perform(
            PowerAction::LockSession,
            Box::new({
                let callbacks = callbacks.clone();
                let events = events.clone();
                move |result| {
                    assert_eq!(result, Err(expected_error));
                    let expected: &[&str] = match failure {
                        Failure::FactoryError | Failure::FactoryPanic => &["create"],
                        _ => &["create", "perform", "drop"],
                    };
                    assert_eq!(&*events.lock(), expected);
                    callbacks.fetch_add(1, Ordering::SeqCst);
                }
            }),
        )
        .unwrap();
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
        host.perform(
            PowerAction::LockSession,
            Box::new(move |result| {
                assert_eq!(result, Err(expected_error));
            }),
        )
        .unwrap(); // Error paths release admission, not only native success.
    }
}

#[test]
fn power_consumer_panic_is_contained_once_and_does_not_strand_admission() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let host = host_with_spawner(factory(events.clone(), Failure::None), Inline);
    let callbacks = Arc::new(AtomicUsize::new(0));
    host.perform(
        PowerAction::LockSession,
        Box::new({
            let callbacks = callbacks.clone();
            move |_| {
                callbacks.fetch_add(1, Ordering::SeqCst);
                panic!("recorded Power consumer panic");
            }
        }),
    )
    .unwrap();
    host.perform(
        PowerAction::LockSession,
        Box::new({
            let callbacks = callbacks.clone();
            move |result| {
                assert_eq!(result, Ok(PowerRequestAccepted));
                callbacks.fetch_add(1, Ordering::SeqCst);
            }
        }),
    )
    .unwrap();
    assert_eq!(callbacks.load(Ordering::SeqCst), 2);
    assert_eq!(
        *events.lock(),
        ["create", "perform", "drop", "create", "perform", "drop"]
    );
}

#[test]
fn non_send_power_driver_is_created_used_and_dropped_on_request_owner() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let caller = std::thread::current().id();
    let host = host(factory(events.clone(), Failure::None));
    let (sender, receiver) = std::sync::mpsc::channel();
    host.perform(
        PowerAction::LockSession,
        Box::new({
            let events = events.clone();
            move |result| {
                assert_ne!(std::thread::current().id(), caller);
                assert_eq!(*events.lock(), ["create", "perform", "drop"]);
                sender.send(result).unwrap();
            }
        }),
    )
    .unwrap();
    drop(host);
    assert_eq!(
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap(),
        Ok(PowerRequestAccepted),
    );
    assert_eq!(
        receiver.recv_timeout(std::time::Duration::from_secs(5)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected),
    );
}

#[test]
fn power_gate_remains_busy_through_creation_native_use_and_driver_retirement() {
    type HostSlot = Arc<Mutex<Option<Arc<dyn PowerHost>>>>;
    fn reject_overlap(slot: &HostSlot, callbacks: &Arc<AtomicUsize>) {
        let host = slot.lock().as_ref().unwrap().clone();
        assert_eq!(
            host.perform(
                PowerAction::LockSession,
                Box::new({
                    let callbacks = callbacks.clone();
                    move |_| {
                        callbacks.fetch_add(1, Ordering::SeqCst);
                    }
                })
            ),
            Err(PowerError::Busy),
        );
    }
    struct GuardedDriver(HostSlot, Arc<AtomicUsize>);
    impl Driver for GuardedDriver {
        fn perform(&mut self, _: PowerAction) -> Result<PowerRequestAccepted, PowerError> {
            reject_overlap(&self.0, &self.1);
            Ok(PowerRequestAccepted)
        }
    }
    impl Drop for GuardedDriver {
        fn drop(&mut self) {
            reject_overlap(&self.0, &self.1);
        }
    }
    let slot: HostSlot = Arc::new(Mutex::new(None));
    let rejected_callbacks = Arc::new(AtomicUsize::new(0));
    let accepted_callbacks = Arc::new(AtomicUsize::new(0));
    let host = host_with_spawner(
        {
            let slot = slot.clone();
            let callbacks = rejected_callbacks.clone();
            move || {
                reject_overlap(&slot, &callbacks);
                Ok(GuardedDriver(slot.clone(), callbacks.clone()))
            }
        },
        Inline,
    );
    *slot.lock() = Some(host.clone());
    host.perform(
        PowerAction::LockSession,
        Box::new({
            let callbacks = accepted_callbacks.clone();
            move |result| {
                assert_eq!(result, Ok(PowerRequestAccepted));
                callbacks.fetch_add(1, Ordering::SeqCst);
            }
        }),
    )
    .unwrap();
    assert_eq!(accepted_callbacks.load(Ordering::SeqCst), 1);
    assert_eq!(rejected_callbacks.load(Ordering::SeqCst), 0);
    let stored = slot.lock().take();
    drop(stored); // Retire the test-only cycle outside its mutex.
}
