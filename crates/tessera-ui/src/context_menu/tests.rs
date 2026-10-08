// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::SeelenPalette;
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use parking_lot::Mutex;
use slint::platform::{Key, WindowEvent};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct Host {
    events: Arc<Mutex<Vec<String>>>,
    observations: AtomicUsize,
    deny_focus: AtomicBool,
    deny_attach: AtomicBool,
    tooltip_role: AtomicBool,
}

struct Lease(Arc<Mutex<Vec<String>>>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.lock().push("detach".into());
    }
}

impl DesktopHost for Host {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        self.observations.fetch_add(1, Ordering::Relaxed);
        Err("Menu must not observe the desktop".into())
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        Err("Unexpected direct activation".into())
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        Err("Unexpected direct launch".into())
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        Err("Unexpected direct system action".into())
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        Err("Unexpected direct save".into())
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
        Ok(Some(Box::new(Lease(Arc::clone(&self.events)))))
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
