// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

const ACTIONS: [(PowerMenuAction, &str, PowerAction); 6] = [
    (
        PowerMenuAction::LockSession,
        "Lock session",
        PowerAction::LockSession,
    ),
    (PowerMenuAction::LogOut, "Log out", PowerAction::LogOut),
    (
        PowerMenuAction::PowerOff,
        "Power off",
        PowerAction::PowerOff {
            updates: PowerUpdatePolicy::OmitExplicitInstallation,
        },
    ),
    (
        PowerMenuAction::Reboot,
        "Reboot",
        PowerAction::Reboot {
            updates: PowerUpdatePolicy::OmitExplicitInstallation,
        },
    ),
    (PowerMenuAction::Suspend, "Suspend", PowerAction::Suspend),
    (
        PowerMenuAction::Hibernate,
        "Hibernate",
        PowerAction::Hibernate,
    ),
];
const UPDATES_LABEL: &str = "There's pending system updates. Install now?";

fn click(fixture: &Fixture, label: &str) {
    let element = ElementHandle::find_by_accessible_label(&fixture.component(), label)
        .next()
        .unwrap_or_else(|| panic!("genuine native control: {label}"));
    let origin = element.absolute_position();
    let size = element.size();
    assert!(size.width > 0.0 && size.height > 0.0);
    let position = LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    let component = fixture.component();
    let window = component.window();
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn performed(fixture: &Fixture) -> Vec<PowerAction> {
    fixture
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::Perform(action) => Some(action),
            _ => None,
        })
        .collect()
}

fn updates_reply(fixture: &Fixture, reply: Reply<PowerUpdateHint, PowerUpdatesError>) {
    fixture.updates.state.lock().replies.push_back(reply);
}

fn choose(fixture: &Fixture, choice: bool) {
    fixture.component().set_install_updates(choice);
    fixture.component().invoke_updates_choice_changed(choice);
}

fn press(fixture: &Fixture, key: Key) {
    fixture
        .component()
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
}

fn release(fixture: &Fixture, key: Key) {
    fixture
        .component()
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
}

#[test]
fn all_six_genuine_pointer_controls_record_exact_native_intents() {
    i_slint_backend_testing::init_no_event_loop();
    for (_, label, expected) in ACTIONS {
        let fixture = Fixture::on_installed_platform();
        fixture.loaded();
        click(&fixture, label);
        assert_eq!(performed(&fixture), vec![expected], "{label}");
        assert!(!fixture.popup.is_visible());
        assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
        assert_eq!(fixture.opened.get(), 1, "no confirmation presentation");
    }
}

#[test]
fn all_six_genuine_tab_return_and_space_release_reject_repeat() {
    i_slint_backend_testing::init_no_event_loop();
    for (index, (_, label, expected)) in ACTIONS.into_iter().enumerate() {
        for key in [Key::Return, Key::Space] {
            let fixture = Fixture::on_installed_platform();
            fixture.loaded();
            fixture.component().invoke_focus_content();
            for _ in 0..=index {
                fixture.key(Key::Tab);
            }
            press(&fixture, key);
            fixture
                .component()
                .window()
                .dispatch_event(WindowEvent::KeyPressRepeated { text: key.into() });
            if key == Key::Space {
                assert!(performed(&fixture).is_empty(), "{label}: Space only arms");
            }
            release(&fixture, key);
            press(&fixture, key);
            release(&fixture, key);
            assert_eq!(
                performed(&fixture),
                vec![expected],
                "{label}: fresh key only"
            );
            assert_eq!(fixture.power.state.lock().maximum_active, 1);
        }
    }
}

#[test]
fn accepted_action_blocks_all_six_until_terminal_even_after_reopen() {
    i_slint_backend_testing::init_no_event_loop();
    for (action, label, expected) in ACTIONS {
        let fixture = Fixture::on_installed_platform();
        fixture.loaded();
        click(&fixture, label);
        fixture.loaded();
        assert!(fixture.component().get_lock_busy());
        for (other, other_label, _) in ACTIONS {
            click(&fixture, other_label);
            fixture.component().invoke_action_requested(other);
        }
        assert_eq!(performed(&fixture), vec![expected]);
        assert_eq!(fixture.power.state.lock().maximum_active, 1);
        finish(&fixture.power.state, Ok(PowerRequestAccepted));
        fixture.drain();
        assert!(!fixture.component().get_lock_busy());
        fixture.component().invoke_action_requested(action);
        assert_eq!(performed(&fixture), vec![expected, expected]);
    }
}

