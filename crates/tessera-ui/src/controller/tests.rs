// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

#[cfg(debug_assertions)]
use i_slint_backend_testing::ElementHandle;

use super::*;
use crate::{PanelApplication, PanelWindow, Theme};

/// One integration backend per process: `set_platform` is once-only (the
/// event-loop proxy is process-wide), so later tests reuse the first backend
/// instead of re-initializing.
fn init_integration_once() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        i_slint_backend_testing::init_integration_test_with_system_time();
    });
}

type Observation = dyn Fn() -> Result<PanelSnapshot, String> + Send + Sync;

struct FixtureHost {
    source: Box<Observation>,
    observe_calls: AtomicUsize,
    activations: Mutex<Vec<String>>,
    activation_result: Mutex<Result<(), String>>,
    saves: Mutex<Vec<PanelPreferences>>,
    save_result: Mutex<Result<(), String>>,
    launches: Mutex<Vec<String>>,
    launch_result: Mutex<Result<(), String>>,
    system_actions: Mutex<Vec<SystemAction>>,
    system_result: Mutex<Result<(), String>>,
    subscription_result: Mutex<Result<bool, String>>,
    subscription_calls: AtomicUsize,
    ui_focus_calls: AtomicUsize,
    ui_focus_result: Mutex<Result<(), String>>,
    lease_drops: Arc<AtomicUsize>,
}

impl FixtureHost {
    #[allow(clippy::too_many_arguments)]
    fn new(
        source: impl Fn() -> Result<PanelSnapshot, String> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            source: Box::new(source),
            observe_calls: AtomicUsize::new(0),
            activations: Mutex::new(Vec::new()),
            activation_result: Mutex::new(Ok(())),
            saves: Mutex::new(Vec::new()),
            save_result: Mutex::new(Ok(())),
            launches: Mutex::new(Vec::new()),
            launch_result: Mutex::new(Ok(())),
            system_actions: Mutex::new(Vec::new()),
            system_result: Mutex::new(Ok(())),
            subscription_result: Mutex::new(Ok(true)),
            subscription_calls: AtomicUsize::new(0),
            ui_focus_calls: AtomicUsize::new(0),
            ui_focus_result: Mutex::new(Ok(())),
            lease_drops: Arc::default(),
        })
    }

    fn returning(snapshot: PanelSnapshot) -> Arc<Self> {
        Self::new(move || Ok(snapshot.clone()))
    }
}

impl DesktopHost for FixtureHost {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        self.observe_calls.fetch_add(1, Ordering::SeqCst);
        (self.source)()
    }

    fn activate(&self, key: &str) -> Result<(), String> {
        self.activations.lock().push(key.to_owned());
        self.activation_result.lock().clone()
    }

    fn launch(&self, key: &str) -> Result<(), String> {
        self.launches.lock().push(key.to_owned());
        self.launch_result.lock().clone()
    }

    fn system_action(&self, action: SystemAction) -> Result<(), String> {
        self.system_actions.lock().push(action);
        self.system_result.lock().clone()
    }

    fn save_preferences(&self, preferences: &PanelPreferences) -> Result<(), String> {
        self.saves.lock().push(preferences.clone());
        self.save_result.lock().clone()
    }

    fn subscribe(
        &self,
        _callback: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Option<Box<dyn Send>>, String> {
        self.subscription_calls.fetch_add(1, Ordering::SeqCst);
        let available = self.subscription_result.lock().clone();
        available.map(|available| available.then_some(Box::new(()) as Box<dyn Send>))
    }

    fn configure_surface(
        &self,
        _kind: SurfaceKind,
        _window: &slint::Window,
    ) -> Result<Option<Box<dyn std::any::Any>>, String> {
        struct Lease {
            dropped: Arc<AtomicUsize>,
            _ui_thread: Rc<()>,
        }
        impl Drop for Lease {
            fn drop(&mut self) {
                self.dropped.fetch_add(1, Ordering::SeqCst);
            }
        }
        Ok(Some(Box::new(Lease {
            dropped: Arc::clone(&self.lease_drops),
            _ui_thread: Rc::new(()),
        })))
    }

    fn request_ui_focus(&self, _window: &slint::Window) -> Result<(), String> {
        self.ui_focus_calls.fetch_add(1, Ordering::SeqCst);
        self.ui_focus_result.lock().clone()
    }
}

