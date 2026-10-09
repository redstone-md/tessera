// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::power_menu::PowerMenuController;
use tessera_core::Rect;
use tessera_system::display_context::{
    DisplayContextCompletion, DisplayContextError, DisplayContextHost, DisplayLayout,
    DisplaySelection,
};
use tessera_system::power::{
    PowerAction, PowerCompletion, PowerError, PowerHost, PowerRequestAccepted,
};

thread_local! {
    static POWER_PERFORM_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
}

type DisplayResult = Result<Option<DisplayLayout>, DisplayContextError>;

#[derive(Clone, Copy)]
enum DisplayReply {
    Ready(DisplayLayout),
    Delayed,
    Failure(DisplayContextError),
    None,
}

struct RecordingDisplayHost {
    reply: Mutex<DisplayReply>,
    reads: AtomicUsize,
    completion: Mutex<Option<DisplayContextCompletion>>,
}

impl RecordingDisplayHost {
    fn new(reply: DisplayReply) -> Arc<Self> {
        Arc::new(Self {
            reply: Mutex::new(reply),
            reads: AtomicUsize::new(0),
            completion: Mutex::default(),
        })
    }

    fn complete(&self, result: DisplayResult) {
        let completion = self
            .completion
            .lock()
            .take()
            .expect("accepted display read");
        completion(result);
    }
}

