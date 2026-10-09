// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::cell::Cell;

use super::*;
use tessera_system::shortcuts::mocks::RecordingShortcutHost;

fn trigger(generation: u64) -> ShortcutEvent {
    ShortcutEvent::Triggered(ShortcutTrigger {
        generation,
        action: ShortcutAction::ToggleLauncher,
        cursor: None,
    })
}

#[test]
fn current_getter_reentry_is_checked_after_return_and_does_not_recurse() {
    let slot: Rc<RefCell<std::rc::Weak<ShortcutsController>>> =
        Rc::new(RefCell::new(std::rc::Weak::new()));
    let entered = Rc::new(Cell::new(false));
    let weak = slot.clone();
    let once = entered.clone();
    let host = Arc::new(RecordingShortcutHost::new());
    let provider = host.clone();
    let controller = Rc::new(ShortcutsController::new(
        ShortcutConfig::default(),
        ShortcutsBindings {
            factory: Rc::new(move || Ok(Some(provider.clone()))),
            is_current: Rc::new(move || {
                if !once.replace(true) {
                    weak.borrow().upgrade().unwrap().cancel_current();
                }
                true
            }),
            project: Rc::new(|_| {}),
            deliver: Rc::new(|_| panic!("cancelled getter scope")),
            wake: Arc::new(|| {}),
        },
    ));
    *slot.borrow_mut() = Rc::downgrade(&controller);
    controller.apply_saved(ShortcutConfig::default());
    assert!(host.configurations().is_empty());
    controller.apply_saved(ShortcutConfig::default());
    assert_eq!(host.configurations().len(), 1);
}

#[test]
fn root_action_reentry_cancels_remaining_already_drained_events() {
    let slot: Rc<RefCell<std::rc::Weak<ShortcutsController>>> =
        Rc::new(RefCell::new(std::rc::Weak::new()));
    let weak = slot.clone();
    let count = Rc::new(Cell::new(0));
    let delivered = count.clone();
    let host = Arc::new(RecordingShortcutHost::new());
    let provider = host.clone();
    let controller = Rc::new(ShortcutsController::new(
        ShortcutConfig::default(),
        ShortcutsBindings {
            factory: Rc::new(move || Ok(Some(provider.clone()))),
            is_current: Rc::new(|| true),
            project: Rc::new(|_| {}),
            deliver: Rc::new(move |_| {
                delivered.set(delivered.get() + 1);
                weak.borrow().upgrade().unwrap().cancel_current();
            }),
            wake: Arc::new(|| {}),
        },
    ));
    *slot.borrow_mut() = Rc::downgrade(&controller);
    controller.apply_saved(ShortcutConfig::default());
    host.take_next_completion().unwrap().complete_registered();
    controller.process_pending();
    host.emit(trigger(1));
    host.emit(trigger(1));
    controller.process_pending();
    assert_eq!(count.get(), 1);
}

#[test]
fn native_pause_authority_requires_ack_and_old_subscription_never_reenters_new_scope() {
    let host = Arc::new(RecordingShortcutHost::new());
    let provider = host.clone();
    let count = Rc::new(Cell::new(0));
    let delivered = count.clone();
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
    host.take_next_completion().unwrap().complete_registered();
    controller.process_pending();
    let old_source = controller.mailbox.lock().source;
    controller.set_paused(true);
    assert!(!controller.view().pause_applied);
    host.take_next_completion().unwrap().complete_registered();
    assert!(
        !controller.view().pause_applied,
        "provider callback alone never mutates UI authority"
    );
    controller.process_pending();
    assert!(controller.view().pause_applied);
    controller.cancel_current();
    controller.set_paused(false);
    controller.apply_saved(ShortcutConfig::default());
    let pending = host.take_next_completion().unwrap();
    let generation = pending.generation();
    pending.complete_registered();
    controller.process_pending();
    mailbox::event(
        &controller.mailbox,
        &controller.bindings.wake,
        old_source,
        trigger(generation),
    );
    controller.process_pending();
    assert_eq!(count.get(), 0);
    host.emit(trigger(generation));
    controller.process_pending();
    assert_eq!(count.get(), 1);
}

#[test]
fn same_cached_provider_cannot_reuse_an_already_applied_generation_on_reopen() {
    let host = Arc::new(RecordingShortcutHost::new());
    let provider = host.clone();
    let controller = ShortcutsController::new(
        ShortcutConfig::default(),
        ShortcutsBindings {
            factory: Rc::new(move || Ok(Some(provider.clone()))),
            is_current: Rc::new(|| true),
            project: Rc::new(|_| {}),
            deliver: Rc::new(|_| panic!("reused generation")),
            wake: Arc::new(|| {}),
        },
    );
    controller.apply_saved(ShortcutConfig::default());
    host.take_next_completion().unwrap().complete_registered();
    controller.process_pending();
    controller.cancel_current();
    controller.apply_saved(ShortcutConfig::default());
    let pending = host.take_next_completion().unwrap();
    let duplicate = ShortcutSnapshot::new(pending.config(), 1, Ok(()), Ok(()));
    pending.complete(Ok(duplicate));
    controller.process_pending();
    assert!(controller.view().message.contains("invalid"));
    host.emit(trigger(1));
    controller.process_pending();
}

