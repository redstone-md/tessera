// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pure recording calls only, including on Windows. Never invoke OS Lock.

use super::*;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::ThreadId;

const CLEANUP_ERROR: u32 = 0xdead_beef;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Event {
    Create(ThreadId),
    Lock(i32, ThreadId),
    LastError(u32, ThreadId),
    Drop(ThreadId),
}

struct Record {
    events: Mutex<Vec<Event>>,
    status: i32,
    error: AtomicU32,
}

impl Record {
    fn new(status: i32, error: u32) -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
            status,
            error: AtomicU32::new(error),
        })
    }
}

struct RecordingCalls {
    record: Arc<Record>,
    owner: ThreadId,
}

impl RecordingCalls {
    fn new(record: Arc<Record>) -> Self {
        let owner = std::thread::current().id();
        record.events.lock().push(Event::Create(owner));
        Self { record, owner }
    }
}

impl Calls for RecordingCalls {
    fn lock_work_station(&self) -> i32 {
        assert_eq!(std::thread::current().id(), self.owner);
        self.record
            .events
            .lock()
            .push(Event::Lock(self.record.status, self.owner));
        self.record.status
    }

    fn last_error(&self) -> u32 {
        assert_eq!(std::thread::current().id(), self.owner);
        assert_eq!(
            self.record.status, 0,
            "nonzero BOOL must not read stale last error"
        );
        let code = self.record.error.load(Ordering::SeqCst);
        self.record
            .events
            .lock()
            .push(Event::LastError(code, self.owner));
        code
    }
}

impl Drop for RecordingCalls {
    fn drop(&mut self) {
        assert_eq!(std::thread::current().id(), self.owner);
        // Model cleanup overwriting thread-local error. The receipt must already
        // contain the exact failure code before cleanup or consumer execution.
        self.record.error.store(CLEANUP_ERROR, Ordering::SeqCst);
        self.record.events.lock().push(Event::Drop(self.owner));
    }
}

#[test]
fn recording_power_factory_and_clones_make_zero_native_calls_before_perform() {
    let record = Record::new(1, 5);
    let host = crate::power::worker::host({
        let record = record.clone();
        move || Ok(PowerDriver::new(RecordingCalls::new(record.clone())))
    });
    let clone = host.clone();
    assert!(record.events.lock().is_empty());
    drop(host);
    drop(clone);
    assert!(record.events.lock().is_empty());
    assert_eq!(record.error.load(Ordering::SeqCst), 5);
}

#[test]
fn every_nonzero_bool_is_one_native_initiation_without_last_error() {
    let owner = std::thread::current().id();
    for status in [i32::MIN, -42, -1, 1, 2, 42, i32::MAX] {
        let record = Record::new(status, ACCESS_DENIED);
        let mut driver = PowerDriver::new(RecordingCalls::new(record.clone()));
        assert_eq!(*record.events.lock(), [Event::Create(owner)]);
        let result = driver.perform(PowerAction::LockSession);
        drop(driver);
        assert_eq!(result, Ok(PowerRequestAccepted));
        assert_eq!(
            *record.events.lock(),
            [
                Event::Create(owner),
                Event::Lock(status, owner),
                Event::Drop(owner),
            ]
        );
    }
}

#[test]
fn zero_bool_captures_raw_last_error_before_cleanup_including_unspecified_zero() {
    let owner = std::thread::current().id();
    for code in [0, 1, ACCESS_DENIED, 6, 0x8007_0005, u32::MAX] {
        let record = Record::new(0, code);
        let mut driver = PowerDriver::new(RecordingCalls::new(record.clone()));
        let result = driver.perform(PowerAction::LockSession);
        drop(driver);
        assert_eq!(record.error.load(Ordering::SeqCst), CLEANUP_ERROR);
        assert_eq!(
            result,
            Err(if code == ACCESS_DENIED {
                PowerError::AccessDenied
            } else {
                PowerError::Native { code }
            })
        );
        assert_eq!(
            *record.events.lock(),
            [
                Event::Create(owner),
                Event::Lock(0, owner),
                Event::LastError(code, owner),
                Event::Drop(owner),
            ]
        );
    }
}

#[test]
fn recording_lock_and_cleanup_are_worker_owned_before_exactly_one_completion() {
    let caller = std::thread::current().id();
    for (status, code) in [(-1, 5), (1, 6), (0, 0), (0, 5), (0, u32::MAX)] {
        let record = Record::new(status, code);
        let host = crate::power::worker::host({
            let record = record.clone();
            move || Ok(PowerDriver::new(RecordingCalls::new(record.clone())))
        });
        assert!(record.events.lock().is_empty());
        let (sender, receiver) = std::sync::mpsc::channel();
        host.perform(
            PowerAction::LockSession,
            Box::new({
                let record = record.clone();
                move |result| {
                    sender
                        .send((
                            result,
                            std::thread::current().id(),
                            record.events.lock().clone(),
                        ))
                        .unwrap();
                }
            }),
        )
        .unwrap();
        drop(host);
        let (result, owner, events) = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_ne!(owner, caller);
        assert_eq!(
            receiver.recv_timeout(std::time::Duration::from_secs(5)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected),
        );
        assert_eq!(record.error.load(Ordering::SeqCst), CLEANUP_ERROR);
        if status != 0 {
            assert_eq!(result, Ok(PowerRequestAccepted));
            assert_eq!(
                events,
                [
                    Event::Create(owner),
                    Event::Lock(status, owner),
                    Event::Drop(owner)
                ]
            );
        } else {
            assert_eq!(
                result,
                Err(if code == ACCESS_DENIED {
                    PowerError::AccessDenied
                } else {
                    PowerError::Native { code }
                })
            );
            assert_eq!(
                events,
                [
                    Event::Create(owner),
                    Event::Lock(0, owner),
                    Event::LastError(code, owner),
                    Event::Drop(owner),
                ]
            );
        }
    }
}
