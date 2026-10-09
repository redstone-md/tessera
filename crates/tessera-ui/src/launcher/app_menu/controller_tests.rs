// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Included under controller::tests to exercise the existing LauncherFixture,
//! real callback routing and complete-record preference transaction.

use super::*;
use crate::generated::{DockMenuAction, LauncherView};
use crate::launcher::LauncherAppMenu;
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::ComponentHandle;
use slint::platform::{Key, PointerEventButton, WindowEvent};

fn right_click(component: &impl ComponentHandle, label: &str) {
    let mut buttons = ElementHandle::find_by_accessible_label(component, label)
        .filter(|element| element.accessible_role() == Some(AccessibleRole::Button));
    let button = buttons.next().expect("real source button");
    assert!(buttons.next().is_none(), "source button is unique");
    let origin = button.absolute_position();
    let size = button.size();
    assert!(size.width > 0.0 && size.height > 0.0);
    let position =
        slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    component
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    component
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Right,
        });
    component
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Right,
        });
}

fn menu(fixture: &LauncherFixture) -> Rc<LauncherAppMenu> {
    fixture
        .controller
        .launcher_app_menu
        .borrow()
        .clone()
        .expect("genuine application menu")
}

fn open_editor(fixture: &LauncherFixture) -> Rc<LauncherAppMenu> {
    right_click(&fixture.launcher, "Launch Rust Editor");
    let menu = menu(fixture);
    assert!(menu.is_open());
    menu
}

fn key(component: &impl ComponentHandle, value: Key) {
    let text: slint::SharedString = value.into();
    component
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    component
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text });
}

fn no_unrelated_effects(
    fixture: &LauncherFixture,
    observations: usize,
    subscriptions: usize,
    recycle: (usize, usize),
) {
    assert_eq!(
        fixture.host.observe_calls.load(Ordering::SeqCst),
        observations
    );
    assert_eq!(
        fixture.host.subscription_calls.load(Ordering::SeqCst),
        subscriptions
    );
    assert!(fixture.host.launches.lock().is_empty());
    assert!(fixture.host.activations.lock().is_empty());
    assert!(fixture.host.window_actions.lock().is_empty());
    assert!(fixture.host.system_actions.lock().is_empty());
    assert_eq!(
        fixture
            .host
            .dock_utility_provider_calls
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        fixture.host.recycle_provider_calls.load(Ordering::SeqCst),
        recycle.0
    );
    assert_eq!(
        fixture
            .host
            .recycle_mutation_provider_calls
            .load(Ordering::SeqCst),
        recycle.1
    );
    assert_eq!(
        fixture.host.display_provider_calls.load(Ordering::SeqCst),
        0
    );
    assert_eq!(fixture.host.power_provider_calls.load(Ordering::SeqCst), 0);
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
}

