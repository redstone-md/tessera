// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use parking_lot::Mutex;
use std::rc::Rc;
use std::sync::atomic::AtomicUsize;
use tessera_system::dock_utilities::DockUtilityErrorKind;

#[derive(Clone, Default)]
struct Deferred(Arc<Mutex<Option<Job>>>);

impl Deferred {
    fn run(&self) {
        let job = self.0.lock().take().unwrap();
        job(); // Never hold the test scheduler lock across callback reentry.
    }
}

impl Spawner for Deferred {
    fn spawn(&self, job: Job) -> std::io::Result<()> {
        let mut pending = self.0.lock();
        assert!(pending.is_none(), "there is no mutation queue");
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
    DriverPanic,
    DropPanic,
}

struct RecordingDriver {
    events: Arc<Mutex<Vec<&'static str>>>,
    failure: Failure,
    status: i32,
    thread: std::thread::ThreadId,
    _not_send: Rc<()>,
}

impl Driver for RecordingDriver {
    fn empty(&mut self) -> Result<RecycleBinEmptyOutcome, DockUtilityError> {
        assert_eq!(std::thread::current().id(), self.thread);
        self.events.lock().push("empty");
        if matches!(self.failure, Failure::DriverPanic) {
            panic!("recorded driver panic");
        }
        Ok(RecycleBinEmptyOutcome {
            native_hresult: self.status,
        })
    }
}

impl Drop for RecordingDriver {
    fn drop(&mut self) {
        assert_eq!(std::thread::current().id(), self.thread);
        self.events.lock().push("drop");
        if matches!(self.failure, Failure::DropPanic) {
            panic!("recorded drop panic");
        }
    }
}

fn factory(
    events: Arc<Mutex<Vec<&'static str>>>,
    failure: Failure,
    status: i32,
) -> impl Fn() -> Result<RecordingDriver, DockUtilityError> + Send + Sync {
    move || {
        events.lock().push("create");
        match failure {
            Failure::FactoryError => return Err(native_error(0x8007_0005, "recorded setup")),
            Failure::FactoryPanic => panic!("recorded factory panic"),
            _ => {}
        }
        Ok(RecordingDriver {
            events: Arc::clone(&events),
            failure,
            status,
            thread: std::thread::current().id(),
            _not_send: Rc::new(()),
        })
    }
}

#[test]
fn lazy_factory_and_clone_shared_busy_have_no_queue_or_rejected_callback() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let scheduler = Deferred::default();
    let host = host_with_spawner(factory(events.clone(), Failure::None, 1), scheduler.clone());
    let cloned_host = Arc::clone(&host);
    let accepted = Arc::new(AtomicUsize::new(0));
    let rejected = Arc::new(AtomicUsize::new(0));
    assert!(events.lock().is_empty());
    host.empty(Box::new({
        let accepted = accepted.clone();
        let events = events.clone();
        move |result| {
            assert_eq!(result.unwrap().native_hresult, 1);
            assert_eq!(*events.lock(), ["create", "empty", "drop"]);
            accepted.fetch_add(1, Ordering::SeqCst);
        }
    }))
    .unwrap();
    assert!(events.lock().is_empty());
    let error = cloned_host
        .empty(Box::new({
            let rejected = rejected.clone();
            move |_| {
                rejected.fetch_add(1, Ordering::SeqCst);
            }
        }))
        .unwrap_err();
    assert_eq!(error.kind, DockUtilityErrorKind::Busy);
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
    assert_eq!(rejected.load(Ordering::SeqCst), 0);
    scheduler.run();
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    assert_eq!(rejected.load(Ordering::SeqCst), 0);
    cloned_host
        .empty(Box::new(|result| assert!(result.is_ok())))
        .unwrap();
    scheduler.run();
}

#[test]
fn accepted_operation_and_completion_survive_all_host_drops() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let scheduler = Deferred::default();
    let host = host_with_spawner(factory(events.clone(), Failure::None, 0), scheduler.clone());
    let (sender, receiver) = std::sync::mpsc::channel();
    host.empty(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    drop(host); // No join, cancellation, or dependency on host/UI liveness.
    assert!(events.lock().is_empty());
    scheduler.run();
    assert_eq!(receiver.recv().unwrap().unwrap().native_hresult, 0);
    assert_eq!(*events.lock(), ["create", "empty", "drop"]);
}

