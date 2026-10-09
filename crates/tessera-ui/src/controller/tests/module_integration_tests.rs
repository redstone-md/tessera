// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::quick_settings::{
    RecordedAudioRequest, RecordingAudioHost, recorded_audio_snapshot, recorded_audio_volume,
};
use tessera_system::audio::{AudioError, AudioErrorKind, AudioEvent, AudioFlow};
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

fn toolbar_audio_fixture() -> (LauncherFixture, Arc<RecordingAudioHost>) {
    let audio = Arc::new(RecordingAudioHost::default());
    let fixture = LauncherFixture::with_snapshot_host_configured(
        seeded_preferences().with_media_enabled(false),
        launcher_snapshot(),
        |host| *host.audio_provider.lock() = Some(audio.clone()),
        |_| {},
    );
    assert!(fixture.host.media_provider.lock().is_none());
    assert_eq!(fixture.host.audio_provider_calls.load(Ordering::SeqCst), 0);
    (fixture, audio)
}

fn loaded_toolbar_audio(
    fixture: &LauncherFixture,
    audio: &RecordingAudioHost,
) -> Rc<crate::quick_settings::QuickSettingsController> {
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    audio.finish(Ok(recorded_audio_snapshot(40.0, 30.0)));
    quick.component().invoke_audio_event_ready();
    assert!(quick.component().get_output_ready());
    quick
}

fn toolbar_audio_slider(
    popup: &crate::generated::QuickSettings,
) -> i_slint_backend_testing::ElementHandle {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle};
    ElementHandle::find_by_accessible_label(popup, "Output volume")
        .find(|element| element.accessible_role() == Some(AccessibleRole::Slider))
        .expect("real native output Slider")
}

fn retire_toolbar_audio_source(fixture: &LauncherFixture, retirement: &str) {
    match retirement {
        "hidden toolbar" => fixture.toolbar.hide().unwrap(),
        "toolbar geometry" => {
            let size = fixture.toolbar.window().size();
            fixture
                .toolbar
                .window()
                .set_size(slint::PhysicalSize::new(size.width + 1, size.height));
        }
        "root scope" => drop(power_menu::PowerAdmissionScope::new(&fixture.controller)),
        _ => unreachable!(),
    }
    assert!(
        fixture
            .controller
            .quick_settings
            .borrow()
            .as_ref()
            .unwrap()
            .is_open(),
        "exercise Root source retirement without closing Quick Settings",
    );
}

fn advance_toolbar_audio_timer(milliseconds: u64) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(milliseconds));
    slint::platform::update_timers_and_animations();
}

#[test]
fn toolbar_audio_trailing_volume_rejects_retired_sources_without_media() {
    for retirement in ["hidden toolbar", "toolbar geometry", "root scope"] {
        let (fixture, audio) = toolbar_audio_fixture();
        let quick = loaded_toolbar_audio(&fixture, &audio);
        let popup = quick.component();
        toolbar_audio_slider(&popup).set_accessible_value("60");
        assert_eq!(audio.requests(), [RecordedAudioRequest::Read]);
        retire_toolbar_audio_source(&fixture, retirement);
        advance_toolbar_audio_timer(100);
        assert_eq!(
            audio.requests(),
            [RecordedAudioRequest::Read],
            "the native 100ms trailing volume timer survived {retirement}",
        );
        assert_eq!(audio.watch_count(), 1);
        assert_eq!(fixture.host.audio_provider_calls.load(Ordering::SeqCst), 1);
        if retirement == "hidden toolbar" {
            fixture.toolbar.show().unwrap();
            advance_toolbar_audio_timer(1000);
            assert_eq!(
                audio.requests(),
                [RecordedAudioRequest::Read],
                "restored Toolbar visibility cannot resurrect an observed retired intent",
            );
        }
        assert!(fixture.host.saves.lock().is_empty());
    }
}

#[test]
fn toolbar_audio_mute_and_refresh_reject_retired_sources_without_media() {
    for action in ["mute", "refresh", "retry provider"] {
        let (fixture, audio) = toolbar_audio_fixture();
        let quick = if action == "retry provider" {
            *fixture.host.audio_provider.lock() = None;
            click_component(&fixture.toolbar, "Open quick settings");
            fixture.controller.quick_settings.borrow().clone().unwrap()
        } else {
            loaded_toolbar_audio(&fixture, &audio)
        };
        let popup = quick.component();
        if action == "refresh" {
            audio.event(AudioEvent::WatchUnavailable(AudioError::new(
                AudioErrorKind::Other,
                "Recording watch unavailable",
            )));
            popup.invoke_audio_event_ready();
        }
        let requests = audio.requests();
        let watches = audio.watch_count();
        let acquisitions = fixture.host.audio_provider_calls.load(Ordering::SeqCst);
        retire_toolbar_audio_source(&fixture, "hidden toolbar");
        if action == "mute" {
            click_component(&popup, "Mute output");
        } else {
            click_component(&popup, toolbar_media_refresh_label(&popup));
        }
        advance_toolbar_audio_timer(100);
        assert_eq!(audio.requests(), requests, "retired Root accepted {action}");
        assert_eq!(
            audio.watch_count(),
            watches,
            "retired Root subscribed for {action}"
        );
        assert_eq!(
            fixture.host.audio_provider_calls.load(Ordering::SeqCst),
            acquisitions,
            "retired Root acquired the audio factory for {action}",
        );
        assert!(quick.is_open());
        assert!(fixture.host.saves.lock().is_empty());
    }
}

#[test]
fn toolbar_audio_factory_source_retirement_prevents_watch_and_read_without_media() {
    for retirement in ["hidden toolbar", "toolbar geometry", "root scope"] {
        let (fixture, audio) = toolbar_audio_fixture();
        let toolbar = fixture.toolbar.clone_strong();
        let root = fixture.controller.clone();
        AUDIO_FACTORY_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || match retirement {
                "hidden toolbar" => toolbar.hide().unwrap(),
                "toolbar geometry" => {
                    let size = toolbar.window().size();
                    toolbar
                        .window()
                        .set_size(slint::PhysicalSize::new(size.width + 1, size.height));
                }
                "root scope" => drop(power_menu::PowerAdmissionScope::new(&root)),
                _ => unreachable!(),
            }));
        });
        click_component(&fixture.toolbar, "Open quick settings");
        assert!(AUDIO_FACTORY_HOOK.with(|hook| hook.borrow().is_none()));
        assert_eq!(fixture.host.audio_provider_calls.load(Ordering::SeqCst), 1);
        assert!(
            audio.requests().is_empty(),
            "factory source retired before native read: {retirement}"
        );
        assert_eq!(
            audio.watch_count(),
            0,
            "factory source retired before subscription: {retirement}"
        );
        assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 0);
        assert!(
            fixture
                .controller
                .quick_settings
                .borrow()
                .as_ref()
                .unwrap()
                .is_open()
        );
        assert!(fixture.host.saves.lock().is_empty());
    }
}

#[test]
fn toolbar_audio_accepted_completion_retires_source_without_pending_replay_or_overlap() {
    for reopen_before_completion in [false, true] {
        let (fixture, audio) = toolbar_audio_fixture();
        let quick = loaded_toolbar_audio(&fixture, &audio);
        let popup = quick.component();
        toolbar_audio_slider(&popup).set_accessible_value("60");
        advance_toolbar_audio_timer(100);
        let accepted = recorded_audio_volume(AudioFlow::Output, "output-A", 60.0);
        assert_eq!(
            audio.requests(),
            [RecordedAudioRequest::Read, accepted.clone()]
        );
        toolbar_audio_slider(&popup).set_accessible_value("90");
        advance_toolbar_audio_timer(100);
        retire_toolbar_audio_source(&fixture, "hidden toolbar");
        if reopen_before_completion {
            click_component(&popup, toolbar_media_refresh_label(&popup));
            fixture.toolbar.show().unwrap();
            click_component(&fixture.toolbar, "Open quick settings");
            assert_eq!(
                audio.requests(),
                [RecordedAudioRequest::Read, accepted.clone()],
                "a new popup must wait for the already accepted audio flight",
            );
        }
        audio.finish(Ok(recorded_audio_snapshot(60.0, 30.0)));
        popup.invoke_audio_event_ready();
        if !reopen_before_completion {
            assert_eq!(
                audio.requests(),
                [RecordedAudioRequest::Read, accepted.clone()],
                "accepted completion must not submit the retired trailing volume",
            );
            assert!(
                !popup.get_output_ready(),
                "retired source cannot present current audio authority"
            );
            fixture.toolbar.show().unwrap();
            click_component(&fixture.toolbar, "Open quick settings");
        }
        assert_eq!(
            audio.requests(),
            [
                RecordedAudioRequest::Read,
                accepted,
                RecordedAudioRequest::Read
            ],
        );
        assert!(
            !popup.get_output_ready(),
            "old flight cannot confirm the reopened popup"
        );
        audio.finish(Ok(recorded_audio_snapshot(30.0, 20.0)));
        popup.invoke_audio_event_ready();
        assert!((popup.get_output_percent() - 30.0).abs() < 0.001);
        advance_toolbar_audio_timer(1000);
        assert_eq!(
            audio.requests().len(),
            3,
            "no replay of retired 90 percent intent"
        );
        assert_eq!(
            audio.maximum_active(),
            1,
            "one shared audio flight across retirement/reopen"
        );
        assert!(fixture.host.saves.lock().is_empty());
    }
}

