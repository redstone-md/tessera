// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use parking_lot::Mutex;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread::ThreadId;
use std::time::Duration;

use tessera_system::media::{
    MediaAction, MediaCapabilities, MediaHost, MediaPlayback, MediaSession, MediaSessionKey,
};

use super::*;

const WAIT: Duration = Duration::from_secs(3);

struct Pause {
    entered: Sender<()>,
    release: Receiver<()>,
}

impl Pause {
    fn wait(self) {
        self.entered.send(()).unwrap();
        self.release.recv_timeout(WAIT).unwrap();
    }
}

fn pause() -> (Pause, Receiver<()>, Sender<()>) {
    let (entered, entered_receiver) = mpsc::channel();
    let (release, release_receiver) = mpsc::channel();
    (
        Pause {
            entered,
            release: release_receiver,
        },
        entered_receiver,
        release,
    )
}

struct Recording {
    snapshot: Result<MediaSnapshot, MediaError>,
    transport_result: Result<(), MediaError>,
    start_error: Option<MediaError>,
    refresh_error: Option<MediaError>,
    fail_initializations: usize,
    initializations: usize,
    reads: usize,
    starts: usize,
    refreshes: usize,
    stops: usize,
    commands: Vec<MediaCommand>,
    operations: Vec<(&'static str, ThreadId)>,
    dirty: Option<Arc<dyn Fn() + Send + Sync>>,
    old_callbacks: Vec<Arc<dyn Fn() + Send + Sync>>,
    pause_read: Option<Pause>,
    pause_start: Option<Pause>,
    panic_read: bool,
    panic_transport: bool,
    panic_stop: bool,
}

impl Recording {
    fn new() -> Self {
        Self {
            snapshot: Ok(MediaSnapshot {
                current: Some(session()),
            }),
            transport_result: Ok(()),
            start_error: None,
            refresh_error: None,
            fail_initializations: 0,
            initializations: 0,
            reads: 0,
            starts: 0,
            refreshes: 0,
            stops: 0,
            commands: Vec::new(),
            operations: Vec::new(),
            dirty: None,
            old_callbacks: Vec::new(),
            pause_read: None,
            pause_start: None,
            panic_read: false,
            panic_transport: false,
            panic_stop: false,
        }
    }

    fn record(&mut self, operation: &'static str) {
        self.operations
            .push((operation, std::thread::current().id()));
    }
}

/// Deliberately !Send, just like an apartment-owned native session adapter.
struct RecordingDriver {
    recording: Arc<Mutex<Recording>>,
    retired: Sender<ThreadId>,
    finished: Sender<ThreadId>,
    _owner_only: Rc<()>,
}

impl Driver for RecordingDriver {
    fn read(&mut self) -> Result<MediaSnapshot, MediaError> {
        let (pause, panic, result) = {
            let mut recording = self.recording.lock();
            recording.record("read");
            recording.reads += 1;
            let panic = std::mem::take(&mut recording.panic_read);
            (
                recording.pause_read.take(),
                panic,
                recording.snapshot.clone(),
            )
        };
        if let Some(pause) = pause {
            pause.wait();
        }
        assert!(!panic, "recorded read panic");
        result
    }

    fn execute(&mut self, command: MediaCommand) -> Result<(), MediaError> {
        let panic = {
            let mut recording = self.recording.lock();
            recording.record("execute");
            let current = recording
                .snapshot
                .as_ref()
                .map_err(Clone::clone)?
                .current
                .as_ref()
                .ok_or_else(|| error(MediaErrorKind::SessionChanged))?;
            if current.key != command.expected_session {
                return Err(error(MediaErrorKind::SessionChanged));
            }
            if !current.capabilities.allows(command.action) {
                return Err(error(MediaErrorKind::CommandUnavailable));
            }
            recording.commands.push(command);
            std::mem::take(&mut recording.panic_transport)
        };
        assert!(!panic, "recorded transport panic after native effect");
        self.recording.lock().transport_result.clone()
    }