#[test]
fn inline_completion_can_reenter_only_after_driver_drop_and_gate_release() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let host = host_with_spawner(factory(events.clone(), Failure::None, 0), Inline);
    let completed = Arc::new(AtomicUsize::new(0));
    host.empty(Box::new({
        let host = Arc::clone(&host);
        let events = events.clone();
        let completed = completed.clone();
        move |result| {
            assert!(result.is_ok());
            assert_eq!(*events.lock(), ["create", "empty", "drop"]);
            completed.fetch_add(1, Ordering::SeqCst);
            host.empty(Box::new(move |result| {
                assert!(result.is_ok());
                completed.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
        }
    }))
    .unwrap();
    assert_eq!(completed.load(Ordering::SeqCst), 2);
    assert_eq!(
        *events.lock(),
        ["create", "empty", "drop", "create", "empty", "drop"]
    );
}

#[test]
fn failed_and_panicking_spawn_transfer_no_callback_and_release_flight() {
    struct FailOnce(AtomicUsize, bool);
    impl Spawner for FailOnce {
        fn spawn(&self, job: Job) -> std::io::Result<()> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                if self.1 {
                    panic!("recorded spawn panic");
                }
                return Err(std::io::Error::other("recorded spawn failure"));
            }
            job();
            Ok(())
        }
    }
    for panic in [false, true] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let host = host_with_spawner(
            factory(events.clone(), Failure::None, 0),
            FailOnce(AtomicUsize::new(0), panic),
        );
        let callbacks = Arc::new(AtomicUsize::new(0));
        let error = host
            .empty(Box::new({
                let callbacks = callbacks.clone();
                move |_| {
                    callbacks.fetch_add(1, Ordering::SeqCst);
                }
            }))
            .unwrap_err();
        assert_eq!(error.kind, DockUtilityErrorKind::Other);
        assert_eq!(callbacks.load(Ordering::SeqCst), 0);
        assert!(events.lock().is_empty());
        host.empty(Box::new({
            let callbacks = callbacks.clone();
            move |result| {
                assert!(result.is_ok());
                callbacks.fetch_add(1, Ordering::SeqCst);
            }
        }))
        .unwrap();
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
        assert_eq!(*events.lock(), ["create", "empty", "drop"]);
    }
}

#[test]
fn factory_driver_and_drop_failures_complete_once_after_cleanup() {
    for failure in [
        Failure::FactoryError,
        Failure::FactoryPanic,
        Failure::DriverPanic,
        Failure::DropPanic,
    ] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let host = host_with_spawner(factory(events.clone(), failure, 0), Inline);
        let callbacks = Arc::new(AtomicUsize::new(0));
        host.empty(Box::new({
            let callbacks = callbacks.clone();
            let events = events.clone();
            move |result| {
                let error = result.unwrap_err();
                assert_eq!(
                    error.kind,
                    if matches!(failure, Failure::FactoryError) {
                        DockUtilityErrorKind::AccessDenied
                    } else {
                        DockUtilityErrorKind::Other
                    }
                );
                let expected: &[&str] =
                    if matches!(failure, Failure::FactoryError | Failure::FactoryPanic) {
                        &["create"]
                    } else {
                        &["create", "empty", "drop"]
                    };
                assert_eq!(&*events.lock(), expected);
                callbacks.fetch_add(1, Ordering::SeqCst);
            }
        }))
        .unwrap();
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
        // Error completion releases admission too, not only successful returns.
        host.empty(Box::new(|result| assert!(result.is_err())))
            .unwrap();
    }
}

#[test]
fn consumer_panic_is_contained_without_stranding_next_admission() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let host = host_with_spawner(factory(events.clone(), Failure::None, 0), Inline);
    host.empty(Box::new(|_| panic!("recorded consumer panic")))
        .unwrap();
    host.empty(Box::new(|result| assert!(result.is_ok())))
        .unwrap();
    assert_eq!(
        *events.lock(),
        ["create", "empty", "drop", "create", "empty", "drop"]
    );
}

