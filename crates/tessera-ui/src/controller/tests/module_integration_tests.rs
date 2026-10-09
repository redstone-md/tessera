// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use tessera_system::calendar::{
    CalendarError, CalendarHost, CalendarReadCompletion, CalendarSnapshot, CivilDate, WeekStart,
};
use tessera_system::visibility::{
    PointerHost, PointerWatchCallback, PointerWatchCompletion, PointerWatchError, PointerWatchGuard,
};

#[test]
fn genuine_toolbar_network_bluetooth_keyboard_are_lazy_independent_and_exclusive() {
    let fixture = LauncherFixture::new();
    fixture.host.shell_identity.lock().language = "pt-BR".into();
    fixture.controller.core.refresh_identity();
    fixture.controller.render();
    assert_eq!(fixture.toolbar.get_language().as_str(), "pt-BR");
    assert_eq!(
        fixture.host.network_provider_calls.load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        fixture.host.bluetooth_provider_calls.load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        fixture
            .host
            .input_language_provider_calls
            .load(Ordering::SeqCst),
        0
    );
    let activity = fixture.host.unrelated_activity();
    click_component(&fixture.toolbar, "Open network");
    let network = fixture.controller.network_menu.borrow().clone().unwrap();
    assert!(network.is_open());
    assert_eq!(
        fixture.host.network_provider_calls.load(Ordering::SeqCst),
        1
    );
    click_component(&fixture.toolbar, "Open Bluetooth");
    let bluetooth = fixture.controller.bluetooth.borrow().clone().unwrap();
    assert!(bluetooth.is_open());
    assert!(!network.is_open());
    fixture
        .toolbar
        .invoke_network_requested(crate::generated::TileBounds {
            origin: slint::LogicalPosition::new(f32::MAX, 0.0),
            width: 16.0,
            height: 16.0,
        });
    assert!(
        bluetooth.is_open(),
        "rejected placement must retain the real current popup"
    );
    click_component(&fixture.toolbar, "Open keyboard selector");
    let input = fixture.controller.input_language.borrow().clone().unwrap();
    assert!(input.is_open());
    assert!(!bluetooth.is_open());
    assert_eq!(
        fixture.toolbar.get_language().as_str(),
        "pt-BR",
        "selector errors do not invent a language"
    );
    assert_eq!(
        fixture
            .host
            .input_language_provider_calls
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(fixture.host.unrelated_activity(), activity);
    fixture.toolbar.hide().unwrap();
    fixture
        .toolbar
        .invoke_bluetooth_requested(crate::generated::TileBounds {
            origin: slint::LogicalPosition::new(1.0, 1.0),
            width: 16.0,
            height: 16.0,
        });
    assert_eq!(
        fixture.host.bluetooth_provider_calls.load(Ordering::SeqCst),
        1
    );
    let scope = native_toolbar::ToolbarPopupScope::new(&fixture.controller);
    drop(scope);
    assert!(!input.is_open());
    assert!(fixture.controller.network_menu.borrow().is_none());
    assert!(fixture.controller.bluetooth.borrow().is_none());
    assert!(fixture.controller.input_language.borrow().is_none());
    let terminal = power_menu::PowerAdmissionScope::new(&fixture.controller);
    drop(terminal);
    fixture.panel.set_start_of_week_index(2);
    fixture.panel.invoke_save_preferences_requested();
    assert!(
        fixture.host.saves.lock().is_empty(),
        "closed root rejects queued save input"
    );
}

struct DateFixture {
    reads: AtomicUsize,
}
impl CalendarHost for DateFixture {
    fn read(&self, completion: CalendarReadCompletion) -> Result<(), CalendarError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        completion(CalendarSnapshot::new(
            CivilDate::new(2024, 2, 29).unwrap(),
            "fr-FR".into(),
            [
                "janvier",
                "février",
                "mars",
                "avril",
                "mai",
                "juin",
                "juillet",
                "août",
                "septembre",
                "octobre",
                "novembre",
                "décembre",
            ]
            .map(String::from),
            [
                "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
            ]
            .map(String::from),
            ["lun", "mar", "mer", "jeu", "ven", "sam", "dim"].map(String::from),
            WeekStart::Sunday,
        ));
        Ok(())
    }
}

