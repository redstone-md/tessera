// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::{
    artwork::{DecodePlan, MAX_ENCODED_BYTES, copied_size, read_count},
    owner::{Calls, Event, Owner, nullable_result, playback},
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread::ThreadId,
};
use tessera_system::media::{
    MediaAction, MediaCapabilities, MediaCommand, MediaError, MediaErrorKind, MediaEvent,
    MediaPlayback, MediaRequest, MediaSeekCommand, MediaSession, MediaSessionKey, MediaTimeline,
};

type Dirty = Arc<dyn Fn() + Send + Sync>;

struct Record {
    thread: ThreadId,
    live: Vec<u64>,
    current: Result<Option<u64>, MediaError>,
    capabilities: MediaCapabilities,
    snapshot_error: Option<MediaError>,
    art_error: Option<MediaError>,
    timeline: Result<MediaTimeline, MediaError>,
    snapshot_replacement: Option<u64>,
    capability_replacement: Option<u64>,
    transport_result: Result<bool, MediaError>,
    seek_range: Result<(i64, i64), MediaError>,
    seek_result: Result<bool, MediaError>,
    seek_effects: Vec<(u64, i64)>,
    snapshot_callback: Option<Event>,
    range_callback: Option<Event>,
    range_replacement: Option<u64>,
    effects: Vec<(u64, MediaAction)>,
    registrations: Vec<(usize, Event, Option<u64>, Dirty)>,
    register_attempts: usize,
    fail_registration: Option<usize>,
    removed: Vec<usize>,
    log: Vec<&'static str>,
}

impl Record {
    fn new() -> Self {
        Self {
            thread: std::thread::current().id(),
            live: vec![1],
            current: Ok(Some(1)),
            capabilities: MediaCapabilities {
                previous: true,
                toggle: true,
                next: true,
            },
            snapshot_error: None,
            art_error: None,
            timeline: Ok(MediaTimeline {
                start_ticks: 0,
                end_ticks: 900_000_000,
                position_ticks: 300_000_000,
                min_seek_ticks: 0,
                max_seek_ticks: 900_000_000,
                last_updated_utc_ticks: Some(133_000_000_000_000_000),
            }),
            snapshot_replacement: None,
            capability_replacement: None,
            transport_result: Ok(true),
            seek_range: Ok((0, 900_000_000)),
            seek_result: Ok(true),
            seek_effects: Vec::new(),
            snapshot_callback: None,
            range_callback: None,
            range_replacement: None,
            effects: Vec::new(),
            registrations: Vec::new(),
            register_attempts: 0,
            fail_registration: None,
            removed: Vec::new(),
            log: Vec::new(),
        }
    }

    fn replace(&mut self, session: u64) {
        self.current = Ok(Some(session));
        self.live = vec![session];
    }

    fn assert_owner(&self) {
        assert_eq!(self.thread, std::thread::current().id());
    }
}

struct RecordingCalls(Rc<RefCell<Record>>);
impl Calls for RecordingCalls {
    type Session = u64;
    type Token = usize;

    fn sessions(&mut self) -> Result<Vec<u64>, MediaError> {
        let mut record = self.0.borrow_mut();
        record.assert_owner();
        record.log.push("sessions");
        Ok(record.live.clone())
    }

    fn current(&mut self) -> Result<Option<u64>, MediaError> {
        let mut record = self.0.borrow_mut();
        record.assert_owner();
        record.log.push("current");
        record.current.clone()
    }

    fn snapshot(&mut self, _: &u64, key: MediaSessionKey) -> Result<MediaSession, MediaError> {
        let mut record = self.0.borrow_mut();
        record.assert_owner();
        record.log.push("snapshot");
        let result = match &record.snapshot_error {
            Some(error) => Err(error.clone()),
            None => Ok(MediaSession {
                key,
                // Every incarnation deliberately has the identical AUMID.
                source_app_id: "same.player".into(),
                title: "Recorded title".into(),
                author: "Recorded artist".into(),
                playback: MediaPlayback::Playing,
                capabilities: record.capabilities,
                timeline: record.timeline.clone(),
                seek: Err(MediaError::new(
                    MediaErrorKind::CommandUnavailable,
                    "Unobserved",
                )),
                artwork: None,
                artwork_notice: record.art_error.clone(),
            }),
        };
        if let Some(replacement) = record.snapshot_replacement.take() {
            record.replace(replacement);
        }
        if let Some(event) = record.snapshot_callback.take() {
            fire(&record, event);
        }
        result
    }

    fn capabilities(&mut self, _: &u64) -> Result<MediaCapabilities, MediaError> {
        let mut record = self.0.borrow_mut();
        record.assert_owner();
        record.log.push("capabilities");
        let capabilities = record.capabilities;
        if let Some(replacement) = record.capability_replacement.take() {
            record.replace(replacement);
        }
        Ok(capabilities)
    }

    fn transport(&mut self, session: &u64, action: MediaAction) -> Result<bool, MediaError> {
        let mut record = self.0.borrow_mut();
        record.assert_owner();
        record.log.push("transport");
        record.effects.push((*session, action));
        record.transport_result.clone()
    }

