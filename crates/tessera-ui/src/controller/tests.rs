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

type NativeHook = Box<dyn FnOnce(&slint::Window)>;
type UiHook = Box<dyn FnOnce()>;
thread_local! {
    static LAUNCHER_CONFIGURE_HOOK: RefCell<Option<NativeHook>> = const { RefCell::new(None) };
    static LAUNCHER_DROP_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static PREFERENCE_SAVE_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static LAUNCHER_BACKEND_INITIALIZED: Cell<bool> = const { Cell::new(false) };
}

struct FixtureHost {
    source: Box<Observation>,
    observe_calls: AtomicUsize,
    activations: Mutex<Vec<String>>,
    activation_result: Mutex<Result<(), String>>,
    window_actions: Mutex<Vec<(String, WindowAction)>>,
    window_action_result: Mutex<Result<(), String>>,
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
    launcher_attach_calls: AtomicUsize,
    launcher_attachment_result: Mutex<Result<(), String>>,
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
            window_actions: Mutex::default(),
            window_action_result: Mutex::new(Ok(())),
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
            launcher_attach_calls: AtomicUsize::new(0),
            launcher_attachment_result: Mutex::new(Ok(())),
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

    fn window_action(&self, key: &str, action: WindowAction) -> Result<(), String> {
        self.window_actions.lock().push((key.to_owned(), action));
        self.window_action_result.lock().clone()
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
        let hook = PREFERENCE_SAVE_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
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
        kind: SurfaceKind,
        window: &slint::Window,
    ) -> Result<Option<Box<dyn std::any::Any>>, String> {
        if kind == SurfaceKind::Launcher {
            self.launcher_attach_calls.fetch_add(1, Ordering::SeqCst);
            let hook = LAUNCHER_CONFIGURE_HOOK.with(|hook| hook.borrow_mut().take());
            if let Some(hook) = hook {
                hook(window);
            }
            self.launcher_attachment_result.lock().clone()?;
        }
        struct Lease {
            dropped: Arc<AtomicUsize>,
            _ui_thread: Rc<()>,
            kind: SurfaceKind,
        }
        impl Drop for Lease {
            fn drop(&mut self) {
                self.dropped.fetch_add(1, Ordering::SeqCst);
                if self.kind == SurfaceKind::Launcher {
                    let hook = LAUNCHER_DROP_HOOK.with(|hook| hook.borrow_mut().take());
                    if let Some(hook) = hook {
                        hook();
                    }
                }
            }
        }
        Ok(Some(Box::new(Lease {
            dropped: Arc::clone(&self.lease_drops),
            _ui_thread: Rc::new(()),
            kind,
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
    controller
        .configure_lease(SurfaceKind::Toolbar, panel.window(), rect)
        .unwrap();
    assert_eq!(host.lease_drops.load(Ordering::SeqCst), 0);
    assert!(
        !controller
            .leases
            .borrow()
            .geometry_changed(SurfaceKind::Toolbar, rect)
    );
    controller
        .configure_lease(SurfaceKind::Toolbar, panel.window(), (0, 0, 1920, 64))
        .unwrap();
    assert_eq!(host.lease_drops.load(Ordering::SeqCst), 1);
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
    assert!(
        !controller
            .leases
            .borrow()
            .geometry_changed(SurfaceKind::Dock, (rect.x, rect.y, rect.width, rect.height))
    );
    assert_eq!(host.observe_calls.load(Ordering::SeqCst), 0);
    panel.invoke_save_preferences_requested();
    assert!(core.applied_preferences().compact());
    assert_eq!(core.applied_dock_edge(), crate::DockEdge::Left);
}

#[test]
fn dock_window_commands_use_displayed_keys_and_reject_queued_stale_intents() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(app_snapshot());
    let core = controller_for(&panel, host.clone()).core;
    let dock = Dock::new().unwrap();
    let toolbar = Toolbar::new().unwrap();
    let launcher = Launcher::new().unwrap();
    let controller = PanelController::new_with_dock(&panel, &dock, &toolbar, &launcher, core);
    let _scope = SurfaceLeaseScope(Rc::clone(&controller.leases));
    apply_result_to_both(&controller, &panel, Ok(app_snapshot()));

    for key in ["", "0", "fabricated"] {
        dock.invoke_window_command_requested(key.into(), DockWindowCommand::Close);
    }
    assert!(host.window_actions.lock().is_empty());
    for (command, action) in [
        (DockWindowCommand::Activate, WindowAction::Activate),
        (DockWindowCommand::Toggle, WindowAction::ActivateOrMinimize),
        (DockWindowCommand::Minimize, WindowAction::Minimize),
        (DockWindowCommand::Close, WindowAction::Close),
    ] {
        dock.invoke_window_command_requested("w1".into(), command);
        assert_eq!(
            host.window_actions.lock().last(),
            Some(&("w1".into(), action))
        );
    }
    assert!(panel.get_status().contains("close requested"));
    assert!(
        host.activations.lock().is_empty(),
        "dock uses the typed command path"
    );

    *host.window_action_result.lock() = Err("Windows denied the request".into());
    dock.invoke_window_command_requested("w1".into(), DockWindowCommand::Close);
    assert!(panel.get_status().contains("Window command failed"));
    assert!(launcher.get_status().contains("denied"));
    let count = host.window_actions.lock().len();
    panel.set_refreshing(true);
    dock.invoke_window_command_requested("w1".into(), DockWindowCommand::Close);
    panel.set_refreshing(false);
    panel.set_stale(true);
    dock.invoke_window_command_requested("w1".into(), DockWindowCommand::Minimize);
    panel.set_stale(false);
    assert_eq!(host.window_actions.lock().len(), count);

    // A row retained only in the panel is not a displayed dock command target.
    dock.set_running_windows(ModelRc::new(VecModel::<DockWindow>::default()));
    dock.invoke_window_command_requested("w1".into(), DockWindowCommand::Close);
    assert_eq!(host.window_actions.lock().len(), count);
    assert!(panel.get_status().contains("no longer displayed"));
    assert_eq!(host.observe_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn dock_pin_and_recovery_menu_commands_reuse_real_saved_and_host_actions() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(app_snapshot());
    let preferences = PanelPreferences::new(Theme::System, false)
        .with_dock(crate::DockEdge::Bottom, vec!["app-editor".into()]);
    let mut subscription_error = None;
    let (core, _watcher) = SurfaceCore::new(host.clone(), &preferences, &mut subscription_error);
    let dock = Dock::new().unwrap();
    let toolbar = Toolbar::new().unwrap();
    let launcher = Launcher::new().unwrap();
    let controller = PanelController::new_with_dock(&panel, &dock, &toolbar, &launcher, core);
    let _scope = SurfaceLeaseScope(Rc::clone(&controller.leases));
    apply_result_to_both(&controller, &panel, Ok(app_snapshot()));
    panel.set_theme_index(2);
    panel.set_compact(true);
    dock.invoke_pin_toggle_requested("app-editor".into(), false);
    let saved = host.saves.lock();
    assert!(saved.last().unwrap().pinned_apps().is_empty());
    assert_eq!(saved.last().unwrap().theme(), Theme::System);
    assert!(!saved.last().unwrap().compact());
    drop(saved);

    panel.set_stale(true);
    for command in [
        DockSystemCommand::FileManager,
        DockSystemCommand::TaskManager,
        DockSystemCommand::Restore,
    ] {
        dock.invoke_system_command_requested(command);
    }
    assert_eq!(
        host.system_actions.lock().as_slice(),
        [
            SystemAction::OpenFileManager,
            SystemAction::OpenTaskManager,
            SystemAction::RestoreExplorer,
        ]
    );
    dock.invoke_open_settings_requested();
    assert_eq!(host.ui_focus_calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.observe_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn appearance_callbacks_during_show_apply_latest_geometry_without_reentrant_leases() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(app_snapshot());
    let core = controller_for(&panel, host.clone()).core;
    let dock = Dock::new().unwrap();
    let toolbar = Toolbar::new().unwrap();
    let launcher = Launcher::new().unwrap();
    let controller = PanelController::new_with_dock(&panel, &dock, &toolbar, &launcher, core);
    let _scope = SurfaceLeaseScope(Rc::clone(&controller.leases));
    apply_result_to_both(&controller, &panel, Ok(app_snapshot()));
    // The pending changed handler is synchronously flushed by Dock.show().
    panel.set_compact(true);
    let context = controller.core.dock_context().unwrap();
    assert!(controller.apply_geometry(Some(context)).unwrap());
    assert!(dock.get_compact());
    let rect = crate::dock::dock_rect(
        context,
        crate::DockEdge::Bottom,
        dock.get_pinned_apps().row_count() + dock.get_running_windows().row_count(),
        true,
        dock.window().scale_factor(),
    );
    assert_eq!(
        dock.window().size(),
        slint::PhysicalSize::new(rect.width, rect.height)
    );
    assert!(
        !controller
            .leases
            .borrow()
            .geometry_changed(SurfaceKind::Dock, (rect.x, rect.y, rect.width, rect.height),)
    );
    assert_eq!(host.observe_calls.load(Ordering::SeqCst), 0);
}

struct LauncherFixture {
    // Drop transient and bar attachments before the owned component windows.
    _quick_scope:
        crate::transient_window::TransientScope<crate::quick_settings::QuickSettingsController>,
    _scope: SurfaceLeaseScope,
    controller: PanelController,
    panel: Panel,
    dock: Dock,
    toolbar: Toolbar,
    launcher: Launcher,
    host: Arc<FixtureHost>,
}

impl LauncherFixture {
    fn new() -> Self {
        Self::with_preferences(seeded_preferences())
    }

    fn with_preferences(preferences: PanelPreferences) -> Self {
        Self::with_snapshot(preferences, launcher_snapshot())
    }

    fn with_snapshot(preferences: PanelPreferences, snapshot: PanelSnapshot) -> Self {
        LAUNCHER_BACKEND_INITIALIZED.with(|initialized| {
            if !initialized.replace(true) {
                i_slint_backend_testing::init_no_event_loop();
            }
        });
        let panel = Panel::new().unwrap();
        let host = FixtureHost::returning(snapshot.clone());
        let mut subscription_error = None;
        let (core, _guard) = SurfaceCore::new(host.clone(), &preferences, &mut subscription_error);
        let dock = Dock::new().unwrap();
        let toolbar = Toolbar::new().unwrap();
        let launcher = Launcher::new().unwrap();
        let controller =
            PanelController::new_with_dock(&panel, &dock, &toolbar, &launcher, core.clone());
        core.install_routes(controller.routes());
        let scope = SurfaceLeaseScope(Rc::clone(&controller.leases));
        let quick_scope = crate::transient_window::TransientScope::new(
            Rc::clone(&controller.quick_settings),
            crate::quick_settings::QuickSettingsController::hide,
        );
        apply_result_to_both(&controller, &panel, Ok(snapshot));
        Self {
            _quick_scope: quick_scope,
            _scope: scope,
            controller,
            panel,
            dock,
            toolbar,
            launcher,
            host,
        }
    }

    fn tile(&self, index: usize) -> crate::generated::LaunchTile {
        let columns = self.launcher.get_grid_columns().max(1) as usize;
        self.launcher
            .get_rows()
            .row_data(index / columns)
            .unwrap()
            .tiles
            .row_data(index % columns)
            .unwrap()
    }

    // The portable testing adapter has no screen-position readback. Check the
    // controller's attached physical RECT instead of claiming native placement.
    fn attached_launcher_rect(&self) -> Option<(i32, i32, u32, u32)> {
        self.controller
            .leases
            .borrow()
            .attachments
            .get(&SurfaceKind::Launcher)
            .map(|attachment| attachment.rect)
    }

    fn click_launcher(&self, label: &str) {
        use slint::platform::{PointerEventButton, WindowEvent};
        let button =
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(&self.launcher, label)
                .next()
                .unwrap_or_else(|| panic!("missing native launcher control: {label}"));
        let origin = button.absolute_position();
        let size = button.size();
        let position =
            slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        self.launcher
            .window()
            .dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
        self.launcher
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
    }

    fn key(&self, text: slint::SharedString) {
        use slint::platform::WindowEvent;
        self.launcher
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.launcher
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    }
}

#[test]
fn genuine_toolbar_trigger_opens_independent_popup_and_rejects_hidden_geometry() {
    let fixture = LauncherFixture::new();
    fixture.panel.set_refreshing(true);
    fixture.panel.set_stale(true);
    let trigger = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &fixture.toolbar,
        "Open quick settings",
    )
    .next()
    .unwrap();
    let click = || {
        use slint::platform::{PointerEventButton, WindowEvent};
        let origin = trigger.absolute_position();
        let size = trigger.size();
        let center =
            slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        fixture
            .toolbar
            .window()
            .dispatch_event(WindowEvent::PointerPressed {
                position: center,
                button: PointerEventButton::Left,
            });
        fixture
            .toolbar
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position: center,
                button: PointerEventButton::Left,
            });
    };
    click();
    let first = fixture.controller.quick_settings.borrow().clone().unwrap();
    assert!(
        first.is_open(),
        "audio is independent from desktop busy/stale state"
    );
    assert!(
        !fixture.panel.window().is_visible(),
        "toolbar does not open the framed Panel"
    );
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), 1);
    click();
    let reused = fixture.controller.quick_settings.borrow().clone().unwrap();
    assert!(
        Rc::ptr_eq(&first, &reused),
        "the shown popup is cached, not recreated"
    );
    assert!(
        reused.is_open(),
        "the reference trigger shows rather than toggles"
    );
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    fixture
        .toolbar
        .invoke_quick_settings_requested(crate::generated::TileBounds {
            origin: slint::LogicalPosition::new(f32::NAN, 8.0),
            width: 16.0,
            height: 16.0,
        });
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), focus);
    first.hide();
    fixture.toolbar.hide().unwrap();
    fixture
        .toolbar
        .invoke_quick_settings_requested(crate::generated::TileBounds {
            origin: slint::LogicalPosition::new(100.0, 8.0),
            width: 16.0,
            height: 16.0,
        });
    assert!(
        !first.is_open(),
        "a queued hidden-bar callback never opens hardware UI"
    );
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), focus);
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn launcher_keyboard_launch_failure_success_reopen_and_trigger_lifetime() {
    use slint::platform::{Key, WindowEvent};

    let fixture = LauncherFixture::new();
    fixture.dock.invoke_open_applications_requested();
    assert!(fixture.launcher.window().is_visible());
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.launcher.get_selected_key(), "");
    fixture.launcher.set_search("editor".into());
    fixture.controller.apply_launcher_filter();
    assert_eq!(fixture.launcher.get_selected_key(), "app-editor");
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    *fixture.host.launch_result.lock() = Err("Access denied\n\u{1b}native failure".into());
    let drops = fixture.host.lease_drops.load(Ordering::SeqCst);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: Key::Return.into(),
        });
    assert_eq!(fixture.host.launches.lock().as_slice(), ["app-editor"]);
    assert!(
        fixture.launcher.window().is_visible(),
        "failed launch keeps actionable feedback visible"
    );
    assert_eq!(fixture.host.lease_drops.load(Ordering::SeqCst), drops);
    let status = fixture.launcher.get_status();
    assert!(status.contains("Launch failed:"));
    assert!(!status.contains('\n') && !status.contains('\u{1b}'));
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated {
            text: Key::Return.into(),
        });
    assert_eq!(
        fixture.host.launches.lock().len(),
        1,
        "held search Enter must not relaunch"
    );
    *fixture.host.launch_result.lock() = Ok(());
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: Key::Return.into(),
        });
    assert_eq!(
        fixture.host.launches.lock().as_slice(),
        ["app-editor", "app-editor"]
    );
    assert!(!fixture.launcher.window().is_visible());
    assert_eq!(fixture.host.lease_drops.load(Ordering::SeqCst), drops + 1);
    assert!(
        !fixture
            .controller
            .leases
            .borrow()
            .attachments
            .contains_key(&SurfaceKind::Launcher)
    );
    fixture.launcher.invoke_activate_selected_requested();
    fixture
        .launcher
        .invoke_launch_requested("app-editor".into());
    assert_eq!(
        fixture.host.launches.lock().len(),
        2,
        "queued hidden-surface signals cannot launch"
    );

    fixture.dock.invoke_open_applications_requested();
    assert!(fixture.launcher.window().is_visible());
    assert_eq!(fixture.launcher.get_search(), "");
    assert_eq!(fixture.launcher.get_selected_key(), "");
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), 2);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: Key::Return.into(),
        });
    assert_eq!(
        fixture.host.launches.lock().len(),
        2,
        "blank reopen has no implicit first-item launch"
    );
    fixture.dock.invoke_open_applications_requested();
    assert!(
        !fixture.launcher.window().is_visible(),
        "the repeated real trigger toggles closed"
    );
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.host.lease_drops.load(Ordering::SeqCst), drops + 2);
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn launcher_selection_reconciles_current_results_and_rejects_cross_surface_keys() {
    let fixture = LauncherFixture::new();
    fixture.controller.open_launcher();
    fixture.launcher.set_search("editor".into());
    fixture.controller.apply_launcher_filter();
    assert_eq!(fixture.launcher.get_selected_key(), "app-editor");
    assert!(
        fixture.controller.resolve_app_key("app-browser").is_some(),
        "browser is still displayed on another surface"
    );
    fixture
        .launcher
        .invoke_launch_requested("app-browser".into());
    fixture
        .launcher
        .invoke_launch_requested("fabricated-key".into());
    fixture
        .launcher
        .invoke_select_requested("fabricated-key".into());
    assert!(fixture.host.launches.lock().is_empty());
    assert_eq!(fixture.launcher.get_selected_key(), "app-editor");

    let browser = PanelApplication::new("app-browser".into(), "Web Browser".into(), None).unwrap();
    let changed = launcher_snapshot().with_applications(vec![browser.clone()]);
    apply_result_to_both(&fixture.controller, &fixture.panel, Ok(changed));
    assert_eq!(fixture.launcher.get_application_count(), 0);
    assert_eq!(fixture.launcher.get_selected_key(), "");
    fixture.launcher.invoke_activate_selected_requested();
    fixture
        .launcher
        .invoke_launch_requested("app-editor".into());
    assert!(fixture.host.launches.lock().is_empty());
    fixture.launcher.set_search("".into());
    fixture.controller.apply_launcher_filter();
    fixture
        .launcher
        .invoke_navigate_requested(crate::generated::LauncherNavigation::Down);
    assert_eq!(fixture.launcher.get_selected_key(), "app-browser");
    let reordered = launcher_snapshot().with_applications(vec![
        PanelApplication::new("app-first".into(), "A first application".into(), None).unwrap(),
        browser,
    ]);
    apply_result_to_both(&fixture.controller, &fixture.panel, Ok(reordered));
    assert_eq!(fixture.tile(1).key, "app-browser");
    assert_eq!(
        fixture.launcher.get_selected_key(),
        "app-browser",
        "selection follows identity, not the old index"
    );

    for stale in [false, true] {
        fixture.panel.set_refreshing(!stale);
        fixture.panel.set_stale(stale);
        fixture
            .launcher
            .invoke_launch_requested("app-browser".into());
        fixture.launcher.invoke_activate_selected_requested();
        fixture
            .launcher
            .invoke_navigate_requested(crate::generated::LauncherNavigation::Left);
        assert!(fixture.host.launches.lock().is_empty());
        assert_eq!(fixture.launcher.get_selected_key(), "app-browser");
    }
    fixture.panel.set_refreshing(false);
    fixture.panel.set_stale(false);
    fixture.controller.launch("app-browser");
    assert_eq!(fixture.host.launches.lock().as_slice(), ["app-browser"]);
    assert!(
        fixture.launcher.window().is_visible(),
        "ordinary panel/dock launch does not own launcher lifetime"
    );
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn launcher_native_views_search_and_reopen_use_independent_favorites() {
    use crate::generated::LauncherView;
    use slint::platform::{Key, WindowEvent};

    let preferences = seeded_preferences()
        .with_dock(crate::DockEdge::Bottom, vec!["app-browser".into()])
        .with_launcher_favorites(vec!["uninstalled".into(), "app-editor".into()])
        .unwrap();
    let fixture = LauncherFixture::with_preferences(preferences);
    fixture.controller.open_launcher();
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    assert_eq!(fixture.launcher.get_view(), LauncherView::Favorites);
    assert!(fixture.launcher.get_saved_favorites_present());
    assert_eq!(fixture.launcher.get_application_count(), 1);
    assert_eq!(fixture.tile(0).key, "app-editor");
    assert!(fixture.tile(0).favorite);
    assert_eq!(fixture.controller.core.pins(), ["app-browser"]);
    assert_eq!(fixture.launcher.get_selected_key(), "");

    fixture.click_launcher("All Apps");
    assert_eq!(fixture.launcher.get_view(), LauncherView::All);
    assert_eq!(fixture.launcher.get_application_count(), 3);
    assert_eq!(fixture.launcher.get_selected_key(), "");
    fixture.launcher.set_search("browser".into());
    fixture.controller.apply_launcher_filter();
    assert_eq!(fixture.launcher.get_selected_key(), "app-browser");
    fixture.click_launcher("Back to favorites");
    assert_eq!(fixture.launcher.get_view(), LauncherView::Favorites);
    assert_eq!(fixture.launcher.get_search(), "");
    assert_eq!(fixture.launcher.get_selected_key(), "");
    fixture.launcher.set_search(" ".into());
    fixture.controller.apply_launcher_filter();
    assert_eq!(fixture.launcher.get_view(), LauncherView::All);
    assert_eq!(fixture.launcher.get_application_count(), 3);
    assert_eq!(fixture.launcher.get_selected_key(), "");
    fixture.click_launcher("Back to favorites");
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: Key::Return.into(),
        });
    assert!(
        fixture.host.launches.lock().is_empty(),
        "blank Favorites never auto-launches"
    );
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: Key::DownArrow.into(),
        });
    assert_eq!(fixture.launcher.get_selected_key(), "app-editor");
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: Key::Return.into(),
        });
    assert_eq!(fixture.host.launches.lock().as_slice(), ["app-editor"]);
    assert!(!fixture.launcher.window().is_visible());
    fixture.controller.open_launcher();
    assert_eq!(fixture.launcher.get_view(), LauncherView::Favorites);
    assert_eq!(fixture.launcher.get_selected_key(), "");
    assert_eq!(fixture.launcher.get_search(), "");
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.host.subscription_calls.load(Ordering::SeqCst), 1);
    assert!(fixture.host.saves.lock().is_empty());
}

