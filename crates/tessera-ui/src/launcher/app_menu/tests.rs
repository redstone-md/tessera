// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::{LaunchRow, LaunchTile, LauncherView};
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use parking_lot::Mutex;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ModelRc, VecModel};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

thread_local! {
    static DETACH: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::default();
}

#[derive(Default)]
pub(super) struct Host {
    events: Arc<Mutex<Vec<String>>>,
    observations: AtomicUsize,
    launches: AtomicUsize,
    saves: AtomicUsize,
    systems: AtomicUsize,
    subscriptions: AtomicUsize,
    providers: AtomicUsize,
    unrelated_commands: AtomicUsize,
    deny_attach: AtomicBool,
    deny_focus: AtomicBool,
    close_during_attach: AtomicBool,
}

struct Lease(Arc<Mutex<Vec<String>>>);

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.lock().push("detach".into());
        let callback = DETACH.with(|hook| hook.borrow_mut().take());
        if let Some(callback) = callback {
            callback();
        }
    }
}

impl DesktopHost for Host {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        self.observations.fetch_add(1, Ordering::Relaxed);
        Err("No desktop observation belongs to a favorite popup".into())
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        self.unrelated_commands.fetch_add(1, Ordering::Relaxed);
        Err("No activation belongs to a favorite popup".into())
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        self.launches.fetch_add(1, Ordering::Relaxed);
        Err("No launch belongs to a favorite popup".into())
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        self.systems.fetch_add(1, Ordering::Relaxed);
        Err("No system command belongs to a favorite popup".into())
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        self.saves.fetch_add(1, Ordering::Relaxed);
        Err("The existing parent transaction owns persistence".into())
    }
    fn subscribe(&self, _: Arc<dyn Fn() + Send + Sync>) -> Result<Option<Box<dyn Send>>, String> {
        self.subscriptions.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn audio_host(
        &self,
    ) -> Result<Option<Arc<dyn tessera_system::audio::AudioHost>>, tessera_system::audio::AudioError>
    {
        self.providers.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn media_host(
        &self,
    ) -> Result<Option<Arc<dyn tessera_system::media::MediaHost>>, tessera_system::media::MediaError>
    {
        self.providers.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn folder_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::folders::FolderHost>>,
        tessera_system::folders::FolderError,
    > {
        self.providers.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn calendar_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::calendar::CalendarHost>>,
        tessera_system::calendar::CalendarError,
    > {
        self.providers.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn dock_utilities_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::dock_utilities::DockUtilitiesHost>>,
        tessera_system::dock_utilities::DockUtilityError,
    > {
        self.providers.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn recycle_bin_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::recycle_bin::RecycleBinHost>>,
        tessera_system::dock_utilities::DockUtilityError,
    > {
        self.providers.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn recycle_bin_mutation_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::recycle_bin_mutation::RecycleBinMutationHost>>,
        tessera_system::dock_utilities::DockUtilityError,
    > {
        self.providers.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn display_context_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::display_context::DisplayContextHost>>,
        tessera_system::display_context::DisplayContextError,
    > {
        self.providers.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn power_host(
        &self,
    ) -> Result<Option<Arc<dyn tessera_system::power::PowerHost>>, tessera_system::power::PowerError>
    {
        self.providers.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn power_updates_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::power_updates::PowerUpdatesHost>>,
        tessera_system::power_updates::PowerUpdatesError,
    > {
        self.providers.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn configure_surface(
        &self,
        kind: SurfaceKind,
        window: &slint::Window,
    ) -> Result<Option<Box<dyn std::any::Any>>, String> {
        assert_eq!(kind, SurfaceKind::Popup);
        if self.deny_attach.load(Ordering::Relaxed) {
            self.events.lock().push("attach-denied".into());
            return Err("Attachment denied".into());
        }
        self.events.lock().push("attach".into());
        if self.close_during_attach.load(Ordering::Relaxed) {
            window.dispatch_event(WindowEvent::CloseRequested);
            assert!(
                window.is_visible(),
                "in-flight lease must detach before hide"
            );
        }
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

pub(super) fn setup() -> (Arc<Host>, Launcher, Rc<LauncherAppMenu>) {
    i_slint_backend_testing::init_no_event_loop();
    setup_with_platform()
}

pub(super) fn setup_with_platform() -> (Arc<Host>, Launcher, Rc<LauncherAppMenu>) {
    DETACH.with(|hook| {
        hook.borrow_mut().take();
    });
    let host = Arc::new(Host::default());
    let launcher = Launcher::new().unwrap();
    launcher.set_view(LauncherView::All);
    launcher.set_rows(ModelRc::new(VecModel::from(vec![LaunchRow {
        tiles: ModelRc::new(VecModel::from(vec![LaunchTile {
            key: "opaque app key".into(),
            label: "Context application".into(),
            icon: Default::default(),
            favorite: false,
        }])),
    }])));
    launcher.set_application_count(1);
    launcher
        .window()
        .set_size(slint::PhysicalSize::new(900, 700));
    launcher.show().unwrap();
    launcher
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    launcher.invoke_focus_search();
    let menu = LauncherAppMenu::new(host.clone()).unwrap();
    (host, launcher, menu)
}

pub(super) fn show(
    menu: &Rc<LauncherAppMenu>,
    launcher: &Launcher,
    favorite: bool,
) -> Result<(), String> {
    menu.show(
        launcher,
        "opaque app key".into(),
        favorite,
        slint::LogicalPosition::new(150.0, 200.0),
        DockContext::new(0, 0, 1920, 1080, false).unwrap(),
    )
}

fn key(component: &impl ComponentHandle, key: Key) {
    let text: SharedString = key.into();
    component
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    component
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text });
}

pub(super) fn press(menu: &LauncherAppMenu, value: Key) {
    key(&*menu.surface, value);
}

fn button(component: &impl ComponentHandle, label: &str) -> ElementHandle {
    let mut rows = ElementHandle::find_by_accessible_label(component, label)
        .filter(|element| element.accessible_role() == Some(AccessibleRole::Button));
    let row = rows.next().expect("Expected a real accessible button");
    assert!(rows.next().is_none(), "Button must be unique");
    row
}

fn center(element: &ElementHandle) -> slint::LogicalPosition {
    let origin = element.absolute_position();
    let size = element.size();
    assert!(size.width > 0.0 && size.height > 0.0);
    slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
}

fn pointer(
    component: &impl ComponentHandle,
    position: slint::LogicalPosition,
    button: PointerEventButton,
) {
    component
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    component
        .window()
        .dispatch_event(WindowEvent::PointerPressed { position, button });
    component
        .window()
        .dispatch_event(WindowEvent::PointerReleased { position, button });
}

pub(super) fn click_row(menu: &LauncherAppMenu, label: &str) {
    pointer(
        &*menu.surface,
        center(&button(&*menu.surface, label)),
        PointerEventButton::Left,
    );
}

fn no_effects(host: &Host) {
    assert_eq!(host.observations.load(Ordering::Relaxed), 0);
    assert_eq!(host.launches.load(Ordering::Relaxed), 0);
    assert_eq!(host.saves.load(Ordering::Relaxed), 0);
    assert_eq!(host.systems.load(Ordering::Relaxed), 0);
    assert_eq!(host.subscriptions.load(Ordering::Relaxed), 0);
    assert_eq!(host.providers.load(Ordering::Relaxed), 0);
    assert_eq!(host.unrelated_commands.load(Ordering::Relaxed), 0);
}

fn record_requests(launcher: &Launcher) -> Rc<RefCell<Vec<(SharedString, bool)>>> {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let captured = requests.clone();
    launcher.on_favorite_toggle_requested(move |key, desired| {
        captured.borrow_mut().push((key, desired))
    });
    requests
}

#[test]
fn genuine_right_release_menu_and_shift_f10_forward_the_real_anchor_without_selection() {
    let (host, launcher, menu) = setup();
    let requests = record_requests(&launcher);
    let contexts = Rc::new(RefCell::new(Vec::new()));
    let captured = contexts.clone();
    let weak_menu = Rc::downgrade(&menu);
    let weak_launcher = launcher.as_weak();
    launcher.on_app_menu_requested(move |key, anchor| {
        captured.borrow_mut().push((key.clone(), anchor));
        weak_menu
            .upgrade()
            .unwrap()
            .show(
                &weak_launcher.upgrade().unwrap(),
                key,
                false,
                anchor,
                DockContext::new(0, 0, 1920, 1080, false).unwrap(),
            )
            .unwrap();
    });
    let selections = Rc::new(Cell::new(0));
    let captured = selections.clone();
    launcher.on_select_requested(move |_| captured.set(captured.get() + 1));
    let launches = Rc::new(Cell::new(0));
    let captured = launches.clone();
    launcher.on_launch_requested(move |_| captured.set(captured.get() + 1));
    let tile = button(&launcher, "Launch Context application");
    let position = center(&tile);
    launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    launcher
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Right,
        });
    assert!(!menu.is_open(), "press is not context activation");
    launcher
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Right,
        });
    assert!(menu.is_open());
    assert_eq!(contexts.borrow().len(), 1);
    assert_eq!(contexts.borrow()[0], ("opaque app key".into(), position));
    assert_eq!(
        selections.get(),
        0,
        "right-click focus must not select another application"
    );
    assert_eq!(launcher.get_selected_key(), "");
    button(&*menu.surface, "Pin");
    assert!(
        ElementHandle::find_by_accessible_label(&*menu.surface, "Open")
            .next()
            .is_none()
    );
    menu.hide();
    key(&launcher, Key::Menu);
    assert!(menu.is_open());
    assert_eq!(contexts.borrow()[1].1, center(&tile));
    menu.hide();
    launcher.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    key(&launcher, Key::F10);
    launcher.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    assert!(menu.is_open());
    assert_eq!(contexts.borrow().len(), 3);
    assert_eq!(contexts.borrow()[2].1, center(&tile));
    assert_eq!(launches.get(), 0);
    assert!(requests.borrow().is_empty());
    menu.hide();
    no_effects(&host);
}

#[test]
fn cancelled_or_outside_right_release_never_opens_and_left_click_keeps_launch_route() {
    let (host, launcher, menu) = setup();
    let contexts = Rc::new(Cell::new(0));
    let captured = contexts.clone();
    launcher.on_app_menu_requested(move |_, _| captured.set(captured.get() + 1));
    let launches = Rc::new(Cell::new(0));
    let captured = launches.clone();
    launcher.on_launch_requested(move |_| captured.set(captured.get() + 1));
    let position = center(&button(&launcher, "Launch Context application"));
    let outside = slint::LogicalPosition::new(1.0, 1.0);
    launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    launcher
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Right,
        });
    launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: outside });
    launcher
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: outside,
            button: PointerEventButton::Right,
        });
    assert_eq!(contexts.get(), 0);
    launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    launcher
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Right,
        });
    launcher.window().dispatch_event(WindowEvent::PointerExited);
    launcher
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Right,
        });
    assert_eq!(contexts.get(), 0);
    assert!(!menu.is_open());
    pointer(&launcher, position, PointerEventButton::Left);
    assert_eq!(launches.get(), 1);
    assert_eq!(contexts.get(), 0);
    no_effects(&host);
}