    fn seek_range(&mut self, _: &u64) -> Result<(i64, i64), MediaError> {
        let mut record = self.0.borrow_mut();
        record.assert_owner();
        record.log.push("seek-range");
        let result = record.seek_range.clone();
        if let Some(event) = record.range_callback.take() {
            fire(&record, event);
        }
        if let Some(replacement) = record.range_replacement.take() {
            record.replace(replacement);
        }
        result
    }

    fn seek(&mut self, session: &u64, position_ticks: i64) -> Result<bool, MediaError> {
        let mut record = self.0.borrow_mut();
        record.assert_owner();
        record.log.push("seek");
        record.seek_effects.push((*session, position_ticks));
        record.seek_result.clone()
    }

    fn register(
        &mut self,
        event: Event,
        session: Option<&u64>,
        callback: Dirty,
    ) -> Result<usize, MediaError> {
        let mut record = self.0.borrow_mut();
        record.assert_owner();
        record.log.push("register");
        record.register_attempts += 1;
        let token = record.register_attempts;
        if record.fail_registration == Some(token) {
            return Err(MediaError::with_hresult(
                MediaErrorKind::Other,
                "Recorded registration failure",
                -99,
            ));
        }
        record
            .registrations
            .push((token, event, session.copied(), callback));
        Ok(token)
    }

    fn remove(&mut self, token: usize) {
        let mut record = self.0.borrow_mut();
        record.assert_owner();
        record.log.push("remove");
        record.removed.push(token);
    }
}

impl Drop for RecordingCalls {
    fn drop(&mut self) {
        let mut record = self.0.borrow_mut();
        record.assert_owner();
        record.log.push("drop-calls");
    }
}

fn fixture() -> (Owner<RecordingCalls>, Rc<RefCell<Record>>) {
    let record = Rc::new(RefCell::new(Record::new()));
    (Owner::new(RecordingCalls(record.clone())), record)
}

fn displayed(owner: &mut Owner<RecordingCalls>) -> MediaSession {
    owner.read().unwrap().current.unwrap()
}

fn command(key: MediaSessionKey, action: MediaAction) -> MediaRequest {
    MediaRequest::Transport(MediaCommand {
        expected_session: key,
        action,
    })
}

fn dirty_counter() -> (Arc<dyn Fn(MediaEvent) + Send + Sync>, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let captured = counter.clone();
    (
        Arc::new(move |_| {
            captured.fetch_add(1, Ordering::Relaxed);
        }),
        counter,
    )
}

fn fire(record: &Record, event: Event) {
    let callback = record
        .registrations
        .iter()
        .rev()
        .find(|(_, registered, _, _)| *registered == event)
        .unwrap()
        .3
        .clone();
    callback();
}

fn watched_fixture() -> (Owner<RecordingCalls>, Rc<RefCell<Record>>) {
    let (mut owner, record) = fixture();
    owner.start_watch(dirty_counter().0).unwrap();
    (owner, record)
}

fn seek_command(session: &MediaSession, position_ticks: i64) -> MediaRequest {
    let observation = session.seek.as_ref().unwrap();
    MediaRequest::Seek(MediaSeekCommand {
        expected_session: session.key,
        expected_revision: observation.revision,
        observed_min_ticks: observation.min_ticks,
        observed_max_ticks: observation.max_ticks,
        position_ticks,
    })
}

#[test]
fn confirmed_no_current_is_not_unavailable_or_pointer_failure() {
    let (mut owner, record) = fixture();
    record.borrow_mut().current = Ok(None);
    assert!(owner.read().unwrap().current.is_none());
    let failure = MediaError::with_hresult(
        MediaErrorKind::Unavailable,
        "Recorded current query failure",
        -42,
    );
    record.borrow_mut().current = Err(failure.clone());
    assert_eq!(owner.read().unwrap_err(), failure);
    assert!(!nullable_result(0, false, "current").unwrap());
    assert!(nullable_result(1, true, "current").unwrap());
    let pointer_failure = 0x8000_4003_u32 as i32;
    assert_eq!(
        nullable_result(pointer_failure, false, "current")
            .unwrap_err()
            .hresult,
        Some(pointer_failure)
    );
}

#[test]
fn key_is_stable_for_live_identity_not_source_and_rotates_on_retirement() {
    let (mut owner, record) = fixture();
    let first = displayed(&mut owner);
    assert_eq!(displayed(&mut owner).key, first.key);
    record.borrow_mut().replace(2);
    let second = displayed(&mut owner);
    assert_eq!(first.source_app_id, second.source_app_id);
    assert_ne!(first.key, second.key);
    // Reused recorded pointer after observed disconnect must not reuse a key.
    record.borrow_mut().current = Ok(None);
    record.borrow_mut().live.clear();
    assert!(owner.read().unwrap().current.is_none());
    record.borrow_mut().replace(1);
    assert_ne!(displayed(&mut owner).key, first.key);
}

#[test]
fn changing_current_does_not_rotate_another_still_live_session() {
    let (mut owner, record) = fixture();
    record.borrow_mut().live = vec![1, 2];
    let first = displayed(&mut owner).key;
    record.borrow_mut().current = Ok(Some(2));
    assert_ne!(displayed(&mut owner).key, first);
    record.borrow_mut().current = Ok(Some(1));
    assert_eq!(displayed(&mut owner).key, first);
}

