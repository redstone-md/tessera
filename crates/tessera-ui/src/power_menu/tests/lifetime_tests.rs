// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::power_menu::mailbox::{revision, wake_failed};
use i_slint_backend_testing::{TestingBackend, TestingBackendOptions};
use slint::platform::{Platform, WindowAdapter};

fn elapsed(seconds: u64) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(seconds));
}

fn pending(fixture: &Fixture) {
    fixture
        .updates
        .state
        .lock()
        .replies
        .push_back(Reply::Inline(Ok(PowerUpdateHint::Pending)));
}

fn choose(fixture: &Fixture, value: bool) {
    let component = fixture.component();
    component.set_install_updates(value);
    component.invoke_updates_choice_changed(value);
}

fn trace_current(fixture: &Fixture) {
    *fixture.host.trace.root.lock() = Some(fixture.component().as_weak());
}

fn click(component: &PowerMenuSurface, label: &str) {
    let element = ElementHandle::find_by_accessible_label(component, label)
        .next()
        .unwrap();
    let origin = element.absolute_position();
    let size = element.size();
    let position = LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    component
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
    component
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
}

#[test]
fn lifetime_constructor_hidden_does_not_arm_source_retirement() {
    let fixture = Fixture::new();
    assert!(!fixture.popup.retirement_timer.running());
    let epoch = fixture.popup.state.borrow().epoch;
    elapsed(60);
    assert!(fixture.popup.surface_snapshot().is_some());
    assert_eq!(fixture.popup.state.borrow().epoch, epoch);
    assert!(!fixture.popup.is_visible());
    assert_eq!(fixture.display.state.lock().calls, 0);
    assert_eq!(fixture.updates.state.lock().calls, 0);
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
}

#[test]
fn lifetime_actual_thirty_seconds_retires_ownership_and_recreation_resets_choice_only_on_publish() {
    let fixture = Fixture::new();
    pending(&fixture);
    fixture.loaded();
    let green = fixture.component().presentation_theme();
    let seed = crate::SourceSeed::from_rgb(0x4d90fe).unwrap();
    let blue = PresentationTheme::from_source(slint::language::ColorScheme::Dark, seed);
    let events = fixture.events();
    fixture.popup.set_theme(blue.clone());
    assert_eq!(fixture.component().presentation_theme(), blue);
    assert_ne!(fixture.component().presentation_theme(), green);
    assert_eq!(
        fixture.events(),
        events,
        "same-scheme seed preview has no native effects"
    );
    choose(&fixture, false);
    fixture.popup.set_user_name("Genuine Account");
    let old = fixture.component();
    let epoch = fixture.popup.state.borrow().epoch;
    fixture.popup.hide();
    elapsed(29);
    assert!(fixture.popup.surface_snapshot().is_some());
    assert!(!fixture.component().get_install_updates());
    elapsed(1);
    assert!(fixture.popup.surface_snapshot().is_none());
    assert_eq!(fixture.popup.state.borrow().epoch, epoch + 1);
    assert!(!old.window().is_visible());
    assert!(
        !fixture.popup.state.borrow().install_updates,
        "retirement itself is not component creation"
    );
    fixture
        .updates
        .state
        .lock()
        .replies
        .push_back(Reply::Delayed);
    let blue_light = PresentationTheme::from_source(slint::language::ColorScheme::Light, seed);
    assert!(fixture.popup.show(blue_light.clone()).unwrap());
    assert_eq!(fixture.component().presentation_theme(), blue_light);
    trace_current(&fixture);
    assert!(fixture.component().get_install_updates());
    assert_eq!(fixture.component().get_user_name(), "Genuine Account");
    assert!(!fixture.component().get_updates_known_pending());
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(
        !fixture.popup.is_visible(),
        "new epoch must await its own first update outcome"
    );
    finish(&fixture.updates.state, Ok(PowerUpdateHint::Pending));
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert_eq!(fixture.component().presentation_theme(), blue_light);
    assert!(fixture.component().get_install_updates());
    assert_eq!(fixture.opened.get(), 2);
}

