// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use slint::platform::Key;
use tessera_system::shortcuts::mocks::RecordingShortcutHost;
use tessera_system::shortcuts::{KeyModifiers, ShortcutCompletion, ShortcutSink};

use super::*;

struct Fixture {
    controller: Rc<ShortcutsController>,
    host: Arc<RecordingShortcutHost>,
    current: Rc<Cell<bool>>,
    factories: Rc<Cell<usize>>,
    delivered: Rc<RefCell<Vec<ShortcutTrigger>>>,
    views: Rc<RefCell<Vec<ShortcutsView>>>,
    wakes: Arc<AtomicUsize>,
}

impl Fixture {
    fn new() -> Self {
        let host = Arc::new(RecordingShortcutHost::new());
        let current = Rc::new(Cell::new(true));
        let factories = Rc::new(Cell::new(0));
        let delivered = Rc::new(RefCell::new(Vec::new()));
        let views = Rc::new(RefCell::new(Vec::new()));
        let wakes = Arc::new(AtomicUsize::new(0));
        let provider = host.clone();
        let calls = factories.clone();
        let admitted = current.clone();
        let actions = delivered.clone();
        let projections = views.clone();
        let notifications = wakes.clone();
        let controller = Rc::new(ShortcutsController::new(
            ShortcutConfig::default(),
            ShortcutsBindings {
                factory: Rc::new(move || {
                    calls.set(calls.get() + 1);
                    Ok(Some(provider.clone()))
                }),
                is_current: Rc::new(move || admitted.get()),
                project: Rc::new(move |view| projections.borrow_mut().push(view)),
                deliver: Rc::new(move |trigger| actions.borrow_mut().push(trigger)),
                wake: Arc::new(move || {
                    notifications.fetch_add(1, Ordering::Relaxed);
                }),
            },
        ));
        Self {
            controller,
            host,
            current,
            factories,
            delivered,
            views,
            wakes,
        }
    }

    fn apply(&self) -> u64 {
        self.controller.apply_saved(self.controller.draft_config());
        let pending = self.host.take_next_completion().unwrap();
        let generation = pending.generation();
        pending.complete_registered();
        self.controller.process_pending();
        generation
    }

    fn trigger(&self, generation: u64, action: ShortcutAction) {
        self.host.emit(ShortcutEvent::Triggered(ShortcutTrigger {
            generation,
            action,
            cursor: None,
        }));
    }
}

fn event(text: impl Into<slint::SharedString>, control: bool, win: bool) -> KeyEvent {
    let mut event = KeyEvent::default();
    event.text = text.into();
    event.modifiers.control = control;
    event.modifiers.meta = win;
    event
}

fn settings(key: u16) -> KeyChord {
    KeyChord::new(
        KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        },
        key,
    )
    .unwrap()
}

#[test]
fn construction_and_all_draft_edits_never_acquire_configure_or_save() {
    let fixture = Fixture::new();
    fixture.controller.project();
    fixture.controller.set_draft_enabled(false);
    fixture.controller.set_draft_enabled(true);
    fixture
        .controller
        .set_settings_override(Some(settings(0x4c)))
        .unwrap();
    fixture.controller.reset_settings();
    fixture.controller.begin_capture();
    assert!(fixture.controller.key_pressed(event("k", true, false)));
    assert!(fixture.controller.key_released(event("k", false, false)));
    assert_eq!(
        fixture.controller.draft_config().settings_override(),
        Some(&settings(0x4b))
    );
    fixture.controller.reset_draft(ShortcutConfig::default());
    assert!(fixture.host.configurations().is_empty());
    assert_eq!(fixture.factories.get(), 0);
    assert_eq!(fixture.wakes.load(Ordering::Relaxed), 0);
    assert!(!fixture.views.borrow().is_empty());
}

