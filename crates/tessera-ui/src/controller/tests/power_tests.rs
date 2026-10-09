// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::super::popups::PopupKind;
use super::*;
use crate::generated::{PowerMenuAction, PowerMenuSurface};
use crate::power_menu::PowerMenuController;
use tessera_core::Rect;
use tessera_system::display_context::{
    DisplayContextCompletion, DisplayContextError, DisplayContextHost, DisplayLayout,
    DisplaySelection,
};
use tessera_system::power::{
    PowerAction, PowerCompletion, PowerError, PowerHost, PowerRequestAccepted, PowerUpdatePolicy,
};
use tessera_system::power_updates::{
    PowerUpdateHint, PowerUpdatesCompletion, PowerUpdatesError, PowerUpdatesHost,
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
    selections: Mutex<Vec<DisplaySelection>>,
}

impl RecordingDisplayHost {
    fn new(reply: DisplayReply) -> Arc<Self> {
        Arc::new(Self {
            reply: Mutex::new(reply),
            reads: AtomicUsize::new(0),
            completion: Mutex::default(),
            selections: Mutex::default(),
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

    fn read_selected(
        &self,
        selection: DisplaySelection,
        completion: DisplayContextCompletion,
    ) -> Result<(), DisplayContextError> {
        self.selections.lock().push(selection);
        self.read(completion)
    }
}

#[derive(Default)]
struct RecordingPowerHost {
    actions: Mutex<Vec<PowerAction>>,
    completion: Mutex<Option<PowerCompletion>>,
    inline: bool,
    admission_error: Option<PowerError>,
    inline_error: Option<PowerError>,
    completions: AtomicUsize,
}

impl RecordingPowerHost {
    fn complete(&self) {
        let completion = self
            .completion
            .lock()
            .take()
            .expect("accepted Power request");
        self.completions.fetch_add(1, Ordering::SeqCst);
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
        if let Some(error) = self.admission_error {
            return Err(error);
        }
        if self.inline {
            self.completions.fetch_add(1, Ordering::SeqCst);
            completion(self.inline_error.map_or(Ok(PowerRequestAccepted), Err));
        } else {
            assert!(self.completion.lock().replace(completion).is_none());
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum UpdatesReply {
    Inline(Result<PowerUpdateHint, PowerUpdatesError>),
    Delayed,
}

struct RecordingUpdatesHost {
    reply: Mutex<UpdatesReply>,
    reads: AtomicUsize,
    completion: Mutex<Option<PowerUpdatesCompletion>>,
}

impl RecordingUpdatesHost {
    fn new(reply: UpdatesReply) -> Arc<Self> {
        Arc::new(Self {
            reply: Mutex::new(reply),
            reads: AtomicUsize::new(0),
            completion: Mutex::default(),
        })
    }

    fn complete(&self, result: Result<PowerUpdateHint, PowerUpdatesError>) {
        let completion = self
            .completion
            .lock()
            .take()
            .expect("accepted update-hint read");
        completion(result);
    }
}

impl PowerUpdatesHost for RecordingUpdatesHost {
    fn read(&self, completion: PowerUpdatesCompletion) -> Result<(), PowerUpdatesError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let reply = *self.reply.lock();
        match reply {
            UpdatesReply::Inline(result) => completion(result),
            UpdatesReply::Delayed => assert!(self.completion.lock().replace(completion).is_none()),
        }
        Ok(())
    }
}

const UPDATES_PROMPT: &str = "There's pending system updates. Install now?";

fn native_power_cases(pending: bool) -> [(&'static str, PowerMenuAction, PowerAction); 6] {
    let updates = if pending {
        PowerUpdatePolicy::RequestInstallation
    } else {
        PowerUpdatePolicy::OmitExplicitInstallation
    };
    [
        (
            "Lock session",
            PowerMenuAction::LockSession,
            PowerAction::LockSession,
        ),
        ("Log out", PowerMenuAction::LogOut, PowerAction::LogOut),
        (
            if pending {
                "Update and shut down"
            } else {
                "Power off"
            },
            PowerMenuAction::PowerOff,
            PowerAction::PowerOff { updates },
        ),
        (
            if pending {
                "Update and restart"
            } else {
                "Reboot"
            },
            PowerMenuAction::Reboot,
            PowerAction::Reboot { updates },
        ),
        ("Suspend", PowerMenuAction::Suspend, PowerAction::Suspend),
        (
            "Hibernate",
            PowerMenuAction::Hibernate,
            PowerAction::Hibernate,
        ),
    ]
}

fn assert_native_actions_enabled(surface: &PowerMenuSurface, enabled: bool) {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle};
    for (label, _, _) in
        native_power_cases(surface.get_updates_known_pending() && surface.get_install_updates())
    {
        let mut buttons = ElementHandle::find_by_accessible_label(surface, label)
            .filter(|element| element.accessible_role() == Some(AccessibleRole::Button));
        let button = buttons.next().expect("genuine Power action button");
        assert!(buttons.next().is_none());
        assert_eq!(button.accessible_enabled(), Some(enabled), "{label}");
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
    fixture_with_power_preferences(reply, inline, seeded_preferences())
}

fn fixture_with_power_preferences(
    reply: DisplayReply,
    inline: bool,
    preferences: PanelPreferences,
) -> (
    LauncherFixture,
    Arc<RecordingDisplayHost>,
    Arc<RecordingPowerHost>,
) {
    let fixture = LauncherFixture::with_preferences(preferences);
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
    let surface = actor.component();
    POWER_WINDOW.with(|window| *window.borrow_mut() = Some(surface.as_weak()));
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
    let surface = actor.component();
    assert!(surface.window().is_visible());
    actor
}

fn assert_hidden(actor: &PowerMenuController) {
    assert!(!actor.is_visible());
    let surface = actor.component();
    assert!(!surface.window().is_visible());
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

#[derive(Clone, Copy)]
enum TooltipRetirementInput {
    FocusedAccessible,
    Pointer,
}

fn prepare_power_tooltip_retirement(fixture: &LauncherFixture, input: TooltipRetirementInput) {
    use slint::platform::{Key, WindowEvent};
    if matches!(input, TooltipRetirementInput::FocusedAccessible) {
        // Clear a cached fixture's previous pointer hover before presenting the
        // foreign Tooltip. Native Tab then focuses the actual normal footer.
        fixture
            .launcher
            .window()
            .dispatch_event(WindowEvent::PointerExited);
        fixture.launcher.invoke_focus_search();
        assert!(!fixture.launcher.get_feedback_visible());
        assert!(!fixture.launcher.get_refreshing());
        assert!(!fixture.launcher.get_stale());
        for _ in 0..2 * fixture.launcher.get_application_count() + 4 {
            native_key(&fixture.launcher, Key::Tab);
        }
    }
}

fn activate_power_with_live_toolbar_tooltip(
    fixture: &LauncherFixture,
    input: TooltipRetirementInput,
) {
    use i_slint_backend_testing::ElementHandle;
    use slint::platform::{PointerEventButton, WindowEvent};
    let footer = ElementHandle::find_by_accessible_label(&fixture.launcher, "Open power menu")
        .next()
        .unwrap();
    TOOLTIP_DROP_HOOK.with(|hook| assert!(hook.borrow().is_some()));
    match input {
        TooltipRetirementInput::FocusedAccessible => {
            // No hover relay: the real AX action reaches Root's guarded
            // open_power_menu retirement seam, as in the original regression.
            footer.invoke_accessible_default_action();
        }
        TooltipRetirementInput::Pointer => {
            let position = native_center(&footer);
            fixture
                .launcher
                .window()
                .dispatch_event(WindowEvent::PointerPressed {
                    position,
                    button: PointerEventButton::Left,
                });
            // Launcher now owns real hints. Hover/Down legitimately retires
            // the previous Toolbar lease, before the old pointer's release.
            TOOLTIP_DROP_HOOK.with(|hook| assert!(hook.borrow().is_none()));
            fixture
                .launcher
                .window()
                .dispatch_event(WindowEvent::PointerReleased {
                    position,
                    button: PointerEventButton::Left,
                });
        }
    }
    TOOLTIP_DROP_HOOK.with(|hook| assert!(hook.borrow().is_none()));
}

#[test]
fn launcher_observation_results_refresh_actual_feedback_visibility() {
    use i_slint_backend_testing::ElementHandle;
    let fixture = LauncherFixture::new();
    fixture.controller.open_launcher();
    let assert_feedback = |visible| {
        assert_eq!(fixture.panel.get_status_is_feedback(), visible);
        assert_eq!(fixture.launcher.get_feedback_visible(), visible);
        assert_eq!(fixture.launcher.get_status(), fixture.panel.get_status());
        for label in ["Refresh the desktop", "Exit Tessera"] {
            let control = ElementHandle::find_by_accessible_label(&fixture.launcher, label).next();
            assert_eq!(control.is_some(), visible, "{label}");
            if let Some(control) = control {
                assert_eq!(control.accessible_enabled(), Some(true), "{label}");
            }
        }
    };
    assert_feedback(false);
    fixture.controller.report(
        Err("controlled command failure".into()),
        "Command accepted",
        |error| format!("Could not launch: {error}"),
    );
    assert_feedback(true);
    assert!(!fixture.launcher.get_stale());

    // Exercise the actual completion -> render -> launcher projection path,
    // not a manually synchronized property or a cosmetic flag reset.
    apply_result_to_both(&fixture.controller, &fixture.panel, Ok(launcher_snapshot()));
    assert_feedback(false);
    assert!(fixture.panel.get_has_snapshot());
    assert!(!fixture.launcher.get_stale());
    apply_result_to_both(
        &fixture.controller,
        &fixture.panel,
        Err("controlled observation failure".into()),
    );
    assert_feedback(true);
    assert!(fixture.panel.get_has_snapshot());
    assert!(fixture.panel.get_stale());
    assert!(fixture.launcher.get_stale());
    assert!(!fixture.launcher.get_refreshing());
    apply_result_to_both(&fixture.controller, &fixture.panel, Ok(launcher_snapshot()));
    assert_feedback(false);
    assert!(!fixture.panel.get_stale());
    assert!(!fixture.launcher.get_stale());
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
        "Expand applications menu",
    ] {
        assert!(
            ElementHandle::find_by_accessible_label(&fixture.launcher, label)
                .next()
                .is_some()
        );
    }
    for label in ["Refresh the desktop", "Exit Tessera"] {
        assert!(
            ElementHandle::find_by_accessible_label(&fixture.launcher, label)
                .next()
                .is_none()
        );
    }
    fixture.launcher.set_feedback_visible(true);
    for label in ["Refresh the desktop", "Exit Tessera"] {
        let control = ElementHandle::find_by_accessible_label(&fixture.launcher, label)
            .next()
            .expect("real conditional recovery control");
        assert_eq!(control.accessible_enabled(), Some(true));
    }
    fixture.launcher.set_feedback_visible(false);
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
    let surface = actor.component();
    assert!(surface.window().is_visible());
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
            let surface = actor.component();
            assert!(surface.window().is_visible());
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
        let surface = actor.component();
        let unrelated = fixture.host.unrelated_activity();
        for label in [
            "Lock session",
            "Log out",
            "Power off",
            "Reboot",
            "Suspend",
            "Hibernate",
        ] {
            assert_eq!(
                ElementHandle::find_by_accessible_label(&surface, label)
                    .filter(|element| element.accessible_role()
                        == Some(i_slint_backend_testing::AccessibleRole::Button))
                    .count(),
                1
            );
        }
        assert!(
            ElementHandle::find_by_accessible_label(&surface, "Confirm")
                .next()
                .is_none()
        );
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
            click_component(&surface, "Lock session");
        } else {
            native_key(&surface, Key::Tab);
            if input == 1 {
                native_key(&surface, Key::Return);
                surface
                    .window()
                    .dispatch_event(WindowEvent::KeyPressRepeated {
                        text: Key::Return.into(),
                    });
            } else {
                surface.window().dispatch_event(WindowEvent::KeyPressed {
                    text: Key::Space.into(),
                });
                surface
                    .window()
                    .dispatch_event(WindowEvent::KeyPressRepeated {
                        text: Key::Space.into(),
                    });
                assert!(
                    power.actions.lock().is_empty(),
                    "Space only arms before release"
                );
                surface.window().dispatch_event(WindowEvent::KeyReleased {
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
    let surface = actor.component();
    click_component(&surface, "Lock session");
    assert_hidden(&actor);
    assert_eq!(power.actions.lock().len(), 1);
    fixture.controller.hide_launcher();
    fixture.controller.open_launcher();
    let reopened = open_power(&fixture);
    assert!(Rc::ptr_eq(&actor, &reopened));
    assert!(surface.get_lock_busy());
    assert!(!surface.get_action_enabled());
    click_component(&surface, "Lock session");
    native_key(&surface, Key::Tab);
    native_key(&surface, Key::Return);
    native_key(&surface, Key::Space);
    assert_eq!(power.actions.lock().len(), 1);
    power.complete();
    drain_power(&actor);
    assert!(actor.is_visible());
    assert!(surface.window().is_visible());
    assert!(!surface.get_lock_busy());
    assert!(surface.get_action_enabled());
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
        let surface = actor.component();
        if close {
            actor.close();
        } else {
            fixture.controller.hide_launcher();
        }
        display.complete(Ok(Some(desktop_layout())));
        drain_power(&actor);
        assert!(!actor.is_visible());
        assert!(!surface.window().is_visible());
        assert_eq!(fixture.host.power_lease_drops.load(Ordering::SeqCst), 0);
        assert!(power.actions.lock().is_empty());
    }
}

#[test]
fn power_lease_drop_reopens_same_actor_and_rejects_retired_lock_intent() {
    let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let actor = open_power(&fixture);
    let surface = actor.component();
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
            let surface = replacement.component();
            assert!(surface.window().is_visible());
        }));
    });
    click_component(&surface, "Lock session");
    drain_power(&actor);
    assert!(actor.is_visible());
    assert!(surface.window().is_visible());
    assert!(power.actions.lock().is_empty());
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn power_factory_reopens_same_actor_without_locking_the_replacement_scope() {
    let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let actor = open_power(&fixture);
    let surface = actor.component();
    POWER_FACTORY_HOOK.with(|hook| {
        let launcher = fixture.launcher.as_weak();
        let actor = Rc::clone(&actor);
        *hook.borrow_mut() = Some(Box::new(move || {
            assert_hidden(&actor);
            click_component(&launcher.upgrade().unwrap(), "Open power menu");
            drain_power(&actor);
            assert!(actor.is_visible());
            let surface = actor.component();
            assert!(surface.window().is_visible());
        }));
    });
    click_component(&surface, "Lock session");
    drain_power(&actor);
    assert!(actor.is_visible());
    assert!(surface.window().is_visible());
    assert!(power.actions.lock().is_empty());
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn different_user_popup_during_power_retirement_invalidates_old_lock_even_when_power_hidden() {
    let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let actor = open_power(&fixture);
    let surface = actor.component();
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
    click_component(&surface, "Lock session");
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
    let surface = actor.component();
    let size = surface.window().size();
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let drops = fixture.host.power_lease_drops.load(Ordering::SeqCst);
    let activity = fixture.host.unrelated_activity();
    actor.set_theme(Theme::Light);
    actor.update_motion();
    fixture.controller.render();
    fixture.controller.update_geometry();
    drain_power(&actor);
    assert!(actor.is_visible());
    assert!(surface.window().is_visible());
    assert_eq!(surface.window().size(), size);
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
    let surface = actor.component();
    let power_weak = surface.as_weak();
    let panel_weak = fixture.panel.as_weak();
    click_component(&surface, "Lock session");
    assert!(power.completion.lock().is_some());
    assert_hidden(&actor);
    drop(surface);
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

fn assert_toolbar_tooltip_retirement_revokes_old_power_footer(input: TooltipRetirementInput) {
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
            prepare_power_tooltip_retirement(&fixture, input);
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
            activate_power_with_live_toolbar_tooltip(&fixture, input);
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
fn toolbar_tooltip_retirement_hides_or_reopens_launcher_and_revokes_old_power_footer() {
    assert_toolbar_tooltip_retirement_revokes_old_power_footer(
        TooltipRetirementInput::FocusedAccessible,
    );
}

#[test]
fn launcher_pointer_hint_retirement_hides_or_reopens_without_stale_power() {
    assert_toolbar_tooltip_retirement_revokes_old_power_footer(TooltipRetirementInput::Pointer);
}

fn assert_toolbar_tooltip_retirement_preserves_newer_calendar(input: TooltipRetirementInput) {
    let (fixture, display, power) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    prepare_power_tooltip_retirement(&fixture, input);
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
    activate_power_with_live_toolbar_tooltip(&fixture, input);
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
fn toolbar_tooltip_retirement_opens_newer_actual_calendar_without_stale_power() {
    assert_toolbar_tooltip_retirement_preserves_newer_calendar(
        TooltipRetirementInput::FocusedAccessible,
    );
}

#[test]
fn launcher_pointer_hint_retirement_opens_newer_actual_calendar_without_stale_power() {
    assert_toolbar_tooltip_retirement_preserves_newer_calendar(TooltipRetirementInput::Pointer);
}

#[test]
fn scoped_root_drop_closes_power_admission_before_actual_power_lease_footer_and_old_lock_input() {
    let (fixture, display, power) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let actor = open_power(&fixture);
    let actor_weak = Rc::downgrade(&actor);
    let surface = actor.component();
    let power_weak = surface.as_weak();
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
        let surface = surface.clone_strong();
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
                surface.window().is_visible(),
                "dispatch old Lock while its native lease is still retiring"
            );
            click_component(&surface, "Lock session");
            assert!(cache.borrow().is_none());
            retired.set(true);
        }));
    });
    POWER_CONFIGURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|_| panic!("teardown Power attached")));
    });
    drop(surface);
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
        let surface = actor.component();
        let power_weak = surface.as_weak();
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
        drop(surface);
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

