// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::error::Error;

/// A failed startup read disables ordinary saves for this session, protecting
/// damaged or newer records from immediate pin/favorite/display-mode changes.
/// Missing files still load writable defaults. Recovery is deliberately manual.
#[cfg(any(windows, test))]
fn load_preferences(
    store: std::io::Result<crate::settings::SettingsStore>,
) -> (
    Option<crate::settings::SettingsStore>,
    crate::settings::Preferences,
    Option<String>,
) {
    use crate::settings::Preferences;

    match store {
        Ok(store) => match store.load() {
            Ok(preferences) => (Some(store), preferences, None),
            Err(_) => (
                None,
                Preferences::default(),
                Some(
                    "Preferences could not be loaded; saving is disabled. Back up and rename %LOCALAPPDATA%\\Tessera\\settings.json, then restart. Saved data is unchanged; appearance preview still works."
                        .into(),
                ),
            ),
        },
        Err(_) => (
            None,
            Preferences::default(),
            Some("User settings location is unavailable; saving is disabled. Appearance preview still works.".into()),
        ),
    }
}

/// Map the complete applied record, never an appearance preview or partial launcher group.
#[cfg(any(windows, test))]
fn from_ui_preferences(preferences: &tessera_ui::PanelPreferences) -> crate::settings::Preferences {
    use crate::settings::{DockEdge, LauncherDisplayMode, Preferences, Theme};

    let theme = match preferences.theme() {
        tessera_ui::Theme::System => Theme::System,
        tessera_ui::Theme::Light => Theme::Light,
        tessera_ui::Theme::Dark => Theme::Dark,
    };
    let edge = match preferences.dock_edge() {
        tessera_ui::DockEdge::Bottom => DockEdge::Bottom,
        tessera_ui::DockEdge::Top => DockEdge::Top,
        tessera_ui::DockEdge::Left => DockEdge::Left,
        tessera_ui::DockEdge::Right => DockEdge::Right,
    };
    let mode = match preferences.launcher().display_mode() {
        tessera_ui::LauncherDisplayMode::Windowed => LauncherDisplayMode::Windowed,
        tessera_ui::LauncherDisplayMode::Fullscreen => LauncherDisplayMode::Fullscreen,
    };
    let start = match preferences.general().start_of_week() {
        tessera_ui::StartOfWeek::Monday => crate::settings::StartOfWeek::Monday,
        tessera_ui::StartOfWeek::Sunday => crate::settings::StartOfWeek::Sunday,
        tessera_ui::StartOfWeek::Saturday => crate::settings::StartOfWeek::Saturday,
    };
    Preferences::new(theme, preferences.compact())
        .with_source_seed(preferences.source_seed())
        .with_dock(edge, preferences.pinned_apps().to_vec())
        .with_launcher_favorites(preferences.launcher().favorites().to_vec())
        .with_launcher_display_mode(mode)
        .with_general(crate::settings::GeneralPreferences::default().with_start_of_week(start))
        .with_media_enabled(preferences.media_enabled())
        .with_dock_locked(preferences.dock_locked())
        .with_shortcuts(preferences.shortcuts().clone())
}

#[cfg(any(windows, test))]
fn to_ui_preferences(
    preferences: &crate::settings::Preferences,
) -> Result<tessera_ui::PanelPreferences, String> {
    use crate::settings::{DockEdge, LauncherDisplayMode, Theme};

    let theme = match preferences.theme() {
        Theme::System => tessera_ui::Theme::System,
        Theme::Light => tessera_ui::Theme::Light,
        Theme::Dark => tessera_ui::Theme::Dark,
    };
    let edge = match preferences.dock_edge() {
        DockEdge::Bottom => tessera_ui::DockEdge::Bottom,
        DockEdge::Top => tessera_ui::DockEdge::Top,
        DockEdge::Left => tessera_ui::DockEdge::Left,
        DockEdge::Right => tessera_ui::DockEdge::Right,
    };
    let mode = match preferences.launcher_display_mode() {
        LauncherDisplayMode::Windowed => tessera_ui::LauncherDisplayMode::Windowed,
        LauncherDisplayMode::Fullscreen => tessera_ui::LauncherDisplayMode::Fullscreen,
    };
    let start = match preferences.general().start_of_week() {
        crate::settings::StartOfWeek::Monday => tessera_ui::StartOfWeek::Monday,
        crate::settings::StartOfWeek::Sunday => tessera_ui::StartOfWeek::Sunday,
        crate::settings::StartOfWeek::Saturday => tessera_ui::StartOfWeek::Saturday,
    };
    tessera_ui::PanelPreferences::new(theme, preferences.compact())
        .with_source_seed(preferences.source_seed())
        .with_dock(edge, preferences.pinned_apps().to_vec())
        .with_launcher_display_mode(mode)
        .with_general(tessera_ui::GeneralPreferences::default().with_start_of_week(start))
        .with_media_enabled(preferences.media_enabled())
        .with_dock_locked(preferences.dock_locked())
        .with_shortcuts(preferences.shortcuts().clone())
        .with_launcher_favorites(preferences.launcher_favorites().to_vec())
}