#[test]
fn all_six_require_actual_visible_current_authorized_input() {
    i_slint_backend_testing::init_no_event_loop();
    for (action, _, _) in ACTIONS {
        let fixture = Fixture::on_installed_platform();
        fixture.component().invoke_action_requested(action);
        assert!(fixture.popup.show(Theme::Dark).unwrap());
        fixture.component().invoke_action_requested(action);
        assert!(
            performed(&fixture).is_empty(),
            "initial metadata has not authorized input"
        );
        finish(&fixture.display.state, Ok(Some(layout())));
        fixture.drain();
        fixture.component().set_action_enabled(false);
        fixture.component().invoke_action_requested(action);
        assert!(performed(&fixture).is_empty());
        fixture.component().set_action_enabled(true);
        fixture.component().hide().unwrap();
        fixture.component().invoke_action_requested(action);
        assert!(
            performed(&fixture).is_empty(),
            "domain visibility is not native visibility"
        );
        fixture.popup.hide();
        fixture.component().invoke_action_requested(action);
        assert!(performed(&fixture).is_empty());
        assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn update_policy_requires_known_pending_and_choice_only_for_off_and_reboot() {
    i_slint_backend_testing::init_no_event_loop();
    for hint in [
        Ok(PowerUpdateHint::Pending),
        Ok(PowerUpdateHint::NotDetected),
        Err(PowerUpdatesError::Unavailable),
    ] {
        for choice in [true, false] {
            for (action, _, ordinary) in ACTIONS {
                let fixture = Fixture::on_installed_platform();
                updates_reply(&fixture, Reply::Inline(hint));
                fixture.loaded();
                choose(&fixture, choice);
                fixture.component().invoke_action_requested(action);
                let install = hint == Ok(PowerUpdateHint::Pending) && choice;
                let expected = match action {
                    PowerMenuAction::PowerOff if install => PowerAction::PowerOff {
                        updates: PowerUpdatePolicy::RequestInstallation,
                    },
                    PowerMenuAction::Reboot if install => PowerAction::Reboot {
                        updates: PowerUpdatePolicy::RequestInstallation,
                    },
                    _ => ordinary,
                };
                assert_eq!(performed(&fixture), vec![expected]);
            }
        }
    }
}

#[test]
fn native_pending_checkbox_pointer_space_and_tab_change_choice_without_command() {
    let fixture = Fixture::new();
    updates_reply(&fixture, Reply::Inline(Ok(PowerUpdateHint::Pending)));
    fixture.loaded();
    assert!(fixture.component().get_install_updates());
    click(&fixture, UPDATES_LABEL);
    assert!(!fixture.component().get_install_updates());
    fixture.component().invoke_focus_content();
    fixture.key(Key::Tab);
    fixture.key(Key::Space);
    assert!(fixture.component().get_install_updates());
    assert!(performed(&fixture).is_empty());
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(
        performed(&fixture),
        vec![PowerAction::LockSession],
        "checkbox precedes six-button grid"
    );
}

#[test]
fn unknown_hint_is_factual_not_false_and_does_not_disable_native_commands() {
    let fixture = Fixture::new();
    updates_reply(
        &fixture,
        Reply::Inline(Err(PowerUpdatesError::AccessDenied)),
    );
    fixture.loaded();
    assert!(!fixture.component().get_updates_known_pending());
    let status = fixture.component().get_updates_status().to_string();
    assert!(status.starts_with("System update hint unavailable:"));
    assert!(!status.to_lowercase().contains("no updates"));
    assert!(
        ElementHandle::find_by_accessible_label(&fixture.component(), UPDATES_LABEL)
            .next()
            .is_none()
    );
    assert!(fixture.component().get_action_enabled());
    click(&fixture, "Reboot");
    assert_eq!(
        performed(&fixture),
        vec![PowerAction::Reboot {
            updates: PowerUpdatePolicy::OmitExplicitInstallation
        }]
    );
}

#[test]
fn initial_display_and_updates_each_must_finish_before_first_presentation() {
    i_slint_backend_testing::init_no_event_loop();
    for updates_first in [true, false] {
        let fixture = Fixture::on_installed_platform();
        updates_reply(&fixture, Reply::Delayed);
        fixture.popup.show(Theme::Dark).unwrap();
        if updates_first {
            finish(&fixture.updates.state, Ok(PowerUpdateHint::Pending));
        } else {
            finish(&fixture.display.state, Ok(Some(layout())));
        }
        fixture.drain();
        assert!(!fixture.popup.is_visible());
        assert_eq!(fixture.opened.get(), 0);
        assert!(fixture.sibling.window().is_visible());
        if updates_first {
            finish(&fixture.display.state, Ok(Some(layout())));
        } else {
            finish(&fixture.updates.state, Err(PowerUpdatesError::Unavailable));
        }
        fixture.drain();
        assert!(fixture.popup.is_visible());
        assert_eq!(fixture.opened.get(), 1);
    }
}

#[test]
fn later_hung_status_refresh_does_not_await_or_replay_focus_attachment_or_block_commands() {
    let fixture = Fixture::new();
    updates_reply(&fixture, Reply::Inline(Ok(PowerUpdateHint::Pending)));
    fixture.loaded();
    choose(&fixture, false);
    updates_reply(&fixture, Reply::Delayed);
    fixture.popup.show(Theme::Dark).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert!(!fixture.component().get_install_updates());
    assert!(fixture.component().get_updates_known_pending());
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|event| **event == Event::Attach)
            .count(),
        1
    );
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|event| **event == Event::Focus)
            .count(),
        1
    );
    assert_eq!(fixture.opened.get(), 1);
    click(&fixture, "Power off");
    assert_eq!(
        performed(&fixture),
        vec![PowerAction::PowerOff {
            updates: PowerUpdatePolicy::OmitExplicitInstallation
        }]
    );
    assert_eq!(
        fixture.updates.state.lock().active,
        1,
        "readonly hang is independent of mutation flight"
    );
    finish(&fixture.power.state, Ok(PowerRequestAccepted));
    fixture.drain();
    assert_eq!(*fixture.results.borrow(), vec![Ok(())]);
}