#[test]
fn genuine_power_footer_all_six_native_buttons_dispatch_closed_typed_intents_and_only_initiation_receipts()
 {
    let (fixture, display, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), true);
    let updates = RecordingUpdatesHost::new(UpdatesReply::Inline(Ok(PowerUpdateHint::Pending)));
    *fixture.host.power_updates_provider.lock() = Some(updates.clone());
    let activity = fixture.host.unrelated_activity();
    let actor = open_power(&fixture);
    for (index, (label, _, action)) in native_power_cases(true).into_iter().enumerate() {
        if index != 0 {
            let reopened = open_power(&fixture);
            assert!(Rc::ptr_eq(&actor, &reopened));
        }
        let surface = actor.component();
        assert!(surface.get_install_updates());
        assert!(surface.get_updates_known_pending());
        assert_native_actions_enabled(&surface, true);
        click_component(&surface, label);
        assert!(!surface.window().is_visible());
        assert!(!actor.is_visible());
        assert_eq!(power.actions.lock().last().copied(), Some(action));
        assert_eq!(power.actions.lock().len(), index + 1);
        drain_power(&actor);
        assert_eq!(
            fixture.panel.get_status(),
            "Power request accepted; OS state and update completion are not observed."
        );
        assert_eq!(
            fixture.host.power_lease_drops.load(Ordering::SeqCst),
            index + 1
        );
        assert_eq!(display.reads.load(Ordering::SeqCst), index + 1);
        assert_eq!(updates.reads.load(Ordering::SeqCst), index + 1);
        assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture.host.display_provider_calls.load(Ordering::SeqCst),
            1
        );
        assert_eq!(
            fixture
                .host
                .power_updates_factory_calls
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(fixture.host.unrelated_activity(), activity);
        assert!(!fixture.panel.window().is_visible());
    }
    assert_eq!(power.completions.load(Ordering::SeqCst), 6);
}