#[test]
fn pointer_return_and_space_dispatch_exact_captured_membership_once_after_detach() {
    let (host, launcher, menu) = setup();
    let requests = Rc::new(RefCell::new(Vec::new()));
    let captured = requests.clone();
    let events = host.events.clone();
    let weak_menu = Rc::downgrade(&menu);
    launcher.on_favorite_toggle_requested(move |key, desired| {
        assert!(!weak_menu.upgrade().unwrap().is_open());
        assert_eq!(events.lock().last().unwrap(), "detach");
        captured.borrow_mut().push((key, desired));
    });
    for (favorite, input) in [
        (false, None),
        (true, Some(Key::Return)),
        (false, Some(Key::Space)),
    ] {
        show(&menu, &launcher, favorite).unwrap();
        let old = menu.scope.borrow().as_ref().unwrap().token.clone();
        let expected = if favorite {
            DockMenuAction::FavoriteRemove
        } else {
            DockMenuAction::FavoriteAdd
        };
        let wrong = if favorite {
            DockMenuAction::FavoriteAdd
        } else {
            DockMenuAction::FavoriteRemove
        };
        menu.surface.invoke_action_requested(wrong);
        menu.surface.invoke_action_requested(DockMenuAction::Unpin);
        assert!(
            menu.is_open(),
            "wrong desired state and dock commands fail closed"
        );
        match input {
            Some(value) => press(&menu, value),
            None => click_row(&menu, "Pin"),
        }
        assert!(!menu.is_open());
        menu.execute(&old, expected);
        menu.surface.invoke_action_requested(expected);
        press(&menu, Key::Return);
    }
    assert_eq!(
        &*requests.borrow(),
        &[
            ("opaque app key".into(), true),
            ("opaque app key".into(), false),
            ("opaque app key".into(), true)
        ]
    );
    no_effects(&host);
}