#[cfg(windows)]
mod desktop {
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock};
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;
    use tessera_system::audio::{AudioError, AudioHost};
    use tessera_system::battery::{BatteryError, BatteryHost};
    use tessera_system::bluetooth::{BluetoothError, BluetoothHost};
    use tessera_system::calendar::{CalendarError, CalendarHost};
    use tessera_system::display_context::{DisplayContextError, DisplayContextHost};
    use tessera_system::dock_utilities::{DockUtilitiesHost, DockUtilityError};
    use tessera_system::file_search::FileSearchHost;
    use tessera_system::folders::{FolderError, FolderHost};
    use tessera_system::input_language::{InputLanguageError, InputLanguageHost};
    use tessera_system::media::{MediaError, MediaHost};
    use tessera_system::network::{NetworkError, NetworkHost};
    use tessera_system::power::{PowerError, PowerHost};
    use tessera_system::power_updates::{PowerUpdatesError, PowerUpdatesHost};
    use tessera_system::profile::{ProfileError, ProfileHost};
    use tessera_system::recycle_bin::RecycleBinHost;
    use tessera_system::recycle_bin_mutation::RecycleBinMutationHost;
    use tessera_system::shortcuts::{ShortcutError, ShortcutHost};
    use tessera_system::visibility::{PointerHost, PointerWatchError};
    use tessera_system::web_search::WebSearchHost;
    use tessera_ui::{
        DesktopHost, DockContext, PanelApplication, PanelPreferences, PanelSnapshot, PanelWindow,
        PixelIcon, RunOptions, ShellIdentity, SurfaceKind, SurfaceMode, SystemAction,
    };
    use tessera_windows::{ActivationTarget, Application, IconPixels};

    use crate::provider::Provider;
    use crate::settings::SettingsStore;

    struct CatalogCache {
        checked: Instant,
        entries: Vec<Application>,
        presentation: Vec<PanelApplication>,
        failed: bool,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) enum Presentation {
        Utility,
        Desktop,
        Diagnostic,
    }

    struct CachedWindowIcon {
        checked: Instant,
        pixels: Option<PixelIcon>,
    }

    struct AppHost {
        targets: Mutex<HashMap<String, ActivationTarget>>,
        catalog: Mutex<Option<CatalogCache>>,
        settings: Option<SettingsStore>,
        presentation: Presentation,
        window_icons: Mutex<HashMap<String, CachedWindowIcon>>,
        audio: LazyLock<Provider<dyn AudioHost>>,
        battery: LazyLock<Provider<dyn BatteryHost>>,
        folders: LazyLock<Provider<dyn FolderHost>>,
        calendar: LazyLock<Provider<dyn CalendarHost>>,
        dock_utilities: LazyLock<Provider<dyn DockUtilitiesHost>>,
        recycle_bin: LazyLock<Provider<dyn RecycleBinHost>>,
        recycle_bin_mutation: LazyLock<Provider<dyn RecycleBinMutationHost>>,
        display_context: LazyLock<Provider<dyn DisplayContextHost>>,
        power: LazyLock<Provider<dyn PowerHost>>,
        power_updates: LazyLock<Provider<dyn PowerUpdatesHost>>,
        network: LazyLock<Provider<dyn NetworkHost>>,
        bluetooth: LazyLock<Provider<dyn BluetoothHost>>,
        input_language: LazyLock<Provider<dyn InputLanguageHost>>,
        media: LazyLock<Provider<dyn MediaHost>>,
        web_search: LazyLock<Option<Arc<dyn WebSearchHost>>>,
        file_search: LazyLock<Option<Arc<dyn FileSearchHost>>>,
        pointer: LazyLock<Provider<dyn PointerHost>>,
        shortcuts: LazyLock<Provider<dyn ShortcutHost>>,
        profile: LazyLock<Provider<dyn ProfileHost>>,
    }

    fn icon(pixels: &IconPixels) -> Option<PixelIcon> {
        PixelIcon::new(pixels.width(), pixels.height(), pixels.rgba().to_vec())
    }

    fn native_window_handle(window: &slint::Window) -> Result<std::num::NonZeroIsize, String> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = window.window_handle();
        let native = handle.window_handle().map_err(|error| error.to_string())?;
        match native.as_raw() {
            RawWindowHandle::Win32(native) => Ok(native.hwnd),
            _ => Err("The desktop surface is not a native Windows window".into()),
        }
    }

    impl AppHost {
        // WinEvent bursts must not repeat synchronous icon extraction for
        // every retained window. Cache bounded identities, not HWNDs alone.
        fn window_icon(&self, key: &str, target: ActivationTarget) -> Option<PixelIcon> {
            const LIMIT: usize = 256;
            let mut icons = self.window_icons.lock();
            if let Some(cached) = icons.get(key) {
                if cached.checked.elapsed() < Duration::from_secs(300) {
                    return cached.pixels.clone();
                }
            } else if icons.len() >= LIMIT {
                return None;
            }
            let pixels = tessera_windows::window_icon(target).as_ref().and_then(icon);
            icons.insert(
                key.to_owned(),
                CachedWindowIcon {
                    checked: Instant::now(),
                    pixels: pixels.clone(),
                },
            );
            pixels
        }

        // COM catalog/icon work stays on the observation worker and is cached independently
        // of cheap desktop notifications. Failures cannot take away window switching.
        fn applications(&self) -> (Vec<PanelApplication>, bool) {
            let mut cache = self.catalog.lock();
            if cache
                .as_ref()
                .is_none_or(|cache| cache.checked.elapsed() >= Duration::from_secs(60))
            {
                let (entries, failed) = match tessera_windows::catalog() {
                    Ok(entries) => (entries, false),
                    Err(_) => (Vec::new(), true),
                };
                let presentation = entries
                    .iter()
                    .filter_map(|app| {
                        PanelApplication::new(
                            app.id().to_owned(),
                            app.name().to_owned(),
                            app.icon().and_then(icon),
                        )
                    })
                    .collect();
                *cache = Some(CatalogCache {
                    checked: Instant::now(),
                    entries,
                    presentation,
                    failed,
                });
            }
            let cache = cache.as_ref().expect("Catalog is initialized above");
            (cache.presentation.clone(), cache.failed)
        }

        fn window_target(&self, key: &str) -> Result<ActivationTarget, String> {
            self.targets.lock().get(key).copied().ok_or_else(|| {
                "This window is no longer in the observation. Refresh and retry".to_owned()
            })
        }
    }

    impl DesktopHost for AppHost {
        fn shortcuts_host(&self) -> Result<Option<Arc<dyn ShortcutHost>>, ShortcutError> {
            self.shortcuts
                .get(tessera_windows::shortcuts::native_shortcuts_host)
                .map(Some)
        }

        fn profile_host(&self) -> Result<Option<Arc<dyn ProfileHost>>, ProfileError> {
            self.profile
                .get(tessera_windows::profile::native_profile_host)
                .map(Some)
        }

        fn audio_host(&self) -> Result<Option<Arc<dyn AudioHost>>, AudioError> {
            self.audio
                .get(|| {
                    tessera_windows::AudioService::new()
                        .map(|host| -> Arc<dyn AudioHost> { Arc::new(host) })
                })
                .map(Some)
        }

        fn folder_host(&self) -> Result<Option<Arc<dyn FolderHost>>, FolderError> {
            self.folders
                .get(tessera_windows::folders::native_folder_host)
                .map(Some)
        }

        fn calendar_host(&self) -> Result<Option<Arc<dyn CalendarHost>>, CalendarError> {
            self.calendar
                .get(tessera_windows::calendar::native_calendar_host)
                .map(Some)
        }
        fn display_context_host(
            &self,
        ) -> Result<Option<Arc<dyn DisplayContextHost>>, DisplayContextError> {
            self.display_context
                .get(tessera_windows::display_context::native_display_context_host)
                .map(Some)
        }

        fn power_host(&self) -> Result<Option<Arc<dyn PowerHost>>, PowerError> {
            self.power
                .get(tessera_windows::power::native_power_host)
                .map(Some)
        }

        fn power_updates_host(
            &self,
        ) -> Result<Option<Arc<dyn PowerUpdatesHost>>, PowerUpdatesError> {
            self.power_updates
                .get(|| Ok(tessera_windows::power_updates::native_power_updates_host()))
                .map(Some)
        }

        fn battery_host(&self) -> Result<Option<Arc<dyn BatteryHost>>, BatteryError> {
            if self.presentation != Presentation::Desktop {
                return Ok(None);
            }
            self.battery
                .get(tessera_windows::battery::native_battery_host)
                .map(Some)
        }

        fn network_host(&self) -> Result<Option<Arc<dyn NetworkHost>>, NetworkError> {
            self.network
                .get(tessera_windows::network::native_network_host)
                .map(Some)
        }

        fn bluetooth_host(&self) -> Result<Option<Arc<dyn BluetoothHost>>, BluetoothError> {
            self.bluetooth
                .get(|| Ok(tessera_windows::bluetooth::native_bluetooth_host()))
                .map(Some)
        }

        fn input_language_host(
            &self,
        ) -> Result<Option<Arc<dyn InputLanguageHost>>, InputLanguageError> {
            self.input_language
                .get(tessera_windows::input_language::native_input_language_host)
                .map(Some)
        }

        fn media_host(&self) -> Result<Option<Arc<dyn MediaHost>>, MediaError> {
            self.media
                .get(|| {
                    tessera_windows::media::MediaService::new()
                        .map(|host| -> Arc<dyn MediaHost> { Arc::new(host) })
                })
                .map(Some)
        }

        fn web_search_host(&self) -> Option<Arc<dyn WebSearchHost>> {
            if self.presentation != Presentation::Desktop {
                return None;
            }
            self.web_search.as_ref().map(Arc::clone)
        }

        fn file_search_host(&self) -> Option<Arc<dyn FileSearchHost>> {
            if self.presentation != Presentation::Desktop {
                return None;
            }
            self.file_search.as_ref().map(Arc::clone)
        }

        fn pointer_host(&self) -> Result<Option<Arc<dyn PointerHost>>, PointerWatchError> {
            if self.presentation != Presentation::Desktop {
                return Ok(None);
            }
            self.pointer
                .get(tessera_windows::visibility::native_pointer_host)
                .map(Some)
        }

        fn dock_utilities_host(
            &self,
        ) -> Result<Option<Arc<dyn DockUtilitiesHost>>, DockUtilityError> {
            self.dock_utilities
                .get(tessera_windows::dock_utilities::native_dock_utilities_host)
                .map(Some)
        }

        fn recycle_bin_host(&self) -> Result<Option<Arc<dyn RecycleBinHost>>, DockUtilityError> {
            self.recycle_bin
                .get(tessera_windows::recycle_bin::native_recycle_bin_host)
                .map(Some)
        }

        fn recycle_bin_mutation_host(
            &self,
        ) -> Result<Option<Arc<dyn RecycleBinMutationHost>>, DockUtilityError> {
            self.recycle_bin_mutation
                .get(tessera_windows::recycle_bin_mutation::native_recycle_bin_mutation_host)
                .map(Some)
        }

        fn observe(&self) -> Result<PanelSnapshot, String> {
            let snapshot = tessera_windows::observe().map_err(|error| error.to_string())?;
            let candidates: Vec<_> = snapshot
                .windows()
                .iter()
                .filter_map(|window| {
                    ActivationTarget::from_window(window).map(|target| {
                        // Snapshot-local identity is never a persisted application pin.
                        let key = format!("{}:{}", target.window_id().value(), target.process_id());
                        (key, target, window)
                    })
                })
                .collect();
            let targets = candidates
                .iter()
                .map(|(key, target, _)| (key.clone(), *target))
                .collect::<HashMap<_, _>>();
            // Free closed identities before extracting icons for newly opened windows.
            self.window_icons
                .lock()
                .retain(|key, _| targets.contains_key(key));
            let windows = candidates
                .into_iter()
                .map(|(key, target, window)| {
                    let pixels = self.window_icon(&key, target);
                    PanelWindow::new(key, window.title().to_owned(), window.minimized())
                        .with_icon(pixels)
                        .with_application_identity(
                            snapshot
                                .window_application_identity(window.id())
                                .map(str::to_owned),
                        )
                })
                .collect();
            *self.targets.lock() = targets;
            let (applications, catalog_failed) = self.applications();
            let mut view = PanelSnapshot::new(
                snapshot.monitors().len(),
                windows,
                snapshot.warnings().len() + usize::from(catalog_failed),
            )
            .with_applications(applications);
            if let Some(primary) = snapshot
                .monitors()
                .iter()
                .find(|monitor| monitor.primary())
                .or_else(|| snapshot.monitors().first())
            {
                let foreground = snapshot.foreground_window_id();
                let fullscreen = snapshot
                    .visibility_windows()
                    .unwrap_or(snapshot.windows())
                    .iter()
                    .any(|window| {
                        Some(window.id()) == foreground
                            && window.monitor_id() == Some(primary.id())
                            && window.covers_monitor()
                            && !window.maximized()
                            && ActivationTarget::from_window(window).is_some()
                    });
                let area = primary.bounds();
                if let Some(context) =
                    DockContext::new(area.x(), area.y(), area.width(), area.height(), fullscreen)
                {
                    view = view.with_dock_context(context);
                    if let (Some(windows), Some(foreground_interactable)) = (
                        snapshot.visibility_windows(),
                        snapshot.foreground_interactable(),
                    ) {
                        view = view.with_visibility_windows(
                            windows
                                .iter()
                                .map(|window| tessera_system::visibility::VisibilityWindow {
                                    bounds: window.bounds(),
                                    belongs_to_monitor: window.monitor_id() == Some(primary.id()),
                                    minimized: window.minimized(),
                                })
                                .collect(),
                            foreground_interactable,
                        );
                    }
                }
            }
            Ok(view)
        }

        fn shell_identity(&self) -> Result<ShellIdentity, String> {
            let identity =
                tessera_windows::desktop_identity().map_err(|error| error.to_string())?;
            let focused_window_key = self
                .targets
                .lock()
                .iter()
                .find(|(_, target)| Some(target.window_id()) == identity.foreground_window)
                .map(|(key, _)| key.clone());
            Ok(ShellIdentity {
                user_name: identity.user_name,
                clock: identity.clock,
                language: identity.language,
                focused_window_key,
            })
        }

        fn clock_text(&self) -> Result<String, String> {
            tessera_windows::clock_text().map_err(|error| error.to_string())
        }

        fn configure_surface(
            &self,
            kind: SurfaceKind,
            window: &slint::Window,
        ) -> Result<Option<Box<dyn std::any::Any>>, String> {
            if self.presentation != Presentation::Desktop {
                return Ok(None);
            }
            let handle = native_window_handle(window)?;
            if kind == SurfaceKind::Tooltip {
                use slint::winit_030::WinitWindowAccessor;
                window
                    .with_winit_window(|native| native.set_cursor_hittest(false))
                    .ok_or("The tooltip native window is unavailable")?
                    .map_err(|error| format!("Tooltip cursor pass-through failed: {error}"))?;
            }
            let kind = match kind {
                SurfaceKind::Dock => tessera_windows::ShellSurfaceKind::Dock,
                SurfaceKind::Toolbar => tessera_windows::ShellSurfaceKind::Toolbar,
                SurfaceKind::Launcher => tessera_windows::ShellSurfaceKind::Launcher,
                SurfaceKind::Popup => tessera_windows::ShellSurfaceKind::Popup,
                SurfaceKind::Tooltip => tessera_windows::ShellSurfaceKind::Tooltip,
            };
            let lease = tessera_windows::OwnedShellSurface::attach(handle.get(), kind)
                .map_err(|error| error.to_string())?;
            Ok(Some(Box::new(lease)))
        }

        fn request_ui_focus(&self, window: &slint::Window) -> Result<(), String> {
            if self.presentation != Presentation::Desktop {
                return Ok(());
            }
            let handle = native_window_handle(window)?;
            match tessera_windows::request_owned_foreground(handle.get())
                .map_err(|error| error.to_string())?
            {
                true => Ok(()),
                false => Err("Windows declined activation; click the window to focus it".into()),
            }
        }

        fn activate(&self, key: &str) -> Result<(), String> {
            let target = self.window_target(key)?;
            tessera_windows::activate(target).map_err(|error| error.to_string())
        }

        fn window_action(&self, key: &str, action: tessera_ui::WindowAction) -> Result<(), String> {
            let target = self.window_target(key)?;
            let action = match action {
                tessera_ui::WindowAction::Activate => tessera_windows::WindowAction::Activate,
                tessera_ui::WindowAction::ActivateOrMinimize => {
                    tessera_windows::WindowAction::ActivateOrMinimize
                }
                tessera_ui::WindowAction::Minimize => tessera_windows::WindowAction::Minimize,
                tessera_ui::WindowAction::Close => tessera_windows::WindowAction::Close,
            };
            tessera_windows::window_action(target, action).map_err(|error| error.to_string())
        }

        fn window_preview(
            &self,
            key: &str,
            destination: &slint::Window,
            bounds: tessera_core::Rect,
        ) -> Result<Option<Box<dyn std::any::Any>>, String> {
            let target = self.window_target(key)?;
            let handle = native_window_handle(destination)?;
            let preview = tessera_windows::window_preview::OwnedWindowPreview::attach(
                handle.get(),
                target,
                bounds,
            )?;
            Ok(Some(Box::new(preview)))
        }

        fn launch(&self, key: &str) -> Result<(), String> {
            let app = self
                .catalog
                .lock()
                .as_ref()
                .and_then(|cache| cache.entries.iter().find(|app| app.id() == key))
                .cloned()
                .ok_or_else(|| {
                    "This application is not in the trusted catalog. Refresh and retry".to_owned()
                })?;
            // Release the catalog lock before native shell activation.
            tessera_windows::launch(&app).map_err(|error| error.to_string())
        }

        fn system_action(&self, action: SystemAction) -> Result<(), String> {
            if action == SystemAction::RestoreExplorer && self.presentation != Presentation::Utility
            {
                // The independent owner restores taskbar presentation and, in
                // sign-in-shell mode, the original registry value after GUI exit.
                return slint::quit_event_loop().map_err(|error| error.to_string());
            }
            match action {
                SystemAction::OpenFileManager => tessera_windows::open_file_manager(),
                SystemAction::OpenTaskManager => tessera_windows::open_task_manager(),
                SystemAction::RestoreExplorer => tessera_windows::restore_explorer(),
            }
            .map_err(|error| error.to_string())
        }

        fn ui_animations_enabled(&self) -> bool {
            tessera_windows::ui_animations_enabled().unwrap_or(false)
        }

        fn subscribe_ui_motion(
            &self,
            callback: Arc<dyn Fn(bool) + Send + Sync>,
        ) -> Result<Option<Box<dyn Send>>, String> {
            tessera_windows::watch_ui_motion(callback)
                .map(|watcher| Some(Box::new(watcher) as Box<dyn Send>))
                .map_err(|error| error.to_string())
        }

        fn subscribe(
            &self,
            callback: Arc<dyn Fn() + Send + Sync>,
        ) -> Result<Option<Box<dyn Send>>, String> {
            tessera_windows::watch_desktop(callback)
                .map(|guard| Some(Box::new(guard) as Box<dyn Send>))
                .map_err(|error| error.to_string())
        }

        fn save_preferences(&self, preferences: &PanelPreferences) -> Result<(), String> {
            let store = self.settings.as_ref().ok_or_else(|| {
                "Preference saving is unavailable for this session; preview still works. Check the startup notice and restart after recovery.".to_owned()
            })?;
            store
                .save(&super::from_ui_preferences(preferences))
                .map_err(|error| error.to_string())
        }
    }

    pub(super) fn run(
        presentation: Presentation,
        heartbeat: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let diagnostic = presentation == Presentation::Diagnostic;
        // The production UI runner owns backend selection and its passive
        // first-show hook; selecting here would recreate Winit's event loop.
        let heartbeat = crate::diagnostic::startup_phase(
            diagnostic,
            crate::diagnostic::StartupPhase::HeartbeatEventConnect,
            || {
                heartbeat
                    .map(tessera_windows::ShellHeartbeat::connect)
                    .transpose()
                    .map_err(Into::into)
            },
        )?
        .map(|event| {
            // The independent supervisor handles a failed/absent heartbeat by restoring Explorer.
            Arc::new(move || {
                let _ = event.pulse();
            }) as Arc<dyn Fn() + Send + Sync>
        });
        if diagnostic {
            return crate::diagnostic::run(heartbeat);
        }
        let (settings, preferences, notice) =
            super::load_preferences(SettingsStore::for_current_user());
        let host = AppHost {
            targets: Mutex::new(HashMap::new()),
            catalog: Mutex::new(None),
            settings,
            presentation,
            window_icons: Mutex::new(HashMap::new()),
            audio: LazyLock::new(Provider::default),
            battery: LazyLock::new(Provider::default),
            folders: LazyLock::new(Provider::default),
            calendar: LazyLock::new(Provider::default),
            dock_utilities: LazyLock::new(Provider::default),
            recycle_bin: LazyLock::new(Provider::default),
            recycle_bin_mutation: LazyLock::new(Provider::default),
            display_context: LazyLock::new(Provider::default),
            power: LazyLock::new(Provider::default),
            power_updates: LazyLock::new(Provider::default),
            network: LazyLock::new(Provider::default),
            bluetooth: LazyLock::new(Provider::default),
            input_language: LazyLock::new(Provider::default),
            media: LazyLock::new(Provider::default),
            web_search: LazyLock::new(tessera_windows::web_search::native_web_search_host),
            file_search: LazyLock::new(tessera_windows::file_search::native_file_search_host),
            pointer: LazyLock::new(Provider::default),
            shortcuts: LazyLock::new(Provider::default),
            profile: LazyLock::new(Provider::default),
        };
        tessera_ui::run(
            host,
            super::to_ui_preferences(&preferences)?,
            notice,
            RunOptions {
                surface: if presentation == Presentation::Utility {
                    SurfaceMode::Panel
                } else {
                    SurfaceMode::Dock
                },
                heartbeat,
                global_shortcuts_enabled: true,
            },
        )?;
        Ok(())
    }
}