#[test]
fn accepted_power_flight_disables_all_six_real_buttons_and_double_callbacks_until_native_terminal()
{
    use slint::platform::Key;
    let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let updates = RecordingUpdatesHost::new(UpdatesReply::Inline(Ok(PowerUpdateHint::Pending)));
    *fixture.host.power_updates_provider.lock() = Some(updates);
    let actor = open_power(&fixture);
    let surface = actor.component();
    click_component(&surface, "Update and shut down");
    assert_eq!(
        power.actions.lock().as_slice(),
        [PowerAction::PowerOff {
            updates: PowerUpdatePolicy::RequestInstallation
        }]
    );
    assert_eq!(power.completions.load(Ordering::SeqCst), 0);
    fixture.controller.hide_launcher();
    fixture.controller.open_launcher();
    let reopened = open_power(&fixture);
    assert!(Rc::ptr_eq(&actor, &reopened));
    let current = actor.component();
    assert!(current.get_lock_busy());
    assert_native_actions_enabled(&current, false);
    for (label, callback, _) in native_power_cases(true) {
        click_component(&current, label);
        current.invoke_action_requested(callback);
        current.invoke_action_requested(callback);
    }
    native_key(&current, Key::Tab);
    native_key(&current, Key::Return);
    native_key(&current, Key::Space);
    assert_eq!(power.actions.lock().len(), 1);
    assert!(power.completion.lock().is_some());
    power.complete();
    drain_power(&actor);
    assert!(current.window().is_visible());
    assert!(!current.get_lock_busy());
    assert_native_actions_enabled(&current, true);
    assert_eq!(power.actions.lock().len(), 1);
    assert_eq!(power.completions.load(Ordering::SeqCst), 1);
}

#[test]
fn root_power_admission_error_has_zero_callbacks_and_no_accepted_or_completed_state() {
    // This exercises portable admission rejection, not a fabricated native
    // spawner. The real worker's spawn/driver panic tests own that proof.
    for error in [
        PowerError::AccessDenied,
        PowerError::Busy,
        PowerError::Unavailable,
    ] {
        let (fixture, _, _) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
        let power = Arc::new(RecordingPowerHost {
            admission_error: Some(error),
            ..Default::default()
        });
        *fixture.host.power_provider.lock() = Some(power.clone());
        let actor = open_power(&fixture);
        let surface = actor.component();
        click_component(&surface, "Hibernate");
        drain_power(&actor);
        assert_eq!(power.actions.lock().as_slice(), [PowerAction::Hibernate]);
        assert_eq!(power.completions.load(Ordering::SeqCst), 0);
        assert!(power.completion.lock().is_none());
        assert!(!surface.window().is_visible());
        assert!(!fixture.panel.get_status().contains("request accepted"));
        assert!(fixture.panel.get_status().contains(&error.to_string()));
        let reopened = open_power(&fixture);
        assert!(Rc::ptr_eq(&actor, &reopened));
        let current = actor.component();
        assert!(!current.get_lock_busy());
        assert_native_actions_enabled(&current, true);
        assert_eq!(
            power.actions.lock().len(),
            1,
            "explicit reopen does not retry"
        );
    }
}

#[test]
fn root_inline_power_error_after_perform_reentry_releases_flight_without_replay_or_false_acceptance()
 {
    let (fixture, _, _) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let power = Arc::new(RecordingPowerHost {
        inline: true,
        inline_error: Some(PowerError::Unavailable),
        ..Default::default()
    });
    *fixture.host.power_provider.lock() = Some(power.clone());
    let actor = open_power(&fixture);
    let surface = actor.component();
    let reentered = Rc::new(Cell::new(false));
    POWER_PERFORM_HOOK.with(|hook| {
        let launcher = fixture.launcher.as_weak();
        let actor = Rc::clone(&actor);
        let reentered = Rc::clone(&reentered);
        *hook.borrow_mut() = Some(Box::new(move || {
            assert_hidden(&actor);
            click_component(&launcher.upgrade().unwrap(), "Open power menu");
            drain_power(&actor);
            let current = actor.component();
            assert!(current.window().is_visible());
            assert_native_actions_enabled(&current, false);
            current.invoke_action_requested(PowerMenuAction::Suspend);
            current.invoke_action_requested(PowerMenuAction::Suspend);
            reentered.set(true);
        }));
    });
    click_component(&surface, "Log out");
    drain_power(&actor);
    assert!(reentered.get());
    assert!(actor.is_visible());
    assert_native_actions_enabled(&surface, true);
    assert_eq!(power.actions.lock().as_slice(), [PowerAction::LogOut]);
    assert_eq!(power.completions.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 1);
    assert!(
        fixture
            .panel
            .get_status()
            .contains(&PowerError::Unavailable.to_string())
    );
    assert!(!fixture.panel.get_status().contains("request accepted"));
}

#[test]
fn root_power_initial_metadata_awaits_hint_but_cached_reopen_does_not_wait_and_focus_denial_still_coordinates()
 {
    let (fixture, display, power) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let updates = RecordingUpdatesHost::new(UpdatesReply::Delayed);
    *fixture.host.power_updates_provider.lock() = Some(updates.clone());
    fixture.click_launcher("Open user menu");
    let user = fixture.controller.user_menu.borrow().clone().unwrap();
    let activity = fixture.host.unrelated_activity();
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    *fixture.host.ui_focus_result.lock() = Err("focus refused".into());
    fixture.click_launcher("Open power menu");
    let actor = current_power(&fixture);
    let surface = actor.component();
    drain_power(&actor);
    assert_hidden(&actor);
    assert!(user.is_open());
    assert!(user.component().window().is_visible());
    assert_eq!(fixture.host.power_lease_drops.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), focus);
    assert_eq!(display.reads.load(Ordering::SeqCst), 1);
    assert_eq!(updates.reads.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
    updates.complete(Ok(PowerUpdateHint::Pending));
    drain_power(&actor);
    assert!(surface.window().is_visible());
    assert!(!user.is_open());
    assert!(!user.component().window().is_visible());
    assert!(
        fixture
            .panel
            .get_status()
            .contains("keyboard focus was not granted")
    );
    assert_native_actions_enabled(&surface, true);
    click_component(&surface, UPDATES_PROMPT);
    assert!(!surface.get_install_updates());
    actor.hide();
    let reopened = open_power(&fixture);
    assert!(Rc::ptr_eq(&actor, &reopened));
    assert!(
        updates.completion.lock().is_some(),
        "later hint refresh remains accepted"
    );
    let current = actor.component();
    assert!(current.get_updates_known_pending());
    assert!(
        !current.get_install_updates(),
        "same presentation keeps genuine choice"
    );
    assert_native_actions_enabled(&current, true);
    assert_eq!(display.reads.load(Ordering::SeqCst), 2);
    assert_eq!(updates.reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture.host.display_provider_calls.load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        fixture
            .host
            .power_updates_factory_calls
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.host.unrelated_activity(), activity);
    assert!(power.actions.lock().is_empty());
    updates.complete(Ok(PowerUpdateHint::NotDetected));
    drain_power(&actor);
    assert!(!current.get_updates_known_pending());
    assert!(
        !current.get_install_updates(),
        "hint changes do not rewrite choice"
    );
}

#[test]
fn root_unknown_or_not_detected_hint_keeps_default_choice_but_off_and_reboot_omit_explicit_installation()
 {
    use i_slint_backend_testing::ElementHandle;
    for hint in [
        Err(PowerUpdatesError::Busy),
        Ok(PowerUpdateHint::NotDetected),
    ] {
        let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), true);
        let updates = RecordingUpdatesHost::new(UpdatesReply::Inline(hint));
        *fixture.host.power_updates_provider.lock() = Some(updates);
        let actor = open_power(&fixture);
        for (index, (label, callback, action)) in native_power_cases(false)
            .into_iter()
            .filter(|(_, callback, _)| {
                matches!(
                    callback,
                    PowerMenuAction::PowerOff | PowerMenuAction::Reboot
                )
            })
            .enumerate()
        {
            if index != 0 {
                open_power(&fixture);
            }
            let surface = actor.component();
            assert!(surface.get_install_updates());
            assert!(!surface.get_updates_known_pending());
            assert!(
                ElementHandle::find_by_accessible_label(&surface, UPDATES_PROMPT)
                    .next()
                    .is_none()
            );
            assert_eq!(surface.get_updates_status().is_empty(), hint.is_ok());
            if hint.is_err() {
                assert!(surface.get_updates_status().contains("unavailable"));
            }
            assert_native_actions_enabled(&surface, true);
            click_component(&surface, label);
            surface.invoke_action_requested(callback);
            surface.invoke_action_requested(callback);
            drain_power(&actor);
            assert_eq!(power.actions.lock().last().copied(), Some(action));
            assert_eq!(power.actions.lock().len(), index + 1);
        }
    }
}