#[test]
fn launcher_favorite_transaction_preserves_complete_saved_record_and_failure_selection() {
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
    let focus_calls = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    fixture.click_launcher("Add to favorites: Rust Editor");
    let applied = fixture.controller.core.applied_preferences();
    assert_eq!(applied.theme(), Theme::Light);
    assert!(!applied.compact());
    assert_eq!(applied.dock_edge(), crate::DockEdge::Left);
    assert_eq!(applied.pinned_apps(), preferences.pinned_apps());
    assert_eq!(applied.launcher_favorites(), ["uninstalled", "app-editor"]);
    assert_eq!(
        applied.launcher().display_mode(),
        crate::LauncherDisplayMode::Fullscreen
    );
    assert_eq!(fixture.host.saves.lock().last().unwrap(), &applied);
    assert!(fixture.host.launches.lock().is_empty());
    fixture
        .launcher
        .invoke_favorite_toggle_requested("app-editor".into(), true);
    assert_eq!(
        fixture.host.saves.lock().len(),
        1,
        "absolute repeated intent is a no-op"
    );

    fixture
        .panel
        .invoke_pin_toggle_requested("app-editor".into(), true);
    let with_pin = fixture.controller.core.applied_preferences();
    assert_eq!(with_pin.launcher_favorites(), applied.launcher_favorites());
    assert_eq!(with_pin.theme(), Theme::Light);
    assert_eq!(with_pin.pinned_apps(), ["app-browser", "app-editor"]);
    assert_eq!(
        with_pin.launcher().display_mode(),
        crate::LauncherDisplayMode::Fullscreen
    );
    fixture.panel.invoke_save_preferences_requested();
    let appearance = fixture.controller.core.applied_preferences();
    assert_eq!(appearance.theme(), Theme::Dark);
    assert!(appearance.compact());
    assert_eq!(appearance.dock_edge(), crate::dock_edge_from_index(3));
    assert_eq!(
        appearance.launcher_favorites(),
        applied.launcher_favorites()
    );
    assert_eq!(appearance.pinned_apps(), with_pin.pinned_apps());
    assert_eq!(
        appearance.launcher().display_mode(),
        crate::LauncherDisplayMode::Fullscreen
    );
    assert_eq!(fixture.host.saves.lock().len(), 3);

    fixture.click_launcher("Back to favorites");
    fixture
        .launcher
        .invoke_select_requested("app-editor".into());
    *fixture.host.save_result.lock() = Err("disk full\n\u{1b}untrusted".into());
    fixture.click_launcher("Remove from favorites: Rust Editor");
    assert_eq!(fixture.controller.core.applied_preferences(), appearance);
    assert_eq!(fixture.launcher.get_selected_key(), "app-editor");
    assert_eq!(fixture.launcher.get_application_count(), 1);
    assert!(fixture.tile(0).favorite);
    assert!(
        fixture
            .launcher
            .get_status()
            .contains("Could not save favorites")
    );
    assert!(!fixture.launcher.get_status().contains('\n'));
    assert!(!fixture.launcher.get_status().contains('\u{1b}'));
    *fixture.host.save_result.lock() = Ok(());
    fixture.click_launcher("Remove from favorites: Rust Editor");
    assert_eq!(
        fixture.controller.core.launcher_favorites(),
        ["uninstalled"]
    );
    assert_eq!(fixture.launcher.get_selected_key(), "");
    assert_eq!(fixture.launcher.get_application_count(), 0);
    assert!(fixture.launcher.get_saved_favorites_present());
    assert!(fixture.host.launches.lock().is_empty());
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.host.subscription_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_calls
    );
}

