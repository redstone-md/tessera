// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::{DockRecycleState, DockStatus, SeelenPalette};
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use parking_lot::Mutex;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct Host {
    events: Arc<Mutex<Vec<String>>>,
    observations: AtomicUsize,
    forbidden_calls: AtomicUsize,
    recycle_acquisitions: AtomicUsize,
    mutation_acquisitions: AtomicUsize,
    detach_callback: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    deny_focus: AtomicBool,
    deny_attach: AtomicBool,
    tooltip_role: AtomicBool,
}

struct Lease {
    events: Arc<Mutex<Vec<String>>>,
    on_drop: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.events.lock().push("detach".into());
        if let Some(callback) = self.on_drop.take() {
            callback();
        }
    }
}

impl DesktopHost for Host {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        self.observations.fetch_add(1, Ordering::Relaxed);
        Err("Menu must not observe the desktop".into())
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        self.forbidden_calls.fetch_add(1, Ordering::Relaxed);
        Err("Unexpected direct activation".into())
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        self.forbidden_calls.fetch_add(1, Ordering::Relaxed);
        Err("Unexpected direct launch".into())
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        self.forbidden_calls.fetch_add(1, Ordering::Relaxed);
        Err("Unexpected direct system action".into())
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        self.forbidden_calls.fetch_add(1, Ordering::Relaxed);
        Err("Unexpected direct save".into())
    }
    fn window_action(&self, _: &str, _: crate::WindowAction) -> Result<(), String> {
        self.forbidden_calls.fetch_add(1, Ordering::Relaxed);
        Err("Unexpected direct window command".into())
    }
    fn subscribe(&self, _: Arc<dyn Fn() + Send + Sync>) -> Result<Option<Box<dyn Send>>, String> {
        self.forbidden_calls.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn recycle_bin_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::recycle_bin::RecycleBinHost>>,
        tessera_system::dock_utilities::DockUtilityError,
    > {
        self.recycle_acquisitions.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn recycle_bin_mutation_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::recycle_bin_mutation::RecycleBinMutationHost>>,
        tessera_system::dock_utilities::DockUtilityError,
    > {
        self.mutation_acquisitions.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn configure_surface(
        &self,
        kind: SurfaceKind,
        _: &slint::Window,
    ) -> Result<Option<Box<dyn std::any::Any>>, String> {
        assert_eq!(
            kind,
            if self.tooltip_role.load(Ordering::Relaxed) {
                SurfaceKind::Tooltip
            } else {
                SurfaceKind::Popup
            }
        );
        if self.deny_attach.load(Ordering::Relaxed) {
            self.events.lock().push("attach-denied".into());
            return Err("Native surface attachment denied".into());
        }
        self.events.lock().push("attach".into());
        Ok(Some(Box::new(Lease {
            events: Arc::clone(&self.events),
            on_drop: self.detach_callback.lock().take(),
        })))
    }
    fn request_ui_focus(&self, _: &slint::Window) -> Result<(), String> {
        self.events.lock().push("focus".into());
        if self.deny_focus.load(Ordering::Relaxed) {
            Err("denied".into())
        } else {
            Ok(())
        }
    }
}

fn setup() -> (Arc<Host>, Dock, Rc<ContextMenuController>) {
    i_slint_backend_testing::init_no_event_loop();
    let host = Arc::new(Host::default());
    let dock = Dock::new().unwrap();
    let menu = ContextMenuController::new(host.clone(), &dock).unwrap();
    (host, dock, menu)
}

fn show(menu: &Rc<ContextMenuController>, kind: DockMenuKind) -> Result<(), String> {
    menu.show(
        kind,
        "opaque window key".into(),
        slint::PhysicalPosition::new(100, 900),
        DockContext::new(0, 0, 1920, 1080, false).unwrap(),
    )
}

fn press(menu: &ContextMenuController, key: Key) {
    let text: SharedString = key.into();
    menu.surface
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    menu.surface
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text });
}

fn assert_no_effects(host: &Host) {
    assert_eq!(host.observations.load(Ordering::Relaxed), 0);
    assert_eq!(host.forbidden_calls.load(Ordering::Relaxed), 0);
    assert_eq!(host.recycle_acquisitions.load(Ordering::Relaxed), 0);
    assert_eq!(host.mutation_acquisitions.load(Ordering::Relaxed), 0);
}

