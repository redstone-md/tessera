// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use super::mocks::RecordingShortcutHost;
use super::*;

// Keep the dependency-free portable crate; explicitly recover poisoned test
// state rather than unwrapping std locks or adding a synchronization package.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn error(kind: ShortcutErrorKind) -> ShortcutError {
    ShortcutError::new(kind, Some(5), "Controlled test failure")
}

fn trigger(generation: u64) -> ShortcutEvent {
    ShortcutEvent::Triggered(ShortcutTrigger {
        generation,
        action: ShortcutAction::ToggleLauncher,
        cursor: None,
    })
}

#[test]
fn source_defaults_and_immutable_launcher_survive_settings_edit_and_pause() {
    let original = ShortcutConfig::default();
    assert!(original.enabled());
    assert_eq!(original.settings_override(), None);
    assert_eq!(
        original.effective_chord(ShortcutAction::ToggleLauncher),
        KeyChord::BareWin
    );
    assert_eq!(
        original
            .effective_chord(ShortcutAction::OpenSettings)
            .label(),
        "Win + K"
    );
    let chord = KeyChord::new(
        KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        },
        0x41,
    )
    .unwrap();
    let edited = original
        .clone()
        .with_settings_override(Some(chord))
        .unwrap()
        .with_enabled(false);
    assert!(!edited.enabled());
    assert_eq!(edited.settings_override(), Some(&chord));
    assert_eq!(edited.effective_chord(ShortcutAction::OpenSettings), chord);
    assert_eq!(
        edited.effective_chord(ShortcutAction::ToggleLauncher),
        KeyChord::BareWin
    );
    assert!(original.enabled());
    assert_eq!(original.settings_override(), None);
    assert_eq!(
        edited
            .with_settings_override(None)
            .unwrap()
            .with_enabled(true),
        original
    );
}

#[test]
fn settings_cannot_steal_bare_win_or_ingest_direct_invalid_enum() {
    let config = ShortcutConfig::default();
    assert_eq!(
        config
            .clone()
            .with_settings_override(Some(KeyChord::BareWin))
            .unwrap_err()
            .kind(),
        ShortcutErrorKind::Conflict
    );
    let invalid = KeyChord::Chord {
        modifiers: KeyModifiers::default(),
        key: 0x3A,
    };
    assert_eq!(
        config
            .with_settings_override(Some(invalid))
            .unwrap_err()
            .kind(),
        ShortcutErrorKind::InvalidData
    );
}

#[test]
fn reject_reserved_unassigned_out_of_range_modifier_and_f12_keys() {
    for key in [
        0,
        7,
        0x0A,
        0x0B,
        0x0E,
        0x0F,
        0x10,
        0x11,
        0x12,
        0x3A,
        0x40,
        0x5B,
        0x5C,
        0x5E,
        0x7B,
        0x88,
        0x8F,
        0x97,
        0x9F,
        0xA0,
        0xA1,
        0xA2,
        0xA3,
        0xA4,
        0xA5,
        0xB8,
        0xB9,
        0xC1,
        0xC2,
        0xE0,
        0xE8,
        0xFC,
        0xFF,
        256,
        u16::MAX,
    ] {
        assert!(
            KeyChord::new(KeyModifiers::default(), key).is_err(),
            "accepted reserved key {key:#X}"
        );
    }
    assert!(KeyChord::BareWin.validate().is_ok());
}

#[test]
fn valid_defined_keys_include_digits_ime_function_oem_media_and_gamepad() {
    for key in [
        1, 3, 8, 0x16, 0x1A, 0x20, 0x39, 0x41, 0x5A, 0x5D, 0x60, 0x70, 0x7A, 0x7C, 0x87, 0x92,
        0x96, 0xA6, 0xB7, 0xBA, 0xC0, 0xC3, 0xDA, 0xDF, 0xE1, 0xE6, 0xE7, 0xE9, 0xF5, 0xFB, 0xFD,
        0xFE,
    ] {
        assert!(
            KeyChord::new(KeyModifiers::default(), key).is_ok(),
            "rejected defined key {key:#X}"
        );
    }
}

#[test]
fn labels_are_readable_deterministic_and_never_authority() {
    let modifiers = KeyModifiers {
        control: true,
        alt: true,
        shift: true,
        win: true,
    };
    assert_eq!(
        KeyChord::new(modifiers, 0x4B).unwrap().label(),
        "Ctrl + Alt + Shift + Win + K"
    );
    assert_eq!(KeyChord::BareWin.label(), "Win");
    for (key, label) in [
        (0x30, "0"),
        (0x61, "Num 1"),
        (0x7C, "F13"),
        (0x21, "Page Up"),
        (0xB3, "Media Play / Pause"),
        (0xBA, "OEM 1"),
        (0xC3, "Gamepad A"),
    ] {
        assert_eq!(
            KeyChord::new(KeyModifiers::default(), key).unwrap().label(),
            label
        );
    }
    for key in 0..=255 {
        if let Ok(chord) = KeyChord::new(KeyModifiers::default(), key) {
            assert!(
                !chord.label().contains("Invalid"),
                "missing key label {key:#X}"
            );
            assert!(!chord.label().contains("0x"));
        }
    }
    assert_eq!(
        KeyChord::Chord {
            modifiers,
            key: u16::MAX
        }
        .label(),
        "Invalid shortcut"
    );
}