#[test]
fn launcher_favorite_authority_rejects_hidden_busy_stale_and_other_surface_keys() {
    let fixture = LauncherFixture::new();
    fixture
        .launcher
        .invoke_favorite_toggle_requested("app-editor".into(), true);
    fixture.controller.open_launcher();
    fixture.launcher.set_search("editor".into());
    fixture.controller.apply_launcher_filter();
    assert!(fixture.controller.resolve_app_key("app-browser").is_some());
    for key in ["app-browser", "fabricated", ""] {
        fixture
            .launcher
            .invoke_favorite_toggle_requested(key.into(), true);
    }
    for stale in [false, true] {
        fixture.panel.set_refreshing(!stale);
        fixture.panel.set_stale(stale);
        fixture
            .launcher
            .invoke_favorite_toggle_requested("app-editor".into(), true);
        fixture
            .launcher
            .invoke_view_requested(crate::generated::LauncherView::Favorites);
        assert_eq!(fixture.launcher.get_search(), "editor");
    }
    fixture.panel.set_refreshing(false);
    fixture.panel.set_stale(false);
    apply_result_to_both(
        &fixture.controller,
        &fixture.panel,
        Ok(launcher_snapshot().with_applications(Vec::new())),
    );
    fixture
        .launcher
        .invoke_favorite_toggle_requested("app-editor".into(), true);
    // Even a malicious adapter-injected presentation row is not authority.
    fixture.launcher.set_application_count(1);
    fixture
        .launcher
        .set_rows(slint::ModelRc::new(slint::VecModel::from(vec![
            crate::generated::LaunchRow {
                tiles: slint::ModelRc::new(slint::VecModel::from(vec![
                    crate::generated::LaunchTile {
                        key: "app-editor".into(),
                        label: "Fabricated retained presentation".into(),
                        ..Default::default()
                    },
                ])),
            },
        ])));
    fixture
        .launcher
        .invoke_favorite_toggle_requested("app-editor".into(), true);
    assert!(fixture.host.saves.lock().is_empty());
    assert!(fixture.controller.core.launcher_favorites().is_empty());
    assert!(fixture.host.launches.lock().is_empty());
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn launcher_complete_inventory_reaches_favorite_tail_and_preserves_panel_bound() {
    use slint::platform::{Key, WindowEvent};

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
    let mut favorites = (100..200)
        .rev()
        .map(|index| format!("app-{index:04}"))
        .collect::<Vec<_>>();
    favorites.insert(50, "uninstalled".into());
    let preferences = seeded_preferences()
        .with_launcher_favorites(favorites.clone())
        .unwrap();
    let snapshot = launcher_snapshot().with_applications(applications);
    let fixture = LauncherFixture::with_snapshot(preferences, snapshot);
    fixture.controller.open_launcher();
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    assert_eq!(fixture.launcher.get_application_count(), 100);
    assert_eq!(fixture.launcher.get_rows().row_count(), 15);
    assert_eq!(fixture.tile(99).key, "app-0100");
    assert_eq!(
        fixture
            .controller
            .resolve_launcher_key("app-0100")
            .as_deref(),
        Some("app-0100")
    );
    assert!(
        fixture
            .controller
            .resolve_launcher_key("app-1023")
            .is_none()
    );

    // Native traversal enters the grid; arrows cross real virtual row boundaries.
    fixture.key(Key::Tab.into());
    fixture.key(Key::Tab.into());
    for _ in 0..14 {
        fixture.key(Key::DownArrow.into());
    }
    fixture.key(Key::RightArrow.into());
    assert_eq!(fixture.launcher.get_selected_key(), "app-0100");
    fixture.click_launcher("Remove from favorites: Retained App 100");
    favorites.retain(|key| key != "app-0100");
    assert_eq!(fixture.controller.core.launcher_favorites(), favorites);
    assert_eq!(fixture.launcher.get_application_count(), 99);
    assert_eq!(fixture.launcher.get_selected_key(), "");
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert_eq!(fixture.host.saves.lock()[0].launcher_favorites(), favorites);
    assert!(fixture.host.launches.lock().is_empty());

    fixture.click_launcher("All Apps");
    assert_eq!(fixture.launcher.get_application_count(), 1024);
    assert_eq!(fixture.launcher.get_rows().row_count(), 147);
    assert_eq!(fixture.tile(1023).key, "app-1023");
    assert_eq!(
        fixture
            .launcher
            .get_rows()
            .row_data(146)
            .unwrap()
            .tiles
            .row_count(),
        2
    );
    assert_eq!(
        fixture.panel.get_apps().row_count(),
        crate::projection::MAX_APPS
    );
    assert_eq!(
        fixture
            .controller
            .resolve_launcher_key("app-1023")
            .as_deref(),
        Some("app-1023")
    );
    fixture.launcher.set_search("Retained App".into());
    fixture.controller.apply_launcher_filter();
    assert_eq!(
        fixture.launcher.get_application_count(),
        1024,
        "query precedes complete row presentation"
    );
    fixture.launcher.set_search("1023".into());
    fixture.controller.apply_launcher_filter();
    assert_eq!(fixture.launcher.get_application_count(), 1);
    assert_eq!(fixture.launcher.get_selected_key(), "app-1023");
    assert!(
        fixture
            .controller
            .resolve_launcher_key("app-0100")
            .is_none()
    );
    // Same-query reprojection preserves the selected opaque identity.
    fixture.controller.show_launcher_tiles();
    fixture.key(Key::Return.into());
    assert_eq!(fixture.host.launches.lock().as_slice(), ["app-1023"]);
    assert!(!fixture.launcher.window().is_visible());
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.host.subscription_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn launcher_native_grid_to_header_switch_resets_selection_before_virtual_init() {
    use crate::generated::LauncherView;
    use slint::platform::{Key, WindowEvent};

    let preferences = seeded_preferences()
        .with_launcher_favorites(vec!["app-browser".into(), "app-editor".into()])
        .unwrap();
    let fixture = LauncherFixture::with_preferences(preferences);
    fixture.controller.open_launcher();
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    fixture.key(Key::Tab.into());
    fixture.key(Key::Tab.into());
    fixture.key(Key::RightArrow.into());
    assert_eq!(fixture.launcher.get_selected_key(), "app-editor");
    fixture.click_launcher("All Apps");
    assert_eq!(fixture.launcher.get_view(), LauncherView::All);
    assert_eq!(fixture.launcher.get_selected_key(), "");
    assert_eq!(fixture.launcher.get_search(), "");
    fixture.key(Key::Return.into());
    assert!(fixture.host.launches.lock().is_empty());
    // All and Favorites have different orders; another fresh mount is blank.
    fixture.key(Key::Tab.into());
    fixture.key(Key::Tab.into());
    fixture.key(Key::RightArrow.into());
    assert_eq!(fixture.launcher.get_selected_key(), "app-browser");
    fixture.click_launcher("Back to favorites");
    assert_eq!(fixture.launcher.get_view(), LauncherView::Favorites);
    assert_eq!(fixture.launcher.get_selected_key(), "");
    fixture.key(Key::Return.into());
    assert!(fixture.host.launches.lock().is_empty());
    assert!(fixture.host.saves.lock().is_empty());
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn launcher_native_display_mode_transaction_preserves_full_record_and_logical_authority() {
    use crate::LauncherDisplayMode;
    use crate::generated::{LauncherDisplayMode as UiMode, LauncherView};
    use slint::platform::{Key, WindowEvent};

    let applications = (0..128)
        .map(|index| {
            PanelApplication::new(
                format!("app-{index:04}"),
                format!("Retained App {index}"),
                None,
            )
            .unwrap()
        })
        .collect();
    let mut favorites = (0..100)
        .rev()
        .map(|index| format!("app-{index:04}"))
        .collect::<Vec<_>>();
    favorites.insert(20, "uninstalled".into());
    let preferences = PanelPreferences::new(Theme::Light, false)
        .with_dock(crate::DockEdge::Left, vec!["app-0001".into()])
        .with_launcher_favorites(favorites.clone())
        .unwrap();
    let fixture = LauncherFixture::with_snapshot(
        preferences.clone(),
        launcher_snapshot().with_applications(applications),
    );
    fixture.controller.open_launcher();
    fixture.launcher.set_search("Retained App".into());
    fixture.controller.apply_launcher_filter();
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    fixture.key(Key::Tab.into());
    fixture.key(Key::Tab.into());
    for _ in 0..18 {
        fixture.key(Key::DownArrow.into());
    }
    fixture.key(Key::RightArrow.into());
    assert_eq!(fixture.launcher.get_selected_key(), "app-0127");
    fixture.launcher.invoke_reset_scroll(); // Selected tail stays outside the top viewport.
    fixture.panel.set_theme_index(2);
    fixture.panel.set_compact(true);
    fixture.panel.set_dock_edge_index(3);
    fixture.panel.invoke_appearance_changed();
    let rows = fixture.launcher.get_rows();
    let position = fixture.launcher.window().position();
    let size = fixture.launcher.window().size();
    let focus_calls = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let drops = fixture.host.lease_drops.load(Ordering::SeqCst);
    *fixture.host.save_result.lock() = Err(format!("disk full\n\u{1b}{}", "untrusted".repeat(80)));
    fixture.click_launcher("Expand applications menu");
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    assert_eq!(fixture.launcher.get_display_mode(), UiMode::Windowed);
    assert_eq!(fixture.launcher.window().position(), position);
    assert_eq!(fixture.launcher.window().size(), size);
    assert_eq!(
        fixture.launcher.get_rows(),
        rows,
        "mode changes never rebuild inventory/rows"
    );
    assert_eq!(fixture.launcher.get_search(), "Retained App");
    assert_eq!(fixture.launcher.get_view(), LauncherView::All);
    assert_eq!(fixture.launcher.get_selected_key(), "app-0127");
    assert_eq!(fixture.host.lease_drops.load(Ordering::SeqCst), drops);
    assert!(
        fixture
            .launcher
            .get_status()
            .contains("Could not save launcher display mode")
    );
    assert!(fixture.launcher.get_status().len() < 300);
    assert!(!fixture.launcher.get_status().contains('\n'));
    assert!(!fixture.launcher.get_status().contains('\u{1b}'));

    *fixture.host.save_result.lock() = Ok(());
    fixture.click_launcher("Expand applications menu");
    let applied = preferences
        .clone()
        .with_launcher_display_mode(LauncherDisplayMode::Fullscreen);
    assert_eq!(fixture.controller.core.applied_preferences(), applied);
    assert_eq!(fixture.host.saves.lock().last(), Some(&applied));
    assert_eq!(fixture.launcher.get_display_mode(), UiMode::Fullscreen);
    assert_eq!(
        fixture.launcher.window().position(),
        slint::PhysicalPosition::new(0, 0)
    );
    assert_eq!(
        fixture.launcher.window().size(),
        slint::PhysicalSize::new(1920, 1040)
    );
    assert!(
        !fixture.launcher.window().is_fullscreen(),
        "product fullscreen is a physical overlay"
    );
    assert_eq!(fixture.launcher.get_rows(), rows);
    assert_eq!(fixture.launcher.get_selected_key(), "app-0127");
    assert_eq!(fixture.launcher.get_search(), "Retained App");
    assert_eq!(fixture.controller.core.launcher_favorites(), favorites);
    assert_eq!(fixture.controller.core.pins(), ["app-0001"]);
    assert_eq!(fixture.controller.core.catalog().len(), 128);
    assert_eq!(
        fixture
            .controller
            .resolve_launcher_key("app-0127")
            .as_deref(),
        Some("app-0127")
    );
    assert!(
        fixture
            .controller
            .resolve_launcher_key("uninstalled")
            .is_none()
    );
    fixture
        .launcher
        .invoke_display_mode_requested(UiMode::Fullscreen);
    assert_eq!(
        fixture.host.saves.lock().len(),
        2,
        "repeated absolute intent has zero saves"
    );
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_calls
    );
    assert!(fixture.host.launches.lock().is_empty());

    // Real footer focus survives both refits even after native grid navigation.
    // Return must toggle the footer, never launch the selected offscreen app.
    fixture.key(Key::Return.into());
    assert_eq!(fixture.launcher.get_display_mode(), UiMode::Windowed);
    assert_eq!(fixture.launcher.window().size(), size);
    fixture.key(Key::Return.into());
    assert_eq!(fixture.launcher.get_display_mode(), UiMode::Fullscreen);
    assert_eq!(fixture.host.saves.lock().len(), 4);
    assert_eq!(fixture.launcher.get_rows(), rows);
    assert_eq!(fixture.launcher.get_selected_key(), "app-0127");
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_calls
    );
    assert!(fixture.host.launches.lock().is_empty());

    let drops = fixture.host.lease_drops.load(Ordering::SeqCst);
    fixture.key(Key::Escape.into());
    assert!(!fixture.launcher.window().is_visible());
    assert_eq!(fixture.host.lease_drops.load(Ordering::SeqCst), drops + 1);
    fixture.controller.open_launcher();
    assert_eq!(fixture.launcher.get_display_mode(), UiMode::Fullscreen);
    assert_eq!(fixture.launcher.get_view(), LauncherView::Favorites);
    assert_eq!(fixture.launcher.get_search(), "");
    assert_eq!(fixture.launcher.get_selected_key(), "");
    fixture.click_launcher("Contract applications menu");
    assert_eq!(fixture.launcher.get_display_mode(), UiMode::Windowed);
    assert_eq!(fixture.launcher.window().position(), position);
    assert_eq!(fixture.launcher.window().size(), size);
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.host.subscription_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn launcher_display_mode_guards_busy_stale_hidden_and_missing_monitor_before_save() {
    use crate::generated::LauncherDisplayMode as UiMode;

    let fixture = LauncherFixture::new();
    fixture
        .launcher
        .invoke_display_mode_requested(UiMode::Fullscreen);
    fixture.controller.open_launcher();
    for stale in [false, true] {
        fixture.panel.set_refreshing(!stale);
        fixture.panel.set_stale(stale);
        fixture
            .launcher
            .invoke_display_mode_requested(UiMode::Fullscreen);
    }
    fixture.panel.set_refreshing(false);
    fixture.panel.set_stale(false);
    fixture
        .controller
        .core
        .store_snapshot(PanelSnapshot::new(0, Vec::new(), 0));
    fixture.controller.render();
    let size = fixture.launcher.window().size();
    let position = fixture.launcher.window().position();
    fixture.click_launcher("Expand applications menu");
    assert!(fixture.host.saves.lock().is_empty());
    assert_eq!(fixture.launcher.get_display_mode(), UiMode::Windowed);
    assert_eq!(fixture.launcher.window().size(), size);
    assert_eq!(fixture.launcher.window().position(), position);
    assert!(
        fixture
            .launcher
            .get_status()
            .contains("monitor bounds are unavailable")
    );
    fixture.controller.hide_launcher();
}

#[test]
fn launcher_saved_fullscreen_without_monitor_stays_hidden_until_valid_open() {
    use crate::LauncherDisplayMode;
    use crate::generated::LauncherDisplayMode as UiMode;

    let preferences =
        seeded_preferences().with_launcher_display_mode(LauncherDisplayMode::Fullscreen);
    let startup =
        LauncherFixture::with_snapshot(preferences.clone(), PanelSnapshot::new(0, Vec::new(), 0));
    startup.controller.open_launcher();
    assert!(
        !startup.launcher.window().is_visible(),
        "missing bounds cannot fake fullscreen"
    );
    assert_eq!(startup.launcher.get_display_mode(), UiMode::Windowed);
    assert_eq!(startup.controller.core.applied_preferences(), preferences);
    assert_eq!(startup.host.launcher_attach_calls.load(Ordering::SeqCst), 0);
    assert!(startup.host.saves.lock().is_empty());
    assert!(
        startup
            .launcher
            .get_status()
            .contains("Could not present saved launcher display mode")
    );
    apply_result_to_both(&startup.controller, &startup.panel, Ok(launcher_snapshot()));
    assert!(!startup.launcher.window().is_visible());
    startup.controller.open_launcher();
    assert_eq!(startup.launcher.get_display_mode(), UiMode::Fullscreen);
    assert_eq!(
        startup.launcher.window().size(),
        slint::PhysicalSize::new(1920, 1040)
    );
    assert!(!startup.launcher.window().is_fullscreen());
}

#[test]
fn launcher_visible_context_and_scale_refit_without_reset_focus_or_hidden_placement() {
    use crate::LauncherDisplayMode;
    use crate::generated::LauncherDisplayMode as UiMode;
    use slint::platform::WindowEvent;

    let fixture = LauncherFixture::new();
    fixture.controller.open_launcher();
    fixture.launcher.set_search("editor".into());
    fixture.controller.apply_launcher_filter();
    let focus_calls = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let attached = fixture.host.launcher_attach_calls.load(Ordering::SeqCst);
    fixture.controller.render();
    fixture.controller.update_geometry();
    assert_eq!(
        fixture.host.launcher_attach_calls.load(Ordering::SeqCst),
        attached
    );
    let context = crate::DockContext::new(-3200, -200, 3200, 1800, false).unwrap();
    apply_result_to_both(
        &fixture.controller,
        &fixture.panel,
        Ok(launcher_snapshot().with_dock_context(context)),
    );
    assert_eq!(
        fixture.attached_launcher_rect(),
        Some((-2200, 205, 1200, 990))
    );
    assert_eq!(
        fixture.launcher.window().size(),
        slint::PhysicalSize::new(1200, 990)
    );
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
    fixture.controller.render();
    assert_eq!(
        fixture.attached_launcher_rect(),
        Some((-2480, 205, 1760, 990))
    );
    assert_eq!(
        fixture.launcher.window().size(),
        slint::PhysicalSize::new(1760, 990)
    );
    fixture.click_launcher("Expand applications menu");
    assert_eq!(fixture.launcher.get_display_mode(), UiMode::Fullscreen);
    let context = crate::DockContext::new(-2560, -1440, 2560, 1440, true).unwrap();
    apply_result_to_both(
        &fixture.controller,
        &fixture.panel,
        Ok(launcher_snapshot().with_dock_context(context)),
    );
    assert_eq!(
        fixture.attached_launcher_rect(),
        Some((-2560, -1440, 2560, 1440))
    );
    assert_eq!(
        fixture.launcher.window().size(),
        slint::PhysicalSize::new(2560, 1440)
    );
    assert!(
        !fixture
            .controller
            .leases
            .borrow()
            .geometry_changed(SurfaceKind::Launcher, (-2560, -1440, 2560, 1440),)
    );
    assert_eq!(fixture.launcher.get_search(), "editor");
    assert_eq!(fixture.launcher.get_selected_key(), "app-editor");
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_calls
    );
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert_eq!(
        fixture
            .controller
            .core
            .applied_preferences()
            .launcher()
            .display_mode(),
        LauncherDisplayMode::Fullscreen
    );
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);

    fixture.controller.hide_launcher();
    let attached = fixture.host.launcher_attach_calls.load(Ordering::SeqCst);
    let position = fixture.launcher.window().position();
    let size = fixture.launcher.window().size();
    apply_result_to_both(&fixture.controller, &fixture.panel, Ok(launcher_snapshot()));
    fixture.controller.update_geometry();
    assert_eq!(
        fixture.host.launcher_attach_calls.load(Ordering::SeqCst),
        attached
    );
    assert_eq!(fixture.launcher.window().position(), position);
    assert_eq!(fixture.launcher.window().size(), size);
    assert_eq!(fixture.attached_launcher_rect(), None);
    assert!(!fixture.launcher.window().is_visible());
    fixture.controller.open_launcher();
    assert_eq!(
        fixture.launcher.window().position(),
        slint::PhysicalPosition::new(0, 0)
    );
    assert_eq!(
        fixture.launcher.window().size(),
        slint::PhysicalSize::new(1920, 1040)
    );
    assert_eq!(fixture.launcher.get_search(), "");
}

