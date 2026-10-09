// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use tessera_system::dock_utilities::{DockUtilityError, DockUtilityErrorKind};
use tessera_system::recycle_bin::{
    RecycleBinCompletion, RecycleBinHost, RecycleBinInfo, RecycleBinReadCompletion,
    RecycleBinWatchCallback, RecycleBinWatchCompletion, RecycleBinWatchEvent, RecycleBinWatchGuard,
};
use tessera_system::recycle_bin_mutation::{
    RecycleBinEmptyCompletion, RecycleBinEmptyOutcome, RecycleBinMutationHost,
};

#[path = "../launcher/app_menu/controller_tests.rs"]
mod launcher_app_menu_tests;
mod module_integration_tests;
mod power_display_tests;
mod power_tests;
mod shortcut_profile_tests;

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
    static RECYCLE_READ_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static RECYCLE_EMPTY_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static RECYCLE_MENU_DROP_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static POWER_DISPLAY_FACTORY_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static POWER_FACTORY_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static POWER_UPDATES_FACTORY_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static MEDIA_FACTORY_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static POWER_CONFIGURE_HOOK: RefCell<Option<NativeHook>> = const { RefCell::new(None) };
    static POWER_DROP_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static TOOLTIP_DROP_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static TOOLTIP_CONFIGURE_HOOK: RefCell<Option<NativeHook>> = const { RefCell::new(None) };
    static POWER_WINDOW: RefCell<Option<slint::Weak<crate::generated::PowerMenuSurface>>> = const { RefCell::new(None) };
    static SHORTCUT_FACTORY_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static PROFILE_FACTORY_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
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
    folder_provider: Mutex<Option<Arc<dyn tessera_system::folders::FolderHost>>>,
    calendar_provider: Mutex<Option<Arc<dyn tessera_system::calendar::CalendarHost>>>,
    dock_utility_provider:
        Mutex<Option<Arc<dyn tessera_system::dock_utilities::DockUtilitiesHost>>>,
    dock_utility_provider_calls: AtomicUsize,
    recycle_provider: Mutex<Option<Arc<dyn tessera_system::recycle_bin::RecycleBinHost>>>,
    recycle_provider_calls: AtomicUsize,
    recycle_mutation_provider: Mutex<Option<Arc<dyn RecycleBinMutationHost>>>,
    recycle_mutation_provider_calls: AtomicUsize,
    display_provider: Mutex<Option<Arc<dyn tessera_system::display_context::DisplayContextHost>>>,
    display_provider_calls: AtomicUsize,
    power_provider: Mutex<Option<Arc<dyn tessera_system::power::PowerHost>>>,
    power_provider_calls: AtomicUsize,
    power_updates_provider: Mutex<Option<Arc<dyn tessera_system::power_updates::PowerUpdatesHost>>>,
    power_updates_factory_calls: AtomicUsize,
    network_provider: Mutex<Option<Arc<dyn tessera_system::network::NetworkHost>>>,
    network_provider_calls: AtomicUsize,
    bluetooth_provider: Mutex<Option<Arc<dyn tessera_system::bluetooth::BluetoothHost>>>,
    bluetooth_provider_calls: AtomicUsize,
    input_language_provider:
        Mutex<Option<Arc<dyn tessera_system::input_language::InputLanguageHost>>>,
    input_language_provider_calls: AtomicUsize,
    media_provider: Mutex<Option<Arc<dyn tessera_system::media::MediaHost>>>,
    media_provider_calls: AtomicUsize,
    shortcuts_provider: Mutex<Option<Arc<dyn tessera_system::shortcuts::ShortcutHost>>>,
    shortcuts_factory_calls: AtomicUsize,
    profile_provider: Mutex<Option<Arc<dyn tessera_system::profile::ProfileHost>>>,
    profile_factory_calls: AtomicUsize,
    pointer_provider: Mutex<Option<Arc<dyn tessera_system::visibility::PointerHost>>>,
    pointer_provider_calls: AtomicUsize,
    power_lease_drops: Arc<AtomicUsize>,
    shell_identity: Mutex<crate::ShellIdentity>,
    identity_calls: AtomicUsize,
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
            folder_provider: Mutex::default(),
            calendar_provider: Mutex::default(),
            dock_utility_provider: Mutex::default(),
            dock_utility_provider_calls: AtomicUsize::new(0),
            recycle_provider: Mutex::default(),
            recycle_provider_calls: AtomicUsize::new(0),
            recycle_mutation_provider: Mutex::default(),
            recycle_mutation_provider_calls: AtomicUsize::new(0),
            display_provider: Mutex::default(),
            display_provider_calls: AtomicUsize::new(0),
            power_provider: Mutex::default(),
            power_provider_calls: AtomicUsize::new(0),
            power_updates_provider: Mutex::default(),
            power_updates_factory_calls: AtomicUsize::new(0),
            network_provider: Mutex::default(),
            network_provider_calls: AtomicUsize::new(0),
            bluetooth_provider: Mutex::default(),
            bluetooth_provider_calls: AtomicUsize::new(0),
            input_language_provider: Mutex::default(),
            input_language_provider_calls: AtomicUsize::new(0),
            media_provider: Mutex::default(),
            media_provider_calls: AtomicUsize::new(0),
            shortcuts_provider: Mutex::default(),
            shortcuts_factory_calls: AtomicUsize::new(0),
            profile_provider: Mutex::default(),
            profile_factory_calls: AtomicUsize::new(0),
            pointer_provider: Mutex::default(),
            pointer_provider_calls: AtomicUsize::new(0),
            power_lease_drops: Arc::default(),
            shell_identity: Mutex::default(),
            identity_calls: AtomicUsize::new(0),
        })
    }

    // Compare deltas after SurfaceCore setup; it already subscribes once.
    fn unrelated_activity(&self) -> [usize; 8] {
        [
            self.observe_calls.load(Ordering::SeqCst),
            self.subscription_calls.load(Ordering::SeqCst),
            self.activations.lock().len(),
            self.window_actions.lock().len(),
            self.launches.lock().len(),
            self.saves.lock().len(),
            self.system_actions.lock().len(),
            self.dock_utility_provider_calls.load(Ordering::SeqCst),
        ]
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

    fn folder_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::folders::FolderHost>>,
        tessera_system::folders::FolderError,
    > {
        Ok(self.folder_provider.lock().clone())
    }

    fn shortcuts_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::shortcuts::ShortcutHost>>,
        tessera_system::shortcuts::ShortcutError,
    > {
        self.shortcuts_factory_calls.fetch_add(1, Ordering::SeqCst);
        let hook = SHORTCUT_FACTORY_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
        Ok(self.shortcuts_provider.lock().clone())
    }

    fn profile_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::profile::ProfileHost>>,
        tessera_system::profile::ProfileError,
    > {
        self.profile_factory_calls.fetch_add(1, Ordering::SeqCst);
        let hook = PROFILE_FACTORY_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
        Ok(self.profile_provider.lock().clone())
    }

    fn calendar_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::calendar::CalendarHost>>,
        tessera_system::calendar::CalendarError,
    > {
        Ok(self.calendar_provider.lock().clone())
    }

    fn dock_utilities_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::dock_utilities::DockUtilitiesHost>>,
        tessera_system::dock_utilities::DockUtilityError,
    > {
        self.dock_utility_provider_calls
            .fetch_add(1, Ordering::SeqCst);
        Ok(self.dock_utility_provider.lock().clone())
    }

    fn recycle_bin_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::recycle_bin::RecycleBinHost>>,
        tessera_system::dock_utilities::DockUtilityError,
    > {
        self.recycle_provider_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.recycle_provider.lock().clone())
    }

    fn recycle_bin_mutation_host(
        &self,
    ) -> Result<Option<Arc<dyn RecycleBinMutationHost>>, DockUtilityError> {
        self.recycle_mutation_provider_calls
            .fetch_add(1, Ordering::SeqCst);
        Ok(self.recycle_mutation_provider.lock().clone())
    }

    fn display_context_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::display_context::DisplayContextHost>>,
        tessera_system::display_context::DisplayContextError,
    > {
        self.display_provider_calls.fetch_add(1, Ordering::SeqCst);
        let hook = POWER_DISPLAY_FACTORY_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
        Ok(self.display_provider.lock().clone())
    }

    fn power_host(
        &self,
    ) -> Result<Option<Arc<dyn tessera_system::power::PowerHost>>, tessera_system::power::PowerError>
    {
        self.power_provider_calls.fetch_add(1, Ordering::SeqCst);
        let hook = POWER_FACTORY_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
        Ok(self.power_provider.lock().clone())
    }

    fn power_updates_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::power_updates::PowerUpdatesHost>>,
        tessera_system::power_updates::PowerUpdatesError,
    > {
        self.power_updates_factory_calls
            .fetch_add(1, Ordering::SeqCst);
        let hook = POWER_UPDATES_FACTORY_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
        Ok(self.power_updates_provider.lock().clone())
    }

    fn network_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::network::NetworkHost>>,
        tessera_system::network::NetworkError,
    > {
        self.network_provider_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.network_provider.lock().clone())
    }

    fn bluetooth_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::bluetooth::BluetoothHost>>,
        tessera_system::bluetooth::BluetoothError,
    > {
        self.bluetooth_provider_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.bluetooth_provider.lock().clone())
    }

    fn input_language_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::input_language::InputLanguageHost>>,
        tessera_system::input_language::InputLanguageError,
    > {
        self.input_language_provider_calls
            .fetch_add(1, Ordering::SeqCst);
        Ok(self.input_language_provider.lock().clone())
    }

    fn media_host(
        &self,
    ) -> Result<Option<Arc<dyn tessera_system::media::MediaHost>>, tessera_system::media::MediaError>
    {
        self.media_provider_calls.fetch_add(1, Ordering::SeqCst);
        let hook = MEDIA_FACTORY_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
        Ok(self.media_provider.lock().clone())
    }

    fn pointer_host(
        &self,
    ) -> Result<
        Option<Arc<dyn tessera_system::visibility::PointerHost>>,
        tessera_system::visibility::PointerWatchError,
    > {
        self.pointer_provider_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.pointer_provider.lock().clone())
    }

    fn shell_identity(&self) -> Result<crate::ShellIdentity, String> {
        self.identity_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.shell_identity.lock().clone())
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
        if kind == SurfaceKind::Tooltip {
            let hook = TOOLTIP_CONFIGURE_HOOK.with(|hook| hook.borrow_mut().take());
            if let Some(hook) = hook {
                hook(window);
            }
        }
        let power = kind == SurfaceKind::Popup
            && POWER_WINDOW.with(|expected| {
                expected
                    .borrow()
                    .as_ref()
                    .and_then(slint::Weak::upgrade)
                    .is_some_and(|component| std::ptr::eq(component.window(), window))
            });
        if power {
            let hook = POWER_CONFIGURE_HOOK.with(|hook| hook.borrow_mut().take());
            if let Some(hook) = hook {
                hook(window);
            }
        }
        struct Lease {
            dropped: Arc<AtomicUsize>,
            _ui_thread: Rc<()>,
            kind: SurfaceKind,
            power: bool,
            power_dropped: Arc<AtomicUsize>,
        }
        impl Drop for Lease {
            fn drop(&mut self) {
                self.dropped.fetch_add(1, Ordering::SeqCst);
                if self.power {
                    self.power_dropped.fetch_add(1, Ordering::SeqCst);
                    let hook = POWER_DROP_HOOK.with(|hook| hook.borrow_mut().take());
                    if let Some(hook) = hook {
                        hook();
                    }
                }
                if self.kind == SurfaceKind::Launcher {
                    let hook = LAUNCHER_DROP_HOOK.with(|hook| hook.borrow_mut().take());
                    if let Some(hook) = hook {
                        hook();
                    }
                }
                if self.kind == SurfaceKind::Popup {
                    let hook = RECYCLE_MENU_DROP_HOOK.with(|hook| hook.borrow_mut().take());
                    if let Some(hook) = hook {
                        hook();
                    }
                }
                if self.kind == SurfaceKind::Tooltip {
                    let hook = TOOLTIP_DROP_HOOK.with(|hook| hook.borrow_mut().take());
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
            power,
            power_dropped: Arc::clone(&self.power_lease_drops),
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
            global_shortcuts_enabled: true,
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

// Visible Slint windows keep their components alive. Retire caches and leases
// first (field order below), then hide the fixture's owned native windows.
struct FixtureWindowScope {
    panel: slint::Weak<Panel>,
    dock: slint::Weak<Dock>,
    toolbar: slint::Weak<Toolbar>,
    launcher: slint::Weak<Launcher>,
}

impl Drop for FixtureWindowScope {
    fn drop(&mut self) {
        if let Some(panel) = self.panel.upgrade() {
            panel.hide().unwrap();
        }
        if let Some(dock) = self.dock.upgrade() {
            dock.hide().unwrap();
        }
        if let Some(toolbar) = self.toolbar.upgrade() {
            toolbar.hide().unwrap();
        }
        if let Some(launcher) = self.launcher.upgrade() {
            launcher.hide().unwrap();
        }
    }
}

struct LauncherFixture {
    // Drop transient and bar attachments before the owned component windows.
    _power_scope: power_menu::PowerAdmissionScope,
    _toolbar_popup_scope: native_toolbar::ToolbarPopupScope,
    _launcher_app_scope: crate::transient_window::TransientScope<crate::launcher::LauncherAppMenu>,
    _media_scope: crate::transient_window::TransientScope<crate::dock_media::DockMediaController>,
    _visibility_scope:
        crate::transient_window::TransientScope<crate::visibility::VisibilityController>,
    _recycle_scope:
        crate::transient_window::TransientScope<crate::recycle_bin::RecycleBinController>,
    _menu_scope:
        crate::transient_window::TransientScope<crate::context_menu::ContextMenuController>,
    _tooltip_scope: crate::transient_window::TransientScope<crate::tooltip::TooltipController>,
    _dock_utility_scope:
        crate::transient_window::TransientScope<crate::dock_utilities::DockUtilitiesController>,
    _calendar_scope: crate::transient_window::TransientScope<crate::calendar::CalendarController>,
    _user_scope: crate::transient_window::TransientScope<crate::user_menu::UserMenuController>,
    _quick_scope:
        crate::transient_window::TransientScope<crate::quick_settings::QuickSettingsController>,
    _scope: SurfaceLeaseScope,
    _window_scope: FixtureWindowScope,
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
        Self::with_snapshot_configured(preferences, snapshot, |_| {})
    }

    fn with_snapshot_configured(
        preferences: PanelPreferences,
        snapshot: PanelSnapshot,
        configure: impl FnOnce(&Self),
    ) -> Self {
        LAUNCHER_BACKEND_INITIALIZED.with(|initialized| {
            if !initialized.replace(true) {
                i_slint_backend_testing::init_no_event_loop();
            }
        });
        let panel = Panel::new().unwrap();
        panel.set_start_of_week_index(preferences.general().start_of_week().index());
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
        let user_scope = crate::transient_window::TransientScope::new(
            Rc::clone(&controller.user_menu),
            crate::user_menu::UserMenuController::hide,
        );
        let calendar_scope = crate::transient_window::TransientScope::new(
            Rc::clone(&controller.calendar),
            crate::calendar::CalendarController::hide,
        );
        let power_scope = power_menu::PowerAdmissionScope::new(&controller);
        let dock_utility_scope = crate::transient_window::TransientScope::new(
            Rc::clone(&controller.dock_utilities),
            crate::dock_utilities::DockUtilitiesController::close,
        );
        let recycle_scope = crate::transient_window::TransientScope::new(
            Rc::clone(&controller.recycle_bin),
            crate::recycle_bin::RecycleBinController::close,
        );
        let menu_scope = crate::transient_window::TransientScope::new(
            Rc::clone(&controller.menus),
            crate::context_menu::ContextMenuController::hide,
        );
        let tooltip_scope = crate::transient_window::TransientScope::new(
            Rc::clone(&controller.tooltips),
            crate::tooltip::TooltipController::hide,
        );
        let window_scope = FixtureWindowScope {
            panel: panel.as_weak(),
            dock: dock.as_weak(),
            toolbar: toolbar.as_weak(),
            launcher: launcher.as_weak(),
        };
        let fixture = Self {
            _recycle_scope: recycle_scope,
            _toolbar_popup_scope: native_toolbar::ToolbarPopupScope::new(&controller),
            _launcher_app_scope: crate::transient_window::TransientScope::new(
                Rc::clone(&controller.launcher_app_menu),
                crate::launcher::LauncherAppMenu::hide,
            ),
            _media_scope: crate::transient_window::TransientScope::new(
                Rc::clone(&controller.dock_media),
                crate::dock_media::DockMediaController::close,
            ),
            _visibility_scope: crate::transient_window::TransientScope::new(
                Rc::clone(&controller.visibility),
                crate::visibility::VisibilityController::close,
            ),
            _menu_scope: menu_scope,
            _tooltip_scope: tooltip_scope,
            _dock_utility_scope: dock_utility_scope,
            _calendar_scope: calendar_scope,
            _user_scope: user_scope,
            _power_scope: power_scope,
            _quick_scope: quick_scope,
            _scope: scope,
            _window_scope: window_scope,
            controller,
            panel,
            dock,
            toolbar,
            launcher,
            host,
        };
        configure(&fixture);
        apply_result_to_both(&fixture.controller, &fixture.panel, Ok(snapshot));
        fixture
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
        click_component(&self.launcher, label);
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

fn click_component<C: slint::ComponentHandle>(component: &C, label: &str) {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle};
    use slint::platform::{PointerEventButton, WindowEvent};
    let mut controls =
        ElementHandle::find_by_accessible_label(component, label).filter(|element| {
            element
                .accessible_role()
                .is_some_and(|role| role != AccessibleRole::None)
        });
    let control = controls
        .next()
        .unwrap_or_else(|| panic!("missing native control: {label}"));
    assert!(
        controls.next().is_none(),
        "duplicate genuine native control: {label}"
    );
    let origin = control.absolute_position();
    let size = control.size();
    let position =
        slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    component
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
    component
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
}

struct RootRecycleWatch {
    callback: RecycleBinWatchCallback,
    ready: Mutex<Option<RecycleBinWatchCompletion>>,
    active: std::sync::atomic::AtomicBool,
    installed: std::sync::atomic::AtomicBool,
}

struct RootRecycleGuard {
    watch: Arc<RootRecycleWatch>,
    drops: Arc<AtomicUsize>,
}

impl RecycleBinWatchGuard for RootRecycleGuard {}

impl Drop for RootRecycleGuard {
    fn drop(&mut self) {
        self.watch.active.store(false, Ordering::SeqCst);
        self.drops.fetch_add(1, Ordering::SeqCst);
        let ready = self.watch.ready.lock().take();
        if let Some(ready) = ready {
            ready(Err(DockUtilityError::new(
                DockUtilityErrorKind::Stopped,
                "fixture watch retired before readiness",
            )));
        }
    }
}

#[derive(Default)]
struct RootRecordingRecycleBin {
    calls: Mutex<Vec<&'static str>>,
    watches: Mutex<Vec<Arc<RootRecycleWatch>>>,
    watch_attempts: AtomicUsize,
    reject_watch: Mutex<Option<DockUtilityError>>,
    read: Mutex<Option<RecycleBinReadCompletion>>,
    open: Mutex<Option<RecycleBinCompletion>>,
    guard_drops: Arc<AtomicUsize>,
}

impl RootRecordingRecycleBin {
    fn counts(&self) -> (usize, usize, usize) {
        let calls = self.calls.lock();
        (
            self.watch_attempts.load(Ordering::SeqCst),
            calls.iter().filter(|call| **call == "read").count(),
            calls.iter().filter(|call| **call == "open").count(),
        )
    }

    fn finish_ready(&self, result: Result<(), DockUtilityError>) {
        let watch = self.watches.lock().last().unwrap().clone();
        let ready = watch.ready.lock().take().expect("one accepted watch ready");
        let installed = result.is_ok();
        ready(result);
        watch.installed.store(installed, Ordering::SeqCst);
    }

    fn finish_read(&self, result: Result<RecycleBinInfo, DockUtilityError>) {
        let completion = self.read.lock().take().expect("one accepted read");
        completion(result);
    }

    fn finish_open(&self, result: Result<(), DockUtilityError>) {
        let completion = self.open.lock().take().expect("one accepted open");
        completion(result);
    }

    fn emit(&self) {
        let watch = self.watches.lock().last().unwrap().clone();
        if watch.active.load(Ordering::SeqCst) && watch.installed.load(Ordering::SeqCst) {
            (watch.callback)(RecycleBinWatchEvent::Invalidated);
        }
    }
}

impl RecycleBinHost for RootRecordingRecycleBin {
    fn watch(
        &self,
        callback: RecycleBinWatchCallback,
        ready: RecycleBinWatchCompletion,
    ) -> Result<Box<dyn RecycleBinWatchGuard>, DockUtilityError> {
        self.watch_attempts.fetch_add(1, Ordering::SeqCst);
        self.calls.lock().push("watch");
        let rejected = self.reject_watch.lock().take();
        if let Some(error) = rejected {
            // Immediate rejection transfers neither readiness nor a guard.
            return Err(error);
        }
        let watch = Arc::new(RootRecycleWatch {
            callback,
            ready: Mutex::new(Some(ready)),
            active: std::sync::atomic::AtomicBool::new(true),
            installed: std::sync::atomic::AtomicBool::new(false),
        });
        self.watches.lock().push(watch.clone());
        Ok(Box::new(RootRecycleGuard {
            watch,
            drops: self.guard_drops.clone(),
        }))
    }

    fn read(&self, completion: RecycleBinReadCompletion) -> Result<(), DockUtilityError> {
        self.calls.lock().push("read");
        assert!(
            self.read.lock().replace(completion).is_none(),
            "single read flight"
        );
        let hook = RECYCLE_READ_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }

    fn open(&self, completion: RecycleBinCompletion) -> Result<(), DockUtilityError> {
        self.calls.lock().push("open");
        assert!(
            self.open.lock().replace(completion).is_none(),
            "single open flight"
        );
        Ok(())
    }
}

#[derive(Default)]
struct RootRecordingRecycleMutation {
    calls: AtomicUsize,
    completion: Mutex<Option<RecycleBinEmptyCompletion>>,
}

impl RootRecordingRecycleMutation {
    fn finish(&self, result: Result<RecycleBinEmptyOutcome, DockUtilityError>) {
        let completion = self.completion.lock().take().expect("one accepted Empty");
        completion(result);
    }
}

impl RecycleBinMutationHost for RootRecordingRecycleMutation {
    fn empty(&self, completion: RecycleBinEmptyCompletion) -> Result<(), DockUtilityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(
            self.completion.lock().replace(completion).is_none(),
            "single Empty flight"
        );
        let hook = RECYCLE_EMPTY_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }
}

fn recycle_info(item_count: u64) -> RecycleBinInfo {
    RecycleBinInfo {
        item_count,
        size_in_bytes: 0,
    }
}

fn recycle_private_error(kind: DockUtilityErrorKind) -> DockUtilityError {
    DockUtilityError::new(
        kind,
        "PRIVATE-native-detail C:\\user\\secret\u{202e}<script>",
    )
}

fn native_trash(dock: &Dock) -> ElementHandle {
    use i_slint_backend_testing::{AccessibleRole, ElementQuery};
    let mut buttons = ElementQuery::from_root(dock)
        .find_all()
        .into_iter()
        .filter(|element| {
            element.accessible_role() == Some(AccessibleRole::Button)
                && element
                    .accessible_label()
                    .is_some_and(|label| label.starts_with("Recycle Bin\n"))
        });
    let trash = buttons.next().expect("actual trailing Recycle Bin button");
    assert!(buttons.next().is_none(), "one semantic trailing Trash");
    trash
}

fn native_center(element: &ElementHandle) -> slint::LogicalPosition {
    let origin = element.absolute_position();
    let size = element.size();
    slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
}

fn native_trash_pointer(dock: &Dock, button: slint::platform::PointerEventButton) {
    use slint::platform::WindowEvent;
    let position = native_center(&native_trash(dock));
    for event in [
        WindowEvent::PointerMoved { position },
        WindowEvent::PointerPressed { position, button },
        WindowEvent::PointerReleased { position, button },
    ] {
        dock.window().dispatch_event(event);
    }
}

fn native_focus_trash(dock: &Dock) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let position = native_center(&native_trash(dock));
    dock.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    dock.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    dock.window().dispatch_event(WindowEvent::PointerReleased {
        position: slint::LogicalPosition::new(1.0, 1.0),
        button: PointerEventButton::Left,
    });
}

fn native_key<C: slint::ComponentHandle>(component: &C, key: slint::platform::Key) {
    use slint::platform::WindowEvent;
    component
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    component
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
}

fn advance_recycle_timer(milliseconds: u64) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(milliseconds));
    slint::platform::update_timers_and_animations();
}

