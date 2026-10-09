// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
#[cfg(not(windows))]
use crate::network::native_network_host;
use parking_lot::Mutex as RecordingMutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(3);

type Wake = Arc<dyn Fn() + Send + Sync>;

struct Recording {
    calls: Sender<&'static str>,
    wake: Arc<RecordingMutex<Option<Wake>>>,
    read_release: Option<Receiver<()>>,
    register_release: Option<Receiver<()>>,
    register_error: bool,
    unregister_error: bool,
    panic_read: bool,
}

impl NetworkOwner for Recording {
    fn read(&mut self) -> Result<NetworkSnapshot, NetworkError> {
        self.calls.send("read").unwrap();
        if let Some(release) = self.read_release.take() {
            release.recv_timeout(DEADLINE).unwrap();
        }
        if self.panic_read {
            self.panic_read = false;
            panic!("recorded read panic");
        }
        Ok(NetworkSnapshot { interfaces: vec![] })
    }
    fn open_settings(&mut self) -> Result<(), NetworkError> {
        self.calls.send("settings").unwrap();
        Ok(())
    }
    fn register(&mut self, wake: Wake) -> Result<(), NetworkError> {
        *self.wake.lock() = Some(wake);
        self.calls.send("register").unwrap();
        if let Some(release) = self.register_release.take() {
            release.recv_timeout(DEADLINE).unwrap();
        }
        if self.register_error {
            Err(NetworkError::new(
                NetworkErrorKind::ServiceUnavailable,
                "Recorded registration failed.",
            ))
        } else {
            Ok(())
        }
    }
    fn unregister(&mut self) -> Result<(), NetworkError> {
        self.calls.send("unregister").unwrap();
        if self.unregister_error {
            Err(NetworkError::new(
                NetworkErrorKind::Other,
                "Recorded retirement failed.",
            ))
        } else {
            self.wake.lock().take();
            Ok(())
        }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        let _ = self.calls.send("drop");
    }
}

fn recording() -> (
    Recording,
    Receiver<&'static str>,
    Arc<RecordingMutex<Option<Wake>>>,
) {
    let (calls, receiver) = channel();
    let wake = Arc::new(RecordingMutex::new(None));
    (
        Recording {
            calls,
            wake: Arc::clone(&wake),
            read_release: None,
            register_release: None,
            register_error: false,
            unregister_error: false,
            panic_read: false,
        },
        receiver,
        wake,
    )
}