fn click_row(menu: &ContextMenuController, label: &str) {
    let mut rows = ElementHandle::find_by_accessible_label(menu.component(), label)
        .filter(|element| element.accessible_role() == Some(AccessibleRole::Button));
    let row = rows
        .next()
        .expect("Recycle menu must contain its real command button");
    assert!(rows.next().is_none(), "Command row must be unique");
    let origin = row.absolute_position();
    let size = row.size();
    assert!(size.width > 0.0 && size.height > 0.0);
    let position =
        slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    let window = menu.component().window();
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn record_other_commands(dock: &Dock) -> Rc<RefCell<Vec<&'static str>>> {
    let commands = Rc::new(RefCell::new(Vec::new()));
    let recorded = Rc::clone(&commands);
    dock.on_launch_requested(move |_| recorded.borrow_mut().push("launch"));
    let recorded = Rc::clone(&commands);
    dock.on_window_command_requested(move |_, _| recorded.borrow_mut().push("window"));
    let recorded = Rc::clone(&commands);
    dock.on_pin_toggle_requested(move |_, _| recorded.borrow_mut().push("pin"));
    let recorded = Rc::clone(&commands);
    dock.on_system_command_requested(move |_| recorded.borrow_mut().push("system"));
    let recorded = Rc::clone(&commands);
    dock.on_open_settings_requested(move || recorded.borrow_mut().push("settings"));
    let recorded = Rc::clone(&commands);
    dock.on_exit_requested(move || recorded.borrow_mut().push("exit"));
    let recorded = Rc::clone(&commands);
    dock.on_reserved_action_requested(move |_| recorded.borrow_mut().push("reserved"));
    let recorded = Rc::clone(&commands);
    dock.on_open_applications_requested(move || recorded.borrow_mut().push("applications"));
    commands
}

#[test]
fn typed_action_drops_lease_before_parent_callback_and_no_callback_owns_menu() {
    let (host, dock, menu) = setup();
    let weak = Rc::downgrade(&menu);
    let callback_menu = weak.clone();
    let events = Arc::clone(&host.events);
    dock.on_window_command_requested(move |key, action| {
        let menu = callback_menu.upgrade().unwrap();
        assert!(!menu.surface.is_visible());
        assert_eq!(events.lock().last().unwrap(), "detach");
        assert_eq!(action, DockWindowCommand::Activate);
        assert_eq!(key, "opaque window key");
        events.lock().push("activate".into());
    });
    show(&menu, DockMenuKind::Window).unwrap();
    menu.surface.invoke_action_requested(DockMenuAction::Exit);
    assert!(
        menu.surface.is_visible(),
        "wrong-kind action must not execute or detach"
    );
    assert_eq!(&*host.events.lock(), &["attach", "focus"]);
    press(&menu, Key::Return);
    assert_eq!(
        &*host.events.lock(),
        &["attach", "focus", "detach", "activate"]
    );
    menu.surface
        .invoke_action_requested(DockMenuAction::Activate);
    assert_eq!(
        host.events.lock().len(),
        4,
        "hidden menu must reject repeated input"
    );
    assert_eq!(host.observations.load(Ordering::Relaxed), 0);
    drop(menu);
    assert!(
        weak.upgrade().is_none(),
        "component callbacks must not retain the menu controller"
    );
}

#[test]
fn generated_keyboard_navigation_and_escape_keep_commands_bounded() {
    let (host, dock, menu) = setup();
    let actions = Rc::new(RefCell::new(Vec::new()));
    let captured = Rc::clone(&actions);
    dock.on_pin_toggle_requested(move |key, pin| captured.borrow_mut().push((key, pin)));
    show(&menu, DockMenuKind::Pinned).unwrap();
    press(&menu, Key::UpArrow);
    assert_eq!(menu.surface.get_selected_index(), 1);
    press(&menu, Key::DownArrow);
    assert_eq!(menu.surface.get_selected_index(), 0);
    press(&menu, Key::End);
    assert_eq!(menu.surface.get_selected_index(), 1);
    press(&menu, Key::Home);
    assert_eq!(menu.surface.get_selected_index(), 0);
    menu.surface.set_selected_index(99);
    press(&menu, Key::Return);
    assert!(menu.surface.is_visible());
    assert!(actions.borrow().is_empty());
    press(&menu, Key::End);
    press(&menu, Key::Space);
    assert_eq!(&*actions.borrow(), &[("opaque window key".into(), false)]);
    assert!(!menu.surface.is_visible());
    show(&menu, DockMenuKind::Bar).unwrap();
    press(&menu, Key::Escape);
    assert!(!menu.surface.is_visible());
    assert_eq!(host.events.lock().last().unwrap(), "detach");
    assert_eq!(host.observations.load(Ordering::Relaxed), 0);
}