#[test]
fn off_and_reboot_capture_policy_before_lease_drop_factory_and_perform_reentry() {
    i_slint_backend_testing::init_no_event_loop();
    for point in [Hook::LeaseDrop, Hook::PowerFactory, Hook::Perform] {
        for action in [PowerMenuAction::PowerOff, PowerMenuAction::Reboot] {
            for choice in [true, false] {
                let fixture = Fixture::on_installed_platform();
                updates_reply(&fixture, Reply::Inline(Ok(PowerUpdateHint::Pending)));
                fixture.loaded();
                choose(&fixture, choice);
                let component = fixture.component();
                hook(point, move || {
                    component.set_install_updates(!choice);
                    component.invoke_updates_choice_changed(!choice);
                    component.set_updates_known_pending(false);
                });
                fixture.component().invoke_action_requested(action);
                let updates = if choice {
                    PowerUpdatePolicy::RequestInstallation
                } else {
                    PowerUpdatePolicy::OmitExplicitInstallation
                };
                let expected = match action {
                    PowerMenuAction::PowerOff => PowerAction::PowerOff { updates },
                    _ => PowerAction::Reboot { updates },
                };
                assert_eq!(performed(&fixture), vec![expected]);
            }
        }
    }
}

#[test]
fn late_status_completion_cannot_rewrite_reserved_or_accepted_native_intent() {
    i_slint_backend_testing::init_no_event_loop();
    for hint in [PowerUpdateHint::Pending, PowerUpdateHint::NotDetected] {
        for point in [Hook::LeaseDrop, Hook::PowerFactory, Hook::Perform] {
            let fixture = Fixture::on_installed_platform();
            fixture.loaded();
            updates_reply(&fixture, Reply::Delayed);
            fixture.popup.show(Theme::Dark).unwrap();
            finish(&fixture.display.state, Ok(Some(layout())));
            fixture.drain();
            let updates = fixture.updates.clone();
            let popup = fixture.popup.clone();
            hook(point, move || {
                finish(&updates.state, Ok(hint));
                popup.process_events();
            });
            click(&fixture, "Reboot");
            assert_eq!(
                performed(&fixture),
                vec![PowerAction::Reboot {
                    updates: PowerUpdatePolicy::OmitExplicitInstallation
                }]
            );
            fixture.drain();
            assert_eq!(performed(&fixture).len(), 1);
        }
    }
}