fn next(calls: &Receiver<&'static str>) -> &'static str {
    calls.recv_timeout(DEADLINE).unwrap()
}

#[test]
fn construction_is_pure_and_unsupported_platform_never_returns_empty_snapshot() {
    let (owner, calls, _) = recording();
    let host = start(owner).unwrap();
    assert!(calls.try_recv().is_err());
    drop(host);
    assert_eq!(next(&calls), "drop");
    #[cfg(not(windows))]
    assert_eq!(
        native_network_host().err().unwrap().kind,
        NetworkErrorKind::Unsupported
    );
}

#[test]
fn independent_read_and_settings_flights_reject_busy_without_callbacks() {
    let (mut owner, calls, _) = recording();
    let (release, released) = channel();
    owner.read_release = Some(released);
    let host = start(owner).unwrap();
    let (completed, completions) = channel();
    let read_completed = completed.clone();
    host.read(Box::new(move |result| {
        read_completed.send(("read", result.is_ok())).unwrap();
    }))
    .unwrap();
    assert_eq!(next(&calls), "read");
    let rejected = Arc::new(AtomicBool::new(false));
    let read_rejected = Arc::clone(&rejected);
    assert_eq!(
        host.read(Box::new(move |_| {
            read_rejected.store(true, Ordering::SeqCst);
        }))
        .unwrap_err()
        .kind,
        NetworkErrorKind::Busy
    );
    host.open_settings(Box::new(move |result| {
        completed.send(("settings", result.is_ok())).unwrap();
    }))
    .unwrap();
    let settings_rejected = Arc::clone(&rejected);
    assert_eq!(
        host.open_settings(Box::new(move |_| {
            settings_rejected.store(true, Ordering::SeqCst);
        }))
        .unwrap_err()
        .kind,
        NetworkErrorKind::Busy
    );
    release.send(()).unwrap();
    assert_eq!(completions.recv_timeout(DEADLINE).unwrap(), ("read", true));
    assert_eq!(next(&calls), "settings");
    assert_eq!(
        completions.recv_timeout(DEADLINE).unwrap(),
        ("settings", true)
    );
    assert!(!rejected.load(Ordering::SeqCst));
    drop(host);
    assert_eq!(next(&calls), "drop");
}

#[test]
fn completion_reentry_has_released_admission_and_consumer_panic_is_contained() {
    let (owner, calls, _) = recording();
    let host = start(owner).unwrap();
    let reentrant = Arc::clone(&host);
    let (completed, completion) = channel();
    host.read(Box::new(move |_| {
        reentrant
            .read(Box::new(move |result| {
                completed.send(result.is_ok()).unwrap();
            }))
            .unwrap();
        panic!("recorded consumer panic");
    }))
    .unwrap();
    assert_eq!(next(&calls), "read");
    assert_eq!(next(&calls), "read");
    assert!(completion.recv_timeout(DEADLINE).unwrap());
    drop(host);
    assert_eq!(next(&calls), "drop");
}

#[test]
fn owner_panic_settles_once_and_keeps_read_service_usable() {
    let (mut owner, calls, _) = recording();
    owner.panic_read = true;
    let host = start(owner).unwrap();
    let (completed, completion) = channel();
    host.read(Box::new(move |result| {
        completed.send(result.unwrap_err().kind).unwrap();
    }))
    .unwrap();
    assert_eq!(next(&calls), "read");
    assert_eq!(
        completion.recv_timeout(DEADLINE).unwrap(),
        NetworkErrorKind::Other
    );
    let (completed, completion) = channel();
    host.read(Box::new(move |result| {
        completed.send(result.is_ok()).unwrap();
    }))
    .unwrap();
    assert_eq!(next(&calls), "read");
    assert!(completion.recv_timeout(DEADLINE).unwrap());
    drop(host);
    assert_eq!(next(&calls), "drop");
}

#[test]
fn registration_failure_is_not_false_readiness_and_reads_remain_independent() {
    let (mut owner, calls, _) = recording();
    owner.register_error = true;
    let host = start(owner).unwrap();
    let (event_sender, events) = channel();
    let guard = host
        .subscribe(Arc::new(move |event| {
            event_sender.send(event).unwrap();
        }))
        .unwrap()
        .unwrap();
    assert_eq!(next(&calls), "register");
    assert!(
        matches!(events.recv_timeout(DEADLINE).unwrap(), NetworkEvent::WatchUnavailable(error)
        if error.kind == NetworkErrorKind::ServiceUnavailable)
    );
    let (completed, completion) = channel();
    host.read(Box::new(move |result| {
        completed.send(result.is_ok()).unwrap();
    }))
    .unwrap();
    assert_eq!(next(&calls), "read");
    assert!(completion.recv_timeout(DEADLINE).unwrap());
    assert!(events.try_recv().is_err());
    drop(guard);
    assert_eq!(next(&calls), "unregister");
    drop(host);
    assert_eq!(next(&calls), "drop");
}

#[test]
fn guard_drop_before_registration_ready_closes_admission_without_join() {
    let (mut owner, calls, _) = recording();
    let (release, released) = channel();
    owner.register_release = Some(released);
    let host = start(owner).unwrap();
    let event_called = Arc::new(AtomicBool::new(false));
    let called = Arc::clone(&event_called);
    let guard = host
        .subscribe(Arc::new(move |_| {
            called.store(true, Ordering::SeqCst);
        }))
        .unwrap()
        .unwrap();
    assert_eq!(next(&calls), "register");
    // This returns before the blocked worker registration is allowed to complete.
    drop(guard);
    assert!(!event_called.load(Ordering::SeqCst));
    release.send(()).unwrap();
    assert_eq!(next(&calls), "unregister");
    assert!(!event_called.load(Ordering::SeqCst));
    drop(host);
    assert_eq!(next(&calls), "drop");
}

#[test]
fn dirty_bursts_coalesce_while_native_read_is_in_flight() {
    let (mut owner, calls, wake) = recording();
    let (release, released) = channel();
    owner.read_release = Some(released);
    let host = start(owner).unwrap();
    let (event_sender, events) = channel();
    let guard = host
        .subscribe(Arc::new(move |event| {
            event_sender.send(event).unwrap();
        }))
        .unwrap()
        .unwrap();
    assert_eq!(next(&calls), "register");
    assert_eq!(
        events.recv_timeout(DEADLINE).unwrap(),
        NetworkEvent::WatchReady
    );
    let (completed, completion) = channel();
    host.read(Box::new(move |result| {
        completed.send(result.is_ok()).unwrap();
    }))
    .unwrap();
    assert_eq!(next(&calls), "read");
    let wake = wake.lock().clone().unwrap();
    for _ in 0..10_000 {
        wake();
    }
    release.send(()).unwrap();
    assert!(completion.recv_timeout(DEADLINE).unwrap());
    assert_eq!(
        events.recv_timeout(DEADLINE).unwrap(),
        NetworkEvent::Changed
    );
    // A settings completion forms a worker barrier after the whole dirty burst.
    let (completed, completion) = channel();
    host.open_settings(Box::new(move |_| {
        completed.send(()).unwrap();
    }))
    .unwrap();
    assert_eq!(next(&calls), "settings");
    completion.recv_timeout(DEADLINE).unwrap();
    assert!(events.try_recv().is_err());
    drop(guard);
    assert_eq!(next(&calls), "unregister");
    drop(host);
    assert_eq!(next(&calls), "drop");
}

#[test]
fn last_facade_drop_is_nonblocking_and_settles_accepted_work_once() {
    let (mut owner, calls, _) = recording();
    let (release, released) = channel();
    owner.read_release = Some(released);
    let host = start(owner).unwrap();
    let (completed, completion) = channel();
    host.read(Box::new(move |result| {
        completed.send(("read", result.is_ok())).unwrap();
    }))
    .unwrap();
    assert_eq!(next(&calls), "read");
    let (completed, settings_completion) = channel();
    host.open_settings(Box::new(move |result| {
        completed.send(result.unwrap_err().kind).unwrap();
    }))
    .unwrap();
    drop(host); // no GUI join, even with native read blocked
    release.send(()).unwrap();
    assert_eq!(completion.recv_timeout(DEADLINE).unwrap(), ("read", true));
    assert_eq!(
        settings_completion.recv_timeout(DEADLINE).unwrap(),
        NetworkErrorKind::Stopped
    );
    assert_eq!(next(&calls), "drop");
}

#[test]
fn subscriber_admission_is_bounded_and_callback_panics_do_not_kill_worker() {
    let (owner, calls, _) = recording();
    let host = start(owner).unwrap();
    let mut guards = vec![];
    for _ in 0..MAX_SUBSCRIBERS {
        guards.push(
            host.subscribe(Arc::new(|_| {
                panic!("recorded notification consumer panic");
            }))
            .unwrap()
            .unwrap(),
        );
    }
    assert_eq!(
        host.subscribe(Arc::new(|_| {})).err().unwrap().kind,
        NetworkErrorKind::Busy
    );
    assert_eq!(next(&calls), "register");
    let (completed, completion) = channel();
    host.open_settings(Box::new(move |result| {
        completed.send(result.is_ok()).unwrap();
    }))
    .unwrap();
    assert_eq!(next(&calls), "settings");
    assert!(completion.recv_timeout(DEADLINE).unwrap());
    drop(guards);
    assert_eq!(next(&calls), "unregister");
    drop(host);
    assert_eq!(next(&calls), "drop");
}

#[test]
fn failed_retirement_is_recorded_without_blocking_reads_or_guard_drop() {
    let (mut owner, calls, _) = recording();
    owner.unregister_error = true;
    let host = start(owner).unwrap();
    let (event_sender, events) = channel();
    let guard = host
        .subscribe(Arc::new(move |event| {
            event_sender.send(event).unwrap();
        }))
        .unwrap()
        .unwrap();
    assert_eq!(next(&calls), "register");
    assert_eq!(
        events.recv_timeout(DEADLINE).unwrap(),
        NetworkEvent::WatchReady
    );
    drop(guard);
    assert_eq!(next(&calls), "unregister");
    let (completed, completion) = channel();
    host.read(Box::new(move |result| {
        completed.send(result.is_ok()).unwrap();
    }))
    .unwrap();
    assert_eq!(next(&calls), "read");
    assert!(completion.recv_timeout(DEADLINE).unwrap());
    drop(host);
    assert_eq!(next(&calls), "drop");
}

#[test]
fn stopped_and_disconnected_admission_transfers_no_completions_or_events() {
    for disconnected in [false, true] {
        let (sender, receiver) = sync_channel(1);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                stopped: !disconnected,
                ..State::default()
            }),
            sender,
            dirty: AtomicBool::new(false),
            read_gate: FlightGate::default(),
            action_gate: FlightGate::default(),
        });
        if disconnected {
            drop(receiver);
        }
        let host = Host { shared };
        let called = Arc::new(AtomicBool::new(false));
        let read_called = Arc::clone(&called);
        assert_eq!(
            host.read(Box::new(move |_| {
                read_called.store(true, Ordering::SeqCst);
            }))
            .unwrap_err()
            .kind,
            NetworkErrorKind::Stopped
        );
        let action_called = Arc::clone(&called);
        assert_eq!(
            host.open_settings(Box::new(move |_| {
                action_called.store(true, Ordering::SeqCst);
            }))
            .unwrap_err()
            .kind,
            NetworkErrorKind::Stopped
        );
        let event_called = Arc::clone(&called);
        assert_eq!(
            host.subscribe(Arc::new(move |_| {
                event_called.store(true, Ordering::SeqCst);
            }))
            .err()
            .unwrap()
            .kind,
            NetworkErrorKind::Stopped
        );
        assert!(!called.load(Ordering::SeqCst));
    }
}