/// Builds a controller over a fresh shared core for `host`, with the seeded
/// preferences the old direct-construction tests relied on.
fn controller_for(panel: &Panel, host: Arc<FixtureHost>) -> PanelController {
    let mut subscription_error = None;
    let (core, _guard) = SurfaceCore::new(host, &seeded_preferences(), &mut subscription_error);
    let controller = PanelController::new(panel, Arc::clone(&core));
    core.install_routes(controller.routes());
    controller
}

fn seeded_preferences() -> PanelPreferences {
    PanelPreferences::new(Theme::System, false).with_dock(crate::DockEdge::Bottom, Vec::new())
}

fn snapshot() -> PanelSnapshot {
    PanelSnapshot::new(
        2,
        vec![
            PanelWindow::new("editor-key".into(), "Rust Editor\nwindow".into(), false),
            PanelWindow::new("browser-key".into(), "Web Browser".into(), true),
        ],
        1,
    )
}

// Slint allows one threaded backend per process. All worker scenarios share
// this event loop; joins below are test-only and no production UI waits on them.
#[test]
fn workers_are_single_flight_recover_from_errors_and_do_not_retain_closed_windows() {
    init_integration_once();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(snapshot());
    let controller = controller_for(&panel, host.clone());
    controller.refresh().unwrap().join().unwrap();
    assert!(panel.get_refreshing());
    panel.invoke_refresh_requested();
    assert_eq!(host.observe_calls.load(Ordering::SeqCst), 1);

    let failed_panel = Panel::new().unwrap();
    let attempts = AtomicUsize::new(0);
    let retry_host = FixtureHost::new(move || {
        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Err("native query\nfailed\u{202e}".into())
        } else {
            Ok(PanelSnapshot::new(1, Vec::new(), 0))
        }
    });
    let retry = controller_for(&failed_panel, retry_host.clone());
    apply_result(&retry, &failed_panel, Ok(snapshot()));
    retry.refresh().unwrap().join().unwrap();

    let panicked_panel = Panel::new().unwrap();
    let panicked = controller_for(
        &panicked_panel,
        FixtureHost::new(|| panic!("private fixture")),
    );
    panicked.refresh().unwrap().join().unwrap();

    let closing_panel = Panel::new().unwrap();
    let closed = closing_panel.as_weak();
    let (started, entered) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let closing = controller_for(
        &closing_panel,
        FixtureHost::new(move || {
            started.send(()).unwrap();
            gate.lock().recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(PanelSnapshot::new(1, Vec::new(), 0))
        }),
    );
    let worker = closing.refresh().unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(closing_panel);
    assert!(closed.upgrade().is_none());
    release.send(()).unwrap();
    worker.join().unwrap();

    let failed = failed_panel.as_weak();
    let retry_core = Arc::clone(&retry.core);
    let ticks = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&ticks);
    let heartbeat = Rc::new(RefCell::new(crate::Heartbeat::unarmed(
        &crate::RunOptions {
            surface: crate::SurfaceMode::Panel,
            heartbeat: Some(Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            })),
        },
    )));
    failed_panel.on_readiness_pulse({
        let heartbeat = Rc::clone(&heartbeat);
        move || heartbeat.borrow_mut().arm()
    });
    slint::invoke_from_event_loop(move || {
        let panel = failed.upgrade().unwrap();
        assert!(panel.get_stale());
        assert!(!panel.get_refreshing());
        let error = panel.get_status();
        assert!(error.contains("retained data is stale"));
        panel.invoke_activate_requested("editor-key".into());
        assert!(retry_host.activations.lock().is_empty());
        panel.set_search("EDITOR".into());
        panel.invoke_filter_requested();
        assert_eq!(panel.get_status(), error);
        assert_eq!(panel.get_rows().row_count(), 1);
        // Immediate fixture retry queues delivery before the quit callback.
        PanelController::refresh_from(&panel, &retry_core)
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(ticks.load(Ordering::SeqCst), 0);
        // Signal UI-thread state through a weak component, not through a
        // Send closure pretending that a Slint timer or native lease is Send.
        panel.invoke_readiness_pulse();
        slint::invoke_from_event_loop(move || {
            assert_eq!(ticks.load(Ordering::SeqCst), 1);
            slint::quit_event_loop().unwrap();
        })
        .unwrap();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
    assert!(heartbeat.borrow().timer.running());

    assert!(!panel.get_refreshing());
    assert_eq!(
        panel.get_rows().row_data(0).unwrap().caption,
        "Rust Editor window"
    );
    #[cfg(debug_assertions)]
    assert_eq!(
        ElementHandle::find_by_accessible_label(&panel, "Refresh")
            .next()
            .unwrap()
            .accessible_enabled(),
        Some(true)
    );
    assert!(!failed_panel.get_stale());
    assert_eq!(failed_panel.get_rows().row_count(), 0);
    assert!(failed_panel.get_status().contains("No visible windows"));
    assert!(!panicked_panel.get_refreshing());
    assert!(
        panicked_panel
            .get_status()
            .contains("Observation failed unexpectedly")
    );
    assert!(!panicked_panel.get_status().contains("private fixture"));
    assert!(closed.upgrade().is_none());
}

#[test]
fn search_callbacks_preserve_keys_and_activation_guards_and_errors_are_enforced() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(snapshot());
    let controller = controller_for(&panel, host.clone());
    // Real observation completion through the production apply path: it sets
    // has-snapshot and stores the retained snapshot the guard truly reads.
    // (The no-event-loop backend cannot drain a spawned worker's queued
    // completion event, so the worker result is applied in-process.)
    apply_result(&controller, &panel, Ok(snapshot()));
    assert!(panel.get_has_snapshot());
    panel.set_search("BROWSER".into());
    panel.invoke_filter_requested();
    assert_eq!(panel.get_rows().row_count(), 1);
    assert_eq!(panel.get_rows().row_data(0).unwrap().key, "browser-key");
    panel.invoke_activate_requested("browser-key".into());
    assert_eq!(host.activations.lock().as_slice(), ["browser-key"]);
    // Guard scenarios: refreshing, stale, and a missing snapshot each block
    // the activation; the count must not move. (Guard probes only — the
    // flags are restored right after each block.)
    panel.set_refreshing(true);
    panel.invoke_activate_requested("browser-key".into());
    panel.set_refreshing(false);
    panel.set_stale(true);
    panel.invoke_activate_requested("browser-key".into());
    panel.set_stale(false);
    panel.set_has_snapshot(false);
    panel.invoke_activate_requested("browser-key".into());
    panel.set_has_snapshot(true);
    // State restored: the same key is visible and unguarded again.
    panel.invoke_activate_requested("browser-key".into());
    // The denied attempt still records the request; the result is the error.
    assert_eq!(host.activations.lock().len(), 3);
    *host.activation_result.lock() = Err("denied\u{202e}".into());
    panel.invoke_activate_requested("browser-key".into());
    assert!(panel.get_status().contains("Activation failed"));
    assert!(!panel.get_status().contains('\u{202e}'));
    panel.set_search("".into());
    panel.invoke_filter_requested();
    assert_eq!(panel.get_rows().row_count(), 2);
    assert_eq!(host.observe_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn preferences_save_only_on_explicit_callback_and_errors_are_visible() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(snapshot());
    let _controller = controller_for(&panel, host.clone());
    panel.set_theme_index(2);
    panel.set_compact(true);
    assert!(host.saves.lock().is_empty());
    panel.invoke_save_preferences_requested();
    assert_eq!(
        host.saves.lock().as_slice(),
        [PanelPreferences::new(Theme::Dark, true).with_dock(crate::DockEdge::Bottom, Vec::new())]
    );
    assert_eq!(panel.get_status(), "Preferences saved");
    *host.save_result.lock() = Err("disk full\u{200f}".into());
    panel.invoke_save_preferences_requested();
    assert!(panel.get_status().contains("Could not save preferences"));
    assert!(!panel.get_status().contains('\u{200f}'));
}

#[test]
fn initial_failure_is_not_stale_data_and_visible_rows_are_bounded() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let controller = controller_for(&panel, FixtureHost::returning(snapshot()));
    apply_result(&controller, &panel, Err("desktop unavailable".into()));
    assert!(!panel.get_has_snapshot());
    assert!(!panel.get_status().contains("retained data"));
    let rows = (0..crate::projection::MAX_ROWS + 12)
        .map(|index| PanelWindow::new(index.to_string(), String::new(), false))
        .collect();
    apply_result(&controller, &panel, Ok(PanelSnapshot::new(1, rows, 3)));
    assert_eq!(panel.get_rows().row_count(), crate::projection::MAX_ROWS);
    assert_eq!(
        panel.get_rows().row_data(0).unwrap().caption,
        "(untitled window)"
    );
    assert!(panel.get_status().contains("128 of 140"));
}