#[test]
fn native_pin_detaches_before_one_complete_applied_record_save_without_selecting_or_launching() {
    let preferences = PanelPreferences::new(Theme::Light, false)
        .with_dock(crate::DockEdge::Left, vec!["app-browser".into()])
        .with_launcher_display_mode(crate::LauncherDisplayMode::Fullscreen)
        .with_launcher_favorites(vec!["uninstalled".into()])
        .unwrap();
    let fixture = LauncherFixture::with_preferences(preferences.clone());
    fixture.controller.open_launcher();
    fixture.click_launcher("All Apps");
    fixture.panel.set_theme_index(2);
    fixture.panel.set_compact(true);
    fixture.panel.set_dock_edge_index(3);
    let observations = fixture.host.observe_calls.load(Ordering::SeqCst);
    let subscriptions = fixture.host.subscription_calls.load(Ordering::SeqCst);
    let recycle = (
        fixture.host.recycle_provider_calls.load(Ordering::SeqCst),
        fixture
            .host
            .recycle_mutation_provider_calls
            .load(Ordering::SeqCst),
    );
    assert_eq!(fixture.launcher.get_selected_key(), "");
    let popup = open_editor(&fixture);
    assert_eq!(
        fixture.launcher.get_selected_key(),
        "",
        "context opening is not selection"
    );
    assert!(fixture.host.saves.lock().is_empty());
    let drops = fixture.host.lease_drops.load(Ordering::SeqCst);
    let captured_popup = popup.clone();
    let host = fixture.host.clone();
    PREFERENCE_SAVE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(!captured_popup.is_open());
            assert!(!captured_popup.component().window().is_visible());
            assert!(
                host.lease_drops.load(Ordering::SeqCst) > drops,
                "native lease detached before save"
            );
            assert_eq!(host.saves.lock().len(), 1);
        }))
    });
    click_component(popup.component(), "Pin");
    let expected = preferences
        .with_launcher_favorites(vec!["uninstalled".into(), "app-editor".into()])
        .unwrap();
    assert_eq!(fixture.controller.core.applied_preferences(), expected);
    assert_eq!(&*fixture.host.saves.lock(), &[expected]);
    assert_eq!(fixture.controller.core.pins(), ["app-browser"]);
    assert_eq!(
        fixture.launcher.get_display_mode(),
        crate::generated::LauncherDisplayMode::Fullscreen
    );
    popup
        .component()
        .invoke_action_requested(DockMenuAction::FavoriteAdd);
    fixture
        .launcher
        .invoke_favorite_toggle_requested("app-editor".into(), true);
    assert_eq!(
        fixture.host.saves.lock().len(),
        1,
        "captured desired Pin is idempotent"
    );
    no_unrelated_effects(&fixture, observations, subscriptions, recycle);
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn native_unpin_failure_preserves_favorite_order_query_selection_and_applied_record() {
    let preferences = seeded_preferences()
        .with_launcher_display_mode(crate::LauncherDisplayMode::Fullscreen)
        .with_launcher_favorites(vec![
            "uninstalled".into(),
            "app-editor".into(),
            "app-browser".into(),
        ])
        .unwrap();
    let fixture = LauncherFixture::with_preferences(preferences.clone());
    fixture.controller.open_launcher();
    fixture.launcher.set_search("editor".into());
    fixture.controller.apply_launcher_filter();
    assert_eq!(fixture.launcher.get_selected_key(), "app-editor");
    let popup = open_editor(&fixture);
    let rows = fixture.launcher.get_rows();
    let view = fixture.launcher.get_view();
    let count = fixture.launcher.get_application_count();
    let mode = fixture.launcher.get_display_mode();
    *fixture.host.save_result.lock() = Err("disk full\n\u{1b}private".into());
    key(popup.component(), Key::Return);
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    assert_eq!(
        fixture.controller.core.launcher_favorites(),
        ["uninstalled", "app-editor", "app-browser"]
    );
    assert_eq!(fixture.launcher.get_rows(), rows);
    assert_eq!(fixture.launcher.get_view(), view);
    assert_eq!(fixture.launcher.get_application_count(), count);
    assert_eq!(fixture.launcher.get_display_mode(), mode);
    assert_eq!(fixture.launcher.get_selected_key(), "app-editor");
    assert_eq!(fixture.launcher.get_search(), "editor");
    assert!(fixture.tile(0).favorite);
    assert!(
        fixture
            .launcher
            .get_status()
            .contains("Could not save favorites")
    );
    assert!(!fixture.launcher.get_status().contains('\n'));
    assert!(!fixture.launcher.get_status().contains('\u{1b}'));
    popup
        .component()
        .invoke_action_requested(DockMenuAction::FavoriteRemove);
    assert_eq!(
        fixture.host.saves.lock().len(),
        1,
        "failed closed scope cannot replay"
    );
}

#[test]
fn native_unpin_in_favorites_removes_only_launcher_membership_and_returns_search_focus() {
    let preferences = seeded_preferences()
        .with_dock(crate::DockEdge::Bottom, vec!["app-editor".into()])
        .with_launcher_favorites(vec!["uninstalled".into(), "app-editor".into()])
        .unwrap();
    let fixture = LauncherFixture::with_preferences(preferences.clone());
    fixture.controller.open_launcher();
    fixture
        .launcher
        .invoke_select_requested("app-editor".into());
    let popup = open_editor(&fixture);
    key(popup.component(), Key::Space);
    let expected = preferences
        .with_launcher_favorites(vec!["uninstalled".into()])
        .unwrap();
    assert_eq!(fixture.controller.core.applied_preferences(), expected);
    assert_eq!(&*fixture.host.saves.lock(), &[expected]);
    assert_eq!(fixture.controller.core.pins(), ["app-editor"]);
    assert_eq!(fixture.launcher.get_view(), LauncherView::Favorites);
    assert_eq!(fixture.launcher.get_application_count(), 0);
    assert_eq!(fixture.launcher.get_selected_key(), "");
    assert_eq!(fixture.launcher.get_search(), "");
    // Native text reaches the real search field after the selected row expires.
    fixture.key("r".into());
    assert_eq!(fixture.launcher.get_search(), "r");
    assert!(fixture.host.launches.lock().is_empty());
}