#[test]
fn foreground_denial_leaves_mouse_commands_available_and_teardown_releases_lease() {
    let (host, dock, menu) = setup();
    host.deny_focus.store(true, Ordering::Relaxed);
    dock.global::<SeelenPalette>()
        .set_color_scheme(slint::language::ColorScheme::Dark);
    assert!(
        show(&menu, DockMenuKind::Bar)
            .unwrap_err()
            .contains("Menu opened")
    );
    assert!(menu.surface.is_visible());
    assert_eq!(
        menu.surface.global::<SeelenPalette>().get_color_scheme(),
        slint::language::ColorScheme::Dark
    );
    let events = Arc::clone(&host.events);
    dock.on_open_settings_requested(move || events.lock().push("settings".into()));
    menu.surface
        .invoke_action_requested(DockMenuAction::Settings);
    assert_eq!(
        &*host.events.lock(),
        &["attach", "focus", "detach", "settings"]
    );
    assert!(show(&menu, DockMenuKind::Bar).is_err());
    drop(menu);
    assert_eq!(host.events.lock().last().unwrap(), "detach");
    assert_eq!(host.observations.load(Ordering::Relaxed), 0);
}

#[test]
fn attachment_failure_hides_without_focus_and_a_later_show_can_recover() {
    let (host, _dock, menu) = setup();
    host.deny_attach.store(true, Ordering::Relaxed);
    assert_eq!(
        show(&menu, DockMenuKind::Bar).unwrap_err(),
        "Native surface attachment denied"
    );
    assert!(!menu.surface.is_visible());
    assert!(menu.surface.request_focus().is_err());
    assert_eq!(&*host.events.lock(), &["attach-denied"]);

    host.deny_attach.store(false, Ordering::Relaxed);
    show(&menu, DockMenuKind::Bar).unwrap();
    show(&menu, DockMenuKind::Bar).unwrap();
    assert_eq!(
        &*host.events.lock(),
        &[
            "attach-denied",
            "attach",
            "focus",
            "detach",
            "attach",
            "focus"
        ]
    );
    menu.hide();
    assert!(!menu.surface.is_visible());
    assert_eq!(host.events.lock().last().unwrap(), "detach");
    assert_eq!(host.observations.load(Ordering::Relaxed), 0);
}

#[test]
fn passive_transient_surface_never_forwards_a_focus_request() {
    let (host, _dock, _menu) = setup();
    host.tooltip_role.store(true, Ordering::Relaxed);
    let tooltip = TransientWindow::new(
        host.clone(),
        crate::generated::TooltipSurface::new().unwrap(),
        SurfaceKind::Tooltip,
    );
    tooltip.set_content("A genuine title".into());
    tooltip
        .present(
            slint::PhysicalPosition::new(100, 100),
            slint::PhysicalSize::new(160, 48),
        )
        .unwrap();
    assert!(tooltip.is_visible());
    assert!(tooltip.request_focus().is_err());
    assert_eq!(&*host.events.lock(), &["attach"]);
    drop(tooltip);
    assert_eq!(&*host.events.lock(), &["attach", "detach"]);
    assert_eq!(host.observations.load(Ordering::Relaxed), 0);
}

#[test]
fn target_images_are_kind_scoped_and_reset_before_reusing_the_menu() {
    let (host, dock, menu) = setup();
    let pinned = slint::Image::from_rgba8(slint::SharedPixelBuffer::new(3, 5));
    let running = slint::Image::from_rgba8(slint::SharedPixelBuffer::new(7, 9));
    let empty = slint::Image::default().size();
    dock.set_pinned_apps(slint::ModelRc::new(slint::VecModel::from(vec![
        crate::generated::DockApp {
            key: "opaque window key".into(),
            label: "Pinned application".into(),
            icon: pinned.clone(),
            pinned: true,
        },
    ])));
    dock.set_running_windows(slint::ModelRc::new(slint::VecModel::from(vec![
        crate::generated::DockWindow {
            key: "opaque window key".into(),
            caption: "Running application".into(),
            icon: running.clone(),
        },
    ])));
    for (kind, expected) in [
        (DockMenuKind::Pinned, pinned.size()),
        (DockMenuKind::Window, running.size()),
        (DockMenuKind::Bar, empty),
        (DockMenuKind::Recycle, empty),
    ] {
        show(&menu, kind).unwrap();
        assert_eq!(menu.surface.get_target_icon().size(), expected);
    }
    dock.set_running_windows(slint::ModelRc::default());
    show(&menu, DockMenuKind::Window).unwrap();
    assert_eq!(menu.surface.get_target_icon().size(), empty);
    dock.set_pinned_apps(slint::ModelRc::default());
    show(&menu, DockMenuKind::Pinned).unwrap();
    assert_eq!(menu.surface.get_target_icon().size(), empty);
    menu.hide();
    assert_eq!(host.observations.load(Ordering::Relaxed), 0);
}