#[test]
fn updates_none_and_factory_errors_retry_but_successful_provider_is_cached_across_read_errors() {
    i_slint_backend_testing::init_no_event_loop();
    for error in [None, Some(PowerUpdatesError::AccessDenied)] {
        let fixture = Fixture::on_installed_platform();
        *fixture.host.updates.lock() = match error {
            None => Ok(None),
            Some(error) => Err(error),
        };
        fixture.loaded();
        assert!(!fixture.component().get_updates_status().is_empty());
        assert_eq!(fixture.updates.state.lock().calls, 0);
        fixture.popup.hide();
        *fixture.host.updates.lock() = Ok(Some(fixture.updates.clone()));
        updates_reply(&fixture, Reply::Rejected(PowerUpdatesError::Busy));
        fixture.loaded();
        assert!(!fixture.component().get_updates_status().is_empty());
        assert_eq!(fixture.host.updates_getters.load(Ordering::Relaxed), 2);
        fixture.popup.hide();
        *fixture.host.updates.lock() = Err(PowerUpdatesError::Unavailable);
        updates_reply(
            &fixture,
            Reply::Inline(Err(PowerUpdatesError::AccessDenied)),
        );
        fixture.loaded();
        assert!(!fixture.component().get_updates_status().is_empty());
        fixture.popup.hide();
        updates_reply(&fixture, Reply::Inline(Ok(PowerUpdateHint::Pending)));
        fixture.loaded();
        assert!(fixture.component().get_updates_known_pending());
        assert!(fixture.component().get_updates_status().is_empty());
        assert_eq!(fixture.host.updates_getters.load(Ordering::Relaxed), 2);
        assert_eq!(fixture.updates.state.lock().calls, 3);
    }
}

struct CallbackThenReject(Result<PowerUpdateHint, PowerUpdatesError>);
impl PowerUpdatesHost for CallbackThenReject {
    fn read(&self, completion: PowerUpdatesCompletion) -> Result<(), PowerUpdatesError> {
        completion(self.0);
        Err(PowerUpdatesError::AccessDenied)
    }
}

#[test]
fn updates_inline_callback_is_first_terminal_even_when_read_also_returns_error() {
    let fixture = Fixture::new();
    *fixture.host.updates.lock() = Ok(Some(Arc::new(CallbackThenReject(Ok(
        PowerUpdateHint::Pending,
    )))));
    fixture.loaded();
    assert!(fixture.component().get_updates_known_pending());
    assert!(fixture.component().get_updates_status().is_empty());
    fixture.drain();
    assert!(fixture.component().get_updates_known_pending());
    click(&fixture, "Update and restart");
    assert_eq!(
        performed(&fixture),
        vec![PowerAction::Reboot {
            updates: PowerUpdatePolicy::RequestInstallation
        }]
    );
}

#[test]
fn updates_stale_read_and_many_refreshes_coalesce_to_one_current_followup() {
    let fixture = Fixture::new();
    updates_reply(&fixture, Reply::Delayed);
    fixture.popup.show(Theme::Dark).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    for _ in 0..8 {
        fixture.popup.hide();
        fixture.popup.show(Theme::Dark).unwrap();
    }
    assert_eq!(fixture.updates.state.lock().calls, 1);
    updates_reply(&fixture, Reply::Delayed);
    finish(&fixture.updates.state, Ok(PowerUpdateHint::Pending));
    fixture.drain();
    assert!(
        !fixture.popup.is_visible(),
        "stale result cannot satisfy fresh initial metadata"
    );
    assert!(!fixture.component().get_updates_known_pending());
    assert_eq!(fixture.updates.state.lock().calls, 2);
    assert_eq!(fixture.updates.state.lock().maximum_active, 1);
    // The old display request also retires before the bounded fresh display read.
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    finish(&fixture.updates.state, Ok(PowerUpdateHint::NotDetected));
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert!(!fixture.component().get_updates_known_pending());
    assert!(fixture.component().get_updates_status().is_empty());
    assert_eq!(fixture.updates.state.lock().calls, 2);
}

#[test]
fn updates_read_reentry_cannot_overlap_or_publish_old_generation() {
    let fixture = Fixture::new();
    updates_reply(&fixture, Reply::Delayed);
    let popup = fixture.popup.clone();
    hook(Hook::UpdatesRead, move || {
        popup.hide();
        popup.show(Theme::Dark).unwrap();
    });
    fixture.popup.show(Theme::Dark).unwrap();
    assert_eq!(fixture.updates.state.lock().calls, 1);
    updates_reply(&fixture, Reply::Delayed);
    finish(&fixture.updates.state, Ok(PowerUpdateHint::Pending));
    fixture.drain();
    assert_eq!(fixture.updates.state.lock().calls, 2);
    assert_eq!(fixture.updates.state.lock().maximum_active, 1);
    assert!(!fixture.component().get_updates_known_pending());
    assert!(!fixture.popup.is_visible());
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    finish(&fixture.updates.state, Ok(PowerUpdateHint::Pending));
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert!(fixture.component().get_updates_known_pending());
}