    fn start_watch(&mut self, dirty: Arc<dyn Fn() + Send + Sync>) -> Result<(), MediaError> {
        let pause = {
            let mut recording = self.recording.lock();
            recording.record("start");
            recording.starts += 1;
            recording.pause_start.take()
        };
        if let Some(pause) = pause {
            pause.wait();
        }
        let mut recording = self.recording.lock();
        // Simulates acquiring one callback before a second registration fails.
        recording.old_callbacks.push(Arc::clone(&dirty));
        recording.dirty = Some(dirty);
        recording.start_error.clone().map_or(Ok(()), Err)
    }

    fn refresh_watch(&mut self) -> Result<(), MediaError> {
        let mut recording = self.recording.lock();
        recording.record("refresh");
        recording.refreshes += 1;
        recording.refresh_error.clone().map_or(Ok(()), Err)
    }

    fn stop_watch(&mut self) {
        let panic = {
            let mut recording = self.recording.lock();
            recording.record("stop");
            recording.stops += 1;
            recording.dirty = None;
            std::mem::take(&mut recording.panic_stop)
        };
        self.retired.send(std::thread::current().id()).unwrap();
        assert!(!panic, "recorded retirement panic");
    }
}

impl Drop for RecordingDriver {
    fn drop(&mut self) {
        self.recording.lock().record("drop");
        let _ = self.finished.send(std::thread::current().id());
    }
}

struct Harness {
    service: MediaService,
    recording: Arc<Mutex<Recording>>,
    retired: Receiver<ThreadId>,
    finished: Receiver<ThreadId>,
}

fn start(recording: Recording) -> Harness {
    let recording = Arc::new(Mutex::new(recording));
    let driver_recording = Arc::clone(&recording);
    let (retired_sender, retired) = mpsc::channel();
    let (finished_sender, finished) = mpsc::channel();
    let service = spawn(move || {
        {
            let mut recording = driver_recording.lock();
            recording.record("initialize");
            recording.initializations += 1;
            if recording.fail_initializations > 0 {
                recording.fail_initializations -= 1;
                return Err(error(MediaErrorKind::Unavailable));
            }
        }
        Ok(RecordingDriver {
            recording: Arc::clone(&driver_recording),
            retired: retired_sender.clone(),
            finished: finished_sender.clone(),
            _owner_only: Rc::new(()),
        })
    })
    .unwrap();
    Harness {
        service,
        recording,
        retired,
        finished,
    }
}

fn finish(harness: Harness) {
    let Harness {
        service,
        recording,
        finished,
        ..
    } = harness;
    drop(service);
    let owner = finished.recv_timeout(WAIT).unwrap();
    assert_ne!(owner, std::thread::current().id());
    assert!(
        recording
            .lock()
            .operations
            .iter()
            .all(|(_, thread)| *thread == owner)
    );
}

fn session() -> MediaSession {
    MediaSession {
        key: MediaSessionKey::issue().unwrap(),
        source_app_id: "same.player.aumid".into(),
        title: "Track".into(),
        author: "Artist".into(),
        playback: MediaPlayback::Playing,
        capabilities: MediaCapabilities {
            previous: true,
            toggle: true,
            next: true,
        },
        artwork: None,
        artwork_notice: None,
    }
}

fn error(kind: MediaErrorKind) -> MediaError {
    MediaError::with_hresult(kind, "recorded media failure", -2147024891)
}

fn read(service: &MediaService) -> Result<MediaSnapshot, MediaError> {
    let (sender, receiver) = mpsc::channel();
    service
        .read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    receiver.recv_timeout(WAIT).unwrap()
}

fn execute(
    service: &MediaService,
    key: MediaSessionKey,
    action: MediaAction,
) -> Result<(), MediaError> {
    let (sender, receiver) = mpsc::channel();
    service
        .execute(
            MediaCommand {
                expected_session: key,
                action,
            },
            Box::new(move |result| sender.send(result).unwrap()),
        )
        .unwrap();
    receiver.recv_timeout(WAIT).unwrap()
}

fn watch(service: &MediaService) -> (Box<dyn Send>, Receiver<MediaEvent>) {
    let (sender, receiver) = mpsc::channel();
    let guard = service
        .subscribe(Arc::new(move |event| sender.send(event).unwrap()))
        .unwrap()
        .unwrap();
    (guard, receiver)
}

#[test]
fn production_driver_preserves_empty_error_playback_and_artwork_health() {
    let harness = start(Recording::new());
    harness.recording.lock().snapshot = Ok(MediaSnapshot { current: None });
    assert_eq!(read(&harness.service).unwrap().current, None);
    let unavailable = error(MediaErrorKind::Unavailable);
    harness.recording.lock().snapshot = Err(unavailable.clone());
    assert_eq!(read(&harness.service), Err(unavailable));
    for playback in [
        MediaPlayback::Closed,
        MediaPlayback::Opened,
        MediaPlayback::Changing,
        MediaPlayback::Stopped,
        MediaPlayback::Playing,
        MediaPlayback::Paused,
    ] {
        let mut current = session();
        current.playback = playback;
        current.title = "🎧".repeat(513);
        current.source_app_id = "界".repeat(257);
        current.artwork_notice = Some(error(MediaErrorKind::Other));
        harness.recording.lock().snapshot = Ok(MediaSnapshot {
            current: Some(current),
        });
        let actual = read(&harness.service).unwrap().current.unwrap();
        assert_eq!(actual.playback, playback);
        assert_eq!(actual.title.chars().count(), 512);
        assert!(actual.source_app_id.is_empty());
        assert!(actual.artwork.is_none());
        assert_eq!(actual.artwork_notice, Some(error(MediaErrorKind::Other)));
        assert!(actual.capabilities.toggle);
    }
    finish(harness);
}

#[test]
fn displayed_identity_replacement_capability_and_native_rejection_never_retry() {
    let harness = start(Recording::new());
    let displayed = read(&harness.service).unwrap().current.unwrap();
    let replacement = session();
    assert_eq!(displayed.source_app_id, replacement.source_app_id);
    assert_ne!(displayed.key, replacement.key);
    harness.recording.lock().snapshot = Ok(MediaSnapshot {
        current: Some(replacement.clone()),
    });
    assert_eq!(
        execute(&harness.service, displayed.key, MediaAction::Toggle)
            .unwrap_err()
            .kind,
        MediaErrorKind::SessionChanged
    );
    let mut disabled = replacement.clone();
    disabled.capabilities.toggle = false;
    harness.recording.lock().snapshot = Ok(MediaSnapshot {
        current: Some(disabled),
    });
    assert_eq!(
        execute(&harness.service, replacement.key, MediaAction::Toggle)
            .unwrap_err()
            .kind,
        MediaErrorKind::CommandUnavailable
    );
    assert!(harness.recording.lock().commands.is_empty());
    harness.recording.lock().snapshot = Ok(MediaSnapshot {
        current: Some(replacement.clone()),
    });
    harness.recording.lock().transport_result = Err(error(MediaErrorKind::Rejected));
    assert_eq!(
        execute(&harness.service, replacement.key, MediaAction::Next),
        Err(error(MediaErrorKind::Rejected))
    );
    assert_eq!(harness.recording.lock().commands.len(), 1);
    harness.recording.lock().transport_result = Err(error(MediaErrorKind::Other));
    assert_eq!(
        execute(&harness.service, replacement.key, MediaAction::Previous),
        Err(error(MediaErrorKind::Other))
    );
    assert_eq!(harness.recording.lock().commands.len(), 2);
    harness.recording.lock().transport_result = Ok(());
    assert_eq!(
        execute(&harness.service, replacement.key, MediaAction::Toggle),
        Ok(())
    );
    assert_eq!(harness.recording.lock().commands.len(), 3);
    assert_eq!(
        harness.recording.lock().reads,
        1,
        "transport does not perform optimistic readback"
    );
    finish(harness);
}

#[test]
fn initialization_retries_only_on_reads_and_watch_readiness_recovers() {
    let mut recording = Recording::new();
    recording.fail_initializations = 2;
    let harness = start(recording);
    let (guard, events) = watch(&harness.service);
    assert_eq!(
        events.recv_timeout(WAIT).unwrap(),
        MediaEvent::WatchUnavailable(error(MediaErrorKind::Unavailable))
    );
    let key = MediaSessionKey::issue().unwrap();
    assert_eq!(
        execute(&harness.service, key, MediaAction::Toggle),
        Err(error(MediaErrorKind::Unavailable))
    );
    assert_eq!(harness.recording.lock().initializations, 1);
    assert_eq!(
        read(&harness.service),
        Err(error(MediaErrorKind::Unavailable))
    );
    assert_eq!(harness.recording.lock().initializations, 2);
    assert!(read(&harness.service).unwrap().current.is_some());
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::WatchReady);
    assert_eq!(harness.recording.lock().initializations, 3);
    assert!(harness.recording.lock().commands.is_empty());
    drop(guard);
    harness.retired.recv_timeout(WAIT).unwrap();
    finish(harness);
}