impl DisplayContextHost for RecordingDisplayHost {
    fn read(&self, completion: DisplayContextCompletion) -> Result<(), DisplayContextError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let reply = *self.reply.lock();
        match reply {
            DisplayReply::Ready(layout) => completion(Ok(Some(layout))),
            DisplayReply::None => completion(Ok(None)),
            DisplayReply::Failure(error) => completion(Err(error)),
            DisplayReply::Delayed => {
                assert!(self.completion.lock().replace(completion).is_none());
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct RecordingPowerHost {
    actions: Mutex<Vec<PowerAction>>,
    completion: Mutex<Option<PowerCompletion>>,
    inline: bool,
}

impl RecordingPowerHost {
    fn complete(&self) {
        let completion = self
            .completion
            .lock()
            .take()
            .expect("accepted Lock request");
        completion(Ok(PowerRequestAccepted));
    }
}

impl PowerHost for RecordingPowerHost {
    fn perform(&self, action: PowerAction, completion: PowerCompletion) -> Result<(), PowerError> {
        self.actions.lock().push(action);
        let hook = POWER_PERFORM_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
        if self.inline {
            completion(Ok(PowerRequestAccepted));
        } else {
            assert!(self.completion.lock().replace(completion).is_none());
        }
        Ok(())
    }
}

fn desktop_layout() -> DisplayLayout {
    DisplayLayout::new(
        Rect::new(-2240, -360, 4160, 1560).unwrap(),
        Rect::new(-1920, -240, 1920, 1080).unwrap(),
        1.5625,
        DisplaySelection::Primary,
    )
    .unwrap()
}

fn fixture_with_power(
    reply: DisplayReply,
    inline: bool,
) -> (
    LauncherFixture,
    Arc<RecordingDisplayHost>,
    Arc<RecordingPowerHost>,
) {
    let fixture = LauncherFixture::new();
    let display = RecordingDisplayHost::new(reply);
    let power = Arc::new(RecordingPowerHost {
        inline,
        ..Default::default()
    });
    *fixture.host.display_provider.lock() = Some(display.clone());
    *fixture.host.power_provider.lock() = Some(power.clone());
    fixture.controller.open_launcher();
    fixture.toolbar.set_clock("Native clock fixture".into());
    assert!(fixture.launcher.window().is_visible());
    (fixture, display, power)
}

// Register the actual component before draining inline mailbox delivery so the
// parent fixture can distinguish Power's native attachment from other popups.
fn current_power(fixture: &LauncherFixture) -> Rc<PowerMenuController> {
    let actor = fixture
        .controller
        .power_menu
        .borrow()
        .clone()
        .expect("cached Power");
    POWER_WINDOW.with(|window| *window.borrow_mut() = Some(actor.component().as_weak()));
    actor
}

fn drain_power(actor: &PowerMenuController) {
    actor.process_events();
    slint::platform::update_timers_and_animations();
    actor.process_events();
}

fn open_power(fixture: &LauncherFixture) -> Rc<PowerMenuController> {
    fixture.click_launcher("Open power menu");
    let actor = current_power(fixture);
    drain_power(&actor);
    assert!(actor.is_visible());
    assert!(actor.component().window().is_visible());
    actor
}

fn assert_hidden(actor: &PowerMenuController) {
    assert!(!actor.is_visible());
    assert!(!actor.component().window().is_visible());
}

fn present_toolbar_tooltip(fixture: &LauncherFixture) {
    use i_slint_backend_testing::ElementHandle;
    assert!(fixture.toolbar.window().is_visible());
    let clock = ElementHandle::find_by_accessible_label(&fixture.toolbar, "Open calendar")
        .next()
        .unwrap();
    let size = clock.size();
    let attached = Rc::new(Cell::new(false));
    TOOLTIP_CONFIGURE_HOOK.with(|hook| {
        let attached = Rc::clone(&attached);
        *hook.borrow_mut() = Some(Box::new(move |window| {
            assert!(window.is_visible());
            attached.set(true);
        }));
    });
    fixture.controller.show_tooltip(
        &fixture.toolbar,
        SurfaceKind::Toolbar,
        "Calendar",
        crate::generated::TileBounds {
            origin: clock.absolute_position(),
            width: size.width,
            height: size.height,
        },
        crate::tooltip::Side::Bottom,
    );
    advance_recycle_timer(101);
    assert!(attached.get(), "the real Tooltip must show and attach");
    TOOLTIP_CONFIGURE_HOOK.with(|hook| assert!(hook.borrow().is_none()));
}

fn activate_power_with_live_toolbar_tooltip(fixture: &LauncherFixture) {
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::{PointerEventButton, WindowEvent};
    let footer = ElementHandle::find_by_accessible_label(&fixture.launcher, "Open power menu")
        .next()
        .unwrap();
    let position = native_center(&footer);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
    // A primitive press/hover must not consume the retirement seam. The
    // different Toolbar owner keeps this lease for root open_power_menu.
    TOOLTIP_DROP_HOOK.with(|hook| assert!(hook.borrow().is_some()));
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    TOOLTIP_DROP_HOOK.with(|hook| assert!(hook.borrow().is_none()));
}

#[test]
fn genuine_power_footer_pointer_and_tab_return_query_without_command_or_sibling_routes() {
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::Key;
    let (fixture, display, power) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let unrelated = fixture.host.unrelated_activity();
    for label in [
        "Open user menu",
        "Open settings and recovery",
        "Open power menu",
        "Refresh the desktop",
        "Exit Tessera",
    ] {
        assert!(
            ElementHandle::find_by_accessible_label(&fixture.launcher, label)
                .next()
                .is_some()
        );
    }
    let actor = open_power(&fixture);
    assert_eq!(display.reads.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
    assert!(power.actions.lock().is_empty());
    assert!(!fixture.panel.window().is_visible());
    assert!(fixture.controller.user_menu.borrow().is_none());
    actor.hide();
    // The footer click leaves native focus on Power. Back to Settings, then
    // forward to Power, verifies the real footer keyboard route independently.
    fixture
        .launcher
        .window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
    native_key(&fixture.launcher, Key::Tab);
    fixture
        .launcher
        .window()
        .dispatch_event(slint::platform::WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
    native_key(&fixture.launcher, Key::Tab);
    native_key(&fixture.launcher, Key::Return);
    drain_power(&actor);
    assert!(actor.is_visible());
    assert!(actor.component().window().is_visible());
    assert_eq!(display.reads.load(Ordering::SeqCst), 2);
    assert!(power.actions.lock().is_empty());
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.host.unrelated_activity(), unrelated);
}

#[test]
fn delayed_failed_and_empty_display_preserve_actual_user_until_power_presents() {
    for terminal in [
        Ok(Some(desktop_layout())),
        Err(DisplayContextError::Unavailable),
        Ok(None),
    ] {
        let (fixture, display, power) = fixture_with_power(DisplayReply::Delayed, false);
        fixture.click_launcher("Open user menu");
        let user = fixture.controller.user_menu.borrow().clone().unwrap();
        assert!(user.is_open());
        assert!(user.component().window().is_visible());
        fixture.click_launcher("Open power menu");
        let actor = current_power(&fixture);
        assert_hidden(&actor);
        assert!(user.is_open());
        assert!(user.component().window().is_visible());
        display.complete(terminal);
        drain_power(&actor);
        if terminal.is_ok_and(|layout| layout.is_some()) {
            assert!(actor.is_visible());
            assert!(actor.component().window().is_visible());
            assert!(!user.is_open());
            assert!(!user.component().window().is_visible());
        } else {
            assert_hidden(&actor);
            assert!(user.is_open());
            assert!(user.component().window().is_visible());
        }
        assert!(power.actions.lock().is_empty());
        assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
    }
    // Also exercise successful inline empty/error completions, not only held ones.
    for reply in [
        DisplayReply::None,
        DisplayReply::Failure(DisplayContextError::Native { code: 5 }),
    ] {
        let (fixture, _, _) = fixture_with_power(reply, false);
        fixture.click_launcher("Open user menu");
        let user = fixture.controller.user_menu.borrow().clone().unwrap();
        fixture.click_launcher("Open power menu");
        let actor = current_power(&fixture);
        drain_power(&actor);
        assert_hidden(&actor);
        assert!(user.is_open());
        assert!(user.component().window().is_visible());
    }
}

#[test]
fn power_projects_source_desktop_and_selected_negative_fractional_monitor_at_actual_root_scale() {
    let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    fixture
        .launcher
        .window()
        .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
    fixture.controller.render();
    let actor = open_power(&fixture);
    let surface = actor.component();
    assert_eq!(surface.window().scale_factor(), 1.0);
    assert_eq!(
        surface.window().size(),
        slint::PhysicalSize::new(4160, 1560)
    );
    assert_eq!(surface.get_selected_x(), 320.0);
    assert_eq!(surface.get_selected_y(), 120.0);
    assert_eq!(surface.get_selected_width(), 1920.0);
    assert_eq!(surface.get_selected_height(), 1080.0);
    assert_eq!(surface.get_metric_scale(), 1.5625);
    assert!(power.actions.lock().is_empty());
}

#[test]
fn genuine_lock_pointer_return_space_detach_before_factory_and_perform_exactly_once() {
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::{Key, WindowEvent};
    for input in 0..3 {
        let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), true);
        let actor = open_power(&fixture);
        let unrelated = fixture.host.unrelated_activity();
        for fake in [
            "Log out",
            "Power off",
            "Reboot",
            "Suspend",
            "Hibernate",
            "Confirm",
        ] {
            assert!(
                ElementHandle::find_by_accessible_label(actor.component(), fake)
                    .next()
                    .is_none()
            );
        }
        let retired = Rc::new(Cell::new(false));
        POWER_FACTORY_HOOK.with(|hook| {
            let actor = Rc::clone(&actor);
            let drops = Arc::clone(&fixture.host.power_lease_drops);
            let retired = Rc::clone(&retired);
            *hook.borrow_mut() = Some(Box::new(move || {
                assert_hidden(&actor);
                assert_eq!(drops.load(Ordering::SeqCst), 1);
                retired.set(true);
            }));
        });
        POWER_PERFORM_HOOK.with(|hook| {
            let actor = Rc::clone(&actor);
            let retired = Rc::clone(&retired);
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(retired.get());
                assert_hidden(&actor);
            }));
        });
        if input == 0 {
            click_component(actor.component(), "Lock session");
        } else {
            native_key(actor.component(), Key::Tab);
            if input == 1 {
                native_key(actor.component(), Key::Return);
                actor
                    .component()
                    .window()
                    .dispatch_event(WindowEvent::KeyPressRepeated {
                        text: Key::Return.into(),
                    });
            } else {
                actor
                    .component()
                    .window()
                    .dispatch_event(WindowEvent::KeyPressed {
                        text: Key::Space.into(),
                    });
                actor
                    .component()
                    .window()
                    .dispatch_event(WindowEvent::KeyPressRepeated {
                        text: Key::Space.into(),
                    });
                assert!(
                    power.actions.lock().is_empty(),
                    "Space only arms before release"
                );
                actor
                    .component()
                    .window()
                    .dispatch_event(WindowEvent::KeyReleased {
                        text: Key::Space.into(),
                    });
            }
        }
        drain_power(&actor);
        assert_hidden(&actor);
        assert!(retired.get());
        assert_eq!(power.actions.lock().as_slice(), [PowerAction::LockSession]);
        assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.host.unrelated_activity(), unrelated);
        assert!(!fixture.panel.window().is_visible());
        assert!(fixture.controller.user_menu.borrow().is_none());
    }
}