#[test]
fn same_aumid_replacement_and_foreign_keys_have_zero_effects() {
    let (mut owner, record) = fixture();
    let old = displayed(&mut owner).key;
    record.borrow_mut().replace(2);
    assert_eq!(
        owner
            .execute(command(old, MediaAction::Toggle))
            .unwrap_err()
            .kind,
        MediaErrorKind::SessionChanged
    );
    assert_eq!(
        owner
            .execute(command(
                MediaSessionKey::issue().unwrap(),
                MediaAction::Next
            ))
            .unwrap_err()
            .kind,
        MediaErrorKind::SessionChanged
    );
    assert!(record.borrow().effects.is_empty());
}

#[test]
fn replacement_during_capability_read_has_zero_effects() {
    let (mut owner, record) = fixture();
    let key = displayed(&mut owner).key;
    record.borrow_mut().capability_replacement = Some(2);
    assert_eq!(
        owner
            .execute(command(key, MediaAction::Toggle))
            .unwrap_err()
            .kind,
        MediaErrorKind::SessionChanged
    );
    assert!(record.borrow().effects.is_empty());
}

#[test]
fn disabled_capability_cannot_dispatch_and_capabilities_are_reread() {
    let (mut owner, record) = fixture();
    let key = displayed(&mut owner).key;
    record.borrow_mut().capabilities = MediaCapabilities::default();
    for action in [
        MediaAction::Previous,
        MediaAction::Toggle,
        MediaAction::Next,
    ] {
        assert_eq!(
            owner.execute(command(key, action)).unwrap_err().kind,
            MediaErrorKind::CommandUnavailable
        );
    }
    assert!(record.borrow().effects.is_empty());
}

#[test]
fn accepted_false_and_hresult_transport_results_are_distinct_never_retried() {
    for action in [
        MediaAction::Previous,
        MediaAction::Toggle,
        MediaAction::Next,
    ] {
        for outcome in [
            Ok(true),
            Ok(false),
            Err(MediaError::with_hresult(
                MediaErrorKind::Rejected,
                "Recorded Try failure",
                -77,
            )),
        ] {
            let (mut owner, record) = fixture();
            let key = displayed(&mut owner).key;
            record.borrow_mut().log.clear();
            record.borrow_mut().transport_result = outcome.clone();
            let result = owner.execute(command(key, action));
            match outcome {
                Ok(true) => assert_eq!(result, Ok(())),
                Ok(false) => {
                    let error = result.unwrap_err();
                    assert_eq!(error.kind, MediaErrorKind::Rejected);
                    assert_eq!(error.hresult, None);
                }
                Err(error) => assert_eq!(result.unwrap_err(), error),
            }
            assert_eq!(record.borrow().effects, vec![(1, action)]);
            assert_eq!(
                record.borrow().log,
                [
                    "sessions",
                    "current",
                    "capabilities",
                    "sessions",
                    "current",
                    "transport"
                ]
            );
        }
    }
}

#[test]
fn stale_metadata_or_decode_never_publishes_into_replacement() {
    let (mut owner, record) = fixture();
    record.borrow_mut().snapshot_replacement = Some(2);
    assert_eq!(
        owner.read().unwrap_err().kind,
        MediaErrorKind::SessionChanged
    );
    assert_eq!(displayed(&mut owner).source_app_id, "same.player");
    // Even a failed metadata result is rechecked for session replacement.
    record.borrow_mut().snapshot_error =
        Some(MediaError::new(MediaErrorKind::Other, "metadata failed"));
    record.borrow_mut().snapshot_replacement = Some(3);
    assert_eq!(
        owner.read().unwrap_err().kind,
        MediaErrorKind::SessionChanged
    );
}

#[test]
fn artwork_failure_preserves_real_metadata_and_controls() {
    let (mut owner, record) = fixture();
    let notice = MediaError::with_hresult(MediaErrorKind::Other, "Recorded unsupported codec", -88);
    record.borrow_mut().art_error = Some(notice.clone());
    let session = displayed(&mut owner);
    assert_eq!(session.title, "Recorded title");
    assert_eq!(session.author, "Recorded artist");
    assert_eq!(session.playback, MediaPlayback::Playing);
    assert!(session.capabilities.toggle);
    assert!(session.artwork.is_none());
    assert_eq!(session.artwork_notice, Some(notice));
}

#[test]
fn authoritative_playback_states_are_not_collapsed() {
    let states = [
        MediaPlayback::Closed,
        MediaPlayback::Opened,
        MediaPlayback::Changing,
        MediaPlayback::Stopped,
        MediaPlayback::Playing,
        MediaPlayback::Paused,
    ];
    for (value, expected) in states.into_iter().enumerate() {
        assert_eq!(playback(value as i32).unwrap(), expected);
    }
    assert!(playback(-1).is_err());
    assert!(playback(6).is_err());
}

