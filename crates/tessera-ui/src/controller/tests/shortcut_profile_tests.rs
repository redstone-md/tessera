// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use slint::language::KeyEvent;
use tessera_core::Rect;
use tessera_system::display_context::{
    DisplayContextCompletion, DisplayContextError, DisplayContextHost, DisplayLayout,
    DisplaySelection,
};
use tessera_system::profile::{
    ProfileCommand, ProfileError, ProfileHost, ProfileOpenCompletion, ProfilePhoto,
    ProfilePhotoState, ProfileReadCompletion, ProfileSnapshot, ProfileTarget,
};
use tessera_system::shortcuts::mocks::RecordingShortcutHost;
use tessera_system::shortcuts::{
    ShortcutAction, ShortcutCompletion, ShortcutConfig, ShortcutError, ShortcutEvent, ShortcutHost,
    ShortcutSink, ShortcutSubscription, ShortcutTrigger, TriggerPoint,
};

#[derive(Default)]
struct RootShortcuts {
    recording: RecordingShortcutHost,
    subscriptions: AtomicUsize,
}

impl ShortcutHost for RootShortcuts {
    fn configure(
        &self,
        config: ShortcutConfig,
        completion: ShortcutCompletion,
    ) -> Result<(), ShortcutError> {
        self.recording.configure(config, completion)
    }
    fn subscribe(
        &self,
        sink: ShortcutSink,
    ) -> Result<Box<dyn ShortcutSubscription>, ShortcutError> {
        self.subscriptions.fetch_add(1, Ordering::SeqCst);
        self.recording.subscribe(sink)
    }
}

fn install_shortcuts(fixture: &LauncherFixture) -> Arc<RootShortcuts> {
    let host = Arc::new(RootShortcuts::default());
    *fixture.host.shortcuts_provider.lock() = Some(host.clone());
    host
}

fn acknowledge(fixture: &LauncherFixture, host: &RootShortcuts) -> u64 {
    let completion = host
        .recording
        .take_next_completion()
        .expect("one accepted configuration");
    let generation = completion.generation();
    completion.complete_registered();
    fixture.panel.invoke_shortcuts_event_ready();
    generation
}

fn emit(
    fixture: &LauncherFixture,
    host: &RootShortcuts,
    generation: u64,
    action: ShortcutAction,
    cursor: Option<TriggerPoint>,
) {
    host.recording
        .emit(ShortcutEvent::Triggered(ShortcutTrigger {
            generation,
            action,
            cursor,
        }));
    fixture.panel.invoke_shortcuts_event_ready();
}

fn control_key(text: &str) -> KeyEvent {
    let mut event = KeyEvent::default();
    event.text = text.into();
    event.modifiers.control = true;
    event
}

#[test]
fn root_preferences_keep_all_groups_and_ordinary_options_enable_global_input() {
    assert!(crate::RunOptions::default().global_shortcuts_enabled);
    let shortcuts = ShortcutConfig::default().with_enabled(false);
    let saved = seeded_preferences()
        .with_source_seed(crate::SourceSeed::from_rgb(0x123456).unwrap())
        .with_general(
            crate::GeneralPreferences::default().with_start_of_week(crate::StartOfWeek::Sunday),
        )
        .with_media_enabled(true)
        .with_shortcuts(shortcuts.clone())
        .with_dock(crate::DockEdge::Right, vec!["saved-pin".into()])
        .with_launcher_favorites(vec!["saved-favorite".into()])
        .unwrap();
    assert_eq!(saved.shortcuts(), &shortcuts);
    for changed in [
        saved
            .clone()
            .with_appearance(crate::Theme::Dark, true, crate::DockEdge::Left),
        saved
            .clone()
            .with_launcher_display_mode(crate::LauncherDisplayMode::Fullscreen),
        saved.clone().with_media_enabled(false),
        saved
            .clone()
            .with_general(crate::GeneralPreferences::default()),
    ] {
        assert_eq!(changed.shortcuts(), &shortcuts);
        assert_eq!(changed.source_seed().rgb(), 0x123456);
    }
    let replaced = saved.clone().with_shortcuts(ShortcutConfig::default());
    assert_eq!(replaced.general(), saved.general());
    assert_eq!(replaced.source_seed(), saved.source_seed());
    assert_eq!(replaced.media_enabled(), saved.media_enabled());
    assert_eq!(replaced.launcher(), saved.launcher());
    assert_eq!(replaced.theme(), saved.theme());
    assert_eq!(replaced.compact(), saved.compact());
    assert_eq!(replaced.dock_edge(), saved.dock_edge());
    assert_eq!(replaced.pinned_apps(), saved.pinned_apps());
}

