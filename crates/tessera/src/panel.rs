// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::error::Error;

/// A failed startup read disables ordinary saves for this session, protecting
/// damaged or newer records from immediate pin/favorite changes. Missing files
/// still load writable defaults. Recovery is deliberately manual.
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

#[cfg(windows)]
mod desktop {
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock};
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;
    use tessera_system::audio::{AudioError, AudioHost};
    use tessera_ui::{
        DesktopHost, DockContext, PanelApplication, PanelPreferences, PanelSnapshot, PanelWindow,
        PixelIcon, RunOptions, ShellIdentity, SurfaceKind, SurfaceMode, SystemAction,
    };
    use tessera_windows::{ActivationTarget, Application, IconPixels};

    use crate::audio_provider::AudioProvider;
    use crate::settings::{DockEdge, Preferences, SettingsStore, Theme};

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
        audio: LazyLock<AudioProvider>,
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
        fn audio_host(&self) -> Result<Option<Arc<dyn AudioHost>>, AudioError> {
            self.audio
                .get(|| {
                    tessera_windows::AudioService::new()
                        .map(|host| -> Arc<dyn AudioHost> { Arc::new(host) })
                })
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
                let foreground = tessera_windows::foreground_window_id();
                let fullscreen = snapshot.windows().iter().any(|window| {
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
            store
                .save(
                    &Preferences::new(theme, preferences.compact())
                        .with_dock(edge, preferences.pinned_apps().to_vec())
                        .with_launcher_favorites(preferences.launcher_favorites().to_vec()),
                )
                .map_err(|error| error.to_string())
        }
    }

    pub(super) fn run(
        presentation: Presentation,
        heartbeat: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if presentation != Presentation::Utility {
            // Shell surfaces start passive, including their first native
            // show. Interactive windows request foreground explicitly only
            // after their validated role is attached; never activate a hint.
            slint::BackendSelector::new()
                .with_winit_window_attributes_hook(|attributes| attributes.with_active(false))
                .select()?;
        }
        let (settings, preferences, notice) =
            super::load_preferences(SettingsStore::for_current_user());
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
        let heartbeat = heartbeat
            .map(tessera_windows::ShellHeartbeat::connect)
            .transpose()?
            .map(|event| {
                // The independent supervisor handles a failed/absent heartbeat by restoring Explorer.
                Arc::new(move || {
                    let _ = event.pulse();
                }) as Arc<dyn Fn() + Send + Sync>
            });
        let host = AppHost {
            targets: Mutex::new(HashMap::new()),
            catalog: Mutex::new(None),
            settings,
            presentation,
            window_icons: Mutex::new(HashMap::new()),
            audio: LazyLock::new(AudioProvider::default),
        };
        tessera_ui::run(
            host,
            PanelPreferences::new(theme, preferences.compact())
                .with_dock(edge, preferences.pinned_apps().to_vec())
                .with_launcher_favorites(preferences.launcher_favorites().to_vec())?,
            notice,
            RunOptions {
                surface: if presentation == Presentation::Utility {
                    SurfaceMode::Panel
                } else {
                    SurfaceMode::Dock
                },
                heartbeat,
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

    #[test]
    fn invalid_or_future_startup_records_disable_all_normal_preference_saves() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        for original in [
            "damaged",
            r#"{"schema_version":9,"theme":"dark","compact":true}"#,
            r#"{"schema_version":2,"theme":"dark","compact":true}"#,
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
}