#[test]
fn escape_close_hide_and_reopen_retire_old_issued_actions() {
    let (host, launcher, menu) = setup();
    let requests = record_requests(&launcher);
    for native_close in [false, true] {
        show(&menu, &launcher, false).unwrap();
        let old = menu.scope.borrow().as_ref().unwrap().token.clone();
        if native_close {
            menu.surface
                .window()
                .dispatch_event(WindowEvent::CloseRequested);
        } else {
            press(&menu, Key::Escape);
        }
        assert!(!menu.is_open());
        assert!(!menu.surface.window().is_visible());
        show(&menu, &launcher, false).unwrap();
        menu.execute(&old, DockMenuAction::FavoriteAdd);
        assert!(requests.borrow().is_empty());
        assert!(menu.is_open(), "retired action cannot detach a newer menu");
        menu.hide();
    }
    show(&menu, &launcher, false).unwrap();
    let old = menu.scope.borrow().as_ref().unwrap().token.clone();
    menu.hide();
    launcher.hide().unwrap();
    launcher.show().unwrap();
    show(&menu, &launcher, true).unwrap();
    menu.execute(&old, DockMenuAction::FavoriteAdd);
    assert!(requests.borrow().is_empty());
    menu.hide();
    no_effects(&host);
}

#[test]
fn hidden_refreshing_stale_and_unattached_windows_have_no_action_authority() {
    let (host, launcher, menu) = setup();
    let requests = record_requests(&launcher);
    for (hidden, refreshing, stale) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        launcher.show().unwrap();
        launcher.set_refreshing(false);
        launcher.set_stale(false);
        show(&menu, &launcher, false).unwrap();
        if hidden {
            launcher.hide().unwrap();
        }
        launcher.set_refreshing(refreshing);
        launcher.set_stale(stale);
        menu.surface
            .invoke_action_requested(DockMenuAction::FavoriteAdd);
        assert!(!menu.is_open());
        assert!(requests.borrow().is_empty());
        assert!(show(&menu, &launcher, false).is_err());
    }
    launcher.set_refreshing(false);
    launcher.set_stale(false);
    launcher.show().unwrap();
    show(&menu, &launcher, false).unwrap();
    menu.surface.window().hide().unwrap();
    menu.surface
        .invoke_action_requested(DockMenuAction::FavoriteAdd);
    assert!(
        requests.borrow().is_empty(),
        "actual window visibility is required too"
    );
    menu.hide();
    no_effects(&host);
}