#[test]
fn root_construction_and_inert_startup_drafts_save_hide_never_acquire_global_input() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    assert_eq!(
        fixture.host.shortcuts_factory_calls.load(Ordering::SeqCst),
        0
    );
    fixture.controller.start_shortcuts(false);
    fixture.controller.open_panel();
    fixture.panel.invoke_shortcuts_capture_requested();
    assert!(
        fixture.panel.get_shortcuts_capturing(),
        "local capture needs no installed backend"
    );
    assert!(
        fixture
            .panel
            .invoke_shortcuts_capture_pressed(control_key("l"))
    );
    let mut released = KeyEvent::default();
    released.text = "l".into();
    assert!(fixture.panel.invoke_shortcuts_capture_released(released));
    fixture.panel.invoke_save_preferences_requested();
    fixture.panel.invoke_cancel_preferences_requested();
    fixture.panel.invoke_shortcuts_event_ready();
    fixture.controller.start_shortcuts(true); // one startup policy, never an inert-to-native upgrade
    assert_eq!(
        fixture.host.shortcuts_factory_calls.load(Ordering::SeqCst),
        0
    );
    assert_eq!(host.subscriptions.load(Ordering::SeqCst), 0);
    assert!(host.recording.configurations().is_empty());
    assert_eq!(fixture.host.profile_factory_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn root_global_bindings_survive_closed_preferences_and_use_existing_launcher_settings() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let display = Arc::new(PointDisplay::default());
    *fixture.host.display_provider.lock() = Some(display.clone());
    let point = TriggerPoint { x: -600, y: 100 };
    let selection = DisplaySelection::AtPoint {
        x: point.x,
        y: point.y,
    };
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.start_shortcuts(true);
    let generation = acknowledge(&fixture, &host);
    assert!(!fixture.panel.window().is_visible());
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(point),
    );
    display.finish(Ok(Some(point_layout(selection))));
    fixture.panel.invoke_shortcuts_event_ready();
    assert!(fixture.launcher.window().is_visible());
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(point),
    );
    assert!(!fixture.launcher.window().is_visible());
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::OpenSettings,
        Some(point),
    );
    display.finish(Ok(Some(point_layout(selection))));
    fixture.panel.invoke_shortcuts_event_ready();
    assert!(fixture.panel.window().is_visible());
    fixture.panel.invoke_cancel_preferences_requested();
    assert!(!fixture.panel.window().is_visible());
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(point),
    );
    display.finish(Ok(Some(point_layout(selection))));
    fixture.panel.invoke_shortcuts_event_ready();
    assert!(fixture.launcher.window().is_visible());
    assert_eq!(
        fixture.host.shortcuts_factory_calls.load(Ordering::SeqCst),
        1
    );
    assert_eq!(host.subscriptions.load(Ordering::SeqCst), 1);
    assert_eq!(host.recording.configurations().len(), 1);
    assert!(fixture.host.activations.lock().is_empty());
    assert!(fixture.host.launches.lock().is_empty());
    assert!(fixture.host.system_actions.lock().is_empty());
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn root_capture_waits_for_pause_ack_and_restores_saved_not_draft_until_complete_save() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.start_shortcuts(true);
    acknowledge(&fixture, &host);
    fixture.controller.open_panel();
    fixture.panel.invoke_shortcuts_capture_requested();
    assert!(!fixture.panel.get_shortcuts_capturing());
    assert!(!host.recording.configurations().last().unwrap().enabled());
    assert!(
        !fixture
            .panel
            .invoke_shortcuts_capture_pressed(control_key("l"))
    );
    acknowledge(&fixture, &host);
    assert!(fixture.panel.get_shortcuts_capturing());
    fixture
        .panel
        .invoke_shortcuts_capture_pressed(control_key("l"));
    let mut released = KeyEvent::default();
    released.text = "l".into();
    fixture.panel.invoke_shortcuts_capture_released(released);
    assert!(!fixture.panel.get_shortcuts_capturing());
    assert!(fixture.panel.get_shortcuts_dirty());
    assert_eq!(
        host.recording.configurations().last(),
        Some(&ShortcutConfig::default()),
        "resume cannot install an unsaved captured chord"
    );
    assert!(fixture.host.saves.lock().is_empty());
    acknowledge(&fixture, &host);
    let draft = fixture.controller.shortcut_draft();
    fixture.panel.invoke_save_preferences_requested();
    assert_eq!(
        fixture.host.saves.lock().last().unwrap().shortcuts(),
        &draft
    );
    assert_eq!(
        fixture.controller.core.applied_preferences().shortcuts(),
        &draft
    );
    assert_eq!(host.recording.configurations().last(), Some(&draft));
    acknowledge(&fixture, &host);
    assert!(!fixture.panel.get_shortcuts_dirty());
    assert_eq!(host.subscriptions.load(Ordering::SeqCst), 1);
}