#[test]
fn watch_registers_both_manager_and_current_events_and_callbacks_only_dirty() {
    let (mut owner, record) = fixture();
    let (dirty, count) = dirty_counter();
    owner.start_watch(dirty).unwrap();
    let events = record
        .borrow()
        .registrations
        .iter()
        .map(|(_, event, session, _)| (*event, *session))
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        [
            (Event::Sessions, None),
            (Event::Current, None),
            (Event::Properties, Some(1)),
            (Event::Playback, Some(1)),
            (Event::Timeline, Some(1))
        ]
    );
    let callbacks = record
        .borrow()
        .registrations
        .iter()
        .map(|(_, _, _, callback)| callback.clone())
        .collect::<Vec<_>>();
    let log_before = record.borrow().log.clone();
    for _ in 0..100 {
        for callback in &callbacks {
            callback();
        }
    }
    assert_eq!(count.load(Ordering::Relaxed), 500);
    assert_eq!(record.borrow().log, log_before);
    assert!(record.borrow().effects.is_empty());
    // Coalescing belongs to the actor; this adapter never queues read/effects.
}

#[test]
fn rebind_retires_old_callbacks_before_registering_replacement() {
    let (mut owner, record) = fixture();
    let (dirty, count) = dirty_counter();
    owner.start_watch(dirty).unwrap();
    let old_properties = record.borrow().registrations[2].3.clone();
    let old_timeline = record.borrow().registrations[4].3.clone();
    let manager = record.borrow().registrations[0].3.clone();
    record.borrow_mut().replace(2);
    owner.refresh_watch().unwrap();
    assert_eq!(record.borrow().removed, [5, 4, 3]);
    assert_eq!(record.borrow().registrations[5].1, Event::Properties);
    assert_eq!(record.borrow().registrations[5].2, Some(2));
    assert_eq!(record.borrow().registrations[7].1, Event::Timeline);
    assert_eq!(record.borrow().registrations[7].2, Some(2));
    assert_eq!(count.swap(0, Ordering::Relaxed), 1);
    old_properties();
    old_timeline();
    assert_eq!(count.load(Ordering::Relaxed), 0);
    manager();
    record.borrow().registrations[5].3.clone()();
    record.borrow().registrations[7].3.clone()();
    assert_eq!(count.load(Ordering::Relaxed), 3);
    let attempts = record.borrow().register_attempts;
    owner.refresh_watch().unwrap();
    assert_eq!(record.borrow().register_attempts, attempts);
}

#[test]
fn partial_registration_rolls_back_every_acquired_token_and_late_callbacks() {
    for failed in 1..=5 {
        let (mut owner, record) = fixture();
        let (dirty, count) = dirty_counter();
        record.borrow_mut().fail_registration = Some(failed);
        let error = owner.start_watch(dirty).unwrap_err();
        assert_eq!(error.kind, MediaErrorKind::WatchUnavailable);
        assert_eq!(error.hresult, Some(-99));
        let mut removed = record.borrow().removed.clone();
        removed.sort_unstable();
        assert_eq!(removed, (1..failed).collect::<Vec<_>>());
        for (_, _, _, callback) in &record.borrow().registrations {
            callback();
        }
        assert_eq!(count.load(Ordering::Relaxed), 0);
        let before = record.borrow().removed.clone();
        owner.stop_watch();
        assert_eq!(record.borrow().removed, before);
    }
}

#[test]
fn rebind_failure_and_drop_cleanup_are_idempotent_and_owner_local() {
    for failed in 6..=8 {
        let (mut owner, record) = fixture();
        let (dirty, count) = dirty_counter();
        owner.start_watch(dirty).unwrap();
        record.borrow_mut().replace(2);
        record.borrow_mut().fail_registration = Some(failed);
        assert_eq!(
            owner.refresh_watch().unwrap_err().kind,
            MediaErrorKind::WatchUnavailable
        );
        let mut removed = record.borrow().removed.clone();
        removed.sort_unstable();
        assert_eq!(removed, (1..failed).collect::<Vec<_>>());
        assert_eq!(count.swap(0, Ordering::Relaxed), 1);
        for (_, _, _, callback) in &record.borrow().registrations {
            callback();
        }
        assert_eq!(count.load(Ordering::Relaxed), 0);
        drop(owner);
        assert_eq!(record.borrow().log.last(), Some(&"drop-calls"));
        assert_eq!(record.borrow().removed.len(), failed - 1);
    }
}