#[test]
fn general_draft_is_applied_only_after_one_successful_save_and_preserves_calendar_browsing() {
    let fixture = LauncherFixture::new();
    let date = Arc::new(DateFixture {
        reads: AtomicUsize::new(0),
    });
    *fixture.host.calendar_provider.lock() = Some(date.clone());
    fixture.toolbar.set_clock("Observed clock".into());
    click_component(&fixture.toolbar, "Open calendar");
    let calendar = fixture.controller.calendar.borrow().clone().unwrap();
    calendar.component().invoke_calendar_event_ready();
    assert_eq!(
        calendar
            .component()
            .get_weekdays()
            .row_data(0)
            .unwrap()
            .as_str(),
        "lun"
    );
    click_component(calendar.component(), "Next month");
    let browsed = calendar.component().get_title_text();
    fixture.panel.set_start_of_week_index(2);
    assert_eq!(
        calendar
            .component()
            .get_weekdays()
            .row_data(0)
            .unwrap()
            .as_str(),
        "lun"
    );
    assert!(fixture.host.saves.lock().is_empty());
    fixture.controller.toggle_pin("app-editor", true);
    assert_eq!(
        fixture.host.saves.lock()[0].general().start_of_week(),
        crate::StartOfWeek::Monday
    );
    let reads = date.reads.load(Ordering::SeqCst);
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    fixture.panel.invoke_save_preferences_requested();
    assert_eq!(fixture.host.saves.lock().len(), 2);
    assert_eq!(
        fixture
            .controller
            .core
            .applied_preferences()
            .general()
            .start_of_week(),
        crate::StartOfWeek::Saturday
    );
    assert_eq!(
        calendar
            .component()
            .get_weekdays()
            .row_data(0)
            .unwrap()
            .as_str(),
        "sam"
    );
    assert_eq!(calendar.component().get_title_text(), browsed);
    assert_eq!(date.reads.load(Ordering::SeqCst), reads);
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), focus);
    let applied = fixture.controller.core.applied_preferences();
    fixture.panel.set_start_of_week_index(1);
    *fixture.host.save_result.lock() = Err("recording storage failure".into());
    fixture.panel.invoke_save_preferences_requested();
    assert_eq!(fixture.controller.core.applied_preferences(), applied);
    assert_eq!(
        calendar
            .component()
            .get_weekdays()
            .row_data(0)
            .unwrap()
            .as_str(),
        "sam"
    );
    assert_eq!(calendar.component().get_title_text(), browsed);
    let saves = fixture.host.saves.lock().len();
    fixture.panel.set_start_of_week_index(7);
    fixture.panel.invoke_save_preferences_requested();
    assert_eq!(
        fixture.host.saves.lock().len(),
        saves,
        "invalid draft cannot write"
    );
}

#[test]
fn every_existing_preference_builder_preserves_applied_general_and_media() {
    let general =
        crate::GeneralPreferences::default().with_start_of_week(crate::StartOfWeek::Sunday);
    let applied = PanelPreferences::new(Theme::Dark, true)
        .with_general(general)
        .with_media_enabled(true)
        .with_dock(crate::DockEdge::Left, vec!["pin".into()])
        .with_launcher_favorites(vec!["favorite".into()])
        .unwrap();
    for changed in [
        applied
            .clone()
            .with_appearance(Theme::Light, false, crate::DockEdge::Right),
        applied
            .clone()
            .with_dock(crate::DockEdge::Top, vec!["other".into()]),
        applied
            .clone()
            .with_launcher_favorites(vec!["new".into()])
            .unwrap(),
        applied
            .clone()
            .with_launcher_display_mode(crate::LauncherDisplayMode::Fullscreen),
    ] {
        assert_eq!(changed.general(), general);
        assert!(changed.media_enabled());
    }
    assert_eq!(
        PanelPreferences::default().general().start_of_week(),
        crate::StartOfWeek::Monday
    );
    assert!(!PanelPreferences::default().media_enabled());
}