#[test]
fn lifetime_repeated_completed_hide_restarts_full_deadline_even_already_hidden() {
    let fixture = Fixture::new();
    fixture.loaded();
    fixture.popup.hide();
    elapsed(27);
    fixture.popup.hide();
    elapsed(3);
    assert!(
        fixture.popup.surface_snapshot().is_some(),
        "old first-hide deadline was cancelled"
    );
    elapsed(26);
    assert!(fixture.popup.surface_snapshot().is_some());
    elapsed(1);
    assert!(fixture.popup.surface_snapshot().is_none());
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
}

#[test]
fn lifetime_new_show_cancels_expiry_before_metadata_effects_and_keeps_same_component_choice() {
    let fixture = Fixture::new();
    pending(&fixture);
    fixture.loaded();
    choose(&fixture, false);
    let epoch = fixture.popup.state.borrow().epoch;
    fixture.popup.hide();
    elapsed(29);
    fixture
        .updates
        .state
        .lock()
        .replies
        .push_back(Reply::Delayed);
    assert!(fixture.popup.show(Theme::Dark).unwrap());
    assert!(!fixture.popup.retirement_timer.running());
    elapsed(35);
    assert!(fixture.popup.surface_snapshot().is_some());
    assert_eq!(fixture.popup.state.borrow().epoch, epoch);
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(
        fixture.popup.is_visible(),
        "same source lifetime does not await refresh"
    );
    assert!(!fixture.component().get_install_updates());
    finish(&fixture.updates.state, Ok(PowerUpdateHint::NotDetected));
    fixture.drain();
    assert!(!fixture.component().get_updates_known_pending());
    assert!(
        !fixture.component().get_install_updates(),
        "not detected hides control, never rewrites choice"
    );
}

#[test]
fn lifetime_none_projection_retains_bounded_real_name_without_implicit_surface_or_provider_reads() {
    let fixture = Fixture::new();
    fixture.popup.hide();
    elapsed(30);
    assert!(fixture.popup.surface_snapshot().is_none());
    let name = format!("  {}\n  ", "A".repeat(400));
    fixture.popup.set_user_name(&name);
    fixture.popup.set_theme(Theme::Light);
    fixture.popup.disable_motion();
    fixture.popup.update_motion();
    fixture.popup.refresh_display();
    fixture.drain();
    fixture.popup.hide();
    elapsed(60);
    assert!(fixture.popup.surface_snapshot().is_none());
    assert!(!fixture.popup.retirement_timer.running());
    assert!(!fixture.popup.work_timer.running());
    assert_eq!(fixture.popup.state.borrow().user_name.chars().count(), 256);
    assert_eq!(fixture.display.state.lock().calls, 0);
    assert_eq!(fixture.updates.state.lock().calls, 0);
    assert_eq!(fixture.host.display_getters.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.host.updates_getters.load(Ordering::Relaxed), 0);
    fixture.loaded();
    assert_eq!(fixture.component().get_user_name().chars().count(), 256);
}

#[test]
fn lifetime_accepted_suspend_survives_retirement_and_held_terminal_drains_before_reopen_busy() {
    let fixture = Fixture::new();
    fixture.loaded();
    fixture
        .component()
        .invoke_action_requested(PowerMenuAction::Suspend);
    assert_eq!(fixture.power.state.lock().active, 1);
    elapsed(30);
    assert!(fixture.popup.surface_snapshot().is_none());
    assert!(fixture.popup.state.borrow().lock.is_some());
    finish(&fixture.power.state, Ok(PowerRequestAccepted));
    assert!(fixture.popup.mailbox.lock().lock.terminal.is_some());
    assert!(!fixture.popup.mailbox.lock().wake_queued);
    assert!(!fixture.popup.work_timer.running());
    assert!(
        fixture.results.borrow().is_empty(),
        "receipt may be held while presentationless"
    );
    fixture
        .updates
        .state
        .lock()
        .replies
        .push_back(Reply::Delayed);
    fixture.popup.show(Theme::Light).unwrap();
    trace_current(&fixture);
    assert!(fixture.popup.state.borrow().lock.is_none());
    assert!(!fixture.component().get_lock_busy());
    assert_eq!(fixture.results.borrow().as_slice(), [Ok(())]);
    assert_eq!(fixture.power.state.lock().calls, 1);
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    finish(&fixture.updates.state, Ok(PowerUpdateHint::NotDetected));
    fixture.drain();
    fixture
        .component()
        .invoke_action_requested(PowerMenuAction::Hibernate);
    assert_eq!(fixture.power.state.lock().calls, 2);
    assert_eq!(fixture.power.state.lock().maximum_active, 1);
}

