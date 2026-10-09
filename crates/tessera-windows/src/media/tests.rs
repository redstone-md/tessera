// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use parking_lot::Mutex;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread::ThreadId;
use std::time::Duration;

use tessera_system::media::{
    MediaAction, MediaCapabilities, MediaCommand, MediaHost, MediaObservationRevision,
    MediaPlayback, MediaRequest, MediaSeekCommand, MediaSeekObservation, MediaSession,
    MediaSessionKey, MediaTimeline,
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
    read_watch_error: Option<MediaError>,
    fail_initializations: usize,
    initializations: usize,
    reads: usize,
    starts: usize,
    refreshes: usize,
    stops: usize,
    commands: Vec<MediaCommand>,
    seeks: Vec<MediaSeekCommand>,
    operations: Vec<(&'static str, ThreadId)>,
    dirty: Option<Arc<dyn Fn(MediaEvent) + Send + Sync>>,
    old_callbacks: Vec<Arc<dyn Fn(MediaEvent) + Send + Sync>>,
    pause_read: Option<Pause>,
    pause_execute: Option<Pause>,
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
            read_watch_error: None,
            fail_initializations: 0,
            initializations: 0,
            reads: 0,
            starts: 0,
            refreshes: 0,
            stops: 0,
            commands: Vec::new(),
            operations: Vec::new(),
            seeks: Vec::new(),
            dirty: None,
            old_callbacks: Vec::new(),
            pause_read: None,
            pause_execute: None,
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
    watch_failure: Option<MediaError>,
    _owner_only: Rc<()>,
}

impl Driver for RecordingDriver {
    fn read(&mut self) -> Result<MediaSnapshot, MediaError> {
        let (pause, panic, result) = {
            let mut recording = self.recording.lock();
            recording.record("read");
            recording.reads += 1;
            let panic = std::mem::take(&mut recording.panic_read);
            if let Some(error) = recording.read_watch_error.take() {
                // A read can lose native registrations without losing media facts.
                recording.record("read-retire");
                recording.dirty = None;
                if let Ok(snapshot) = &mut recording.snapshot
                    && let Some(current) = &mut snapshot.current
                {
                    current.seek = Err(error.clone());
                }
                self.watch_failure = Some(error);
            }
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

    fn take_watch_failure(&mut self) -> Option<MediaError> {
        self.watch_failure.take()
    }

    fn execute(&mut self, request: MediaRequest) -> Result<(), MediaError> {
        let (pause, panic) = {
            let mut recording = self.recording.lock();
            recording.record("execute");
            let current = recording
                .snapshot
                .as_ref()
                .map_err(Clone::clone)?
                .current
                .as_ref()
                .ok_or_else(|| error(MediaErrorKind::SessionChanged))?;
            match request {
                MediaRequest::Transport(command) => {
                    if current.key != command.expected_session {
                        return Err(error(MediaErrorKind::SessionChanged));
                    }
                    if !current.capabilities.allows(command.action) {
                        return Err(error(MediaErrorKind::CommandUnavailable));
                    }
                    recording.commands.push(command);
                }
                MediaRequest::Seek(command) => {
                    if current.key != command.expected_session {
                        return Err(error(MediaErrorKind::SessionChanged));
                    }
                    let observation = current.seek.as_ref().map_err(Clone::clone)?;
                    if observation.revision != command.expected_revision
                        || observation.min_ticks != command.observed_min_ticks
                        || observation.max_ticks != command.observed_max_ticks
                    {
                        return Err(error(MediaErrorKind::SessionChanged));
                    }
                    if !(observation.min_ticks..=observation.max_ticks)
                        .contains(&command.position_ticks)
                    {
                        return Err(error(MediaErrorKind::CommandUnavailable));
                    }
                    recording.seeks.push(command);
                }
            }
            (
                recording.pause_execute.take(),
                std::mem::take(&mut recording.panic_transport),
            )
        };
        if let Some(pause) = pause {
            pause.wait();
        }
        assert!(!panic, "recorded transport panic after native effect");
        self.recording.lock().transport_result.clone()
    }

    fn start_watch(
        &mut self,
        dirty: Arc<dyn Fn(MediaEvent) + Send + Sync>,
    ) -> Result<(), MediaError> {
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
        let result = recording.start_error.clone().map_or(Ok(()), Err);
        if result.is_ok() {
            self.watch_failure = None;
        }
        result
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
            watch_failure: None,
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
        timeline: Ok(MediaTimeline {
            start_ticks: 0,
            end_ticks: 900_000_000,
            position_ticks: 300_000_000,
            min_seek_ticks: 0,
            max_seek_ticks: 900_000_000,
            last_updated_utc_ticks: None,
        }),
        seek: Ok(MediaSeekObservation {
            revision: MediaObservationRevision::issue().unwrap(),
            min_ticks: 0,
            max_ticks: 900_000_000,
        }),
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
            }
            .into(),
            Box::new(move |result| sender.send(result).unwrap()),
        )
        .unwrap();
    receiver.recv_timeout(WAIT).unwrap()
}

fn seek_command(current: &MediaSession) -> MediaSeekCommand {
    let observation = current.seek.as_ref().unwrap();
    MediaSeekCommand {
        expected_session: current.key,
        expected_revision: observation.revision,
        observed_min_ticks: observation.min_ticks,
        observed_max_ticks: observation.max_ticks,
        position_ticks: 450_000_000,
    }
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
fn actor_preserves_independent_timeline_and_seek_failures_and_transport_completion() {
    let harness = start(Recording::new());
    let mut expected = session();
    expected.timeline = Err(error(MediaErrorKind::Unavailable));
    expected.seek = Err(error(MediaErrorKind::CommandUnavailable));
    expected.artwork_notice = Some(error(MediaErrorKind::Other));
    harness.recording.lock().snapshot = Ok(MediaSnapshot {
        current: Some(expected.clone()),
    });
    assert_eq!(
        read(&harness.service).unwrap().current,
        Some(expected.clone())
    );
    for action in [
        MediaAction::Previous,
        MediaAction::Toggle,
        MediaAction::Next,
    ] {
        execute(&harness.service, expected.key, action).unwrap();
    }
    assert_eq!(
        harness.recording.lock().commands,
        [
            MediaAction::Previous,
            MediaAction::Toggle,
            MediaAction::Next
        ]
        .map(|action| MediaCommand {
            expected_session: expected.key,
            action
        })
    );
    assert_eq!(harness.recording.lock().reads, 1);
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
    let current = read(&harness.service).unwrap().current.unwrap();
    let key = current.key;
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
                }
                .into(),
                Box::new(move |_| {
                    command_calls.fetch_add(1, Ordering::AcqRel);
                })
            )
            .is_err()
    );
    let seek_calls = Arc::clone(&calls);
    assert!(
        harness
            .service
            .execute(
                MediaRequest::Seek(seek_command(&current)),
                Box::new(move |_| {
                    seek_calls.fetch_add(1, Ordering::AcqRel);
                }),
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
fn seek_owns_shared_flight_and_panic_never_replays_accepted_native_effect() {
    let harness = start(Recording::new());
    let current = read(&harness.service).unwrap().current.unwrap();
    let command = seek_command(&current);
    let (pause, entered, release) = pause();
    {
        let mut recording = harness.recording.lock();
        recording.pause_execute = Some(pause);
        recording.panic_transport = true;
    }
    let (sender, receiver) = mpsc::channel();
    harness
        .service
        .execute(
            MediaRequest::Seek(command),
            Box::new(move |result| sender.send(result).unwrap()),
        )
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
    for request in [
        MediaCommand {
            expected_session: current.key,
            action: MediaAction::Toggle,
        }
        .into(),
        MediaRequest::Seek(command),
    ] {
        let command_calls = Arc::clone(&calls);
        assert!(
            harness
                .service
                .execute(
                    request,
                    Box::new(move |_| {
                        command_calls.fetch_add(1, Ordering::AcqRel);
                    })
                )
                .is_err()
        );
    }
    assert_eq!(calls.load(Ordering::Acquire), 0);
    release.send(()).unwrap();
    assert_eq!(
        receiver.recv_timeout(WAIT).unwrap().unwrap_err().kind,
        MediaErrorKind::Other
    );
    assert!(receiver.try_recv().is_err());
    harness.finished.recv_timeout(WAIT).unwrap();
    // The flight is free and the facade remains live: queue admission succeeds,
    // but the retired driver fails this accepted intent through its completion.
    let (sender, receiver) = mpsc::channel();
    harness
        .service
        .execute(
            MediaRequest::Seek(command),
            Box::new(move |result| sender.send(result).unwrap()),
        )
        .unwrap();
    assert_eq!(
        receiver.recv_timeout(WAIT).unwrap().unwrap_err().kind,
        MediaErrorKind::Other
    );
    assert!(receiver.try_recv().is_err());
    assert_eq!(harness.recording.lock().initializations, 1);
    assert_eq!(harness.recording.lock().seeks, [command]);
    read(&harness.service).unwrap();
    assert_eq!(harness.recording.lock().initializations, 2);
    assert_eq!(harness.recording.lock().seeks, [command]);
    assert!(harness.recording.lock().commands.is_empty());
    assert_eq!(calls.load(Ordering::Acquire), 0);
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
    dirty(MediaEvent::Changed);
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
        dirty(MediaEvent::Changed);
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
    dirty(MediaEvent::Changed);
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
    dirty(MediaEvent::Changed);
    for _ in 0..1_000 {
        dirty(MediaEvent::Changed);
        dirty(MediaEvent::SeekInvalidated);
    }
    release.send(()).unwrap();
    receiver.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(
        events.recv_timeout(WAIT).unwrap(),
        MediaEvent::SeekInvalidated
    );
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::Changed);
    assert_eq!(harness.recording.lock().refreshes, 1);
    // Invalidation is published before queued pressure Wakes are drained.
    // A bounded, test-only Subscribe ACK checkpoints the same owner FIFO.
    let (sender, checkpoint_events) = mpsc::channel();
    let checkpoint = Arc::new(Subscription {
        active: AtomicBool::new(true),
        callback: Arc::new(move |event| sender.send(event).unwrap()),
    });
    harness
        .service
        .queue
        .sender
        .send(Message::Subscribe(Arc::clone(&checkpoint)))
        .unwrap_or_else(|_| panic!("checkpoint admission failed"));
    assert_eq!(
        checkpoint_events.recv_timeout(WAIT).unwrap(),
        MediaEvent::WatchReady
    );
    checkpoint.active.store(false, Ordering::Release);
    read(&harness.service).unwrap();
    assert_eq!(harness.recording.lock().refreshes, 1);
    assert_eq!(
        harness
            .service
            .queue
            .state
            .subscriptions
            .load(Ordering::Acquire),
        1
    );
    assert!(events.try_recv().is_err());
    drop(guard);
    harness.retired.recv_timeout(WAIT).unwrap();
    dirty(MediaEvent::Changed);
    dirty(MediaEvent::SeekInvalidated);
    read(&harness.service).unwrap();
    assert_eq!(harness.recording.lock().refreshes, 1);
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
    late(MediaEvent::Changed);
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
    dirty(MediaEvent::Changed);
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
    dirty(MediaEvent::Changed);
    read(&harness.service).unwrap();
    assert_eq!(harness.recording.lock().refreshes, 1);
    assert!(events.try_recv().is_err());
    drop(guard);
    harness.retired.recv_timeout(WAIT).unwrap();
    finish(harness);
}

#[test]
fn read_time_watch_failure_retires_before_completion_and_next_read_recovers() {
    for fail_metadata in [false, true] {
        let harness = start(Recording::new());
        let (guard, events) = watch(&harness.service);
        assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::WatchReady);
        let (other_guard, other_events) = watch(&harness.service);
        assert_eq!(
            other_events.recv_timeout(WAIT).unwrap(),
            MediaEvent::WatchReady
        );
        let current = read(&harness.service).unwrap().current.unwrap();
        let old_dirty = harness.recording.lock().dirty.clone().unwrap();
        let watch_error = error(MediaErrorKind::WatchUnavailable);
        let metadata_error = error(MediaErrorKind::Unavailable);
        let mut unavailable = current.clone();
        unavailable.seek = Err(watch_error.clone());
        {
            let mut recording = harness.recording.lock();
            recording.read_watch_error = Some(watch_error.clone());
            if fail_metadata {
                recording.snapshot = Err(metadata_error.clone());
            }
        }
        let (sender, receiver) = mpsc::channel();
        harness
            .service
            .read(Box::new(move |result| {
                // Read completion must already observe the watch failure.
                let event = events.try_recv();
                sender.send((result, event, events)).unwrap();
            }))
            .unwrap();
        let (result, event, events) = receiver.recv_timeout(WAIT).unwrap();
        assert_eq!(
            event.unwrap(),
            MediaEvent::WatchUnavailable(watch_error.clone())
        );
        assert_eq!(
            other_events.recv_timeout(WAIT).unwrap(),
            MediaEvent::WatchUnavailable(watch_error.clone())
        );
        if fail_metadata {
            assert_eq!(result, Err(metadata_error));
        } else {
            // Title, timeline, artwork and transport capabilities survive.
            assert_eq!(result.unwrap().current, Some(unavailable.clone()));
        }
        let owner = harness.retired.recv_timeout(WAIT).unwrap();
        {
            let recording = harness.recording.lock();
            assert!(recording.dirty.is_none());
            assert_eq!(recording.starts, 1);
            assert_eq!(recording.stops, 1);
            assert!(recording.commands.is_empty());
            assert!(recording.seeks.is_empty());
        }

        // Late callbacks and requests cannot retry watch registration.
        old_dirty(MediaEvent::Changed);
        old_dirty(MediaEvent::SeekInvalidated);
        harness.recording.lock().snapshot = Ok(MediaSnapshot {
            current: Some(unavailable),
        });
        execute(&harness.service, current.key, MediaAction::Toggle).unwrap();
        let (sender, receiver) = mpsc::channel();
        harness
            .service
            .execute(
                MediaRequest::Seek(seek_command(&current)),
                Box::new(move |result| sender.send(result).unwrap()),
            )
            .unwrap();
        assert_eq!(receiver.recv_timeout(WAIT).unwrap(), Err(watch_error));
        assert_eq!(harness.recording.lock().starts, 1);
        assert_eq!(harness.recording.lock().refreshes, 0);
        assert!(events.try_recv().is_err());
        assert!(other_events.try_recv().is_err());

        let mut recovered = current.clone();
        recovered.seek.as_mut().unwrap().revision = MediaObservationRevision::issue().unwrap();
        harness.recording.lock().snapshot = Ok(MediaSnapshot {
            current: Some(recovered.clone()),
        });
        assert_eq!(
            read(&harness.service).unwrap().current,
            Some(recovered.clone())
        );
        assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::WatchReady);
        assert_eq!(
            other_events.recv_timeout(WAIT).unwrap(),
            MediaEvent::WatchReady
        );
        assert_eq!(harness.recording.lock().starts, 2);
        let command = seek_command(&recovered);
        let (sender, receiver) = mpsc::channel();
        harness
            .service
            .execute(
                MediaRequest::Seek(command),
                Box::new(move |result| sender.send(result).unwrap()),
            )
            .unwrap();
        receiver.recv_timeout(WAIT).unwrap().unwrap();

        // A later read cannot re-report a drained failure or replay commands.
        old_dirty(MediaEvent::Changed);
        old_dirty(MediaEvent::SeekInvalidated);
        read(&harness.service).unwrap();
        assert!(events.try_recv().is_err());
        assert!(other_events.try_recv().is_err());
        {
            let recording = harness.recording.lock();
            assert_eq!(recording.initializations, 1);
            assert_eq!(recording.starts, 2);
            assert_eq!(recording.stops, 1);
            assert_eq!(recording.refreshes, 0);
            assert_eq!(
                recording.commands,
                [MediaCommand {
                    expected_session: current.key,
                    action: MediaAction::Toggle,
                }]
            );
            assert_eq!(recording.seeks, [command]);
        }
        drop(guard);
        drop(other_guard);
        assert_eq!(harness.retired.recv_timeout(WAIT).unwrap(), owner);
        finish(harness);
    }
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
    dirty(MediaEvent::Changed);
    assert_eq!(events.recv_timeout(WAIT).unwrap(), MediaEvent::Changed);
    assert!(finished.try_recv().is_err());
    drop(guard);
    let owner = retired.recv_timeout(WAIT).unwrap();
    assert_eq!(finished.recv_timeout(WAIT).unwrap(), owner);
    assert_ne!(owner, std::thread::current().id());
    dirty(MediaEvent::Changed);
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
                }
                .into(),
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