#[test]
fn composed_media_is_one_real_large_tile_and_exactly_three_virtual_slots_on_all_edges() {
    use i_slint_backend_testing::ElementHandle;
    let fixture = LauncherFixture::with_preferences(seeded_preferences().with_media_enabled(true));
    assert!(fixture.dock.get_media_view().enabled);
    assert!(!fixture.dock.get_media_view().current_present);
    assert!(fixture.dock.get_media_view().title.is_empty());
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 1);
    let context = fixture.controller.core.dock_context().unwrap();
    for edge in [
        crate::DockEdge::Bottom,
        crate::DockEdge::Top,
        crate::DockEdge::Left,
        crate::DockEdge::Right,
    ] {
        for compact in [false, true] {
            fixture
                .panel
                .set_dock_edge_index(crate::dock_edge_to_index(edge));
            fixture.panel.set_compact(compact);
            fixture.controller.sync_appearance();
            let expected = crate::dock::dock_rect(
                context,
                edge,
                3,
                compact,
                fixture.dock.window().scale_factor(),
            );
            let attached = fixture
                .controller
                .leases
                .borrow()
                .attachments
                .get(&SurfaceKind::Dock)
                .unwrap()
                .rect;
            assert_eq!(
                attached,
                (expected.x, expected.y, expected.width, expected.height)
            );
            let tiles: Vec<_> =
                ElementHandle::find_by_accessible_label(&fixture.dock, "Media module").collect();
            assert_eq!(tiles.len(), 1);
            let tile = tiles[0].absolute_position();
            assert!(tile.x.is_finite() && tile.y.is_finite());
            let factor = if compact { 0.8 } else { 1.0 };
            let size = tiles[0].size();
            let (width, height) = if matches!(edge, crate::DockEdge::Left | crate::DockEdge::Right)
            {
                (40.0 * factor, 136.0 * factor)
            } else {
                (136.0 * factor, 40.0 * factor)
            };
            assert!((size.width - width).abs() < 0.01);
            assert!((size.height - height).abs() < 0.01);
        }
    }
    assert!(fixture.host.saves.lock().is_empty());
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
}

struct PointerFixture {
    starts: AtomicUsize,
    drops: Arc<AtomicUsize>,
    events: Mutex<Option<PointerWatchCallback>>,
    ready: Arc<Mutex<Option<PointerWatchCompletion>>>,
}
struct PointerGuard {
    drops: Arc<AtomicUsize>,
    ready: Arc<Mutex<Option<PointerWatchCompletion>>>,
}
impl PointerWatchGuard for PointerGuard {}
impl Drop for PointerGuard {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        let ready = self.ready.lock().take();
        if let Some(ready) = ready {
            ready(Err(PointerWatchError::Stopped));
        }
    }
}
impl PointerHost for PointerFixture {
    fn watch(
        &self,
        events: PointerWatchCallback,
        ready: PointerWatchCompletion,
    ) -> Result<Box<dyn PointerWatchGuard>, PointerWatchError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        *self.events.lock() = Some(events);
        *self.ready.lock() = Some(ready);
        Ok(Box::new(PointerGuard {
            drops: self.drops.clone(),
            ready: self.ready.clone(),
        }))
    }
}