#[test]
fn errors_never_retain_private_input_even_short_or_long_unicode() {
    let long_private = "秘密\n".repeat(4096);
    for private in [
        "C:\\Users\\private\\file",
        "person@example.test",
        "captured-password",
        long_private.as_str(),
    ] {
        let failure = ShortcutError::new(ShortcutErrorKind::AccessDenied, Some(5), private);
        assert_eq!(failure.kind(), ShortcutErrorKind::AccessDenied);
        assert_eq!(failure.native_code(), Some(5));
        assert!(failure.message().len() <= 256);
        assert!(!failure.message().chars().any(char::is_control));
        assert!(!format!("{failure:?} {failure}").contains(private));
        assert_eq!(failure.message(), "Shortcut access was denied.");
    }
}

#[test]
fn snapshots_report_actual_registration_conflict_denial_and_pause() {
    let config = ShortcutConfig::default();
    let snapshot =
        ShortcutSnapshot::new(&config, 7, Ok(()), Err(error(ShortcutErrorKind::Conflict)));
    assert_eq!(snapshot.generation, 7);
    assert!(snapshot.enabled);
    assert_eq!(snapshot.bindings.len(), 2);
    assert_eq!(
        snapshot.binding(ShortcutAction::ToggleLauncher).state,
        ShortcutBindingState::Registered
    );
    assert_eq!(
        snapshot.binding(ShortcutAction::OpenSettings).state,
        ShortcutBindingState::Conflict
    );
    assert_eq!(
        snapshot
            .binding(ShortcutAction::OpenSettings)
            .error
            .as_ref()
            .unwrap()
            .native_code(),
        Some(5)
    );
    let denied = ShortcutSnapshot::new(
        &config,
        8,
        Err(error(ShortcutErrorKind::AccessDenied)),
        Err(error(ShortcutErrorKind::Unsupported)),
    );
    assert_eq!(
        denied.binding(ShortcutAction::ToggleLauncher).state,
        ShortcutBindingState::Denied
    );
    assert_eq!(
        denied.binding(ShortcutAction::OpenSettings).state,
        ShortcutBindingState::Unavailable
    );
    let paused = ShortcutSnapshot::new(
        &config.with_enabled(false),
        9,
        Ok(()),
        Err(error(ShortcutErrorKind::Other)),
    );
    assert!(!paused.enabled);
    assert!(
        paused
            .bindings
            .iter()
            .all(|binding| binding.state == ShortcutBindingState::Paused)
    );
    assert!(paused.binding(ShortcutAction::OpenSettings).error.is_some());
}

#[test]
fn rejection_accepts_no_completion_and_records_no_configuration() {
    let host = RecordingShortcutHost::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let callback_calls = calls.clone();
    host.reject_next(error(ShortcutErrorKind::Conflict));
    assert_eq!(
        host.configure(
            ShortcutConfig::default(),
            Box::new(move |_| {
                callback_calls.fetch_add(1, Ordering::Relaxed);
            })
        )
        .unwrap_err()
        .kind(),
        ShortcutErrorKind::Conflict
    );
    host.close();
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(host.configurations().is_empty());
}

#[test]
fn accepted_explicit_completion_is_exactly_once_and_reentrant() {
    let host = RecordingShortcutHost::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let callback_calls = calls.clone();
    let reentrant = host.clone();
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |result| {
            assert_eq!(result.unwrap().generation, 1);
            callback_calls.fetch_add(1, Ordering::Relaxed);
            reentrant.close();
        }),
    )
    .unwrap();
    assert_eq!(host.pending_count(), 1);
    let pending = host.take_next_completion().unwrap();
    assert_eq!(pending.generation(), 1);
    assert_eq!(pending.config(), &ShortcutConfig::default());
    pending.complete_registered();
    assert!(!host.complete_next(Err(error(ShortcutErrorKind::Other))));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn controlled_inline_callback_runs_before_return_without_lock_reentrancy() {
    let host = RecordingShortcutHost::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let callback_calls = calls.clone();
    let reentrant = host.clone();
    host.inline_next(Err(error(ShortcutErrorKind::AccessDenied)));
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |result| {
            assert_eq!(result.unwrap_err().kind(), ShortcutErrorKind::AccessDenied);
            assert_eq!(reentrant.configurations().len(), 1);
            callback_calls.fetch_add(1, Ordering::Relaxed);
        }),
    )
    .unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(host.pending_count(), 0);
}