#[test]
fn attach_denial_cancellation_and_focus_denial_preserve_truthful_lifecycle() {
    let (host, launcher, menu) = setup();
    let requests = record_requests(&launcher);
    host.deny_attach.store(true, Ordering::Relaxed);
    assert!(
        show(&menu, &launcher, false)
            .unwrap_err()
            .contains("Attachment denied")
    );
    assert!(!menu.is_open());
    assert!(!menu.surface.window().is_visible());
    menu.surface
        .invoke_action_requested(DockMenuAction::FavoriteAdd);
    assert!(requests.borrow().is_empty());
    assert_eq!(&*host.events.lock(), &["attach-denied"]);
    host.deny_attach.store(false, Ordering::Relaxed);
    host.close_during_attach.store(true, Ordering::Relaxed);
    show(&menu, &launcher, false).unwrap();
    assert!(!menu.is_open());
    assert!(!menu.surface.window().is_visible());
    assert_eq!(&*host.events.lock(), &["attach-denied", "attach", "detach"]);
    host.close_during_attach.store(false, Ordering::Relaxed);
    host.deny_focus.store(true, Ordering::Relaxed);
    assert!(
        show(&menu, &launcher, false)
            .unwrap_err()
            .contains("keyboard focus")
    );
    assert!(
        menu.is_open(),
        "denied foreground does not hide a usable pointer menu"
    );
    click_row(&menu, "Pin");
    assert_eq!(&*requests.borrow(), &[("opaque app key".into(), true)]);
    assert!(!menu.is_open());
    no_effects(&host);
}

#[test]
fn detach_reentry_replacement_or_source_retirement_suppresses_old_request() {
    let (host, launcher, menu) = setup();
    let requests = record_requests(&launcher);
    for replacement in [false, true] {
        show(&menu, &launcher, false).unwrap();
        let weak_menu = Rc::downgrade(&menu);
        let weak_launcher = launcher.as_weak();
        DETACH.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let menu = weak_menu.upgrade().unwrap();
                let launcher = weak_launcher.upgrade().unwrap();
                if replacement {
                    show(&menu, &launcher, true).unwrap();
                } else {
                    menu.hide();
                    launcher.hide().unwrap();
                    launcher.show().unwrap();
                }
            }))
        });
        press(&menu, Key::Return);
        assert!(requests.borrow().is_empty());
        assert_eq!(menu.is_open(), replacement);
        if replacement {
            assert!(menu.surface.get_launcher_favorite());
        }
        menu.hide();
    }
    no_effects(&host);
}

#[test]
fn show_detach_reentry_cannot_overwrite_newer_key_or_desired_membership() {
    let (host, launcher, menu) = setup();
    let requests = record_requests(&launcher);
    show(&menu, &launcher, false).unwrap();
    let weak_menu = Rc::downgrade(&menu);
    let weak_launcher = launcher.as_weak();
    DETACH.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            weak_menu
                .upgrade()
                .unwrap()
                .show(
                    &weak_launcher.upgrade().unwrap(),
                    "replacement app".into(),
                    true,
                    slint::LogicalPosition::new(300.0, 250.0),
                    DockContext::new(0, 0, 1920, 1080, false).unwrap(),
                )
                .unwrap();
        }))
    });
    menu.show(
        &launcher,
        "obsolete outer app".into(),
        false,
        slint::LogicalPosition::new(600.0, 250.0),
        DockContext::new(0, 0, 1920, 1080, false).unwrap(),
    )
    .unwrap();
    assert!(menu.is_open());
    assert_eq!(menu.scope.borrow().as_ref().unwrap().key, "replacement app");
    press(&menu, Key::Space);
    assert_eq!(&*requests.borrow(), &[("replacement app".into(), false)]);
    no_effects(&host);
}

#[test]
fn detach_does_not_retain_a_dropped_launcher_or_callback_owner() {
    let (host, launcher, menu) = setup();
    let requests = record_requests(&launcher);
    let launcher_owner = Rc::new(RefCell::new(Some(launcher)));
    show(&menu, launcher_owner.borrow().as_ref().unwrap(), false).unwrap();
    let weak_launcher = launcher_owner.borrow().as_ref().unwrap().as_weak();
    let owner = launcher_owner.clone();
    DETACH.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            // A shown SDK Window retains its component until owner shutdown
            // hides it; dropping only the Rust handle is not window teardown.
            let launcher = owner.borrow_mut().take().unwrap();
            launcher.hide().unwrap();
        }))
    });
    press(&menu, Key::Return);
    assert!(weak_launcher.upgrade().is_none());
    assert!(requests.borrow().is_empty());
    assert!(!menu.is_open());
    let weak_menu = Rc::downgrade(&menu);
    drop(menu);
    assert!(weak_menu.upgrade().is_none());
    no_effects(&host);
}

