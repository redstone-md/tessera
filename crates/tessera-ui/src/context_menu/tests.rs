// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use parking_lot::Mutex;
use slint::platform::{Key, WindowEvent};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct Host {
    events: Arc<Mutex<Vec<String>>>,
    observations: AtomicUsize,
    deny_focus: AtomicBool,
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
        assert_eq!(kind, SurfaceKind::Popup);
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
    menu.surface
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
}

#[test]
fn typed_action_drops_lease_before_parent_callback_and_no_callback_owns_menu() {
    let (host, dock, menu) = setup();
    let weak = Rc::downgrade(&menu);
    let callback_menu = weak.clone();
    let events = Arc::clone(&host.events);
    dock.on_window_command_requested(move |key, action| {
        let menu = callback_menu.upgrade().unwrap();
        assert!(!menu.visible.get());
        assert!(menu.lease.borrow().is_none());
        assert_eq!(events.lock().last().unwrap(), "detach");
        assert_eq!(action, DockWindowCommand::Activate);
        assert_eq!(key, "opaque window key");
        events.lock().push("activate".into());
    });
    show(&menu, DockMenuKind::Window).unwrap();
    menu.surface.invoke_action_requested(DockMenuAction::Exit);
    assert!(
        menu.visible.get(),
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
    assert!(menu.visible.get());
    assert!(actions.borrow().is_empty());
    press(&menu, Key::End);
    press(&menu, Key::Space);
    assert_eq!(&*actions.borrow(), &[("opaque window key".into(), false)]);
    assert!(!menu.visible.get());
    show(&menu, DockMenuKind::Bar).unwrap();
    press(&menu, Key::Escape);
    assert!(!menu.visible.get());
    assert!(menu.lease.borrow().is_none());
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
    assert!(menu.visible.get());
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
