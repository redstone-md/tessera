// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

fn seekable() -> MediaSession {
    let mut observed = session(MediaPlayback::Paused);
    observed.seek = Ok(MediaSeekObservation {
        revision: MediaObservationRevision::issue().unwrap(),
        min_ticks: 100,
        max_ticks: 1_100,
    });
    observed.timeline = Ok(MediaTimeline {
        start_ticks: 0,
        end_ticks: 2_000,
        position_ticks: 350,
        min_seek_ticks: 100,
        max_seek_ticks: 1_100,
        last_updated_utc_ticks: Some(i64::MAX),
    });
    observed
}

fn attach(fixture: &Fixture) -> (QuickSettings, PopupMediaToken) {
    let popup = QuickSettings::new().unwrap();
    popup.show().unwrap();
    let token = fixture.controller.attach_popup(&popup).unwrap();
    (popup, token)
}

fn recorded_seek(fixture: &Fixture, index: usize) -> MediaSeekCommand {
    match fixture.media.commands.lock()[index] {
        MediaRequest::Seek(command) => command,
        MediaRequest::Transport(_) => panic!("expected seek"),
    }
}

fn make_due(fixture: &Fixture) {
    fixture.controller.elapse_seek_throttle(SEEK_INTERVAL);
}

fn fire_trailing(fixture: &Fixture) {
    make_due(fixture);
    i_slint_backend_testing::mock_elapsed_time(SEEK_INTERVAL);
    slint::platform::update_timers_and_animations();
}

#[test]
fn seek_leads_then_keeps_one_latest_input_behind_success_readback() {
    let fixture = Fixture::new();
    let observed = seekable();
    fixture.enable(observed.clone());
    let (popup, token) = attach(&fixture);
    let scope = fixture.controller.capture_seek(token).unwrap();
    fixture.controller.seek_from_popup(&scope, 0.2);
    assert_eq!(
        recorded_seek(&fixture, 0),
        MediaSeekCommand {
            expected_session: observed.key,
            expected_revision: observed.seek.as_ref().unwrap().revision,
            observed_min_ticks: 100,
            observed_max_ticks: 1_100,
            position_ticks: 300,
        }
    );
    assert!(
        popup.get_seek_enabled(),
        "seek input is independent of the busy flight"
    );
    assert_eq!(popup.get_seek_progress(), 0.25, "no optimistic position");
    for input in 0..1_000 {
        fixture
            .controller
            .seek_from_popup(&scope, input as f32 / 1_000.0);
    }
    fixture.controller.seek_from_popup(&scope, 0.9);
    assert_eq!(fixture.media.commands.lock().len(), 1);
    assert_eq!(fixture.media.reads(), 1);
    assert_eq!(
        fixture
            .controller
            .state
            .borrow()
            .pending_seek
            .as_ref()
            .unwrap()
            .position_ticks,
        1_000
    );
    fixture.media.finish_command(Ok(()));
    fixture.drain();
    assert_eq!(
        fixture.media.reads(),
        2,
        "native success observes before trailing submission"
    );
    assert_eq!(fixture.media.commands.lock().len(), 1);
    assert!(fixture.view().stale, "raw Dock observation remains dirty");
    assert!(
        !popup.get_media_view().stale,
        "routine readback must not insert a notice that moves and retires the seek input"
    );
    assert!(popup.get_media_view().busy);
    assert!(!popup.get_media_view().toggle_enabled);
    assert!(fixture.controller.state.borrow().pending_seek.is_some());
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(observed),
    }));
    fixture.drain();
    assert!(fixture.controller.seek_timer.running());
    fixture
        .controller
        .elapse_seek_throttle(Duration::from_millis(199));
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(199));
    slint::platform::update_timers_and_animations();
    fixture.drain();
    assert_eq!(
        fixture.media.commands.lock().len(),
        1,
        "trailing input cannot issue before the 200ms acceptance deadline"
    );
    fixture
        .controller
        .elapse_seek_throttle(Duration::from_millis(1));
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
    slint::platform::update_timers_and_animations();
    fixture.drain();
    assert_eq!(
        fixture.media.commands.lock().len(),
        2,
        "timer sends without release"
    );
    assert_eq!(recorded_seek(&fixture, 1).position_ticks, 1_000);
    assert!(fixture.controller.state.borrow().pending_seek.is_none());
    assert_eq!(popup.get_seek_progress(), 0.25);
}