#[test]
fn lifetime_old_accepted_observations_retire_flights_but_never_fulfill_new_epoch_initial_barrier() {
    let fixture = Fixture::new();
    fixture
        .updates
        .state
        .lock()
        .replies
        .push_back(Reply::Delayed);
    fixture.popup.show(Theme::Dark).unwrap();
    fixture.popup.hide();
    elapsed(30);
    assert!(fixture.popup.surface_snapshot().is_none());
    assert_eq!(fixture.display.state.lock().active, 1);
    assert_eq!(fixture.updates.state.lock().active, 1);
    fixture
        .updates
        .state
        .lock()
        .replies
        .push_back(Reply::Delayed);
    fixture.popup.show(Theme::Light).unwrap();
    trace_current(&fixture);
    finish(&fixture.display.state, Ok(Some(layout())));
    finish(&fixture.updates.state, Ok(PowerUpdateHint::Pending));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().get_updates_known_pending());
    assert_eq!(fixture.display.state.lock().calls, 2);
    assert_eq!(fixture.updates.state.lock().calls, 2);
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(
        !fixture.popup.is_visible(),
        "fresh display does not borrow old status outcome"
    );
    finish(&fixture.updates.state, Err(PowerUpdatesError::AccessDenied));
    fixture.drain();
    assert!(
        fixture.popup.is_visible(),
        "honest Unknown is a resolved first outcome"
    );
    assert!(!fixture.component().get_updates_status().is_empty());
    assert_eq!(fixture.display.state.lock().maximum_active, 1);
    assert_eq!(fixture.updates.state.lock().maximum_active, 1);
}

#[test]
fn lifetime_old_held_generated_pointer_return_space_hide_viewport_close_reject_new_surface_authority()
 {
    let fixture = Fixture::new();
    fixture.loaded();
    let old = fixture.component();
    fixture.popup.hide();
    elapsed(30);
    fixture.loaded();
    let generation = fixture.popup.state.borrow().generation;
    let events = fixture.events();
    // An external test holder can keep/show the old physical generated object.
    // This is not production ownership and must never regain domain authority.
    old.set_action_enabled(true);
    old.set_lock_busy(false);
    old.show().unwrap();
    click(&old, "Log out");
    old.invoke_focus_content();
    old.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Tab.into(),
    });
    old.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Tab.into(),
    });
    old.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    old.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    old.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Space.into(),
    });
    old.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Space.into(),
    });
    old.invoke_action_requested(PowerMenuAction::Hibernate);
    old.invoke_updates_choice_changed(false);
    old.invoke_hide_requested();
    old.invoke_viewport_changed();
    old.window().dispatch_event(WindowEvent::CloseRequested);
    i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
    assert!(fixture.popup.is_visible());
    assert_eq!(fixture.popup.state.borrow().generation, generation);
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.events(), events);
    old.hide().unwrap();
}