type ToolbarMediaObserver = Arc<dyn Fn(tessera_system::media::MediaEvent) + Send + Sync>;

#[derive(Default)]
struct ToolbarRecordingMedia {
    reads: AtomicUsize,
    commands: Mutex<Vec<tessera_system::media::MediaRequest>>,
    pending_reads: Mutex<std::collections::VecDeque<tessera_system::media::MediaReadCompletion>>,
    pending_commands:
        Mutex<std::collections::VecDeque<tessera_system::media::MediaCommandCompletion>>,
    watches: Mutex<Vec<ToolbarMediaObserver>>,
    watch_drops: Arc<AtomicUsize>,
}

struct ToolbarMediaWatch(Arc<AtomicUsize>);

impl Drop for ToolbarMediaWatch {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl tessera_system::media::MediaHost for ToolbarRecordingMedia {
    fn read(
        &self,
        completion: tessera_system::media::MediaReadCompletion,
    ) -> Result<(), tessera_system::media::MediaError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.pending_reads.lock().push_back(completion);
        Ok(())
    }

    fn execute(
        &self,
        command: tessera_system::media::MediaRequest,
        completion: tessera_system::media::MediaCommandCompletion,
    ) -> Result<(), tessera_system::media::MediaError> {
        self.commands.lock().push(command);
        self.pending_commands.lock().push_back(completion);
        Ok(())
    }

    fn subscribe(
        &self,
        changed: Arc<dyn Fn(tessera_system::media::MediaEvent) + Send + Sync>,
    ) -> Result<Option<Box<dyn Send>>, tessera_system::media::MediaError> {
        self.watches.lock().push(changed.clone());
        changed(tessera_system::media::MediaEvent::WatchReady);
        Ok(Some(Box::new(ToolbarMediaWatch(self.watch_drops.clone()))))
    }
}

impl ToolbarRecordingMedia {
    fn finish_read(
        &self,
        fixture: &LauncherFixture,
        snapshot: tessera_system::media::MediaSnapshot,
    ) {
        let completion = self
            .pending_reads
            .lock()
            .pop_front()
            .expect("accepted media read");
        completion(Ok(snapshot));
        // The real root wake route drains the shared domain even with Dock media off.
        fixture.dock.invoke_media_event_ready();
    }

    fn finish_command(&self, fixture: &LauncherFixture) {
        let completion = self
            .pending_commands
            .lock()
            .pop_front()
            .expect("accepted transport");
        completion(Ok(()));
        fixture.dock.invoke_media_event_ready();
    }

    fn event(&self, event: tessera_system::media::MediaEvent) {
        let callback = self
            .watches
            .lock()
            .last()
            .cloned()
            .expect("existing media watch");
        callback(event);
    }
}

fn toolbar_media_fixture(enabled: bool) -> (LauncherFixture, Arc<ToolbarRecordingMedia>) {
    let media = Arc::new(ToolbarRecordingMedia::default());
    let fixture = LauncherFixture::with_snapshot_host_configured(
        seeded_preferences().with_media_enabled(enabled),
        launcher_snapshot(),
        |host| *host.media_provider.lock() = Some(media.clone()),
        |_| {},
    );
    (fixture, media)
}

fn toolbar_media_snapshot() -> tessera_system::media::MediaSnapshot {
    use tessera_system::media::{
        MediaCapabilities, MediaPlayback, MediaSession, MediaSessionKey, MediaSnapshot,
        MediaTimeline,
    };
    MediaSnapshot {
        current: Some(MediaSession {
            key: MediaSessionKey::issue().unwrap(),
            source_app_id: "actual.player".into(),
            title: "Observed current track".into(),
            author: "Observed artist".into(),
            playback: MediaPlayback::Playing,
            capabilities: MediaCapabilities {
                previous: true,
                toggle: true,
                next: true,
            },
            timeline: Ok(MediaTimeline {
                start_ticks: 100_000_000,
                end_ticks: 1_300_000_000,
                position_ticks: 400_000_000,
                min_seek_ticks: 100_000_000,
                max_seek_ticks: 1_300_000_000,
                last_updated_utc_ticks: Some(133_000_000_000_000_000),
            }),
            seek: Err(tessera_system::media::MediaError::new(
                tessera_system::media::MediaErrorKind::Unsupported,
                "Seek not exposed by transport fixture",
            )),
            artwork: None,
            artwork_notice: None,
        }),
    }
}

fn press_toolbar_media(
    popup: &crate::generated::QuickSettings,
    label: &str,
) -> slint::LogicalPosition {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle};
    let control = ElementHandle::find_by_accessible_label(popup, label)
        .find(|element| element.accessible_role() == Some(AccessibleRole::Button))
        .expect("real current-player transport");
    let position = native_center(&control);
    popup
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerPressed {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
    position
}

fn release_toolbar_media(
    popup: &crate::generated::QuickSettings,
    position: slint::LogicalPosition,
) {
    popup
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerReleased {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
}

fn toolbar_media_refresh_label(popup: &crate::generated::QuickSettings) -> &'static str {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle};
    ["Retry audio", "Refresh audio and media", "Refresh audio"]
        .into_iter()
        .find(|label| {
            ElementHandle::find_by_accessible_label(popup, label)
                .any(|element| element.accessible_role() == Some(AccessibleRole::Button))
        })
        .expect("real audio/media refresh footer")
}