#[test]
fn one_shared_flight_rejects_read_and_transport_without_callbacks() {
    let harness = start(Recording::new());
    let key = read(&harness.service).unwrap().current.unwrap().key;
    let (pause, entered, release) = pause();
    harness.recording.lock().pause_read = Some(pause);
    let (result_sender, result_receiver) = mpsc::channel();
    harness
        .service
        .read(Box::new(move |result| result_sender.send(result).unwrap()))
        .unwrap();
    entered.recv_timeout(WAIT).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let read_calls = Arc::clone(&calls);
    assert!(
        harness
            .service
            .read(Box::new(move |_| {
                read_calls.fetch_add(1, Ordering::AcqRel);
            }))
            .is_err()
    );
    let command_calls = Arc::clone(&calls);
    assert!(
        harness
            .service
            .execute(
                MediaCommand {
                    expected_session: key,
                    action: MediaAction::Toggle
                },
                Box::new(move |_| {
                    command_calls.fetch_add(1, Ordering::AcqRel);
                })
            )
            .is_err()
    );
    assert_eq!(calls.load(Ordering::Acquire), 0);
    release.send(()).unwrap();
    assert!(result_receiver.recv_timeout(WAIT).unwrap().is_ok());
    assert!(result_receiver.try_recv().is_err());
    assert!(execute(&harness.service, key, MediaAction::Toggle).is_ok());
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert_eq!(harness.recording.lock().commands.len(), 1);
    finish(harness);
}