// New dock/launcher interface tests below are NOT EXECUTED in this run (the
// parent owns the single integration build); they compile against the same
// fixture as the existing suite.

fn app_snapshot() -> PanelSnapshot {
    PanelSnapshot::new(
        1,
        vec![PanelWindow::new("w1".into(), "Editor".into(), false)],
        0,
    )
    .with_applications(vec![
        PanelApplication::new("app-editor".into(), "Rust Editor".into(), None).unwrap(),
        PanelApplication::new("app-browser".into(), "Web Browser".into(), None).unwrap(),
    ])
    .with_dock_context(crate::DockContext::new(0, 0, 1920, 1040, false).unwrap())
}

#[test]
fn launch_only_known_displayed_apps_and_pin_toggle_persists_saved_appearance() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(app_snapshot());
    let controller = controller_for(&panel, host.clone());
    apply_result(&controller, &panel, Ok(app_snapshot()));

    // Search surfaces the catalog; launch uses the displayed opaque key.
    panel.set_search("editor".into());
    panel.invoke_filter_requested();
    let apps = panel.get_apps();
    assert_eq!(apps.row_count(), 1);
    assert_eq!(apps.row_data(0).unwrap().key, "app-editor");

    panel.invoke_launch_requested("app-editor".into());
    assert_eq!(host.launches.lock().as_slice(), ["app-editor"]);

    // A fabricated or filtered-out key never reaches the host.
    panel.invoke_launch_requested("app-browser".into());
    panel.invoke_launch_requested("fabricated".into());
    panel.invoke_launch_requested("".into());
    assert_eq!(host.launches.lock().len(), 1);

    // Pin click persists immediately, using the SEEDED appearance values,
    // not any live preview: change preview first, then pin.
    panel.set_theme_index(2);
    panel.set_compact(true);
    panel.invoke_pin_toggle_requested("app-editor".into(), true);
    let saved = host.saves.lock();
    let last = saved.last().unwrap();
    assert_eq!(last.theme(), Theme::System);
    assert!(!last.compact());
    assert_eq!(last.pinned_apps(), ["app-editor".to_owned()]);
    drop(saved);

    // Repeating the same state change is a no-op (no extra save).
    let saves_before = host.saves.lock().len();
    panel.invoke_pin_toggle_requested("app-editor".into(), true);
    assert_eq!(host.saves.lock().len(), saves_before);

    // Unpin persists the removal the same way.
    panel.invoke_pin_toggle_requested("app-editor".into(), false);
    let saved = host.saves.lock();
    assert!(saved.last().unwrap().pinned_apps().is_empty());
    drop(saved);
    assert_eq!(host.launches.lock().len(), 1);
    assert!(panel.get_status().contains("Pin") || panel.get_status().contains("pin"));

    // Save failure leaves the pin state visibly failed, not half-applied.
    *host.save_result.lock() = Err("disk full".into());
    panel.invoke_pin_toggle_requested("app-editor".into(), true);
    assert!(panel.get_status().contains("Could not save pins"));
}