#[test]
fn typed_capture_commits_only_matching_release_and_preserves_down_modifiers() {
    let fixture = Fixture::new();
    fixture.controller.begin_capture();
    fixture
        .controller
        .key_pressed(event(Key::Meta, false, true));
    fixture.controller.key_pressed(event("k", false, true));
    let mut repeated = event("k", false, true);
    repeated.repeat = true;
    fixture.controller.key_pressed(repeated);
    assert_eq!(fixture.controller.draft_config(), ShortcutConfig::default());
    fixture
        .controller
        .key_released(event(Key::Meta, false, false));
    fixture.controller.key_released(event("K", false, false));
    assert!(!fixture.controller.view().capturing);
    let chord = fixture
        .controller
        .draft_config()
        .settings_override()
        .copied()
        .unwrap();
    assert_eq!(
        chord,
        KeyChord::new(
            KeyModifiers {
                win: true,
                ..KeyModifiers::default()
            },
            0x4b
        )
        .unwrap()
    );
    assert!(fixture.host.configurations().is_empty());
}

#[test]
fn capture_escape_disabled_cancel_and_intrinsic_backtab_use_sdk_events() {
    let fixture = Fixture::new();
    assert!(!fixture.controller.key_pressed(event("a", false, false)));
    assert!(!fixture.controller.key_released(event("a", false, false)));
    fixture.controller.begin_capture();
    fixture.controller.key_pressed(event("k", true, false));
    fixture
        .controller
        .key_pressed(event(Key::Escape, false, false));
    assert!(!fixture.controller.key_released(event("k", false, false)));
    assert_eq!(fixture.controller.draft_config(), ShortcutConfig::default());
    fixture.controller.begin_capture();
    fixture.controller.set_draft_enabled(false);
    assert!(!fixture.controller.view().capturing);
    fixture.controller.set_draft_enabled(true);
    fixture.controller.begin_capture();
    fixture
        .controller
        .key_pressed(event(Key::Backtab, true, false));
    fixture
        .controller
        .key_released(event(Key::Backtab, false, false));
    assert_eq!(
        fixture.controller.draft_config().settings_override(),
        Some(
            &KeyChord::new(
                KeyModifiers {
                    control: true,
                    shift: true,
                    ..KeyModifiers::default()
                },
                0x09,
            )
            .unwrap()
        )
    );
}

#[test]
fn readonly_win_rejects_override_and_invalid_keys_never_become_authority() {
    let fixture = Fixture::new();
    let original = fixture.controller.draft_config();
    assert!(
        fixture
            .controller
            .set_settings_override(Some(KeyChord::BareWin))
            .is_err()
    );
    assert_eq!(fixture.controller.draft_config(), original);
    for text in [
        slint::SharedString::from(Key::F12),
        "é".into(),
        "ab".into(),
        "+".into(),
    ] {
        fixture.controller.begin_capture();
        fixture
            .controller
            .key_pressed(event(text.clone(), true, false));
        fixture.controller.key_released(event(text, false, false));
        assert_eq!(fixture.controller.draft_config(), original);
        assert!(!fixture.controller.view().message.is_empty());
        fixture.controller.cancel_capture();
    }
    fixture.controller.begin_capture();
    fixture.controller.key_pressed(event("a", true, false));
    fixture.controller.key_pressed(event("b", true, false));
    fixture.controller.key_released(event("b", false, false));
    assert!(!fixture.controller.view().capturing);
    assert_eq!(fixture.controller.draft_config(), original);
    assert_eq!(
        original.effective_chord(ShortcutAction::ToggleLauncher),
        KeyChord::BareWin
    );
}

#[test]
fn only_parent_complete_save_applies_typed_draft_and_status_stays_applied() {
    let fixture = Fixture::new();
    let generation = fixture.apply();
    fixture
        .controller
        .set_settings_override(Some(settings(0x4c)))
        .unwrap();
    assert_eq!(fixture.host.configurations().len(), 1);
    assert!(fixture.controller.view().dirty);
    assert!(
        fixture
            .controller
            .view()
            .settings_status
            .contains("Registered")
    );
    assert!(fixture.controller.view().settings_status.contains('K'));
    // The parent can store its full preference value through this seam. Merely
    // obtaining the typed draft (including a failed parent Save) does nothing.
    let full_save_shortcuts = fixture.controller.draft_config();
    assert_eq!(fixture.host.configurations().len(), 1);
    fixture.controller.apply_saved(full_save_shortcuts.clone());
    fixture.trigger(generation, ShortcutAction::OpenSettings);
    fixture.controller.process_pending();
    assert!(fixture.delivered.borrow().is_empty());
    assert_eq!(fixture.host.configurations()[1], full_save_shortcuts);
    fixture
        .host
        .take_next_completion()
        .unwrap()
        .complete_registered();
    fixture.controller.process_pending();
    assert!(!fixture.controller.view().dirty);
    assert!(fixture.controller.view().settings_status.contains('L'));
}