#[test]
fn lifetime_cancelled_native_attachment_is_not_hidden_proof_and_deadline_starts_after_unwind() {
    let fixture = Fixture::new();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::Configure, move || {
        let popup = weak.upgrade().unwrap();
        popup.hide();
        assert!(popup.component().window().is_visible());
        assert!(!popup.retirement_timer.running());
        elapsed(31);
        assert!(popup.surface_snapshot().is_some());
        assert!(popup.component().window().is_visible());
        assert!(!popup.retirement_timer.running());
    });
    fixture.popup.show(Theme::Dark).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert!(!fixture.component().window().is_visible());
    assert!(fixture.popup.retirement_timer.running());
    elapsed(29);
    assert!(fixture.popup.surface_snapshot().is_some());
    elapsed(1);
    assert!(fixture.popup.surface_snapshot().is_none());
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.opened.get(), 0);
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn lifetime_old_wake_enqueue_failure_cannot_clear_new_current_target_queued_flag() {
    let fixture = Fixture::new();
    let mailbox = Arc::new(Mutex::new(Mailbox::default()));
    mailbox
        .lock()
        .install(1, fixture.component().as_weak())
        .unwrap();
    let old_revision = revision(&mailbox);
    mailbox.lock().wake_queued = true;
    mailbox.lock().retire(1);
    mailbox
        .lock()
        .install(2, fixture.component().as_weak())
        .unwrap();
    let current_revision = revision(&mailbox);
    mailbox.lock().wake_queued = true;
    wake_failed(&mailbox, old_revision);
    assert!(
        mailbox.lock().wake_queued,
        "failed old A must not clear queued B"
    );
    mailbox.lock().retire(1);
    assert!(
        mailbox.lock().wake_queued,
        "old retirement cannot clear newer target"
    );
    wake_failed(&mailbox, current_revision);
    assert!(!mailbox.lock().wake_queued);
}

#[test]
fn lifetime_close_revokes_epochs_before_lease_drop_and_never_joins_or_resurrects_work() {
    let fixture = Fixture::new();
    fixture.loaded();
    fixture
        .updates
        .state
        .lock()
        .replies
        .push_back(Reply::Delayed);
    fixture.popup.show(Theme::Light).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    let old = fixture.component();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::LeaseDrop, move || {
        let popup = weak.upgrade().unwrap();
        assert!(popup.state.borrow().closed);
        assert!(popup.surface_snapshot().is_none());
        assert!(popup.show(Theme::Dark).is_err());
        old.invoke_action_requested(PowerMenuAction::Reboot);
        old.invoke_hide_requested();
    });
    fixture.popup.close();
    assert!(fixture.popup.surface_snapshot().is_none());
    assert_eq!(
        fixture.updates.state.lock().active,
        1,
        "close did not join accepted readonly work"
    );
    finish(&fixture.updates.state, Ok(PowerUpdateHint::Pending));
    fixture.drain();
    elapsed(60);
    assert!(!fixture.popup.retirement_timer.running());
    assert!(!fixture.popup.fit_timer.running());
    assert!(!fixture.popup.work_timer.running());
    assert!(!fixture.popup.focus_watch.running());
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert!(fixture.results.borrow().is_empty());
}

// Public SDK Platform seam provides genuine component-construction reentry and
// factory failure; this does not fake a generated callback or native OS effect.
struct ConstructionPlatform {
    backend: TestingBackend,
    next: Rc<RefCell<Option<UiHook>>>,
    reject: Rc<Cell<bool>>,
}

impl Platform for ConstructionPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let next = self.next.borrow_mut().take();
        if let Some(next) = next {
            next();
        }
        if self.reject.replace(false) {
            return Err("recording component constructor failure".into());
        }
        self.backend.create_window_adapter()
    }
    fn duration_since_start(&self) -> Duration {
        self.backend.duration_since_start()
    }
}

type ConstructionFixture = (Fixture, Rc<RefCell<Option<UiHook>>>, Rc<Cell<bool>>);

fn construction_fixture() -> ConstructionFixture {
    let next = Rc::new(RefCell::new(None));
    let reject = Rc::new(Cell::new(false));
    slint::platform::set_platform(Box::new(ConstructionPlatform {
        backend: TestingBackend::new(TestingBackendOptions {
            mock_time: true,
            threading: false,
        }),
        next: next.clone(),
        reject: reject.clone(),
    }))
    .unwrap();
    (Fixture::on_installed_platform(), next, reject)
}