#[test]
fn seek_throttle_uses_elapsed_local_acceptance_and_latest_input_not_debounce() {
    let fixture = Fixture::new();
    fixture.enable(seekable());
    let (_popup, token) = attach(&fixture);
    let scope = fixture.controller.capture_seek(token).unwrap();
    let accepted = fixture.controller.seek_now();
    fixture.controller.state.borrow_mut().last_seek_accepted = Some(accepted);
    fixture.controller.state.borrow_mut().pending_seek = Some(PendingSeek {
        scope: scope.clone(),
        position_ticks: 450,
    });
    fixture
        .controller
        .drive_seek_at(accepted + Duration::from_millis(199));
    assert!(fixture.media.commands.lock().is_empty());
    assert!(fixture.controller.seek_timer.running());
    fixture.controller.state.borrow_mut().pending_seek = Some(PendingSeek {
        scope,
        position_ticks: 650,
    });
    fixture.controller.drive_seek_at(accepted + SEEK_INTERVAL);
    assert_eq!(recorded_seek(&fixture, 0).position_ticks, 650);
    assert!(!fixture.controller.seek_timer.running());
}

#[test]
fn seek_pending_cancels_on_transport_intention_even_when_transport_is_rejected_busy() {
    let fixture = Fixture::new();
    fixture.enable(seekable());
    let (_popup, token) = attach(&fixture);
    let scope = fixture.controller.capture_seek(token).unwrap();
    fixture.controller.request(MediaAction::Toggle);
    fixture.controller.seek_from_popup(&scope, 0.8);
    assert!(fixture.controller.state.borrow().pending_seek.is_some());
    fixture.controller.request(MediaAction::Next);
    assert!(fixture.controller.state.borrow().pending_seek.is_none());
    assert_eq!(fixture.media.commands.lock().len(), 1);
    fixture.media.finish_command(Ok(()));
    fixture.drain();
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(seekable()),
    }));
    fixture.drain();
    fire_trailing(&fixture);
    assert_eq!(fixture.media.commands.lock().len(), 1);
}

#[test]
fn seek_invalidated_mailbox_revokes_before_ui_drain_and_never_restores_old_capture() {
    let fixture = Fixture::new();
    let observed = seekable();
    fixture.enable(observed.clone());
    let (popup, token) = attach(&fixture);
    let scope = fixture.controller.capture_seek(token).unwrap();
    fixture.controller.state.borrow_mut().last_seek_accepted = Some(fixture.controller.seek_now());
    fixture.controller.seek_from_popup(&scope, 0.6);
    fixture.media.event(MediaEvent::SeekInvalidated);
    assert!(!fixture.controller.seek_scope_current(&scope));
    assert!(fixture.controller.capture_seek(token).is_none());
    fire_trailing(&fixture);
    assert!(fixture.media.commands.lock().is_empty());
    assert!(fixture.controller.state.borrow().pending_seek.is_none());
    fixture.drain();
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(observed),
    }));
    fixture.drain();
    assert!(popup.get_seek_enabled());
    assert!(
        !fixture.controller.seek_scope_current(&scope),
        "same native revision cannot resurrect a revoked gesture"
    );
    fixture.controller.seek_from_popup(&scope, 0.7);
    assert!(fixture.media.commands.lock().is_empty());
}

#[test]
fn position_only_changed_preserves_capture_and_pending_gesture_after_observation() {
    let fixture = Fixture::new();
    let mut observed = seekable();
    fixture.enable(observed.clone());
    let (popup, token) = attach(&fixture);
    let scope = fixture.controller.capture_seek(token).unwrap();
    let invalidations = Rc::new(Cell::new(0));
    let count = invalidations.clone();
    popup.on_media_seek_invalidated(move || count.set(count.get() + 1));
    fixture.media.event(MediaEvent::Changed);
    assert!(fixture.controller.seek_scope_current(&scope));
    fixture.drain();
    assert!(fixture.view().stale, "raw Dock observation remains dirty");
    assert!(
        !popup.get_media_view().stale,
        "ordinary Changed cannot insert a transient notice above the held slider"
    );
    assert!(popup.get_media_view().busy);
    assert!(!popup.get_media_view().toggle_enabled);
    fixture.controller.seek_from_popup(&scope, 0.8);
    assert!(fixture.controller.state.borrow().pending_seek.is_some());
    observed.timeline.as_mut().unwrap().position_ticks = 850;
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(observed),
    }));
    fixture.drain();
    assert_eq!(recorded_seek(&fixture, 0).position_ticks, 900);
    assert_eq!(popup.get_seek_progress(), 0.75);
    assert_eq!(invalidations.get(), 0);
    assert!(fixture.controller.seek_scope_current(&scope));
}