#[test]
fn updates_factory_reentry_retires_reserved_read_and_cached_success_serves_fresh_scope() {
    let fixture = Fixture::new();
    let popup = fixture.popup.clone();
    hook(Hook::UpdatesFactory, move || {
        popup.hide();
        popup.show(Theme::Dark).unwrap();
    });
    fixture.popup.show(Theme::Dark).unwrap();
    assert_eq!(
        fixture.updates.state.lock().calls,
        0,
        "old factory reservation cannot submit"
    );
    fixture.drain();
    assert_eq!(fixture.updates.state.lock().calls, 1);
    assert_eq!(fixture.host.updates_getters.load(Ordering::Relaxed), 1);
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(fixture.popup.is_visible());
}

#[test]
fn pending_choice_survives_not_detected_and_late_status_never_replays_focus_or_attach() {
    let fixture = Fixture::new();
    updates_reply(&fixture, Reply::Inline(Ok(PowerUpdateHint::Pending)));
    fixture.loaded();
    choose(&fixture, false);
    updates_reply(&fixture, Reply::Delayed);
    fixture.popup.show(Theme::Dark).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    let before = fixture.events();
    finish(&fixture.updates.state, Ok(PowerUpdateHint::NotDetected));
    fixture.drain();
    assert!(!fixture.component().get_updates_known_pending());
    assert!(!fixture.component().get_install_updates());
    assert!(fixture.component().get_updates_status().is_empty());
    assert_eq!(
        fixture.events(),
        before,
        "status publication is not presentation replay"
    );
    assert!(
        ElementHandle::find_by_accessible_label(&fixture.component(), UPDATES_LABEL)
            .next()
            .is_none()
    );
    updates_reply(&fixture, Reply::Delayed);
    fixture.popup.show(Theme::Dark).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    let before = fixture.events();
    finish(&fixture.updates.state, Ok(PowerUpdateHint::Pending));
    fixture.drain();
    assert!(fixture.component().get_updates_known_pending());
    assert!(!fixture.component().get_install_updates());
    assert_eq!(fixture.events(), before);
    click(&fixture, "Reboot");
    assert_eq!(
        performed(&fixture),
        vec![PowerAction::Reboot {
            updates: PowerUpdatePolicy::OmitExplicitInstallation
        }]
    );
}

#[test]
fn pending_checkbox_disabled_or_hidden_input_cannot_change_reserved_command_policy() {
    let fixture = Fixture::new();
    updates_reply(&fixture, Reply::Inline(Ok(PowerUpdateHint::Pending)));
    fixture.loaded();
    fixture.component().set_action_enabled(false);
    click(&fixture, UPDATES_LABEL);
    fixture.component().invoke_focus_content();
    fixture.key(Key::Tab);
    fixture.key(Key::Space);
    assert!(
        fixture.component().get_install_updates(),
        "disabled checkbox ignores genuine input"
    );
    assert!(performed(&fixture).is_empty());
    fixture.component().set_action_enabled(true);
    click(&fixture, "Update and restart");
    choose(&fixture, false);
    updates_reply(&fixture, Reply::Inline(Ok(PowerUpdateHint::Pending)));
    fixture.loaded();
    assert!(
        fixture.component().get_install_updates(),
        "hidden/command-flight injected toggle has no domain authority"
    );
    click(&fixture, UPDATES_LABEL);
    fixture.component().invoke_focus_content();
    fixture.key(Key::Tab);
    fixture.key(Key::Space);
    assert!(fixture.component().get_install_updates());
    assert_eq!(
        performed(&fixture),
        vec![PowerAction::Reboot {
            updates: PowerUpdatePolicy::RequestInstallation
        }]
    );
}

#[test]
fn updates_inline_error_callback_wins_over_different_immediate_error() {
    let fixture = Fixture::new();
    *fixture.host.updates.lock() = Ok(Some(Arc::new(CallbackThenReject(Err(
        PowerUpdatesError::Busy,
    )))));
    fixture.loaded();
    assert_eq!(
        fixture.component().get_updates_status().as_str(),
        "System update hint unavailable: A pending-update hint read is already in progress"
    );
    fixture.drain();
    assert!(!fixture.component().get_updates_known_pending());
    click(&fixture, "Power off");
    assert_eq!(
        performed(&fixture),
        vec![PowerAction::PowerOff {
            updates: PowerUpdatePolicy::OmitExplicitInstallation
        }]
    );
}