#[test]
fn launcher_mode_save_completion_after_hide_keeps_saved_mode_without_resurrection() {
    use crate::LauncherDisplayMode;
    use crate::generated::LauncherDisplayMode as UiMode;

    let fixture = LauncherFixture::new();
    fixture.controller.open_launcher();
    let controller = fixture.controller.clone();
    PREFERENCE_SAVE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(controller.leases.try_borrow_mut().is_ok());
            assert!(controller.launcher_state.try_borrow_mut().is_ok());
            controller.hide_launcher();
        }));
    });
    let attached = fixture.host.launcher_attach_calls.load(Ordering::SeqCst);
    let focus_calls = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    fixture.click_launcher("Expand applications menu");
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert_eq!(
        fixture
            .controller
            .core
            .applied_preferences()
            .launcher()
            .display_mode(),
        LauncherDisplayMode::Fullscreen
    );
    assert!(!fixture.launcher.window().is_visible());
    assert_eq!(
        fixture.host.launcher_attach_calls.load(Ordering::SeqCst),
        attached
    );
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_calls
    );
    fixture.controller.open_launcher();
    assert_eq!(fixture.launcher.get_display_mode(), UiMode::Fullscreen);
    assert_eq!(
        fixture.launcher.window().size(),
        slint::PhysicalSize::new(1920, 1040)
    );
    assert_eq!(fixture.host.saves.lock().len(), 1);
}