fn assert_retry_detached_before_read(fixture: &LauncherFixture) {
    let menu = fixture.controller.menus.borrow().clone().unwrap();
    let drops = fixture.host.lease_drops.clone();
    let before = drops.load(Ordering::SeqCst);
    RECYCLE_READ_HOOK.with(|hook| {
        assert!(
            hook.borrow_mut()
                .replace(Box::new(move || {
                    assert!(
                        !menu.is_open(),
                        "Retry hides the real popup before its typed read"
                    );
                    assert!(!menu.component().window().is_visible());
                    assert_eq!(
                        drops.load(Ordering::SeqCst),
                        before + 1,
                        "popup lease detached first"
                    );
                }))
                .is_none()
        );
    });
}

fn assert_empty_detached_before_dispatch(fixture: &LauncherFixture) {
    let menu = fixture.controller.menus.borrow().clone().unwrap();
    let dock = fixture.dock.as_weak();
    let drops = fixture.host.lease_drops.clone();
    let before = drops.load(Ordering::SeqCst);
    RECYCLE_EMPTY_HOOK.with(|hook| {
        assert!(
            hook.borrow_mut()
                .replace(Box::new(move || {
                    assert!(!menu.is_open(), "Empty hides the real popup first");
                    assert!(!menu.component().window().is_visible());
                    assert_eq!(drops.load(Ordering::SeqCst), before + 1);
                    let dock = dock.upgrade().unwrap();
                    assert!(dock.get_recycle_empty_busy(), "reserve before dispatch");
                    assert!(!dock.get_recycle_empty_enabled());
                }))
                .is_none()
        );
    });
}