#[test]
fn accepted_lock_survives_launcher_hide_reopen_rejects_busy_input_and_never_replays() {
    use slint::platform::Key;
    let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let actor = open_power(&fixture);
    click_component(actor.component(), "Lock session");
    assert_hidden(&actor);
    assert_eq!(power.actions.lock().len(), 1);
    fixture.controller.hide_launcher();
    fixture.controller.open_launcher();
    let reopened = open_power(&fixture);
    assert!(Rc::ptr_eq(&actor, &reopened));
    assert!(actor.component().get_lock_busy());
    assert!(!actor.component().get_action_enabled());
    click_component(actor.component(), "Lock session");
    native_key(actor.component(), Key::Tab);
    native_key(actor.component(), Key::Return);
    native_key(actor.component(), Key::Space);
    assert_eq!(power.actions.lock().len(), 1);
    power.complete();
    drain_power(&actor);
    assert!(actor.is_visible());
    assert!(actor.component().window().is_visible());
    assert!(!actor.component().get_lock_busy());
    assert!(actor.component().get_action_enabled());
    assert_eq!(
        power.actions.lock().len(),
        1,
        "completion never replays the busy input"
    );
}

#[test]
fn stale_hidden_footer_and_reentrant_query_factory_hide_cannot_resurrect_power() {
    let (fixture, display, power) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    fixture.controller.hide_launcher();
    fixture.launcher.invoke_open_power_menu_requested();
    assert!(fixture.controller.power_menu.borrow().is_none());
    assert_eq!(
        fixture.host.display_provider_calls.load(Ordering::SeqCst),
        0
    );
    fixture.controller.open_launcher();
    POWER_DISPLAY_FACTORY_HOOK.with(|hook| {
        let controller = fixture.controller.clone();
        *hook.borrow_mut() = Some(Box::new(move || controller.hide_launcher()));
    });
    fixture.click_launcher("Open power menu");
    let actor = current_power(&fixture);
    drain_power(&actor);
    assert_hidden(&actor);
    assert!(!fixture.launcher.window().is_visible());
    assert_eq!(display.reads.load(Ordering::SeqCst), 0);
    assert!(power.actions.lock().is_empty());
}

