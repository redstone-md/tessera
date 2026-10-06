// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::error::Error;

#[cfg(windows)]
mod desktop {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;
    use tessera_ui::{
        DesktopHost, DockContext, PanelApplication, PanelPreferences, PanelSnapshot, PanelWindow,
        PixelIcon, RunOptions, SurfaceMode, SystemAction,
    };
    use tessera_windows::{ActivationTarget, Application, IconPixels};

    use crate::settings::{DockEdge, Preferences, SettingsStore, Theme};

    struct CatalogCache {
        checked: Instant,
        entries: Vec<Application>,
        presentation: Vec<PanelApplication>,
        failed: bool,
    }

    struct AppHost {
        targets: Mutex<HashMap<String, ActivationTarget>>,
        catalog: Mutex<Option<CatalogCache>>,
        settings: Option<SettingsStore>,
    }

    fn icon(pixels: &IconPixels) -> Option<PixelIcon> {
        PixelIcon::new(pixels.width(), pixels.height(), pixels.rgba().to_vec())
    }

    impl AppHost {
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
    }

    impl DesktopHost for AppHost {
        fn observe(&self) -> Result<PanelSnapshot, String> {
            let snapshot = tessera_windows::observe().map_err(|error| error.to_string())?;
            let mut targets = HashMap::new();
            let mut windows = Vec::new();
            for window in snapshot.windows() {
                if let Some(target) = ActivationTarget::from_window(window) {
                    // Snapshot-local identity is never a persisted application pin.
                    let key = format!("{}:{}", target.window_id().value(), target.process_id());
                    targets.insert(key.clone(), target);
                    let row = PanelWindow::new(key, window.title().to_owned(), window.minimized());
                    windows.push(
                        row.with_icon(tessera_windows::window_icon(target).as_ref().and_then(icon)),
                    );
                }
            }
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
                let area = primary.work_area();
                if let Some(context) =
                    DockContext::new(area.x(), area.y(), area.width(), area.height(), fullscreen)
                {
                    view = view.with_dock_context(context);
                }
            }
            Ok(view)
        }

        fn activate(&self, key: &str) -> Result<(), String> {
            let target = self.targets.lock().get(key).copied().ok_or_else(|| {
                "This window is no longer in the observation. Refresh and retry".to_owned()
            })?;
            tessera_windows::activate(target).map_err(|error| error.to_string())
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
            match action {
                SystemAction::OpenFileManager => tessera_windows::open_file_manager(),
                SystemAction::OpenTaskManager => tessera_windows::open_task_manager(),
                SystemAction::RestoreExplorer => tessera_windows::restore_explorer(),
            }
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
                "User settings location is unavailable; preview still works".to_owned()
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
                        .with_dock(edge, preferences.pinned_apps().to_vec()),
                )
                .map_err(|error| error.to_string())
        }
    }

    pub(super) fn run(
        surface: SurfaceMode,
        heartbeat: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (settings, preferences, notice) = match SettingsStore::for_current_user() {
            Ok(store) => {
                let (preferences, notice) = match store.load() {
                    Ok(preferences) => (preferences, None),
                    Err(error) => (
                        Preferences::default(),
                        Some(format!(
                            "Preferences could not be loaded: {error}. Defaults are active; Save replaces the file."
                        )),
                    ),
                };
                (Some(store), preferences, notice)
            }
            Err(error) => (
                None,
                Preferences::default(),
                Some(format!(
                    "Preferences cannot be saved: {error}. Appearance preview is available."
                )),
            ),
        };
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
        };
        tessera_ui::run(
            host,
            PanelPreferences::new(theme, preferences.compact())
                .with_dock(edge, preferences.pinned_apps().to_vec()),
            notice,
            RunOptions { surface, heartbeat },
        )?;
        Ok(())
    }
}

#[cfg(windows)]
pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    desktop::run(tessera_ui::SurfaceMode::Panel, None)
}

#[cfg(windows)]
pub(crate) fn run_desktop(heartbeat: Option<&str>) -> Result<(), Box<dyn Error>> {
    desktop::run(tessera_ui::SurfaceMode::Dock, heartbeat)
}

#[cfg(not(windows))]
pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    Err(tessera_windows::ObservationError::UnsupportedPlatform.into())
}

#[cfg(not(windows))]
pub(crate) fn run_desktop(_heartbeat: Option<&str>) -> Result<(), Box<dyn Error>> {
    run()
}