#[test]
fn read_time_rebind_failure_preserves_facts_and_recovers_only_with_explicit_watch() {
    let (mut owner, record) = fixture();
    let (dirty, count) = dirty_counter();
    owner.start_watch(dirty.clone()).unwrap();
    let old = displayed(&mut owner);
    let old_seek = seek_command(&old, 10);
    assert!(owner.take_watch_failure().is_none());
    let timeline = MediaTimeline {
        start_ticks: -50,
        end_ticks: 500,
        position_ticks: 100,
        min_seek_ticks: -50,
        max_seek_ticks: 500,
        last_updated_utc_ticks: None,
    };
    let capabilities = MediaCapabilities {
        previous: false,
        toggle: true,
        next: false,
    };
    {
        let mut record = record.borrow_mut();
        // No manager callback: the explicit read must discover B itself.
        record.replace(2);
        record.timeline = Ok(timeline);
        record.seek_range = Ok((-50, 500));
        record.capabilities = capabilities;
        // B acquires properties/playback tokens before timeline registration fails.
        record.fail_registration = Some(8);
    }
    assert_eq!(count.load(Ordering::Relaxed), 0);
    let failed = displayed(&mut owner);
    assert_ne!(failed.key, old.key);
    assert_eq!(failed.source_app_id, "same.player");
    assert_eq!(failed.title, "Recorded title");
    assert_eq!(failed.author, "Recorded artist");
    assert_eq!(failed.playback, MediaPlayback::Playing);
    assert_eq!(failed.timeline, Ok(timeline));
    assert_eq!(failed.capabilities, capabilities);
    let failure = failed.seek.as_ref().unwrap_err().clone();
    assert_eq!(failure.kind, MediaErrorKind::WatchUnavailable);
    assert_eq!(failure.hresult, Some(-99));
    assert_eq!(owner.take_watch_failure(), Some(failure));
    assert!(owner.take_watch_failure().is_none());
    assert_eq!(record.borrow().removed, [5, 4, 3, 7, 6, 2, 1]);
    assert_eq!(record.borrow().register_attempts, 8);
    assert_eq!(count.swap(0, Ordering::Relaxed), 1);
    for (_, _, _, callback) in &record.borrow().registrations {
        callback();
    }
    assert_eq!(count.load(Ordering::Relaxed), 0);
    assert_eq!(
        owner.execute(old_seek).unwrap_err().kind,
        MediaErrorKind::SessionChanged
    );
    assert!(record.borrow().effects.is_empty());
    assert!(record.borrow().seek_effects.is_empty());

    record.borrow_mut().fail_registration = None;
    owner.start_watch(dirty).unwrap();
    assert_eq!(record.borrow().register_attempts, 13);
    assert_eq!(record.borrow().removed, [5, 4, 3, 7, 6, 2, 1]);
    let recovered = displayed(&mut owner);
    assert_eq!(recovered.key, failed.key);
    assert_eq!(recovered.timeline, Ok(timeline));
    assert_eq!(recovered.capabilities, capabilities);
    let authority = recovered.seek.as_ref().unwrap();
    assert_eq!((authority.min_ticks, authority.max_ticks), (-50, 500));
    assert!(owner.take_watch_failure().is_none());
    assert!(record.borrow().effects.is_empty());
    assert!(record.borrow().seek_effects.is_empty());
    fire(&record.borrow(), Event::Timeline);
    assert_eq!(count.load(Ordering::Relaxed), 1);
    owner.execute(seek_command(&recovered, 100)).unwrap();
    assert_eq!(record.borrow().seek_effects, [(2, 100)]);
    assert!(record.borrow().effects.is_empty());
}

#[test]
fn read_time_rebind_failure_survives_snapshot_error_and_restart_clears_latch() {
    for drain_before_restart in [true, false] {
        let (mut owner, record) = fixture();
        let (dirty, _) = dirty_counter();
        owner.start_watch(dirty.clone()).unwrap();
        let old = displayed(&mut owner);
        let snapshot_error = MediaError::with_hresult(
            MediaErrorKind::Unavailable,
            "Recorded metadata failure",
            -77,
        );
        {
            let mut record = record.borrow_mut();
            record.replace(2);
            record.fail_registration = Some(8);
            record.snapshot_error = Some(snapshot_error.clone());
        }
        assert_eq!(owner.read().err(), Some(snapshot_error));
        assert_eq!(record.borrow().removed, [5, 4, 3, 7, 6, 2, 1]);
        assert_eq!(record.borrow().register_attempts, 8);
        if drain_before_restart {
            let failure = owner.take_watch_failure().unwrap();
            assert_eq!(failure.kind, MediaErrorKind::WatchUnavailable);
            assert_eq!(failure.hresult, Some(-99));
            assert_eq!(failure.message, "Recorded registration failure");
            assert!(owner.take_watch_failure().is_none());
        }
        {
            let mut record = record.borrow_mut();
            record.fail_registration = None;
            record.snapshot_error = None;
        }
        owner.start_watch(dirty).unwrap();
        // Restart clears even an undrained failure before exposing new authority.
        assert!(owner.take_watch_failure().is_none());
        let recovered = displayed(&mut owner);
        assert_ne!(recovered.key, old.key);
        assert!(recovered.seek.is_ok());
        assert!(owner.take_watch_failure().is_none());
        assert_eq!(record.borrow().register_attempts, 13);
        assert!(record.borrow().effects.is_empty());
        assert!(record.borrow().seek_effects.is_empty());
    }
}

#[test]
fn stopping_and_dropping_watch_suppresses_all_late_delivery() {
    let (mut owner, record) = fixture();
    let (dirty, count) = dirty_counter();
    owner.start_watch(dirty).unwrap();
    let late = record
        .borrow()
        .registrations
        .iter()
        .map(|(_, _, _, callback)| callback.clone())
        .collect::<Vec<_>>();
    drop(owner);
    assert_eq!(record.borrow().removed, [5, 4, 3, 2, 1]);
    for callback in late {
        callback();
    }
    assert_eq!(count.load(Ordering::Relaxed), 0);
}