#[test]
fn toolbar_current_player_default_dock_off_observes_timeline_and_three_real_transports_without_saves()
 {
    use i_slint_backend_testing::ElementHandle;
    use tessera_system::media::{MediaAction, MediaCommand};
    let (fixture, media) = toolbar_media_fixture(false);
    let activity = fixture.host.unrelated_activity();
    let dock_rect = fixture
        .controller
        .leases
        .borrow()
        .attachments
        .get(&SurfaceKind::Dock)
        .unwrap()
        .rect;
    assert!(
        !fixture
            .controller
            .core
            .applied_preferences()
            .media_enabled()
    );
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 0);
    assert_eq!(media.reads.load(Ordering::SeqCst), 0);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    let popup = quick.component();
    assert!(quick.is_open() && quick.media_input_ready());
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(media.watches.lock().len(), 1);
    assert_eq!(media.reads.load(Ordering::SeqCst), 1);
    let snapshot = toolbar_media_snapshot();
    let key = snapshot.current.as_ref().unwrap().key;
    media.finish_read(&fixture, snapshot.clone());
    let view = popup.get_media_view();
    assert!(view.enabled && view.current_present && !view.stale && !view.busy);
    assert_eq!(view.title.as_str(), "Observed current track");
    assert_eq!(view.author.as_str(), "Observed artist");
    assert_eq!(view.playback.as_str(), "Playing");
    assert!(popup.get_timeline_available());
    assert_eq!(popup.get_timeline_time().as_str(), "0:30 / 2:00 · observed");
    assert!((popup.get_timeline_progress() - 0.25).abs() < 0.0001);
    assert!(popup.get_timeline_notice().is_empty());
    for (index, (label, action)) in [
        ("Previous track", MediaAction::Previous),
        ("Pause", MediaAction::Toggle),
        ("Next track", MediaAction::Next),
    ]
    .into_iter()
    .enumerate()
    {
        click_component(&popup, label);
        assert_eq!(
            media.commands.lock().as_slice(),
            &[
                MediaCommand {
                    expected_session: key,
                    action: MediaAction::Previous
                }
                .into(),
                MediaCommand {
                    expected_session: key,
                    action: MediaAction::Toggle
                }
                .into(),
                MediaCommand {
                    expected_session: key,
                    action: MediaAction::Next
                }
                .into(),
            ][..index + 1],
        );
        assert_eq!(
            media.commands.lock()[index],
            MediaCommand {
                expected_session: key,
                action
            }
            .into(),
        );
        assert!(popup.get_media_view().busy);
        click_component(&popup, "Next track");
        assert_eq!(
            media.commands.lock().len(),
            index + 1,
            "busy input is not queued"
        );
        media.finish_command(&fixture);
        assert!(
            popup.get_media_view().busy,
            "OS acceptance still needs readback"
        );
        assert_eq!(media.pending_reads.lock().len(), 1);
        assert_eq!(popup.get_media_view().playback.as_str(), "Playing");
        media.finish_read(&fixture, snapshot.clone());
        assert!(!popup.get_media_view().busy);
    }
    click_component(&popup, toolbar_media_refresh_label(&popup));
    assert_eq!(
        media.reads.load(Ordering::SeqCst),
        5,
        "current Toolbar scope admits refresh"
    );
    click_component(&popup, toolbar_media_refresh_label(&popup));
    assert_eq!(
        media.reads.load(Ordering::SeqCst),
        5,
        "refresh shares the existing read flight"
    );
    media.finish_read(&fixture, snapshot);
    assert_eq!(
        media.commands.lock().len(),
        3,
        "refresh only observes; transports never replay"
    );
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(media.watches.lock().len(), 1);
    assert!(!fixture.dock.get_media_view().enabled);
    assert_eq!(
        ElementHandle::find_by_accessible_label(&fixture.dock, "Media module").count(),
        0,
    );
    assert_eq!(
        fixture
            .controller
            .leases
            .borrow()
            .attachments
            .get(&SurfaceKind::Dock)
            .unwrap()
            .rect,
        dock_rect,
        "popup demand adds no Dock geometry slots",
    );
    assert!(
        !fixture
            .controller
            .core
            .applied_preferences()
            .media_enabled()
    );
    assert_eq!(fixture.host.unrelated_activity(), activity);
    quick.hide();
    assert!(!quick.media_input_ready());
    assert_eq!(media.watch_drops.load(Ordering::SeqCst), 1);
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn toolbar_current_player_hide_reopen_serializes_accepted_read_and_command_without_replay() {
    let (fixture, media) = toolbar_media_fixture(false);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    let popup = quick.component();
    assert_eq!(media.pending_reads.lock().len(), 1);
    quick.hide();
    click_component(&fixture.toolbar, "Open quick settings");
    assert_eq!(
        media.reads.load(Ordering::SeqCst),
        1,
        "accepted old read owns the flight"
    );
    media.finish_read(&fixture, toolbar_media_snapshot());
    assert!(
        !popup.get_media_view().current_present,
        "old presentation cannot publish metadata"
    );
    assert_eq!(media.reads.load(Ordering::SeqCst), 2);
    let snapshot = toolbar_media_snapshot();
    media.finish_read(&fixture, snapshot.clone());
    click_component(&popup, "Pause");
    assert_eq!(media.commands.lock().len(), 1);
    quick.hide();
    click_component(&fixture.toolbar, "Open quick settings");
    assert_eq!(
        media.reads.load(Ordering::SeqCst),
        2,
        "accepted transport owns the flight"
    );
    assert_eq!(media.commands.lock().len(), 1);
    media.finish_command(&fixture);
    assert_eq!(
        media.commands.lock().len(),
        1,
        "reopen never replays an accepted transport"
    );
    assert_eq!(media.reads.load(Ordering::SeqCst), 3);
    assert!(!popup.get_media_view().current_present);
    media.finish_read(&fixture, snapshot);
    assert!(quick.media_input_ready());
    assert!(popup.get_media_view().current_present);
    assert!(!popup.get_media_view().busy);
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(media.pending_reads.lock().len(), 0);
    assert_eq!(media.pending_commands.lock().len(), 0);
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn toolbar_current_player_held_input_rejects_old_popup_hidden_toolbar_and_exclusive_popup() {
    for refresh in [false, true] {
        for retirement in [
            "reopen",
            "hidden toolbar",
            "toolbar geometry",
            "root scope",
            "exclusive popup",
        ] {
            let (fixture, media) = toolbar_media_fixture(false);
            click_component(&fixture.toolbar, "Open quick settings");
            let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
            let popup = quick.component();
            let label = if refresh {
                toolbar_media_refresh_label(&popup)
            } else {
                "Next track"
            };
            let snapshot = toolbar_media_snapshot();
            media.finish_read(&fixture, snapshot.clone());
            let position = press_toolbar_media(&popup, label);
            match retirement {
                "reopen" => {
                    quick.hide();
                    click_component(&fixture.toolbar, "Open quick settings");
                    media.finish_read(&fixture, snapshot.clone());
                    assert!(quick.media_input_ready());
                }
                "hidden toolbar" => {
                    fixture.toolbar.hide().unwrap();
                    assert!(
                        quick.is_open(),
                        "test root source admission, not just popup visibility"
                    );
                }
                "root scope" => {
                    drop(power_menu::PowerAdmissionScope::new(&fixture.controller));
                    assert!(
                        quick.is_open(),
                        "test root authority independent of popup visibility"
                    );
                }
                "toolbar geometry" => {
                    let size = fixture.toolbar.window().size();
                    fixture
                        .toolbar
                        .window()
                        .set_size(slint::PhysicalSize::new(size.width + 1, size.height));
                    assert!(
                        quick.is_open(),
                        "test actual captured Toolbar geometry admission"
                    );
                }
                "exclusive popup" => {
                    click_component(&fixture.toolbar, "Open network");
                    assert!(
                        fixture
                            .controller
                            .network_menu
                            .borrow()
                            .as_ref()
                            .unwrap()
                            .is_open()
                    );
                    assert!(!quick.is_open());
                }
                _ => unreachable!(),
            }
            let reads = media.reads.load(Ordering::SeqCst);
            let watches = media.watches.lock().len();
            let acquisitions = fixture.host.media_provider_calls.load(Ordering::SeqCst);
            release_toolbar_media(&popup, position);
            assert!(
                media.commands.lock().is_empty(),
                "held {label} input survived {retirement}"
            );
            assert_eq!(
                media.reads.load(Ordering::SeqCst),
                reads,
                "held {label} refreshed after {retirement}"
            );
            assert_eq!(media.watches.lock().len(), watches);
            assert_eq!(
                fixture.host.media_provider_calls.load(Ordering::SeqCst),
                acquisitions
            );
            if matches!(
                retirement,
                "hidden toolbar" | "toolbar geometry" | "root scope"
            ) {
                assert!(
                    !quick.media_input_ready(),
                    "retired source has no media authority"
                );
                let view = popup.get_media_view();
                assert!(!view.enabled && !view.current_present);
                assert!(view.session_identity.is_empty());
                assert!(!popup.get_timeline_available());
                assert!(popup.get_timeline_time().is_empty());
                assert_eq!(popup.get_timeline_progress(), 0.0);
                assert!(
                    i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                        &popup,
                        "Next track",
                    )
                    .next()
                    .is_none(),
                    "retired current-player transport is no longer an AX input target",
                );
                if refresh {
                    click_component(&popup, toolbar_media_refresh_label(&popup));
                } else {
                    // The truthful retired projection removes this AX node. A new
                    // native gesture at its captured former position still has no authority.
                    popup
                        .window()
                        .dispatch_event(slint::platform::WindowEvent::PointerPressed {
                            position,
                            button: slint::platform::PointerEventButton::Left,
                        });
                    release_toolbar_media(&popup, position);
                }
                assert!(
                    media.commands.lock().is_empty(),
                    "new input needs its actual Toolbar source"
                );
                assert_eq!(media.reads.load(Ordering::SeqCst), reads);
                assert_eq!(media.watches.lock().len(), watches);
                assert_eq!(
                    fixture.host.media_provider_calls.load(Ordering::SeqCst),
                    acquisitions
                );
            }
            assert!(fixture.host.saves.lock().is_empty());
            assert!(!fixture.dock.get_media_view().enabled);
        }
    }
}