#[test]
fn root_genuine_update_checkbox_choice_is_captured_before_provider_reentrant_property_and_double_callback()
 {
    for (label, callback, action) in
        native_power_cases(false)
            .into_iter()
            .filter(|(_, callback, _)| {
                matches!(
                    callback,
                    PowerMenuAction::PowerOff | PowerMenuAction::Reboot
                )
            })
    {
        let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), true);
        let updates = RecordingUpdatesHost::new(UpdatesReply::Inline(Ok(PowerUpdateHint::Pending)));
        *fixture.host.power_updates_provider.lock() = Some(updates);
        let actor = open_power(&fixture);
        let surface = actor.component();
        assert!(surface.get_install_updates());
        click_component(&surface, UPDATES_PROMPT);
        assert!(!surface.get_install_updates());
        surface.invoke_updates_choice_changed(false);
        surface.invoke_updates_choice_changed(false);
        assert!(
            power.actions.lock().is_empty(),
            "choice alone is read-only UI intent"
        );
        POWER_FACTORY_HOOK.with(|hook| {
            let surface = surface.clone_strong();
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(!surface.window().is_visible());
                surface.set_install_updates(true);
                surface.invoke_updates_choice_changed(true);
                surface.invoke_action_requested(callback);
                surface.invoke_action_requested(callback);
            }));
        });
        click_component(&surface, label);
        drain_power(&actor);
        assert_eq!(power.actions.lock().as_slice(), [action]);
        assert_eq!(power.completions.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 1);
        open_power(&fixture);
        let current = actor.component();
        assert!(
            !current.get_install_updates(),
            "hidden injected property cannot change domain choice"
        );
        assert_native_actions_enabled(&current, true);
    }
}

#[test]
fn root_accepted_hint_read_is_independent_of_command_and_late_status_cannot_rewrite_captured_reboot()
 {
    let (fixture, display, power) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let updates = RecordingUpdatesHost::new(UpdatesReply::Inline(Ok(PowerUpdateHint::Pending)));
    *fixture.host.power_updates_provider.lock() = Some(updates.clone());
    let actor = open_power(&fixture);
    *updates.reply.lock() = UpdatesReply::Delayed;
    actor.hide();
    open_power(&fixture);
    let surface = actor.component();
    assert!(updates.completion.lock().is_some());
    assert_native_actions_enabled(&surface, true);
    click_component(&surface, "Update and restart");
    assert_eq!(
        power.actions.lock().as_slice(),
        [PowerAction::Reboot {
            updates: PowerUpdatePolicy::RequestInstallation
        }]
    );
    assert!(power.completion.lock().is_some());
    assert!(
        updates.completion.lock().is_some(),
        "neither flight waits for the other"
    );
    updates.complete(Ok(PowerUpdateHint::NotDetected));
    drain_power(&actor);
    assert!(!actor.is_visible());
    assert!(!surface.window().is_visible());
    assert!(
        surface.get_updates_known_pending(),
        "old hidden observation has no projection authority"
    );
    assert_eq!(power.actions.lock().len(), 1);
    assert_eq!(display.reads.load(Ordering::SeqCst), 2);
    power.complete();
    drain_power(&actor);
    assert!(!actor.is_visible());
    assert_eq!(power.actions.lock().len(), 1);
    assert_eq!(
        fixture.panel.get_status(),
        "Power request accepted; OS state and update completion are not observed."
    );
}

#[test]
fn root_update_metadata_factory_close_or_new_user_operation_cannot_publish_stale_native_power() {
    for close in [false, true] {
        let (fixture, display, power) =
            fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
        let updates = RecordingUpdatesHost::new(UpdatesReply::Inline(Ok(PowerUpdateHint::Pending)));
        *fixture.host.power_updates_provider.lock() = Some(updates.clone());
        let old_surface = Rc::new(RefCell::new(None));
        POWER_UPDATES_FACTORY_HOOK.with(|hook| {
            let controller = fixture.controller.clone();
            let launcher = fixture.launcher.as_weak();
            let old_surface = Rc::clone(&old_surface);
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(controller.power_menu.try_borrow_mut().is_ok());
                let actor = controller.power_menu.borrow().clone().unwrap();
                let surface = actor.component();
                POWER_WINDOW.with(|window| *window.borrow_mut() = Some(surface.as_weak()));
                *old_surface.borrow_mut() = Some(surface);
                if close {
                    actor.close();
                } else {
                    click_component(&launcher.upgrade().unwrap(), "Open user menu");
                }
            }));
        });
        POWER_CONFIGURE_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|_| panic!("stale metadata scope attached Power")));
        });
        fixture.click_launcher("Open power menu");
        let actor = fixture.controller.power_menu.borrow().clone().unwrap();
        drain_power(&actor);
        assert!(!actor.is_visible());
        let surface = old_surface.borrow().as_ref().unwrap().clone_strong();
        assert!(!surface.window().is_visible());
        surface.invoke_action_requested(PowerMenuAction::PowerOff);
        if close {
            assert!(actor.component_if_present().is_none());
        } else {
            let user = fixture.controller.user_menu.borrow().clone().unwrap();
            assert!(user.is_open());
            assert!(user.component().window().is_visible());
        }
        assert_eq!(updates.reads.load(Ordering::SeqCst), 0);
        assert_eq!(display.reads.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture
                .host
                .power_updates_factory_calls
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.host.power_lease_drops.load(Ordering::SeqCst), 0);
        assert!(power.actions.lock().is_empty());
        assert!(POWER_CONFIGURE_HOOK.with(|hook| hook.borrow_mut().take().is_some()));
    }
}