#[test]
fn lifetime_constructor_publication_checks_show_generation_not_only_reserved_epoch() {
    let (fixture, next, _) = construction_fixture();
    fixture.popup.hide();
    elapsed(30);
    let epoch = fixture.popup.state.borrow().epoch;
    fixture.popup.state.borrow_mut().install_updates = false;
    let weak = Rc::downgrade(&fixture.popup);
    *next.borrow_mut() = Some(Box::new(move || weak.upgrade().unwrap().hide()));
    assert!(!fixture.popup.show(Theme::Dark).unwrap());
    assert!(fixture.popup.surface_snapshot().is_none());
    assert_eq!(
        fixture.popup.state.borrow().epoch,
        epoch + 1,
        "candidate reserved one epoch; reentrant hide changed only generation"
    );
    assert!(
        !fixture.popup.state.borrow().install_updates,
        "discarded constructor must not reset choice"
    );
    assert!(!fixture.popup.retirement_timer.running());
    fixture.loaded();
    assert!(fixture.component().get_install_updates());
}

#[test]
fn lifetime_outer_constructor_never_overwrites_inner_published_replacement() {
    let (fixture, next, _) = construction_fixture();
    fixture.popup.hide();
    elapsed(30);
    let prior_epoch = fixture.popup.state.borrow().epoch;
    let weak = Rc::downgrade(&fixture.popup);
    *next.borrow_mut() = Some(Box::new(move || {
        let popup = weak.upgrade().unwrap();
        assert!(popup.show(Theme::Light).unwrap());
    }));
    assert!(!fixture.popup.show(Theme::Dark).unwrap());
    trace_current(&fixture);
    assert!(fixture.popup.surface_snapshot().is_some());
    assert_eq!(
        fixture.popup.state.borrow().epoch,
        prior_epoch + 2,
        "unpublished outer and published inner candidates cannot share input epoch"
    );
    assert_eq!(
        fixture.popup.state.borrow().theme,
        PresentationTheme::from(Theme::Light)
    );
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert_eq!(fixture.opened.get(), 1);
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn lifetime_constructor_failure_does_not_reset_retained_domain_choice_or_create_idle_timer() {
    let (fixture, _, reject) = construction_fixture();
    fixture.popup.hide();
    elapsed(30);
    {
        let mut state = fixture.popup.state.borrow_mut();
        state.install_updates = false;
        state.updates_hint = Some(PowerUpdateHint::Pending);
    }
    reject.set(true);
    assert!(fixture.popup.show(Theme::Dark).is_err());
    assert!(fixture.popup.surface_snapshot().is_none());
    assert!(!fixture.popup.state.borrow().install_updates);
    assert_eq!(
        fixture.popup.state.borrow().updates_hint,
        Some(PowerUpdateHint::Pending)
    );
    assert!(!fixture.popup.retirement_timer.running());
    assert_eq!(fixture.display.state.lock().calls, 0);
    fixture.loaded();
    assert!(fixture.component().get_install_updates());
}

#[test]
fn lifetime_epoch_identity_exhaustion_closes_without_wrap_or_native_entry() {
    let fixture = Fixture::new();
    fixture.popup.state.borrow_mut().epoch = u64::MAX;
    // The owned surface remains real, but its callbacks intentionally lose the
    // original epoch. An explicit hide may still safely retire this owner.
    fixture.popup.hide();
    elapsed(30);
    assert!(fixture.popup.state.borrow().closed);
    assert!(fixture.popup.surface_snapshot().is_none());
    assert!(fixture.popup.show(Theme::Light).is_err());
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 0);
}

#[test]
fn lifetime_wake_identity_exhaustion_closes_new_candidate_without_native_entry() {
    let fixture = Fixture::new();
    fixture.popup.hide();
    elapsed(30);
    crate::power_menu::mailbox::exhaust_revision(&fixture.popup.mailbox);
    assert!(fixture.popup.show(Theme::Light).is_err());
    assert!(fixture.popup.state.borrow().closed);
    assert!(fixture.popup.surface_snapshot().is_none());
    assert_eq!(revision(&fixture.popup.mailbox), u64::MAX);
    assert_eq!(fixture.display.state.lock().calls, 0);
    assert_eq!(fixture.updates.state.lock().calls, 0);
    assert_eq!(fixture.power.state.lock().calls, 0);
}