#[test]
fn late_display_after_launcher_hide_or_actor_close_never_presents() {
    for close in [false, true] {
        let (fixture, display, power) = fixture_with_power(DisplayReply::Delayed, false);
        fixture.click_launcher("Open power menu");
        let actor = current_power(&fixture);
        if close {
            actor.close();
        } else {
            fixture.controller.hide_launcher();
        }
        display.complete(Ok(Some(desktop_layout())));
        drain_power(&actor);
        assert_hidden(&actor);
        assert_eq!(fixture.host.power_lease_drops.load(Ordering::SeqCst), 0);
        assert!(power.actions.lock().is_empty());
    }
}

#[test]
fn power_lease_drop_reopens_same_actor_and_rejects_retired_lock_intent() {
    let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let actor = open_power(&fixture);
    POWER_DROP_HOOK.with(|hook| {
        let launcher = fixture.launcher.as_weak();
        let cache = Rc::clone(&fixture.controller.power_menu);
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(cache.try_borrow_mut().is_ok());
            assert!(launcher.upgrade().unwrap().window().is_visible());
            click_component(&launcher.upgrade().unwrap(), "Open power menu");
            let replacement = cache.borrow().clone().unwrap();
            drain_power(&replacement);
            assert!(replacement.is_visible());
            assert!(replacement.component().window().is_visible());
        }));
    });
    click_component(actor.component(), "Lock session");
    drain_power(&actor);
    assert!(actor.is_visible());
    assert!(actor.component().window().is_visible());
    assert!(power.actions.lock().is_empty());
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn power_factory_reopens_same_actor_without_locking_the_replacement_scope() {
    let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let actor = open_power(&fixture);
    POWER_FACTORY_HOOK.with(|hook| {
        let launcher = fixture.launcher.as_weak();
        let actor = Rc::clone(&actor);
        *hook.borrow_mut() = Some(Box::new(move || {
            assert_hidden(&actor);
            click_component(&launcher.upgrade().unwrap(), "Open power menu");
            drain_power(&actor);
            assert!(actor.is_visible());
            assert!(actor.component().window().is_visible());
        }));
    });
    click_component(actor.component(), "Lock session");
    drain_power(&actor);
    assert!(actor.is_visible());
    assert!(actor.component().window().is_visible());
    assert!(power.actions.lock().is_empty());
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn different_user_popup_during_power_retirement_invalidates_old_lock_even_when_power_hidden() {
    let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let actor = open_power(&fixture);
    let user_cache = Rc::clone(&fixture.controller.user_menu);
    POWER_DROP_HOOK.with(|hook| {
        let launcher = fixture.launcher.as_weak();
        let power_cache = Rc::clone(&fixture.controller.power_menu);
        let user_cache = Rc::clone(&user_cache);
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(power_cache.try_borrow_mut().is_ok());
            click_component(&launcher.upgrade().unwrap(), "Open user menu");
            let user = user_cache.borrow().clone().unwrap();
            assert!(user.is_open());
            assert!(user.component().window().is_visible());
        }));
    });
    click_component(actor.component(), "Lock session");
    drain_power(&actor);
    assert_hidden(&actor);
    let user = user_cache.borrow().clone().unwrap();
    assert!(user.is_open());
    assert!(user.component().window().is_visible());
    assert!(power.actions.lock().is_empty());
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn theme_and_unchanged_launcher_refit_preserve_power_without_reads_or_focus_replay() {
    let (fixture, display, power) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let actor = open_power(&fixture);
    actor.disable_motion();
    let size = actor.component().window().size();
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let drops = fixture.host.power_lease_drops.load(Ordering::SeqCst);
    let activity = fixture.host.unrelated_activity();
    actor.set_theme(Theme::Light);
    actor.update_motion();
    fixture.controller.render();
    fixture.controller.update_geometry();
    drain_power(&actor);
    assert!(actor.is_visible());
    assert!(actor.component().window().is_visible());
    assert_eq!(actor.component().window().size(), size);
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), focus);
    assert_eq!(fixture.host.power_lease_drops.load(Ordering::SeqCst), drops);
    assert_eq!(display.reads.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.host.unrelated_activity(), activity);
    assert!(power.actions.lock().is_empty());
}

