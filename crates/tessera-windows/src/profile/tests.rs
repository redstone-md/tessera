// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::actor::{self, Driver};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use tessera_system::profile::{
    ProfileCommand, ProfileError, ProfileErrorKind, ProfilePhotoState, ProfileSnapshot,
};

#[path = "source_tests.rs"]
mod source_tests;

const WAIT: Duration = Duration::from_secs(5);

struct RecordingDriver {
    entered: Option<mpsc::Sender<()>>,
    gate: Option<mpsc::Receiver<()>>,
    dropped: mpsc::Sender<(std::thread::ThreadId, std::thread::ThreadId)>,
    owner: std::thread::ThreadId,
    reads: Arc<AtomicUsize>,
    panic_read: bool,
}

impl Driver for RecordingDriver {
    fn read(&mut self) -> Result<ProfileSnapshot, ProfileError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(entered) = self.entered.take() {
            entered.send(()).unwrap();
        }
        if let Some(gate) = self.gate.take() {
            gate.recv_timeout(WAIT).unwrap();
        }
        assert!(!self.panic_read, "controlled recording driver panic");
        ProfileSnapshot::new(
            "Recording Name".into(),
            ProfilePhotoState::Absent,
            None,
            None,
        )
    }
    fn execute(&mut self, _: ProfileCommand) -> Result<(), ProfileError> {
        Ok(())
    }
}

impl Drop for RecordingDriver {
    fn drop(&mut self) {
        let _ = self.dropped.send((self.owner, std::thread::current().id()));
    }
}