#[test]
fn toolbar_current_player_invalidation_session_switch_and_timeline_failure_keep_authority_truthful()
{
    use tessera_system::media::{MediaError, MediaErrorKind, MediaEvent};
    let (fixture, media) = toolbar_media_fixture(false);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    let popup = quick.component();
    let original = toolbar_media_snapshot();
    media.finish_read(&fixture, original.clone());
    let position = press_toolbar_media(&popup, "Next track");
    media.event(MediaEvent::Changed);
    release_toolbar_media(&popup, position);
    assert!(
        media.commands.lock().is_empty(),
        "undrained native invalidation blocks dispatch"
    );
    fixture.dock.invoke_media_event_ready();
    assert_eq!(media.pending_reads.lock().len(), 1);
    media.event(MediaEvent::Changed);
    media.finish_read(&fixture, original.clone());
    assert!(popup.get_media_view().stale);
    assert_eq!(
        media.pending_reads.lock().len(),
        1,
        "late invalidation needs one follow-up read"
    );
    media.finish_read(&fixture, original);
    let position = press_toolbar_media(&popup, "Next track");
    media.event(MediaEvent::Changed);
    fixture.dock.invoke_media_event_ready();
    let mut replacement = toolbar_media_snapshot();
    let session = replacement.current.as_mut().unwrap();
    session.title = "Replacement current track".into();
    session.timeline = Err(MediaError::new(
        MediaErrorKind::Unavailable,
        "Timeline not exposed",
    ));
    let replacement_key = session.key;
    media.finish_read(&fixture, replacement.clone());
    release_toolbar_media(&popup, position);
    assert!(
        media.commands.lock().is_empty(),
        "old press cannot target replacement incarnation"
    );
    assert_eq!(
        popup.get_media_view().title.as_str(),
        "Replacement current track"
    );
    assert!(!popup.get_timeline_available());
    assert!(popup.get_timeline_time().is_empty());
    assert_eq!(popup.get_timeline_progress(), 0.0);
    assert!(
        popup
            .get_timeline_notice()
            .as_str()
            .contains("Timeline not exposed")
    );
    assert!(popup.get_media_view().previous_enabled);
    assert!(popup.get_media_view().toggle_enabled);
    assert!(popup.get_media_view().next_enabled);
    media.event(MediaEvent::WatchUnavailable(MediaError::new(
        MediaErrorKind::Unavailable,
        "Recorded watch failure",
    )));
    fixture.dock.invoke_media_event_ready();
    assert_eq!(
        popup.get_media_view().watch_notice.as_str(),
        "Recorded watch failure"
    );
    assert_eq!(
        media.reads.load(Ordering::SeqCst),
        4,
        "watch failure is not a retry loop"
    );
    click_component(&popup, "Next track");
    assert_eq!(
        media.commands.lock()[0],
        tessera_system::media::MediaCommand {
            expected_session: replacement_key,
            action: tessera_system::media::MediaAction::Next,
        }
        .into(),
    );
    media.finish_command(&fixture);
    media.finish_read(&fixture, replacement);
    assert_eq!(media.commands.lock().len(), 1);
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(media.watches.lock().len(), 1);
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn toolbar_current_player_and_saved_dock_media_share_one_provider_watch_and_readback() {
    let (fixture, media) = toolbar_media_fixture(true);
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(media.pending_reads.lock().len(), 1);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    let popup = quick.component();
    assert_eq!(media.reads.load(Ordering::SeqCst), 1);
    assert_eq!(media.watches.lock().len(), 1);
    let snapshot = toolbar_media_snapshot();
    media.finish_read(&fixture, snapshot.clone());
    assert_eq!(
        fixture.dock.get_media_view().title,
        popup.get_media_view().title
    );
    click_component(&popup, "Pause");
    assert!(fixture.dock.get_media_view().busy);
    quick.hide();
    assert_eq!(
        media.watch_drops.load(Ordering::SeqCst),
        0,
        "saved Dock demand retains its watch"
    );
    media.finish_command(&fixture);
    assert_eq!(media.reads.load(Ordering::SeqCst), 2);
    media.finish_read(&fixture, snapshot);
    assert!(!fixture.dock.get_media_view().busy);
    click_component(&fixture.toolbar, "Open quick settings");
    assert_eq!(
        media.reads.load(Ordering::SeqCst),
        2,
        "attach reuses confirmed shared observation"
    );
    assert!(popup.get_media_view().current_present);
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(media.watches.lock().len(), 1);
    assert_eq!(media.commands.lock().len(), 1);
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn toolbar_current_player_media_factory_source_retirement_prevents_unaccepted_native_observation() {
    for root_closed in [false, true] {
        let (fixture, media) = toolbar_media_fixture(false);
        let root = fixture.controller.clone();
        let toolbar = fixture.toolbar.clone_strong();
        MEDIA_FACTORY_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                if root_closed {
                    drop(power_menu::PowerAdmissionScope::new(&root));
                } else {
                    toolbar.hide().unwrap();
                }
            }));
        });
        click_component(&fixture.toolbar, "Open quick settings");
        assert!(MEDIA_FACTORY_HOOK.with(|hook| hook.borrow().is_none()));
        assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            media.reads.load(Ordering::SeqCst),
            0,
            "retired source must reject native read before acceptance",
        );
        assert!(
            media.watches.lock().is_empty(),
            "retired source must not subscribe"
        );
        assert!(media.pending_reads.lock().is_empty());
        assert!(media.commands.lock().is_empty());
        assert!(fixture.host.saves.lock().is_empty());
        assert!(!fixture.dock.get_media_view().enabled);
        if root_closed {
            assert!(fixture.controller.power_admission_closed.get());
            fixture.dock.invoke_media_event_ready();
            assert_eq!(media.reads.load(Ordering::SeqCst), 0);
        } else {
            assert!(!fixture.toolbar.window().is_visible());
            fixture.toolbar.show().unwrap();
            click_component(&fixture.toolbar, "Open quick settings");
            let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
            assert!(quick.media_input_ready());
            assert_eq!(media.reads.load(Ordering::SeqCst), 1);
            assert_eq!(media.watches.lock().len(), 1);
            media.finish_read(&fixture, toolbar_media_snapshot());
            click_component(&quick.component(), "Next track");
            assert_eq!(
                media.commands.lock().len(),
                1,
                "new real source can dispatch"
            );
        }
    }
}

#[test]
fn toolbar_current_player_media_factory_reentry_preserves_new_popup_without_old_observation() {
    for replacement in ["quick settings", "network"] {
        let (fixture, media) = toolbar_media_fixture(false);
        let toolbar = fixture.toolbar.clone_strong();
        MEDIA_FACTORY_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                click_component(
                    &toolbar,
                    if replacement == "network" {
                        "Open network"
                    } else {
                        "Open quick settings"
                    },
                );
            }));
        });
        click_component(&fixture.toolbar, "Open quick settings");
        assert!(MEDIA_FACTORY_HOOK.with(|hook| hook.borrow().is_none()));
        let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
        if replacement == "network" {
            assert!(
                fixture
                    .controller
                    .network_menu
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .is_open()
            );
            assert!(!quick.is_open());
            assert!(!quick.media_input_ready());
            assert_eq!(media.reads.load(Ordering::SeqCst), 0);
            assert!(media.watches.lock().is_empty());
            assert_eq!(media.watch_drops.load(Ordering::SeqCst), 0);
            click_component(&fixture.toolbar, "Open quick settings");
            assert!(
                !fixture
                    .controller
                    .network_menu
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .is_open()
            );
        }
        assert!(quick.is_open() && quick.media_input_ready());
        assert_eq!(
            media.reads.load(Ordering::SeqCst),
            1,
            "only replacement Quick can observe"
        );
        assert_eq!(media.pending_reads.lock().len(), 1);
        assert_eq!(
            media.watches.lock().len(),
            1,
            "old factory cannot duplicate a subscription"
        );
        assert_eq!(media.watch_drops.load(Ordering::SeqCst), 0);
        let snapshot = toolbar_media_snapshot();
        let key = snapshot.current.as_ref().unwrap().key;
        media.finish_read(&fixture, snapshot.clone());
        click_component(&quick.component(), "Next track");
        assert_eq!(
            media.commands.lock()[0],
            tessera_system::media::MediaCommand {
                expected_session: key,
                action: tessera_system::media::MediaAction::Next,
            }
            .into(),
        );
        media.finish_command(&fixture);
        media.finish_read(&fixture, snapshot);
        assert!(quick.is_open() && quick.media_input_ready());
        assert_eq!(media.commands.lock().len(), 1);
        assert!(fixture.host.saves.lock().is_empty());
    }
}