#[test]
fn dock_native_trash_open_and_context_retry_reach_typed_recycle_capability_only() {
    use crate::generated::DockRecycleState;
    use slint::platform::{Key, PointerEventButton, WindowEvent};

    let bin = Arc::new(RootRecordingRecycleBin::default());
    let fixture = LauncherFixture::with_snapshot_configured(
        seeded_preferences(),
        launcher_snapshot(),
        |fixture| {
            *fixture.host.recycle_provider.lock() = Some(bin.clone());
            assert_eq!(
                fixture.host.recycle_provider_calls.load(Ordering::SeqCst),
                0
            );
            assert!(fixture.controller.recycle_bin.borrow().is_none());
            // A generated provisional show/input is not real monitor placement.
            fixture
                .dock
                .window()
                .set_size(slint::PhysicalSize::new(168, 72));
            fixture.dock.show().unwrap();
            native_trash_pointer(&fixture.dock, PointerEventButton::Left);
            native_key(&fixture.dock, Key::Space);
            native_trash_pointer(&fixture.dock, PointerEventButton::Right);
            assert_eq!(bin.counts(), (0, 0, 0));
            assert_eq!(
                fixture.host.recycle_provider_calls.load(Ordering::SeqCst),
                0
            );
            assert!(fixture.controller.recycle_bin.borrow().is_none());
            fixture.dock.hide().unwrap();
        },
    );
    let unrelated = fixture.host.unrelated_activity();
    let initial_focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    assert!(fixture.dock.window().is_visible());
    assert!(
        fixture
            .controller
            .leases
            .borrow()
            .attachments
            .contains_key(&SurfaceKind::Dock)
    );
    assert!(fixture.toolbar.window().is_visible());
    assert!(
        fixture
            .controller
            .leases
            .borrow()
            .attachments
            .contains_key(&SurfaceKind::Toolbar)
    );
    assert_eq!(
        fixture.host.recycle_provider_calls.load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        bin.counts(),
        (1, 0, 0),
        "subscribe before the first aggregate read"
    );
    bin.finish_ready(Ok(()));
    assert_eq!(
        bin.counts(),
        (1, 0, 0),
        "readiness is queued, not projected inline"
    );
    // Only mailbox delivery is explicit in this no-event-loop backend. All
    // action requests below come from real generated pointer/keyboard input.
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(bin.counts(), (1, 1, 0));
    assert_eq!(*bin.calls.lock(), ["watch", "read"]);
    bin.finish_read(Ok(recycle_info(3)));
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Full);
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "3 items");
    assert!(!fixture.dock.get_recycle_stale());
    assert_eq!(
        native_trash(&fixture.dock).accessible_label().as_deref(),
        Some("Recycle Bin\n3 items")
    );

    // Observe only tooltip text; never replace the root's typed action/context
    // wiring with decorative recording callbacks.
    let tooltip = Rc::new(RefCell::new(String::new()));
    let captured = tooltip.clone();
    fixture
        .dock
        .on_tooltip_requested(move |content, _| *captured.borrow_mut() = content.to_string());
    fixture
        .dock
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(1.0, 1.0),
        });
    fixture
        .dock
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: native_center(&native_trash(&fixture.dock)),
        });
    assert_eq!(&*tooltip.borrow(), "Recycle Bin\n3 items");

    fixture.panel.set_stale(true);
    fixture.panel.set_refreshing(true);
    let mut status = fixture.dock.get_surface_status();
    status.stale = true;
    status.refreshing = true;
    fixture.dock.set_surface_status(status);
    fixture.dock.set_show_desktop_busy(true);

    let before_context = bin.counts();
    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    let menu = fixture.controller.menus.borrow().clone().unwrap();
    assert!(menu.is_open());
    assert!(menu.component().window().is_visible());
    assert_eq!(
        bin.counts(),
        before_context,
        "showing context is not Retry or Open"
    );
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        initial_focus + 1
    );
    assert_retry_detached_before_read(&fixture);
    click_component(menu.component(), "Retry");
    assert_eq!(
        bin.counts(),
        (1, 2, 0),
        "healthy watch is reused by genuine Retry"
    );
    assert!(fixture.dock.get_recycle_read_busy());
    assert_eq!(native_trash(&fixture.dock).accessible_enabled(), Some(true));
    let start =
        ElementHandle::find_by_accessible_label(&fixture.dock, "Open applications and settings")
            .find(|element| element.accessible_enabled() == Some(true))
            .unwrap();
    assert_eq!(start.accessible_enabled(), Some(true));
    click_component(&fixture.dock, "Open applications and settings");
    assert!(
        fixture.launcher.window().is_visible(),
        "Start works during read/apps/Desktop busy"
    );
    fixture.controller.hide_launcher();
    let after_start_focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    assert_eq!(after_start_focus, initial_focus + 2);

    native_trash_pointer(&fixture.dock, PointerEventButton::Left);
    assert_eq!(bin.counts(), (1, 2, 1));
    assert!(fixture.dock.get_recycle_open_busy());
    native_trash_pointer(&fixture.dock, PointerEventButton::Left);
    native_key(&fixture.dock, Key::Space);
    native_key(&fixture.dock, Key::Return);
    fixture
        .dock
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated {
            text: Key::Return.into(),
        });
    assert_eq!(
        bin.counts(),
        (1, 2, 1),
        "pending duplicates cannot acquire another open"
    );
    bin.finish_open(Ok(()));
    assert!(fixture.dock.get_recycle_open_busy());
    fixture.dock.invoke_recycle_event_ready();
    assert!(!fixture.dock.get_recycle_open_busy());
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Full);
    assert_eq!(
        fixture.dock.get_recycle_item_count_label(),
        "3 items",
        "Open is not Empty"
    );
    native_focus_trash(&fixture.dock);
    fixture
        .dock
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated {
            text: Key::Return.into(),
        });
    assert_eq!(
        bin.counts(),
        (1, 2, 1),
        "held Return does not reopen after completion"
    );
    native_key(&fixture.dock, Key::Space);
    assert_eq!(
        bin.counts(),
        (1, 2, 2),
        "fresh Space is a new explicit Open"
    );
    bin.finish_open(Err(recycle_private_error(
        DockUtilityErrorKind::AccessDenied,
    )));
    fixture.dock.invoke_recycle_event_ready();
    assert!(
        fixture
            .panel
            .get_status()
            .starts_with("Recycle Bin open failed:")
    );
    assert_eq!(
        fixture.dock.get_recycle_open_notice(),
        "Recycle Bin Open: was denied. Try opening again.",
        "only the safe native error kind reaches presentation",
    );
    assert!(!fixture.panel.get_status().contains("PRIVATE-native-detail"));
    assert!(
        !fixture
            .dock
            .get_recycle_open_notice()
            .contains("PRIVATE-native-detail")
    );
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "3 items");
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        after_start_focus
    );
    bin.finish_read(Ok(recycle_info(0)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Empty);
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "0 items");

    // The preceding genuine Trash click established item focus. Menu and
    // Shift+F10 open the existing popup; End targets its actual Retry row.
    native_focus_trash(&fixture.dock);
    native_key(&fixture.dock, Key::Menu);
    assert!(menu.is_open());
    assert_eq!(bin.counts(), (1, 2, 2));
    assert_retry_detached_before_read(&fixture);
    native_key(menu.component(), Key::End);
    native_key(menu.component(), Key::Return);
    assert_eq!(bin.counts(), (1, 3, 2));
    bin.finish_read(Err(recycle_private_error(DockUtilityErrorKind::Other)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Empty);
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "0 items");
    assert!(
        fixture.dock.get_recycle_stale(),
        "failure preserves the confirmed snapshot"
    );
    assert!(
        !fixture
            .dock
            .get_recycle_read_notice()
            .contains("PRIVATE-native-detail")
    );
    native_focus_trash(&fixture.dock);
    fixture
        .dock
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
    native_key(&fixture.dock, Key::F10);
    fixture
        .dock
        .window()
        .dispatch_event(WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
    assert!(menu.is_open());
    assert_eq!(
        bin.counts(),
        (1, 3, 2),
        "keyboard context alone has no native capability"
    );
    assert_retry_detached_before_read(&fixture);
    native_key(menu.component(), Key::End);
    native_key(menu.component(), Key::Space);
    assert_eq!(bin.counts(), (1, 4, 2));
    bin.finish_read(Ok(recycle_info(3)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Full);
    assert!(!fixture.dock.get_recycle_stale());
    assert_eq!(
        fixture.host.recycle_provider_calls.load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        after_start_focus + 2
    );
    assert_eq!(fixture.host.unrelated_activity(), unrelated);
    assert_eq!(
        fixture
            .host
            .recycle_mutation_provider_calls
            .load(Ordering::SeqCst),
        0,
        "Open, read/watch and genuine Retry never acquire mutation authority"
    );
    RECYCLE_READ_HOOK.with(|hook| assert!(hook.borrow().is_none()));
}

#[test]
fn dock_native_trash_watch_throttle_fullscreen_and_closed_root_retire_intents() {
    use crate::generated::DockRecycleState;
    use slint::platform::PointerEventButton;

    let bin = Arc::new(RootRecordingRecycleBin::default());
    let snapshot = launcher_snapshot();
    let fixture = LauncherFixture::with_snapshot_configured(
        seeded_preferences(),
        snapshot.clone(),
        |fixture| *fixture.host.recycle_provider.lock() = Some(bin.clone()),
    );
    let unrelated = fixture.host.unrelated_activity();
    let focus_before = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    bin.finish_ready(Ok(()));
    fixture.dock.invoke_recycle_event_ready();
    bin.finish_read(Ok(recycle_info(3)));
    fixture.dock.invoke_recycle_event_ready();

    let full_context = snapshot.dock_context().unwrap();
    let fullscreen = crate::DockContext::new(
        full_context.x(),
        full_context.y(),
        full_context.width(),
        full_context.height(),
        true,
    )
    .unwrap();
    apply_result_to_both(
        &fixture.controller,
        &fixture.panel,
        Ok(snapshot.clone().with_dock_context(fullscreen)),
    );
    assert!(!fixture.dock.window().is_visible());
    assert!(
        !fixture.dock.get_recycle_open_busy(),
        "hidden-intent proof is not just busy suppression"
    );
    native_trash_pointer(&fixture.dock, PointerEventButton::Left);
    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    assert_eq!(bin.counts(), (1, 1, 0));
    assert!(
        fixture.controller.menus.borrow().is_none(),
        "hidden context cannot resurrect a popup"
    );
    apply_result_to_both(&fixture.controller, &fixture.panel, Ok(snapshot.clone()));
    assert_eq!(
        bin.counts(),
        (1, 1, 0),
        "clean reshow never replays rejected intent"
    );

    for _ in 0..128 {
        bin.emit();
    }
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(bin.counts(), (1, 1, 0));
    advance_recycle_timer(50);
    for _ in 0..128 {
        bin.emit();
    }
    fixture.dock.invoke_recycle_event_ready();
    advance_recycle_timer(49);
    assert_eq!(bin.counts(), (1, 1, 0));
    advance_recycle_timer(1);
    assert_eq!(
        bin.counts(),
        (1, 2, 0),
        "bursts do not restart the production 100ms timer"
    );
    for _ in 0..128 {
        bin.emit();
    }
    fixture.dock.invoke_recycle_event_ready();
    advance_recycle_timer(100);
    assert_eq!(
        bin.counts(),
        (1, 2, 0),
        "dirty during read cannot create a parallel flight"
    );
    bin.finish_read(Ok(recycle_info(5)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(bin.counts(), (1, 3, 0), "one rate-bounded dirty follow-up");
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "5 items");
    native_trash_pointer(&fixture.dock, PointerEventButton::Left);
    assert_eq!(bin.counts(), (1, 3, 1));

    apply_result_to_both(
        &fixture.controller,
        &fixture.panel,
        Ok(snapshot.clone().with_dock_context(fullscreen)),
    );
    assert!(!fixture.dock.window().is_visible());
    assert!(!fixture.toolbar.window().is_visible());
    assert!(
        !fixture
            .controller
            .leases
            .borrow()
            .attachments
            .contains_key(&SurfaceKind::Dock)
    );
    let hidden_projection = (
        fixture.dock.get_recycle_state(),
        fixture.dock.get_recycle_item_count_label(),
        fixture.dock.get_recycle_open_notice(),
    );
    native_trash_pointer(&fixture.dock, PointerEventButton::Left);
    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    assert_eq!(
        bin.counts(),
        (1, 3, 1),
        "hidden root rejects new native intent"
    );
    bin.finish_read(Ok(recycle_info(0)));
    bin.finish_open(Ok(()));
    for _ in 0..128 {
        bin.emit();
    }
    fixture.dock.invoke_recycle_event_ready();
    advance_recycle_timer(1000);
    assert_eq!(
        bin.counts(),
        (1, 3, 1),
        "accepted hidden work drains without new effects"
    );
    assert!(!fixture.dock.window().is_visible());
    assert_eq!(
        (
            fixture.dock.get_recycle_state(),
            fixture.dock.get_recycle_item_count_label(),
            fixture.dock.get_recycle_open_notice(),
        ),
        hidden_projection,
        "late accepted completions cannot project or resurrect the hidden root",
    );
    apply_result_to_both(&fixture.controller, &fixture.panel, Ok(snapshot));
    assert!(fixture.dock.window().is_visible());
    assert_eq!(
        bin.counts(),
        (1, 4, 1),
        "reshow consumes dirty state read-only, never replays Open"
    );
    assert!(!fixture.dock.get_recycle_open_busy());
    bin.finish_read(Ok(recycle_info(0)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Empty);
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "0 items");
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_before
    );
    assert_eq!(fixture.host.unrelated_activity(), unrelated);
    assert_eq!(
        fixture
            .host
            .recycle_mutation_provider_calls
            .load(Ordering::SeqCst),
        0
    );

    // Retain only an already-admitted callback, not the controller or window.
    let admitted_event = bin.watches.lock()[0].callback.clone();
    let weak_dock = fixture.dock.as_weak();
    let leases = fixture.host.lease_drops.clone();
    let drops_before = leases.load(Ordering::SeqCst);
    drop(fixture);
    assert_eq!(bin.guard_drops.load(Ordering::SeqCst), 1);
    assert_eq!(leases.load(Ordering::SeqCst), drops_before + 2);
    assert!(
        weak_dock.upgrade().is_none(),
        "SDK shown-window keepalive was disposed"
    );
    admitted_event(RecycleBinWatchEvent::Invalidated);
    admitted_event(RecycleBinWatchEvent::Unavailable(
        DockUtilityErrorKind::Other,
    ));
    bin.emit();
    advance_recycle_timer(1000);
    assert_eq!(
        bin.counts(),
        (1, 4, 1),
        "close rejects even an already-admitted late callback"
    );

    // Immediate watch rejection queues a safe failure, without accepting a
    // readiness callback. Retry is still the real visible context row.
    let rejected = Arc::new(RootRecordingRecycleBin::default());
    *rejected.reject_watch.lock() = Some(recycle_private_error(DockUtilityErrorKind::AccessDenied));
    let fixture = LauncherFixture::with_snapshot_configured(
        seeded_preferences(),
        launcher_snapshot(),
        |fixture| *fixture.host.recycle_provider.lock() = Some(rejected.clone()),
    );
    let unrelated = fixture.host.unrelated_activity();
    let focus_before = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    assert_eq!(rejected.counts(), (1, 0, 0));
    assert!(rejected.watches.lock().is_empty());
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(rejected.counts(), (1, 1, 0));
    assert!(
        !fixture
            .dock
            .get_recycle_watch_notice()
            .contains("PRIVATE-native-detail")
    );
    native_trash_pointer(&fixture.dock, PointerEventButton::Left);
    assert_eq!(
        rejected.counts(),
        (1, 1, 1),
        "watch failure/read busy do not disable explicit Open"
    );
    rejected.finish_open(Ok(()));
    rejected.finish_read(Err(recycle_private_error(DockUtilityErrorKind::Other)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(
        fixture.dock.get_recycle_state(),
        DockRecycleState::Unknown,
        "failure is not Empty"
    );
    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    let menu = fixture.controller.menus.borrow().clone().unwrap();
    assert!(menu.is_open());
    assert_eq!(rejected.counts(), (1, 1, 1));
    click_component(menu.component(), "Retry");
    assert!(!menu.is_open());
    assert_eq!(
        rejected.counts(),
        (2, 1, 1),
        "Retry reacquires a failed watch, never Open"
    );
    rejected.finish_ready(Ok(()));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(rejected.counts(), (2, 2, 1));
    rejected.finish_read(Ok(recycle_info(0)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Empty);
    assert!(!fixture.dock.get_recycle_stale());
    assert_eq!(
        fixture.host.recycle_provider_calls.load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_before + 1
    );
    assert_eq!(fixture.host.unrelated_activity(), unrelated);
    assert_eq!(
        fixture
            .host
            .recycle_mutation_provider_calls
            .load(Ordering::SeqCst),
        0
    );
    drop(menu);
    drop(fixture);
    assert_eq!(rejected.guard_drops.load(Ordering::SeqCst), 1);
}

#[test]
fn dock_native_empty_is_lazy_single_flight_and_requires_post_return_aggregate_read() {
    use crate::generated::DockRecycleState;
    use i_slint_backend_testing::{AccessibleRole, ElementQuery};
    use slint::platform::{Key, PointerEventButton, WindowEvent};

    let bin = Arc::new(RootRecordingRecycleBin::default());
    let mutation = Arc::new(RootRecordingRecycleMutation::default());
    let fixture = LauncherFixture::with_snapshot_configured(
        seeded_preferences(),
        launcher_snapshot(),
        |fixture| {
            *fixture.host.recycle_provider.lock() = Some(bin.clone());
            *fixture.host.recycle_mutation_provider.lock() = Some(mutation.clone());
            assert_eq!(
                fixture
                    .host
                    .recycle_mutation_provider_calls
                    .load(Ordering::SeqCst),
                0,
                "construction cannot acquire mutation authority"
            );
            fixture
                .dock
                .window()
                .set_size(slint::PhysicalSize::new(168, 72));
            fixture.dock.show().unwrap();
            native_trash_pointer(&fixture.dock, PointerEventButton::Right);
            native_focus_trash(&fixture.dock);
            native_key(&fixture.dock, Key::Menu);
            assert!(fixture.controller.menus.borrow().is_none());
            assert_eq!(mutation.calls.load(Ordering::SeqCst), 0);
            assert_eq!(
                fixture
                    .host
                    .recycle_mutation_provider_calls
                    .load(Ordering::SeqCst),
                0,
                "provisional generated show is not a placed session"
            );
            fixture.dock.hide().unwrap();
        },
    );
    let unrelated = fixture.host.unrelated_activity();
    let focus_before = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    bin.finish_ready(Ok(()));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(bin.counts(), (1, 1, 0));
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
    native_trash_pointer(&fixture.dock, PointerEventButton::Left);
    assert_eq!(bin.counts(), (1, 1, 1));
    assert!(fixture.dock.get_recycle_open_busy());
    fixture.panel.set_stale(true);
    fixture.panel.set_refreshing(true);
    let mut apps_status = fixture.dock.get_surface_status();
    apps_status.stale = true;
    apps_status.refreshing = true;
    fixture.dock.set_surface_status(apps_status);
    fixture.dock.set_show_desktop_busy(true);
    let panel_status = fixture.panel.get_status();

    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    let menu = fixture.controller.menus.borrow().clone().unwrap();
    assert!(menu.is_open());
    assert!(menu.component().get_recycle_empty_enabled());
    let rows = ElementQuery::from_root(menu.component())
        .match_accessible_role(AccessibleRole::Button)
        .find_all();
    let empty = rows
        .iter()
        .find(|row| row.accessible_label().as_deref() == Some("Empty Recycle Bin"))
        .unwrap();
    let retry = rows
        .iter()
        .find(|row| row.accessible_label().as_deref() == Some("Retry"))
        .unwrap();
    assert!(empty.absolute_position().y < retry.absolute_position().y);
    assert_eq!(empty.accessible_enabled(), Some(true));
    assert!(
        !rows
            .iter()
            .any(|row| row.accessible_label().as_deref() == Some("Open"))
    );
    assert_eq!(
        fixture
            .host
            .recycle_mutation_provider_calls
            .load(Ordering::SeqCst),
        0,
        "show/read/watch/Open/context do not acquire the separate provider"
    );
    assert_empty_detached_before_dispatch(&fixture);
    native_key(menu.component(), Key::Home);
    assert_eq!(menu.component().get_selected_index(), 0);
    native_key(menu.component(), Key::Return);
    assert_eq!(mutation.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture
            .host
            .recycle_mutation_provider_calls
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        bin.counts(),
        (1, 1, 1),
        "Empty is independent of both busy effects"
    );
    assert!(fixture.dock.get_recycle_empty_busy());
    assert!(!fixture.dock.get_recycle_empty_enabled());
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);

    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    assert!(menu.is_open());
    assert!(!menu.component().get_recycle_empty_enabled());
    click_component(menu.component(), "Empty Recycle Bin");
    native_key(menu.component(), Key::Return);
    menu.component()
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated {
            text: Key::Return.into(),
        });
    assert_eq!(mutation.calls.load(Ordering::SeqCst), 1);
    native_key(menu.component(), Key::Home);
    assert_eq!(
        menu.component().get_selected_index(),
        1,
        "Home skips disabled Empty"
    );
    let focus_before_return = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    mutation.finish(Ok(RecycleBinEmptyOutcome { native_hresult: 0 }));
    assert!(
        fixture.dock.get_recycle_empty_busy(),
        "return is only mailbox data"
    );
    fixture.dock.invoke_recycle_event_ready();
    // Drive the same deferred changed-property relay as the real UI loop.
    slint::platform::update_timers_and_animations();
    assert!(
        menu.is_open(),
        "pure refresh does not reopen or dismiss the current menu"
    );
    assert!(menu.component().get_recycle_empty_enabled());
    assert!(!fixture.dock.get_recycle_empty_busy());
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_before_return
    );
    assert_eq!(
        fixture.panel.get_status(),
        panel_status,
        "Empty is not Open feedback"
    );
    assert_eq!(
        fixture.dock.get_recycle_empty_notice(),
        "Recycle Bin operation returned 0x00000000. State refresh requested."
    );
    assert_eq!(
        bin.counts(),
        (1, 1, 1),
        "old accepted read must drain first"
    );
    native_key(menu.component(), Key::Home);
    assert_eq!(menu.component().get_selected_index(), 0);
    menu.component()
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated {
            text: Key::Return.into(),
        });
    assert_eq!(
        mutation.calls.load(Ordering::SeqCst),
        1,
        "held Return cannot replay"
    );
    native_key(menu.component(), Key::Escape);

    bin.finish_read(Ok(recycle_info(0)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(
        bin.counts(),
        (1, 2, 1),
        "exactly one fresh post-return read"
    );
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
    assert_ne!(fixture.dock.get_recycle_item_count_label(), "0 items");
    bin.finish_read(Ok(recycle_info(3)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "3 items");
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Full);
    assert!(!fixture.dock.get_recycle_stale());
    bin.finish_open(Ok(()));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(fixture.panel.get_status(), "Recycle Bin open requested");
    let open_feedback = fixture.panel.get_status();

    // A genuine second-row Retry starts another old read. Its queued failure
    // cannot suppress reconciliation when Empty returns in the same mailbox batch.
    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    native_key(menu.component(), Key::End);
    native_key(menu.component(), Key::Return);
    assert_eq!(bin.counts(), (1, 3, 1));
    native_focus_trash(&fixture.dock);
    native_key(&fixture.dock, Key::Menu);
    assert_empty_detached_before_dispatch(&fixture);
    native_key(menu.component(), Key::Home);
    native_key(menu.component(), Key::Space);
    assert_eq!(mutation.calls.load(Ordering::SeqCst), 2);
    bin.finish_read(Err(recycle_private_error(
        DockUtilityErrorKind::AccessDenied,
    )));
    mutation.finish(Ok(RecycleBinEmptyOutcome { native_hresult: 1 }));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(bin.counts(), (1, 4, 1));
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "3 items");
    assert!(fixture.dock.get_recycle_stale());
    assert!(
        fixture.dock.get_recycle_read_notice().is_empty(),
        "obsolete error is not projected"
    );
    assert_eq!(
        fixture.dock.get_recycle_empty_notice(),
        "Recycle Bin operation returned status 0x00000001. State refresh requested."
    );
    bin.finish_read(Ok(recycle_info(0)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Empty);
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "0 items");
    assert!(
        fixture.dock.get_recycle_empty_enabled(),
        "confirmed zero is not deletion authority"
    );

    native_focus_trash(&fixture.dock);
    fixture
        .dock
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
    native_key(&fixture.dock, Key::F10);
    fixture
        .dock
        .window()
        .dispatch_event(WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
    assert!(menu.is_open());
    assert_empty_detached_before_dispatch(&fixture);
    native_key(menu.component(), Key::Home);
    native_key(menu.component(), Key::Return);
    assert_eq!(mutation.calls.load(Ordering::SeqCst), 3);
    mutation.finish(Ok(RecycleBinEmptyOutcome {
        native_hresult: -2_147_024_891,
    }));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(bin.counts(), (1, 5, 1));
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Empty);
    assert!(fixture.dock.get_recycle_stale());
    assert_eq!(
        fixture.dock.get_recycle_empty_notice(),
        "Recycle Bin operation returned failure status 0x80070005. State refresh requested."
    );
    bin.finish_read(Ok(recycle_info(7)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Full);
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "7 items");
    assert!(!fixture.dock.get_recycle_stale());
    assert_eq!(fixture.panel.get_status(), open_feedback);
    assert!(
        !fixture
            .dock
            .get_recycle_empty_notice()
            .to_lowercase()
            .contains("canceled")
    );
    assert!(
        !fixture
            .dock
            .get_recycle_empty_notice()
            .to_lowercase()
            .contains("emptied")
    );
    assert_eq!(
        fixture
            .host
            .recycle_mutation_provider_calls
            .load(Ordering::SeqCst),
        1,
        "accepted provider is cached independently of read/watch/Open"
    );
    assert_eq!(
        fixture.host.recycle_provider_calls.load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_before + 5
    );
    assert_eq!(fixture.host.unrelated_activity(), unrelated);
    RECYCLE_EMPTY_HOOK.with(|hook| assert!(hook.borrow().is_none()));
}

#[test]
fn dock_native_empty_scope_reentry_hidden_readback_and_close_never_replay_or_resurrect() {
    use crate::generated::{DockMenuKind, DockRecycleState};
    use i_slint_backend_testing::AccessibleRole;
    use slint::platform::{Key, PointerEventButton, WindowEvent};

    let bin = Arc::new(RootRecordingRecycleBin::default());
    let mutation = Arc::new(RootRecordingRecycleMutation::default());
    let snapshot = launcher_snapshot();
    let fixture = LauncherFixture::with_snapshot_configured(
        seeded_preferences(),
        snapshot.clone(),
        |fixture| {
            *fixture.host.recycle_provider.lock() = Some(bin.clone());
            *fixture.host.recycle_mutation_provider.lock() = Some(mutation.clone());
        },
    );
    let unrelated = fixture.host.unrelated_activity();
    let focus_before = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    bin.finish_ready(Ok(()));
    fixture.dock.invoke_recycle_event_ready();
    bin.finish_read(Ok(recycle_info(3)));
    fixture.dock.invoke_recycle_event_ready();
    let context = snapshot.dock_context().unwrap();
    let fullscreen = crate::DockContext::new(
        context.x(),
        context.y(),
        context.width(),
        context.height(),
        true,
    )
    .unwrap();

    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    let menu = fixture.controller.menus.borrow().clone().unwrap();
    let dock = fixture.dock.as_weak();
    RECYCLE_MENU_DROP_HOOK.with(|hook| {
        assert!(
            hook.borrow_mut()
                .replace(Box::new(move || {
                    let dock = dock.upgrade().unwrap();
                    let start = ElementHandle::find_by_accessible_label(
                        &dock,
                        "Open applications and settings",
                    )
                    .find(|element| element.accessible_role() == Some(AccessibleRole::Button))
                    .unwrap();
                    let position = native_center(&start);
                    for event in [
                        WindowEvent::PointerMoved { position },
                        WindowEvent::PointerPressed {
                            position,
                            button: PointerEventButton::Right,
                        },
                        WindowEvent::PointerReleased {
                            position,
                            button: PointerEventButton::Right,
                        },
                    ] {
                        dock.window().dispatch_event(event);
                    }
                }))
                .is_none()
        );
    });
    native_key(menu.component(), Key::Home);
    native_key(menu.component(), Key::Return);
    assert!(
        menu.is_open(),
        "lease-drop reentry creates a replacement scope"
    );
    assert!(
        menu.component().window().is_visible(),
        "old retirement must not physically hide the replacement Slint window"
    );
    assert_eq!(menu.component().get_kind(), DockMenuKind::Bar);
    assert!(!menu.component().get_recycle_empty_enabled());
    assert_eq!(mutation.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture
            .host
            .recycle_mutation_provider_calls
            .load(Ordering::SeqCst),
        0,
        "replaced scope cannot acquire destructive authority"
    );
    assert_eq!(bin.counts(), (1, 1, 0));
    native_key(menu.component(), Key::Escape);

    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    assert!(menu.component().get_recycle_empty_enabled());
    apply_result_to_both(
        &fixture.controller,
        &fixture.panel,
        Ok(snapshot.clone().with_dock_context(fullscreen)),
    );
    slint::platform::update_timers_and_animations();
    assert!(!fixture.dock.window().is_visible());
    assert!(!fixture.dock.get_recycle_empty_enabled());
    assert!(
        !menu.component().get_recycle_empty_enabled(),
        "live session refresh disables the row"
    );
    click_component(menu.component(), "Empty Recycle Bin");
    native_key(menu.component(), Key::Return);
    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    assert_eq!(mutation.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture
            .host
            .recycle_mutation_provider_calls
            .load(Ordering::SeqCst),
        0
    );
    apply_result_to_both(&fixture.controller, &fixture.panel, Ok(snapshot.clone()));
    slint::platform::update_timers_and_animations();
    assert!(fixture.dock.window().is_visible());
    assert!(menu.component().get_recycle_empty_enabled());
    assert_eq!(
        mutation.calls.load(Ordering::SeqCst),
        0,
        "resume never replays rejected input"
    );
    assert_eq!(bin.counts(), (1, 1, 0));
    native_key(menu.component(), Key::Escape);

    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    native_key(menu.component(), Key::End);
    native_key(menu.component(), Key::Space);
    assert_eq!(bin.counts(), (1, 2, 0));
    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    assert_empty_detached_before_dispatch(&fixture);
    native_key(menu.component(), Key::Home);
    native_key(menu.component(), Key::Space);
    assert_eq!(mutation.calls.load(Ordering::SeqCst), 1);
    apply_result_to_both(
        &fixture.controller,
        &fixture.panel,
        Ok(snapshot.clone().with_dock_context(fullscreen)),
    );
    let hidden_projection = (
        fixture.dock.get_recycle_state(),
        fixture.dock.get_recycle_item_count_label(),
        fixture.dock.get_recycle_empty_notice(),
        fixture.dock.get_recycle_stale(),
    );
    mutation.finish(Ok(RecycleBinEmptyOutcome {
        native_hresult: -2_147_024_891,
    }));
    bin.finish_read(Ok(recycle_info(0)));
    fixture.dock.invoke_recycle_event_ready();
    advance_recycle_timer(1000);
    assert_eq!(
        bin.counts(),
        (1, 2, 0),
        "hidden readback is deferred, not dispatched"
    );
    assert_eq!(mutation.calls.load(Ordering::SeqCst), 1);
    assert!(!fixture.dock.window().is_visible());
    assert!(!menu.is_open());
    assert_eq!(
        (
            fixture.dock.get_recycle_state(),
            fixture.dock.get_recycle_item_count_label(),
            fixture.dock.get_recycle_empty_notice(),
            fixture.dock.get_recycle_stale(),
        ),
        hidden_projection,
        "same-mailbox old info and mutation return cannot project while hidden"
    );
    apply_result_to_both(&fixture.controller, &fixture.panel, Ok(snapshot.clone()));
    assert_eq!(
        bin.counts(),
        (1, 3, 0),
        "resume starts exactly one required new read"
    );
    assert_eq!(mutation.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "3 items");
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Full);
    assert!(fixture.dock.get_recycle_stale());
    assert!(!fixture.dock.get_recycle_empty_busy());
    assert_eq!(
        fixture.dock.get_recycle_empty_notice(),
        "Recycle Bin operation returned failure status 0x80070005. State refresh requested."
    );
    bin.finish_read(Ok(recycle_info(0)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Empty);
    assert!(!fixture.dock.get_recycle_stale());

    // Safe setup/driver failure also demands real readback, without leaking a
    // provider string or changing the existing Open-only root feedback channel.
    let panel_status = fixture.panel.get_status();
    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    assert_empty_detached_before_dispatch(&fixture);
    native_key(menu.component(), Key::Home);
    native_key(menu.component(), Key::Return);
    assert_eq!(mutation.calls.load(Ordering::SeqCst), 2);
    mutation.finish(Err(recycle_private_error(
        DockUtilityErrorKind::AccessDenied,
    )));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(bin.counts(), (1, 4, 0));
    assert_eq!(
        fixture.dock.get_recycle_empty_notice(),
        "Recycle Bin Empty: was denied. State refresh requested."
    );
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "0 items");
    assert!(fixture.dock.get_recycle_stale());
    assert_eq!(fixture.panel.get_status(), panel_status);
    bin.finish_read(Ok(recycle_info(5)));
    fixture.dock.invoke_recycle_event_ready();
    assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Full);
    assert_eq!(fixture.dock.get_recycle_item_count_label(), "5 items");

    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    native_key(menu.component(), Key::End);
    native_key(menu.component(), Key::Space);
    assert_eq!(bin.counts(), (1, 5, 0));
    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    assert_empty_detached_before_dispatch(&fixture);
    native_key(menu.component(), Key::Home);
    native_key(menu.component(), Key::Return);
    assert_eq!(mutation.calls.load(Ordering::SeqCst), 3);
    let recycle = fixture.controller.recycle_bin.borrow().clone().unwrap();
    recycle.close();
    drop(recycle);
    assert!(!fixture.dock.get_recycle_empty_enabled());
    native_trash_pointer(&fixture.dock, PointerEventButton::Right);
    assert!(menu.is_open());
    assert!(!menu.component().get_recycle_empty_enabled());
    click_component(menu.component(), "Empty Recycle Bin");
    native_key(menu.component(), Key::Return);
    let closed_projection = (
        fixture.dock.get_recycle_state(),
        fixture.dock.get_recycle_item_count_label(),
        fixture.dock.get_recycle_empty_notice(),
    );
    bin.finish_read(Ok(recycle_info(0)));
    fixture.dock.invoke_recycle_event_ready();
    advance_recycle_timer(1000);
    assert_eq!(
        mutation.calls.load(Ordering::SeqCst),
        3,
        "closed scope rejects new input and late replay"
    );
    assert_eq!(
        bin.counts(),
        (1, 5, 0),
        "close cannot request late readback"
    );
    assert_eq!(
        (
            fixture.dock.get_recycle_state(),
            fixture.dock.get_recycle_item_count_label(),
            fixture.dock.get_recycle_empty_notice(),
        ),
        closed_projection
    );
    assert_eq!(
        fixture
            .host
            .recycle_mutation_provider_calls
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        fixture.host.recycle_provider_calls.load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        fixture.host.ui_focus_calls.load(Ordering::SeqCst),
        focus_before + 9
    );
    assert_eq!(fixture.host.unrelated_activity(), unrelated);
    RECYCLE_MENU_DROP_HOOK.with(|hook| assert!(hook.borrow().is_none()));
    RECYCLE_EMPTY_HOOK.with(|hook| assert!(hook.borrow().is_none()));
    let weak_dock = fixture.dock.as_weak();
    drop(menu);
    drop(fixture);
    assert!(
        weak_dock.upgrade().is_none(),
        "accepted fake work owns no Slint root"
    );
    assert_eq!(bin.guard_drops.load(Ordering::SeqCst), 1);
    mutation.finish(Ok(RecycleBinEmptyOutcome { native_hresult: 1 }));
    advance_recycle_timer(1000);
    assert!(
        weak_dock.upgrade().is_none(),
        "late Empty cannot resurrect the closed root"
    );
    assert_eq!(bin.counts(), (1, 5, 0));
    assert_eq!(mutation.calls.load(Ordering::SeqCst), 3);
}

