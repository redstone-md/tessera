// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use super::*;

const WAIT: Duration = Duration::from_secs(5);

fn receive<T>(receiver: &Receiver<T>) -> T {
    receiver
        .recv_timeout(WAIT)
        .expect("recording worker stalled")
}

struct RecordingDriver(Arc<AtomicUsize>);

impl Driver for RecordingDriver {
    fn toggle_desktop(&mut self) -> Result<(), DockUtilityError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn hresult_kinds_and_sanitized_native_messages() {
    use DockUtilityErrorKind::*;
    for (code, kind) in [
        (0x8000_4001, Unsupported),
        (0x8000_4002, Unsupported),
        (0x8004_0154, Unsupported),
        (0x8007_0005, AccessDenied),
        (0x8007_00AA, Busy),
        (0x8001_010A, Busy),
        (0x8001_0001, Busy),
        (0x8007_06BA, Unavailable),
        (0x8001_0108, Unavailable),
        (0x8001_0007, Unavailable),
        (0x8001_0106, Other),
        (0x8004_01F0, Other),
        (0x8007_0057, Other),
        (0x8000_4005, Other),
    ] {
        let error = native_error(code, "ToggleDesktop");
        assert_eq!(error.kind, kind);
        assert_eq!(error.message, format!("ToggleDesktop: 0x{code:08X}"));
    }
}

#[test]
fn construction_is_lazy_and_two_explicit_requests_dispatch_twice() {
    let factories = Arc::new(AtomicUsize::new(0));
    let dispatches = Arc::new(AtomicUsize::new(0));
    let factory_count = factories.clone();
    let driver_count = dispatches.clone();
    let host = start(move || {
        factory_count.fetch_add(1, Ordering::SeqCst);
        Ok(RecordingDriver(driver_count.clone()))
    })
    .unwrap();
    assert_eq!(factories.load(Ordering::SeqCst), 0);
    assert_eq!(dispatches.load(Ordering::SeqCst), 0);
    let (done, results) = channel();
    for id in 0..2 {
        let done = done.clone();
        host.toggle_desktop(Box::new(move |result| done.send((id, result)).unwrap()))
            .unwrap();
    }
    drop(done);
    for id in 0..2 {
        let (actual, result) = receive(&results);
        assert_eq!(actual, id);
        result.unwrap();
    }
    assert!(matches!(
        results.recv_timeout(WAIT),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
    assert_eq!(factories.load(Ordering::SeqCst), 2);
    assert_eq!(dispatches.load(Ordering::SeqCst), 2);
}

struct BlockingDriver {
    count: Arc<AtomicUsize>,
    entered: std::sync::mpsc::Sender<()>,
    release: Option<Receiver<()>>,
}

impl Driver for BlockingDriver {
    fn toggle_desktop(&mut self) -> Result<(), DockUtilityError> {
        if self.count.fetch_add(1, Ordering::SeqCst) == 0 {
            self.entered.send(()).unwrap();
            receive(
                self.release
                    .as_ref()
                    .expect("first request owns the barrier"),
            );
        }
        Ok(())
    }
}

#[test]
fn running_plus_sixteen_queued_rejects_next_and_drop_drains_without_join() {
    assert_eq!(QUEUE_CAPACITY, 16);
    let dispatches = Arc::new(AtomicUsize::new(0));
    let driver_count = dispatches.clone();
    let (entered, running) = channel();
    let (release, unblocked) = channel();
    let mut unblocked = Some(unblocked);
    let host = start(move || {
        Ok(BlockingDriver {
            count: driver_count.clone(),
            entered: entered.clone(),
            release: unblocked.take(),
        })
    })
    .unwrap();
    let (done, results) = channel();
    let first_done = done.clone();
    host.toggle_desktop(Box::new(move |result| {
        first_done.send((0, result)).unwrap()
    }))
    .unwrap();
    receive(&running);
    for id in 1..=QUEUE_CAPACITY {
        let done = done.clone();
        host.toggle_desktop(Box::new(move |result| done.send((id, result)).unwrap()))
            .unwrap();
    }
    let rejected_callbacks = Arc::new(AtomicUsize::new(0));
    let rejected_count = rejected_callbacks.clone();
    let error = host
        .toggle_desktop(Box::new(move |_| {
            rejected_count.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap_err();
    assert_eq!(error.kind, DockUtilityErrorKind::Busy);
    // Dropping on another thread with a deadline detects a forbidden GUI join.
    let (dropped, drop_done) = channel();
    std::thread::spawn(move || {
        drop(host);
        dropped.send(()).unwrap();
    });
    receive(&drop_done);
    release.send(()).unwrap();
    drop(done);
    for id in 0..=QUEUE_CAPACITY {
        let (actual, result) = receive(&results);
        assert_eq!(actual, id);
        result.unwrap();
    }
    assert_eq!(dispatches.load(Ordering::SeqCst), QUEUE_CAPACITY + 1);
    assert_eq!(rejected_callbacks.load(Ordering::SeqCst), 0);
    assert!(matches!(
        results.recv_timeout(WAIT),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn disconnected_queue_rejects_without_completion() {
    let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
    drop(receiver);
    let host = WorkerHost { sender };
    let calls = Arc::new(AtomicUsize::new(0));
    let callbacks = calls.clone();
    let error = host
        .toggle_desktop(Box::new(move |_| {
            callbacks.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap_err();
    assert_eq!(error.kind, DockUtilityErrorKind::Stopped);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn callback_reentry_queues_and_consumer_panic_does_not_strand_work() {
    let dispatches = Arc::new(AtomicUsize::new(0));
    let driver_count = dispatches.clone();
    let host = start(move || Ok(RecordingDriver(driver_count.clone()))).unwrap();
    let reentrant_host = host.clone();
    let (done, results) = channel();
    host.toggle_desktop(Box::new(move |result| {
        result.unwrap();
        reentrant_host
            .toggle_desktop(Box::new(move |result| done.send(result).unwrap()))
            .unwrap();
        panic!("consumer panic is contained");
    }))
    .unwrap();
    receive(&results).unwrap();
    assert_eq!(dispatches.load(Ordering::SeqCst), 2);
}

struct PanickingDriver;

impl Driver for PanickingDriver {
    fn toggle_desktop(&mut self) -> Result<(), DockUtilityError> {
        panic!("recorded native driver panic");
    }
}

#[test]
fn factory_and_driver_panics_complete_once_without_automatic_retry() {
    let factories = Arc::new(AtomicUsize::new(0));
    let count = factories.clone();
    let host = start(move || {
        if count.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("recorded factory panic");
        }
        Ok(PanickingDriver)
    })
    .unwrap();
    let (done, results) = channel();
    for _ in 0..2 {
        let done = done.clone();
        host.toggle_desktop(Box::new(move |result| done.send(result).unwrap()))
            .unwrap();
        let error = receive(&results).unwrap_err();
        assert_eq!(error.kind, DockUtilityErrorKind::Other);
        assert_eq!(error.message, "Dock utility worker: 0x80004005");
    }
    drop(done);
    assert_eq!(factories.load(Ordering::SeqCst), 2);
    assert!(matches!(
        results.recv_timeout(WAIT),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn factory_error_has_one_completion_and_only_a_later_request_retries() {
    let factories = Arc::new(AtomicUsize::new(0));
    let dispatches = Arc::new(AtomicUsize::new(0));
    let count = factories.clone();
    let driver_count = dispatches.clone();
    let host = start(move || {
        if count.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(native_error(0x8007_0005, "Dock utility factory"));
        }
        Ok(RecordingDriver(driver_count.clone()))
    })
    .unwrap();
    let (done, results) = channel();
    let first = done.clone();
    host.toggle_desktop(Box::new(move |result| first.send(result).unwrap()))
        .unwrap();
    assert_eq!(
        receive(&results).unwrap_err().kind,
        DockUtilityErrorKind::AccessDenied
    );
    assert_eq!(factories.load(Ordering::SeqCst), 1);
    assert_eq!(dispatches.load(Ordering::SeqCst), 0);
    host.toggle_desktop(Box::new(move |result| done.send(result).unwrap()))
        .unwrap();
    receive(&results).unwrap();
    assert_eq!(factories.load(Ordering::SeqCst), 2);
    assert_eq!(dispatches.load(Ordering::SeqCst), 1);
    assert!(matches!(
        results.recv_timeout(WAIT),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
}