#[test]
fn root_late_hint_from_old_launcher_scope_cannot_initialize_or_mutate_fresh_power_scope() {
    let (fixture, display, power) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let updates = RecordingUpdatesHost::new(UpdatesReply::Delayed);
    *fixture.host.power_updates_provider.lock() = Some(updates.clone());
    fixture.click_launcher("Open power menu");
    let actor = current_power(&fixture);
    let old = actor.component();
    drain_power(&actor);
    assert_hidden(&actor);
    fixture.controller.hide_launcher();
    advance_recycle_timer(30_000);
    assert!(actor.component_if_present().is_none());
    assert!(!old.window().is_visible());
    fixture.controller.open_launcher();
    fixture.click_launcher("Open user menu");
    let user = fixture.controller.user_menu.borrow().clone().unwrap();
    fixture.click_launcher("Open power menu");
    current_power(&fixture);
    drain_power(&actor);
    assert_hidden(&actor);
    updates.complete(Ok(PowerUpdateHint::Pending));
    drain_power(&actor);
    let surface = actor.component();
    assert!(!std::ptr::eq(old.window(), surface.window()));
    assert!(!surface.get_updates_known_pending());
    assert_hidden(&actor);
    assert!(user.is_open());
    assert!(user.component().window().is_visible());
    assert_eq!(
        updates.reads.load(Ordering::SeqCst),
        2,
        "one fresh read, not old terminal reuse"
    );
    assert!(updates.completion.lock().is_some());
    assert_eq!(fixture.host.power_lease_drops.load(Ordering::SeqCst), 0);
    updates.complete(Ok(PowerUpdateHint::NotDetected));
    drain_power(&actor);
    assert!(surface.window().is_visible());
    assert!(!surface.get_updates_known_pending());
    assert!(surface.get_install_updates());
    assert!(!user.is_open());
    assert_eq!(display.reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture.host.display_provider_calls.load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        fixture
            .host
            .power_updates_factory_calls
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
    assert!(power.actions.lock().is_empty());
}

#[test]
fn root_accepted_power_survives_actual_thirty_second_presentation_retirement_and_drains_held_terminal_on_weak_root_wake()
 {
    use slint::platform::Key;
    for complete_while_retired in [false, true] {
        let (fixture, display, power) =
            fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
        let updates = RecordingUpdatesHost::new(UpdatesReply::Inline(Ok(PowerUpdateHint::Pending)));
        *fixture.host.power_updates_provider.lock() = Some(updates.clone());
        let actor = open_power(&fixture);
        let old = actor.component();
        click_component(&old, UPDATES_PROMPT);
        click_component(&old, "Power off");
        assert!(power.completion.lock().is_some());
        assert_eq!(power.completions.load(Ordering::SeqCst), 0);
        assert_eq!(
            power.actions.lock().as_slice(),
            [PowerAction::PowerOff {
                updates: PowerUpdatePolicy::OmitExplicitInstallation
            }]
        );
        advance_recycle_timer(29_999);
        assert!(actor.component_if_present().is_some());
        advance_recycle_timer(1);
        assert!(
            actor.component_if_present().is_none(),
            "real actor ownership retires, not a policy reset"
        );
        assert!(!old.window().is_visible());
        assert!(!actor.is_visible());
        if complete_while_retired {
            power.complete();
            fixture.panel.invoke_power_command_event_ready();
            assert!(!actor.command_busy());
            assert!(actor.component_if_present().is_none());
            assert_eq!(
                fixture.panel.get_status(),
                "Power request accepted; OS state and update completion are not observed.",
                "presentationless command terminal needs no explicit Power reopen"
            );
        }
        actor.set_theme(Theme::Light);
        actor.update_motion();
        fixture.controller.render();
        assert!(
            actor.component_if_present().is_none(),
            "projection never creates a presentation"
        );
        let reopened = open_power(&fixture);
        assert!(Rc::ptr_eq(&actor, &reopened));
        let current = actor.component();
        assert!(!std::ptr::eq(old.window(), current.window()));
        assert!(
            current.get_install_updates(),
            "only a genuinely new presentation defaults checked"
        );
        assert!(current.get_updates_known_pending());
        assert_eq!(current.get_lock_busy(), !complete_while_retired);
        assert_native_actions_enabled(&current, complete_while_retired);
        for (label, callback, _) in native_power_cases(false) {
            click_component(&old, label);
            old.invoke_action_requested(callback);
            old.invoke_action_requested(callback);
        }
        native_key(&old, Key::Tab);
        native_key(&old, Key::Return);
        native_key(&old, Key::Space);
        old.invoke_updates_choice_changed(false);
        old.invoke_hide_requested();
        old.invoke_viewport_changed();
        assert!(current.window().is_visible());
        assert!(current.get_install_updates());
        assert_eq!(
            power.actions.lock().len(),
            1,
            "retired controls cannot enter the fresh scope"
        );
        if !complete_while_retired {
            power.complete();
            drain_power(&actor);
        }
        assert!(!current.get_lock_busy());
        assert_native_actions_enabled(&current, true);
        assert_eq!(power.completions.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture.panel.get_status(),
            "Power request accepted; OS state and update completion are not observed."
        );
        assert_eq!(display.reads.load(Ordering::SeqCst), 2);
        assert_eq!(updates.reads.load(Ordering::SeqCst), 2);
        assert_eq!(
            fixture.host.display_provider_calls.load(Ordering::SeqCst),
            1
        );
        assert_eq!(
            fixture
                .host
                .power_updates_factory_calls
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 1);
        assert_eq!(power.actions.lock().len(), 1);
    }
}

#[test]
fn root_scope_close_before_native_accepted_terminal_never_reopens_or_publishes_receipt() {
    let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let actor = open_power(&fixture);
    let old = actor.component();
    let panel = fixture.panel.clone_strong();
    let launcher = fixture.launcher.clone_strong();
    let cache = Rc::clone(&fixture.controller.power_menu);
    let closed = Rc::clone(&fixture.controller.power_admission_closed);
    let host = Arc::clone(&fixture.host);
    click_component(&old, "Suspend");
    assert!(power.completion.lock().is_some());
    let before = panel.get_status();
    drop(fixture);
    assert!(closed.get());
    assert!(cache.borrow().is_none());
    assert!(actor.component_if_present().is_none());
    assert!(!old.window().is_visible());
    launcher.invoke_open_power_menu_requested();
    old.invoke_action_requested(PowerMenuAction::Hibernate);
    old.invoke_action_requested(PowerMenuAction::Hibernate);
    assert!(cache.borrow().is_none());
    power.complete();
    drain_power(&actor);
    assert_eq!(panel.get_status(), before);
    assert!(!panel.window().is_visible());
    assert!(!launcher.window().is_visible());
    assert!(cache.borrow().is_none());
    assert!(actor.component_if_present().is_none());
    assert_eq!(host.power_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(power.completions.load(Ordering::SeqCst), 1);
    assert_eq!(power.actions.lock().as_slice(), [PowerAction::Suspend]);
}

#[test]
fn root_power_uses_genuine_bounded_launcher_account_without_extra_identity_observation_or_fake_profile()
 {
    use i_slint_backend_testing::ElementHandle;
    let (fixture, display, power) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    fixture.host.shell_identity.lock().user_name =
        format!(" \u{202e}Account\t{}\0 ", "界".repeat(80));
    fixture.controller.core.refresh_identity();
    fixture.controller.render();
    let expected = format!("Account {}…", "界".repeat(56));
    assert_eq!(fixture.launcher.get_user_name(), expected);
    assert_eq!(fixture.toolbar.get_user_name(), expected);
    let identity_reads = fixture.host.identity_calls.load(Ordering::SeqCst);
    let activity = fixture.host.unrelated_activity();
    let actor = open_power(&fixture);
    let old = actor.component();
    assert_eq!(old.get_user_name(), expected);
    assert!(
        ElementHandle::find_by_accessible_label(&old, &format!("Goodbye {expected}"))
            .next()
            .is_some()
    );
    assert_eq!(
        fixture.host.identity_calls.load(Ordering::SeqCst),
        identity_reads
    );
    assert_eq!(fixture.host.unrelated_activity(), activity);
    actor.hide();
    advance_recycle_timer(30_000);
    assert!(actor.component_if_present().is_none());
    assert!(!old.window().is_visible());
    fixture.host.shell_identity.lock().user_name = "New account".into();
    fixture.controller.core.refresh_identity();
    fixture.controller.render();
    assert_eq!(fixture.launcher.get_user_name(), "New account");
    assert!(
        actor.component_if_present().is_none(),
        "retained account projection cannot create Power UI"
    );
    let identity_reads = fixture.host.identity_calls.load(Ordering::SeqCst);
    let reopened = open_power(&fixture);
    assert!(Rc::ptr_eq(&actor, &reopened));
    let current = actor.component();
    assert!(!std::ptr::eq(old.window(), current.window()));
    assert_eq!(current.get_user_name(), fixture.launcher.get_user_name());
    assert_eq!(current.get_user_name(), "New account");
    assert_eq!(
        old.get_user_name(),
        expected,
        "retired physical handle is not the fresh account projection"
    );
    assert!(
        ElementHandle::find_by_accessible_label(&current, "Goodbye New account")
            .next()
            .is_some()
    );
    assert_eq!(
        fixture.host.identity_calls.load(Ordering::SeqCst),
        identity_reads
    );
    assert_eq!(fixture.host.unrelated_activity(), activity);
    assert_eq!(display.reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture.host.display_provider_calls.load(Ordering::SeqCst),
        1
    );
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
    assert!(power.actions.lock().is_empty());
}

fn open_logout_user(fixture: &LauncherFixture) -> Rc<crate::user_menu::UserMenuController> {
    fixture.click_launcher("Open user menu");
    let user = fixture.controller.user_menu.borrow().clone().unwrap();
    assert!(user.is_open());
    assert!(user.component().window().is_visible());
    user
}

fn logout_control(
    user: &crate::user_menu::UserMenuController,
) -> i_slint_backend_testing::ElementHandle {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle};
    let mut controls = ElementHandle::find_by_accessible_label(user.component(), "Log out")
        .filter(|element| element.accessible_role() == Some(AccessibleRole::Button));
    let control = controls.next().expect("genuine bound User Logout");
    assert!(controls.next().is_none());
    control
}

fn assert_no_logout_side_routes(fixture: &LauncherFixture, display: &RecordingDisplayHost) {
    assert!(fixture.host.saves.lock().is_empty());
    assert!(fixture.host.system_actions.lock().is_empty());
    assert!(fixture.host.launches.lock().is_empty());
    assert_eq!(
        fixture.host.shortcuts_factory_calls.load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        fixture
            .host
            .power_updates_factory_calls
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        fixture.host.display_provider_calls.load(Ordering::SeqCst),
        0
    );
    assert_eq!(display.reads.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.host.power_lease_drops.load(Ordering::SeqCst), 0);
    assert!(!fixture.panel.window().is_visible());
}

#[test]
fn genuine_user_logout_pointer_return_space_and_ax_share_exact_log_out_without_power_presentation()
{
    use slint::platform::{Key, WindowEvent};
    for input in 0..4 {
        let (fixture, display, power) =
            fixture_with_power(DisplayReply::Ready(desktop_layout()), true);
        let user = open_logout_user(&fixture);
        let surface = user.component();
        let control = logout_control(&user);
        assert_eq!(control.accessible_enabled(), Some(true));
        assert_eq!((control.size().width, control.size().height), (28.0, 28.0));
        assert!(
            fixture.controller.power_menu.borrow().is_none(),
            "paint never discovers Power"
        );
        let drops = fixture.host.lease_drops.load(Ordering::SeqCst);
        let detached = Rc::new(Cell::new(false));
        POWER_FACTORY_HOOK.with(|hook| {
            let user = Rc::clone(&user);
            let leases = Arc::clone(&fixture.host.lease_drops);
            let cache = Rc::clone(&fixture.controller.power_menu);
            let detached = detached.clone();
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(!user.is_open());
                assert!(!user.component().window().is_visible());
                assert_eq!(leases.load(Ordering::SeqCst), drops + 1);
                assert!(
                    cache
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .component_if_present()
                        .is_none()
                );
                detached.set(true);
            }));
        });
        POWER_PERFORM_HOOK.with(|hook| {
            let user = Rc::clone(&user);
            let detached = detached.clone();
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(detached.get());
                assert!(!user.is_open());
                assert!(!user.component().window().is_visible());
            }));
        });
        match input {
            0 => click_component(surface, "Log out"),
            3 => control.invoke_accessible_default_action(),
            _ => {
                surface.invoke_focus_content();
                // Unsupported profile has no Home/Accounts tab stops.
                native_key(surface, Key::Tab);
                let key = if input == 1 { Key::Return } else { Key::Space };
                surface
                    .window()
                    .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
                surface
                    .window()
                    .dispatch_event(WindowEvent::KeyPressRepeated { text: key.into() });
                if input == 2 {
                    assert!(
                        power.actions.lock().is_empty(),
                        "Space release owns activation"
                    );
                }
                surface
                    .window()
                    .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
            }
        }
        let authority = fixture.controller.power_menu.borrow().clone().unwrap();
        assert!(
            authority.command_busy(),
            "inline terminal remains asynchronous"
        );
        assert_eq!(*power.actions.lock(), [PowerAction::LogOut]);
        assert!(authority.component_if_present().is_none());
        fixture.panel.invoke_power_command_event_ready();
        assert!(!authority.command_busy());
        assert!(
            authority.component_if_present().is_none(),
            "terminal cannot create Power"
        );
        assert!(detached.get());
        assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 1);
        assert_no_logout_side_routes(&fixture, &display);
        assert_eq!(
            fixture.panel.get_status(),
            "Power request accepted; OS state and update completion are not observed."
        );
        surface.invoke_logout_requested(surface.get_logout_key());
        control.invoke_accessible_default_action();
        assert_eq!(power.actions.lock().len(), 1, "hidden input never replays");
    }
}