#[test]
fn visibility_retains_one_pointer_watch_across_fullscreen_and_missing_geometry_without_focus() {
    let pointer = Arc::new(PointerFixture {
        starts: AtomicUsize::new(0),
        drops: Arc::default(),
        events: Mutex::default(),
        ready: Arc::default(),
    });
    let fixture = LauncherFixture::with_snapshot_configured(
        seeded_preferences(),
        launcher_snapshot(),
        |fixture| {
            *fixture.host.pointer_provider.lock() = Some(pointer.clone());
        },
    );
    assert_eq!(pointer.starts.load(Ordering::SeqCst), 1);
    let ready = pointer
        .ready
        .lock()
        .take()
        .expect("one admitted pointer readiness");
    ready(Ok(()));
    fixture.panel.invoke_visibility_event_ready();
    assert_eq!(
        fixture
            .controller
            .visibility
            .borrow()
            .as_ref()
            .unwrap()
            .readiness(),
        crate::visibility::PointerReadiness::Ready
    );
    click_component(&fixture.toolbar, "Open network");
    let popup = fixture.controller.network_menu.borrow().clone().unwrap();
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let context = fixture.controller.core.dock_context().unwrap();
    let fullscreen = crate::DockContext::new(
        context.x(),
        context.y(),
        context.width(),
        context.height(),
        true,
    )
    .unwrap();
    apply_result_to_both(
        &fixture.controller,
        &fixture.panel,
        Ok(launcher_snapshot().with_dock_context(fullscreen)),
    );
    assert!(!fixture.dock.window().is_visible());
    assert!(!fixture.toolbar.window().is_visible());
    assert!(!popup.is_open());
    assert_eq!(pointer.drops.load(Ordering::SeqCst), 0);
    let activity = fixture.host.unrelated_activity();
    fixture.dock.invoke_launch_requested("app-editor".into());
    fixture
        .dock
        .invoke_pin_toggle_requested("app-editor".into(), true);
    fixture
        .dock
        .invoke_system_command_requested(crate::generated::DockSystemCommand::TaskManager);
    fixture
        .toolbar
        .invoke_bluetooth_requested(crate::generated::TileBounds {
            origin: slint::LogicalPosition::new(1.0, 1.0),
            width: 16.0,
            height: 16.0,
        });
    assert_eq!(fixture.host.unrelated_activity(), activity);
    assert_eq!(
        fixture.host.bluetooth_provider_calls.load(Ordering::SeqCst),
        0
    );
    let without_geometry =
        PanelSnapshot::new(2, Vec::new(), 0).with_applications(fixture.controller.core.catalog());
    apply_result_to_both(&fixture.controller, &fixture.panel, Ok(without_geometry));
    assert!(
        !fixture.controller.apply_geometry(None).unwrap(),
        "retained frame is not fresh readiness"
    );
    assert!(fixture.dock.window().is_visible());
    assert!(fixture.toolbar.window().is_visible());
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), focus);
    assert_eq!(pointer.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.host.pointer_provider_calls.load(Ordering::SeqCst),
        1
    );
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
    let scope = crate::transient_window::TransientScope::new(
        Rc::clone(&fixture.controller.visibility),
        crate::visibility::VisibilityController::close,
    );
    drop(scope);
    assert_eq!(pointer.drops.load(Ordering::SeqCst), 1);
    assert!(fixture.controller.visibility.borrow().is_none());
}

#[test]
fn complete_overlap_facts_survive_the_independent_128_row_ui_cap() {
    use tessera_core::Rect;
    use tessera_system::visibility::{VisibilityFacts, VisibilityWindow, any_window_overlaps};
    let monitor = Rect::new(0, 0, 1920, 1080).unwrap();
    let hitbox = Rect::new(0, 1000, 1920, 80).unwrap();
    let mut facts = vec![
        VisibilityWindow {
            bounds: Rect::new(0, 0, 50, 50).unwrap(),
            belongs_to_monitor: true,
            minimized: false,
        };
        140
    ];
    facts.push(VisibilityWindow {
        bounds: Rect::new(20, 1020, 200, 60).unwrap(),
        belongs_to_monitor: true,
        minimized: false,
    });
    let rows = (0..141)
        .map(|index| PanelWindow::new(index.to_string(), format!("Window {index}"), false))
        .collect();
    let snapshot = PanelSnapshot::new(1, rows, 0).with_visibility_windows(facts, true);
    assert_eq!(crate::projection::project(&snapshot, "").rows.len(), 128);
    let (windows, foreground_interactable) = snapshot.visibility_facts().unwrap();
    assert_eq!(windows.len(), 141);
    let mut complete = VisibilityFacts {
        monitor,
        hitbox,
        windows,
        foreground_interactable,
    };
    assert!(
        any_window_overlaps(&complete),
        "tail eligibility is not a displayed-row fact"
    );
    complete.foreground_interactable = false;
    assert!(
        !any_window_overlaps(&complete),
        "eligible foreground remains a separate admission gate"
    );
    assert!(
        PanelSnapshot::new(0, Vec::new(), 0)
            .visibility_facts()
            .is_none()
    );
}