#[test]
fn completion_reentry_and_consumer_panic_release_admission_before_delivery() {
    let harness = start(Recording::new());
    let reentrant = harness.service.clone();
    let (sender, receiver) = mpsc::channel();
    harness
        .service
        .read(Box::new(move |result| {
            assert!(result.is_ok());
            reentrant
                .read(Box::new(move |next| sender.send(next).unwrap()))
                .unwrap();
            panic!("recorded consumer panic");
        }))
        .unwrap();
    assert!(receiver.recv_timeout(WAIT).unwrap().is_ok());
    assert!(receiver.try_recv().is_err());
    assert_eq!(harness.recording.lock().reads, 2);
    finish(harness);
}

#[test]
fn driver_panic_completes_once_retires_on_owner_and_never_replays_transport() {
    let harness = start(Recording::new());
    let key = read(&harness.service).unwrap().current.unwrap().key;
    let (guard, events) = watch(&harness.service);
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::WatchReady);
    harness.recording.lock().panic_transport = true;
    assert_eq!(
        execute(&harness.service, key, MediaAction::Toggle)
            .unwrap_err()
            .kind,
        MediaErrorKind::Other
    );
    assert!(matches!(
        events.recv_timeout(WAIT).unwrap(),
        MediaEvent::WatchUnavailable(_)
    ));
    let owner = harness.finished.recv_timeout(WAIT).unwrap();
    assert_eq!(harness.retired.recv_timeout(WAIT).unwrap(), owner);
    assert_eq!(harness.recording.lock().commands.len(), 1);
    assert!(execute(&harness.service, key, MediaAction::Toggle).is_err());
    assert_eq!(harness.recording.lock().initializations, 1);
    assert!(read(&harness.service).is_ok());
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::WatchReady);
    assert_eq!(harness.recording.lock().initializations, 2);
    assert_eq!(harness.recording.lock().commands.len(), 1);
    drop(guard);
    harness.retired.recv_timeout(WAIT).unwrap();
    finish(harness);
}