#[test]
fn lifetime_retirement_inside_factory_cannot_lend_none_hidden_authority_to_reserved_command() {
    let fixture = Fixture::new();
    fixture.loaded();
    let weak = Rc::downgrade(&fixture.popup);
    hook(Hook::PowerFactory, move || {
        elapsed(30);
        let popup = weak.upgrade().unwrap();
        assert!(popup.surface_snapshot().is_none());
        assert!(popup.state.borrow().lock.is_some());
    });
    fixture
        .component()
        .invoke_action_requested(PowerMenuAction::Reboot);
    assert_eq!(fixture.host.power_getters.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert!(fixture.popup.state.borrow().lock.is_none());
    assert!(fixture.results.borrow().is_empty());
    fixture.loaded();
    fixture
        .component()
        .invoke_action_requested(PowerMenuAction::Hibernate);
    assert_eq!(
        fixture.host.power_getters.load(Ordering::Relaxed),
        1,
        "success-only provider cache survives domain lifetime"
    );
    assert_eq!(fixture.power.state.lock().calls, 1);
}

#[test]
fn lifetime_completed_initial_metadata_is_not_reawaited_after_native_attachment_failure() {
    let fixture = Fixture::new();
    fixture.host.deny_attachment.store(true, Ordering::Relaxed);
    fixture.popup.show(Theme::Dark).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(!fixture.popup.is_visible());
    assert!(
        fixture.popup.state.borrow().initialized,
        "metadata initialization is independent of native attachment success"
    );
    fixture.host.deny_attachment.store(false, Ordering::Relaxed);
    fixture
        .updates
        .state
        .lock()
        .replies
        .push_back(Reply::Delayed);
    fixture.popup.show(Theme::Light).unwrap();
    finish(&fixture.display.state, Ok(Some(layout())));
    fixture.drain();
    assert!(fixture.popup.is_visible());
    assert_eq!(
        fixture.updates.state.lock().active,
        1,
        "same component's refresh is non-awaited"
    );
    assert_eq!(fixture.opened.get(), 1);
    assert_eq!(fixture.host.trace.leases.load(Ordering::Relaxed), 1);
}

#[test]
fn lifetime_constructor_close_reentry_rejects_publication_without_joining_old_observations() {
    let (fixture, next, _) = construction_fixture();
    fixture
        .updates
        .state
        .lock()
        .replies
        .push_back(Reply::Delayed);
    fixture.popup.show(Theme::Dark).unwrap();
    fixture.popup.hide();
    elapsed(30);
    let weak = Rc::downgrade(&fixture.popup);
    *next.borrow_mut() = Some(Box::new(move || weak.upgrade().unwrap().close()));
    assert!(!fixture.popup.show(Theme::Light).unwrap());
    assert!(fixture.popup.state.borrow().closed);
    assert!(fixture.popup.surface_snapshot().is_none());
    assert_eq!(fixture.display.state.lock().active, 1);
    assert_eq!(fixture.updates.state.lock().active, 1);
    finish(&fixture.display.state, Ok(Some(layout())));
    finish(&fixture.updates.state, Ok(PowerUpdateHint::Pending));
    fixture.drain();
    elapsed(60);
    assert!(fixture.popup.surface_snapshot().is_none());
    assert!(fixture.results.borrow().is_empty());
    assert_eq!(fixture.opened.get(), 0);
    assert_eq!(fixture.power.state.lock().calls, 0);
    assert!(!fixture.popup.work_timer.running());
    assert!(!fixture.popup.retirement_timer.running());
}

#[test]
fn lifetime_cache_only_display_provider_survives_retirement_without_discovery_or_native_read() {
    let fixture = Fixture::new();
    assert!(fixture.popup.display_provider().is_none());
    assert_eq!(fixture.host.display_getters.load(Ordering::Relaxed), 0);
    fixture.loaded();
    let first = fixture.popup.display_provider().unwrap();
    fixture.popup.hide();
    elapsed(30);
    assert!(fixture.popup.surface_snapshot().is_none());
    let after = fixture.popup.display_provider().unwrap();
    assert!(Arc::ptr_eq(&first, &after));
    assert_eq!(fixture.host.display_getters.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.display.state.lock().calls, 1);
    fixture.popup.close();
    assert!(fixture.popup.display_provider().is_none());
    assert_eq!(fixture.display.state.lock().calls, 1);
}
