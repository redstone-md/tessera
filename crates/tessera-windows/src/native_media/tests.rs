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
    MediaAction, MediaCapabilities, MediaCommand, MediaError, MediaErrorKind, MediaPlayback,
    MediaSession, MediaSessionKey, MediaTimeline,
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
                artwork: None,
                artwork_notice: record.art_error.clone(),
            }),
        };
        if let Some(replacement) = record.snapshot_replacement.take() {
            record.replace(replacement);
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

fn command(key: MediaSessionKey, action: MediaAction) -> MediaCommand {
    MediaCommand {
        expected_session: key,
        action,
    }
}

fn dirty_counter() -> (Dirty, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let captured = counter.clone();
    (
        Arc::new(move || {
            captured.fetch_add(1, Ordering::Relaxed);
        }),
        counter,
    )
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