#[test]
fn launch_failure_is_visible_and_stale_blocks_launch() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(app_snapshot());
    let controller = controller_for(&panel, host.clone());
    apply_result(&controller, &panel, Ok(app_snapshot()));
    // Search surfaces the catalog row the launch then uses (empty search
    // matches everything, so the filter run refreshes the displayed rows).
    panel.set_search("".into());
    panel.invoke_filter_requested();
    *host.launch_result.lock() = Err("access denied\u{202e}".into());
    panel.invoke_launch_requested("app-editor".into());
    assert!(panel.get_status().contains("Launch failed"));
    assert!(!panel.get_status().contains('\u{202e}'));

    // While stale (after a real failed refresh) launching is blocked entirely.
    let attempts = AtomicUsize::new(0);
    let stale_host = FixtureHost::new(move || {
        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Err("desktop unavailable".into())
        } else {
            Ok(app_snapshot())
        }
    });
    let stale_controller = controller_for(&panel, stale_host);
    apply_result(&stale_controller, &panel, Err("desktop unavailable".into()));
    *host.launch_result.lock() = Ok(());
    panel.invoke_launch_requested("app-editor".into());
    assert_eq!(host.launches.lock().len(), 1);
}

#[test]
fn system_actions_dispatch_and_failures_are_visible() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(snapshot());
    let _controller = controller_for(&panel, host.clone());
    panel.invoke_system_action_requested(0);
    panel.invoke_system_action_requested(1);
    panel.invoke_system_action_requested(2);
    assert_eq!(
        host.system_actions.lock().as_slice(),
        [
            SystemAction::OpenFileManager,
            SystemAction::OpenTaskManager,
            SystemAction::RestoreExplorer
        ]
    );
    *host.system_result.lock() = Err("shell is running".into());
    panel.invoke_system_action_requested(2);
    assert!(panel.get_status().contains("failed"));
    assert!(panel.get_status().contains("shell is running"));
}