#[test]
fn toolbar_current_player_finite_overflow_open_preserves_live_popup_lease_and_input_authority() {
    let (fixture, media) = toolbar_media_fixture(false);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    let popup = quick.component();
    let snapshot = toolbar_media_snapshot();
    media.finish_read(&fixture, snapshot.clone());
    let identity = popup.get_media_view().session_identity;
    let position = press_toolbar_media(&popup, "Next track");
    let lease_drops = fixture.host.lease_drops.load(Ordering::SeqCst);
    let focuses = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let popup_position = popup.window().position();
    let popup_size = popup.window().size();
    // Rejected coordinates are synthetic; accepted activation remains genuine input.
    fixture
        .toolbar
        .invoke_quick_settings_requested(crate::generated::TileBounds {
            origin: slint::LogicalPosition::new(f32::MAX, 0.0),
            width: 16.0,
            height: 16.0,
        });
    assert!(quick.is_open() && quick.media_input_ready());
    assert!(Rc::ptr_eq(
        &quick,
        fixture.controller.quick_settings.borrow().as_ref().unwrap()
    ));
    assert_eq!(popup.get_media_view().session_identity, identity);
    assert_eq!(popup.window().position(), popup_position);
    assert_eq!(popup.window().size(), popup_size);
    assert_eq!(fixture.host.lease_drops.load(Ordering::SeqCst), lease_drops);
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), focuses);
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(media.reads.load(Ordering::SeqCst), 1);
    assert_eq!(media.watches.lock().len(), 1);
    assert_eq!(media.watch_drops.load(Ordering::SeqCst), 0);
    release_toolbar_media(&popup, position);
    assert_eq!(
        media.commands.lock().len(),
        1,
        "rejected placement must retain the already-captured genuine transport scope",
    );
    media.finish_command(&fixture);
    media.finish_read(&fixture, snapshot.clone());
    click_component(&popup, toolbar_media_refresh_label(&popup));
    assert_eq!(media.reads.load(Ordering::SeqCst), 3);
    media.finish_read(&fixture, snapshot);
    assert_eq!(
        media.commands.lock().len(),
        1,
        "refresh observes without replay"
    );
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(media.watches.lock().len(), 1);
    assert_eq!(media.watch_drops.load(Ordering::SeqCst), 0);
    assert!(quick.is_open() && quick.media_input_ready());
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn toolbar_current_player_retained_domain_does_not_retain_closed_root_caches_or_windows() {
    let (fixture, media) = toolbar_media_fixture(false);
    let quick_cache_owners = Rc::strong_count(&fixture.controller.quick_settings);
    let media_cache_owners = Rc::strong_count(&fixture.controller.dock_media);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    media.finish_read(&fixture, toolbar_media_snapshot());
    assert!(quick.is_open() && quick.media_input_ready());
    assert!(quick.component().get_media_view().current_present);
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(media.watches.lock().len(), 1);
    assert_eq!(
        Rc::strong_count(&fixture.controller.quick_settings),
        quick_cache_owners,
        "live SourceGuard and media/refresh callbacks must not add a Root Quick-cache owner",
    );
    assert_eq!(
        Rc::strong_count(&fixture.controller.dock_media),
        media_cache_owners,
        "live popup attachment must not strongly capture the Root media cache",
    );
    let domain = fixture.controller.dock_media.borrow().clone().unwrap();
    let domain_weak = Rc::downgrade(&domain);
    let quick_cache = Rc::downgrade(&fixture.controller.quick_settings);
    let media_cache = Rc::downgrade(&fixture.controller.dock_media);
    let geometry = Rc::downgrade(&fixture.controller.visibility_geometry);
    let popup_window = quick.component().as_weak();
    let panel_window = fixture.panel.as_weak();
    let dock_window = fixture.dock.as_weak();
    let toolbar_window = fixture.toolbar.as_weak();
    let launcher_window = fixture.launcher.as_weak();
    drop(quick);
    drop(fixture);
    assert!(
        quick_cache.upgrade().is_none(),
        "media callbacks must not retain the Root Quick cache"
    );
    assert!(
        media_cache.upgrade().is_none(),
        "SourceGuard must not retain the Root media cache"
    );
    assert!(
        geometry.upgrade().is_none(),
        "SourceGuard keeps Root geometry weak"
    );
    assert!(popup_window.upgrade().is_none());
    assert!(panel_window.upgrade().is_none());
    assert!(dock_window.upgrade().is_none());
    assert!(toolbar_window.upgrade().is_none());
    assert!(launcher_window.upgrade().is_none());
    assert!(
        domain_weak.upgrade().is_some(),
        "externally retained media domain still exists"
    );
    assert_eq!(media.watch_drops.load(Ordering::SeqCst), 1);
    domain.close();
    drop(domain);
    assert!(
        domain_weak.upgrade().is_none(),
        "cleanup does not leave a domain ownership cycle"
    );
}

fn toolbar_seek_snapshot() -> tessera_system::media::MediaSnapshot {
    use tessera_system::media::{MediaObservationRevision, MediaSeekObservation};
    let mut snapshot = toolbar_media_snapshot();
    snapshot.current.as_mut().unwrap().seek = Ok(MediaSeekObservation {
        revision: MediaObservationRevision::issue().unwrap(),
        min_ticks: 100_000_000,
        max_ticks: 1_300_000_000,
    });
    snapshot
}

fn toolbar_seek_midpoint_snapshot() -> tessera_system::media::MediaSnapshot {
    let mut snapshot = toolbar_seek_snapshot();
    snapshot
        .current
        .as_mut()
        .unwrap()
        .timeline
        .as_mut()
        .unwrap()
        .position_ticks = 700_000_000;
    snapshot
}

fn toolbar_seek_element(
    popup: &crate::generated::QuickSettings,
) -> i_slint_backend_testing::ElementHandle {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle};
    ElementHandle::find_by_accessible_label(popup, "Media position")
        .find(|element| element.accessible_role() == Some(AccessibleRole::Slider))
        .expect("real current-player seek Slider")
}

// Only production raw observation and its post-dispatch fallback are adapted
// for the software backend. Slider input uses the genuine Window event route.
fn dispatch_toolbar_seek_pointer(
    quick: &crate::quick_settings::QuickSettingsController,
    position: slint::LogicalPosition,
    pressed: bool,
) {
    use slint::platform::{PointerEventButton, WindowEvent};
    quick.observe_media_pointer(position, pressed);
    quick.component().window().dispatch_event(if pressed {
        WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        }
    } else {
        WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        }
    });
    if !pressed {
        quick.finish_media_pointer_release();
    }
}

fn flush_toolbar_seek_timer(
    fixture: &LauncherFixture,
    quick: &crate::quick_settings::QuickSettingsController,
) {
    quick.elapse_seek_throttle(std::time::Duration::from_millis(200));
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    fixture.dock.invoke_media_event_ready();
}

#[test]
fn toolbar_current_player_genuine_ax_seek_captures_exact_raw_authority_and_waits_for_readback() {
    use tessera_system::media::{MediaRequest, MediaSeekCommand};
    let (fixture, media) = toolbar_media_fixture(false);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    let popup = quick.component();
    let snapshot = toolbar_seek_snapshot();
    let session = snapshot.current.as_ref().unwrap();
    let seek = session.seek.as_ref().unwrap();
    let expected = MediaRequest::Seek(MediaSeekCommand {
        expected_session: session.key,
        expected_revision: seek.revision,
        observed_min_ticks: 100_000_000,
        observed_max_ticks: 1_300_000_000,
        position_ticks: 1_000_000_000,
    });
    media.finish_read(&fixture, snapshot.clone());
    assert!(popup.get_seek_visible() && popup.get_seek_enabled());
    toolbar_seek_element(&popup).set_accessible_value("0.75");
    flush_toolbar_seek_timer(&fixture, &quick);
    assert_eq!(media.commands.lock().as_slice(), &[expected]);
    assert_eq!(
        popup.get_timeline_time().as_str(),
        "0:30 / 2:00 · observed",
        "requested position is not confirmed native progress",
    );
    media.finish_command(&fixture);
    assert_eq!(media.pending_reads.lock().len(), 1);
    media.finish_read(&fixture, snapshot);
    assert_eq!(media.commands.lock().as_slice(), &[expected]);
    assert!(fixture.host.saves.lock().is_empty());
    assert!(!fixture.dock.get_media_view().enabled);
}