#[test]
fn seek_snapshot_session_revision_and_range_changes_each_reject_old_authority() {
    for changed in 0..3 {
        let fixture = Fixture::new();
        let mut observed = seekable();
        fixture.enable(observed.clone());
        let (popup, token) = attach(&fixture);
        let scope = fixture.controller.capture_seek(token).unwrap();
        let original = observed.clone();
        let invalidations = Rc::new(Cell::new(0));
        let count = invalidations.clone();
        popup.on_media_seek_invalidated(move || count.set(count.get() + 1));
        fixture.controller.retry();
        fixture.controller.seek_from_popup(&scope, 0.7);
        match changed {
            0 => observed.key = MediaSessionKey::issue().unwrap(),
            1 => {
                observed.seek.as_mut().unwrap().revision =
                    MediaObservationRevision::issue().unwrap()
            }
            _ => observed.seek.as_mut().unwrap().max_ticks += 1,
        }
        fixture.media.finish_read(Ok(MediaSnapshot {
            current: Some(observed),
        }));
        fixture.drain();
        assert!(!fixture.controller.seek_scope_current(&scope));
        assert!(fixture.controller.state.borrow().pending_seek.is_none());
        assert!(fixture.media.commands.lock().is_empty());
        assert_eq!(invalidations.get(), 1);
        assert!(popup.get_seek_enabled());
        fixture.controller.retry();
        fixture.media.finish_read(Ok(MediaSnapshot {
            current: Some(original),
        }));
        fixture.drain();
        assert!(
            !fixture.controller.seek_scope_current(&scope),
            "authority cannot resurrect when observations return"
        );
    }
}

#[test]
fn accepted_seek_survives_hide_reopen_but_pending_and_old_feedback_do_not() {
    let fixture = Fixture::new();
    let observed = seekable();
    let (popup, old_token) = attach(&fixture);
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(observed.clone()),
    }));
    fixture.drain();
    let old = fixture.controller.capture_seek(old_token).unwrap();
    fixture.controller.seek_from_popup(&old, 0.2);
    fixture.controller.seek_from_popup(&old, 0.8);
    fixture.controller.detach_popup(old_token);
    popup.hide().unwrap();
    assert!(fixture.controller.state.borrow().pending_seek.is_none());
    assert!(fixture.controller.state.borrow().flight_accepted);
    popup.show().unwrap();
    let new_token = fixture.controller.attach_popup(&popup).unwrap();
    let finished = Rc::new(Cell::new(0));
    let count = finished.clone();
    popup.on_media_seek_finished(move || count.set(count.get() + 1));
    assert_eq!(
        fixture.media.reads(),
        1,
        "reopen cannot overlap accepted seek"
    );
    fixture.media.finish_command(Ok(()));
    fixture.drain();
    assert_eq!(fixture.media.reads(), 2);
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(observed),
    }));
    fixture.drain();
    assert!(fixture.controller.capture_seek(new_token).is_some());
    assert!(!fixture.controller.seek_scope_current(&old));
    assert_eq!(
        finished.get(),
        0,
        "retired completion never clears new preview"
    );
    fire_trailing(&fixture);
    assert_eq!(fixture.media.commands.lock().len(), 1);
}

#[test]
fn retired_seek_failure_cannot_enter_reopened_popup_while_saved_dock_keeps_generation() {
    for reopen_before_completion in [false, true] {
        let fixture = Fixture::new();
        fixture.enable(seekable());
        let generation = fixture.controller.state.borrow().generation;
        let (popup, old_token) = attach(&fixture);
        let old = fixture.controller.capture_seek(old_token).unwrap();
        let finished = Rc::new(Cell::new(0));
        let count = finished.clone();
        popup.on_media_seek_finished(move || count.set(count.get() + 1));
        fixture.controller.seek_from_popup(&old, 0.6);
        fixture.controller.seek_from_popup(&old, 0.8);
        fixture.controller.detach_popup(old_token);
        popup.hide().unwrap();
        let new_token = reopen_before_completion.then(|| {
            popup.show().unwrap();
            fixture.controller.attach_popup(&popup).unwrap()
        });
        fixture
            .media
            .finish_command(Err(error(MediaErrorKind::Rejected)));
        fixture.drain();
        assert_eq!(fixture.controller.state.borrow().generation, generation);
        assert!(fixture.controller.state.borrow().seek_error.is_none());
        assert_eq!(finished.get(), 0, "retired feedback cannot touch new input");
        assert_eq!(fixture.media.reads(), 1, "failure never starts readback");
        let new_token = new_token.unwrap_or_else(|| {
            popup.show().unwrap();
            fixture.controller.attach_popup(&popup).unwrap()
        });
        assert_ne!(new_token, old_token);
        assert!(popup.get_seek_notice().is_empty());
        assert!(fixture.controller.capture_seek(new_token).is_some());
        fire_trailing(&fixture);
        assert_eq!(fixture.media.commands.lock().len(), 1);
    }
}