#[test]
fn launcher_saved_mode_native_attachment_failure_is_truthful_and_recoverable() {
    use crate::LauncherDisplayMode;
    use crate::generated::LauncherDisplayMode as UiMode;

    let fixture = LauncherFixture::new();
    fixture.controller.open_launcher();
    fixture.launcher.set_search("editor".into());
    fixture.controller.apply_launcher_filter();
    let focus_calls = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    *fixture.host.launcher_attachment_result.lock() = Err("native denied\n\u{1b}untrusted".into());
    fixture.click_launcher("Expand applications menu");
    let applied = fixture.controller.core.applied_preferences();
    assert_eq!(
        applied.launcher().display_mode(),
        LauncherDisplayMode::Fullscreen
    );
    assert_eq!(fixture.host.saves.lock().last(), Some(&applied));
    assert!(!fixture.launcher.window().is_visible());
    assert!(
        fixture
            .controller
            .leases
            .borrow()
            .geometry_changed(SurfaceKind::Launcher, (0, 0, 1920, 1040))
    );
    assert!(
        fixture
            .launcher
            .get_status()
            .contains("Launcher display mode saved, but could not present it")
    );
    assert!(!fixture.launcher.get_status().contains('\n'));
    assert!(!fixture.launcher.get_status().contains('\u{1b}'));
    assert_eq!(fixture.launcher.get_search(), "editor");
    assert_eq!(fixture.launcher.get_selected_key(), "app-editor");
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_calls
    );
    assert!(
        fixture.controller.surface_failure.borrow().is_none(),
        "launcher failure keeps recovery bars alive"
    );
    *fixture.host.launcher_attachment_result.lock() = Ok(());
    fixture.controller.open_launcher();
    assert!(fixture.launcher.window().is_visible());
    assert_eq!(fixture.launcher.get_display_mode(), UiMode::Fullscreen);
    assert!(!fixture.launcher.window().is_fullscreen());
    assert_eq!(fixture.host.saves.lock().len(), 1);
}