#[test]
fn scoped_root_drop_releases_power_actor_and_panel_while_accepted_completion_is_held() {
    let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let actor = open_power(&fixture);
    let actor_weak = Rc::downgrade(&actor);
    let power_weak = actor.component().as_weak();
    let panel_weak = fixture.panel.as_weak();
    click_component(actor.component(), "Lock session");
    assert!(power.completion.lock().is_some());
    assert_hidden(&actor);
    drop(actor);
    drop(fixture);
    assert!(actor_weak.upgrade().is_none());
    assert!(power_weak.upgrade().is_none());
    assert!(panel_weak.upgrade().is_none());
    power.complete();
    slint::platform::update_timers_and_animations();
    assert!(actor_weak.upgrade().is_none());
    assert!(panel_weak.upgrade().is_none());
    assert_eq!(power.actions.lock().as_slice(), [PowerAction::LockSession]);
}

#[test]
fn newer_actual_calendar_during_user_retirement_wins_over_old_power_coordination() {
    let (fixture, display, power) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    // Prime the real later cache, so the old dismissal already owns its clone.
    // The Calendar can honestly show pending/unavailable; no fake date is needed.
    click_component(&fixture.toolbar, "Open calendar");
    let calendar = fixture.controller.calendar.borrow().clone().unwrap();
    assert!(calendar.is_open());
    assert!(calendar.component().window().is_visible());
    calendar.hide();
    fixture.click_launcher("Open user menu");
    let user = fixture.controller.user_menu.borrow().clone().unwrap();
    assert!(user.is_open());
    assert!(user.component().window().is_visible());
    RECYCLE_MENU_DROP_HOOK.with(|hook| {
        let toolbar = fixture.toolbar.as_weak();
        let cache = Rc::clone(&fixture.controller.calendar);
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(cache.try_borrow_mut().is_ok());
            click_component(&toolbar.upgrade().unwrap(), "Open calendar");
            let replacement = cache.borrow().clone().unwrap();
            assert!(replacement.is_open());
            assert!(replacement.component().window().is_visible());
        }));
    });
    fixture.click_launcher("Open power menu");
    let actor = current_power(&fixture);
    drain_power(&actor);
    assert_hidden(&actor);
    assert!(!user.is_open());
    assert!(!user.component().window().is_visible());
    assert!(
        calendar.is_open(),
        "an older coordinator cannot close its newer replacement"
    );
    assert!(calendar.component().window().is_visible());
    assert_eq!(display.reads.load(Ordering::SeqCst), 1);
    assert!(power.actions.lock().is_empty());
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
    RECYCLE_MENU_DROP_HOOK.with(|hook| assert!(hook.borrow().is_none()));
}