#[test]
fn recycle_scope_authorizes_retry_and_detaches_before_typed_dispatch() {
    let (host, dock, menu) = setup();
    let weak = Rc::downgrade(&menu);
    let callback_menu = weak.clone();
    let events = Arc::clone(&host.events);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let captured = Rc::clone(&actions);
    dock.on_recycle_action_requested(move |action| {
        let menu = callback_menu.upgrade().unwrap();
        assert!(!menu.is_open());
        assert_eq!(events.lock().last().unwrap(), "detach");
        assert_eq!(action, DockRecycleAction::Retry);
        captured.borrow_mut().push(action);
        events.lock().push("retry".into());
    });
    for kind in [
        DockMenuKind::Bar,
        DockMenuKind::Pinned,
        DockMenuKind::Window,
    ] {
        assert!(!allowed(kind, DockMenuAction::RecycleRetry));
        show(&menu, kind).unwrap();
        let before = host.events.lock().clone();
        menu.component()
            .invoke_action_requested(DockMenuAction::RecycleRetry);
        assert!(menu.is_open(), "wrong scope must not execute or detach");
        assert_eq!(*host.events.lock(), before);
        assert!(actions.borrow().is_empty());
    }
    menu.hide();
    host.events.lock().clear();
    show(&menu, DockMenuKind::Recycle).unwrap();
    assert!(
        menu.key.borrow().is_empty(),
        "Recycle has no caller-controlled key"
    );
    for action in [
        DockMenuAction::Settings,
        DockMenuAction::FileManager,
        DockMenuAction::TaskManager,
        DockMenuAction::Restore,
        DockMenuAction::Exit,
        DockMenuAction::Launch,
        DockMenuAction::Unpin,
        DockMenuAction::Activate,
        DockMenuAction::Minimize,
        DockMenuAction::Close,
    ] {
        assert!(!allowed(DockMenuKind::Recycle, action));
        menu.component().invoke_action_requested(action);
        assert!(menu.is_open(), "Recycle must reject unrelated authority");
        assert!(actions.borrow().is_empty());
        assert_eq!(&*host.events.lock(), &["attach", "focus"]);
    }
    assert!(allowed(DockMenuKind::Recycle, DockMenuAction::RecycleRetry));
    press(&menu, Key::Home);
    press(&menu, Key::Return);
    assert_eq!(&*actions.borrow(), &[DockRecycleAction::Retry]);
    assert_eq!(
        &*host.events.lock(),
        &["attach", "focus", "detach", "retry"]
    );
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleRetry);
    assert_eq!(actions.borrow().len(), 1, "hidden input must be rejected");
    assert_no_effects(&host);

    let retained_component = menu.component().clone_strong();
    drop(menu);
    assert!(
        weak.upgrade().is_none(),
        "surface callbacks must remain weak"
    );
    retained_component.invoke_action_requested(DockMenuAction::RecycleRetry);
    assert_eq!(actions.borrow().len(), 1);
    assert_no_effects(&host);
}

#[test]
fn recycle_native_key_navigation_skips_disabled_empty_and_escape_is_non_effectful() {
    let (host, dock, menu) = setup();
    let actions = Rc::new(RefCell::new(Vec::new()));
    let captured = Rc::clone(&actions);
    dock.on_recycle_action_requested(move |action| captured.borrow_mut().push(action));
    show(&menu, DockMenuKind::Recycle).unwrap();
    press(&menu, Key::Escape);
    assert!(!menu.is_open());
    assert_eq!(host.events.lock().last().unwrap(), "detach");
    assert!(actions.borrow().is_empty());

    show(&menu, DockMenuKind::Recycle).unwrap();
    for key in [Key::UpArrow, Key::DownArrow, Key::End, Key::Home] {
        press(&menu, key);
        assert_eq!(menu.component().get_selected_index(), 1);
        assert!(menu.is_open());
    }
    menu.component().set_selected_index(99);
    press(&menu, Key::Return);
    assert!(menu.is_open());
    assert!(actions.borrow().is_empty());
    press(&menu, Key::Home);
    press(&menu, Key::Space);
    assert!(!menu.is_open());
    assert_eq!(&*actions.borrow(), &[DockRecycleAction::Retry]);
    assert_no_effects(&host);
}

#[test]
fn recycle_focus_denial_preserves_real_pointer_retry_and_lease_order() {
    let (host, dock, menu) = setup();
    host.deny_focus.store(true, Ordering::Relaxed);
    let callback_menu = Rc::downgrade(&menu);
    let events = Arc::clone(&host.events);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let captured = Rc::clone(&actions);
    dock.on_recycle_action_requested(move |action| {
        assert!(!callback_menu.upgrade().unwrap().is_open());
        assert_eq!(events.lock().last().unwrap(), "detach");
        captured.borrow_mut().push(action);
        events.lock().push("retry".into());
    });
    assert!(
        show(&menu, DockMenuKind::Recycle)
            .unwrap_err()
            .contains("Menu opened")
    );
    assert!(menu.is_open());
    assert!(actions.borrow().is_empty());
    assert_no_effects(&host);
    click_row(&menu, "Retry");
    assert!(!menu.is_open());
    assert_eq!(&*actions.borrow(), &[DockRecycleAction::Retry]);
    assert_eq!(
        &*host.events.lock(),
        &["attach", "focus", "detach", "retry"]
    );
    assert_no_effects(&host);
}