#[test]
fn no_current_session_watch_stays_truthful_and_binds_only_when_current_exists() {
    let (mut owner, record) = fixture();
    record.borrow_mut().current = Ok(None);
    let (dirty, _) = dirty_counter();
    owner.start_watch(dirty).unwrap();
    assert_eq!(record.borrow().register_attempts, 2);
    record.borrow_mut().replace(2);
    owner.refresh_watch().unwrap();
    assert_eq!(record.borrow().registrations[2].2, Some(2));
    assert_eq!(record.borrow().registrations[3].2, Some(2));
    assert_eq!(record.borrow().registrations[4].1, Event::Timeline);
    assert_eq!(record.borrow().registrations[4].2, Some(2));
    record.borrow_mut().current = Ok(None);
    owner.refresh_watch().unwrap();
    assert_eq!(record.borrow().removed, [5, 4, 3]);
    assert_eq!(record.borrow().register_attempts, 5);
}

#[test]
fn artwork_stream_and_source_budget_reject_empty_oversized_growth_and_overflow() {
    assert!(read_count(0).is_err());
    assert!(read_count(MAX_ENCODED_BYTES + 1).is_err());
    assert!(read_count(u64::MAX).is_err());
    assert_eq!(
        read_count(MAX_ENCODED_BYTES).unwrap(),
        MAX_ENCODED_BYTES as u32 + 1
    );
    assert!(copied_size(4, 4, 4).is_ok());
    assert!(copied_size(4, 3, 4).is_err());
    assert!(copied_size(4, 5, 4).is_err());
    assert!(copied_size(4, 4, 5).is_err());
    for (width, height) in [
        (0, 1),
        (1, 0),
        (u32::MAX, u32::MAX),
        (16_385, 1),
        (8_192, 8_192),
    ] {
        assert!(DecodePlan::new(width, height).is_err());
    }
}

#[test]
fn artwork_decode_plan_preserves_aspect_without_upscaling_and_validates_output() {
    assert_eq!(
        DecodePlan::new(512, 256).unwrap(),
        DecodePlan {
            width: 128,
            height: 64,
            bytes: 32_768
        }
    );
    assert_eq!(DecodePlan::new(1, 4096).unwrap().width, 1);
    let plan = DecodePlan::new(2, 1).unwrap();
    assert_eq!((plan.width, plan.height, plan.bytes), (2, 1, 8));
    // Real supplied premultiplied RGBA fixture, not a default album image.
    let rgba = [128, 64, 0, 128, 9, 10, 11, 255];
    let art = plan.accept(&rgba).unwrap();
    assert_eq!(art.rgba(), &rgba);
    assert_eq!((art.width(), art.height()), (2, 1));
    assert!(plan.accept(&[]).is_err());
    assert!(plan.accept(&rgba[..4]).is_err());
    assert!(plan.accept(&[255, 0, 0, 128, 9, 10, 11, 255]).is_err());
}

#[test]
fn timeline_failure_preserves_metadata_artwork_and_all_three_transport_authorities() {
    let (mut owner, record) = fixture();
    let notice = MediaError::with_hresult(
        MediaErrorKind::Unavailable,
        "Recorded timeline failure",
        -77,
    );
    let artwork_notice = MediaError::new(MediaErrorKind::Other, "Recorded thumbnail failure");
    record.borrow_mut().timeline = Err(notice.clone());
    record.borrow_mut().art_error = Some(artwork_notice.clone());
    let session = displayed(&mut owner);
    assert_eq!(session.timeline, Err(notice));
    assert_eq!(session.title, "Recorded title");
    assert_eq!(session.author, "Recorded artist");
    assert_eq!(session.playback, MediaPlayback::Playing);
    assert_eq!(session.capabilities, record.borrow().capabilities);
    assert_eq!(session.artwork_notice, Some(artwork_notice));
    assert!(session.artwork.is_none());
    for action in [
        MediaAction::Previous,
        MediaAction::Toggle,
        MediaAction::Next,
    ] {
        owner.execute(command(session.key, action)).unwrap();
    }
    assert_eq!(
        record.borrow().effects,
        [
            (1, MediaAction::Previous),
            (1, MediaAction::Toggle),
            (1, MediaAction::Next)
        ]
    );
}

#[test]
fn replacement_after_independent_timeline_failure_is_still_revalidated() {
    let (mut owner, record) = fixture();
    let old_key = displayed(&mut owner).key;
    record.borrow_mut().timeline = Err(MediaError::new(
        MediaErrorKind::Unavailable,
        "Recorded timeline failure",
    ));
    record.borrow_mut().snapshot_replacement = Some(2);
    assert_eq!(
        owner.read().unwrap_err().kind,
        MediaErrorKind::SessionChanged
    );
    assert_ne!(displayed(&mut owner).key, old_key);
    assert!(record.borrow().effects.is_empty());
}

#[test]
fn metadata_callbacks_revoke_same_key_seek_before_queue_refresh() {
    for event in [Event::Properties, Event::Current] {
        let (mut owner, record) = watched_fixture();
        let old = displayed(&mut owner);
        fire(&record.borrow(), event);
        assert_eq!(
            owner.execute(seek_command(&old, 10)).unwrap_err().kind,
            MediaErrorKind::SessionChanged
        );
        assert!(record.borrow().seek_effects.is_empty());
        let fresh = displayed(&mut owner);
        assert_eq!(fresh.key, old.key);
        assert_ne!(fresh.seek.unwrap().revision, old.seek.unwrap().revision);
    }
}