#[test]
fn toolbar_tooltip_retirement_hides_or_reopens_launcher_and_revokes_old_power_footer() {
    for cached in [false, true] {
        for reopen in [false, true] {
            let (fixture, display, power) =
                fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
            let actor = cached.then(|| {
                let actor = open_power(&fixture);
                actor.hide();
                actor
            });
            let reads = display.reads.load(Ordering::SeqCst);
            let getters = fixture.host.display_provider_calls.load(Ordering::SeqCst);
            present_toolbar_tooltip(&fixture);
            let operation = fixture.controller.popup_operation.borrow().clone();
            let retired = Rc::new(Cell::new(false));
            TOOLTIP_DROP_HOOK.with(|hook| {
                let controller = fixture.controller.clone();
                let launcher = fixture.launcher.as_weak();
                let operation = Rc::clone(&operation);
                let retired = Rc::clone(&retired);
                *hook.borrow_mut() = Some(Box::new(move || {
                    assert!(controller.tooltips.try_borrow_mut().is_ok());
                    assert!(controller.power_menu.try_borrow_mut().is_ok());
                    assert!(controller.launcher_popup_ready());
                    assert!(launcher.upgrade().unwrap().window().is_visible());
                    assert!(Rc::ptr_eq(&operation, &controller.popup_operation.borrow()));
                    controller.hide_launcher();
                    assert!(!launcher.upgrade().unwrap().window().is_visible());
                    assert!(!controller.launcher_popup_ready());
                    if reopen {
                        controller.open_launcher();
                        assert!(controller.launcher_popup_ready());
                        assert!(launcher.upgrade().unwrap().window().is_visible());
                        assert!(!Rc::ptr_eq(
                            &operation,
                            &controller.popup_operation.borrow()
                        ));
                    }
                    retired.set(true);
                }));
            });
            POWER_CONFIGURE_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(|_| panic!("stale Power attached")));
            });
            activate_power_with_live_toolbar_tooltip(&fixture);
            assert!(retired.get());
            assert_eq!(fixture.launcher.window().is_visible(), reopen);
            if reopen {
                assert!(!Rc::ptr_eq(
                    &operation,
                    &fixture.controller.popup_operation.borrow()
                ));
            } else {
                assert!(!fixture.controller.launcher_popup_ready());
            }
            if let Some(actor) = actor {
                drain_power(&actor);
                assert_hidden(&actor);
                assert!(Rc::ptr_eq(
                    &actor,
                    fixture.controller.power_menu.borrow().as_ref().unwrap()
                ));
            } else {
                assert!(fixture.controller.power_menu.borrow().is_none());
            }
            assert_eq!(display.reads.load(Ordering::SeqCst), reads);
            assert_eq!(
                fixture.host.display_provider_calls.load(Ordering::SeqCst),
                getters
            );
            assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
            assert!(power.actions.lock().is_empty());
            assert!(POWER_CONFIGURE_HOOK.with(|hook| hook.borrow_mut().take().is_some()));
        }
    }
}