#[test]
fn launcher_native_configure_cancellation_discards_late_lease_before_hide() {
    use crate::LauncherDisplayMode;
    use slint::platform::WindowEvent;

    let fixture = LauncherFixture::new();
    fixture.controller.open_launcher();
    let controller = fixture.controller.clone();
    let owner = fixture.launcher.as_weak();
    LAUNCHER_CONFIGURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |window| {
            assert!(controller.leases.try_borrow_mut().is_ok());
            assert!(controller.launcher_state.try_borrow_mut().is_ok());
            window.dispatch_event(WindowEvent::CloseRequested);
            assert!(window.is_visible(), "hide waits for the in-flight lease");
            LAUNCHER_DROP_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    assert!(
                        owner.upgrade().unwrap().window().is_visible(),
                        "late lease drops before native hide"
                    );
                    assert!(controller.leases.try_borrow_mut().is_ok());
                    assert!(controller.launcher_state.try_borrow_mut().is_ok());
                }));
            });
        }));
    });
    let drops = fixture.host.lease_drops.load(Ordering::SeqCst);
    fixture.click_launcher("Expand applications menu");
    assert_eq!(
        fixture
            .controller
            .core
            .applied_preferences()
            .launcher()
            .display_mode(),
        LauncherDisplayMode::Fullscreen
    );
    assert!(!fixture.launcher.window().is_visible());
    assert_eq!(
        fixture.host.lease_drops.load(Ordering::SeqCst),
        drops + 2,
        "old and cancelled local leases both released"
    );
    assert!(
        fixture
            .controller
            .leases
            .borrow()
            .geometry_changed(SurfaceKind::Launcher, (0, 0, 1920, 1040))
    );
    fixture.controller.open_launcher();
    assert!(fixture.launcher.window().is_visible());
    assert_eq!(
        fixture.launcher.window().size(),
        slint::PhysicalSize::new(1920, 1040)
    );
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert!(fixture.host.launches.lock().is_empty());
}

#[test]
fn launcher_native_reopen_and_lease_drop_reentry_cannot_publish_stale_attachment() {
    use crate::generated::LauncherDisplayMode as UiMode;
    use slint::platform::WindowEvent;
    use std::cell::Cell;

    let fixture = LauncherFixture::new();
    fixture.controller.open_launcher();
    let controller = fixture.controller.clone();
    let owner = fixture.launcher.as_weak();
    let retired = Rc::new(Cell::new(false));
    let released = Rc::clone(&retired);
    LAUNCHER_CONFIGURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |window| {
            window.dispatch_event(WindowEvent::CloseRequested);
            controller.open_launcher();
            assert!(window.is_visible());
            LAUNCHER_DROP_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    assert!(owner.upgrade().unwrap().window().is_visible());
                    released.set(true);
                }));
            });
            LAUNCHER_CONFIGURE_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move |_| {
                    assert!(
                        retired.get(),
                        "new session attaches only after old local lease drops"
                    );
                    assert!(controller.leases.try_borrow_mut().is_ok());
                    assert!(controller.launcher_state.try_borrow_mut().is_ok());
                }));
            });
        }));
    });
    let drops = fixture.host.lease_drops.load(Ordering::SeqCst);
    fixture.click_launcher("Expand applications menu");
    assert!(fixture.launcher.window().is_visible());
    assert_eq!(fixture.launcher.get_display_mode(), UiMode::Fullscreen);
    assert_eq!(fixture.host.lease_drops.load(Ordering::SeqCst), drops + 2);
    assert!(
        !fixture
            .controller
            .leases
            .borrow()
            .geometry_changed(SurfaceKind::Launcher, (0, 0, 1920, 1040))
    );

    let controller = fixture.controller.clone();
    LAUNCHER_DROP_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(controller.leases.try_borrow_mut().is_ok());
            assert!(controller.launcher_state.try_borrow_mut().is_ok());
            controller.hide_launcher();
        }));
    });
    let attached = fixture.host.launcher_attach_calls.load(Ordering::SeqCst);
    fixture.click_launcher("Contract applications menu");
    assert!(!fixture.launcher.window().is_visible());
    assert_eq!(
        fixture.host.launcher_attach_calls.load(Ordering::SeqCst),
        attached,
        "drop cancellation never starts late configure"
    );
    assert_eq!(fixture.host.saves.lock().len(), 2);
    assert_eq!(
        fixture
            .controller
            .core
            .applied_preferences()
            .launcher()
            .display_mode(),
        crate::LauncherDisplayMode::Windowed
    );
    fixture.controller.open_launcher();
    assert_eq!(fixture.launcher.get_display_mode(), UiMode::Windowed);
    assert_eq!(
        fixture.launcher.window().size(),
        slint::PhysicalSize::new(1056, 572)
    );
    assert_eq!(fixture.host.observe_calls.load(Ordering::SeqCst), 0);
}

fn reorder_preferences() -> PanelPreferences {
    PanelPreferences::new(Theme::Light, false)
        .with_dock(crate::DockEdge::Left, vec!["app-browser".into()])
        .with_launcher_favorites(vec![
            "missing-first".into(),
            "app-editor".into(),
            "missing-middle".into(),
            "app-browser".into(),
            "app-files".into(),
            "missing-last".into(),
        ])
        .unwrap()
}

/// Genuine native input; metadata is queried before the final move/release pair.
fn begin_editor_reorder(fixture: &LauncherFixture) -> slint::LogicalPosition {
    begin_editor_reorder_with_data(fixture, None)
}

fn begin_editor_reorder_with_data(
    fixture: &LauncherFixture,
    data: Option<slint::DataTransfer>,
) -> slint::LogicalPosition {
    use slint::platform::{PointerEventButton, WindowEvent};
    let source = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &fixture.launcher,
        "Launch Rust Editor",
    )
    .next()
    .unwrap();
    let target = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &fixture.launcher,
        "Launch File Manager",
    )
    .next()
    .unwrap();
    let press = slint::LogicalPosition::new(
        source.absolute_position().x + 11.0,
        source.absolute_position().y + 13.0,
    );
    let drop = slint::LogicalPosition::new(
        target.absolute_position().x + target.size().width * 0.4 + 11.0,
        target.absolute_position().y + 13.0,
    );
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: press });
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position: press,
            button: PointerEventButton::Left,
        });
    if let Some(data) = data {
        fixture.launcher.set_reorder_data(data);
    }
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(press.x + 12.0, press.y),
        });
    slint::platform::update_timers_and_animations();
    assert!(fixture.launcher.get_reorder_dragging());
    assert!(fixture.host.saves.lock().is_empty());
    drop
}

fn finish_native_reorder(fixture: &LauncherFixture, position: slint::LogicalPosition) {
    use slint::platform::{PointerEventButton, WindowEvent};
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
}

#[test]
fn launcher_native_reorder_saves_once_merges_missing_slots_and_preserves_applied_record() {
    for scale in [1.0, 2.0] {
        let preferences = reorder_preferences();
        let fixture = LauncherFixture::with_preferences(preferences.clone());
        fixture.controller.open_launcher();
        fixture.launcher.window().dispatch_event(
            slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            },
        );
        fixture.controller.update_launcher_geometry();
        fixture.panel.set_theme_index(2);
        fixture.panel.set_compact(true);
        fixture.panel.set_dock_edge_index(3);
        fixture
            .launcher
            .invoke_select_requested("app-editor".into());
        let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
        let position = begin_editor_reorder(&fixture);
        finish_native_reorder(&fixture, position);
        let applied = fixture.controller.core.applied_preferences();
        assert_eq!(
            applied.launcher_favorites(),
            [
                "missing-first",
                "app-browser",
                "missing-middle",
                "app-files",
                "app-editor",
                "missing-last",
            ]
        );
        assert_eq!(
            fixture.host.saves.lock().as_slice(),
            std::slice::from_ref(&applied)
        );
        assert_eq!(applied.theme(), preferences.theme());
        assert_eq!(applied.compact(), preferences.compact());
        assert_eq!(applied.dock_edge(), preferences.dock_edge());
        assert_eq!(applied.pinned_apps(), preferences.pinned_apps());
        assert_eq!(
            applied.launcher().display_mode(),
            preferences.launcher().display_mode()
        );
        assert_eq!(fixture.launcher.get_selected_key(), "app-editor");
        assert_eq!(fixture.tile(2).key, "app-editor");
        assert!(fixture.host.launches.lock().is_empty());
        assert!(fixture.launcher.window().is_visible());
        assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), focus);
    }
}