#[test]
fn timeline_and_playback_callbacks_retain_seek_revision() {
    let (mut owner, record) = watched_fixture();
    let old = displayed(&mut owner);
    for event in [Event::Timeline, Event::Playback] {
        fire(&record.borrow(), event);
        let fresh = displayed(&mut owner);
        assert_eq!(fresh.seek, old.seek);
        owner.execute(seek_command(&old, 10)).unwrap();
    }
    assert_eq!(record.borrow().seek_effects, [(1, 10), (1, 10)]);
}

#[test]
fn fresh_native_range_failures_independently_reject_without_seek_effects() {
    for (range, kind) in [
        (Ok((1, 900_000_000)), MediaErrorKind::SessionChanged),
        (Ok((0, 0)), MediaErrorKind::CommandUnavailable),
        (Ok((20, 10)), MediaErrorKind::CommandUnavailable),
        (
            Err(MediaError::new(
                MediaErrorKind::CommandUnavailable,
                "Disabled native seek",
            )),
            MediaErrorKind::CommandUnavailable,
        ),
        (
            Err(MediaError::with_hresult(
                MediaErrorKind::Unavailable,
                "Range failed",
                -88,
            )),
            MediaErrorKind::Unavailable,
        ),
    ] {
        let (mut owner, record) = watched_fixture();
        let old = displayed(&mut owner);
        record.borrow_mut().seek_range = range;
        assert_eq!(
            owner.execute(seek_command(&old, 10)).unwrap_err().kind,
            kind
        );
        assert!(record.borrow().seek_effects.is_empty());
        assert!(record.borrow().effects.is_empty());
    }
}

#[test]
fn targets_outside_native_range_are_rejected_without_dispatch() {
    let (mut owner, record) = watched_fixture();
    let old = displayed(&mut owner);
    for target in [-1, 900_000_001] {
        assert_eq!(
            owner.execute(seek_command(&old, target)).unwrap_err().kind,
            MediaErrorKind::Rejected
        );
    }
    assert!(record.borrow().seek_effects.is_empty());
}

#[test]
fn native_seek_false_and_hresult_are_not_retried_or_read_back() {
    for outcome in [
        Ok(false),
        Err(MediaError::with_hresult(
            MediaErrorKind::Rejected,
            "Native seek failure",
            -89,
        )),
    ] {
        let (mut owner, record) = watched_fixture();
        let old = displayed(&mut owner);
        record.borrow_mut().seek_result = outcome.clone();
        record.borrow_mut().log.clear();
        let error = owner.execute(seek_command(&old, 10)).unwrap_err();
        match outcome {
            Ok(false) => {
                assert_eq!(error.kind, MediaErrorKind::Rejected);
                assert_eq!(error.hresult, None);
            }
            Err(expected) => assert_eq!(error, expected),
            Ok(true) => unreachable!(),
        }
        assert_eq!(record.borrow().seek_effects, [(1, 10)]);
        assert!(!record.borrow().log.contains(&"snapshot"));
    }
}

#[test]
fn signed_exact_native_range_seek_requires_explicit_authoritative_readback() {
    let (mut owner, record) = watched_fixture();
    record.borrow_mut().seek_range = Ok((-90, 50));
    let old = displayed(&mut owner);
    assert_eq!(
        (
            old.seek.as_ref().unwrap().min_ticks,
            old.seek.as_ref().unwrap().max_ticks
        ),
        (-90, 50)
    );
    record.borrow_mut().log.clear();
    for target in [-90, -25, 50] {
        owner.execute(seek_command(&old, target)).unwrap();
    }
    assert_eq!(record.borrow().seek_effects, [(1, -90), (1, -25), (1, 50)]);
    assert!(!record.borrow().log.contains(&"snapshot"));
    assert_eq!(old.timeline.as_ref().unwrap().position_ticks, 300_000_000);
    record
        .borrow_mut()
        .timeline
        .as_mut()
        .unwrap()
        .position_ticks = -25;
    let readback = displayed(&mut owner);
    assert_eq!(readback.timeline.unwrap().position_ticks, -25);
    assert_eq!(readback.seek, old.seek);
}

#[test]
fn metadata_callback_during_snapshot_rejects_stale_metadata_revision() {
    for event in [Event::Properties, Event::Current] {
        let (mut owner, record) = watched_fixture();
        let old = displayed(&mut owner);
        record.borrow_mut().snapshot_callback = Some(event);
        assert_eq!(
            owner.read().unwrap_err().kind,
            MediaErrorKind::SessionChanged
        );
        assert_eq!(
            owner.execute(seek_command(&old, 10)).unwrap_err().kind,
            MediaErrorKind::SessionChanged
        );
        assert!(record.borrow().seek_effects.is_empty());
        assert_ne!(
            displayed(&mut owner).seek.unwrap().revision,
            old.seek.unwrap().revision
        );
    }
}