#[test]
fn recycle_attachment_denial_rejects_retry_and_later_show_recovers() {
    let (host, dock, menu) = setup();
    let actions = Rc::new(RefCell::new(Vec::new()));
    let captured = Rc::clone(&actions);
    dock.on_recycle_action_requested(move |action| captured.borrow_mut().push(action));
    host.deny_attach.store(true, Ordering::Relaxed);
    assert_eq!(
        show(&menu, DockMenuKind::Recycle).unwrap_err(),
        "Native surface attachment denied"
    );
    assert!(!menu.is_open());
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleRetry);
    assert!(actions.borrow().is_empty());
    assert_eq!(&*host.events.lock(), &["attach-denied"]);
    assert_no_effects(&host);

    host.deny_attach.store(false, Ordering::Relaxed);
    show(&menu, DockMenuKind::Recycle).unwrap();
    press(&menu, Key::Home);
    press(&menu, Key::Return);
    assert!(!menu.is_open());
    assert_eq!(&*actions.borrow(), &[DockRecycleAction::Retry]);
    assert_eq!(
        &*host.events.lock(),
        &["attach-denied", "attach", "focus", "detach"]
    );
    assert_no_effects(&host);
}

#[test]
fn recycle_dropped_weak_dock_detaches_without_dispatch_or_resurrection() {
    let (host, dock, menu) = setup();
    let actions = Rc::new(RefCell::new(Vec::new()));
    let captured = Rc::clone(&actions);
    dock.on_recycle_action_requested(move |action| captured.borrow_mut().push(action));
    show(&menu, DockMenuKind::Recycle).unwrap();
    drop(dock);
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleRetry);
    assert!(!menu.is_open());
    assert!(actions.borrow().is_empty());
    assert_eq!(&*host.events.lock(), &["attach", "focus", "detach"]);
    assert_eq!(
        show(&menu, DockMenuKind::Recycle).unwrap_err(),
        "The dock is no longer available."
    );
    assert!(!menu.is_open());
    assert_no_effects(&host);
}

#[test]
fn recycle_empty_genuine_inputs_detach_before_dispatch_without_read_or_app_gating() {
    let (host, dock, menu) = setup();
    let other_commands = record_other_commands(&dock);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = Rc::clone(&actions);
    let callback_menu = Rc::downgrade(&menu);
    let events = Arc::clone(&host.events);
    dock.on_recycle_action_requested(move |action| {
        let menu = callback_menu.upgrade().unwrap();
        assert!(!menu.is_open());
        assert_eq!(events.lock().last().unwrap(), "detach");
        assert_eq!(action, DockRecycleAction::Empty);
        // No scope borrow may survive into a parent callback.
        assert!(menu.key.borrow_mut().is_empty());
        recorded.borrow_mut().push(action);
        events.lock().push("empty".into());
    });
    dock.show().unwrap();
    dock.set_recycle_empty_enabled(true);
    dock.set_recycle_read_busy(true);
    dock.set_recycle_open_busy(true);
    dock.set_recycle_stale(true);
    dock.set_recycle_read_notice("Unavailable".into());
    dock.set_recycle_watch_notice("Unavailable".into());
    dock.set_surface_status(DockStatus {
        refreshing: true,
        stale: true,
        ..Default::default()
    });
    for (state, count, key) in [
        (DockRecycleState::Unknown, "", None),
        (DockRecycleState::Empty, "0 items", Some(Key::Return)),
        (DockRecycleState::Full, "12 items", Some(Key::Space)),
    ] {
        dock.set_recycle_state(state);
        dock.set_recycle_item_count_label(count.into());
        host.events.lock().clear();
        show(&menu, DockMenuKind::Recycle).unwrap();
        assert!(menu.key.borrow().is_empty());
        assert!(menu.component().get_recycle_empty_enabled());
        assert_no_effects(&host);
        if let Some(key) = key {
            press(&menu, Key::Home);
            assert_eq!(menu.component().get_selected_index(), 0);
            press(&menu, key);
        } else {
            click_row(&menu, "Empty Recycle Bin");
        }
        assert!(!menu.is_open());
        assert_eq!(
            &*host.events.lock(),
            &["attach", "focus", "detach", "empty"]
        );
        assert!(other_commands.borrow().is_empty());
        assert_no_effects(&host);
    }
    assert_eq!(&*actions.borrow(), &[DockRecycleAction::Empty; 3]);
}