#[test]
fn invalid_nonfinite_anchors_retire_old_scope_without_native_attachment() {
    let (host, launcher, menu) = setup();
    let requests = record_requests(&launcher);
    for anchor in [
        slint::LogicalPosition::new(f32::NAN, 10.0),
        slint::LogicalPosition::new(10.0, f32::INFINITY),
    ] {
        show(&menu, &launcher, false).unwrap();
        let old = menu.scope.borrow().as_ref().unwrap().token.clone();
        let events = host.events.lock().len();
        assert!(
            menu.show(
                &launcher,
                "opaque app key".into(),
                false,
                anchor,
                DockContext::new(0, 0, 1920, 1080, false).unwrap()
            )
            .is_err()
        );
        assert!(!menu.is_open());
        assert_eq!(
            host.events.lock().len(),
            events + 1,
            "only the old lease detaches"
        );
        menu.execute(&old, DockMenuAction::FavoriteAdd);
        assert!(requests.borrow().is_empty());
    }
    no_effects(&host);
}

#[test]
fn dock_adapter_rejects_launcher_only_intents_without_detach_or_dock_pin() {
    use crate::context_menu::ContextMenuController;
    use crate::generated::{Dock, DockMenuKind};
    let (host, launcher, _) = setup();
    let dock = Dock::new().unwrap();
    dock.show().unwrap();
    let commands = Rc::new(Cell::new(0));
    let captured = commands.clone();
    dock.on_pin_toggle_requested(move |_, _| captured.set(captured.get() + 1));
    let dock_menu = ContextMenuController::new(host.clone(), &dock).unwrap();
    for kind in [
        DockMenuKind::Bar,
        DockMenuKind::Pinned,
        DockMenuKind::Window,
        DockMenuKind::Recycle,
    ] {
        dock_menu
            .show(
                kind,
                "dock key".into(),
                slint::PhysicalPosition::new(200, 800),
                DockContext::new(0, 0, 1920, 1080, false).unwrap(),
            )
            .unwrap();
        let events = host.events.lock().len();
        dock_menu
            .component()
            .invoke_action_requested(DockMenuAction::FavoriteAdd);
        dock_menu
            .component()
            .invoke_action_requested(DockMenuAction::FavoriteRemove);
        assert!(dock_menu.is_open());
        assert_eq!(host.events.lock().len(), events);
        assert_eq!(commands.get(), 0);
        dock_menu.hide();
    }
    launcher.hide().unwrap();
    no_effects(&host);
}

#[test]
fn opening_a_tail_application_does_not_materialize_source_rows_or_extract_icons() {
    use crate::launcher::{LauncherInventory, LauncherRows};
    use crate::projection::AppProjection;
    let (host, launcher, menu) = setup();
    let requests = record_requests(&launcher);
    let inventory = Rc::new(LauncherInventory::new(
        (0..1024)
            .map(|index| AppProjection {
                key: format!("opaque-{index:04}"),
                label: format!("Application {index:04}"),
                pinned: false,
                icon: None,
            })
            .collect(),
    ));
    let image_demands = Rc::new(Cell::new(0));
    let captured = image_demands.clone();
    launcher.set_rows(ModelRc::new(LauncherRows::new(
        inventory.clone(),
        vec![],
        7,
        move |_| {
            captured.set(captured.get() + 1);
            Default::default()
        },
    )));
    launcher.set_application_count(1024);
    let key = inventory.application("opaque-1023").unwrap().key.clone();
    let before = image_demands.get();
    menu.show(
        &launcher,
        key.into(),
        false,
        slint::LogicalPosition::new(150.0, 200.0),
        DockContext::new(0, 0, 1920, 1080, false).unwrap(),
    )
    .unwrap();
    assert_eq!(
        image_demands.get(),
        before,
        "opening never demands launcher rows/images"
    );
    assert!(requests.borrow().is_empty());
    press(&menu, Key::Return);
    assert_eq!(&*requests.borrow(), &[("opaque-1023".into(), true)]);
    assert_eq!(
        image_demands.get(),
        before,
        "the adapter does not optimistically reproject inventory"
    );
    no_effects(&host);
}