#[test]
fn favorite_save_admission_revalidates_after_native_menu_retirement_reopens_launcher() {
    let preferences = seeded_preferences()
        .with_launcher_favorites(vec!["app-editor".into(), "app-browser".into()])
        .unwrap()
        .with_general(
            crate::GeneralPreferences::default().with_start_of_week(crate::StartOfWeek::Saturday),
        );
    let fixture = LauncherFixture::with_preferences(preferences.clone());
    fixture.controller.open_launcher();
    fixture
        .controller
        .open_dock_menu(crate::generated::DockMenuKind::Bar, "", (12.0, 12.0));
    assert!(
        fixture
            .controller
            .menus
            .borrow()
            .as_ref()
            .unwrap()
            .is_open()
    );
    let controller = fixture.controller.clone();
    let launcher = fixture.launcher.as_weak();
    let host = Arc::clone(&fixture.host);
    let retired = Rc::new(Cell::new(false));
    let newer_focus = Rc::new(Cell::new(0));
    RECYCLE_MENU_DROP_HOOK.with(|hook| {
        let retired = Rc::clone(&retired);
        let newer_focus = Rc::clone(&newer_focus);
        *hook.borrow_mut() = Some(Box::new(move || {
            retired.set(true);
            controller.hide_launcher();
            controller.open_launcher();
            launcher.upgrade().unwrap().set_search("browser".into());
            controller.apply_launcher_filter();
            newer_focus.set(host.ui_focus_calls.load(Ordering::SeqCst));
        }));
    });
    fixture
        .launcher
        .invoke_favorite_toggle_requested("app-editor".into(), false);
    assert!(
        retired.get(),
        "real native popup attachment retired during admission"
    );
    assert!(
        fixture.host.saves.lock().is_empty(),
        "old source cannot write after native scope replacement"
    );
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    assert!(fixture.launcher.window().is_visible());
    assert_eq!(fixture.launcher.get_search(), "browser");
    assert_eq!(
        fixture.launcher.get_view(),
        crate::generated::LauncherView::All
    );
    assert_eq!(fixture.tile(0).key, "app-browser");
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        newer_focus.get()
    );
    assert!(fixture.host.launches.lock().is_empty());
}

#[test]
fn same_key_launcher_projection_reopen_and_save_revoke_a_held_native_tile_click() {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle};
    use slint::platform::{PointerEventButton, WindowEvent};
    for replacement in ["projection", "reopen", "save"] {
        let preferences = seeded_preferences()
            .with_launcher_favorites(vec!["app-editor".into(), "app-browser".into()])
            .unwrap();
        let fixture = LauncherFixture::with_preferences(preferences);
        fixture.controller.open_launcher();
        let tile = ElementHandle::find_by_accessible_label(&fixture.launcher, "Launch Rust Editor")
            .find(|element| element.accessible_role() == Some(AccessibleRole::Button))
            .expect("real current favorite tile");
        let position = native_center(&tile);
        fixture
            .launcher
            .window()
            .dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
        match replacement {
            "projection" => fixture.controller.apply_launcher_filter(),
            "reopen" => {
                fixture.controller.hide_launcher();
                fixture.controller.open_launcher();
            }
            "save" => fixture.panel.invoke_save_preferences_requested(),
            _ => unreachable!(),
        }
        assert!(fixture.launcher.window().is_visible());
        assert_eq!(fixture.launcher.get_application_count(), 2);
        fixture
            .launcher
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
        assert!(
            fixture.host.launches.lock().is_empty(),
            "old capture survived {replacement}"
        );
        assert_eq!(
            fixture.host.saves.lock().len(),
            usize::from(replacement == "save")
        );
        assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
    }
}