#[test]
fn user_and_six_action_power_share_busy_both_directions_and_only_fresh_input_after_terminal() {
    for user_first in [true, false] {
        let (fixture, _, native) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
        if user_first {
            let user = open_logout_user(&fixture);
            click_component(user.component(), "Log out");
        } else {
            let power = open_power(&fixture);
            click_component(&power.component(), "Suspend");
        }
        let authority = fixture.controller.power_menu.borrow().clone().unwrap();
        let power = open_power(&fixture);
        assert!(Rc::ptr_eq(&authority, &power));
        assert!(power.component().get_lock_busy());
        assert_native_actions_enabled(&power.component(), false);
        for (label, action, _) in native_power_cases(false) {
            click_component(&power.component(), label);
            power.component().invoke_action_requested(action);
        }
        let user = open_logout_user(&fixture);
        assert!(user.component().get_logout_busy());
        assert!(!user.component().get_logout_ready());
        logout_control(&user).invoke_accessible_default_action();
        user.component()
            .invoke_logout_requested(user.component().get_logout_key());
        assert_eq!(native.actions.lock().len(), 1);
        native.complete();
        fixture.panel.invoke_power_command_event_ready();
        assert!(!authority.command_busy());
        assert!(!user.component().get_logout_busy());
        assert!(user.component().get_logout_ready());
        assert!(
            user.is_open(),
            "terminal does not dismiss unrelated current input"
        );
        assert_eq!(native.actions.lock().len(), 1);
        click_component(user.component(), "Log out");
        assert_eq!(native.actions.lock().last(), Some(&PowerAction::LogOut));
        assert_eq!(native.actions.lock().len(), 2);
        assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 1);
        native.complete();
        fixture.panel.invoke_power_command_event_ready();
    }
}

#[test]
fn stale_user_logout_keys_hidden_native_cache_popup_and_closed_root_fail_before_provider() {
    for invalid in 0..10 {
        let preferences = if invalid == 9 {
            reorder_preferences()
        } else {
            seeded_preferences()
        };
        let (fixture, _, native) = fixture_with_power_preferences(
            DisplayReply::Ready(desktop_layout()),
            false,
            preferences,
        );
        let user = open_logout_user(&fixture);
        let key = user.component().get_logout_key();
        match invalid {
            0 => user.hide(),
            1 => {
                user.hide();
                open_logout_user(&fixture);
            }
            2 => user.component().window().hide().unwrap(),
            3 => {
                fixture.controller.user_menu.borrow_mut().take();
            }
            4 => fixture
                .controller
                .admission
                .active_popup
                .set(Some(PopupKind::Calendar)),
            5 => fixture.controller.power_admission_closed.set(true),
            6 => fixture.controller.admission.alive.set(false),
            7 => {
                apply_result(
                    &fixture.controller,
                    &fixture.panel,
                    Ok(launcher_snapshot().with_dock_context(
                        crate::DockContext::new(0, 0, 1920, 1040, true).unwrap(),
                    )),
                );
            }
            8 => fixture.launcher.window().hide().unwrap(),
            9 => {
                begin_editor_reorder(&fixture);
                assert!(fixture.launcher.get_reorder_dragging());
            }
            _ => unreachable!(),
        }
        user.component().invoke_logout_requested(key);
        assert!(native.actions.lock().is_empty());
        assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
        assert!(fixture.controller.power_menu.borrow().is_none());
        if invalid == 9 {
            fixture.controller.cancel_launcher_reorder();
            slint::platform::update_timers_and_animations();
        }
    }
}

#[test]
fn borrowed_launcher_fingerprint_rejects_reopen_native_mismatch_geometry_and_dpi() {
    use slint::platform::WindowEvent;
    for change in 0..4 {
        let fixture = LauncherFixture::new();
        fixture.controller.open_launcher();
        let guard = fixture.controller.launcher_source_guard();
        assert!(guard());
        match change {
            0 => {
                fixture.controller.hide_launcher();
                fixture.controller.open_launcher();
            }
            1 => fixture.launcher.window().hide().unwrap(),
            2 => fixture
                .launcher
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 2.0 }),
            3 => {
                let snapshot = launcher_snapshot()
                    .with_dock_context(crate::DockContext::new(0, 0, 1920, 1080, true).unwrap());
                apply_result(&fixture.controller, &fixture.panel, Ok(snapshot));
            }
            _ => unreachable!(),
        }
        assert!(
            !guard(),
            "same actual source after {change} is not captured input"
        );
    }
}

#[test]
fn user_logout_cancel_profile_and_lease_retirement_reentry_preserves_new_intent() {
    use crate::user_menu::LogoutRetirementStage;
    for stage in 0..3 {
        for replacement in 0..6 {
            let (fixture, display, native) =
                fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
            let user = open_logout_user(&fixture);
            let launcher = fixture.launcher.as_weak();
            let toolbar = fixture.toolbar.as_weak();
            let root = fixture.controller.clone();
            let callback_display = display.clone();
            let callback = move || {
                assert!(root.user_menu.try_borrow_mut().is_ok());
                assert!(root.power_menu.try_borrow_mut().is_ok());
                match replacement {
                    0 => click_component(&launcher.upgrade().unwrap(), "Open user menu"),
                    1 => click_component(&toolbar.upgrade().unwrap(), "Open calendar"),
                    2 => {
                        *callback_display.reply.lock() = DisplayReply::Delayed;
                        click_component(&launcher.upgrade().unwrap(), "Open power menu");
                        let power = root.power_menu.borrow().clone().unwrap();
                        POWER_WINDOW.with(|window| {
                            *window.borrow_mut() = Some(power.component().as_weak())
                        });
                    }
                    3 => root.open_panel(),
                    4 => {
                        root.hide_launcher();
                        root.open_launcher();
                    }
                    5 => drop(shortcuts::RootCapabilityScope::new(&root)),
                    _ => unreachable!(),
                }
            };
            match stage {
                0 => user.on_logout_retirement(LogoutRetirementStage::InputCancelled, callback),
                1 => user.on_logout_retirement(LogoutRetirementStage::ProfileRetired, callback),
                2 => RECYCLE_MENU_DROP_HOOK
                    .with(|hook| *hook.borrow_mut() = Some(Box::new(callback))),
                _ => unreachable!(),
            }
            click_component(user.component(), "Log out");
            assert!(native.actions.lock().is_empty());
            assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
            let authority = fixture.controller.power_menu.borrow().clone().unwrap();
            assert!(!authority.command_busy());
            match replacement {
                0 => {
                    assert!(user.is_open());
                    assert!(user.component().window().is_visible());
                    assert!(user.component().get_logout_ready());
                }
                1 => {
                    let calendar = fixture.controller.calendar.borrow().clone().unwrap();
                    assert!(calendar.is_open());
                    assert!(calendar.component().window().is_visible());
                    assert!(!user.is_open());
                }
                2 => {
                    assert!(!authority.is_visible());
                    assert!(display.completion.lock().is_some());
                    display.complete(Ok(Some(desktop_layout())));
                    drain_power(&authority);
                    assert!(authority.is_visible(), "new pending Power was not revoked");
                    assert_native_actions_enabled(&authority.component(), true);
                }
                3 => assert!(fixture.panel.window().is_visible()),
                4 => assert!(fixture.controller.launcher_popup_ready()),
                5 => assert!(!fixture.controller.root_current()),
                _ => unreachable!(),
            }
            assert!(native.actions.lock().is_empty());
        }
    }
}