#[test]
fn native_filtered_application_past_512_pins_exact_key_without_eager_full_catalog_rows() {
    let applications = (0..1024)
        .map(|index| {
            PanelApplication::new(
                format!("app-{index:04}"),
                format!("Retained App {index}"),
                None,
            )
            .unwrap()
        })
        .collect();
    let preferences = seeded_preferences()
        .with_launcher_favorites(vec!["uninstalled".into()])
        .unwrap();
    let fixture = LauncherFixture::with_snapshot(
        preferences.clone(),
        launcher_snapshot().with_applications(applications),
    );
    fixture.controller.open_launcher();
    fixture.launcher.set_search("1023".into());
    fixture.controller.apply_launcher_filter();
    assert_eq!(fixture.launcher.get_application_count(), 1);
    assert_eq!(fixture.launcher.get_rows().row_count(), 1);
    assert_eq!(
        fixture.panel.get_apps().row_count(),
        crate::projection::MAX_APPS
    );
    assert_eq!(fixture.launcher.get_selected_key(), "app-1023");
    let observations = fixture.host.observe_calls.load(Ordering::SeqCst);
    let subscriptions = fixture.host.subscription_calls.load(Ordering::SeqCst);
    let recycle = (
        fixture.host.recycle_provider_calls.load(Ordering::SeqCst),
        fixture
            .host
            .recycle_mutation_provider_calls
            .load(Ordering::SeqCst),
    );
    right_click(&fixture.launcher, "Launch Retained App 1023");
    let popup = menu(&fixture);
    assert!(popup.is_open());
    assert!(fixture.host.saves.lock().is_empty());
    key(popup.component(), Key::Return);
    let expected = preferences
        .with_launcher_favorites(vec!["uninstalled".into(), "app-1023".into()])
        .unwrap();
    assert_eq!(&*fixture.host.saves.lock(), &[expected]);
    assert_eq!(fixture.launcher.get_application_count(), 1);
    assert_eq!(fixture.launcher.get_selected_key(), "app-1023");
    no_unrelated_effects(&fixture, observations, subscriptions, recycle);
}

#[test]
fn genuine_menu_and_shift_f10_open_pin_while_escape_and_root_hide_retire_it() {
    let fixture = LauncherFixture::new();
    fixture.controller.open_launcher();
    fixture.click_launcher("All Apps");
    let popup = open_editor(&fixture); // right press gives this genuine tile focus
    key(popup.component(), Key::Escape);
    assert!(!popup.is_open());
    fixture.key(Key::Menu.into());
    assert!(popup.is_open());
    key(popup.component(), Key::Escape);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
    fixture.key(Key::F10.into());
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
    assert!(popup.is_open());
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::CloseRequested);
    assert!(!fixture.launcher.window().is_visible());
    assert!(!popup.is_open());
    popup
        .component()
        .invoke_action_requested(DockMenuAction::FavoriteAdd);
    fixture.controller.open_launcher();
    popup
        .component()
        .invoke_action_requested(DockMenuAction::FavoriteAdd);
    assert!(fixture.host.saves.lock().is_empty());
    assert!(fixture.host.launches.lock().is_empty());
}