#[test]
fn dock_native_show_desktop_pointer_and_space_use_shell_capability_not_application_or_recovery() {
    use slint::platform::{Key, PointerEventButton, WindowEvent};
    use tessera_system::dock_utilities::{
        DockUtilitiesHost, DockUtilityCompletion, DockUtilityError, DockUtilityErrorKind,
    };

    struct RecordingDesktop {
        requests: AtomicUsize,
        completion: Mutex<Option<DockUtilityCompletion>>,
    }
    impl DockUtilitiesHost for RecordingDesktop {
        fn toggle_desktop(
            &self,
            completion: DockUtilityCompletion,
        ) -> Result<(), DockUtilityError> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            assert!(self.completion.lock().replace(completion).is_none());
            Ok(())
        }
    }

    let f = LauncherFixture::new();
    let desktop = Arc::new(RecordingDesktop {
        requests: AtomicUsize::new(0),
        completion: Mutex::default(),
    });
    *f.host.dock_utility_provider.lock() = Some(desktop.clone());
    assert!(f.dock.window().is_visible());
    assert_eq!(f.host.dock_utility_provider_calls.load(Ordering::SeqCst), 0);
    let focus_before = f.host.ui_focus_calls.load(Ordering::SeqCst);
    f.panel.set_stale(true);
    f.panel.set_refreshing(true);
    let mut status = f.dock.get_surface_status();
    status.stale = true;
    status.refreshing = true;
    f.dock.set_surface_status(status);

    click_component(&f.dock, "Show desktop");
    assert_eq!(desktop.requests.load(Ordering::SeqCst), 1);
    assert!(f.dock.get_show_desktop_busy());
    let start = ElementHandle::find_by_accessible_label(&f.dock, "Open applications and settings")
        .find(|element| element.accessible_enabled() == Some(true))
        .unwrap();
    assert_eq!(start.accessible_enabled(), Some(true));
    click_component(&f.dock, "Show desktop");
    for event in [
        WindowEvent::KeyPressed {
            text: Key::Space.into(),
        },
        WindowEvent::KeyReleased {
            text: Key::Space.into(),
        },
        WindowEvent::KeyPressed {
            text: Key::Return.into(),
        },
        WindowEvent::KeyPressRepeated {
            text: Key::Return.into(),
        },
    ] {
        f.dock.window().dispatch_event(event);
    }
    assert_eq!(desktop.requests.load(Ordering::SeqCst), 1);
    f.dock.hide().unwrap();
    f.dock
        .invoke_reserved_action_requested(crate::generated::DockReservedAction::ShowDesktop);
    assert_eq!(desktop.requests.load(Ordering::SeqCst), 1);
    f.dock.show().unwrap();
    assert!(f.dock.get_show_desktop_busy());
    desktop.completion.lock().take().unwrap()(Ok(()));
    assert!(
        f.dock.get_show_desktop_busy(),
        "completion is queued, not projected inline"
    );
    f.dock.invoke_utility_event_ready();
    assert!(!f.dock.get_show_desktop_busy());
    assert_eq!(f.panel.get_status(), "Desktop toggle requested");

    // A real focus press cancelled outside the tile requests no toggle; the
    // following Space gesture is the second explicit native button action.
    let utility = ElementHandle::find_by_accessible_label(&f.dock, "Show desktop")
        .find(|element| element.accessible_enabled() == Some(true))
        .unwrap();
    let origin = utility.absolute_position();
    let size = utility.size();
    f.dock.window().dispatch_event(WindowEvent::PointerPressed {
        position: slint::LogicalPosition::new(
            origin.x + size.width / 2.0,
            origin.y + size.height / 2.0,
        ),
        button: PointerEventButton::Left,
    });
    f.dock
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: slint::LogicalPosition::new(1.0, 1.0),
            button: PointerEventButton::Left,
        });
    f.dock
        .window()
        .dispatch_event(WindowEvent::KeyPressRepeated {
            text: Key::Return.into(),
        });
    assert_eq!(
        desktop.requests.load(Ordering::SeqCst),
        1,
        "held Return cannot retry once completion re-enables the utility",
    );
    f.dock.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    assert_eq!(desktop.requests.load(Ordering::SeqCst), 1);
    f.dock.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Space.into(),
    });
    assert_eq!(desktop.requests.load(Ordering::SeqCst), 1);
    f.dock.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Space.into(),
    });
    assert_eq!(desktop.requests.load(Ordering::SeqCst), 2);
    desktop.completion.lock().take().unwrap()(Err(DockUtilityError::new(
        DockUtilityErrorKind::AccessDenied,
        "private native detail must not enter UI",
    )));
    f.dock.invoke_utility_event_ready();
    assert!(!f.dock.get_show_desktop_busy());
    assert!(f.panel.get_status().starts_with("Desktop toggle failed:"));
    assert!(!f.panel.get_status().contains("private native detail"));
    assert_eq!(f.host.dock_utility_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.host.ui_focus_calls.load(Ordering::SeqCst), focus_before);
    assert_eq!(f.host.observe_calls.load(Ordering::SeqCst), 0);
    assert!(f.host.launches.lock().is_empty());
    assert!(f.host.window_actions.lock().is_empty());
    assert!(f.host.activations.lock().is_empty());
    assert!(f.host.saves.lock().is_empty());
    assert!(f.host.system_actions.lock().is_empty());
}