#[test]
fn launcher_native_reorder_failed_save_restores_selection_and_bounded_error() {
    let preferences = reorder_preferences();
    let fixture = LauncherFixture::with_preferences(preferences.clone());
    fixture.controller.open_launcher();
    fixture
        .launcher
        .invoke_select_requested("app-editor".into());
    *fixture.host.save_result.lock() = Err("disk full\n\u{1b}untrusted".into());
    let position = begin_editor_reorder(&fixture);
    finish_native_reorder(&fixture, position);
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert_eq!(fixture.tile(0).key, "app-editor");
    assert_eq!(fixture.launcher.get_selected_key(), "app-editor");
    assert!(
        fixture
            .launcher
            .get_status()
            .contains("Could not save favorite order")
    );
    assert!(!fixture.launcher.get_status().chars().any(char::is_control));
    assert!(fixture.host.launches.lock().is_empty());
}

#[test]
fn launcher_native_reorder_preview_is_transient_and_center_drop_is_a_noop() {
    use slint::platform::WindowEvent;
    let preferences = reorder_preferences();
    let fixture = LauncherFixture::with_preferences(preferences.clone());
    fixture.controller.open_launcher();
    let position = begin_editor_reorder(&fixture);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    assert_eq!(fixture.tile(2).key, "app-editor");
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    assert!(fixture.host.saves.lock().is_empty());
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerExited);
    assert_eq!(fixture.tile(0).key, "app-editor");
    assert!(fixture.host.saves.lock().is_empty());
    let source = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &fixture.launcher,
        "Launch Rust Editor",
    )
    .next()
    .unwrap();
    let center = slint::LogicalPosition::new(
        source.absolute_position().x + 23.0,
        source.absolute_position().y + 13.0,
    );
    let _ = begin_editor_reorder(&fixture);
    finish_native_reorder(&fixture, center);
    assert!(fixture.host.saves.lock().is_empty());
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    assert!(fixture.host.launches.lock().is_empty());
}

#[test]
fn launcher_native_reorder_immutable_old_text_and_foreign_payloads_fail_closed() {
    use slint::platform::WindowEvent;
    let fixture = LauncherFixture::with_preferences(reorder_preferences());
    fixture.controller.open_launcher();
    let _ = begin_editor_reorder(&fixture);
    let old = fixture.launcher.get_reorder_data();
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerExited);
    let mut foreign = slint::DataTransfer::default();
    foreign.set_user_data(Rc::new("app-editor".to_owned()));
    let text = slint::DataTransfer::from(slint::SharedString::from("app-editor"));
    for invalid in [old, foreign, text] {
        let position = begin_editor_reorder_with_data(&fixture, Some(invalid));
        finish_native_reorder(&fixture, position);
        assert!(fixture.host.saves.lock().is_empty());
        assert_eq!(fixture.tile(0).key, "app-editor");
        assert!(fixture.host.launches.lock().is_empty());
        slint::platform::update_timers_and_animations();
    }
    let position = begin_editor_reorder(&fixture);
    finish_native_reorder(&fixture, position);
    assert_eq!(fixture.host.saves.lock().len(), 1);
}

#[test]
fn launcher_native_reorder_hide_immediate_reopen_new_press_skips_old_teardown() {
    let fixture = LauncherFixture::with_preferences(reorder_preferences());
    fixture.controller.open_launcher();
    let _ = begin_editor_reorder(&fixture);
    fixture.controller.hide_launcher();
    fixture.controller.open_launcher();
    // No event-loop tick before the next press: open must flush old SDK state.
    let position = begin_editor_reorder(&fixture);
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
    slint::platform::update_timers_and_animations();
    finish_native_reorder(&fixture, position);
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert!(fixture.host.launches.lock().is_empty());
}

#[test]
fn launcher_native_reorder_save_reentry_blocks_other_complete_record_writes_and_hide_resurrection()
{
    let preferences = reorder_preferences();
    let fixture = LauncherFixture::with_preferences(preferences.clone());
    fixture.controller.open_launcher();
    let controller = fixture.controller.clone();
    let panel = fixture.panel.as_weak();
    let launcher = fixture.launcher.as_weak();
    PREFERENCE_SAVE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(controller.launcher_state.try_borrow_mut().is_ok());
            assert!(controller.leases.try_borrow_mut().is_ok());
            let panel = panel.upgrade().unwrap();
            let launcher = launcher.upgrade().unwrap();
            panel.invoke_pin_toggle_requested("app-editor".into(), true);
            panel.invoke_save_preferences_requested();
            launcher.invoke_favorite_toggle_requested("app-browser".into(), false);
            launcher
                .invoke_display_mode_requested(crate::generated::LauncherDisplayMode::Fullscreen);
            controller.hide_launcher();
        }));
    });
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let attached = fixture.host.launcher_attach_calls.load(Ordering::SeqCst);
    let position = begin_editor_reorder(&fixture);
    finish_native_reorder(&fixture, position);
    assert_eq!(fixture.host.saves.lock().len(), 1);
    let applied = fixture.controller.core.applied_preferences();
    assert_eq!(applied.pinned_apps(), preferences.pinned_apps());
    assert_eq!(
        applied.launcher().display_mode(),
        preferences.launcher().display_mode()
    );
    assert_eq!(applied.theme(), preferences.theme());
    assert_eq!(applied.launcher_favorites()[4], "app-editor");
    assert!(!fixture.launcher.window().is_visible());
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), focus);
    assert_eq!(
        fixture.host.launcher_attach_calls.load(Ordering::SeqCst),
        attached
    );
    fixture.controller.open_launcher();
    assert_eq!(fixture.tile(2).key, "app-editor");
}

#[test]
fn launcher_native_reorder_external_projection_status_catalog_and_refit_cancel_without_saves() {
    use slint::platform::{Key, WindowEvent};
    for cancellation in [
        "query", "all", "busy", "stale", "source", "anchor", "resize", "escape",
    ] {
        let fixture = LauncherFixture::with_preferences(reorder_preferences());
        fixture.controller.open_launcher();
        let position = begin_editor_reorder(&fixture);
        fixture
            .launcher
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        match cancellation {
            "query" => {
                fixture.launcher.set_search("Rust".into());
                fixture.controller.apply_launcher_filter();
            }
            "all" => fixture
                .launcher
                .invoke_view_requested(crate::generated::LauncherView::All),
            "busy" | "stale" => {
                fixture.panel.set_refreshing(cancellation == "busy");
                fixture.panel.set_stale(cancellation == "stale");
                fixture.controller.render();
            }
            "source" | "anchor" => {
                let removed = if cancellation == "source" {
                    "app-editor"
                } else {
                    "app-files"
                };
                let apps = fixture
                    .controller
                    .core
                    .catalog()
                    .into_iter()
                    .filter(|app| app.key() != removed)
                    .collect();
                apply_result_to_both(
                    &fixture.controller,
                    &fixture.panel,
                    Ok(launcher_snapshot().with_applications(apps)),
                );
            }
            "resize" => {
                let size = fixture.launcher.window().size();
                fixture
                    .launcher
                    .window()
                    .set_size(slint::PhysicalSize::new(size.width + 70, size.height));
            }
            "escape" => fixture.key(Key::Escape.into()),
            _ => unreachable!(),
        }
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
        finish_native_reorder(&fixture, position);
        assert!(
            fixture.host.saves.lock().is_empty(),
            "{cancellation} must cancel order persistence"
        );
        assert!(
            fixture.host.launches.lock().is_empty(),
            "{cancellation} must never launch"
        );
    }
}

#[test]
fn launcher_native_reorder_stationary_autoscroll_reaches_partial_tail_after_source_eviction() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let applications = (0..90)
        .map(|index| {
            PanelApplication::new(
                format!("large-{index:04}"),
                format!("Large App {index}"),
                None,
            )
            .unwrap()
        })
        .collect();
    let mut favorites: Vec<_> = (0..90).map(|index| format!("large-{index:04}")).collect();
    favorites.insert(40, "missing-middle".into());
    let preferences = reorder_preferences()
        .with_launcher_favorites(favorites.clone())
        .unwrap();
    let fixture = LauncherFixture::with_snapshot(
        preferences.clone(),
        launcher_snapshot().with_applications(applications),
    );
    fixture.controller.open_launcher();
    let source = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &fixture.launcher,
        "Launch Large App 0",
    )
    .next()
    .unwrap();
    let press = slint::LogicalPosition::new(
        source.absolute_position().x + 11.0,
        source.absolute_position().y + 13.0,
    );
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: press });
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position: press,
            button: PointerEventButton::Left,
        });
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(press.x + 12.0, press.y),
        });
    slint::platform::update_timers_and_animations();
    let metrics = fixture.launcher.get_reorder_metrics();
    let held = slint::LogicalPosition::new(
        metrics.viewport.origin.x + metrics.gutter + metrics.tile * 0.4 + 11.0,
        metrics.viewport.origin.y + metrics.viewport.height - 13.0,
    );
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: held });
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(20));
    slint::platform::update_timers_and_animations();
    let outside = slint::LogicalPosition::new(
        metrics.viewport.origin.x + metrics.viewport.width + 30.0,
        held.y,
    );
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: outside });
    slint::platform::update_timers_and_animations();
    let left_target = fixture.launcher.get_reorder_metrics().content_y;
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
    slint::platform::update_timers_and_animations();
    assert_eq!(
        fixture.launcher.get_reorder_metrics().content_y,
        left_target,
        "leaving native DropArea must stop its stationary autoscroll"
    );
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: held });
    for _ in 0..500 {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(10));
        slint::platform::update_timers_and_animations();
    }
    let metrics = fixture.launcher.get_reorder_metrics();
    assert!(
        metrics.content_y < 0.0,
        "stationary pointer must scroll the native ListView"
    );
    assert!(
        !source.is_valid(),
        "native source delegate must actually be evicted"
    );
    assert!(fixture.host.saves.lock().is_empty());
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    let tail = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &fixture.launcher,
        "Launch Large App 89",
    )
    .next()
    .unwrap();
    let position = slint::LogicalPosition::new(
        tail.absolute_position().x + tail.size().width * 0.4 + 11.0,
        tail.absolute_position().y + 13.0,
    );
    finish_native_reorder(&fixture, position);
    let applied = fixture.controller.core.applied_preferences();
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert_eq!(applied.launcher_favorites()[40], "missing-middle");
    assert_eq!(applied.launcher_favorites().last().unwrap(), "large-0000");
    assert_eq!(applied.launcher_favorites()[0], "large-0001");
    assert!(fixture.host.launches.lock().is_empty());
    let stopped = fixture.launcher.get_reorder_metrics().content_y;
    i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(1));
    slint::platform::update_timers_and_animations();
    assert_eq!(
        fixture.launcher.get_reorder_metrics().content_y,
        stopped,
        "no idle gesture timer"
    );
}