#[test]
fn seek_cancel_and_source_revocation_discard_only_unsubmitted_input() {
    let fixture = Fixture::new();
    fixture.enable(seekable());
    let popup = QuickSettings::new().unwrap();
    popup.show().unwrap();
    let admitted = Rc::new(Cell::new(true));
    let admission = admitted.clone();
    let token = fixture
        .controller
        .attach_popup_scoped(&popup, Rc::new(move || admission.get()), |_| true)
        .unwrap();
    let scope = fixture.controller.capture_seek(token).unwrap();
    fixture.controller.state.borrow_mut().last_seek_accepted = Some(fixture.controller.seek_now());
    fixture.controller.seek_from_popup(&scope, 0.5);
    fixture.controller.cancel_pending_seek(token);
    fire_trailing(&fixture);
    assert!(fixture.media.commands.lock().is_empty());
    fixture.controller.state.borrow_mut().last_seek_accepted = Some(fixture.controller.seek_now());
    fixture.controller.seek_from_popup(&scope, 0.9);
    admitted.set(false);
    fire_trailing(&fixture);
    assert!(fixture.media.commands.lock().is_empty());
    assert!(!popup.get_seek_enabled());
    assert!(
        fixture.controller.state.borrow().enabled,
        "saved Dock demand survives source revocation"
    );
}

#[test]
fn callback_revocation_after_busy_setter_cancels_seek_before_native_submission() {
    let fixture = Fixture::new();
    fixture.enable(seekable());
    let (popup, token) = attach(&fixture);
    let scope = fixture.controller.capture_seek(token).unwrap();
    let media = fixture.media.clone();
    popup.on_media_projection_complete(move || media.event(MediaEvent::SeekInvalidated));
    fixture.controller.seek_from_popup(&scope, 0.7);
    assert!(fixture.media.commands.lock().is_empty());
    assert!(fixture.controller.state.borrow().flight.is_none());
    assert!(
        fixture
            .controller
            .state
            .borrow()
            .last_seek_accepted
            .is_none()
    );
}

#[test]
fn seek_rejection_and_async_failure_preserve_independent_facts_without_auto_readback() {
    for inline_rejection in [false, true] {
        let fixture = Fixture::new();
        fixture.enable(seekable());
        let (popup, token) = attach(&fixture);
        let scope = fixture.controller.capture_seek(token).unwrap();
        if inline_rejection {
            fixture
                .media
                .command_replies
                .lock()
                .push_back(CommandReply::Reject(error(MediaErrorKind::Rejected)));
        }
        fixture.controller.seek_from_popup(&scope, 0.6);
        if !inline_rejection {
            fixture
                .media
                .finish_command(Err(error(MediaErrorKind::Rejected)));
        }
        fixture.drain();
        assert_eq!(
            fixture.media.reads(),
            1,
            "failed seek is never an automatic observation or replay"
        );
        assert_eq!(fixture.media.commands.lock().len(), 1);
        assert!(popup.get_seek_enabled());
        assert!(!popup.get_seek_notice().is_empty());
        assert!(popup.get_timeline_available());
        assert_eq!(popup.get_seek_progress(), 0.25);
        assert!(popup.get_media_view().toggle_enabled);
        assert!(popup.get_media_view().action_notice.is_empty());
        fixture.controller.retry();
        assert_eq!(fixture.media.reads(), 2);
        assert_eq!(
            fixture.media.commands.lock().len(),
            1,
            "Retry observes, never replays"
        );
    }
}

#[test]
fn seek_capability_and_range_failures_do_not_hide_transports_or_timeline() {
    for invalid_range in [false, true] {
        let fixture = Fixture::new();
        let mut observed = seekable();
        if invalid_range {
            observed.seek.as_mut().unwrap().min_ticks = 1_100;
        } else {
            observed.seek = Err(error(MediaErrorKind::Unavailable));
        }
        fixture.enable(observed);
        let (popup, token) = attach(&fixture);
        assert!(fixture.controller.capture_seek(token).is_none());
        assert!(!popup.get_seek_enabled());
        assert!(!popup.get_seek_notice().is_empty());
        assert!(popup.get_timeline_available());
        assert!(popup.get_media_view().toggle_enabled);
    }
}