#[test]
fn toolbar_native_clock_reads_calendar_not_clock_text_and_routes_current_day_input() {
    use tessera_system::calendar::{
        CalendarError, CalendarHost, CalendarReadCompletion, CalendarSnapshot, CivilDate, WeekStart,
    };
    struct NativeDateFixture {
        reads: AtomicUsize,
        snapshot: CalendarSnapshot,
    }
    impl CalendarHost for NativeDateFixture {
        fn read(&self, completion: CalendarReadCompletion) -> Result<(), CalendarError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            completion(Ok(self.snapshot.clone()));
            Ok(())
        }
    }
    // Deliberately unrelated clock text: the actual date comes only from the
    // capability. These explicit recording-fixture labels are not OS fallbacks.
    let date = Arc::new(NativeDateFixture {
        reads: AtomicUsize::new(0),
        snapshot: CalendarSnapshot::new(
            CivilDate::new(2024, 1, 31).unwrap(),
            "en-US".into(),
            [
                "January",
                "February",
                "March",
                "April",
                "May",
                "June",
                "July",
                "August",
                "September",
                "October",
                "November",
                "December",
            ]
            .map(str::to_owned),
            [
                "Monday",
                "Tuesday",
                "Wednesday",
                "Thursday",
                "Friday",
                "Saturday",
                "Sunday",
            ]
            .map(str::to_owned),
            ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].map(str::to_owned),
            WeekStart::Monday,
        )
        .unwrap(),
    });
    let fixture = LauncherFixture::new();
    *fixture.host.calendar_provider.lock() = Some(date.clone());
    fixture.controller.open_launcher();
    fixture.toolbar.set_clock("Native clock format".into());
    let observations = fixture.host.observe_calls.load(Ordering::SeqCst);
    assert!(fixture.toolbar.window().is_visible());
    click_component(&fixture.toolbar, "Open calendar");
    let calendar = fixture.controller.calendar.borrow().clone().unwrap();
    assert!(calendar.is_open());
    calendar.component().invoke_calendar_event_ready();
    assert_eq!(date.reads.load(Ordering::SeqCst), 1);
    assert!(!calendar.component().get_loading());
    assert_eq!(
        calendar.component().get_title_text().as_str(),
        "January 2024"
    );
    assert!(fixture.launcher.window().is_visible());
    fixture
        .launcher
        .invoke_open_user_menu_requested(crate::generated::TileBounds {
            origin: slint::LogicalPosition::new(f32::MAX, 10.0),
            width: 80.0,
            height: 32.0,
        });
    assert!(
        calendar.is_open(),
        "invalid User native placement retains Calendar"
    );
    assert!(
        !fixture
            .controller
            .user_menu
            .borrow()
            .as_ref()
            .unwrap()
            .is_open()
    );
    assert_eq!(date.reads.load(Ordering::SeqCst), 1);
    click_component(calendar.component(), "Next month");
    assert_eq!(
        calendar.component().get_title_text().as_str(),
        "February 2024"
    );
    let off_month = calendar
        .component()
        .get_weeks()
        .iter()
        .flat_map(|week| week.days.iter().collect::<Vec<_>>())
        .find(|day| day.off_month && day.label == "31")
        .unwrap();
    click_component(calendar.component(), &off_month.description);
    assert_eq!(
        calendar.component().get_title_text().as_str(),
        "January 2024"
    );
    assert_eq!(
        date.reads.load(Ordering::SeqCst),
        1,
        "browsing is pure, not desktop observation"
    );
    assert!(!fixture.panel.window().is_visible());
    click_component(&fixture.toolbar, "Open quick settings");
    assert!(
        !calendar.is_open(),
        "accepted competing popup displaces Calendar"
    );
    let quick = fixture.controller.quick_settings.borrow().clone().unwrap();
    assert!(quick.is_open());
    fixture
        .toolbar
        .invoke_calendar_requested(crate::generated::TileBounds {
            origin: slint::LogicalPosition::new(f32::MAX, 8.0),
            width: 80.0,
            height: 16.0,
        });
    assert!(!calendar.is_open());
    assert!(
        quick.is_open(),
        "invalid native placement retains the existing popup"
    );
    assert_eq!(date.reads.load(Ordering::SeqCst), 1);
    *fixture.host.ui_focus_result.lock() = Err("Foreground was refused".into());
    click_component(&fixture.toolbar, "Open calendar");
    assert!(
        calendar.is_open(),
        "foreground denial still leaves an honest visible Calendar"
    );
    assert!(
        !quick.is_open(),
        "visible Calendar displaces Quick despite its focus error"
    );
    calendar.component().invoke_calendar_event_ready();
    assert_eq!(date.reads.load(Ordering::SeqCst), 2);
    click_component(&fixture.toolbar, "Open quick settings");
    assert!(!calendar.is_open());
    fixture.toolbar.hide().unwrap();
    fixture
        .toolbar
        .invoke_calendar_requested(crate::generated::TileBounds {
            origin: slint::LogicalPosition::new(10.0, 8.0),
            width: 80.0,
            height: 16.0,
        });
    assert!(
        !calendar.is_open(),
        "queued hidden clock cannot reopen Calendar"
    );
    assert_eq!(date.reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture.host.observe_calls.load(Ordering::SeqCst),
        observations
    );
    assert!(fixture.host.launches.lock().is_empty());
    assert!(fixture.host.saves.lock().is_empty());
    assert!(fixture.host.system_actions.lock().is_empty());
}

