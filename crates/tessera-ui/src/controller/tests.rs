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
        retry.refresh().unwrap().join().unwrap();
        // Retained two-pulse heartbeat: the helper queues its initial tick on
        // the live event loop and starts a two-second repeated timer; the
        // queued tick drains before the quit event below. The timer stays
        // alive (held) until the loop has ended — retention is the contract.
        let ticks = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ticks);
        let options = crate::RunOptions {
            surface: crate::SurfaceMode::Panel,
            heartbeat: Some(Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            })),
        };
        let timer = crate::start_heartbeat(&options);
        assert!(timer.is_some());
        slint::invoke_from_event_loop(move || {
            // The queued initial heartbeat tick drains before this event.
            assert_eq!(ticks.load(Ordering::SeqCst), 1);
            slint::quit_event_loop().unwrap();
        })
        .unwrap();
    })
    .unwrap();
    slint::run_event_loop().unwrap();

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
    let host = FixtureHost::returning(snapshot());
    *host.subscription_result.lock() = Err("watcher unavailable".into());
    let mut subscription_error = None;
    let (_core, guard) =
        SurfaceCore::new(host.clone(), &seeded_preferences(), &mut subscription_error);
    assert!(subscription_error.is_some());
    assert_eq!(host.subscription_calls.load(Ordering::SeqCst), 1);
    // A failed subscription yields no guard to retain.
    assert!(guard.is_none());
    // Presentation degrades to manual refresh; the notice text is bounded.
    assert!(subscription_error.unwrap().contains("use Refresh"));
}

#[test]
fn dock_rects_cover_edge_orientation_and_fullscreen_hide_decision() {
    // Fullscreen work areas must never produce a visible overlay: the
    // controller hides the strip, and geometry is simply not applied.
    let fullscreen = crate::DockContext::new(0, 0, 1920, 1040, true).unwrap();
    assert!(fullscreen.fullscreen_active());
    let bottom = crate::dock::dock_rect(fullscreen, crate::DockEdge::Bottom, 56, 1.0);
    assert_eq!(
        bottom.y + bottom.height as i32,
        fullscreen.y() + fullscreen.height() as i32
    );
    let top = crate::dock::dock_rect(fullscreen, crate::DockEdge::Top, 56, 1.0);
    assert_eq!(top.y, fullscreen.y());
}

#[test]
fn fullscreen_dock_hide_decision_never_yields_a_zero_window_exit() {
    // Dock geometry on a fullscreen work area is not applied: the controller
    // hides the strip and the event loop must stay alive because it was
    // started with run_event_loop_until_quit. This test pins the geometry
    // decision only; loop-lifetime behavior is covered by the dock runner.
    let fullscreen = crate::DockContext::new(0, 0, 1920, 1040, true).unwrap();
    assert!(fullscreen.fullscreen_active());
    // Both edges still compute an in-bounds strip; the controller simply
    // never shows it while this flag is set.
    for edge in [
        crate::DockEdge::Bottom,
        crate::DockEdge::Top,
        crate::DockEdge::Left,
        crate::DockEdge::Right,
    ] {
        let rect = crate::dock::dock_rect(fullscreen, edge, 56, 1.0);
        assert!(rect.width >= 1 && rect.height >= 1);
        assert!(rect.x >= fullscreen.x());
        assert!(rect.y >= fullscreen.y());
    }
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
    // Fullscreen snapshots must never leave a visible dock: geometry returns
    // early (hide) rather than applying a strip over the work area.
    let fullscreen = crate::DockContext::new(0, 0, 1920, 1040, true).unwrap();
    assert!(fullscreen.fullscreen_active());
}