#[cfg(windows)]
pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    desktop::run(desktop::Presentation::Utility, None)
}

#[cfg(windows)]
pub(crate) fn run_desktop(heartbeat: Option<&str>) -> Result<(), Box<dyn Error>> {
    match heartbeat {
        Some(event) => desktop::run(desktop::Presentation::Desktop, Some(event)),
        None => tessera_windows::start_desktop_session().map_err(Into::into),
    }
}

#[cfg(windows)]
pub(crate) fn run_desktop_diagnostic(heartbeat: &str) -> Result<(), Box<dyn Error>> {
    desktop::run(desktop::Presentation::Diagnostic, Some(heartbeat))
}

#[cfg(not(windows))]
pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    Err(tessera_windows::ObservationError::UnsupportedPlatform.into())
}

#[cfg(not(windows))]
pub(crate) fn run_desktop(_heartbeat: Option<&str>) -> Result<(), Box<dyn Error>> {
    run()
}

#[cfg(not(windows))]
pub(crate) fn run_desktop_diagnostic(_heartbeat: &str) -> Result<(), Box<dyn Error>> {
    run()
}

#[cfg(test)]
mod preference_tests {
    use super::*;
    use crate::settings::{Preferences, SettingsStore};
    use tessera_system::shortcuts::{KeyChord, KeyModifiers, ShortcutConfig};