#[test]
fn root_hide_while_pause_is_pending_never_begins_capture_and_coalesces_saved_resume() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.start_shortcuts(true);
    acknowledge(&fixture, &host);
    fixture.controller.open_panel();
    fixture.panel.invoke_shortcuts_capture_requested();
    let paused = host.recording.take_next_completion().unwrap();
    fixture.panel.invoke_cancel_preferences_requested();
    assert_eq!(
        host.recording.configurations().len(),
        2,
        "accepted pause keeps one native flight"
    );
    paused.complete_registered();
    fixture.panel.invoke_shortcuts_event_ready();
    assert!(!fixture.panel.get_shortcuts_capturing());
    assert_eq!(
        host.recording.configurations().last(),
        Some(&ShortcutConfig::default())
    );
    acknowledge(&fixture, &host);
    assert!(!fixture.panel.window().is_visible());
}

#[test]
fn root_failed_save_never_applies_and_retired_storage_completion_never_reinstalls() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let scope = Rc::new(RefCell::new(Some(shortcuts::RootCapabilityScope::new(
        &fixture.controller,
    ))));
    fixture.controller.start_shortcuts(true);
    acknowledge(&fixture, &host);
    fixture.controller.open_panel();
    fixture.panel.invoke_shortcuts_enabled_edited(false);
    *fixture.host.save_result.lock() = Err("recording storage denial".into());
    fixture.panel.invoke_save_preferences_requested();
    assert_eq!(host.recording.configurations().len(), 1);
    assert!(
        fixture
            .controller
            .core
            .applied_preferences()
            .shortcuts()
            .enabled()
    );
    *fixture.host.save_result.lock() = Ok(());
    let retired = scope.clone();
    PREFERENCE_SAVE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            retired.borrow_mut().take();
        }))
    });
    fixture.panel.invoke_save_preferences_requested();
    assert!(
        !fixture
            .controller
            .core
            .applied_preferences()
            .shortcuts()
            .enabled(),
        "the complete storage write really succeeded"
    );
    assert_eq!(
        host.recording.configurations().len(),
        1,
        "late storage cannot reactivate a retired Root"
    );
    assert!(!fixture.controller.root_current());
}

#[test]
fn root_shortcut_factory_reentry_cannot_subscribe_or_configure_retired_scope() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let scope = Rc::new(RefCell::new(Some(shortcuts::RootCapabilityScope::new(
        &fixture.controller,
    ))));
    let retired = scope.clone();
    SHORTCUT_FACTORY_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            retired.borrow_mut().take();
        }))
    });
    fixture.controller.start_shortcuts(true);
    assert_eq!(
        fixture.host.shortcuts_factory_calls.load(Ordering::SeqCst),
        1
    );
    assert_eq!(host.subscriptions.load(Ordering::SeqCst), 0);
    assert!(host.recording.configurations().is_empty());
    fixture.panel.invoke_shortcuts_event_ready();
    assert!(host.recording.configurations().is_empty());
}

#[derive(Default)]
struct PointDisplay {
    requested: Mutex<Vec<DisplaySelection>>,
    pending: Mutex<Vec<DisplayContextCompletion>>,
}
impl DisplayContextHost for PointDisplay {
    fn read(&self, _: DisplayContextCompletion) -> Result<(), DisplayContextError> {
        Err(DisplayContextError::Unsupported)
    }
    fn read_selected(
        &self,
        selection: DisplaySelection,
        completion: DisplayContextCompletion,
    ) -> Result<(), DisplayContextError> {
        self.requested.lock().push(selection);
        self.pending.lock().push(completion);
        Ok(())
    }
}
impl PointDisplay {
    fn finish(&self, result: Result<Option<DisplayLayout>, DisplayContextError>) {
        let completion = self.pending.lock().remove(0);
        completion(result);
    }
}
fn point_layout(selection: DisplaySelection) -> DisplayLayout {
    DisplayLayout::new(
        Rect::new(-1200, 0, 3120, 1080).unwrap(),
        Rect::new(-1200, 0, 1200, 900).unwrap(),
        1.5,
        selection,
    )
    .unwrap()
}