#[test]
fn subscribers_share_registration_but_each_receives_true_readiness_once() {
    let (owner, calls, _) = recording();
    let host = start(owner).unwrap();
    let (first_sender, first_events) = channel();
    let first = host
        .subscribe(Arc::new(move |event| {
            first_sender.send(event).unwrap();
        }))
        .unwrap()
        .unwrap();
    assert_eq!(next(&calls), "register");
    assert_eq!(
        first_events.recv_timeout(DEADLINE).unwrap(),
        NetworkEvent::WatchReady
    );
    let (second_sender, second_events) = channel();
    let second = host
        .subscribe(Arc::new(move |event| {
            second_sender.send(event).unwrap();
        }))
        .unwrap()
        .unwrap();
    assert_eq!(
        second_events.recv_timeout(DEADLINE).unwrap(),
        NetworkEvent::WatchReady
    );
    assert!(first_events.try_recv().is_err());
    drop(first);
    let (completed, completion) = channel();
    host.open_settings(Box::new(move |_| {
        completed.send(()).unwrap();
    }))
    .unwrap();
    assert_eq!(next(&calls), "settings"); // first drop did not unregister the shared watch
    completion.recv_timeout(DEADLINE).unwrap();
    drop(second);
    assert_eq!(next(&calls), "unregister");
    drop(host);
    assert_eq!(next(&calls), "drop");
}

#[test]
fn independent_hosts_do_not_share_busy_admission() {
    let (mut first_owner, first_calls, _) = recording();
    let (release, released) = channel();
    first_owner.read_release = Some(released);
    let first = start(first_owner).unwrap();
    let (completed, completion) = channel();
    first
        .read(Box::new(move |result| {
            completed.send(result.is_ok()).unwrap();
        }))
        .unwrap();
    assert_eq!(next(&first_calls), "read");
    let (second_owner, second_calls, _) = recording();
    let second = start(second_owner).unwrap();
    let (second_completed, second_completion) = channel();
    second
        .read(Box::new(move |result| {
            second_completed.send(result.is_ok()).unwrap();
        }))
        .unwrap();
    assert_eq!(next(&second_calls), "read");
    assert!(second_completion.recv_timeout(DEADLINE).unwrap());
    drop(second);
    assert_eq!(next(&second_calls), "drop");
    release.send(()).unwrap();
    assert!(completion.recv_timeout(DEADLINE).unwrap());
    drop(first);
    assert_eq!(next(&first_calls), "drop");
}