#[test]
fn recycle_empty_revalidates_stale_scope_key_busy_and_hidden_intents() {
    let (host, dock, menu) = setup();
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = Rc::clone(&actions);
    dock.on_recycle_action_requested(move |action| recorded.borrow_mut().push(action));
    dock.show().unwrap();
    dock.set_recycle_empty_enabled(true);
    for kind in [
        DockMenuKind::Bar,
        DockMenuKind::Pinned,
        DockMenuKind::Window,
    ] {
        assert!(!allowed(kind, DockMenuAction::RecycleEmpty));
        show(&menu, kind).unwrap();
        let before = host.events.lock().clone();
        menu.component()
            .invoke_action_requested(DockMenuAction::RecycleEmpty);
        assert!(menu.is_open());
        assert_eq!(*host.events.lock(), before);
    }
    assert!(allowed(DockMenuKind::Recycle, DockMenuAction::RecycleEmpty));
    show(&menu, DockMenuKind::Recycle).unwrap();
    let before = host.events.lock().clone();
    *menu.key.borrow_mut() = "stale application key".into();
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleEmpty);
    assert_eq!(*host.events.lock(), before);
    *menu.key.borrow_mut() = SharedString::default();
    // A queued row can still look enabled while the live projection is busy.
    dock.set_recycle_empty_busy(true);
    dock.set_recycle_empty_enabled(false);
    assert!(menu.component().get_recycle_empty_enabled());
    click_row(&menu, "Empty Recycle Bin");
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleEmpty);
    assert!(menu.is_open());
    assert_eq!(*host.events.lock(), before);
    dock.set_recycle_empty_busy(false);
    dock.set_recycle_empty_enabled(true);
    dock.hide().unwrap();
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleEmpty);
    assert!(menu.is_open());
    assert_eq!(*host.events.lock(), before);
    dock.show().unwrap();
    menu.hide();
    let before = host.events.lock().clone();
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleEmpty);
    assert_eq!(*host.events.lock(), before);
    assert!(actions.borrow().is_empty());
    assert_no_effects(&host);
    let weak = Rc::downgrade(&menu);
    let retained = menu.component().clone_strong();
    drop(menu);
    assert!(weak.upgrade().is_none());
    retained.invoke_action_requested(DockMenuAction::RecycleEmpty);
    assert!(actions.borrow().is_empty());
    assert_no_effects(&host);
}

#[test]
fn recycle_live_refresh_updates_retained_presentation_without_granting_closed_authority() {
    let (host, dock, menu) = setup();
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = Rc::clone(&actions);
    dock.on_recycle_action_requested(move |action| recorded.borrow_mut().push(action));
    dock.show().unwrap();
    show(&menu, DockMenuKind::Recycle).unwrap();
    assert!(!menu.component().get_recycle_empty_enabled());
    let before = host.events.lock().clone();
    dock.set_recycle_empty_enabled(true);
    menu.refresh_recycle_actions();
    assert!(menu.component().get_recycle_empty_enabled());
    dock.set_recycle_empty_enabled(false);
    menu.refresh_recycle_actions();
    assert!(!menu.component().get_recycle_empty_enabled());
    menu.component().set_selected_index(0);
    press(&menu, Key::Return);
    press(&menu, Key::Space);
    click_row(&menu, "Empty Recycle Bin");
    assert!(menu.is_open());
    assert!(actions.borrow().is_empty());
    assert_eq!(
        *host.events.lock(),
        before,
        "refresh must not attach or focus"
    );
    for key in [Key::UpArrow, Key::DownArrow, Key::Home, Key::End] {
        press(&menu, key);
        assert_eq!(menu.component().get_selected_index(), 1);
    }
    dock.set_recycle_empty_enabled(true);
    menu.refresh_recycle_actions();
    let active_generation = menu.scope_generation.get();
    dock.hide().unwrap();
    menu.refresh_recycle_actions();
    assert!(
        !menu.component().get_recycle_empty_enabled(),
        "hidden Dock has no live admission"
    );
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleEmpty);
    assert!(actions.borrow().is_empty());
    assert!(menu.is_open());
    assert_eq!(menu.scope_generation.get(), active_generation);
    assert_eq!(*host.events.lock(), before);
    dock.show().unwrap();
    menu.refresh_recycle_actions();
    assert!(menu.component().get_recycle_empty_enabled());
    press(&menu, Key::Home);
    assert_eq!(menu.component().get_selected_index(), 0);
    press(&menu, Key::Return);
    assert_eq!(&*actions.borrow(), &[DockRecycleAction::Empty]);
    assert!(!menu.is_open());
    let before = host.events.lock().clone();
    let retired_generation = menu.scope_generation.get();
    assert!(retired_generation.is_some());
    dock.set_recycle_empty_enabled(true);
    menu.refresh_recycle_actions();
    assert!(
        menu.component().get_recycle_empty_enabled(),
        "retained readback mirrors the live admitted Dock, not popup authority"
    );
    assert!(!menu.is_open());
    assert!(!menu.component().window().is_visible());
    assert_eq!(menu.scope_generation.get(), retired_generation);
    assert_eq!(
        *host.events.lock(),
        before,
        "readback cannot attach or focus"
    );
    menu.component().set_selected_index(0);
    press(&menu, Key::Return);
    press(&menu, Key::Space);
    click_row(&menu, "Empty Recycle Bin");
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleEmpty);
    assert_eq!(&*actions.borrow(), &[DockRecycleAction::Empty]);
    assert!(!menu.is_open());
    assert!(!menu.component().window().is_visible());
    assert_eq!(menu.scope_generation.get(), retired_generation);
    assert_eq!(
        *host.events.lock(),
        before,
        "closed intents cannot revive a native scope"
    );
    assert_no_effects(&host);
    menu.scope_generation.set(None);
    menu.refresh_recycle_actions();
    assert!(!menu.component().get_recycle_empty_enabled());
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleEmpty);
    assert_eq!(menu.scope_generation.get(), None);
    assert!(!menu.is_open());
    assert!(!menu.component().window().is_visible());
    assert_eq!(&*actions.borrow(), &[DockRecycleAction::Empty]);
    assert_eq!(
        *host.events.lock(),
        before,
        "exhausted readback cannot resume authority"
    );
    show(&menu, DockMenuKind::Bar).unwrap();
    let before = host.events.lock().clone();
    dock.set_recycle_empty_enabled(true);
    menu.refresh_recycle_actions();
    assert!(
        !menu.component().get_recycle_empty_enabled(),
        "other scopes are untouched"
    );
    assert_eq!(*host.events.lock(), before);
    assert_no_effects(&host);
}