#[test]
fn query_view_projection_catalog_and_membership_replacement_retire_old_menu_authority() {
    let fixture = LauncherFixture::new();
    fixture.controller.open_launcher();
    fixture.launcher.set_search("editor".into());
    fixture.controller.apply_launcher_filter();
    let popup = open_editor(&fixture);
    fixture.launcher.set_search("Rust".into());
    fixture.controller.apply_launcher_filter();
    assert_eq!(fixture.launcher.get_application_count(), 1);
    assert_eq!(
        fixture.tile(0).key,
        "app-editor",
        "replacement query has the same exact keys"
    );
    assert!(
        !popup.is_open(),
        "query meaning, not key equality, owns the scope"
    );
    popup
        .component()
        .invoke_action_requested(DockMenuAction::FavoriteAdd);
    open_editor(&fixture);
    fixture.controller.show_launcher_tiles();
    assert!(
        !popup.is_open(),
        "replaced projection is a new interaction scope"
    );
    popup
        .component()
        .invoke_action_requested(DockMenuAction::FavoriteAdd);
    open_editor(&fixture);
    fixture
        .launcher
        .invoke_view_requested(LauncherView::Favorites);
    assert!(!popup.is_open());
    popup
        .component()
        .invoke_action_requested(DockMenuAction::FavoriteAdd);
    fixture.launcher.set_search("editor".into());
    fixture.controller.apply_launcher_filter();
    open_editor(&fixture);
    assert!(
        fixture.host.saves.lock().is_empty(),
        "retired scopes have issued no saves"
    );
    fixture.click_launcher("Add to favorites: Rust Editor");
    assert_eq!(
        fixture.host.saves.lock().len(),
        1,
        "the existing corner control owns this save"
    );
    assert_eq!(fixture.controller.core.launcher_favorites(), ["app-editor"]);
    assert!(
        !popup.is_open(),
        "independently applied membership retires a stale Pin label"
    );
    popup
        .component()
        .invoke_action_requested(DockMenuAction::FavoriteAdd);
    open_editor(&fixture);
    apply_result_to_both(
        &fixture.controller,
        &fixture.panel,
        Ok(launcher_snapshot().with_applications(Vec::new())),
    );
    assert!(!popup.is_open());
    popup
        .component()
        .invoke_action_requested(DockMenuAction::FavoriteRemove);
    assert_eq!(
        fixture.host.saves.lock().len(),
        1,
        "old menu actions cannot add a second transaction"
    );
}

#[test]
fn hidden_busy_stale_foreign_filtered_catalog_and_presenting_intents_cannot_open_or_save() {
    let fixture = LauncherFixture::new();
    let point = slint::LogicalPosition::new(100.0, 100.0);
    fixture
        .launcher
        .invoke_app_menu_requested("app-editor".into(), point);
    assert!(fixture.controller.launcher_app_menu.borrow().is_none());
    fixture.controller.open_launcher();
    fixture.launcher.set_search("editor".into());
    fixture.controller.apply_launcher_filter();
    for (key, anchor) in [
        ("app-browser", point),
        ("foreign", point),
        ("", point),
        ("app-editor", slint::LogicalPosition::new(f32::NAN, 100.0)),
    ] {
        fixture
            .launcher
            .invoke_app_menu_requested(key.into(), anchor);
        assert!(fixture.controller.launcher_app_menu.borrow().is_none());
    }
    for stale in [false, true] {
        fixture.panel.set_refreshing(!stale);
        fixture.panel.set_stale(stale);
        fixture
            .launcher
            .invoke_app_menu_requested("app-editor".into(), point);
        assert!(fixture.controller.launcher_app_menu.borrow().is_none());
    }
    fixture.panel.set_refreshing(false);
    fixture.panel.set_stale(false);
    fixture.controller.sync_launcher_status();
    let popup = open_editor(&fixture);
    fixture.panel.set_stale(true);
    fixture.controller.sync_launcher_status();
    assert!(!popup.is_open());
    popup
        .component()
        .invoke_action_requested(DockMenuAction::FavoriteAdd);
    fixture.panel.set_stale(false);
    fixture.controller.sync_launcher_status();
    apply_result_to_both(
        &fixture.controller,
        &fixture.panel,
        Ok(launcher_snapshot().with_applications(Vec::new())),
    );
    fixture
        .launcher
        .set_rows(slint::ModelRc::new(slint::VecModel::from(vec![
            crate::generated::LaunchRow {
                tiles: slint::ModelRc::new(slint::VecModel::from(vec![
                    crate::generated::LaunchTile {
                        key: "app-editor".into(),
                        label: "Fabricated retained row".into(),
                        ..Default::default()
                    },
                ])),
            },
        ])));
    fixture.launcher.set_application_count(1);
    fixture
        .launcher
        .invoke_app_menu_requested("app-editor".into(), point);
    assert!(
        !popup.is_open(),
        "injected native rows are not catalog authority"
    );
    fixture.controller.hide_launcher();
    let launcher = fixture.launcher.as_weak();
    LAUNCHER_CONFIGURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |_| {
            launcher
                .upgrade()
                .unwrap()
                .invoke_app_menu_requested("app-editor".into(), point);
        }))
    });
    fixture.controller.open_launcher();
    assert!(!popup.is_open());
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn preference_save_and_native_reorder_invalidate_and_block_application_menus() {
    let fixture = LauncherFixture::with_preferences(reorder_preferences());
    fixture.controller.open_launcher();
    let _ = begin_editor_reorder(&fixture);
    fixture.launcher.invoke_app_menu_requested(
        "app-editor".into(),
        slint::LogicalPosition::new(100.0, 100.0),
    );
    assert!(fixture.controller.launcher_app_menu.borrow().is_none());
    fixture.controller.hide_launcher();
    fixture.controller.open_launcher();
    fixture.click_launcher("All Apps");
    let popup = open_editor(&fixture);
    let launcher = fixture.launcher.as_weak();
    let captured_popup = popup.clone();
    PREFERENCE_SAVE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            launcher.upgrade().unwrap().invoke_app_menu_requested(
                "app-editor".into(),
                slint::LogicalPosition::new(100.0, 100.0),
            );
            assert!(
                !captured_popup.is_open(),
                "the in-progress real transaction blocks reopening"
            );
        }))
    });
    key(popup.component(), Key::Return); // current editor is already a favorite: remove
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert!(
        !fixture
            .controller
            .core
            .launcher_favorites()
            .iter()
            .any(|key| key == "app-editor")
    );
}

