// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use parking_lot::Mutex;

use super::*;

const DEADLINE: Duration = Duration::from_secs(5);

fn unsupported_watch(
    _: RecycleBinWatchCallback,
    _: RecycleBinWatchCompletion,
) -> Result<Box<dyn RecycleBinWatchGuard>, DockUtilityError> {
    Err(native_error(0x8000_4001, "Recording watch"))
}

struct RecordingDriver {
    log: Arc<Mutex<Vec<&'static str>>>,
    gate: Option<Receiver<()>>,
    panic: bool,
    _thread_bound: Rc<()>,
}

impl Drop for RecordingDriver {
    fn drop(&mut self) {
        self.log.lock().push("drop");
    }
}

impl Driver for RecordingDriver {
    fn read(&mut self) -> Result<RecycleBinInfo, DockUtilityError> {
        self.log.lock().push("read");
        if let Some(gate) = self.gate.take() {
            gate.recv_timeout(DEADLINE).unwrap();
        }
        assert!(!self.panic, "recording read panic");
        Ok(RecycleBinInfo {
            item_count: 7,
            size_in_bytes: 0,
        })
    }

    fn open(&mut self) -> Result<(), DockUtilityError> {
        self.log.lock().push("open");
        assert!(!self.panic, "recording open panic");
        Ok(())
    }
}