#[test]
fn toolbar_current_player_held_seek_cannot_cross_same_key_revision_or_session_replacement() {
    use tessera_system::media::{MediaEvent, MediaObservationRevision, MediaSessionKey};
    for replace_session in [false, true] {
        let (fixture, media) = toolbar_media_fixture(false);
        click_component(&fixture.toolbar, "Open quick settings");
        let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
        let popup = quick.component();
        let snapshot = toolbar_seek_snapshot();
        media.finish_read(&fixture, snapshot.clone());
        let position = native_center(&toolbar_seek_element(&popup));
        dispatch_toolbar_seek_pointer(&quick, position, true);
        // A leading request is legitimate; its completion must not authorize
        // the still-held gesture against a later native observation.
        if !media.pending_commands.lock().is_empty() {
            media.finish_command(&fixture);
            media.finish_read(&fixture, snapshot.clone());
        }
        let accepted = media.commands.lock().len();
        let mut replacement = snapshot;
        let session = replacement.current.as_mut().unwrap();
        if replace_session {
            session.key = MediaSessionKey::issue().unwrap();
        }
        session.seek.as_mut().unwrap().revision = MediaObservationRevision::issue().unwrap();
        // Keep title, range, timeline, and (in one case) session key identical:
        // only opaque observation authority can distinguish this readback.
        media.event(MediaEvent::SeekInvalidated);
        fixture.dock.invoke_media_event_ready();
        media.finish_read(&fixture, replacement.clone());
        toolbar_seek_element(&popup).set_accessible_value("0.9");
        dispatch_toolbar_seek_pointer(&quick, position, false);
        flush_toolbar_seek_timer(&fixture, &quick);
        assert_eq!(
            media.commands.lock().len(),
            accepted,
            "held seek retargeted replacement authority; replace_session={replace_session}",
        );
        toolbar_seek_element(&popup).set_accessible_value("0.75");
        flush_toolbar_seek_timer(&fixture, &quick);
        let session = replacement.current.as_ref().unwrap();
        let seek = session.seek.as_ref().unwrap();
        assert_eq!(
            media.commands.lock()[accepted],
            tessera_system::media::MediaRequest::Seek(tessera_system::media::MediaSeekCommand {
                expected_session: session.key,
                expected_revision: seek.revision,
                observed_min_ticks: 100_000_000,
                observed_max_ticks: 1_300_000_000,
                position_ticks: 1_000_000_000,
            }),
            "fresh AX input after release uses current native authority",
        );
        assert!(fixture.host.saves.lock().is_empty());
    }
}

#[test]
fn toolbar_current_player_held_seek_rejects_reopened_hidden_and_exclusive_root_sources() {
    for retirement in ["reopen", "hidden toolbar", "exclusive popup"] {
        let (fixture, media) = toolbar_media_fixture(false);
        click_component(&fixture.toolbar, "Open quick settings");
        let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
        let popup = quick.component();
        let snapshot = toolbar_seek_snapshot();
        media.finish_read(&fixture, snapshot.clone());
        let position = native_center(&toolbar_seek_element(&popup));
        dispatch_toolbar_seek_pointer(&quick, position, true);
        if !media.pending_commands.lock().is_empty() {
            media.finish_command(&fixture);
            media.finish_read(&fixture, snapshot.clone());
        }
        let accepted = media.commands.lock().len();
        match retirement {
            "reopen" => {
                quick.hide();
                click_component(&fixture.toolbar, "Open quick settings");
                media.finish_read(&fixture, snapshot);
                assert!(quick.media_input_ready());
                toolbar_seek_element(&popup).set_accessible_value("0.9");
            }
            "hidden toolbar" => {
                fixture.toolbar.hide().unwrap();
                assert!(quick.is_open(), "source retirement is not popup closure");
                toolbar_seek_element(&popup).set_accessible_value("0.9");
            }
            "exclusive popup" => {
                click_component(&fixture.toolbar, "Open network");
                assert!(!quick.is_open());
            }
            _ => unreachable!(),
        }
        let reads = media.reads.load(Ordering::SeqCst);
        dispatch_toolbar_seek_pointer(&quick, position, false);
        flush_toolbar_seek_timer(&fixture, &quick);
        assert_eq!(
            media.commands.lock().len(),
            accepted,
            "held seek survived {retirement}",
        );
        assert_eq!(
            media.reads.load(Ordering::SeqCst),
            reads,
            "stale seek release must not start a replacement observation",
        );
        assert!(fixture.host.saves.lock().is_empty());
        assert!(!fixture.dock.get_media_view().enabled);
    }
}