#[test]
fn registration_conflict_is_truthful_and_never_dispatches_unregistered_settings() {
    let fixture = Fixture::new();
    fixture.controller.apply_saved(ShortcutConfig::default());
    let pending = fixture.host.take_next_completion().unwrap();
    let generation = pending.generation();
    let conflict = ShortcutError::new(
        ShortcutErrorKind::Conflict,
        Some(1409),
        "Conflicting registration",
    );
    let snapshot = ShortcutSnapshot::new(pending.config(), generation, Ok(()), Err(conflict));
    pending.complete(Ok(snapshot));
    fixture.controller.process_pending();
    assert!(
        fixture
            .controller
            .view()
            .settings_status
            .starts_with("Conflict")
    );
    assert!(
        fixture
            .controller
            .view()
            .launcher_status
            .starts_with("Registered")
    );
    fixture.trigger(generation, ShortcutAction::OpenSettings);
    fixture.trigger(generation, ShortcutAction::ToggleLauncher);
    fixture.controller.process_pending();
    assert_eq!(fixture.delivered.borrow().len(), 1);
    assert_eq!(
        fixture.delivered.borrow()[0].action,
        ShortcutAction::ToggleLauncher
    );
    assert_eq!(fixture.delivered.borrow()[0].cursor, None);
}

#[test]
fn inline_reply_immediate_rejection_and_invalid_snapshot_cross_mailbox() {
    let fixture = Fixture::new();
    fixture.host.inline_next(Ok(ShortcutSnapshot::new(
        &ShortcutConfig::default(),
        1,
        Ok(()),
        Ok(()),
    )));
    fixture.controller.apply_saved(ShortcutConfig::default());
    assert!(fixture.controller.view().applying);
    assert_eq!(fixture.controller.view().settings_status, "Not applied");
    fixture.controller.process_pending();
    assert!(
        fixture
            .controller
            .view()
            .settings_status
            .starts_with("Registered")
    );
    fixture.host.reject_next(ShortcutError::new(
        ShortcutErrorKind::AccessDenied,
        Some(5),
        "Denied",
    ));
    fixture.controller.apply_saved(ShortcutConfig::default());
    fixture.controller.process_pending();
    assert!(fixture.controller.view().message.contains("denied"));
    assert!(!fixture.controller.view().applying);
    fixture.controller.apply_saved(ShortcutConfig::default());
    let pending = fixture.host.take_next_completion().unwrap();
    let mut invalid = ShortcutSnapshot::new(pending.config(), pending.generation(), Ok(()), Ok(()));
    invalid.bindings[1].action = ShortcutAction::ToggleLauncher;
    pending.complete(Ok(invalid));
    fixture.controller.process_pending();
    assert!(fixture.controller.view().message.contains("invalid"));
}

#[test]
fn accepted_flight_survives_pause_and_multiple_saves_with_one_latest_slot() {
    let fixture = Fixture::new();
    fixture.controller.apply_saved(ShortcutConfig::default());
    let first = fixture.host.take_next_completion().unwrap();
    fixture.controller.set_paused(true);
    let saved = ShortcutConfig::default()
        .with_settings_override(Some(settings(0x4c)))
        .unwrap();
    fixture.controller.apply_saved(saved.clone());
    fixture.controller.set_paused(false);
    assert_eq!(fixture.host.configurations().len(), 1);
    assert_eq!(fixture.host.pending_count(), 0);
    first.complete_registered();
    fixture.controller.process_pending();
    assert_eq!(
        fixture.host.configurations(),
        vec![ShortcutConfig::default(), saved]
    );
    let latest = fixture.host.take_next_completion().unwrap();
    let generation = latest.generation();
    latest.complete_registered();
    fixture.controller.process_pending();
    fixture.trigger(1, ShortcutAction::ToggleLauncher);
    fixture.trigger(generation, ShortcutAction::OpenSettings);
    fixture.controller.process_pending();
    assert_eq!(fixture.delivered.borrow().len(), 1);
}