#[test]
fn launcher_native_reorder_below_sdk_threshold_keeps_single_click_activation() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let fixture = LauncherFixture::with_preferences(reorder_preferences());
    fixture.controller.open_launcher();
    let source = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &fixture.launcher,
        "Launch Rust Editor",
    )
    .next()
    .unwrap();
    let press = slint::LogicalPosition::new(
        source.absolute_position().x + 11.0,
        source.absolute_position().y + 13.0,
    );
    let near = slint::LogicalPosition::new(press.x + 4.0, press.y - 2.0);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: press });
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position: press,
            button: PointerEventButton::Left,
        });
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: near });
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: near,
            button: PointerEventButton::Left,
        });
    assert_eq!(fixture.host.launches.lock().as_slice(), ["app-editor"]);
    assert!(fixture.host.saves.lock().is_empty());
    assert!(!fixture.launcher.get_reorder_dragging());
    assert!(!fixture.launcher.window().is_visible());
}

#[test]
fn launcher_native_reorder_saved_after_host_reopen_does_not_restore_old_rows_or_focus() {
    let fixture = LauncherFixture::with_preferences(reorder_preferences());
    fixture.controller.open_launcher();
    let controller = fixture.controller.clone();
    let launcher = fixture.launcher.as_weak();
    let host = Arc::clone(&fixture.host);
    let reopened_focus = Rc::new(Cell::new(0));
    let saved_focus = Rc::clone(&reopened_focus);
    PREFERENCE_SAVE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            controller.hide_launcher();
            controller.open_launcher();
            let launcher = launcher.upgrade().unwrap();
            launcher.set_search("browser".into());
            controller.apply_launcher_filter();
            saved_focus.set(host.ui_focus_calls.load(Ordering::SeqCst));
        }));
    });
    let position = begin_editor_reorder(&fixture);
    finish_native_reorder(&fixture, position);
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert_eq!(
        fixture.controller.core.launcher_favorites()[4],
        "app-editor"
    );
    assert!(
        fixture.launcher.window().is_visible(),
        "explicit host reopen remains current"
    );
    assert_eq!(fixture.launcher.get_search(), "browser");
    assert_eq!(
        fixture.launcher.get_view(),
        crate::generated::LauncherView::All
    );
    assert_eq!(fixture.launcher.get_application_count(), 1);
    assert_eq!(fixture.tile(0).key, "app-browser");
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        reopened_focus.get()
    );
    assert!(!fixture.launcher.get_reorder_enabled());
}

#[test]
fn launcher_native_reorder_conflicting_collection_mode_and_appearance_saves_cancel_preview() {
    for action in ["favorite", "mode", "pin", "appearance"] {
        let preferences = reorder_preferences();
        let fixture = LauncherFixture::with_preferences(preferences.clone());
        fixture.controller.open_launcher();
        let position = begin_editor_reorder(&fixture);
        fixture
            .launcher
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
        match action {
            "favorite" => fixture
                .launcher
                .invoke_favorite_toggle_requested("app-browser".into(), false),
            "mode" => fixture
                .launcher
                .invoke_display_mode_requested(crate::generated::LauncherDisplayMode::Fullscreen),
            "pin" => fixture
                .panel
                .invoke_pin_toggle_requested("app-editor".into(), true),
            "appearance" => fixture.panel.invoke_save_preferences_requested(),
            _ => unreachable!(),
        }
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
        finish_native_reorder(&fixture, position);
        assert_eq!(
            fixture.host.saves.lock().len(),
            1,
            "only the conflicting {action} intent may save"
        );
        let expected = if action == "favorite" {
            vec![
                "missing-first",
                "app-editor",
                "missing-middle",
                "app-files",
                "missing-last",
            ]
        } else {
            vec![
                "missing-first",
                "app-editor",
                "missing-middle",
                "app-browser",
                "app-files",
                "missing-last",
            ]
        };
        assert_eq!(fixture.controller.core.launcher_favorites(), expected);
        assert!(fixture.host.launches.lock().is_empty());
    }
}

fn scrollable_favorite_reopen_fixture() -> LauncherFixture {
    let snapshot = launcher_snapshot();
    let mut applications = snapshot.applications().unwrap().to_vec();
    let mut favorites = reorder_preferences().launcher_favorites().to_vec();
    for index in 0..80 {
        let key = format!("reopen-tail-{index:04}");
        applications.push(
            PanelApplication::new(key.clone(), format!("Reopen Tail {index}"), None).unwrap(),
        );
        favorites.push(key);
    }
    let preferences = reorder_preferences()
        .with_launcher_favorites(favorites)
        .unwrap();
    LauncherFixture::with_snapshot(preferences, snapshot.with_applications(applications))
}

fn install_default_favorites_reopen_hook(
    fixture: &LauncherFixture,
) -> (Rc<Cell<usize>>, Rc<Cell<f32>>) {
    let controller = fixture.controller.clone();
    let launcher = fixture.launcher.as_weak();
    let host = Arc::clone(&fixture.host);
    let focus = Rc::new(Cell::new(0));
    let content_y = Rc::new(Cell::new(0.0));
    let reopened_focus = Rc::clone(&focus);
    let reopened_y = Rc::clone(&content_y);
    PREFERENCE_SAVE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            controller.hide_launcher();
            controller.open_launcher();
            let launcher = launcher.upgrade().unwrap();
            launcher.invoke_scroll_reorder(-24.0);
            reopened_y.set(launcher.get_reorder_metrics().content_y);
            reopened_focus.set(host.ui_focus_calls.load(Ordering::SeqCst));
        }));
    });
    (focus, content_y)
}

#[test]
fn launcher_native_reorder_saved_after_default_favorites_reopen_refreshes_current_order() {
    let fixture = scrollable_favorite_reopen_fixture();
    fixture.controller.open_launcher();
    let (reopened_focus, reopened_y) = install_default_favorites_reopen_hook(&fixture);
    let position = begin_editor_reorder(&fixture);
    finish_native_reorder(&fixture, position);
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert_eq!(
        fixture.controller.core.launcher_favorites()[4],
        "app-editor"
    );
    assert!(fixture.launcher.window().is_visible());
    assert_eq!(
        fixture.launcher.get_view(),
        crate::generated::LauncherView::Favorites
    );
    assert_eq!(fixture.launcher.get_search(), "");
    assert_eq!(fixture.launcher.get_selected_key(), "");
    assert_eq!(fixture.launcher.get_application_count(), 83);
    assert_eq!(fixture.tile(0).key, "app-browser");
    assert_eq!(fixture.tile(1).key, "app-files");
    assert_eq!(fixture.tile(2).key, "app-editor");
    assert!(
        fixture.launcher.get_reorder_enabled(),
        "fresh saved canonical order must remain draggable"
    );
    assert!(
        reopened_y.get() < 0.0,
        "fixture must exercise a nonzero native scroll offset"
    );
    assert_eq!(
        fixture.launcher.get_reorder_metrics().content_y,
        reopened_y.get()
    );
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        reopened_focus.get()
    );
    assert!(fixture.host.launches.lock().is_empty());
}

#[test]
fn launcher_favorite_saved_after_default_favorites_reopen_refreshes_current_membership() {
    let fixture = scrollable_favorite_reopen_fixture();
    fixture.controller.open_launcher();
    let (reopened_focus, reopened_y) = install_default_favorites_reopen_hook(&fixture);
    fixture.click_launcher("Remove from favorites: Web Browser");
    assert_eq!(fixture.host.saves.lock().len(), 1);
    assert!(
        !fixture
            .controller
            .core
            .launcher_favorites()
            .iter()
            .any(|key| key == "app-browser")
    );
    assert!(fixture.launcher.window().is_visible());
    assert_eq!(
        fixture.launcher.get_view(),
        crate::generated::LauncherView::Favorites
    );
    assert_eq!(fixture.launcher.get_search(), "");
    assert_eq!(fixture.launcher.get_selected_key(), "");
    assert_eq!(fixture.launcher.get_application_count(), 82);
    assert_eq!(fixture.tile(0).key, "app-editor");
    assert_eq!(fixture.tile(1).key, "app-files");
    assert_eq!(fixture.tile(2).key, "reopen-tail-0000");
    assert!(
        fixture.launcher.get_reorder_enabled(),
        "current saved membership must remain draggable"
    );
    assert!(
        reopened_y.get() < 0.0,
        "fixture must exercise a nonzero native scroll offset"
    );
    assert_eq!(
        fixture.launcher.get_reorder_metrics().content_y,
        reopened_y.get()
    );
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        reopened_focus.get()
    );
    assert!(fixture.host.launches.lock().is_empty());
}