#[test]
fn user_logout_provider_reentry_rejects_old_command_and_preserves_exact_new_intent() {
    for replacement in 0..6 {
        let (fixture, _, native) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
        let user = open_logout_user(&fixture);
        POWER_FACTORY_HOOK.with(|hook| {
            let root = fixture.controller.clone();
            let launcher = fixture.launcher.as_weak();
            let toolbar = fixture.toolbar.as_weak();
            let user = Rc::clone(&user);
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(!user.is_open());
                assert!(!user.component().window().is_visible());
                match replacement {
                    0 => click_component(&launcher.upgrade().unwrap(), "Open user menu"),
                    1 => click_component(&toolbar.upgrade().unwrap(), "Open calendar"),
                    2 => {
                        click_component(&launcher.upgrade().unwrap(), "Open power menu");
                        let power = root.power_menu.borrow().clone().unwrap();
                        POWER_WINDOW.with(|window| {
                            *window.borrow_mut() = Some(power.component().as_weak())
                        });
                        drain_power(&power);
                    }
                    3 => {
                        root.hide_launcher();
                        root.open_launcher();
                    }
                    4 => {
                        root.power_admission_closed.set(true);
                        root.admission.alive.set(false);
                    }
                    5 => root.open_panel(),
                    _ => unreachable!(),
                }
            }));
        });
        click_component(user.component(), "Log out");
        let authority = fixture.controller.power_menu.borrow().clone().unwrap();
        assert!(!authority.command_busy());
        assert!(native.actions.lock().is_empty());
        assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 1);
        match replacement {
            0 => {
                assert!(user.is_open());
                assert!(user.component().get_logout_ready());
            }
            1 => assert!(
                fixture
                    .controller
                    .calendar
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .is_open()
            ),
            2 => assert!(authority.is_visible()),
            3 => assert!(fixture.controller.launcher_popup_ready()),
            4 => assert!(!fixture.controller.root_current()),
            5 => assert!(fixture.panel.window().is_visible()),
            _ => unreachable!(),
        }
    }
}

#[test]
fn user_logout_admission_error_inline_error_and_unsupported_are_terminal_without_replay() {
    for failure in 0..3 {
        let (fixture, display, _) =
            fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
        let native = Arc::new(RecordingPowerHost {
            inline: failure == 1,
            admission_error: (failure == 0).then_some(PowerError::AccessDenied),
            inline_error: (failure == 1).then_some(PowerError::Unavailable),
            ..Default::default()
        });
        *fixture.host.power_provider.lock() =
            (failure != 2).then(|| native.clone() as Arc<dyn PowerHost>);
        let user = open_logout_user(&fixture);
        click_component(user.component(), "Log out");
        let authority = fixture.controller.power_menu.borrow().clone().unwrap();
        fixture.panel.invoke_power_command_event_ready();
        assert!(!authority.command_busy());
        assert!(!fixture.panel.get_status().contains("request accepted"));
        assert!(fixture.panel.get_status().contains("Power request failed"));
        assert_eq!(
            native.completions.load(Ordering::SeqCst),
            usize::from(failure == 1)
        );
        assert_eq!(native.actions.lock().len(), usize::from(failure != 2));
        open_logout_user(&fixture);
        assert!(user.component().get_logout_ready());
        assert_eq!(
            native.actions.lock().len(),
            usize::from(failure != 2),
            "reopen is not retry"
        );
        assert_no_logout_side_routes(&fixture, &display);
    }
}

#[test]
fn user_logout_submitted_before_perform_reentry_and_inline_completion_releases_same_flight() {
    let (fixture, _, native) = fixture_with_power(DisplayReply::Ready(desktop_layout()), true);
    let user = open_logout_user(&fixture);
    POWER_PERFORM_HOOK.with(|hook| {
        let launcher = fixture.launcher.as_weak();
        let user = Rc::clone(&user);
        let cache = Rc::clone(&fixture.controller.power_menu);
        *hook.borrow_mut() = Some(Box::new(move || {
            let power = cache.borrow().clone().unwrap();
            assert!(power.command_busy());
            click_component(&launcher.upgrade().unwrap(), "Open user menu");
            assert!(user.is_open());
            assert!(user.component().get_logout_busy());
            assert!(!user.component().get_logout_ready());
            user.component()
                .invoke_logout_requested(user.component().get_logout_key());
        }));
    });
    click_component(user.component(), "Log out");
    assert_eq!(*native.actions.lock(), [PowerAction::LogOut]);
    assert!(user.is_open());
    fixture.panel.invoke_power_command_event_ready();
    assert!(!user.component().get_logout_busy());
    assert!(user.component().get_logout_ready());
    assert_eq!(native.completions.load(Ordering::SeqCst), 1);
    assert_eq!(native.actions.lock().len(), 1);
}

#[test]
fn weak_root_command_wake_drains_user_terminal_with_power_never_opened_or_actually_evicted() {
    for prior_power in [false, true] {
        let (fixture, display, native) =
            fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
        if prior_power {
            let authority = open_power(&fixture);
            fixture.controller.hide_power_menu();
            advance_recycle_timer(30_000);
            assert!(authority.component_if_present().is_none());
        }
        let user = open_logout_user(&fixture);
        click_component(user.component(), "Log out");
        let authority = fixture.controller.power_menu.borrow().clone().unwrap();
        assert!(authority.component_if_present().is_none());
        fixture.controller.hide_launcher();
        advance_recycle_timer(30_000);
        native.complete();
        fixture.panel.invoke_power_command_event_ready();
        assert!(!authority.command_busy());
        assert!(authority.component_if_present().is_none());
        assert!(!user.is_open());
        assert!(!fixture.launcher.window().is_visible());
        assert_eq!(native.actions.lock().as_slice(), [PowerAction::LogOut]);
        assert_eq!(
            display.reads.load(Ordering::SeqCst),
            usize::from(prior_power)
        );
        assert_eq!(
            fixture.panel.get_status(),
            "Power request accepted; OS state and update completion are not observed."
        );
        fixture.panel.invoke_power_command_event_ready();
        assert_eq!(
            native.actions.lock().len(),
            1,
            "duplicate wake cannot replay"
        );
    }
}

#[test]
fn accepted_user_logout_completion_owns_no_strong_root_user_power_or_presentation_after_drop() {
    let (fixture, _, native) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let user = open_logout_user(&fixture);
    click_component(user.component(), "Log out");
    let authority = fixture.controller.power_menu.borrow().clone().unwrap();
    let power = Rc::downgrade(&authority);
    let user_weak = Rc::downgrade(&user);
    let window = user.component().as_weak();
    let panel = fixture.panel.as_weak();
    assert!(native.completion.lock().is_some());
    drop(authority);
    drop(user);
    drop(fixture);
    assert!(power.upgrade().is_none());
    assert!(user_weak.upgrade().is_none());
    assert!(window.upgrade().is_none());
    assert!(panel.upgrade().is_none());
    native.complete();
    slint::platform::update_timers_and_animations();
    assert_eq!(native.actions.lock().as_slice(), [PowerAction::LogOut]);
    assert!(power.upgrade().is_none());
    assert!(panel.upgrade().is_none());
}

#[test]
fn newer_already_visible_settings_focus_during_logout_factory_retires_unsubmitted_command() {
    let (fixture, _, native) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    fixture.controller.open_panel();
    assert!(fixture.panel.window().is_visible());
    let user = open_logout_user(&fixture);
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    POWER_FACTORY_HOOK.with(|hook| {
        let root = fixture.controller.clone();
        *hook.borrow_mut() = Some(Box::new(move || root.open_panel()));
    });
    click_component(user.component(), "Log out");
    assert!(fixture.panel.window().is_visible());
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus + 1
    );
    assert!(native.actions.lock().is_empty());
    assert!(
        !fixture
            .controller
            .power_menu
            .borrow()
            .as_ref()
            .unwrap()
            .command_busy()
    );
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn older_settings_tooltip_retirement_cannot_present_or_focus_over_new_user_intent() {
    let (fixture, _, native) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    present_toolbar_tooltip(&fixture);
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    TOOLTIP_DROP_HOOK.with(|hook| {
        let launcher = fixture.launcher.as_weak();
        *hook.borrow_mut() = Some(Box::new(move || {
            click_component(&launcher.upgrade().unwrap(), "Open user menu");
        }));
    });
    fixture.controller.open_panel();
    let user = fixture.controller.user_menu.borrow().clone().unwrap();
    assert!(user.is_open());
    assert!(user.component().window().is_visible());
    assert!(
        !fixture.panel.window().is_visible(),
        "older Settings loses exact transaction"
    );
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus + 1
    );
    assert!(native.actions.lock().is_empty());
    assert!(fixture.host.saves.lock().is_empty());
    TOOLTIP_DROP_HOOK.with(|hook| assert!(hook.borrow().is_none()));
}