#[test]
fn construction_is_lazy_queue_is_16_rejection_completes_none_and_shutdown_drains_accepted() {
    let creates = Arc::new(AtomicUsize::new(0));
    let reads = Arc::new(AtomicUsize::new(0));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let gate = Mutex::new(Some(release_rx));
    let (drop_tx, drop_rx) = mpsc::channel();
    let owner_creates = Arc::clone(&creates);
    let owner_reads = Arc::clone(&reads);
    let host = actor::host(move || {
        owner_creates.fetch_add(1, Ordering::SeqCst);
        Ok(RecordingDriver {
            entered: Some(entered_tx.clone()),
            gate: gate.lock().take(),
            dropped: drop_tx.clone(),
            owner: std::thread::current().id(),
            reads: Arc::clone(&owner_reads),
            panic_read: false,
        })
    });
    assert_eq!(creates.load(Ordering::SeqCst), 0);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    let (completion_tx, completion_rx) = mpsc::channel();
    let first_tx = completion_tx.clone();
    host.read(Box::new(move |result| {
        first_tx.send(result.is_ok()).unwrap();
    }))
    .unwrap();
    entered_rx.recv_timeout(WAIT).unwrap();
    for _ in 0..16 {
        let tx = completion_tx.clone();
        host.read(Box::new(move |result| {
            tx.send(result.is_ok()).unwrap();
        }))
        .unwrap();
    }
    let rejected_calls = Arc::new(AtomicUsize::new(0));
    let rejected = Arc::clone(&rejected_calls);
    let result = host.read(Box::new(move |_| {
        rejected.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(result.unwrap_err().kind(), ProfileErrorKind::Busy);
    drop(host); // no join, and queued accepted completions are not abandoned.
    release_tx.send(()).unwrap();
    for _ in 0..17 {
        assert!(completion_rx.recv_timeout(WAIT).unwrap());
    }
    let (created_on, dropped_on) = drop_rx.recv_timeout(WAIT).unwrap();
    assert_eq!(created_on, dropped_on);
    assert_ne!(created_on, std::thread::current().id());
    assert_eq!(creates.load(Ordering::SeqCst), 1);
    assert_eq!(reads.load(Ordering::SeqCst), 17);
    assert_eq!(rejected_calls.load(Ordering::SeqCst), 0);
    assert!(completion_rx.try_recv().is_err());
}

#[test]
fn accepted_completion_can_reenter_without_locks_or_second_owner() {
    let (drop_tx, drop_rx) = mpsc::channel();
    let creates = Arc::new(AtomicUsize::new(0));
    let created = Arc::clone(&creates);
    let host = actor::host(move || {
        created.fetch_add(1, Ordering::SeqCst);
        Ok(RecordingDriver {
            entered: None,
            gate: None,
            dropped: drop_tx.clone(),
            owner: std::thread::current().id(),
            reads: Arc::new(AtomicUsize::new(0)),
            panic_read: false,
        })
    });
    let weak = Arc::downgrade(&host);
    let (done_tx, done_rx) = mpsc::channel();
    host.read(Box::new(move |result| {
        assert!(result.is_ok());
        weak.upgrade()
            .unwrap()
            .execute(
                ProfileCommand::OpenHome,
                Box::new(move |result| {
                    done_tx.send(result).unwrap();
                }),
            )
            .unwrap();
    }))
    .unwrap();
    done_rx.recv_timeout(WAIT).unwrap().unwrap();
    drop(host);
    let (created_on, dropped_on) = drop_rx.recv_timeout(WAIT).unwrap();
    assert_eq!(created_on, dropped_on);
    assert_eq!(creates.load(Ordering::SeqCst), 1);
}

#[test]
fn completion_panic_does_not_kill_owner_or_drop_next_completion() {
    let (drop_tx, drop_rx) = mpsc::channel();
    let host = actor::host(move || {
        Ok(RecordingDriver {
            entered: None,
            gate: None,
            dropped: drop_tx.clone(),
            owner: std::thread::current().id(),
            reads: Arc::new(AtomicUsize::new(0)),
            panic_read: false,
        })
    });
    host.read(Box::new(|_| panic!("controlled completion panic")))
        .unwrap();
    let (tx, rx) = mpsc::channel();
    host.read(Box::new(move |result| {
        tx.send(result).unwrap();
    }))
    .unwrap();
    rx.recv_timeout(WAIT).unwrap().unwrap();
    drop(host);
    drop_rx.recv_timeout(WAIT).unwrap();
}

#[test]
fn backend_panic_retires_driver_on_owner_and_all_accepted_requests_fail_honestly() {
    let (drop_tx, drop_rx) = mpsc::channel();
    let host = actor::host(move || {
        Ok(RecordingDriver {
            entered: None,
            gate: None,
            dropped: drop_tx.clone(),
            owner: std::thread::current().id(),
            reads: Arc::new(AtomicUsize::new(0)),
            panic_read: true,
        })
    });
    let (tx, rx) = mpsc::channel();
    for _ in 0..2 {
        let tx = tx.clone();
        host.read(Box::new(move |result| {
            tx.send(result).unwrap();
        }))
        .unwrap();
    }
    for _ in 0..2 {
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap().unwrap_err().kind(),
            ProfileErrorKind::Other
        );
    }
    let (created_on, dropped_on) = drop_rx.recv_timeout(WAIT).unwrap();
    assert_eq!(created_on, dropped_on);
    drop(host);
}

#[test]
fn initialization_failure_is_one_completion_per_accepted_request_not_fake_profile_or_respawn() {
    let creates = Arc::new(AtomicUsize::new(0));
    let created = Arc::clone(&creates);
    let host = actor::host::<RecordingDriver, _>(move || {
        created.fetch_add(1, Ordering::SeqCst);
        Err(ProfileError::new(
            ProfileErrorKind::AccessDenied,
            "The profile provider is unavailable.",
            Some(5),
        ))
    });
    let (tx, rx) = mpsc::channel();
    for _ in 0..2 {
        let tx = tx.clone();
        host.read(Box::new(move |result| {
            tx.send(result).unwrap();
        }))
        .unwrap();
    }
    for _ in 0..2 {
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap().unwrap_err().kind(),
            ProfileErrorKind::AccessDenied
        );
    }
    assert_eq!(creates.load(Ordering::SeqCst), 1);
    drop(host);
}

#[test]
fn native_factory_only_constructs_lazy_host_without_profile_io() {
    #[cfg(windows)]
    assert!(super::native_profile_host().is_ok());
    #[cfg(not(windows))]
    assert_eq!(
        super::native_profile_host().err().unwrap().kind(),
        ProfileErrorKind::Unsupported
    );
}