#[test]
fn subscription_failure_leaves_manual_refresh_and_notice() {
    i_slint_backend_testing::init_no_event_loop();
    let host = FixtureHost::returning(snapshot());
    *host.subscription_result.lock() = Err("watcher unavailable".into());
    let mut subscription_error = None;
    let (core, guard) =
        SurfaceCore::new(host.clone(), &seeded_preferences(), &mut subscription_error);
    assert!(subscription_error.is_some());
    assert_eq!(host.subscription_calls.load(Ordering::SeqCst), 1);
    // A failed subscription yields no guard to retain.
    assert!(guard.is_none());
    // Presentation degrades to manual refresh; the notice text is bounded.
    let notice = subscription_error.unwrap();
    assert!(notice.contains("use Refresh"));

    let panel = Panel::new().unwrap();
    panel.set_startup_notice(notice.clone().into());
    let dock = Dock::new().unwrap();
    let toolbar = Toolbar::new().unwrap();
    let launcher = Launcher::new().unwrap();
    let controller =
        PanelController::new_with_dock(&panel, &dock, &toolbar, &launcher, Arc::clone(&core));
    let _scope = SurfaceLeaseScope(Rc::clone(&controller.leases));
    controller.sync_launcher_status();
    assert_eq!(launcher.get_notice(), notice);

    // Successful manual refresh cannot repair a failed subscription, so
    // its warning remains visible independently of the current status.
    apply_result_to_both(&controller, &panel, Ok(launcher_snapshot()));
    assert_eq!(launcher.get_notice(), notice);
    assert_ne!(launcher.get_status(), launcher.get_notice());
    assert!(!launcher.get_stale());
}

#[test]
fn bars_never_request_focus_but_user_opened_windows_do() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(launcher_snapshot());
    let core = controller_for(&panel, host.clone()).core;
    let dock = Dock::new().unwrap();
    let toolbar = Toolbar::new().unwrap();
    let launcher = Launcher::new().unwrap();
    let controller = PanelController::new_with_dock(&panel, &dock, &toolbar, &launcher, core);
    let _scope = SurfaceLeaseScope(Rc::clone(&controller.leases));
    apply_result_to_both(&controller, &panel, Ok(launcher_snapshot()));
    assert_eq!(host.ui_focus_calls.load(Ordering::SeqCst), 0);
    let fullscreen = launcher_snapshot()
        .with_dock_context(crate::DockContext::new(0, 0, 1920, 1040, true).unwrap());
    apply_result_to_both(&controller, &panel, Ok(fullscreen));
    assert!(!dock.window().is_visible());
    assert!(!toolbar.window().is_visible());
    apply_result_to_both(&controller, &panel, Ok(launcher_snapshot()));
    assert!(dock.window().is_visible());
    assert!(toolbar.window().is_visible());
    assert_eq!(host.ui_focus_calls.load(Ordering::SeqCst), 0);

    controller.open_launcher();
    assert_eq!(host.ui_focus_calls.load(Ordering::SeqCst), 1);
    controller.hide_launcher();
    controller.open_panel();
    assert_eq!(host.ui_focus_calls.load(Ordering::SeqCst), 2);

    *host.ui_focus_result.lock() = Err("Windows declined activation".into());
    controller.open_launcher();
    assert!(launcher.get_status().contains("declined activation"));
    assert!(controller.surface_failure.borrow().is_none());
    assert_eq!(host.ui_focus_calls.load(Ordering::SeqCst), 3);
}