#[test]
fn root_shortcut_cursor_targets_genuine_monitor_without_moving_bars_or_changing_source() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let display = Arc::new(PointDisplay::default());
    *fixture.host.display_provider.lock() = Some(display.clone());
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    let source = fixture.controller.core.dock_context();
    let bars = [SurfaceKind::Dock, SurfaceKind::Toolbar].map(|kind| {
        fixture
            .controller
            .leases
            .borrow()
            .attachments
            .get(&kind)
            .unwrap()
            .rect
    });
    fixture.controller.start_shortcuts(true);
    let generation = acknowledge(&fixture, &host);
    let point = TriggerPoint { x: -600, y: 100 };
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(point),
    );
    assert!(!fixture.launcher.window().is_visible());
    let selection = DisplaySelection::AtPoint {
        x: point.x,
        y: point.y,
    };
    assert_eq!(*display.requested.lock(), [selection]);
    display.finish(Ok(Some(point_layout(selection))));
    fixture.panel.invoke_shortcuts_event_ready();
    assert!(fixture.launcher.window().is_visible());
    let rect = fixture.attached_launcher_rect().unwrap();
    assert!(rect.0 >= -1200 && i64::from(rect.0) + i64::from(rect.2) <= 0);
    assert!(rect.1 >= 0 && i64::from(rect.1) + i64::from(rect.3) <= 900);
    assert_eq!(fixture.controller.core.dock_context(), source);
    assert_eq!(
        [SurfaceKind::Dock, SurfaceKind::Toolbar].map(|kind| fixture
            .controller
            .leases
            .borrow()
            .attachments
            .get(&kind)
            .unwrap()
            .rect),
        bars
    );
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn root_shortcut_monitor_no_match_or_legacy_fallback_never_fabricates_primary() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let display = Arc::new(PointDisplay::default());
    *fixture.host.display_provider.lock() = Some(display.clone());
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.start_shortcuts(true);
    let generation = acknowledge(&fixture, &host);
    for result in [
        Ok(None),
        Ok(Some(point_layout(DisplaySelection::Primary))),
        Err(DisplayContextError::Unsupported),
    ] {
        emit(
            &fixture,
            &host,
            generation,
            ShortcutAction::ToggleLauncher,
            Some(TriggerPoint { x: -600, y: 100 }),
        );
        display.finish(result);
        fixture.panel.invoke_shortcuts_event_ready();
        assert!(!fixture.launcher.window().is_visible());
        assert!(!fixture.panel.window().is_visible());
    }
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        None,
    );
    assert_eq!(
        display.requested.lock().len(),
        3,
        "unknown cursor cannot request primary fallback"
    );
    assert!(!fixture.launcher.window().is_visible());
}

#[test]
fn root_saved_disabled_bindings_acknowledge_paused_and_never_deliver_global_actions() {
    let fixture = LauncherFixture::with_preferences(
        seeded_preferences().with_shortcuts(ShortcutConfig::default().with_enabled(false)),
    );
    let host = install_shortcuts(&fixture);
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.start_shortcuts(true);
    assert!(!host.recording.configurations()[0].enabled());
    let generation = acknowledge(&fixture, &host);
    assert!(!fixture.panel.get_shortcuts_enabled());
    assert!(
        fixture
            .panel
            .get_shortcuts_launcher_status()
            .contains("Paused")
    );
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(TriggerPoint { x: 10, y: 10 }),
    );
    assert!(!fixture.launcher.window().is_visible());
    assert_eq!(
        fixture.host.display_provider_calls.load(Ordering::SeqCst),
        0
    );
}

#[test]
fn root_weak_completion_cannot_replace_a_new_root_using_the_same_provider() {
    let old = LauncherFixture::new();
    let host = install_shortcuts(&old);
    let scope = shortcuts::RootCapabilityScope::new(&old.controller);
    old.controller.start_shortcuts(true);
    let late = host.recording.take_next_completion().unwrap();
    let weak = old.panel.as_weak();
    drop(scope);
    drop(old);
    assert!(weak.upgrade().is_none());
    let new = LauncherFixture::new();
    *new.host.shortcuts_provider.lock() = Some(host.clone());
    let _scope = shortcuts::RootCapabilityScope::new(&new.controller);
    new.controller.start_shortcuts(true);
    let generation = acknowledge(&new, &host);
    let status = new.panel.get_shortcuts_launcher_status();
    late.complete_registered();
    new.panel.invoke_shortcuts_event_ready();
    assert_eq!(new.panel.get_shortcuts_launcher_status(), status);
    emit(
        &new,
        &host,
        generation - 1,
        ShortcutAction::OpenSettings,
        Some(TriggerPoint { x: 10, y: 10 }),
    );
    assert!(!new.panel.window().is_visible());
    assert_eq!(new.host.display_provider_calls.load(Ordering::SeqCst), 0);
    assert_eq!(host.subscriptions.load(Ordering::SeqCst), 2);
}