#[test]
fn optional_media_bar_add_and_module_remove_are_absolute_typed_detached_intents() {
    use crate::context_menu::ContextMenuController;
    use crate::generated::{Dock, DockMenuKind};
    let (host, _launcher, _) = setup();
    let dock = Dock::new().unwrap();
    dock.show().unwrap();
    let menu = ContextMenuController::new(host.clone(), &dock).unwrap();
    let requests = Rc::new(RefCell::new(Vec::new()));
    let recorded = requests.clone();
    let events = host.events.clone();
    let weak_menu = Rc::downgrade(&menu);
    dock.on_media_enabled_requested(move |desired| {
        assert!(!weak_menu.upgrade().unwrap().is_open());
        assert_eq!(events.lock().last().unwrap(), "detach");
        recorded.borrow_mut().push(desired);
    });
    let context = DockContext::new(0, 0, 1920, 1080, false).unwrap();
    assert!(
        !dock.get_media_view().enabled,
        "optional membership defaults disabled"
    );
    menu.show(
        DockMenuKind::Bar,
        "".into(),
        slint::PhysicalPosition::new(100, 800),
        context,
    )
    .unwrap();
    button(menu.component(), "Add media module");
    assert!(
        ElementHandle::find_by_accessible_label(menu.component(), "Remove media module")
            .next()
            .is_none()
    );
    assert!(requests.borrow().is_empty());
    menu.component()
        .invoke_action_requested(DockMenuAction::MediaRemove);
    assert!(
        menu.is_open(),
        "wrong desired membership cannot remove or detach"
    );
    pointer(
        menu.component(),
        center(&button(menu.component(), "Add media module")),
        PointerEventButton::Left,
    );
    menu.component()
        .invoke_action_requested(DockMenuAction::MediaBarAdd);
    assert_eq!(&*requests.borrow(), &[true]);
    let mut view = dock.get_media_view();
    view.enabled = true;
    dock.set_media_view(view);
    menu.show_media(&dock, slint::LogicalPosition::new(100.0, 20.0), context)
        .unwrap();
    button(menu.component(), "Remove media module");
    assert!(
        ElementHandle::find_by_accessible_label(menu.component(), "Settings")
            .next()
            .is_none()
    );
    menu.component()
        .invoke_action_requested(DockMenuAction::MediaBarAdd);
    menu.component()
        .invoke_action_requested(DockMenuAction::Settings);
    assert!(
        menu.is_open(),
        "sole module scope rejects bar/application effects"
    );
    key(menu.component(), Key::Space);
    menu.component()
        .invoke_action_requested(DockMenuAction::MediaRemove);
    assert_eq!(&*requests.borrow(), &[true, false]);
    assert!(
        dock.get_media_view().enabled,
        "adapter never optimistically mutates module membership"
    );
    no_effects(&host);
}

#[test]
fn media_context_rejects_foreign_hidden_stale_membership_and_wrong_dock_scopes() {
    use crate::context_menu::ContextMenuController;
    use crate::generated::{Dock, DockMenuKind};
    let (host, _launcher, _) = setup();
    let dock = Dock::new().unwrap();
    dock.show().unwrap();
    let mut view = dock.get_media_view();
    view.enabled = true;
    dock.set_media_view(view);
    let menu = ContextMenuController::new(host.clone(), &dock).unwrap();
    let requests = Rc::new(Cell::new(0));
    let recorded = requests.clone();
    dock.on_media_enabled_requested(move |_| recorded.set(recorded.get() + 1));
    let context = DockContext::new(0, 0, 1920, 1080, false).unwrap();
    let foreign = Dock::new().unwrap();
    foreign.show().unwrap();
    let mut foreign_view = foreign.get_media_view();
    foreign_view.enabled = true;
    foreign.set_media_view(foreign_view);
    assert!(
        menu.show_media(&foreign, slint::LogicalPosition::new(10.0, 10.0), context)
            .is_err()
    );
    menu.show_media(&dock, slint::LogicalPosition::new(10.0, 10.0), context)
        .unwrap();
    let mut view = dock.get_media_view();
    view.enabled = false;
    dock.set_media_view(view);
    menu.component()
        .invoke_action_requested(DockMenuAction::MediaRemove);
    assert_eq!(
        requests.get(),
        0,
        "stale Remove cannot invert newly applied disabled state"
    );
    menu.hide();
    assert!(
        menu.show_media(&dock, slint::LogicalPosition::new(10.0, 10.0), context)
            .is_err()
    );
    let mut view = dock.get_media_view();
    view.enabled = true;
    dock.set_media_view(view);
    menu.show_media(&dock, slint::LogicalPosition::new(10.0, 10.0), context)
        .unwrap();
    dock.hide().unwrap();
    key(menu.component(), Key::Return);
    assert_eq!(requests.get(), 0);
    menu.hide();
    dock.show().unwrap();
    for kind in [
        DockMenuKind::Pinned,
        DockMenuKind::Window,
        DockMenuKind::Recycle,
    ] {
        menu.show(
            kind,
            "opaque app key".into(),
            slint::PhysicalPosition::new(100, 800),
            context,
        )
        .unwrap();
        let events = host.events.lock().len();
        menu.component()
            .invoke_action_requested(DockMenuAction::MediaBarAdd);
        menu.component()
            .invoke_action_requested(DockMenuAction::MediaRemove);
        assert_eq!(host.events.lock().len(), events);
        assert_eq!(requests.get(), 0);
        menu.hide();
    }
    no_effects(&host);
}