#[test]
fn pending_genuine_global_settings_monitor_read_retires_logout_before_any_new_popup_attaches() {
    use tessera_system::shortcuts::mocks::RecordingShortcutHost;
    use tessera_system::shortcuts::{ShortcutAction, ShortcutEvent, ShortcutTrigger, TriggerPoint};
    let (fixture, display, native) = fixture_with_power(DisplayReply::Delayed, false);
    let shortcuts = Arc::new(RecordingShortcutHost::default());
    *fixture.host.shortcuts_provider.lock() = Some(shortcuts.clone());
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.start_shortcuts(true);
    let configuration = shortcuts.take_next_completion().unwrap();
    let generation = configuration.generation();
    configuration.complete_registered();
    fixture.panel.invoke_shortcuts_event_ready();
    let user = open_logout_user(&fixture);
    let point = TriggerPoint { x: -600, y: 100 };
    POWER_FACTORY_HOOK.with(|hook| {
        let shortcuts = shortcuts.clone();
        let panel = fixture.panel.as_weak();
        let display = display.clone();
        *hook.borrow_mut() = Some(Box::new(move || {
            shortcuts.emit(ShortcutEvent::Triggered(ShortcutTrigger {
                generation,
                action: ShortcutAction::OpenSettings,
                cursor: Some(point),
            }));
            panel.upgrade().unwrap().invoke_shortcuts_event_ready();
            assert!(display.completion.lock().is_some());
            assert!(!panel.upgrade().unwrap().window().is_visible());
        }));
    });
    click_component(user.component(), "Log out");
    assert!(native.actions.lock().is_empty());
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 1);
    assert!(
        !fixture
            .controller
            .power_menu
            .borrow()
            .as_ref()
            .unwrap()
            .command_busy()
    );
    let selection = DisplaySelection::AtPoint {
        x: point.x,
        y: point.y,
    };
    assert_eq!(display.selections.lock().as_slice(), [selection]);
    display.complete(Ok(Some(
        DisplayLayout::new(
            Rect::new(-1200, 0, 3120, 1080).unwrap(),
            Rect::new(-1200, 0, 1200, 900).unwrap(),
            1.5,
            selection,
        )
        .unwrap(),
    )));
    fixture.panel.invoke_shortcuts_event_ready();
    assert!(
        fixture.panel.window().is_visible(),
        "new pending Settings is preserved"
    );
    assert_eq!(
        shortcuts.configurations().len(),
        1,
        "Logout never mutates bindings"
    );
    assert!(native.actions.lock().is_empty());
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn direct_user_logout_is_independent_of_accepted_profile_read_and_late_identity_has_no_effect() {
    use tessera_system::profile::{
        ProfileCommand, ProfileError, ProfileHost, ProfileOpenCompletion, ProfilePhotoState,
        ProfileReadCompletion, ProfileSnapshot,
    };
    #[derive(Default)]
    struct ReadingProfile {
        pending: Mutex<Option<ProfileReadCompletion>>,
    }
    impl ProfileHost for ReadingProfile {
        fn read(&self, completion: ProfileReadCompletion) -> Result<(), ProfileError> {
            assert!(self.pending.lock().replace(completion).is_none());
            Ok(())
        }
        fn execute(&self, _: ProfileCommand, _: ProfileOpenCompletion) -> Result<(), ProfileError> {
            panic!("Logout must never use ProfileHost.execute");
        }
    }
    let (fixture, display, native) =
        fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let profile = Arc::new(ReadingProfile::default());
    *fixture.host.profile_provider.lock() = Some(profile.clone());
    let user = open_logout_user(&fixture);
    assert!(user.component().get_profile_loading());
    assert!(profile.pending.lock().is_some());
    assert!(user.component().get_logout_ready());
    click_component(user.component(), "Log out");
    assert_eq!(native.actions.lock().as_slice(), [PowerAction::LogOut]);
    assert!(
        profile.pending.lock().is_some(),
        "visual retirement does not cancel accepted read"
    );
    let completion = profile.pending.lock().take().unwrap();
    completion(Ok(ProfileSnapshot::new(
        "Late recording identity".into(),
        ProfilePhotoState::Absent,
        None,
        None,
    )
    .unwrap()));
    user.component().invoke_profile_event_ready();
    assert!(!user.is_open());
    assert!(user.component().get_profile_name().is_empty());
    assert!(user.component().get_logout_key().is_empty());
    native.complete();
    fixture.panel.invoke_power_command_event_ready();
    assert_no_logout_side_routes(&fixture, &display);
}

#[test]
fn older_user_reopen_retirement_cannot_present_or_focus_over_new_settings_intent() {
    use crate::user_menu::LogoutRetirementStage;
    let (fixture, _, native) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let user = open_logout_user(&fixture);
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let root = fixture.controller.clone();
    user.on_logout_retirement(LogoutRetirementStage::ProfileRetired, move || {
        root.open_panel()
    });
    fixture.click_launcher("Open user menu");
    assert!(fixture.panel.window().is_visible());
    assert!(!user.is_open());
    assert!(!user.component().window().is_visible());
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus + 1
    );
    assert!(native.actions.lock().is_empty());
    assert!(fixture.controller.power_menu.borrow().is_none());
    assert!(fixture.host.saves.lock().is_empty());
}

fn assert_user_logout_exhaustion_keeps_other_user_intents(counter: &'static str) {
    use super::shortcut_profile_tests::{RootFolders, RootProfile};
    use tessera_system::folders::FolderId;
    let (fixture, _, power) = fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
    let folders = Arc::new(RootFolders::default());
    let profile = Arc::new(RootProfile::default());
    *fixture.host.folder_provider.lock() = Some(folders.clone());
    *fixture.host.profile_provider.lock() = Some(profile.clone());
    let user = open_logout_user(&fixture);
    user.component().invoke_folder_event_ready();
    profile.finish();
    user.component().invoke_profile_event_ready();
    assert_eq!(user.component().get_rows().row_count(), FolderId::ALL.len());
    assert_eq!(user.component().get_profile_name(), "Recording user");
    user.exhaust_logout_counter_for_test(counter);
    if counter == "issuance" {
        click_component(user.component(), "Log out");
    } else {
        // The real next prepare/activation, not injected UI flags, reaches the
        // exhausted presentation-key/session counter.
        user.hide();
        open_logout_user(&fixture);
        user.component().invoke_folder_event_ready();
        profile.finish();
        user.component().invoke_profile_event_ready();
    }
    assert!(!user.component().get_logout_ready());
    assert!(user.component().get_logout_key().is_empty());
    assert!(power.actions.lock().is_empty());
    assert!(fixture.controller.power_menu.borrow().is_none());
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
    let drops = fixture.host.lease_drops.load(Ordering::SeqCst);
    user.hide();
    assert!(
        !user.is_open(),
        "exhausted Logout cannot block User teardown"
    );
    assert!(!user.component().window().is_visible());
    assert_eq!(fixture.host.lease_drops.load(Ordering::SeqCst), drops + 1);
    user.hide();
    assert_eq!(fixture.host.lease_drops.load(Ordering::SeqCst), drops + 1);
    open_logout_user(&fixture);
    user.component().invoke_folder_event_ready();
    profile.finish();
    user.component().invoke_profile_event_ready();
    assert_eq!(user.component().get_rows().row_count(), FolderId::ALL.len());
    assert_eq!(user.component().get_profile_name(), "Recording user");
    assert!(!user.component().get_logout_ready());
    assert!(user.component().get_logout_key().is_empty());
    click_component(user.component(), "Open Desktop");
    user.component().invoke_folder_event_ready();
    click_component(user.component(), "Open home folder");
    user.component().invoke_profile_event_ready();
    assert_eq!(folders.opened.lock().as_slice(), [FolderId::Desktop]);
    assert_eq!(profile.opened.lock().as_slice(), ["home"]);
    assert!(power.actions.lock().is_empty());
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn root_user_logout_issuance_exhaustion_retires_popup_and_reopens_folders_profile() {
    assert_user_logout_exhaustion_keeps_other_user_intents("issuance");
}

#[test]
fn root_user_logout_key_exhaustion_retires_popup_and_reopens_folders_profile() {
    assert_user_logout_exhaustion_keeps_other_user_intents("keys");
}

#[test]
fn root_user_logout_session_exhaustion_retires_popup_and_reopens_folders_profile() {
    assert_user_logout_exhaustion_keeps_other_user_intents("sessions");
}

#[test]
fn exhausted_user_logout_reopen_reentry_preserves_exact_new_user_or_settings_intent() {
    use crate::user_menu::LogoutRetirementStage;
    for lease_drop in [false, true] {
        for settings in [false, true] {
            let (fixture, _, power) =
                fixture_with_power(DisplayReply::Ready(desktop_layout()), false);
            let user = open_logout_user(&fixture);
            user.exhaust_logout_counter_for_test("issuance");
            click_component(user.component(), "Log out");
            assert!(user.is_open());
            assert!(user.component().get_logout_key().is_empty());
            let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
            let root = fixture.controller.clone();
            let launcher = fixture.launcher.as_weak();
            let callback = move || {
                if settings {
                    root.open_panel();
                } else {
                    click_component(&launcher.upgrade().unwrap(), "Open user menu");
                }
            };
            if lease_drop {
                RECYCLE_MENU_DROP_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(callback)));
            } else {
                user.on_logout_retirement(LogoutRetirementStage::ProfileRetired, callback);
            }
            fixture.click_launcher("Open user menu");
            assert_eq!(
                fixture.host.ui_focus_calls.load(Ordering::SeqCst),
                focus + 1
            );
            assert!(user.component().get_logout_key().is_empty());
            assert!(!user.component().get_logout_ready());
            if settings {
                assert!(fixture.panel.window().is_visible());
                assert!(
                    !user.is_open(),
                    "old exhausted User cannot regain native presentation"
                );
            } else {
                assert!(
                    user.is_open(),
                    "old exhausted hide cannot tear down replacement"
                );
                assert!(user.component().window().is_visible());
                assert!(!fixture.panel.window().is_visible());
            }
            assert!(power.actions.lock().is_empty());
            assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
            assert!(fixture.controller.power_menu.borrow().is_none());
            assert!(fixture.host.saves.lock().is_empty());
            RECYCLE_MENU_DROP_HOOK.with(|hook| assert!(hook.borrow().is_none()));
        }
    }
}