#[test]
fn root_late_monitor_completion_loses_to_new_saved_generation_and_root_retirement() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let display = Arc::new(PointDisplay::default());
    *fixture.host.display_provider.lock() = Some(display.clone());
    let scope = Rc::new(RefCell::new(Some(shortcuts::RootCapabilityScope::new(
        &fixture.controller,
    ))));
    fixture.controller.start_shortcuts(true);
    let generation = acknowledge(&fixture, &host);
    let selection = DisplaySelection::AtPoint { x: -600, y: 100 };
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(TriggerPoint { x: -600, y: 100 }),
    );
    fixture.controller.open_panel();
    fixture.panel.invoke_shortcuts_enabled_edited(false);
    fixture.panel.invoke_save_preferences_requested();
    acknowledge(&fixture, &host);
    display.finish(Ok(Some(point_layout(selection))));
    fixture.panel.invoke_shortcuts_event_ready();
    assert!(!fixture.launcher.window().is_visible());
    fixture.panel.invoke_shortcuts_enabled_edited(true);
    fixture.panel.invoke_save_preferences_requested();
    let generation = acknowledge(&fixture, &host);
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(TriggerPoint { x: -600, y: 100 }),
    );
    scope.borrow_mut().take();
    display.finish(Ok(Some(point_layout(selection))));
    fixture.panel.invoke_shortcuts_event_ready();
    assert!(!fixture.launcher.window().is_visible());
}

#[test]
fn root_point_result_rechecks_source_dpi_and_undrained_native_fault_before_opening() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let display = Arc::new(PointDisplay::default());
    *fixture.host.display_provider.lock() = Some(display.clone());
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.start_shortcuts(true);
    let generation = acknowledge(&fixture, &host);
    let point = TriggerPoint { x: -600, y: 100 };
    let selection = DisplaySelection::AtPoint {
        x: point.x,
        y: point.y,
    };
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(point),
    );
    fixture
        .dock
        .window()
        .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
    display.finish(Ok(Some(point_layout(selection))));
    fixture.controller.process_shortcut_display();
    assert!(!fixture.launcher.window().is_visible());
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(point),
    );
    host.recording
        .emit(ShortcutEvent::Unavailable(ShortcutError::new(
            tessera_system::shortcuts::ShortcutErrorKind::Other,
            None,
            "recording backend failure",
        )));
    display.finish(Ok(Some(point_layout(selection))));
    // Intentionally process only display: the actor's fault has not been drained.
    fixture.controller.process_shortcut_display();
    assert!(!fixture.launcher.window().is_visible());
    fixture.panel.invoke_shortcuts_event_ready();
}

#[test]
fn root_point_factory_reentry_rejects_returned_provider_before_any_monitor_read() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let display = Arc::new(PointDisplay::default());
    *fixture.host.display_provider.lock() = Some(display.clone());
    let scope = Rc::new(RefCell::new(Some(shortcuts::RootCapabilityScope::new(
        &fixture.controller,
    ))));
    fixture.controller.start_shortcuts(true);
    let generation = acknowledge(&fixture, &host);
    let retired = scope.clone();
    POWER_DISPLAY_FACTORY_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            retired.borrow_mut().take();
        }))
    });
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(TriggerPoint { x: -600, y: 100 }),
    );
    assert!(display.requested.lock().is_empty());
    assert!(!fixture.launcher.window().is_visible());
}

#[derive(Default)]
pub(super) struct RootProfile {
    reads: AtomicUsize,
    pending: Mutex<Vec<ProfileReadCompletion>>,
    pub(super) opened: Mutex<Vec<&'static str>>,
}
impl ProfileHost for RootProfile {
    fn read(&self, completion: ProfileReadCompletion) -> Result<(), ProfileError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.pending.lock().push(completion);
        Ok(())
    }
    fn execute(
        &self,
        command: ProfileCommand,
        completion: ProfileOpenCompletion,
    ) -> Result<(), ProfileError> {
        let label = match command {
            ProfileCommand::OpenHome => "home",
            ProfileCommand::OpenAccountsSettings => "accounts",
            ProfileCommand::OpenOneDrive { expected } => {
                assert_eq!(expected.downcast_ref::<u64>(), Some(&42));
                "onedrive"
            }
        };
        self.opened.lock().push(label);
        completion(Ok(()));
        Ok(())
    }
}
impl RootProfile {
    pub(super) fn finish(&self) {
        let completion = self.pending.lock().remove(0);
        completion(Ok(ProfileSnapshot::new(
            "Recording user".into(),
            ProfilePhotoState::Ready(ProfilePhoto::new(1, 1, vec![200, 50, 20, 255]).unwrap()),
            Some("recording@example.invalid".into()),
            Some(ProfileTarget::new(42_u64)),
        )
        .unwrap()));
    }
}