#[test]
fn explicit_pause_restores_saved_not_unsaved_draft_and_disabled_never_dispatches() {
    let fixture = Fixture::new();
    let generation = fixture.apply();
    fixture
        .controller
        .set_settings_override(Some(settings(0x4c)))
        .unwrap();
    fixture.controller.set_paused(true);
    fixture.trigger(generation, ShortcutAction::ToggleLauncher);
    fixture
        .host
        .take_next_completion()
        .unwrap()
        .complete_registered();
    fixture.controller.process_pending();
    assert!(
        fixture
            .controller
            .view()
            .launcher_status
            .starts_with("Paused")
    );
    assert!(fixture.delivered.borrow().is_empty());
    fixture.controller.set_paused(false);
    assert_eq!(
        fixture.host.configurations().last(),
        Some(&ShortcutConfig::default())
    );
    fixture
        .host
        .take_next_completion()
        .unwrap()
        .complete_registered();
    fixture.controller.process_pending();
    assert!(fixture.controller.view().dirty);
    fixture
        .controller
        .apply_saved(ShortcutConfig::default().with_enabled(false));
    let pending = fixture.host.take_next_completion().unwrap();
    let disabled_generation = pending.generation();
    pending.complete_registered();
    fixture.controller.process_pending();
    fixture.trigger(disabled_generation, ShortcutAction::ToggleLauncher);
    fixture.controller.process_pending();
    assert!(fixture.delivered.borrow().is_empty());
}

#[test]
fn cancel_current_and_late_completion_drop_do_not_replay_or_start_second_flight() {
    let fixture = Fixture::new();
    fixture.controller.apply_saved(ShortcutConfig::default());
    let accepted = fixture.host.take_next_completion().unwrap();
    fixture.controller.cancel_current();
    fixture.controller.apply_saved(ShortcutConfig::default());
    assert_eq!(fixture.host.configurations().len(), 1);
    accepted.complete_registered();
    fixture.controller.process_pending();
    assert_eq!(fixture.host.configurations().len(), 2);
    let late = fixture.host.take_next_completion().unwrap();
    let wakes = fixture.wakes.clone();
    let before = wakes.load(Ordering::Relaxed);
    drop(fixture.controller);
    late.complete_registered();
    fixture.host.emit(ShortcutEvent::Triggered(ShortcutTrigger {
        generation: 2,
        action: ShortcutAction::ToggleLauncher,
        cursor: None,
    }));
    assert_eq!(wakes.load(Ordering::Relaxed), before);
    assert!(fixture.delivered.borrow().is_empty());
}

#[test]
fn event_saturation_is_bounded_and_fails_closed_without_stale_replay() {
    let fixture = Fixture::new();
    let generation = fixture.apply();
    let before = fixture.wakes.load(Ordering::Relaxed);
    for _ in 0..128 {
        fixture.trigger(generation, ShortcutAction::ToggleLauncher);
    }
    assert_eq!(fixture.wakes.load(Ordering::Relaxed), before + 1);
    assert_eq!(fixture.controller.mailbox.lock().events.len(), 1);
    fixture.controller.process_pending();
    assert!(fixture.delivered.borrow().is_empty());
    assert!(fixture.controller.view().message.contains("busy"));
}