#[test]
fn read_panic_has_one_completion_and_reinitializes_only_on_the_next_read() {
    let harness = start(Recording::new());
    harness.recording.lock().panic_read = true;
    let (sender, receiver) = mpsc::channel();
    harness
        .service
        .read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    assert_eq!(
        receiver.recv_timeout(WAIT).unwrap().unwrap_err().kind,
        MediaErrorKind::Other
    );
    assert!(receiver.try_recv().is_err());
    harness.finished.recv_timeout(WAIT).unwrap();
    assert_eq!(harness.recording.lock().initializations, 1);
    assert!(read(&harness.service).is_ok());
    assert_eq!(harness.recording.lock().initializations, 2);
    finish(harness);
}

#[test]
fn watch_consumer_panics_cannot_abandon_reads_or_native_retirement() {
    let harness = start(Recording::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let event_calls = Arc::clone(&calls);
    let guard = harness
        .service
        .subscribe(Arc::new(move |_| {
            event_calls.fetch_add(1, Ordering::AcqRel);
            panic!("recorded watch consumer panic");
        }))
        .unwrap()
        .unwrap();
    read(&harness.service).unwrap();
    assert_eq!(calls.load(Ordering::Acquire), 1);
    let dirty = harness.recording.lock().dirty.clone().unwrap();
    dirty();
    read(&harness.service).unwrap();
    assert_eq!(calls.load(Ordering::Acquire), 2);
    drop(guard);
    harness.retired.recv_timeout(WAIT).unwrap();
    finish(harness);
}

#[test]
fn callback_storm_is_coalesced_and_rebind_precedes_changed() {
    let harness = start(Recording::new());
    let (guard, events) = watch(&harness.service);
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::WatchReady);
    let dirty = harness.recording.lock().dirty.clone().unwrap();
    let (pause, entered, release) = pause();
    harness.recording.lock().pause_read = Some(pause);
    let (sender, receiver) = mpsc::channel();
    harness
        .service
        .read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    entered.recv_timeout(WAIT).unwrap();
    for _ in 0..10_000 {
        dirty();
    }
    assert_eq!(harness.recording.lock().refreshes, 0);
    release.send(()).unwrap();
    receiver.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::Changed);
    assert_eq!(harness.recording.lock().refreshes, 1);
    read(&harness.service).unwrap();
    assert!(events.try_recv().is_err());
    drop(guard);
    harness.retired.recv_timeout(WAIT).unwrap();
    dirty();
    read(&harness.service).unwrap();
    assert_eq!(harness.recording.lock().refreshes, 1);
    assert!(events.try_recv().is_err());
    finish(harness);
}

#[test]
fn dirty_invalidation_survives_a_full_owner_queue_without_blocking_callbacks() {
    let harness = start(Recording::new());
    let (guard, events) = watch(&harness.service);
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::WatchReady);
    let dirty = harness.recording.lock().dirty.clone().unwrap();
    let (pause, entered, release) = pause();
    harness.recording.lock().pause_read = Some(pause);
    let (sender, receiver) = mpsc::channel();
    harness
        .service
        .read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    entered.recv_timeout(WAIT).unwrap();
    for _ in 0..QUEUE_CAPACITY {
        harness
            .service
            .queue
            .sender
            .try_send(Message::Wake)
            .unwrap_or_else(|_| panic!("expected queue capacity"));
    }
    for _ in 0..1_000 {
        dirty();
    }
    release.send(()).unwrap();
    receiver.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::Changed);
    assert_eq!(harness.recording.lock().refreshes, 1);
    drop(guard);
    harness.retired.recv_timeout(WAIT).unwrap();
    assert!(events.try_recv().is_err());
    finish(harness);
}

#[test]
fn partial_watch_start_rolls_back_and_late_callback_is_inert() {
    let mut recording = Recording::new();
    recording.start_error = Some(error(MediaErrorKind::WatchUnavailable));
    let harness = start(recording);
    let (guard, events) = watch(&harness.service);
    assert_eq!(
        events.recv_timeout(WAIT).unwrap(),
        MediaEvent::WatchUnavailable(error(MediaErrorKind::WatchUnavailable))
    );
    let owner = harness.retired.recv_timeout(WAIT).unwrap();
    let late = harness.recording.lock().old_callbacks[0].clone();
    assert!(harness.recording.lock().dirty.is_none());
    late();
    assert!(events.try_recv().is_err());
    harness.recording.lock().start_error = None;
    read(&harness.service).unwrap();
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::WatchReady);
    assert_eq!(harness.recording.lock().starts, 2);
    assert_eq!(harness.recording.lock().refreshes, 0);
    drop(guard);
    assert_eq!(harness.retired.recv_timeout(WAIT).unwrap(), owner);
    finish(harness);
}

