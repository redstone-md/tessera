// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

fn receive<T>(receiver: &Receiver<T>) -> T {
    receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("worker completion")
}

#[test]
fn disconnected_read_and_action_reject_without_invoking_completion() {
    let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
    drop(receiver);
    let host = WorkerHost { sender };
    let called = Arc::new(AtomicUsize::new(0));
    let read_called = Arc::clone(&called);
    assert_eq!(
        host.read(Box::new(move |_| {
            read_called.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap_err()
        .kind,
        InputLanguageErrorKind::Stopped
    );
    let action_called = Arc::clone(&called);
    assert_eq!(
        host.execute(
            InputLanguageAction::OpenKeyboardSettings,
            Box::new(move |_| {
                action_called.fetch_add(1, Ordering::SeqCst);
            })
        )
        .unwrap_err()
        .kind,
        InputLanguageErrorKind::Stopped
    );
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[test]
fn one_bounded_queue_serializes_reads_and_actions_and_rejects_without_callback() {
    let (entered_tx, entered_rx) = channel();
    let (release_tx, release_rx) = channel();
    let (calls_tx, calls_rx) = channel();
    let read_calls = calls_tx.clone();
    let host = start(
        move || {
            read_calls.send("read").unwrap();
            entered_tx.send(()).unwrap();
            receive(&release_rx);
            Ok(InputLanguageSnapshot::default())
        },
        move |_| {
            calls_tx.send("action").unwrap();
            Ok(InputLanguageOutcome::SettingsDispatched)
        },
    )
    .unwrap();
    let (done_tx, done_rx) = channel();
    let first_done = done_tx.clone();
    host.read(Box::new(move |result| {
        first_done.send(result.is_ok()).unwrap();
    }))
    .unwrap();
    receive(&entered_rx);
    assert_eq!(receive(&calls_rx), "read");
    for _ in 0..QUEUE_CAPACITY {
        let done = done_tx.clone();
        host.execute(
            InputLanguageAction::OpenKeyboardSettings,
            Box::new(move |result| {
                done.send(result.is_ok()).unwrap();
            }),
        )
        .unwrap();
    }
    let rejected_called = Arc::new(AtomicBool::new(false));
    let rejected_read = rejected_called.clone();
    assert_eq!(
        host.read(Box::new(move |_| {
            rejected_read.store(true, Ordering::SeqCst);
        }))
        .unwrap_err()
        .kind,
        InputLanguageErrorKind::Busy
    );
    let rejected_action = rejected_called.clone();
    assert_eq!(
        host.execute(
            InputLanguageAction::OpenKeyboardSettings,
            Box::new(move |_| {
                rejected_action.store(true, Ordering::SeqCst);
            })
        )
        .unwrap_err()
        .kind,
        InputLanguageErrorKind::Busy
    );
    assert!(!rejected_called.load(Ordering::SeqCst));
    release_tx.send(()).unwrap();
    for _ in 0..=QUEUE_CAPACITY {
        assert!(receive(&done_rx));
    }
    for _ in 0..QUEUE_CAPACITY {
        assert_eq!(receive(&calls_rx), "action");
    }
    assert!(!rejected_called.load(Ordering::SeqCst));
    assert!(done_rx.try_recv().is_err());
}

#[test]
fn dropping_last_host_does_not_join_and_drains_accepted_requests() {
    let (entered_tx, entered_rx) = channel();
    let (release_tx, release_rx) = channel();
    let host = start(
        move || {
            entered_tx.send(()).unwrap();
            receive(&release_rx);
            Ok(InputLanguageSnapshot::default())
        },
        |_| Ok(InputLanguageOutcome::SettingsDispatched),
    )
    .unwrap();
    let (completion_tx, completion_rx) = channel();
    let first_tx = completion_tx.clone();
    host.read(Box::new(move |result| {
        first_tx.send(result.is_ok()).unwrap();
    }))
    .unwrap();
    receive(&entered_rx);
    host.execute(
        InputLanguageAction::OpenKeyboardSettings,
        Box::new(move |result| {
            completion_tx.send(result.is_ok()).unwrap();
        }),
    )
    .unwrap();
    let (dropped_tx, dropped_rx) = channel();
    std::thread::spawn(move || {
        drop(host);
        dropped_tx.send(()).unwrap();
    });
    receive(&dropped_rx);
    assert!(completion_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    assert!(receive(&completion_rx));
    assert!(receive(&completion_rx));
    assert!(completion_rx.try_recv().is_err());
}

#[test]
fn operation_panics_complete_once_and_do_not_stop_later_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let read_calls = calls.clone();
    let host = start(
        move || {
            if read_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("read provider panic");
            }
            Ok(InputLanguageSnapshot::default())
        },
        |_| -> Result<InputLanguageOutcome, InputLanguageError> {
            panic!("after possible action effect");
        },
    )
    .unwrap();
    let (read_tx, read_rx) = channel();
    host.read(Box::new(move |result| {
        read_tx.send(result).unwrap();
    }))
    .unwrap();
    assert_eq!(
        receive(&read_rx).unwrap_err().kind,
        InputLanguageErrorKind::Other
    );
    let (action_tx, action_rx) = channel();
    host.execute(
        InputLanguageAction::OpenKeyboardSettings,
        Box::new(move |result| {
            action_tx.send(result).unwrap();
        }),
    )
    .unwrap();
    let error = receive(&action_rx).unwrap_err();
    assert!(error.message.contains("effects may have occurred"));
    let (again_tx, again_rx) = channel();
    host.read(Box::new(move |result| {
        again_tx.send(result).unwrap();
    }))
    .unwrap();
    assert!(receive(&again_rx).is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(read_rx.try_recv().is_err());
    assert!(action_rx.try_recv().is_err());
}

#[test]
fn panicking_and_reentrant_consumers_cannot_strand_accepted_work() {
    let host = start(
        || Ok(InputLanguageSnapshot::default()),
        |_| Ok(InputLanguageOutcome::SettingsDispatched),
    )
    .unwrap();
    host.read(Box::new(|_| panic!("consumer panic"))).unwrap();
    let reentrant_host = host.clone();
    let (done_tx, done_rx) = channel();
    host.read(Box::new(move |result| {
        assert!(result.is_ok());
        reentrant_host
            .execute(
                InputLanguageAction::OpenKeyboardSettings,
                Box::new(move |result| {
                    done_tx.send(result).unwrap();
                }),
            )
            .unwrap();
    }))
    .unwrap();
    assert_eq!(
        receive(&done_rx).unwrap(),
        InputLanguageOutcome::SettingsDispatched
    );
}

#[test]
fn actions_execute_exactly_once_and_follow_preceding_read() {
    let (event_tx, event_rx) = channel();
    let read_events = event_tx.clone();
    let requested =
        tessera_system::input_language::ProfileId::new("opaque-native-profile").unwrap();
    let expected = requested.clone();
    let host = start(
        move || {
            read_events.send("read").unwrap();
            Ok(InputLanguageSnapshot::default())
        },
        move |action| {
            assert_eq!(
                action,
                InputLanguageAction::Activate {
                    profile: expected.clone()
                }
            );
            event_tx.send("activate").unwrap();
            Ok(InputLanguageOutcome::Snapshot(
                InputLanguageSnapshot::default(),
            ))
        },
    )
    .unwrap();
    let (done_tx, done_rx) = channel();
    let read_done = done_tx.clone();
    host.read(Box::new(move |result| {
        read_done.send(result.is_ok()).unwrap();
    }))
    .unwrap();
    host.execute(
        InputLanguageAction::Activate { profile: requested },
        Box::new(move |result| {
            done_tx.send(result.is_ok()).unwrap();
        }),
    )
    .unwrap();
    assert_eq!(receive(&event_rx), "read");
    assert_eq!(receive(&event_rx), "activate");
    assert!(receive(&done_rx));
    assert!(receive(&done_rx));
    assert!(event_rx.try_recv().is_err());
}