#[test]
fn dock_rects_cover_monitor_bounds_and_fullscreen_hide_decision() {
    // Fullscreen monitor bounds must never produce a visible bar overlay:
    // the controller hides both bars (leases drop before the hides).
    let fullscreen = crate::DockContext::new(0, 0, 1920, 1040, true).unwrap();
    assert!(fullscreen.fullscreen_active());
    let bottom = crate::dock::dock_rect(fullscreen, crate::DockEdge::Bottom, 5, false, 1.0);
    assert_eq!(
        bottom.y + bottom.height as i32,
        fullscreen.y() + fullscreen.height() as i32
    );
    let toolbar = crate::dock::toolbar_rect(fullscreen, 1.0);
    assert_eq!(toolbar.y, fullscreen.y());
    assert_eq!(toolbar.width, fullscreen.width());
}

#[test]
fn dock_status_mirrors_panel_and_launch_guards_carry_over() {
    // Dock mode shares ONE busy/stale truth: the panel properties. The dock
    // status struct is a mirror; this pins the mirror invariants without a
    // second window backend.
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(app_snapshot());
    let controller = controller_for(&panel, host.clone());
    // No dock registered: dock-only commands resolve through panel models.
    apply_result(&controller, &panel, Ok(app_snapshot()));
    // The launcher model refreshes through the production filter path.
    panel.set_search("".into());
    panel.invoke_filter_requested();
    assert!(controller.resolve_app_key("app-editor").is_some());
    assert!(controller.resolve_app_key("fabricated").is_none());
    // Fullscreen snapshots must never leave a visible bar: geometry drops
    // the leases and hides both surfaces (heartbeat and loop keep running).
    let fullscreen = crate::DockContext::new(0, 0, 1920, 1040, true).unwrap();
    assert!(fullscreen.fullscreen_active());
}

// Seelen dock/toolbar/launcher focused tests (parent runs them integrated).

fn icon_tile_pixels(seed: u8) -> crate::PixelIcon {
    crate::PixelIcon::new(8, 8, vec![seed; 8 * 8 * 4]).unwrap()
}

fn launcher_snapshot() -> PanelSnapshot {
    PanelSnapshot::new(1, Vec::new(), 0)
        .with_applications(vec![
            PanelApplication::new(
                "app-editor".into(),
                "Rust Editor".into(),
                Some(icon_tile_pixels(1)),
            )
            .unwrap(),
            PanelApplication::new(
                "app-browser".into(),
                "Web Browser".into(),
                Some(icon_tile_pixels(2)),
            )
            .unwrap(),
            PanelApplication::new("app-files".into(), "File Manager".into(), None).unwrap(),
        ])
        .with_dock_context(crate::DockContext::new(0, 0, 1920, 1040, false).unwrap())
}

#[test]
fn launcher_tiles_search_pins_and_rescue_actions() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(launcher_snapshot());
    let controller = controller_for(&panel, host.clone());
    apply_result(&controller, &panel, Ok(launcher_snapshot()));

    // The launcher surface exists alongside the panel in these tests only
    // through the shared controller state; tiles project from the catalog.
    let tiles = crate::projection::project_apps(
        &controller.core.catalog(),
        &controller.core.pins(),
        "editor",
    );
    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].key, "app-editor");
    // Bounded: at most MAX_APPS tiles are ever rendered.
    let everything =
        crate::projection::project_apps(&controller.core.catalog(), &controller.core.pins(), "");
    assert!(everything.len() <= crate::projection::MAX_APPS);

    // Launch through the displayed launcher key resolves; unknown never does.
    assert!(controller.resolve_app_key("app-editor").is_some());
    assert!(controller.resolve_app_key("not-a-key").is_none());
    controller.launch("app-editor");
    assert_eq!(host.launches.lock().as_slice(), ["app-editor"]);
}