#[test]
fn toolbar_tooltip_retirement_opens_newer_actual_calendar_without_stale_power() {
    let (fixture, display, power) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    present_toolbar_tooltip(&fixture);
    let operation = fixture.controller.popup_operation.borrow().clone();
    let retired = Rc::new(Cell::new(false));
    TOOLTIP_DROP_HOOK.with(|hook| {
        let controller = fixture.controller.clone();
        let toolbar = fixture.toolbar.as_weak();
        let launcher = fixture.launcher.as_weak();
        let operation = Rc::clone(&operation);
        let retired = Rc::clone(&retired);
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(controller.tooltips.try_borrow_mut().is_ok());
            assert!(controller.calendar.try_borrow_mut().is_ok());
            assert!(controller.power_menu.try_borrow_mut().is_ok());
            assert!(controller.launcher_popup_ready());
            assert!(launcher.upgrade().unwrap().window().is_visible());
            assert!(Rc::ptr_eq(&operation, &controller.popup_operation.borrow()));
            click_component(&toolbar.upgrade().unwrap(), "Open calendar");
            let calendar = controller.calendar.borrow().clone().unwrap();
            assert!(calendar.is_open());
            assert!(calendar.component().window().is_visible());
            assert!(controller.launcher_popup_ready());
            assert!(launcher.upgrade().unwrap().window().is_visible());
            assert!(!Rc::ptr_eq(
                &operation,
                &controller.popup_operation.borrow()
            ));
            retired.set(true);
        }));
    });
    POWER_CONFIGURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|_| panic!("stale Power attached")));
    });
    activate_power_with_live_toolbar_tooltip(&fixture);
    slint::platform::update_timers_and_animations();
    assert!(retired.get());
    assert!(fixture.controller.launcher_popup_ready());
    assert!(fixture.launcher.window().is_visible());
    assert!(!Rc::ptr_eq(
        &operation,
        &fixture.controller.popup_operation.borrow()
    ));
    let calendar = fixture.controller.calendar.borrow().clone().unwrap();
    assert!(calendar.is_open());
    assert!(calendar.component().window().is_visible());
    assert!(fixture.controller.power_menu.borrow().is_none());
    assert_eq!(display.reads.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture.host.display_provider_calls.load(Ordering::SeqCst),
        0
    );
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
    assert!(power.actions.lock().is_empty());
    assert!(POWER_CONFIGURE_HOOK.with(|hook| hook.borrow_mut().take().is_some()));
}

#[test]
fn scoped_root_drop_closes_power_admission_before_actual_power_lease_footer_and_old_lock_input() {
    let (fixture, display, power) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let actor = open_power(&fixture);
    let actor_weak = Rc::downgrade(&actor);
    let power_weak = actor.component().as_weak();
    let launcher_weak = fixture.launcher.as_weak();
    let cache = Rc::clone(&fixture.controller.power_menu);
    let closed = Rc::clone(&fixture.controller.power_admission_closed);
    let host = Arc::clone(&fixture.host);
    let retired = Rc::new(Cell::new(false));
    POWER_DROP_HOOK.with(|hook| {
        let actor_weak = actor_weak.clone();
        let launcher_weak = launcher_weak.clone();
        let cache = Rc::clone(&cache);
        let closed = Rc::clone(&closed);
        let retired = Rc::clone(&retired);
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(closed.get(), "admission closes before taking the cache");
            assert!(cache.try_borrow_mut().is_ok());
            assert!(cache.borrow().is_none());
            let launcher = launcher_weak.upgrade().unwrap();
            assert!(launcher.window().is_visible());
            click_component(&launcher, "Open power menu");
            assert!(cache.borrow().is_none());
            let actor = actor_weak.upgrade().unwrap();
            assert!(!actor.is_visible());
            assert!(
                actor.component().window().is_visible(),
                "dispatch old Lock while its native lease is still retiring"
            );
            click_component(actor.component(), "Lock session");
            assert!(cache.borrow().is_none());
            retired.set(true);
        }));
    });
    POWER_CONFIGURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|_| panic!("teardown Power attached")));
    });
    drop(actor);
    drop(fixture);
    slint::platform::update_timers_and_animations();
    assert!(retired.get());
    assert!(closed.get());
    assert!(cache.borrow().is_none());
    assert!(actor_weak.upgrade().is_none());
    assert!(power_weak.upgrade().is_none());
    assert!(launcher_weak.upgrade().is_none());
    assert_eq!(host.power_lease_drops.load(Ordering::SeqCst), 1);
    assert_eq!(display.reads.load(Ordering::SeqCst), 1);
    assert_eq!(host.display_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.power_provider_calls.load(Ordering::SeqCst), 0);
    assert!(power.actions.lock().is_empty());
    POWER_DROP_HOOK.with(|hook| assert!(hook.borrow().is_none()));
    assert!(POWER_CONFIGURE_HOOK.with(|hook| hook.borrow_mut().take().is_some()));
}