#[derive(Default)]
pub(super) struct RootFolders {
    pub(super) opened: Mutex<Vec<tessera_system::folders::FolderId>>,
}
impl tessera_system::folders::FolderHost for RootFolders {
    fn read(
        &self,
        completion: tessera_system::folders::FolderReadCompletion,
    ) -> Result<(), tessera_system::folders::FolderError> {
        use tessera_system::folders::{FolderAvailability, FolderId, FolderSnapshot, FolderTarget};
        completion(Ok(FolderSnapshot::new(std::array::from_fn(|index| {
            FolderAvailability::Ready(FolderTarget::new(FolderId::ALL[index]))
        }))));
        Ok(())
    }
    fn open(
        &self,
        folder: tessera_system::folders::FolderId,
        target: tessera_system::folders::FolderTarget,
        completion: tessera_system::folders::FolderOpenCompletion,
    ) -> Result<(), tessera_system::folders::FolderError> {
        assert_eq!(
            target.get::<tessera_system::folders::FolderId>(),
            Some(&folder)
        );
        self.opened.lock().push(folder);
        completion(Ok(()));
        Ok(())
    }
}

#[test]
fn root_profile_is_lazy_current_user_only_and_closed_typed_actions_never_persist_identity() {
    let fixture = LauncherFixture::new();
    let host = Arc::new(RootProfile::default());
    *fixture.host.profile_provider.lock() = Some(host.clone());
    let folders = Arc::new(RootFolders::default());
    *fixture.host.folder_provider.lock() = Some(folders.clone());
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    assert_eq!(fixture.host.profile_factory_calls.load(Ordering::SeqCst), 0);
    fixture.controller.open_launcher();
    fixture.click_launcher("Open user menu");
    let user = fixture.controller.user_menu.borrow().clone().unwrap();
    assert_eq!(fixture.host.profile_factory_calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.reads.load(Ordering::SeqCst), 1);
    assert!(user.component().get_profile_loading());
    user.component().invoke_folder_event_ready();
    assert_eq!(
        user.component().get_rows().row_count(),
        7,
        "profile flight does not rewrite folders"
    );
    let folder_keys: Vec<_> = user
        .component()
        .get_rows()
        .iter()
        .map(|row| row.key)
        .collect();
    click_component(user.component(), "Open Desktop");
    assert_eq!(
        *folders.opened.lock(),
        [tessera_system::folders::FolderId::Desktop]
    );
    user.component().invoke_folder_event_ready();
    assert!(
        user.component().get_profile_loading(),
        "folder open does not finish a profile read"
    );
    host.finish();
    user.component().invoke_profile_event_ready();
    assert_eq!(user.component().get_profile_name(), "Recording user");
    assert!(user.component().get_has_photo());
    assert_eq!(
        user.component().get_personal_email(),
        "recording@example.invalid"
    );
    assert_eq!(
        user.component()
            .get_rows()
            .iter()
            .map(|row| row.key)
            .collect::<Vec<_>>(),
        folder_keys
    );
    let key = user.component().get_profile_key();
    user.component()
        .invoke_profile_open_requested(crate::generated::UserProfileAction::Home, key);
    user.component().invoke_profile_event_ready();
    let key = user.component().get_profile_key();
    user.component()
        .invoke_profile_open_requested(crate::generated::UserProfileAction::Accounts, key);
    user.component().invoke_profile_event_ready();
    let key = user.component().get_onedrive_key();
    user.component().invoke_onedrive_open_requested(key.clone());
    user.component().invoke_profile_event_ready();
    assert_eq!(*host.opened.lock(), ["home", "accounts", "onedrive"]);
    fixture.controller.hide_launcher();
    user.component().invoke_onedrive_open_requested(key);
    assert_eq!(host.opened.lock().len(), 3);
    assert!(fixture.host.saves.lock().is_empty());
    assert!(fixture.host.system_actions.lock().is_empty());
    assert_eq!(
        fixture.host.shortcuts_factory_calls.load(Ordering::SeqCst),
        0
    );
}

#[test]
fn root_profile_factory_source_reentry_and_late_read_cannot_resurrect_old_user_scope() {
    let fixture = LauncherFixture::new();
    let host = Arc::new(RootProfile::default());
    *fixture.host.profile_provider.lock() = Some(host.clone());
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.open_launcher();
    let root = fixture.controller.clone();
    PROFILE_FACTORY_HOOK
        .with(|hook| *hook.borrow_mut() = Some(Box::new(move || root.hide_launcher())));
    fixture.click_launcher("Open user menu");
    assert_eq!(host.reads.load(Ordering::SeqCst), 0);
    let user = fixture.controller.user_menu.borrow().clone().unwrap();
    assert!(!user.is_open());
    fixture.controller.open_launcher();
    fixture.click_launcher("Open user menu");
    assert_eq!(host.reads.load(Ordering::SeqCst), 1);
    fixture.controller.hide_launcher();
    fixture.controller.open_launcher();
    fixture.click_launcher("Open user menu");
    assert_eq!(
        host.reads.load(Ordering::SeqCst),
        1,
        "accepted old read persists across Hide/reopen"
    );
    host.finish();
    user.component().invoke_profile_event_ready();
    assert_eq!(
        host.reads.load(Ordering::SeqCst),
        2,
        "new current session alone issues the follow-up"
    );
    assert!(user.component().get_profile_name().is_empty());
    host.finish();
    user.component().invoke_profile_event_ready();
    assert_eq!(user.component().get_profile_name(), "Recording user");
    assert!(host.opened.lock().is_empty());
}