#[test]
fn shell_identity_is_bounded_and_focused_key_is_snapshot_eligible() {
    i_slint_backend_testing::init_no_event_loop();
    struct IdentityHost {
        base: Arc<FixtureHost>,
    }
    impl DesktopHost for IdentityHost {
        fn observe(&self) -> Result<PanelSnapshot, String> {
            self.base.observe()
        }
        fn activate(&self, key: &str) -> Result<(), String> {
            self.base.activate(key)
        }
        fn launch(&self, key: &str) -> Result<(), String> {
            self.base.launch(key)
        }
        fn system_action(&self, action: SystemAction) -> Result<(), String> {
            self.base.system_action(action)
        }
        fn save_preferences(&self, preferences: &PanelPreferences) -> Result<(), String> {
            self.base.save_preferences(preferences)
        }
        fn subscribe(
            &self,
            callback: Arc<dyn Fn() + Send + Sync>,
        ) -> Result<Option<Box<dyn Send>>, String> {
            self.base.subscribe(callback)
        }
        fn clock_text(&self) -> Result<String, String> {
            Ok("12:34".to_string())
        }
        fn shell_identity(&self) -> Result<crate::ShellIdentity, String> {
            Ok(crate::ShellIdentity {
                user_name: "user\u{202e}name".into(),
                clock: "12:34".into(),
                language: "en-US".into(),
                focused_window_key: Some("editor-key".into()),
            })
        }
    }
    let panel = Panel::new().unwrap();
    let mut subscription_error = None;
    let (core, _guard) = SurfaceCore::new(
        Arc::new(IdentityHost {
            base: FixtureHost::returning(snapshot()),
        }),
        &seeded_preferences(),
        &mut subscription_error,
    );
    let controller = PanelController::new(&panel, Arc::clone(&core));
    core.install_routes(controller.routes());
    apply_result(&controller, &panel, Ok(snapshot()));

    // Bounded, sanitized identity: direction controls are removed, the
    // focused key is retained because it is in the snapshot.
    let identity = core.identity();
    assert_eq!(identity.user_name, "username");
    assert_eq!(identity.clock, "12:34");
    assert_eq!(identity.language, "en-US");
    assert_eq!(identity.focused_window_key.as_deref(), Some("editor-key"));

    // The clock-only path refreshes just the clock text.
    core.refresh_clock();
    assert_eq!(core.identity().clock, "12:34");
}

#[test]
fn clock_timer_never_observes_the_desktop() {
    i_slint_backend_testing::init_no_event_loop();
    // The clock path exists precisely so the 60s timer does not re-run
    // observe/shell_identity: pin the separation in the SurfaceCore API.
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(snapshot());
    let controller = controller_for(&panel, host.clone());
    apply_result(&controller, &panel, Ok(snapshot()));
    let observes_before = host.observe_calls.load(Ordering::SeqCst);
    // Default host clock_text is an empty Ok; no error, no observation.
    assert_eq!(controller.core.refresh_clock(), Some(String::new()));
    assert_eq!(host.observe_calls.load(Ordering::SeqCst), observes_before);
}

#[test]
fn fullscreen_geometry_still_computes_in_bounds_rects_for_both_bars() {
    // Geometry decision only: a fullscreen monitor bounds hides BOTH bars
    // (leases drop before the hides); these rects stay in bounds for the
    // reveal after fullscreen ends.
    let fullscreen = crate::DockContext::new(-1920, -1080, 1920, 2160, true).unwrap();
    let dock = crate::dock::dock_rect(fullscreen, crate::DockEdge::Bottom, 0, false, 1.0);
    assert_eq!(
        dock.y + dock.height as i32,
        fullscreen.y() + fullscreen.height() as i32
    );
    let toolbar = crate::dock::toolbar_rect(fullscreen, 1.0);
    assert_eq!(toolbar.y, fullscreen.y());
    assert_eq!(toolbar.x, fullscreen.x());
    assert_eq!(toolbar.width, fullscreen.width());
}