#[test]
fn recycle_empty_focus_denial_keeps_real_pointer_route_and_detach_order() {
    let (host, dock, menu) = setup();
    host.deny_focus.store(true, Ordering::Relaxed);
    dock.show().unwrap();
    dock.set_recycle_empty_enabled(true);
    let events = Arc::clone(&host.events);
    let callback_menu = Rc::downgrade(&menu);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = Rc::clone(&actions);
    dock.on_recycle_action_requested(move |action| {
        assert!(!callback_menu.upgrade().unwrap().is_open());
        assert_eq!(events.lock().last().unwrap(), "detach");
        recorded.borrow_mut().push(action);
        events.lock().push("empty".into());
    });
    assert!(
        show(&menu, DockMenuKind::Recycle)
            .unwrap_err()
            .contains("Menu opened")
    );
    assert!(menu.is_open());
    click_row(&menu, "Empty Recycle Bin");
    assert_eq!(&*actions.borrow(), &[DockRecycleAction::Empty]);
    assert_eq!(
        &*host.events.lock(),
        &["attach", "focus", "detach", "empty"]
    );
    assert_no_effects(&host);
}

#[test]
fn recycle_empty_attachment_failure_and_dropped_dock_cannot_dispatch() {
    let (host, dock, menu) = setup();
    dock.show().unwrap();
    dock.set_recycle_empty_enabled(true);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = Rc::clone(&actions);
    dock.on_recycle_action_requested(move |action| recorded.borrow_mut().push(action));
    host.deny_attach.store(true, Ordering::Relaxed);
    assert!(show(&menu, DockMenuKind::Recycle).is_err());
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleEmpty);
    assert!(!menu.is_open());
    assert!(actions.borrow().is_empty());
    assert_eq!(&*host.events.lock(), &["attach-denied"]);
    host.deny_attach.store(false, Ordering::Relaxed);
    show(&menu, DockMenuKind::Recycle).unwrap();
    dock.hide().unwrap();
    drop(dock);
    let before = host.events.lock().clone();
    menu.refresh_recycle_actions();
    assert!(!menu.component().get_recycle_empty_enabled());
    assert_eq!(*host.events.lock(), before);
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleEmpty);
    assert!(!menu.is_open());
    assert_eq!(host.events.lock().last().unwrap(), "detach");
    assert!(actions.borrow().is_empty());
    assert!(show(&menu, DockMenuKind::Recycle).is_err());
    assert_no_effects(&host);
}