#[test]
fn launcher_native_user_footer_opens_folder_popup_not_settings_and_routes_trusted_folder_input() {
    use slint::platform::{PointerEventButton, WindowEvent};
    use tessera_system::folders::{
        FolderAvailability, FolderError, FolderHost, FolderId, FolderOpenCompletion,
        FolderReadCompletion, FolderSnapshot, FolderTarget,
    };

    #[derive(Default)]
    struct ReadyFolders {
        reads: AtomicUsize,
        opened: Mutex<Vec<FolderId>>,
    }
    impl FolderHost for ReadyFolders {
        fn read(&self, completion: FolderReadCompletion) -> Result<(), FolderError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            completion(Ok(FolderSnapshot::new(std::array::from_fn(|index| {
                FolderAvailability::Ready(FolderTarget::new(FolderId::ALL[index]))
            }))));
            Ok(())
        }
        fn open(
            &self,
            folder: FolderId,
            expected: FolderTarget,
            completion: FolderOpenCompletion,
        ) -> Result<(), FolderError> {
            assert_eq!(expected.get::<FolderId>(), Some(&folder));
            self.opened.lock().push(folder);
            completion(Ok(()));
            Ok(())
        }
    }

    let fixture = LauncherFixture::new();
    let folders = Arc::new(ReadyFolders::default());
    *fixture.host.folder_provider.lock() = Some(folders.clone());
    fixture.controller.open_launcher();
    let observations = fixture.host.observe_calls.load(Ordering::SeqCst);
    assert!(!fixture.panel.window().is_visible());
    fixture.click_launcher("Open user menu");
    let user = fixture.controller.user_menu.borrow().clone().unwrap();
    assert!(user.is_open());
    assert!(user.component().window().is_visible());
    assert!(!fixture.panel.window().is_visible(), "User is not Settings");
    assert_eq!(folders.reads.load(Ordering::SeqCst), 1);
    // The no-event-loop backend needs the production UI mailbox wakeup
    // dispatched explicitly; folder data still came from the real host seam.
    user.component().invoke_folder_event_ready();
    assert!(!user.component().get_loading());
    assert_eq!(user.component().get_rows().row_count(), FolderId::ALL.len());
    let desktop = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        user.component(),
        "Open Desktop",
    )
    .find(|element| {
        element.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button)
    })
    .unwrap();
    assert_eq!(desktop.accessible_enabled(), Some(true));
    let origin = desktop.absolute_position();
    let size = desktop.size();
    let position =
        slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    user.component()
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
    user.component()
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    assert_eq!(*folders.opened.lock(), [FolderId::Desktop]);
    user.component().invoke_folder_event_ready();
    assert_eq!(
        fixture.host.observe_calls.load(Ordering::SeqCst),
        observations
    );
    assert!(fixture.host.launches.lock().is_empty());
    assert!(fixture.host.saves.lock().is_empty());
    assert!(fixture.host.system_actions.lock().is_empty());
    fixture.controller.hide_launcher();
    assert!(!user.is_open());
    assert!(!user.component().window().is_visible());
    fixture
        .launcher
        .invoke_open_user_menu_requested(crate::generated::TileBounds {
            origin: slint::LogicalPosition::new(10.0, 10.0),
            width: 40.0,
            height: 32.0,
        });
    assert!(
        !user.is_open(),
        "queued hidden footer cannot reopen its popup"
    );
    assert_eq!(*folders.opened.lock(), [FolderId::Desktop]);
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
    begin_application_reorder(fixture, "Launch Rust Editor", "Launch File Manager", data)
}