#[test]
fn non_send_driver_is_created_used_and_dropped_on_request_thread_before_callback() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let caller_thread = std::thread::current().id();
    let worker_host = host(factory(events.clone(), Failure::None, -1));
    let (sender, receiver) = std::sync::mpsc::channel();
    worker_host
        .empty(Box::new(move |result| {
            assert_ne!(std::thread::current().id(), caller_thread);
            sender.send(result).unwrap();
        }))
        .unwrap();
    drop(worker_host);
    let result = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert_eq!(result.unwrap().native_hresult, -1);
    assert_eq!(*events.lock(), ["create", "empty", "drop"]);
}

#[test]
fn flight_remains_busy_through_driver_destructor_reentry() {
    struct ReenterOnDrop(Arc<Mutex<Option<std::sync::Weak<dyn RecycleBinMutationHost>>>>);
    impl Driver for ReenterOnDrop {
        fn empty(&mut self) -> Result<RecycleBinEmptyOutcome, DockUtilityError> {
            Ok(RecycleBinEmptyOutcome { native_hresult: 0 })
        }
    }
    impl Drop for ReenterOnDrop {
        fn drop(&mut self) {
            let host = self.0.lock().as_ref().unwrap().upgrade().unwrap();
            assert_eq!(
                host.empty(Box::new(|_| panic!("busy must not complete")))
                    .unwrap_err()
                    .kind,
                DockUtilityErrorKind::Busy
            );
        }
    }
    let weak = Arc::new(Mutex::new(None));
    let host = host_with_spawner(
        {
            let weak = weak.clone();
            move || Ok(ReenterOnDrop(weak.clone()))
        },
        Inline,
    );
    *weak.lock() = Some(Arc::downgrade(&host));
    host.empty(Box::new(|result| assert!(result.is_ok())))
        .unwrap();
}

#[test]
fn entered_recording_call_rejects_clone_busy_and_drains_after_host_drop_without_join() {
    struct BlockingDriver {
        entered: std::sync::mpsc::Sender<()>,
        resume: std::sync::mpsc::Receiver<()>,
        dropped: std::sync::mpsc::Sender<()>,
        _not_send: Rc<()>,
    }
    impl Driver for BlockingDriver {
        fn empty(&mut self) -> Result<RecycleBinEmptyOutcome, DockUtilityError> {
            self.entered.send(()).unwrap();
            self.resume.recv().unwrap();
            Ok(RecycleBinEmptyOutcome { native_hresult: 0 })
        }
    }
    impl Drop for BlockingDriver {
        fn drop(&mut self) {
            self.dropped.send(()).unwrap();
        }
    }
    let (entered, entered_rx) = std::sync::mpsc::channel();
    let (resume, resume_rx) = std::sync::mpsc::channel();
    let (dropped, dropped_rx) = std::sync::mpsc::channel();
    let resume_rx = Mutex::new(Some(resume_rx));
    let worker_host = host(move || {
        Ok(BlockingDriver {
            entered: entered.clone(),
            resume: resume_rx.lock().take().unwrap(),
            dropped: dropped.clone(),
            _not_send: Rc::new(()),
        })
    });
    let clone = Arc::clone(&worker_host);
    let (completed, completed_rx) = std::sync::mpsc::channel();
    worker_host
        .empty(Box::new(move |result| {
            assert!(dropped_rx.try_recv().is_ok());
            completed.send(result).unwrap();
        }))
        .unwrap();
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let rejected = Arc::new(AtomicUsize::new(0));
    let error = clone
        .empty(Box::new({
            let rejected = rejected.clone();
            move |_| {
                rejected.fetch_add(1, Ordering::SeqCst);
            }
        }))
        .unwrap_err();
    assert_eq!(error.kind, DockUtilityErrorKind::Busy);
    assert_eq!(rejected.load(Ordering::SeqCst), 0);
    drop(worker_host);
    drop(clone); // Must return while the recording native call is still blocked.
    assert!(completed_rx.try_recv().is_err());
    resume.send(()).unwrap();
    assert_eq!(
        completed_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .native_hresult,
        0
    );
    assert_eq!(rejected.load(Ordering::SeqCst), 0);
}