#[test]
fn later_actual_user_or_calendar_lease_drop_cannot_reopen_power_after_scoped_root_retirement() {
    for calendar in [false, true] {
        let (fixture, display, power) =
            fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
        let actor = open_power(&fixture);
        actor.hide();
        assert_hidden(&actor);
        let actor_weak = Rc::downgrade(&actor);
        let power_weak = actor.component().as_weak();
        let cache = Rc::clone(&fixture.controller.power_menu);
        let closed = Rc::clone(&fixture.controller.power_admission_closed);
        let host = Arc::clone(&fixture.host);
        let later_window_visible: Box<dyn Fn() -> bool> = if calendar {
            click_component(&fixture.toolbar, "Open calendar");
            let calendar = fixture.controller.calendar.borrow().clone().unwrap();
            assert!(calendar.is_open());
            assert!(calendar.component().window().is_visible());
            let window = calendar.component().as_weak();
            Box::new(move || {
                window
                    .upgrade()
                    .is_some_and(|component| component.window().is_visible())
            })
        } else {
            fixture.click_launcher("Open user menu");
            let user = fixture.controller.user_menu.borrow().clone().unwrap();
            assert!(user.is_open());
            assert!(user.component().window().is_visible());
            let window = user.component().as_weak();
            Box::new(move || {
                window
                    .upgrade()
                    .is_some_and(|component| component.window().is_visible())
            })
        };
        let retired = Rc::new(Cell::new(false));
        RECYCLE_MENU_DROP_HOOK.with(|hook| {
            let launcher = fixture.launcher.as_weak();
            let actor_weak = actor_weak.clone();
            let cache = Rc::clone(&cache);
            let closed = Rc::clone(&closed);
            let retired = Rc::clone(&retired);
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(closed.get());
                assert!(cache.try_borrow_mut().is_ok());
                assert!(cache.borrow().is_none());
                assert!(
                    actor_weak.upgrade().is_none(),
                    "the first Power scope has already retired before this later lease"
                );
                assert!(
                    later_window_visible(),
                    "the later popup's real lease retires before its native hide"
                );
                let launcher = launcher.upgrade().unwrap();
                assert!(launcher.window().is_visible());
                click_component(&launcher, "Open power menu");
                assert!(cache.borrow().is_none());
                retired.set(true);
            }));
        });
        POWER_CONFIGURE_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|_| panic!("later teardown Power attached")));
        });
        drop(actor);
        drop(fixture);
        slint::platform::update_timers_and_animations();
        assert!(retired.get());
        assert!(closed.get());
        assert!(cache.borrow().is_none());
        assert!(actor_weak.upgrade().is_none());
        assert!(power_weak.upgrade().is_none());
        assert_eq!(host.power_lease_drops.load(Ordering::SeqCst), 1);
        assert_eq!(display.reads.load(Ordering::SeqCst), 1);
        assert_eq!(host.display_provider_calls.load(Ordering::SeqCst), 1);
        assert_eq!(host.power_provider_calls.load(Ordering::SeqCst), 0);
        assert!(power.actions.lock().is_empty());
        RECYCLE_MENU_DROP_HOOK.with(|hook| assert!(hook.borrow().is_none()));
        assert!(POWER_CONFIGURE_HOOK.with(|hook| hook.borrow_mut().take().is_some()));
    }
}