#[test]
fn accepted_read_open_drop_before_completion_and_reentry() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let factory_log = log.clone();
    let host = start(
        move || {
            Ok(RecordingDriver {
                log: factory_log.clone(),
                gate: None,
                panic: false,
                _thread_bound: Rc::new(()),
            })
        },
        unsupported_watch,
    )
    .unwrap();
    let nested_host = host.clone();
    let callback_log = log.clone();
    let (done, result) = channel();
    host.read(Box::new(move |info| {
        assert_eq!(info.unwrap().item_count, 7);
        assert_eq!(callback_log.lock().last(), Some(&"drop"));
        callback_log.lock().push("read-completion");
        nested_host
            .open(Box::new(move |opened| {
                opened.unwrap();
                assert_eq!(callback_log.lock().last(), Some(&"drop"));
                callback_log.lock().push("open-completion");
                done.send(()).unwrap();
            }))
            .unwrap();
    }))
    .unwrap();
    drop(host);
    result.recv_timeout(DEADLINE).unwrap();
    assert!(matches!(
        result.recv_timeout(DEADLINE),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
    assert_eq!(
        *log.lock(),
        [
            "read",
            "drop",
            "read-completion",
            "open",
            "drop",
            "open-completion"
        ]
    );
}

#[test]
fn factory_driver_and_consumer_panics_do_not_strand_accepted_requests() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let factory_log = log.clone();
    let mut request = 0;
    let host = start(
        move || {
            request += 1;
            assert_ne!(request, 1, "recording factory panic");
            Ok(RecordingDriver {
                log: factory_log.clone(),
                gate: None,
                panic: request == 2,
                _thread_bound: Rc::new(()),
            })
        },
        unsupported_watch,
    )
    .unwrap();
    let (done, result) = channel();
    for id in 0..3 {
        let done = done.clone();
        host.open(Box::new(move |completion| {
            done.send((id, completion.map_err(|error| error.kind)))
                .unwrap();
            if id == 1 {
                panic!("recording completion panic");
            }
        }))
        .unwrap();
    }
    drop(done);
    drop(host);
    assert_eq!(
        result.recv_timeout(DEADLINE).unwrap(),
        (0, Err(DockUtilityErrorKind::Other))
    );
    assert_eq!(
        result.recv_timeout(DEADLINE).unwrap(),
        (1, Err(DockUtilityErrorKind::Other))
    );
    assert_eq!(result.recv_timeout(DEADLINE).unwrap(), (2, Ok(())));
    assert!(matches!(
        result.recv_timeout(DEADLINE),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
    assert_eq!(*log.lock(), ["open", "drop", "open", "drop"]);
}

struct RecordingGuard(Arc<AtomicUsize>);
impl RecycleBinWatchGuard for RecordingGuard {}
impl Drop for RecordingGuard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn sixteen_pending_requests_drain_fifo_and_watch_retirement_ignores_full_queue() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let factory_log = log.clone();
    let (release, gate) = channel();
    let mut gate = Some(gate);
    let (entered, entering) = channel();
    let retirements = Arc::new(AtomicUsize::new(0));
    let watch_retirements = retirements.clone();
    let host = start(
        move || {
            let gate = gate.take();
            if gate.is_some() {
                entered.send(()).unwrap();
            }
            Ok(RecordingDriver {
                log: factory_log.clone(),
                gate,
                panic: false,
                _thread_bound: Rc::new(()),
            })
        },
        move |_, ready| {
            ready(Ok(()));
            Ok(Box::new(RecordingGuard(watch_retirements.clone()))
                as Box<dyn RecycleBinWatchGuard>)
        },
    )
    .unwrap();
    let (done, result) = channel();
    let first_done = done.clone();
    host.read(Box::new(move |info| {
        info.unwrap();
        first_done.send(0).unwrap();
    }))
    .unwrap();
    entering.recv_timeout(DEADLINE).unwrap();
    for id in 1..=QUEUE_CAPACITY {
        let done = done.clone();
        host.open(Box::new(move |completion| {
            completion.unwrap();
            done.send(id).unwrap();
        }))
        .unwrap();
    }
    let rejected = Arc::new(AtomicUsize::new(0));
    let rejected_completion = rejected.clone();
    let error = host
        .read(Box::new(move |_| {
            rejected_completion.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap_err();
    assert_eq!(error.kind, DockUtilityErrorKind::Busy);
    let (ready_done, readiness) = channel();
    let guard = host
        .watch(
            Arc::new(|_| panic!("unexpected fixture event")),
            Box::new(move |ready| {
                ready_done.send(ready).unwrap();
            }),
        )
        .unwrap();
    readiness.recv_timeout(DEADLINE).unwrap().unwrap();
    drop(guard);
    assert_eq!(retirements.load(Ordering::SeqCst), 1);
    drop(host); // no join: in-flight recording is still blocked until release.
    release.send(()).unwrap();
    drop(done);
    for id in 0..=QUEUE_CAPACITY {
        assert_eq!(result.recv_timeout(DEADLINE).unwrap(), id);
    }
    assert!(matches!(
        result.recv_timeout(DEADLINE),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
    assert_eq!(rejected.load(Ordering::SeqCst), 0);
    assert_eq!(
        log.lock().iter().filter(|event| **event == "drop").count(),
        QUEUE_CAPACITY + 1
    );
}

#[test]
fn disconnected_queue_rejects_without_completion() {
    let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
    drop(receiver);
    let host = WorkerHost {
        sender,
        watch: unsupported_watch,
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let callback_calls = calls.clone();
    assert_eq!(
        host.open(Box::new(move |_| {
            callback_calls.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap_err()
        .kind,
        DockUtilityErrorKind::Stopped
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn sanitized_native_error_uses_existing_safe_kinds() {
    for (code, kind) in [
        (0x8000_4001, DockUtilityErrorKind::Unsupported),
        (0x8007_0005, DockUtilityErrorKind::AccessDenied),
        (0x8007_00AA, DockUtilityErrorKind::Busy),
        (0x8001_0108, DockUtilityErrorKind::Unavailable),
        (0xDEAD_BEEF, DockUtilityErrorKind::Other),
    ] {
        let error = native_error(code, "Recycle Bin recording");
        assert_eq!(error.kind, kind);
        assert_eq!(
            error.message,
            format!("Recycle Bin recording: 0x{code:08X}")
        );
    }
}

#[test]
fn immediate_watch_rejection_transfers_no_ready_or_events() {
    let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
    let host = WorkerHost {
        sender,
        watch: unsupported_watch,
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let ready_calls = calls.clone();
    let event_calls = calls.clone();
    let result = host.watch(
        Arc::new(move |_| {
            event_calls.fetch_add(1, Ordering::SeqCst);
        }),
        Box::new(move |_| {
            ready_calls.fetch_add(1, Ordering::SeqCst);
        }),
    );
    assert_eq!(
        result.err().unwrap().kind,
        DockUtilityErrorKind::Unsupported
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    drop(receiver);
}