#[test]
fn final_current_change_or_range_callback_rejects_before_native_seek() {
    for event in [None, Some(Event::Properties), Some(Event::Current)] {
        let (mut owner, record) = watched_fixture();
        let old = displayed(&mut owner);
        if let Some(event) = event {
            record.borrow_mut().range_callback = Some(event);
        } else {
            record.borrow_mut().range_replacement = Some(2);
        }
        assert_eq!(
            owner.execute(seek_command(&old, 10)).unwrap_err().kind,
            MediaErrorKind::SessionChanged
        );
        assert!(record.borrow().seek_effects.is_empty());
    }
}

#[test]
fn retired_metadata_callback_generation_cannot_revoke_rebound_seek() {
    let (mut owner, record) = watched_fixture();
    let late = record.borrow().registrations[2].3.clone();
    record.borrow_mut().replace(2);
    owner.refresh_watch().unwrap();
    let rebound = displayed(&mut owner);
    late();
    assert_eq!(displayed(&mut owner).seek, rebound.seek);
    owner.execute(seek_command(&rebound, 10)).unwrap();
    assert_eq!(record.borrow().seek_effects, [(2, 10)]);
    drop(owner);
    late();
    assert_eq!(record.borrow().seek_effects, [(2, 10)]);
}

#[test]
fn exhausted_seek_revision_preserves_other_session_facts_and_transport() {
    let (mut owner, record) = watched_fixture();
    let old = displayed(&mut owner);
    owner.exhaust_observation_revisions();
    let fresh = displayed(&mut owner);
    assert_eq!(fresh.key, old.key);
    assert_eq!(fresh.title, old.title);
    assert_eq!(fresh.author, old.author);
    assert_eq!(fresh.playback, old.playback);
    assert_eq!(fresh.capabilities, old.capabilities);
    assert_eq!(fresh.timeline, old.timeline);
    assert_eq!(fresh.artwork, old.artwork);
    assert_eq!(fresh.artwork_notice, old.artwork_notice);
    assert_eq!(fresh.seek.unwrap_err().kind, MediaErrorKind::Other);
    assert_eq!(
        owner.execute(seek_command(&old, 10)).unwrap_err().kind,
        MediaErrorKind::Other
    );
    assert!(record.borrow().seek_effects.is_empty());
    owner
        .execute(command(fresh.key, MediaAction::Toggle))
        .unwrap();
    assert_eq!(record.borrow().effects, [(1, MediaAction::Toggle)]);
}

#[test]
fn seek_observation_requires_watch_and_independent_native_range() {
    let (mut owner, record) = fixture();
    let unwatched = displayed(&mut owner);
    assert_eq!(
        unwatched.seek.unwrap_err().kind,
        MediaErrorKind::CommandUnavailable
    );
    owner.start_watch(dirty_counter().0).unwrap();
    record.borrow_mut().timeline = Err(MediaError::new(
        MediaErrorKind::Unavailable,
        "Timeline unavailable",
    ));
    record.borrow_mut().seek_range = Ok((-50, 50));
    let session = displayed(&mut owner);
    assert!(session.timeline.is_err());
    assert_eq!(
        (
            session.seek.as_ref().unwrap().min_ticks,
            session.seek.as_ref().unwrap().max_ticks
        ),
        (-50, 50)
    );
    owner.execute(seek_command(&session, -10)).unwrap();
    assert_eq!(record.borrow().seek_effects, [(1, -10)]);
}

#[test]
fn metadata_callback_during_snapshot_range_rejects_publication() {
    let (mut owner, record) = watched_fixture();
    let old = displayed(&mut owner);
    record.borrow_mut().range_callback = Some(Event::Properties);
    assert_eq!(
        owner.read().unwrap_err().kind,
        MediaErrorKind::SessionChanged
    );
    assert_eq!(
        owner.execute(seek_command(&old, 10)).unwrap_err().kind,
        MediaErrorKind::SessionChanged
    );
    assert!(record.borrow().seek_effects.is_empty());
}

#[test]
fn detached_session_aba_rebind_rejects_old_seek_without_manager_callback() {
    let (mut owner, record) = watched_fixture();
    record.borrow_mut().live = vec![1, 2];
    let old = displayed(&mut owner);
    record.borrow_mut().current = Ok(Some(2));
    owner.refresh_watch().unwrap();
    record.borrow_mut().current = Ok(Some(1));
    owner.refresh_watch().unwrap();
    assert_eq!(
        owner.execute(seek_command(&old, 10)).unwrap_err().kind,
        MediaErrorKind::SessionChanged
    );
    assert!(record.borrow().seek_effects.is_empty());
    let fresh = displayed(&mut owner);
    assert_eq!(fresh.key, old.key);
    let old_observation = old.seek.as_ref().unwrap();
    let fresh_observation = fresh.seek.as_ref().unwrap();
    assert_eq!(fresh_observation.min_ticks, old_observation.min_ticks);
    assert_eq!(fresh_observation.max_ticks, old_observation.max_ticks);
    assert_ne!(fresh_observation.revision, old_observation.revision);
    owner.execute(seek_command(&fresh, 10)).unwrap();
    assert_eq!(record.borrow().seek_effects, [(1, 10)]);
}