#[test]
fn root_scope_is_independent_of_preferences_and_unavailable_scope_never_calls_factory() {
    let fixture = Fixture::new();
    fixture.current.set(false);
    fixture.controller.apply_saved(ShortcutConfig::default());
    fixture.controller.process_pending();
    assert_eq!(fixture.factories.get(), 0);
    fixture.current.set(true);
    fixture.controller.process_pending();
    fixture
        .host
        .take_next_completion()
        .unwrap()
        .complete_registered();
    fixture.controller.process_pending();
    fixture.trigger(1, ShortcutAction::OpenSettings);
    fixture.controller.process_pending();
    assert_eq!(fixture.delivered.borrow().len(), 1);
    fixture.current.set(false);
    fixture.trigger(1, ShortcutAction::OpenSettings);
    fixture.controller.process_pending();
    assert_eq!(fixture.delivered.borrow().len(), 1);
}

#[test]
fn factory_reentry_cannot_subscribe_or_configure_an_abandoned_scope() {
    let slot: Rc<RefCell<std::rc::Weak<ShortcutsController>>> =
        Rc::new(RefCell::new(std::rc::Weak::new()));
    let provider = Arc::new(RecordingShortcutHost::new());
    let reentry = slot.clone();
    let host = provider.clone();
    let controller = Rc::new(ShortcutsController::new(
        ShortcutConfig::default(),
        ShortcutsBindings {
            factory: Rc::new(move || {
                reentry.borrow().upgrade().unwrap().cancel_current();
                Ok(Some(host.clone()))
            }),
            is_current: Rc::new(|| true),
            project: Rc::new(|_| {}),
            deliver: Rc::new(|_| panic!("abandoned scope must not deliver")),
            wake: Arc::new(|| {}),
        },
    ));
    *slot.borrow_mut() = Rc::downgrade(&controller);
    controller.apply_saved(ShortcutConfig::default());
    controller.process_pending();
    assert!(provider.configurations().is_empty());
    assert!(controller.state.borrow().subscription.is_none());
}

/// This adversarial in-memory guard deliberately invokes a retained sink during
/// Drop, proving retirement ordering independently of a cooperative mock host.
struct DropSinkHost {
    sink: Arc<Mutex<Option<ShortcutSink>>>,
}
struct DropSinkGuard(Arc<Mutex<Option<ShortcutSink>>>);
impl ShortcutSubscription for DropSinkGuard {}
impl Drop for DropSinkGuard {
    fn drop(&mut self) {
        let sink = self.0.lock().clone();
        if let Some(sink) = sink {
            sink(ShortcutEvent::Triggered(ShortcutTrigger {
                generation: 1,
                action: ShortcutAction::ToggleLauncher,
                cursor: None,
            }));
        }
    }
}
impl ShortcutHost for DropSinkHost {
    fn configure(
        &self,
        config: ShortcutConfig,
        completion: ShortcutCompletion,
    ) -> Result<(), ShortcutError> {
        completion(Ok(ShortcutSnapshot::new(&config, 1, Ok(()), Ok(()))));
        Ok(())
    }
    fn subscribe(
        &self,
        sink: ShortcutSink,
    ) -> Result<Box<dyn ShortcutSubscription>, ShortcutError> {
        *self.sink.lock() = Some(sink);
        Ok(Box::new(DropSinkGuard(self.sink.clone())))
    }
}

#[test]
fn native_guard_drop_happens_only_after_ui_delivery_retirement() {
    let host = Arc::new(DropSinkHost {
        sink: Arc::new(Mutex::new(None)),
    });
    let provider = host.clone();
    let actions = Rc::new(Cell::new(0));
    let delivered = actions.clone();
    let controller = ShortcutsController::new(
        ShortcutConfig::default(),
        ShortcutsBindings {
            factory: Rc::new(move || Ok(Some(provider.clone()))),
            is_current: Rc::new(|| true),
            project: Rc::new(|_| {}),
            deliver: Rc::new(move |_| delivered.set(delivered.get() + 1)),
            wake: Arc::new(|| {}),
        },
    );
    controller.apply_saved(ShortcutConfig::default());
    controller.process_pending();
    controller.cancel_current();
    controller.process_pending();
    assert_eq!(actions.get(), 0);
    assert!(controller.mailbox.lock().events.is_empty());
}