#[test]
fn newer_settings_intent_during_launcher_native_attach_retires_original_lease_before_hide() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let display = Arc::new(PointDisplay::default());
    *fixture.host.display_provider.lock() = Some(display.clone());
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.start_shortcuts(true);
    let generation = acknowledge(&fixture, &host);
    let point = TriggerPoint { x: -600, y: 100 };
    let selection = DisplaySelection::AtPoint {
        x: point.x,
        y: point.y,
    };
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(point),
    );
    let root = fixture.controller.clone();
    let panel = fixture.panel.clone_strong();
    let newer = host.clone();
    let next_display = display.clone();
    LAUNCHER_CONFIGURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |_| {
            newer
                .recording
                .emit(ShortcutEvent::Triggered(ShortcutTrigger {
                    generation,
                    action: ShortcutAction::OpenSettings,
                    cursor: Some(point),
                }));
            panel.invoke_shortcuts_event_ready();
            next_display.finish(Ok(Some(point_layout(selection))));
            root.process_shortcut_display();
        }));
    });
    let root = fixture.controller.clone();
    let launcher = fixture.launcher.clone_strong();
    let panel = fixture.panel.clone_strong();
    let drops = Rc::new(Cell::new(0));
    let observed = drops.clone();
    LAUNCHER_DROP_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            observed.set(observed.get() + 1);
            assert!(
                launcher.window().is_visible(),
                "late attachment must retire before native hide"
            );
            assert!(
                panel.window().is_visible(),
                "new Settings presentation wins"
            );
            assert!(
                !root
                    .leases
                    .borrow()
                    .attachments
                    .contains_key(&SurfaceKind::Launcher)
            );
        }));
    });
    let focus_before = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    display.finish(Ok(Some(point_layout(selection))));
    fixture.controller.process_shortcut_display();
    assert_eq!(drops.get(), 1);
    assert!(fixture.panel.window().is_visible());
    assert!(!fixture.launcher.window().is_visible());
    assert!(fixture.attached_launcher_rect().is_none());
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_before + 1,
        "old launcher cannot take focus after newer Settings"
    );
}

#[test]
fn root_dpi_change_inside_launcher_attach_revokes_display_handoff_until_completion() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let display = Arc::new(PointDisplay::default());
    *fixture.host.display_provider.lock() = Some(display.clone());
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.start_shortcuts(true);
    let generation = acknowledge(&fixture, &host);
    let point = TriggerPoint { x: -600, y: 100 };
    let selection = DisplaySelection::AtPoint {
        x: point.x,
        y: point.y,
    };
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(point),
    );
    let dock = fixture.dock.clone_strong();
    LAUNCHER_CONFIGURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |_| {
            dock.window()
                .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                    scale_factor: 2.0,
                });
        }));
    });
    let focus_before = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let drops_before = fixture.host.lease_drops.load(Ordering::SeqCst);
    display.finish(Ok(Some(point_layout(selection))));
    fixture.controller.process_shortcut_display();
    assert!(!fixture.launcher.window().is_visible());
    assert!(fixture.attached_launcher_rect().is_none());
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_before
    );
    assert_eq!(
        fixture.host.lease_drops.load(Ordering::SeqCst),
        drops_before + 1
    );
    fixture.controller.open_launcher();
    assert!(
        fixture.launcher.window().is_visible(),
        "ordinary manual opens remain independent"
    );
}

#[test]
fn root_bounds_change_inside_launcher_attach_revokes_handoff_without_replaying_refit() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let display = Arc::new(PointDisplay::default());
    *fixture.host.display_provider.lock() = Some(display.clone());
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.start_shortcuts(true);
    let generation = acknowledge(&fixture, &host);
    let point = TriggerPoint { x: -600, y: 100 };
    let selection = DisplaySelection::AtPoint {
        x: point.x,
        y: point.y,
    };
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(point),
    );
    let root = fixture.controller.clone();
    let panel = fixture.panel.clone_strong();
    let changed = crate::DockContext::new(64, 32, 1600, 900, false).unwrap();
    LAUNCHER_CONFIGURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |_| {
            apply_result_to_both(
                &root,
                &panel,
                Ok(launcher_snapshot().with_dock_context(changed)),
            );
        }));
    });
    let focus_before = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    display.finish(Ok(Some(point_layout(selection))));
    fixture.controller.process_shortcut_display();
    assert_eq!(fixture.controller.core.dock_context(), Some(changed));
    assert!(fixture.attached_launcher_rect().is_none());
    assert!(!fixture.launcher.window().is_visible());
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_before
    );
}