#[test]
fn media_detach_reentry_cannot_dispatch_into_reopened_module_or_replacement_popup() {
    use crate::context_menu::ContextMenuController;
    use crate::generated::{Dock, DockMenuKind};
    let (host, _launcher, _) = setup();
    let dock = Dock::new().unwrap();
    dock.show().unwrap();
    let mut view = dock.get_media_view();
    view.enabled = true;
    dock.set_media_view(view);
    let menu = ContextMenuController::new(host.clone(), &dock).unwrap();
    let requests = Rc::new(Cell::new(0));
    let recorded = requests.clone();
    dock.on_media_enabled_requested(move |_| recorded.set(recorded.get() + 1));
    let context = DockContext::new(0, 0, 1920, 1080, false).unwrap();
    for replacement in [false, true] {
        menu.show_media(&dock, slint::LogicalPosition::new(100.0, 20.0), context)
            .unwrap();
        let weak_menu = Rc::downgrade(&menu);
        let weak_dock = dock.as_weak();
        DETACH.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let menu = weak_menu.upgrade().unwrap();
                if replacement {
                    menu.show(
                        DockMenuKind::Pinned,
                        "replacement key".into(),
                        slint::PhysicalPosition::new(200, 800),
                        context,
                    )
                    .unwrap();
                } else {
                    menu.hide();
                    let dock = weak_dock.upgrade().unwrap();
                    dock.hide().unwrap();
                    dock.show().unwrap();
                }
            }))
        });
        key(menu.component(), Key::Return);
        assert_eq!(requests.get(), 0);
        assert_eq!(menu.is_open(), replacement);
        if replacement {
            assert_eq!(menu.component().get_kind(), DockMenuKind::Pinned);
        }
        menu.hide();
    }
    no_effects(&host);
}

#[derive(Default)]
struct LauncherIntentRequests {
    count: Cell<usize>,
    last: Cell<Option<&'static str>>,
}

impl LauncherIntentRequests {
    fn get(&self) -> usize {
        self.count.get()
    }

    fn record(&self, label: &'static str) {
        self.count.set(self.count.get() + 1);
        self.last.set(Some(label));
    }
}

fn record_launcher_intents(launcher: &Launcher) -> Rc<LauncherIntentRequests> {
    let requests = Rc::new(LauncherIntentRequests::default());
    let recorded = requests.clone();
    launcher.on_launch_requested(move |_| recorded.record("Launch Context application"));
    let recorded = requests.clone();
    launcher.on_favorite_toggle_requested(move |_, _| {
        recorded.record("Add to favorites: Context application")
    });
    let recorded = requests.clone();
    launcher.on_view_requested(move |_| recorded.record("Back to favorites"));
    let recorded = requests.clone();
    launcher.on_display_mode_requested(move |_| recorded.record("Expand applications menu"));
    let recorded = requests.clone();
    launcher.on_open_user_menu_requested(move |_| recorded.record("Open user menu"));
    let recorded = requests.clone();
    launcher.on_open_settings_requested(move || recorded.record("Open settings and recovery"));
    let recorded = requests.clone();
    launcher.on_open_power_menu_requested(move || recorded.record("Open power menu"));
    let recorded = requests.clone();
    launcher.on_refresh_requested(move || recorded.record("Refresh the desktop"));
    let recorded = requests.clone();
    launcher.on_exit_requested(move || recorded.record("Exit Tessera"));
    requests
}

const LAUNCHER_COMMAND_LABELS: &[&str] = &[
    "Back to favorites",
    "Refresh the desktop",
    "Exit Tessera",
    "Open user menu",
    "Open settings and recovery",
    "Open power menu",
    "Expand applications menu",
];

fn focus_launcher_command(launcher: &Launcher, label: &str) {
    // Fixture has one application and its favorite corner between header and footer.
    // Use native Tab traversal, not programmatic private widget focus access.
    let index = LAUNCHER_COMMAND_LABELS
        .iter()
        .position(|candidate| *candidate == label)
        .unwrap();
    let tabs = if index == 0 { 1 } else { index + 3 };
    launcher.invoke_focus_search();
    for _ in 0..tabs {
        key(launcher, Key::Tab);
    }
}

#[test]
fn window_cancel_revokes_retained_same_key_pointer_and_context_gestures() {
    let (host, launcher, _) = setup();
    let requests = record_launcher_intents(&launcher);
    let contexts = Rc::new(Cell::new(0));
    let recorded = contexts.clone();
    launcher.on_app_menu_requested(move |_, _| recorded.set(recorded.get() + 1));
    for mouse_button in [PointerEventButton::Left, PointerEventButton::Right] {
        for reopen in [false, true] {
            let position = center(&button(&launcher, "Launch Context application"));
            launcher
                .window()
                .dispatch_event(WindowEvent::PointerMoved { position });
            launcher
                .window()
                .dispatch_event(WindowEvent::PointerPressed {
                    position,
                    button: mouse_button,
                });
            let before = (requests.get(), contexts.get());
            launcher.invoke_cancel_input();
            if reopen {
                launcher.hide().unwrap();
                launcher.show().unwrap();
                launcher
                    .window()
                    .dispatch_event(WindowEvent::WindowActiveChanged(true));
            } else {
                // No delegate replacement: the same key/row survives the query.
                launcher.set_search("Context".into());
                launcher.set_rows(launcher.get_rows());
            }
            launcher
                .window()
                .dispatch_event(WindowEvent::PointerReleased {
                    position,
                    button: mouse_button,
                });
            assert_eq!((requests.get(), contexts.get()), before);
            let current = center(&button(&launcher, "Launch Context application"));
            pointer(&launcher, current, mouse_button);
            assert_eq!(
                (requests.get(), contexts.get()),
                if mouse_button == PointerEventButton::Left {
                    (before.0 + 1, before.1)
                } else {
                    (before.0, before.1 + 1)
                },
            );
        }
    }
    assert!(launcher.get_input_available());
    no_effects(&host);
}