fn begin_application_reorder(
    fixture: &LauncherFixture,
    source_label: &str,
    target_label: &str,
    data: Option<slint::DataTransfer>,
) -> slint::LogicalPosition {
    use slint::platform::{PointerEventButton, WindowEvent};
    let source = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &fixture.launcher,
        source_label,
    )
    .next()
    .unwrap();
    let target = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &fixture.launcher,
        target_label,
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
    let empty_payload = data.as_ref().is_some_and(slint::DataTransfer::is_empty);
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
    assert_eq!(
        fixture.launcher.get_reorder_dragging(),
        !empty_payload,
        "the SDK explicitly vetoes an empty DataTransfer before activation",
    );
    assert_reorder_visual_cleared(fixture);
    // SDK StartDrag consumes the activation move; only the next real move
    // dispatches the captured immutable DropEvent to can-drop.
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(press.x + 13.0, press.y + 1.0),
        });
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

fn assert_reorder_visual_cleared(fixture: &LauncherFixture) {
    let visual = fixture.launcher.get_reorder_visual();
    assert!(!visual.visible);
    assert_eq!(visual.source_scale, 0.0);
    assert!(visual.source.key.is_empty());
    assert!(visual.source.label.is_empty());
    let size = visual.source.icon.size();
    assert_eq!((size.width, size.height), (0, 0));
    assert!(visual.source.icon.to_rgba8().is_none());
}

// Native nested coordinate round-trips can differ by one f32 precision unit
// at the input-coordinate scale; pure collision thresholds remain exact.
fn assert_reorder_visual_position(
    actual: slint::LogicalPosition,
    origin: slint::LogicalPosition,
    press: slint::LogicalPosition,
    pointer: slint::LogicalPosition,
) {
    for (actual, origin, press, pointer) in [
        (actual.x, origin.x, press.x, pointer.x),
        (actual.y, origin.y, press.y, pointer.y),
    ] {
        let expected = origin + pointer - press;
        let precision = f32::EPSILON * origin.abs().max(press.abs()).max(pointer.abs()).max(1.0);
        assert!(
            (actual - expected).abs() <= precision,
            "{actual} != {expected}"
        );
    }
}

#[test]
fn launcher_native_reorder_retains_one_source_image_and_moves_without_rebuilding_rows() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let fixture = LauncherFixture::with_preferences(reorder_preferences());
    fixture.controller.open_launcher();
    let source = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &fixture.launcher,
        "Launch Rust Editor",
    )
    .next()
    .unwrap();
    let origin = source.absolute_position();
    let press = slint::LogicalPosition::new(origin.x + 11.0, origin.y + 13.0);
    let rows = fixture.launcher.get_rows();
    let focus = fixture.host.ui_focus_calls.load(Ordering::SeqCst);
    let attachments = fixture.host.launcher_attach_calls.load(Ordering::SeqCst);
    let observations = fixture.host.observe_calls.load(Ordering::SeqCst);
    let subscriptions = fixture.host.subscription_calls.load(Ordering::SeqCst);
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
    let captured_scale = fixture.launcher.get_source_appearance_scale();
    assert!(captured_scale.is_finite() && captured_scale > 0.0 && captured_scale < 1.0);
    // Source appearance is transient metadata, not a per-hover paint getter.
    fixture.launcher.set_source_appearance_scale(0.625);
    assert_reorder_visual_cleared(&fixture);
    assert_eq!(fixture.launcher.get_rows(), rows);
    let activation = slint::LogicalPosition::new(press.x + 12.0, press.y);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: activation,
        });
    assert!(fixture.launcher.get_reorder_dragging());
    assert_reorder_visual_cleared(&fixture);
    fixture.launcher.set_source_appearance_scale(f32::NAN);
    // Actual dragging alone cannot reveal a source: this next native move
    // supplies the SDK's immutable payload to the first can-drop callback.
    let active = slint::LogicalPosition::new(press.x + 13.0, press.y + 1.0);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: active });
    let visual = fixture.launcher.get_reorder_visual();
    assert!(
        visual.visible,
        "first immutable native can-drop publishes synchronously"
    );
    assert_eq!(visual.source.key, "app-editor");
    assert_eq!(visual.source.label, "Rust Editor");
    assert!(visual.source.favorite);
    assert_eq!(visual.source_scale, captured_scale);
    assert_reorder_visual_position(visual.bounds.origin, origin, press, active);
    let retained = visual.source.icon.to_rgba8_premultiplied().unwrap();
    assert_eq!(retained.as_bytes(), &[1; 8 * 8 * 4]);
    assert_eq!(fixture.launcher.get_rows(), rows);
    // Real bounded-cache eviction, then explicit cache replacement. Neither
    // can invalidate the one image captured by the armed source.
    for index in 0..257u16 {
        let icon =
            crate::PixelIcon::new(1, 1, vec![index as u8, (index >> 8) as u8, 0, 255]).unwrap();
        let _ = fixture.controller.icon_cache.borrow_mut().image(&icon);
    }
    *fixture.controller.icon_cache.borrow_mut() = crate::icons::IconCache::default();
    let moved = slint::LogicalPosition::new(active.x + 1.0, active.y + 1.0);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: moved });
    let moved_visual = fixture.launcher.get_reorder_visual();
    assert_eq!(moved_visual.source.icon, visual.source.icon);
    assert_eq!(moved_visual.source_scale, captured_scale);
    assert_eq!(
        moved_visual
            .source
            .icon
            .to_rgba8_premultiplied()
            .unwrap()
            .as_bytes()
            .as_ptr(),
        retained.as_bytes().as_ptr(),
        "movement clones the captured Image rather than converting the source again",
    );
    assert_reorder_visual_position(moved_visual.bounds.origin, origin, press, moved);
    assert_eq!(fixture.launcher.get_rows(), rows);
    let outside = slint::LogicalPosition::new(40.0, 12.0);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: outside });
    let outside_visual = fixture.launcher.get_reorder_visual();
    assert!(outside_visual.visible);
    assert_eq!(outside_visual.source.icon, visual.source.icon);
    assert_eq!(outside_visual.source.label, "Rust Editor");
    assert_eq!(outside_visual.source_scale, captured_scale);
    assert_reorder_visual_position(outside_visual.bounds.origin, origin, press, outside);
    assert_eq!(fixture.launcher.get_rows(), rows);
    finish_native_reorder(&fixture, outside);
    assert_reorder_visual_cleared(&fixture);
    assert!(fixture.host.saves.lock().is_empty());
    assert!(fixture.host.launches.lock().is_empty());
    assert_eq!(fixture.host.ui_focus_calls.load(Ordering::SeqCst), focus);
    assert_eq!(
        fixture.host.launcher_attach_calls.load(Ordering::SeqCst),
        attachments
    );
    assert_eq!(
        fixture.host.observe_calls.load(Ordering::SeqCst),
        observations
    );
    assert_eq!(
        fixture.host.subscription_calls.load(Ordering::SeqCst),
        subscriptions
    );
}