#[test]
fn recycle_empty_detach_reentry_retires_live_authority_or_replaced_scope() {
    let (host, dock, menu) = setup();
    dock.show().unwrap();
    dock.set_recycle_empty_enabled(true);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = Rc::clone(&actions);
    dock.on_recycle_action_requested(move |action| recorded.borrow_mut().push(action));
    for (replacement, hide_dock) in [
        (None, false),
        (None, true),
        (Some(DockMenuKind::Bar), false),
        (Some(DockMenuKind::Recycle), false),
    ] {
        let callback_menu = Rc::downgrade(&menu);
        let callback_dock = dock.as_weak();
        dock.on_recycle_event_ready(move || {
            if let Some(kind) = replacement {
                show(&callback_menu.upgrade().unwrap(), kind).unwrap();
            } else {
                let dock = callback_dock.upgrade().unwrap();
                if hide_dock {
                    dock.hide().unwrap();
                } else {
                    dock.set_recycle_empty_enabled(false);
                }
            }
        });
        let callback_dock = dock.as_weak();
        *host.detach_callback.lock() = Some(Box::new(move || {
            callback_dock
                .upgrade()
                .unwrap()
                .invoke_recycle_event_ready();
        }));
        show(&menu, DockMenuKind::Recycle).unwrap();
        let generation = menu.scope_generation.get();
        menu.component()
            .invoke_action_requested(DockMenuAction::RecycleEmpty);
        assert!(actions.borrow().is_empty());
        if replacement.is_some() {
            assert_ne!(
                menu.scope_generation.get(),
                generation.and_then(|generation| generation.checked_add(1)),
                "even a newly opened Recycle scope must retire the old intent"
            );
        }
        menu.hide();
        dock.show().unwrap();
        dock.set_recycle_empty_enabled(true);
        assert_no_effects(&host);
    }
}

#[test]
fn show_retirement_reentry_preserves_newer_scope_and_attachment() {
    let (host, dock, menu) = setup();
    for generation in [Some(0), Some(u64::MAX), None] {
        let callback_menu = Rc::downgrade(&menu);
        dock.on_recycle_event_ready(move || {
            let menu = callback_menu.upgrade().unwrap();
            menu.show(
                DockMenuKind::Pinned,
                "new pinned key".into(),
                slint::PhysicalPosition::new(300, 900),
                DockContext::new(0, 0, 1920, 1080, false).unwrap(),
            )
            .unwrap();
            menu.component().set_selected_index(1);
        });
        let callback_dock = dock.as_weak();
        *host.detach_callback.lock() = Some(Box::new(move || {
            callback_dock
                .upgrade()
                .unwrap()
                .invoke_recycle_event_ready();
        }));
        host.events.lock().clear();
        show(&menu, DockMenuKind::Recycle).unwrap();
        menu.scope_generation.set(generation);

        menu.show(
            DockMenuKind::Window,
            "outer window key".into(),
            slint::PhysicalPosition::new(700, 900),
            DockContext::new(0, 0, 1920, 1080, false).unwrap(),
        )
        .unwrap();

        assert!(
            menu.is_open(),
            "newer logical popup must survive retirement"
        );
        assert!(
            menu.component().window().is_visible(),
            "newer Slint window must survive retirement"
        );
        assert_eq!(menu.component().get_kind(), DockMenuKind::Pinned);
        assert_eq!(*menu.key.borrow(), "new pinned key");
        assert_eq!(menu.component().get_selected_index(), 1);
        assert_eq!(
            &*host.events.lock(),
            &["attach", "focus", "detach", "attach", "focus"],
            "outer show must not detach, attach, or refocus the replacement"
        );
        menu.hide();
        assert!(!menu.is_open());
        assert!(!menu.component().window().is_visible());
        assert_eq!(
            &*host.events.lock(),
            &["attach", "focus", "detach", "attach", "focus", "detach"],
            "replacement attachment must remain owned until its own retirement"
        );
        assert_no_effects(&host);
    }
}

#[test]
fn recycle_empty_exhausted_scope_generation_rejects_without_reuse_or_panic() {
    let (host, dock, menu) = setup();
    dock.show().unwrap();
    dock.set_recycle_empty_enabled(true);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = Rc::clone(&actions);
    dock.on_recycle_action_requested(move |action| recorded.borrow_mut().push(action));
    show(&menu, DockMenuKind::Recycle).unwrap();
    menu.scope_generation.set(Some(u64::MAX));
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleEmpty);
    assert_eq!(menu.scope_generation.get(), None);
    assert!(!menu.is_open());
    assert!(actions.borrow().is_empty());
    let retired_events = host.events.lock().clone();
    menu.refresh_recycle_actions();
    assert!(!menu.component().get_recycle_empty_enabled());
    assert_eq!(menu.scope_generation.get(), None);
    assert!(!menu.component().window().is_visible());
    assert_eq!(*host.events.lock(), retired_events);
    show(&menu, DockMenuKind::Recycle).unwrap();
    let before = host.events.lock().clone();
    menu.component()
        .invoke_action_requested(DockMenuAction::RecycleEmpty);
    assert!(menu.is_open());
    assert_eq!(menu.scope_generation.get(), None);
    assert_eq!(*host.events.lock(), before);
    assert!(actions.borrow().is_empty());
    assert_no_effects(&host);
}