#[test]
fn newer_launcher_intent_keeps_its_monitor_and_scope_across_deferred_native_reopen() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let display = Arc::new(PointDisplay::default());
    *fixture.host.display_provider.lock() = Some(display.clone());
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.start_shortcuts(true);
    let generation = acknowledge(&fixture, &host);
    let point = TriggerPoint { x: -600, y: 100 };
    let selection = DisplaySelection::AtPoint {
        x: point.x,
        y: point.y,
    };
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(point),
    );
    let root = fixture.controller.clone();
    let panel = fixture.panel.clone_strong();
    let newer = host.clone();
    let next_display = display.clone();
    LAUNCHER_CONFIGURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |_| {
            // This is the existing serialized hide: native hide waits for the late lease.
            root.hide_launcher();
            newer
                .recording
                .emit(ShortcutEvent::Triggered(ShortcutTrigger {
                    generation,
                    action: ShortcutAction::ToggleLauncher,
                    cursor: Some(point),
                }));
            panel.invoke_shortcuts_event_ready();
            next_display.finish(Ok(Some(point_layout(selection))));
            root.process_shortcut_display();
        }));
    });
    let focus_before = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let attach_before = fixture.host.launcher_attach_calls.load(Ordering::SeqCst);
    let drops_before = fixture.host.lease_drops.load(Ordering::SeqCst);
    display.finish(Ok(Some(point_layout(selection))));
    fixture.controller.process_shortcut_display();
    assert!(fixture.launcher.window().is_visible());
    let rect = fixture
        .attached_launcher_rect()
        .expect("only the newer lease is published");
    assert!(
        rect.0 < 0,
        "queued native intent retains its containing cursor monitor"
    );
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_before + 1
    );
    assert_eq!(
        fixture.host.launcher_attach_calls.load(Ordering::SeqCst),
        attach_before + 2
    );
    assert_eq!(
        fixture.host.lease_drops.load(Ordering::SeqCst),
        drops_before + 1
    );
}

#[test]
fn late_native_lease_drop_can_revoke_a_queued_shortcut_reopen_without_focus_or_replay() {
    let fixture = LauncherFixture::new();
    let host = install_shortcuts(&fixture);
    let display = Arc::new(PointDisplay::default());
    *fixture.host.display_provider.lock() = Some(display.clone());
    let _scope = shortcuts::RootCapabilityScope::new(&fixture.controller);
    fixture.controller.start_shortcuts(true);
    let generation = acknowledge(&fixture, &host);
    let point = TriggerPoint { x: -600, y: 100 };
    let selection = DisplaySelection::AtPoint {
        x: point.x,
        y: point.y,
    };
    emit(
        &fixture,
        &host,
        generation,
        ShortcutAction::ToggleLauncher,
        Some(point),
    );
    let root = fixture.controller.clone();
    let panel = fixture.panel.clone_strong();
    let newer = host.clone();
    let next_display = display.clone();
    LAUNCHER_CONFIGURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |_| {
            root.hide_launcher();
            newer
                .recording
                .emit(ShortcutEvent::Triggered(ShortcutTrigger {
                    generation,
                    action: ShortcutAction::ToggleLauncher,
                    cursor: Some(point),
                }));
            panel.invoke_shortcuts_event_ready();
            next_display.finish(Ok(Some(point_layout(selection))));
            root.process_shortcut_display();
        }));
    });
    let dock = fixture.dock.clone_strong();
    LAUNCHER_DROP_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            dock.window()
                .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                    scale_factor: 2.0,
                });
        }));
    });
    let focus_before = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let attach_before = fixture.host.launcher_attach_calls.load(Ordering::SeqCst);
    display.finish(Ok(Some(point_layout(selection))));
    fixture.controller.process_shortcut_display();
    assert!(!fixture.launcher.window().is_visible());
    assert!(
        !fixture.controller.launcher_visible(),
        "rejected queued intent cannot stay logically open"
    );
    assert!(fixture.attached_launcher_rect().is_none());
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_before
    );
    assert_eq!(
        fixture.host.launcher_attach_calls.load(Ordering::SeqCst),
        attach_before + 1
    );
}