#[test]
fn complete_save_finishes_capture_and_unpauses_new_saved_authority_atomically() {
    use tessera_system::shortcuts::KeyModifiers;

    let host = Arc::new(RecordingShortcutHost::new());
    let provider = host.clone();
    let controller = ShortcutsController::new(
        ShortcutConfig::default(),
        ShortcutsBindings {
            factory: Rc::new(move || Ok(Some(provider.clone()))),
            is_current: Rc::new(|| true),
            project: Rc::new(|_| {}),
            deliver: Rc::new(|_| {}),
            wake: Arc::new(|| {}),
        },
    );
    controller.apply_saved(ShortcutConfig::default());
    host.take_next_completion().unwrap().complete_registered();
    controller.process_pending();
    controller.set_paused(true);
    host.take_next_completion().unwrap().complete_registered();
    controller.process_pending();
    assert!(controller.view().pause_applied);
    controller.begin_capture();
    let chord = KeyChord::new(
        KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        },
        0x4c,
    )
    .unwrap();
    let saved = ShortcutConfig::default()
        .with_settings_override(Some(chord))
        .unwrap();
    let before = host.configurations().len();
    controller.apply_saved(saved.clone());
    assert_eq!(host.configurations().len(), before + 1);
    assert_eq!(host.configurations().last(), Some(&saved));
    assert!(host.configurations().last().unwrap().enabled());
    assert!(!controller.view().capturing);
    assert!(!controller.view().pause_applied);
    host.take_next_completion().unwrap().complete_registered();
    controller.process_pending();
    assert!(!controller.view().dirty);
    assert!(controller.view().settings_status.starts_with("Registered"));
}

#[test]
fn async_trigger_getter_rejects_pause_save_and_undrained_provider_fault() {
    let host = Arc::new(RecordingShortcutHost::new());
    let provider = host.clone();
    let controller = ShortcutsController::new(
        ShortcutConfig::default(),
        ShortcutsBindings {
            factory: Rc::new(move || Ok(Some(provider.clone()))),
            is_current: Rc::new(|| true),
            project: Rc::new(|_| {}),
            deliver: Rc::new(|_| {}),
            wake: Arc::new(|| {}),
        },
    );
    controller.apply_saved(ShortcutConfig::default());
    let pending = host.take_next_completion().unwrap();
    let original = ShortcutTrigger {
        generation: pending.generation(),
        action: ShortcutAction::ToggleLauncher,
        cursor: None,
    };
    pending.complete_registered();
    controller.process_pending();
    assert!(controller.accepts_trigger(&original));
    controller.set_paused(true);
    assert!(!controller.accepts_trigger(&original));
    host.take_next_completion().unwrap().complete_registered();
    controller.process_pending();
    controller.apply_saved(ShortcutConfig::default());
    assert!(!controller.accepts_trigger(&original));
    let pending = host.take_next_completion().unwrap();
    let current = ShortcutTrigger {
        generation: pending.generation(),
        ..original
    };
    pending.complete_registered();
    controller.process_pending();
    assert!(controller.accepts_trigger(&current));
    host.emit(ShortcutEvent::Unavailable(ShortcutError::new(
        ShortcutErrorKind::Other,
        None,
        "Provider unavailable",
    )));
    assert!(
        !controller.accepts_trigger(&current),
        "fault retires authority before its queued UI status"
    );
    controller.process_pending();
    assert!(!controller.accepts_trigger(&current));
}

#[test]
fn async_trigger_getter_checks_current_predicate_reentry_after_return() {
    let slot: Rc<RefCell<std::rc::Weak<ShortcutsController>>> =
        Rc::new(RefCell::new(std::rc::Weak::new()));
    let revoke = Rc::new(Cell::new(false));
    let should_revoke = revoke.clone();
    let weak = slot.clone();
    let host = Arc::new(RecordingShortcutHost::new());
    let provider = host.clone();
    let controller = Rc::new(ShortcutsController::new(
        ShortcutConfig::default(),
        ShortcutsBindings {
            factory: Rc::new(move || Ok(Some(provider.clone()))),
            is_current: Rc::new(move || {
                if should_revoke.replace(false) {
                    weak.borrow().upgrade().unwrap().cancel_current();
                }
                true
            }),
            project: Rc::new(|_| {}),
            deliver: Rc::new(|_| {}),
            wake: Arc::new(|| {}),
        },
    ));
    *slot.borrow_mut() = Rc::downgrade(&controller);
    controller.apply_saved(ShortcutConfig::default());
    let pending = host.take_next_completion().unwrap();
    let trigger = ShortcutTrigger {
        generation: pending.generation(),
        action: ShortcutAction::ToggleLauncher,
        cursor: None,
    };
    pending.complete_registered();
    controller.process_pending();
    assert!(controller.accepts_trigger(&trigger));
    revoke.set(true);
    assert!(!controller.accepts_trigger(&trigger));
}