#[test]
fn detach_root_close_reopen_or_new_popup_supersedes_old_favorite_before_any_save() {
    for replacement_popup in [false, true] {
        let fixture = LauncherFixture::new();
        fixture.controller.open_launcher();
        fixture.click_launcher("All Apps");
        let popup = open_editor(&fixture);
        let controller = fixture.controller.clone();
        let launcher = fixture.launcher.as_weak();
        RECYCLE_MENU_DROP_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(
                    controller.launcher_app_menu.try_borrow_mut().is_ok(),
                    "no cache borrow across native detach"
                );
                if replacement_popup {
                    launcher.upgrade().unwrap().invoke_app_menu_requested(
                        "app-browser".into(),
                        slint::LogicalPosition::new(200.0, 100.0),
                    );
                } else {
                    launcher
                        .upgrade()
                        .unwrap()
                        .window()
                        .dispatch_event(WindowEvent::CloseRequested);
                    controller.open_launcher();
                }
            }))
        });
        key(popup.component(), Key::Return);
        assert!(
            fixture.host.saves.lock().is_empty(),
            "detach-time replacement retires captured favorite request"
        );
        if replacement_popup {
            assert!(popup.is_open());
            key(popup.component(), Key::Space);
            assert_eq!(
                fixture.controller.core.launcher_favorites(),
                ["app-browser"]
            );
            assert_eq!(fixture.host.saves.lock().len(), 1);
        } else {
            assert!(!popup.is_open());
            assert!(fixture.launcher.window().is_visible());
            assert_eq!(fixture.launcher.get_view(), LauncherView::Favorites);
        }
    }
}

#[test]
fn save_time_reopen_keeps_persisted_desired_membership_without_restoring_old_query_or_focus() {
    let fixture = LauncherFixture::new();
    fixture.controller.open_launcher();
    fixture.launcher.set_search("editor".into());
    fixture.controller.apply_launcher_filter();
    let popup = open_editor(&fixture);
    let controller = fixture.controller.clone();
    PREFERENCE_SAVE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            controller.hide_launcher();
            controller.open_launcher();
        }))
    });
    key(popup.component(), Key::Return);
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert_eq!(fixture.controller.core.launcher_favorites(), ["app-editor"]);
    assert_eq!(fixture.launcher.get_view(), LauncherView::Favorites);
    assert_eq!(fixture.launcher.get_search(), "");
    assert_eq!(fixture.launcher.get_selected_key(), "");
    assert!(!popup.is_open());
    fixture.key("r".into());
    assert_eq!(fixture.launcher.get_search(), "r");
    assert!(fixture.host.launches.lock().is_empty());
}

fn bar_menu(fixture: &LauncherFixture) -> Rc<crate::context_menu::ContextMenuController> {
    right_click(&fixture.dock, "Open applications and settings");
    let menu = fixture
        .controller
        .menus
        .borrow()
        .clone()
        .expect("real native bar context");
    assert!(menu.is_open());
    menu
}