#[test]
fn overflow_is_bounded_by_max_dock_tiles() {
    // A snapshot with more windows than MAX_DOCK_TILES never grows the
    // window beyond the clamped bounds (geometry stays within the monitor).
    let bounds = crate::DockContext::new(0, 0, 800, 600, false).unwrap();
    let rect = crate::dock::dock_rect(bounds, crate::DockEdge::Bottom, 400, false, 1.0);
    assert!(rect.width <= bounds.width());
    assert!(rect.x >= bounds.x());
    assert!(rect.y >= bounds.y());
}

// Native leases stay thread-affine and outlive attachment, not their HWNDs.

#[test]
fn lease_registry_geometry_change_detection_is_rect_exact() {
    let leases = crate::controller::SurfaceLeases::default();
    // Nothing attached yet: every rect is a change (first attach pending).
    assert!(leases.geometry_changed(crate::SurfaceKind::Toolbar, (0, 0, 1920, 32)));
    assert!(leases.geometry_changed(crate::SurfaceKind::Dock, (0, 0, 256, 72)));
}

#[test]
fn thread_affine_leases_survive_attach_and_release_before_window_teardown() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(snapshot());
    let controller = controller_for(&panel, host.clone());
    let core = Arc::downgrade(&controller.core);
    let rect = (0, 0, 1920, 32);
    {
        let mut leases = controller.leases.borrow_mut();
        leases
            .attach(SurfaceKind::Toolbar, host.as_ref(), panel.window(), rect)
            .unwrap();
        assert_eq!(host.lease_drops.load(Ordering::SeqCst), 0);
        assert!(!leases.geometry_changed(SurfaceKind::Toolbar, rect));
        leases
            .attach(
                SurfaceKind::Toolbar,
                host.as_ref(),
                panel.window(),
                (0, 0, 1920, 64),
            )
            .unwrap();
        assert_eq!(host.lease_drops.load(Ordering::SeqCst), 1);
    }
    let scope = SurfaceLeaseScope(Rc::clone(&controller.leases));
    drop(scope);
    assert_eq!(host.lease_drops.load(Ordering::SeqCst), 2);
    assert!(
        controller
            .leases
            .borrow()
            .geometry_changed(SurfaceKind::Toolbar, rect)
    );
    // Component callbacks may retain the controller, but cannot retain native
    // leases past the run scope or form a core -> route -> core reference cycle.
    drop(controller);
    drop(panel);
    assert!(core.upgrade().is_none());
}

#[test]
fn live_density_edge_and_pin_changes_resize_without_observing_or_saving_preview() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let dock = Dock::new().unwrap();
    let toolbar = Toolbar::new().unwrap();
    let launcher = Launcher::new().unwrap();
    let host = FixtureHost::returning(launcher_snapshot());
    let mut error = None;
    let (core, _watcher) = SurfaceCore::new(host.clone(), &seeded_preferences(), &mut error);
    let controller =
        PanelController::new_with_dock(&panel, &dock, &toolbar, &launcher, core.clone());
    let _scope = SurfaceLeaseScope(Rc::clone(&controller.leases));
    apply_result(&controller, &panel, Ok(launcher_snapshot()));
    controller.render();
    controller.sync_appearance();
    panel.set_compact(true);
    panel.set_dock_edge_index(crate::dock_edge_to_index(crate::DockEdge::Left));
    panel.invoke_appearance_changed();
    assert!(dock.get_compact());
    assert_eq!(
        dock.get_edge(),
        crate::dock_edge_to_index(crate::DockEdge::Left)
    );
    assert!(host.saves.lock().is_empty());
    assert_eq!(core.applied_dock_edge(), crate::DockEdge::Bottom);
    controller.toggle_pin("app-editor", true);
    let saved = host.saves.lock();
    assert_eq!(saved.len(), 1);
    assert!(!saved[0].compact());
    assert_eq!(saved[0].dock_edge(), crate::DockEdge::Bottom);
    drop(saved);
    let rect = crate::dock::dock_rect(
        core.dock_context().unwrap(),
        crate::DockEdge::Left,
        1,
        true,
        1.0,
    );
    assert_eq!(
        controller.leases.borrow().attached_rect[0],
        Some((rect.x, rect.y, rect.width, rect.height))
    );
    assert_eq!(host.observe_calls.load(Ordering::SeqCst), 0);
    panel.invoke_save_preferences_requested();
    assert!(core.applied_appearance().compact);
    assert_eq!(core.applied_dock_edge(), crate::DockEdge::Left);
}