#[test]
fn failed_rebind_retires_tokens_reports_unavailable_and_still_invalidates() {
    let harness = start(Recording::new());
    let (guard, events) = watch(&harness.service);
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::WatchReady);
    let dirty = harness.recording.lock().dirty.clone().unwrap();
    harness.recording.lock().refresh_error = Some(error(MediaErrorKind::WatchUnavailable));
    dirty();
    assert_eq!(
        events.recv_timeout(WAIT).unwrap(),
        MediaEvent::WatchUnavailable(error(MediaErrorKind::WatchUnavailable))
    );
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::Changed);
    harness.retired.recv_timeout(WAIT).unwrap();
    assert!(harness.recording.lock().dirty.is_none());
    harness.recording.lock().refresh_error = None;
    read(&harness.service).unwrap();
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::WatchReady);
    dirty();
    read(&harness.service).unwrap();
    assert_eq!(harness.recording.lock().refreshes, 1);
    assert!(events.try_recv().is_err());
    drop(guard);
    harness.retired.recv_timeout(WAIT).unwrap();
    finish(harness);
}

#[test]
fn guard_drop_during_start_suppresses_readiness_and_retires_on_owner() {
    let mut recording = Recording::new();
    let (pause, entered, release) = pause();
    recording.pause_start = Some(pause);
    let harness = start(recording);
    let (guard, events) = watch(&harness.service);
    entered.recv_timeout(WAIT).unwrap();
    drop(guard);
    release.send(()).unwrap();
    let owner = harness.retired.recv_timeout(WAIT).unwrap();
    assert_ne!(owner, std::thread::current().id());
    assert!(events.try_recv().is_err());
    assert!(harness.recording.lock().dirty.is_none());
    finish(harness);
}

#[test]
fn last_subscription_keeps_owner_alive_then_retires_all_resources_on_owner() {
    let harness = start(Recording::new());
    let (guard, events) = watch(&harness.service);
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::WatchReady);
    let dirty = harness.recording.lock().dirty.clone().unwrap();
    let Harness {
        service,
        recording,
        retired,
        finished,
    } = harness;
    drop(service);
    dirty();
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::Changed);
    assert!(finished.try_recv().is_err());
    drop(guard);
    let owner = retired.recv_timeout(WAIT).unwrap();
    assert_eq!(finished.recv_timeout(WAIT).unwrap(), owner);
    assert_ne!(owner, std::thread::current().id());
    dirty();
    assert!(events.try_recv().is_err());
    assert!(
        recording
            .lock()
            .operations
            .iter()
            .all(|(_, thread)| *thread == owner)
    );
}

#[test]
fn last_facade_drop_never_joins_a_stalled_native_operation() {
    let harness = start(Recording::new());
    let (pause, entered, release) = pause();
    harness.recording.lock().pause_read = Some(pause);
    let (sender, receiver) = mpsc::channel();
    harness
        .service
        .read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    entered.recv_timeout(WAIT).unwrap();
    let Harness {
        service,
        recording,
        finished,
        ..
    } = harness;
    let (dropped_sender, dropped_receiver) = mpsc::channel();
    std::thread::spawn(move || {
        drop(service);
        dropped_sender.send(()).unwrap();
    });
    dropped_receiver.recv_timeout(WAIT).unwrap();
    assert!(finished.try_recv().is_err());
    release.send(()).unwrap();
    receiver.recv_timeout(WAIT).unwrap().unwrap();
    let owner = finished.recv_timeout(WAIT).unwrap();
    assert!(
        recording
            .lock()
            .operations
            .iter()
            .all(|(_, thread)| *thread == owner)
    );
    assert!(receiver.try_recv().is_err());
}