#[test]
fn window_cancel_revokes_held_space_on_body_favorite_header_and_every_footer_without_refocus() {
    let (host, launcher, _) = setup();
    launcher.set_feedback_visible(true);
    let requests = record_launcher_intents(&launcher);
    let labels = [
        "Launch Context application",
        "Add to favorites: Context application",
    ]
    .into_iter()
    .chain(LAUNCHER_COMMAND_LABELS.iter().copied());
    for label in labels {
        let mut matches = ElementHandle::find_by_accessible_label(&launcher, label);
        let element = matches.next().expect("real launcher input node");
        assert!(matches.next().is_none(), "{label} must be unique");
        if LAUNCHER_COMMAND_LABELS.contains(&label) {
            focus_launcher_command(&launcher, label);
        } else {
            // Application/corner retain their native pointer-focus behavior.
            pointer(&launcher, center(&element), PointerEventButton::Right);
        }
        let before = requests.get();
        let text: SharedString = Key::Space.into();
        launcher
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        assert_eq!(requests.get(), before, "{label} waits for Space release");
        launcher.invoke_cancel_input();
        launcher
            .window()
            .dispatch_event(WindowEvent::KeyPressRepeated { text: text.clone() });
        launcher
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
        assert_eq!(
            requests.get(),
            before,
            "{label} cannot revive a cancelled Space"
        );
        // No new focus event: cancellation must not steal native keyboard focus.
        key(&launcher, Key::Space);
        assert_eq!(
            requests.get(),
            before + 1,
            "{label} accepts a fresh focused Space"
        );
        assert_eq!(
            requests.last.get(),
            Some(label),
            "native focus must stay on {label}"
        );
        assert_eq!(launcher.get_search(), "");
    }
    no_effects(&host);
}

#[test]
fn window_cancel_revokes_held_return_repeats_but_keeps_fresh_stable_scope_app_repeats() {
    let (host, launcher, _) = setup();
    let requests = record_launcher_intents(&launcher);
    pointer(
        &launcher,
        center(&button(&launcher, "Launch Context application")),
        PointerEventButton::Right,
    );
    let text: SharedString = Key::Return.into();
    launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated { text: text.clone() });
    assert_eq!(requests.get(), 2);
    launcher.invoke_cancel_input();
    launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated { text: text.clone() });
    launcher
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text: text.clone() });
    assert_eq!(requests.get(), 2);
    launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated { text: text.clone() });
    launcher
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text });
    assert_eq!(
        requests.get(),
        4,
        "fresh same-key authority keeps the original repeat policy"
    );
    no_effects(&host);
}

#[test]
fn window_cancel_revokes_every_static_command_pointer_and_preserves_return_policy() {
    let (host, launcher, _) = setup();
    launcher.set_feedback_visible(true);
    let requests = record_launcher_intents(&launcher);
    for label in LAUNCHER_COMMAND_LABELS {
        let position = center(&button(&launcher, label));
        launcher
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        launcher
            .window()
            .dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
        let before = requests.get();
        launcher.invoke_cancel_input();
        launcher
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
        assert_eq!(requests.get(), before, "{label} cancels its held pointer");
        pointer(&launcher, position, PointerEventButton::Left);
        assert_eq!(requests.get(), before + 1);
        assert_eq!(requests.last.get(), Some(*label));
        focus_launcher_command(&launcher, label);
        let text: SharedString = Key::Return.into();
        launcher
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        assert_eq!(requests.get(), before + 2, "{label} permits a fresh Return");
        launcher
            .window()
            .dispatch_event(WindowEvent::KeyPressRepeated { text: text.clone() });
        let after_repeat = before + if *label == "Back to favorites" { 3 } else { 2 };
        assert_eq!(
            requests.get(),
            after_repeat,
            "{label} keeps its stable-scope repeat policy"
        );
        launcher.invoke_cancel_input();
        launcher
            .window()
            .dispatch_event(WindowEvent::KeyPressRepeated { text: text.clone() });
        launcher
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
        assert_eq!(
            requests.get(),
            after_repeat,
            "{label} cannot revive a cancelled Return"
        );
    }
    no_effects(&host);
}