#[test]
fn toolbar_current_player_genuine_ax_seek_coalesces_latest_on_200ms_timer_without_release() {
    use tessera_system::media::{MediaRequest, MediaSeekCommand};
    let (fixture, media) = toolbar_media_fixture(false);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    let popup = quick.component();
    let snapshot = toolbar_seek_snapshot();
    let session = snapshot.current.as_ref().unwrap();
    let seek = session.seek.as_ref().unwrap();
    let first = MediaSeekCommand {
        expected_session: session.key,
        expected_revision: seek.revision,
        observed_min_ticks: 100_000_000,
        observed_max_ticks: 1_300_000_000,
        position_ticks: 700_000_000,
    };
    let latest = MediaSeekCommand {
        position_ticks: 1_000_000_000,
        ..first
    };
    media.finish_read(&fixture, snapshot.clone());
    let slider = toolbar_seek_element(&popup);
    slider.set_accessible_value("0.5");
    assert_eq!(
        media.commands.lock().as_slice(),
        &[MediaRequest::Seek(first)]
    );
    slider.set_accessible_value("0.6");
    slider.set_accessible_value("0.75");
    assert_eq!(
        media.commands.lock().as_slice(),
        &[MediaRequest::Seek(first)],
        "busy input records only one latest intent, not another native flight",
    );
    media.finish_command(&fixture);
    assert_eq!(media.pending_reads.lock().len(), 1);
    media.finish_read(&fixture, snapshot.clone());
    quick.elapse_seek_throttle(std::time::Duration::from_millis(199));
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(199));
    slint::platform::update_timers_and_animations();
    fixture.dock.invoke_media_event_ready();
    assert_eq!(
        media.commands.lock().as_slice(),
        &[MediaRequest::Seek(first)],
        "trailing intent must not issue before the source 200ms deadline",
    );
    quick.elapse_seek_throttle(std::time::Duration::from_millis(1));
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    slint::platform::update_timers_and_animations();
    fixture.dock.invoke_media_event_ready();
    assert_eq!(
        media.commands.lock().as_slice(),
        &[MediaRequest::Seek(first), MediaRequest::Seek(latest)],
        "timer dispatches only the latest intent without a pointer release",
    );
    media.finish_command(&fixture);
    media.finish_read(&fixture, snapshot);
    flush_toolbar_seek_timer(&fixture, &quick);
    assert_eq!(
        media.commands.lock().len(),
        2,
        "accepted requests never replay"
    );
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn toolbar_current_player_real_held_pointer_move_cannot_retarget_same_key_seek_revision() {
    use slint::platform::WindowEvent;
    use tessera_system::media::{
        MediaEvent, MediaObservationRevision, MediaRequest, MediaSeekCommand,
    };
    let (fixture, media) = toolbar_media_fixture(false);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    let popup = quick.component();
    let mut snapshot = toolbar_seek_snapshot();
    snapshot
        .current
        .as_mut()
        .unwrap()
        .timeline
        .as_mut()
        .unwrap()
        .position_ticks = 700_000_000;
    media.finish_read(&fixture, snapshot.clone());
    assert!((popup.get_seek_progress() - 0.5).abs() < 0.0001);
    let slider = toolbar_seek_element(&popup);
    let position = native_center(&slider);
    // At exactly half range, native Slider symmetry puts the actual thumb at
    // its geometry center. Down captures authority without changing the value.
    dispatch_toolbar_seek_pointer(&quick, position, true);
    assert!(
        media.commands.lock().is_empty(),
        "pressing the unchanged native midpoint thumb must not seek",
    );
    let mut replacement = snapshot;
    let session = replacement.current.as_mut().unwrap();
    session.seek.as_mut().unwrap().revision = MediaObservationRevision::issue().unwrap();
    media.event(MediaEvent::SeekInvalidated);
    fixture.dock.invoke_media_event_ready();
    media.finish_read(&fixture, replacement.clone());
    let moved = slint::LogicalPosition::new(position.x + slider.size().width * 0.25, position.y);
    popup
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: moved });
    flush_toolbar_seek_timer(&fixture, &quick);
    assert!(
        media.commands.lock().is_empty(),
        "real old pointer movement cannot mint the refreshed same-key revision",
    );
    dispatch_toolbar_seek_pointer(&quick, moved, false);
    flush_toolbar_seek_timer(&fixture, &quick);
    assert!(
        media.commands.lock().is_empty(),
        "real old pointer release cannot flush a seek against new authority",
    );
    toolbar_seek_element(&popup).set_accessible_value("0.75");
    flush_toolbar_seek_timer(&fixture, &quick);
    let session = replacement.current.as_ref().unwrap();
    let seek = session.seek.as_ref().unwrap();
    assert_eq!(
        media.commands.lock().as_slice(),
        &[MediaRequest::Seek(MediaSeekCommand {
            expected_session: session.key,
            expected_revision: seek.revision,
            observed_min_ticks: 100_000_000,
            observed_max_ticks: 1_300_000_000,
            position_ticks: 1_000_000_000,
        })],
        "fresh AX input after the old hold ends uses refreshed authority",
    );
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn toolbar_current_player_buffered_pointer_move_at_release_cannot_refresh_seek_authority() {
    use slint::platform::{PointerEventButton, WindowEvent};
    use tessera_system::media::{
        MediaEvent, MediaObservationRevision, MediaRequest, MediaSeekCommand,
    };
    let (fixture, media) = toolbar_media_fixture(false);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    let popup = quick.component();
    let snapshot = toolbar_seek_midpoint_snapshot();
    media.finish_read(&fixture, snapshot.clone());
    assert!((popup.get_seek_progress() - 0.5).abs() < 0.0001);
    let slider = toolbar_seek_element(&popup);
    let position = native_center(&slider);
    dispatch_toolbar_seek_pointer(&quick, position, true);
    assert!(
        media.commands.lock().is_empty(),
        "pressing the unchanged native midpoint thumb must not seek",
    );
    let mut replacement = snapshot;
    replacement
        .current
        .as_mut()
        .unwrap()
        .seek
        .as_mut()
        .unwrap()
        .revision = MediaObservationRevision::issue().unwrap();
    media.event(MediaEvent::SeekInvalidated);
    fixture.dock.invoke_media_event_ready();
    media.finish_read(&fixture, replacement.clone());
    let reads = media.reads.load(Ordering::SeqCst);
    let moved = slint::LogicalPosition::new(position.x + slider.size().width * 0.25, position.y);
    // Winit runs the raw release filter before flushing its buffered move,
    // then delivers native release. Do not finish the fence between them.
    quick.observe_media_pointer(moved, false);
    popup
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: moved });
    assert!(
        media.commands.lock().is_empty(),
        "buffered movement after raw release must retain stale captured authority",
    );
    popup.window().dispatch_event(WindowEvent::PointerReleased {
        position: moved,
        button: PointerEventButton::Left,
    });
    quick.finish_media_pointer_release();
    flush_toolbar_seek_timer(&fixture, &quick);
    assert!(
        media.commands.lock().is_empty(),
        "native release must not flush buffered movement against refreshed authority",
    );
    assert_eq!(
        media.reads.load(Ordering::SeqCst),
        reads,
        "buffered stale release must not start a replacement observation",
    );
    toolbar_seek_element(&popup).set_accessible_value("0.75");
    flush_toolbar_seek_timer(&fixture, &quick);
    let session = replacement.current.as_ref().unwrap();
    let seek = session.seek.as_ref().unwrap();
    assert_eq!(
        media.commands.lock().as_slice(),
        &[MediaRequest::Seek(MediaSeekCommand {
            expected_session: session.key,
            expected_revision: seek.revision,
            observed_min_ticks: 100_000_000,
            observed_max_ticks: 1_300_000_000,
            position_ticks: 1_000_000_000,
        })],
        "fresh AX input only after native release uses refreshed authority",
    );
    assert!(fixture.host.saves.lock().is_empty());
    assert!(!fixture.dock.get_media_view().enabled);
}

#[test]
fn toolbar_current_player_touch_contact_promotion_cannot_admit_seek_while_another_contact_is_held()
{
    use slint::platform::{PointerEventButton, WindowEvent};
    use slint::winit_030::winit::event::TouchPhase;
    use tessera_system::media::{
        MediaEvent, MediaObservationRevision, MediaRequest, MediaSeekCommand,
    };
    let (fixture, media) = toolbar_media_fixture(false);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    let popup = quick.component();
    let snapshot = toolbar_seek_midpoint_snapshot();
    media.finish_read(&fixture, snapshot.clone());
    assert!((popup.get_seek_progress() - 0.5).abs() < 0.0001);
    let slider = toolbar_seek_element(&popup);
    let position = native_center(&slider);
    // Adapt Slint's pinned touch-to-mouse synthesis, not a fresh mouse down:
    // first contact presses; second contact releases the primary grab.
    quick.observe_media_touch(11, TouchPhase::Started);
    popup.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    assert!(
        media.commands.lock().is_empty(),
        "touching the unchanged native midpoint thumb must not seek",
    );
    quick.observe_media_touch(22, TouchPhase::Started);
    popup.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    let mut replacement = snapshot;
    replacement
        .current
        .as_mut()
        .unwrap()
        .seek
        .as_mut()
        .unwrap()
        .revision = MediaObservationRevision::issue().unwrap();
    media.event(MediaEvent::SeekInvalidated);
    fixture.dock.invoke_media_event_ready();
    media.finish_read(&fixture, replacement.clone());
    let reads = media.reads.load(Ordering::SeqCst);
    // Ending the second contact promotes the still-held first contact
    // with a native press, not a new raw contact or mouse authority capture.
    quick.observe_media_touch(22, TouchPhase::Ended);
    popup.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    quick.finish_media_pointer_release();
    let moved = slint::LogicalPosition::new(position.x + slider.size().width * 0.25, position.y);
    quick.observe_media_touch(11, TouchPhase::Moved);
    popup
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: moved });
    flush_toolbar_seek_timer(&fixture, &quick);
    assert!(
        media.commands.lock().is_empty(),
        "promoted contact movement must not mint refreshed seek authority",
    );
    toolbar_seek_element(&popup).set_accessible_value("0.9");
    flush_toolbar_seek_timer(&fixture, &quick);
    assert!(
        media.commands.lock().is_empty(),
        "AX input must not become fresh while the first contact remains held",
    );
    // Unknown/duplicate terminal IDs have no native synthesized event.
    // Their post-dispatch fallback must not retire the real remaining ID.
    for (id, phase) in [(999, TouchPhase::Cancelled), (22, TouchPhase::Ended)] {
        quick.observe_media_touch(id, phase);
        quick.finish_media_pointer_release();
        toolbar_seek_element(&popup).set_accessible_value("0.8");
        flush_toolbar_seek_timer(&fixture, &quick);
        assert!(
            media.commands.lock().is_empty(),
            "unknown or duplicate touch end must not clear contact 11; id={id}",
        );
    }
    quick.observe_media_touch(11, TouchPhase::Ended);
    toolbar_seek_element(&popup).set_accessible_value("0.9");
    assert!(
        media.commands.lock().is_empty(),
        "all contacts ended is not fresh authority before native terminal dispatch",
    );
    popup.window().dispatch_event(WindowEvent::PointerReleased {
        position: moved,
        button: PointerEventButton::Left,
    });
    quick.finish_media_pointer_release();
    flush_toolbar_seek_timer(&fixture, &quick);
    assert!(
        media.commands.lock().is_empty(),
        "touch release must not flush a rejected seek intent",
    );
    assert_eq!(
        media.reads.load(Ordering::SeqCst),
        reads,
        "rejected touch input must not start a replacement observation",
    );
    toolbar_seek_element(&popup).set_accessible_value("0.75");
    flush_toolbar_seek_timer(&fixture, &quick);
    let session = replacement.current.as_ref().unwrap();
    let seek = session.seek.as_ref().unwrap();
    assert_eq!(
        media.commands.lock().as_slice(),
        &[MediaRequest::Seek(MediaSeekCommand {
            expected_session: session.key,
            expected_revision: seek.revision,
            observed_min_ticks: 100_000_000,
            observed_max_ticks: 1_300_000_000,
            position_ticks: 1_000_000_000,
        })],
        "fresh AX input after all contacts and terminal dispatch uses current authority",
    );
    assert!(fixture.host.saves.lock().is_empty());
    assert!(!fixture.dock.get_media_view().enabled);
}