#[test]
fn seek_extreme_signed_range_clamps_finite_input_and_exhaustion_fails_closed() {
    for (fraction, expected) in [(-1.0, i64::MIN), (2.0, i64::MAX)] {
        let fixture = Fixture::new();
        let mut observed = seekable();
        observed.seek.as_mut().unwrap().min_ticks = i64::MIN;
        observed.seek.as_mut().unwrap().max_ticks = i64::MAX;
        fixture.enable(observed);
        let (_popup, token) = attach(&fixture);
        let scope = fixture.controller.capture_seek(token).unwrap();
        fixture.controller.seek_from_popup(&scope, f32::NAN);
        fixture.controller.seek_from_popup(&scope, f32::INFINITY);
        assert!(fixture.media.commands.lock().is_empty());
        fixture.controller.seek_from_popup(&scope, fraction);
        assert_eq!(recorded_seek(&fixture, 0).position_ticks, expected);
    }
    let fixture = Fixture::new();
    fixture.enable(seekable());
    let (_popup, token) = attach(&fixture);
    fixture.controller.state.borrow_mut().seek_epoch = u64::MAX;
    fixture.controller.mailbox.lock().seek_epoch = u64::MAX;
    let scope = fixture.controller.capture_seek(token).unwrap();
    fixture.media.event(MediaEvent::SeekInvalidated);
    assert!(fixture.controller.mailbox.lock().seek_exhausted);
    assert!(fixture.controller.capture_seek(token).is_none());
    fixture.controller.seek_from_popup(&scope, 0.5);
    assert!(fixture.media.commands.lock().is_empty());
}

#[test]
fn seek_read_failure_retires_capture_without_discarding_independent_observed_facts() {
    let fixture = Fixture::new();
    let observed = seekable();
    fixture.enable(observed.clone());
    let (popup, token) = attach(&fixture);
    let scope = fixture.controller.capture_seek(token).unwrap();
    fixture.controller.retry();
    fixture.controller.seek_from_popup(&scope, 0.5);
    fixture
        .media
        .finish_read(Err(error(MediaErrorKind::Unavailable)));
    fixture.drain();
    assert!(!fixture.controller.seek_scope_current(&scope));
    assert!(!popup.get_seek_enabled());
    assert!(popup.get_media_view().stale, "read failure remains visible");
    assert!(fixture.controller.state.borrow().pending_seek.is_none());
    assert!(popup.get_timeline_available());
    assert!(fixture.media.commands.lock().is_empty());
    fixture.controller.retry();
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(observed),
    }));
    fixture.drain();
    assert!(popup.get_seek_enabled());
    assert!(!fixture.controller.seek_scope_current(&scope));
}

#[test]
fn seek_preview_uses_captured_range_mapping_and_never_utc_or_live_model_position() {
    let fixture = Fixture::new();
    let mut observed = seekable();
    observed.seek.as_mut().unwrap().min_ticks = 100_000_000;
    observed.seek.as_mut().unwrap().max_ticks = 1_300_000_000;
    fixture.enable(observed);
    let (_popup, token) = attach(&fixture);
    let scope = fixture.controller.capture_seek(token).unwrap();
    assert_eq!(
        scope.preview_time(0.25).as_deref(),
        Some("0:30 / 2:00 · seek preview")
    );
    assert_eq!(
        scope.preview_time(-1.0).as_deref(),
        Some("0:00 / 2:00 · seek preview")
    );
    assert!(scope.preview_time(f32::NAN).is_none());
    fixture.controller.seek_from_popup(&scope, 0.25);
    assert_eq!(recorded_seek(&fixture, 0).position_ticks, 400_000_000);
}

#[test]
fn local_seek_observation_counter_exhaustion_never_reissues_old_scope() {
    let fixture = Fixture::new();
    let mut observed = seekable();
    fixture.enable(observed.clone());
    let (_popup, token) = attach(&fixture);
    fixture.controller.state.borrow_mut().seek_observation = u64::MAX;
    let old = fixture.controller.capture_seek(token).unwrap();
    fixture.controller.retry();
    observed.seek.as_mut().unwrap().max_ticks += 1;
    fixture.media.finish_read(Ok(MediaSnapshot {
        current: Some(observed),
    }));
    fixture.drain();
    assert!(fixture.controller.state.borrow().seek_observation_exhausted);
    assert!(fixture.controller.capture_seek(token).is_none());
    fixture.controller.seek_from_popup(&old, 0.5);
    assert!(fixture.media.commands.lock().is_empty());
}