#[test]
fn retirement_panic_cannot_abandon_next_accepted_completion() {
    let harness = start(Recording::new());
    let (guard, events) = watch(&harness.service);
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::WatchReady);
    harness.recording.lock().panic_stop = true;
    drop(guard);
    harness.retired.recv_timeout(WAIT).unwrap();
    harness.finished.recv_timeout(WAIT).unwrap();
    assert!(read(&harness.service).is_ok());
    assert_eq!(harness.recording.lock().initializations, 2);
    finish(harness);
}

fn isolated_queue(capacity: usize) -> (Arc<QueueLifetime>, Receiver<Message>) {
    let (sender, receiver) = mpsc::sync_channel(capacity);
    let queue = Arc::new(QueueLifetime {
        sender,
        state: Arc::new(QueueState {
            shutdown: AtomicBool::new(false),
            subscriptions: AtomicUsize::new(0),
        }),
        flight: FlightGate::default(),
    });
    (queue, receiver)
}

#[test]
fn full_or_disconnected_queue_rejects_without_callbacks_and_releases_flight() {
    let (queue, receiver) = isolated_queue(1);
    queue
        .sender
        .try_send(Message::Wake)
        .unwrap_or_else(|_| panic!("empty queue"));
    let calls = Arc::new(AtomicUsize::new(0));
    let full_calls = Arc::clone(&calls);
    assert!(
        queue
            .read(Box::new(move |_| {
                full_calls.fetch_add(1, Ordering::AcqRel);
            }))
            .is_err()
    );
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert!(queue.flight.try_enter().is_some());
    let event_calls = Arc::clone(&calls);
    assert!(
        queue
            .subscribe(Arc::new(move |_| {
                event_calls.fetch_add(1, Ordering::AcqRel);
            }))
            .is_err()
    );
    assert_eq!(queue.state.subscriptions.load(Ordering::Acquire), 0);
    drop(receiver);
    let disconnected_calls = Arc::clone(&calls);
    assert!(
        queue
            .execute(
                MediaCommand {
                    expected_session: MediaSessionKey::issue().unwrap(),
                    action: MediaAction::Toggle
                },
                Box::new(move |_| {
                    disconnected_calls.fetch_add(1, Ordering::AcqRel);
                })
            )
            .is_err()
    );
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert!(queue.flight.try_enter().is_some());
}

#[test]
fn receiver_teardown_completes_accepted_request_once_and_releases_admission() {
    let (queue, receiver) = isolated_queue(1);
    let (sender, result_receiver) = mpsc::channel();
    queue
        .read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    assert!(queue.flight.try_enter().is_none());
    drop(receiver);
    assert_eq!(
        result_receiver
            .recv_timeout(WAIT)
            .unwrap()
            .unwrap_err()
            .kind,
        MediaErrorKind::Unavailable
    );
    assert!(result_receiver.try_recv().is_err());
    assert!(queue.flight.try_enter().is_some());
}

#[test]
fn subscription_admission_is_bounded_and_shutdown_survives_a_full_queue() {
    let (queue, receiver) = isolated_queue(MAX_SUBSCRIPTIONS);
    let mut guards = Vec::new();
    for _ in 0..MAX_SUBSCRIPTIONS {
        guards.push(
            queue
                .subscribe(Arc::new(|_| panic!("no actor delivery")))
                .unwrap()
                .unwrap(),
        );
    }
    assert!(
        queue
            .subscribe(Arc::new(|_| panic!("rejected event")))
            .is_err()
    );
    assert_eq!(
        queue.state.subscriptions.load(Ordering::Acquire),
        MAX_SUBSCRIPTIONS
    );
    let state = Arc::clone(&queue.state);
    drop(guards);
    assert_eq!(state.subscriptions.load(Ordering::Acquire), 0);
    drop(queue);
    assert!(
        state.shutdown.load(Ordering::Acquire),
        "shutdown is out-of-band even when Wake cannot enqueue"
    );
    assert_eq!(receiver.try_iter().count(), MAX_SUBSCRIPTIONS);
}

#[cfg(not(windows))]
#[test]
fn production_constructor_off_windows_is_honestly_unsupported() {
    assert!(matches!(
        MediaService::new(),
        Err(MediaError {
            kind: MediaErrorKind::Unsupported,
            ..
        })
    ));
}