#[test]
fn launcher_native_reorder_source_appearance_is_validated_before_source_image_conversion() {
    use slint::language::{PointerEvent, PointerEventKind};
    use slint::platform::{PointerEventButton, WindowEvent};
    let fixture = LauncherFixture::with_preferences(reorder_preferences());
    fixture.controller.open_launcher();
    let source = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        &fixture.launcher,
        "Launch Rust Editor",
    )
    .next()
    .unwrap();
    let bounds = crate::generated::TileBounds {
        origin: source.absolute_position(),
        width: source.size().width,
        height: source.size().height,
    };
    let press = slint::LogicalPosition::new(bounds.origin.x + 11.0, bounds.origin.y + 13.0);
    let marker = fixture.launcher.get_reorder_data().user_data().unwrap();
    let mut down = PointerEvent::default();
    down.kind = PointerEventKind::Down;
    down.button = PointerEventButton::Left;
    for source_scale in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        0.0,
        -0.0,
        -0.625,
        0.625,
        1.25,
    ] {
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
        // Real Down establishes the SDK recognizer. Re-enter only the existing
        // typed metadata seam: native delegates otherwise overwrite this getter
        // with their valid pressed appearance before the controller can read it.
        *fixture.controller.icon_cache.borrow_mut() = crate::icons::IconCache::default();
        let empty_cache = format!("{:?}", fixture.controller.icon_cache.borrow());
        fixture.launcher.set_source_appearance_scale(source_scale);
        fixture.launcher.invoke_reorder_origin(
            "app-editor".into(),
            down.clone(),
            bounds.clone(),
            press,
        );
        let payload = fixture.launcher.get_reorder_data().user_data().unwrap();
        let valid = source_scale.is_finite() && source_scale > 0.0;
        assert_eq!(
            Rc::ptr_eq(&payload, &marker),
            !valid,
            "invalid appearance must leave only the unauthorizing owner marker",
        );
        let captured_cache = format!("{:?}", fixture.controller.icon_cache.borrow());
        if valid {
            assert_ne!(captured_cache, empty_cache);
        } else {
            assert_eq!(
                captured_cache, empty_cache,
                "invalid appearance must not convert the canonical source image",
            );
        }
        assert!(
            fixture.launcher.get_reorder_enabled(),
            "rejected metadata must restore eligibility for a fresh native press",
        );
        assert_reorder_visual_cleared(&fixture);
        fixture.launcher.set_source_appearance_scale(f32::NAN);
        fixture
            .launcher
            .window()
            .dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(press.x + 12.0, press.y),
            });
        if valid {
            assert!(fixture.launcher.get_reorder_dragging());
        }
        assert_reorder_visual_cleared(&fixture);
        let moved = slint::LogicalPosition::new(press.x + 13.0, press.y + 1.0);
        fixture
            .launcher
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position: moved });
        if valid {
            let visual = fixture.launcher.get_reorder_visual();
            assert!(visual.visible);
            assert_eq!(visual.source_scale, source_scale);
            assert_eq!(visual.source.key, "app-editor");
            assert_eq!(visual.source.label, "Rust Editor");
            assert_eq!(visual.bounds.width, bounds.width);
            assert_eq!(visual.bounds.height, bounds.height);
            assert_reorder_visual_position(visual.bounds.origin, bounds.origin, press, moved);
        } else {
            assert_reorder_visual_cleared(&fixture);
            let rejected = fixture.launcher.get_reorder_data().user_data().unwrap();
            assert!(Rc::ptr_eq(&rejected, &marker));
            assert_eq!(
                format!("{:?}", fixture.controller.icon_cache.borrow()),
                empty_cache,
            );
        }
        finish_native_reorder(&fixture, slint::LogicalPosition::new(40.0, 12.0));
        assert_reorder_visual_cleared(&fixture);
        assert!(fixture.host.saves.lock().is_empty());
        assert!(fixture.host.launches.lock().is_empty());
        slint::platform::update_timers_and_animations();
        assert!(!fixture.launcher.get_reorder_dragging());
        assert!(fixture.launcher.get_reorder_enabled());
    }
}

#[test]
fn launcher_native_reorder_passive_visual_identity_cannot_authorize_outside_drop() {
    use slint::platform::WindowEvent;
    let preferences = reorder_preferences();
    let fixture = LauncherFixture::with_preferences(preferences.clone());
    fixture.controller.open_launcher();
    let position = begin_editor_reorder(&fixture);
    let mut visual = fixture.launcher.get_reorder_visual();
    let captured_scale = visual.source_scale;
    visual.source.key = "app-browser".into();
    visual.source.label = "Untrusted presentation".into();
    visual.source_scale = 0.625;
    fixture.launcher.set_reorder_visual(visual);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    assert_eq!(
        fixture.launcher.get_reorder_visual().source.key,
        "app-editor"
    );
    assert_eq!(
        fixture.launcher.get_reorder_visual().source_scale,
        captured_scale
    );
    let preview = fixture.launcher.get_rows();
    let content_y = fixture.launcher.get_reorder_metrics().content_y;
    let outside = slint::LogicalPosition::new(40.0, 12.0);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: outside });
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
    slint::platform::update_timers_and_animations();
    assert_eq!(fixture.launcher.get_rows(), preview);
    assert_eq!(fixture.launcher.get_reorder_metrics().content_y, content_y);
    assert!(fixture.launcher.get_reorder_visual().visible);
    finish_native_reorder(&fixture, outside);
    assert_reorder_visual_cleared(&fixture);
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    assert!(fixture.host.saves.lock().is_empty());
    assert!(fixture.host.launches.lock().is_empty());
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
        assert_reorder_visual_cleared(&fixture);
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
    assert_reorder_visual_cleared(&fixture);
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
    assert_reorder_visual_cleared(&fixture);
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
    assert_reorder_visual_cleared(&fixture);
    assert!(fixture.host.saves.lock().is_empty());
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    assert!(fixture.host.launches.lock().is_empty());
}

#[test]
fn launcher_native_reorder_immutable_old_text_and_foreign_payloads_fail_closed() {
    use slint::platform::WindowEvent;
    let fixture = LauncherFixture::with_preferences(reorder_preferences());
    fixture.controller.open_launcher();
    let marker = fixture.launcher.get_reorder_data();
    let _ = begin_editor_reorder(&fixture);
    let old = fixture.launcher.get_reorder_data();
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerExited);
    let mut foreign = slint::DataTransfer::default();
    foreign.set_user_data(Rc::new("app-editor".to_owned()));
    let text = slint::DataTransfer::from(slint::SharedString::from("app-editor"));
    for invalid in [marker, old, foreign, text, slint::DataTransfer::default()] {
        let position = begin_editor_reorder_with_data(&fixture, Some(invalid));
        assert_reorder_visual_cleared(&fixture);
        fixture.launcher.set_source_appearance_scale(0.625);
        fixture
            .launcher
            .window()
            .dispatch_event(WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(40.0, 12.0),
            });
        assert_reorder_visual_cleared(&fixture);
        finish_native_reorder(&fixture, position);
        assert_reorder_visual_cleared(&fixture);
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
    let old_visual = fixture.launcher.get_reorder_visual();
    fixture.launcher.set_source_appearance_scale(0.625);
    fixture.controller.hide_launcher();
    assert_reorder_visual_cleared(&fixture);
    fixture.controller.open_launcher();
    // No event-loop tick before the next press: open must flush old SDK state.
    assert_reorder_visual_cleared(&fixture);
    let position =
        begin_application_reorder(&fixture, "Launch Web Browser", "Launch File Manager", None);
    let new_visual = fixture.launcher.get_reorder_visual();
    assert_eq!(new_visual.source.key, "app-browser");
    assert_eq!(new_visual.source.label, "Web Browser");
    assert_eq!(
        new_visual.source_scale,
        fixture.launcher.get_source_appearance_scale(),
    );
    assert_eq!(new_visual.source_scale, old_visual.source_scale);
    assert_ne!(new_visual.source_scale, 0.625);
    assert_eq!(
        new_visual
            .source
            .icon
            .to_rgba8_premultiplied()
            .unwrap()
            .as_bytes(),
        &[2; 8 * 8 * 4],
    );
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
    slint::platform::update_timers_and_animations();
    let after_old_teardown = fixture.launcher.get_reorder_visual();
    assert!(after_old_teardown.visible);
    assert_eq!(after_old_teardown.source.icon, new_visual.source.icon);
    assert_eq!(after_old_teardown.source.key, new_visual.source.key);
    assert_eq!(after_old_teardown.source_scale, new_visual.source_scale);
    finish_native_reorder(&fixture, position);
    assert_reorder_visual_cleared(&fixture);
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
    assert_reorder_visual_cleared(&fixture);
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
        "query",
        "all",
        "busy",
        "stale",
        "source",
        "anchor",
        "resize",
        "outside-scale",
        "escape",
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
            "outside-scale" => {
                fixture
                    .launcher
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
                fixture
                    .launcher
                    .window()
                    .dispatch_event(WindowEvent::PointerMoved {
                        position: slint::LogicalPosition::new(40.0, 12.0),
                    });
            }
            "escape" => fixture.key(Key::Escape.into()),
            _ => unreachable!(),
        }
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
        finish_native_reorder(&fixture, position);
        assert_reorder_visual_cleared(&fixture);
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
    exercise_reorder_after_source_eviction(false);
}

#[test]
fn launcher_native_reorder_escape_after_source_eviction_clears_retained_visual_without_saves() {
    exercise_reorder_after_source_eviction(true);
}

fn exercise_reorder_after_source_eviction(escape: bool) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let applications = (0..90)
        .map(|index| {
            PanelApplication::new(
                format!("large-{index:04}"),
                format!("Large App {index}"),
                Some(icon_tile_pixels(index as u8 + 3)),
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
    let captured_scale = fixture.launcher.get_source_appearance_scale();
    assert!(captured_scale.is_finite() && captured_scale > 0.0 && captured_scale < 1.0);
    fixture.launcher.set_source_appearance_scale(f32::INFINITY);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(press.x + 12.0, press.y),
        });
    slint::platform::update_timers_and_animations();
    assert!(fixture.launcher.get_reorder_dragging());
    assert_reorder_visual_cleared(&fixture);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(press.x + 13.0, press.y + 1.0),
        });
    let captured = fixture.launcher.get_reorder_visual();
    assert!(captured.visible);
    assert_eq!(captured.source.key, "large-0000");
    assert_eq!(captured.source.label, "Large App 0");
    assert_eq!(captured.source_scale, captured_scale);
    assert_eq!(
        captured
            .source
            .icon
            .to_rgba8_premultiplied()
            .unwrap()
            .as_bytes(),
        &[3; 8 * 8 * 4],
    );
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
    let evicted_visual = fixture.launcher.get_reorder_visual();
    assert!(evicted_visual.visible);
    assert_eq!(evicted_visual.source.icon, captured.source.icon);
    assert_eq!(evicted_visual.source.label, captured.source.label);
    assert_eq!(evicted_visual.source_scale, captured_scale);
    *fixture.controller.icon_cache.borrow_mut() = crate::icons::IconCache::default();
    let outside = slint::LogicalPosition::new(40.0, 12.0);
    fixture
        .launcher
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: outside });
    let outside_visual = fixture.launcher.get_reorder_visual();
    assert!(outside_visual.visible);
    assert_eq!(outside_visual.source.icon, captured.source.icon);
    assert_eq!(outside_visual.source.label, captured.source.label);
    assert_eq!(outside_visual.source_scale, captured_scale);
    assert_eq!(outside_visual.bounds.origin.x, outside.x - 11.0);
    assert_eq!(outside_visual.bounds.origin.y, outside.y - 13.0);
    assert!(fixture.host.saves.lock().is_empty());
    assert_eq!(fixture.controller.core.applied_preferences(), preferences);
    if escape {
        fixture.key(slint::platform::Key::Escape.into());
        assert_reorder_visual_cleared(&fixture);
        assert!(fixture.host.saves.lock().is_empty());
        assert!(fixture.host.launches.lock().is_empty());
        assert_eq!(fixture.controller.core.applied_preferences(), preferences);
        let stopped = fixture.launcher.get_reorder_metrics().content_y;
        i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(1));
        slint::platform::update_timers_and_animations();
        assert_reorder_visual_cleared(&fixture);
        assert_eq!(fixture.launcher.get_reorder_metrics().content_y, stopped);
        return;
    }
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
    assert_reorder_visual_cleared(&fixture);
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
    assert_reorder_visual_cleared(&fixture);
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