#[test]
fn toolbar_current_player_actual_seek_popup_resize_cancels_latest_ax_without_replaying_accepted_seek()
 {
    use tessera_system::media::{MediaRequest, MediaSeekCommand};
    let (fixture, media) = toolbar_media_fixture(false);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    let popup = quick.component();
    let snapshot = toolbar_seek_snapshot();
    let session = snapshot.current.as_ref().unwrap();
    let seek = session.seek.as_ref().unwrap();
    let first = MediaRequest::Seek(MediaSeekCommand {
        expected_session: session.key,
        expected_revision: seek.revision,
        observed_min_ticks: seek.min_ticks,
        observed_max_ticks: seek.max_ticks,
        position_ticks: 700_000_000,
    });
    media.finish_read(&fixture, snapshot.clone());
    let slider = toolbar_seek_element(&popup);
    let original_position = popup.window().position();
    let original_size = popup.window().size();
    let original_scale = popup.window().scale_factor();
    let original_bounds = (slider.absolute_position(), slider.size());
    slider.set_accessible_value("0.5");
    slider.set_accessible_value("0.75");
    assert_eq!(media.commands.lock().as_slice(), &[first]);
    assert_eq!(media.pending_commands.lock().len(), 1);
    let reads = media.reads.load(Ordering::SeqCst);

    assert!(original_size.width > 40);
    let resized_size = slint::PhysicalSize::new(original_size.width - 40, original_size.height);
    // TestingWindow::set_size dispatches genuine WindowEvent::Resized to Slint
    // and updates the backend's physical size; set_position is unsupported.
    popup.window().set_size(resized_size);
    assert_eq!(popup.window().size(), resized_size);
    assert_ne!(popup.window().size(), original_size);
    assert_eq!(popup.window().position(), original_position);
    assert_eq!(popup.window().scale_factor(), original_scale);
    assert_ne!(
        (slider.absolute_position(), slider.size()),
        original_bounds,
        "the supported native resize must change measured Slider geometry before cancellation",
    );
    // Adapt native Resized classification only after the real frame changed.
    quick.observe_media_geometry_changed();
    assert!(quick.is_open() && quick.media_input_ready());
    assert_eq!(media.pending_commands.lock().len(), 1);
    assert_eq!(media.commands.lock().as_slice(), &[first]);
    assert!(media.pending_reads.lock().is_empty());
    assert_eq!(media.reads.load(Ordering::SeqCst), reads);

    media.finish_command(&fixture);
    assert_eq!(media.pending_reads.lock().len(), 1);
    media.finish_read(&fixture, snapshot);
    flush_toolbar_seek_timer(&fixture, &quick);
    assert_eq!(
        media.commands.lock().as_slice(),
        &[first],
        "actual popup resize cancels only unsubmitted latest AX input",
    );
    assert!(media.pending_commands.lock().is_empty());
    assert_eq!(popup.get_timeline_time().as_str(), "0:30 / 2:00 · observed");
    let reads = media.reads.load(Ordering::SeqCst);
    // A later fit may restore the old frame; cancellation must stay a tombstone.
    popup.window().set_size(original_size);
    assert_eq!(popup.window().size(), original_size);
    assert_eq!(popup.window().position(), original_position);
    assert_eq!(popup.window().scale_factor(), original_scale);
    assert_eq!(
        (slider.absolute_position(), slider.size()),
        original_bounds,
        "restore the actual captured Window and Slider frame before observing geometry",
    );
    quick.observe_media_geometry_changed();
    flush_toolbar_seek_timer(&fixture, &quick);
    assert_eq!(popup.window().position(), original_position);
    assert_eq!(
        media.commands.lock().as_slice(),
        &[first],
        "returning to the captured frame cannot resurrect pending input or replay accepted work",
    );
    assert_eq!(media.reads.load(Ordering::SeqCst), reads);
    assert!(fixture.host.saves.lock().is_empty());
    assert!(!fixture.dock.get_media_view().enabled);
}

#[test]
fn toolbar_current_player_actual_readback_layout_change_cancels_held_pointer_with_same_authority() {
    use slint::platform::WindowEvent;
    use tessera_system::media::{MediaEvent, MediaRequest, MediaSeekCommand};
    let (fixture, media) = toolbar_media_fixture(false);
    click_component(&fixture.toolbar, "Open quick settings");
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    let popup = quick.component();
    let snapshot = toolbar_seek_midpoint_snapshot();
    media.finish_read(&fixture, snapshot.clone());
    assert!(popup.get_seek_visible() && popup.get_seek_enabled());
    assert!((popup.get_seek_progress() - 0.5).abs() < 0.0001);
    let slider = toolbar_seek_element(&popup);
    let original_bounds = (slider.absolute_position(), slider.size());
    let original_size = popup.window().size();
    let position = native_center(&slider);
    // The unchanged native midpoint thumb captures input without a leading seek.
    dispatch_toolbar_seek_pointer(&quick, position, true);
    assert!(
        media.commands.lock().is_empty(),
        "unchanged midpoint down must not submit a native seek",
    );

    let mut replacement = snapshot.clone();
    replacement.current.as_mut().unwrap().title =
        "Measured layout title ".repeat(6).trim_end().to_owned();
    let original_session = snapshot.current.as_ref().unwrap();
    let replacement_session = replacement.current.as_ref().unwrap();
    assert_eq!(replacement_session.key, original_session.key);
    assert_eq!(replacement_session.seek, original_session.seek);
    assert_eq!(replacement_session.timeline, original_session.timeline);
    let reads = media.reads.load(Ordering::SeqCst);
    // Ordinary Changed/read feedback changes measured metadata, not authority.
    media.event(MediaEvent::Changed);
    fixture.dock.invoke_media_event_ready();
    assert_eq!(media.reads.load(Ordering::SeqCst), reads + 1);
    media.finish_read(&fixture, replacement.clone());
    let current_slider = toolbar_seek_element(&popup);
    assert_ne!(
        (current_slider.absolute_position(), current_slider.size()),
        original_bounds,
        "actual native Slider bounds must move after measured metadata wraps",
    );
    assert_ne!(
        popup.window().size().height,
        original_size.height,
        "production projection must refit the real popup height",
    );
    assert!(popup.get_media_view().current_present);
    assert_eq!(
        popup.get_media_view().title.as_str(),
        replacement_session.title.as_str(),
        "successful host read must project the metadata that changed actual layout",
    );
    assert!(quick.is_open() && quick.media_input_ready());
    assert!(popup.get_seek_visible() && popup.get_seek_enabled());
    assert!((popup.get_seek_progress() - 0.5).abs() < 0.0001);
    let reads = media.reads.load(Ordering::SeqCst);
    let moved = slint::LogicalPosition::new(position.x + slider.size().width * 0.25, position.y);
    popup
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: moved });
    flush_toolbar_seek_timer(&fixture, &quick);
    assert!(
        media.commands.lock().is_empty(),
        "genuine held movement cannot retarget a changed frame even with identical native authority",
    );
    current_slider.set_accessible_value("0.9");
    flush_toolbar_seek_timer(&fixture, &quick);
    assert!(
        media.commands.lock().is_empty(),
        "AX cannot mint fresh authority while the canceled pointer remains held",
    );
    dispatch_toolbar_seek_pointer(&quick, moved, false);
    flush_toolbar_seek_timer(&fixture, &quick);
    assert!(
        media.commands.lock().is_empty(),
        "native release cannot flush input canceled by an actual layout change",
    );
    assert_eq!(
        media.reads.load(Ordering::SeqCst),
        reads,
        "rejected old-frame input must not start native replacement reads",
    );
    toolbar_seek_element(&popup).set_accessible_value("0.75");
    flush_toolbar_seek_timer(&fixture, &quick);
    let session = replacement.current.as_ref().unwrap();
    let seek = session.seek.as_ref().unwrap();
    assert_eq!(
        media.commands.lock().as_slice(),
        &[MediaRequest::Seek(MediaSeekCommand {
            expected_session: session.key,
            expected_revision: seek.revision,
            observed_min_ticks: seek.min_ticks,
            observed_max_ticks: seek.max_ticks,
            position_ticks: 1_000_000_000,
        })],
        "fresh AX after release captures the valid current frame and unchanged typed authority",
    );
    assert!(fixture.host.saves.lock().is_empty());
    assert!(!fixture.dock.get_media_view().enabled);
}