#[test]
fn close_cancels_each_queued_acceptance_once_and_rejects_new_work() {
    let host = RecordingShortcutHost::new();
    let calls = Arc::new(AtomicUsize::new(0));
    for _ in 0..3 {
        let calls = calls.clone();
        host.configure(
            ShortcutConfig::default(),
            Box::new(move |result| {
                assert_eq!(result.unwrap_err().kind(), ShortcutErrorKind::Busy);
                calls.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .unwrap();
    }
    host.close();
    host.close();
    assert_eq!(calls.load(Ordering::Relaxed), 3);
    assert_eq!(host.pending_count(), 0);
    assert!(
        host.configure(
            ShortcutConfig::default(),
            Box::new(|_| panic!("rejected callback"))
        )
        .is_err()
    );
    assert!(host.subscribe(Arc::new(|_| panic!("closed sink"))).is_err());
}

#[test]
fn transferred_callback_can_finish_late_after_close_or_cancel_on_drop() {
    let host = RecordingShortcutHost::new();
    let results = Arc::new(Mutex::new(Vec::new()));
    for _ in 0..2 {
        let results = results.clone();
        host.configure(
            ShortcutConfig::default(),
            Box::new(move |result| {
                locked(&results).push(result);
            }),
        )
        .unwrap();
    }
    let late = host.take_next_completion().unwrap();
    let canceled = host.take_next_completion().unwrap();
    host.close();
    assert!(locked(&results).is_empty());
    late.complete_registered();
    drop(canceled);
    let results = locked(&results);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].as_ref().unwrap().generation, 1);
    assert_eq!(
        results[1].as_ref().unwrap_err().kind(),
        ShortcutErrorKind::Busy
    );
}

#[test]
fn subscription_drop_and_close_disable_events_but_stale_events_are_testable() {
    let host = RecordingShortcutHost::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let received = events.clone();
    let guard = host
        .subscribe(Arc::new(move |event| locked(&received).push(event)))
        .unwrap();
    host.emit(trigger(42));
    host.emit(trigger(1));
    host.emit(ShortcutEvent::Unavailable(error(
        ShortcutErrorKind::Conflict,
    )));
    assert_eq!(locked(&events).len(), 3);
    assert_eq!(locked(&events)[1], trigger(1));
    drop(guard);
    host.emit(trigger(43));
    assert_eq!(locked(&events).len(), 3);
    let received = events.clone();
    let _guard = host
        .subscribe(Arc::new(move |event| locked(&received).push(event)))
        .unwrap();
    host.close();
    host.emit(trigger(44));
    assert_eq!(locked(&events).len(), 3);
}

#[test]
fn event_callback_can_drop_another_guard_before_its_admission() {
    let host = RecordingShortcutHost::new();
    let second_guard = Arc::new(Mutex::new(None));
    let captured = second_guard.clone();
    let _first = host
        .subscribe(Arc::new(move |_| {
            locked(&captured).take();
        }))
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let received = calls.clone();
    *locked(&second_guard) = Some(
        host.subscribe(Arc::new(move |_| {
            received.fetch_add(1, Ordering::Relaxed);
        }))
        .unwrap(),
    );
    host.emit(trigger(1));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[test]
fn queue_is_bounded_and_generations_increase_on_acceptance_only() {
    let host = RecordingShortcutHost::new();
    for _ in 0..16 {
        host.configure(ShortcutConfig::default(), Box::new(|_| {}))
            .unwrap();
    }
    assert!(
        host.configure(
            ShortcutConfig::default(),
            Box::new(|_| panic!("full queue completion"))
        )
        .is_err()
    );
    assert_eq!(host.configurations().len(), 16);
    assert_eq!(host.take_next_completion().unwrap().generation(), 1);
    host.configure(ShortcutConfig::default(), Box::new(|_| {}))
        .unwrap();
    for generation in 2..=17 {
        assert_eq!(
            host.take_next_completion().unwrap().generation(),
            generation
        );
    }
}

#[test]
fn last_host_drop_completes_owned_pending_work_and_disables_guard() {
    let calls = Arc::new(AtomicUsize::new(0));
    let host = RecordingShortcutHost::new();
    let received = calls.clone();
    let _guard = host
        .subscribe(Arc::new(move |_| {
            received.fetch_add(100, Ordering::Relaxed);
        }))
        .unwrap();
    let received = calls.clone();
    host.configure(
        ShortcutConfig::default(),
        Box::new(move |result| {
            assert!(result.is_err());
            received.fetch_add(1, Ordering::Relaxed);
        }),
    )
    .unwrap();
    drop(host);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