#[test]
fn native_optional_media_add_persists_whole_applied_record_before_provider_acquisition() {
    let preferences = PanelPreferences::new(Theme::Light, false)
        .with_dock(crate::DockEdge::Left, vec!["app-browser".into()])
        .with_launcher_display_mode(crate::LauncherDisplayMode::Fullscreen)
        .with_launcher_favorites(vec!["uninstalled".into(), "app-editor".into()])
        .unwrap();
    assert!(!preferences.media_enabled());
    let fixture = LauncherFixture::with_preferences(preferences.clone());
    fixture.panel.set_theme_index(2);
    fixture.panel.set_compact(true);
    let popup = bar_menu(&fixture);
    assert!(fixture.host.saves.lock().is_empty());
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 0);
    let expected = preferences.with_media_enabled(true);
    let expected_for_factory = expected.clone();
    let host = fixture.host.clone();
    let controller = fixture.controller.clone();
    let captured_popup = popup.clone();
    MEDIA_FACTORY_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(!captured_popup.is_open());
            assert_eq!(
                &*host.saves.lock(),
                std::slice::from_ref(&expected_for_factory)
            );
            assert_eq!(
                controller.core.applied_preferences(),
                expected_for_factory,
                "confirmed record precedes module provider initialization"
            );
        }))
    });
    click_component(popup.component(), "Add media module");
    assert_eq!(fixture.controller.core.applied_preferences(), expected);
    assert_eq!(&*fixture.host.saves.lock(), &[expected]);
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 1);
    assert!(fixture.dock.get_media_view().enabled);
    fixture.dock.invoke_media_enabled_requested(true);
    popup
        .component()
        .invoke_action_requested(DockMenuAction::MediaBarAdd);
    assert_eq!(
        fixture.host.saves.lock().len(),
        1,
        "absolute repeated Add never saves twice"
    );
    assert!(
        !fixture.launcher.window().is_visible(),
        "right-click Start never opens Launcher"
    );
}

#[test]
fn native_optional_media_failed_add_does_not_adopt_module_or_acquire_provider() {
    let preferences = seeded_preferences()
        .with_launcher_favorites(vec!["app-editor".into()])
        .unwrap();
    let fixture = LauncherFixture::with_preferences(preferences.clone());
    let popup = bar_menu(&fixture);
    *fixture.host.save_result.lock() = Err("disk full\n\u{1b}private".into());
    click_component(popup.component(), "Add media module");
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    assert!(!fixture.dock.get_media_view().enabled);
    assert_eq!(fixture.host.media_provider_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert!(
        fixture
            .panel
            .get_status()
            .contains("Could not save media module")
    );
    assert!(!fixture.panel.get_status().contains('\u{1b}'));
    assert!(!popup.is_open());
}

#[test]
fn native_optional_media_remove_preserves_unrelated_record_and_stale_scopes_cannot_readd() {
    let preferences = seeded_preferences()
        .with_launcher_favorites(vec!["app-editor".into()])
        .unwrap();
    let fixture = LauncherFixture::with_preferences(preferences.clone());
    let popup = bar_menu(&fixture);
    click_component(popup.component(), "Add media module");
    assert!(fixture.dock.get_media_view().enabled);
    let acquisitions = fixture.host.media_provider_calls.load(Ordering::SeqCst);
    let popup = bar_menu(&fixture);
    click_component(popup.component(), "Remove media module");
    assert!(!fixture.dock.get_media_view().enabled);
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    assert_eq!(fixture.host.saves.lock().len(), 2);
    assert_eq!(fixture.host.saves.lock().last().unwrap(), &preferences);
    assert_eq!(
        fixture.host.media_provider_calls.load(Ordering::SeqCst),
        acquisitions
    );
    popup
        .component()
        .invoke_action_requested(DockMenuAction::MediaBarAdd);
    fixture.dock.invoke_media_enabled_requested(false);
    assert_eq!(fixture.host.saves.lock().len(), 2);
    assert!(fixture.host.observe_calls.load(Ordering::SeqCst) == 0);
    assert!(fixture.host.launches.lock().is_empty());
    assert!(fixture.host.system_actions.lock().is_empty());
}