    fn custom_shortcuts() -> ShortcutConfig {
        ShortcutConfig::default()
            .with_enabled(false)
            .with_settings_override(Some(
                KeyChord::new(
                    KeyModifiers {
                        control: true,
                        shift: true,
                        ..KeyModifiers::default()
                    },
                    0x50,
                )
                .unwrap(),
            ))
            .unwrap()
    }

    #[test]
    fn invalid_or_future_startup_records_disable_all_normal_preference_saves() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        for original in [
            "damaged",
            r#"{"schema_version":9,"theme":"dark","compact":true}"#,
            r#"{"schema_version":2,"theme":"dark","compact":true}"#,
            r#"{"schema_version":3,"theme":"dark","compact":true,"dock_edge":"bottom","pinned_apps":[],"launcher":{"display_mode":"normal","favorites":[]}}"#,
            r#"{"schema_version":3,"theme":"dark","compact":true,"dock_edge":"bottom","pinned_apps":[],"launcher":{"display_mode":"fullscreen","favorites":[],"unknown":true}}"#,
            r#"{"schema_version":3,"theme":"dark","compact":true,"dock_edge":"bottom","pinned_apps":[],"launcher":["fullscreen",["A"]]}"#,
            r#"{"schema_version":4,"theme":"dark","compact":true,"dock_edge":"bottom","pinned_apps":[],"launcher":{"display_mode":"windowed","favorites":[]},"dock":{"media_enabled":false}}"#,
            r#"{"schema_version":4,"theme":"dark","compact":true,"dock_edge":"bottom","pinned_apps":[],"launcher":{"display_mode":"windowed","favorites":[]},"general":{"start_of_week":"friday"},"dock":{"media_enabled":false}}"#,
            r#"{"schema_version":4,"theme":"dark","compact":true,"dock_edge":"bottom","pinned_apps":[],"launcher":{"display_mode":"windowed","favorites":[]},"general":{"start_of_week":"monday"}}"#,
            r#"{"schema_version":4,"theme":"dark","compact":true,"dock_edge":"bottom","pinned_apps":[],"launcher":{"display_mode":"windowed","favorites":[]},"general":{"start_of_week":"monday"},"dock":{"media_enabled":"false"}}"#,
        ] {
            std::fs::write(&path, original).unwrap();
            let (store, preferences, notice) =
                load_preferences(Ok(SettingsStore::new(path.clone())));
            assert!(store.is_none());
            assert_eq!(preferences, Preferences::default());
            let notice = notice.unwrap();
            assert!(notice.len() <= 200);
            assert!(notice.contains("Back up and rename"));
            assert!(notice.contains("restart"));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }

    #[test]
    fn missing_startup_record_remains_writable_without_implicit_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let (store, preferences, notice) = load_preferences(Ok(SettingsStore::new(path.clone())));
        assert!(store.is_some());
        assert_eq!(preferences, Preferences::default());
        assert!(notice.is_none());
        assert!(!path.exists());
    }

    #[test]
    fn complete_preferences_map_both_directions_without_losing_launcher_mode_or_tail() {
        use crate::settings::{
            DockEdge, GeneralPreferences, LauncherDisplayMode, StartOfWeek, Theme,
        };

        let mut favorites: Vec<_> = (0..80).map(|index| format!("favorite-{index}")).collect();
        favorites.extend(["missing-app".into(), "exact".into(), "EXACT".into()]);
        for (theme, ui_theme) in [
            (Theme::System, tessera_ui::Theme::System),
            (Theme::Light, tessera_ui::Theme::Light),
            (Theme::Dark, tessera_ui::Theme::Dark),
        ] {
            for (edge, ui_edge) in [
                (DockEdge::Bottom, tessera_ui::DockEdge::Bottom),
                (DockEdge::Top, tessera_ui::DockEdge::Top),
                (DockEdge::Left, tessera_ui::DockEdge::Left),
                (DockEdge::Right, tessera_ui::DockEdge::Right),
            ] {
                for (mode, ui_mode) in [
                    (
                        LauncherDisplayMode::Windowed,
                        tessera_ui::LauncherDisplayMode::Windowed,
                    ),
                    (
                        LauncherDisplayMode::Fullscreen,
                        tessera_ui::LauncherDisplayMode::Fullscreen,
                    ),
                ] {
                    for compact in [false, true] {
                        for (start, ui_start) in [
                            (StartOfWeek::Monday, tessera_ui::StartOfWeek::Monday),
                            (StartOfWeek::Sunday, tessera_ui::StartOfWeek::Sunday),
                            (StartOfWeek::Saturday, tessera_ui::StartOfWeek::Saturday),
                        ] {
                            for media_enabled in [false, true] {
                                let stored = Preferences::new(theme, compact)
                                    .with_dock(edge, vec!["dock-only".into()])
                                    .with_launcher_favorites(favorites.clone())
                                    .with_launcher_display_mode(mode)
                                    .with_general(
                                        GeneralPreferences::default().with_start_of_week(start),
                                    )
                                    .with_media_enabled(media_enabled)
                                    .with_shortcuts(custom_shortcuts());
                                let ui = to_ui_preferences(&stored).unwrap();
                                assert_eq!(ui.theme(), ui_theme);
                                assert_eq!(ui.compact(), compact);
                                assert_eq!(ui.dock_edge(), ui_edge);
                                assert_eq!(ui.launcher().display_mode(), ui_mode);
                                assert_eq!(ui.launcher().favorites(), favorites);
                                assert_eq!(ui.pinned_apps(), ["dock-only"]);
                                assert_eq!(ui.general().start_of_week(), ui_start);
                                assert_eq!(ui.media_enabled(), media_enabled);
                                assert_eq!(ui.shortcuts(), &custom_shortcuts());
                                assert_eq!(from_ui_preferences(&ui), stored);
                                assert_eq!(
                                    to_ui_preferences(&from_ui_preferences(&ui)).unwrap(),
                                    ui
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn applied_appearance_pin_and_favorite_edits_keep_complete_group_through_storage_mapping() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(directory.path().join("settings.json"));
        let favorites: Vec<_> = (0..80).map(|index| format!("favorite-{index}")).collect();
        let applied = tessera_ui::PanelPreferences::new(tessera_ui::Theme::Dark, true)
            .with_source_seed(tessera_core::SourceSeed::from_rgb(0x123456).unwrap())
            .with_dock(tessera_ui::DockEdge::Left, vec!["dock-only".into()])
            .with_launcher_favorites(favorites.clone())
            .unwrap()
            .with_launcher_display_mode(tessera_ui::LauncherDisplayMode::Fullscreen)
            .with_general(
                tessera_ui::GeneralPreferences::default()
                    .with_start_of_week(tessera_ui::StartOfWeek::Saturday),
            )
            .with_media_enabled(true)
            .with_shortcuts(custom_shortcuts());
        for edited in [
            applied.clone().with_appearance(
                tessera_ui::Theme::Light,
                false,
                tessera_ui::DockEdge::Right,
            ),
            applied
                .clone()
                .with_dock(tessera_ui::DockEdge::Top, vec!["new-dock".into()]),
            applied
                .clone()
                .with_launcher_favorites(vec!["missing-app".into()])
                .unwrap(),
            applied
                .clone()
                .with_shortcuts(ShortcutConfig::default().with_enabled(false)),
        ] {
            store.save(&from_ui_preferences(&edited)).unwrap();
            let reloaded = to_ui_preferences(&store.load().unwrap()).unwrap();
            assert_eq!(reloaded, edited);
            assert_eq!(reloaded.source_seed().rgb(), 0x123456);
            assert_eq!(
                reloaded.launcher().display_mode(),
                tessera_ui::LauncherDisplayMode::Fullscreen
            );
        }
        let windowed =
            applied.with_launcher_display_mode(tessera_ui::LauncherDisplayMode::Windowed);
        store.save(&from_ui_preferences(&windowed)).unwrap();
        assert_eq!(to_ui_preferences(&store.load().unwrap()).unwrap(), windowed);
    }

    #[test]
    fn every_shortcut_configuration_maps_bijectively_and_retains_all_saved_groups() {
        for bits in 0..16 {
            let modifiers = KeyModifiers {
                control: bits & 1 != 0,
                alt: bits & 2 != 0,
                shift: bits & 4 != 0,
                win: bits & 8 != 0,
            };
            for enabled in [false, true] {
                for chord in [None, Some(KeyChord::new(modifiers, 0x4B).unwrap())] {
                    let config = ShortcutConfig::default()
                        .with_enabled(enabled)
                        .with_settings_override(chord)
                        .unwrap();
                    let stored = Preferences::new(crate::settings::Theme::Dark, true)
                        .with_source_seed(tessera_core::SourceSeed::from_rgb(0x123456).unwrap())
                        .with_dock(crate::settings::DockEdge::Right, vec!["dock-only".into()])
                        .with_launcher_favorites(vec!["exact".into(), "EXACT".into()])
                        .with_launcher_display_mode(
                            crate::settings::LauncherDisplayMode::Fullscreen,
                        )
                        .with_general(
                            crate::settings::GeneralPreferences::default()
                                .with_start_of_week(crate::settings::StartOfWeek::Saturday),
                        )
                        .with_media_enabled(true)
                        .with_shortcuts(config.clone());
                    let ui = to_ui_preferences(&stored).unwrap();
                    assert_eq!(ui.shortcuts(), &config);
                    assert_eq!(from_ui_preferences(&ui), stored);
                    assert_eq!(to_ui_preferences(&from_ui_preferences(&ui)).unwrap(), ui);
                }
            }
        }
    }

    #[test]
    fn invalid_v6_shortcuts_disable_saves_without_replacing_private_or_future_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let valid = serde_json::to_string(&Preferences::default()).unwrap();
        for original in [
            valid.replace(
                r#","shortcuts":{"enabled":true,"settings_override":null}"#,
                "",
            ),
            valid.replace(
                r#""settings_override":null"#,
                r#""settings_override":"bare_win""#,
            ),
            valid.replace(
                r#""settings_override":null"#,
                r#""settings_override":null,"personal_email":"private@example.invalid""#,
            ),
            valid.replace(r#""schema_version":6"#, r#""schema_version":7"#),
            valid.replace(r#""source_seed":8168540"#, r#""source_seed":16777216"#),
            valid.replace(r#""source_seed":8168540,"#, ""),
        ] {
            assert_ne!(original, valid);
            std::fs::write(&path, &original).unwrap();
            let (store, preferences, notice) =
                load_preferences(Ok(SettingsStore::new(path.clone())));
            assert!(store.is_none());
            assert_eq!(preferences, Preferences::default());
            assert!(notice.unwrap().contains("saving is disabled"));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }
}
