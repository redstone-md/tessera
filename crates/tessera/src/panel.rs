// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::error::Error;

#[cfg(windows)]
mod desktop {
    use std::collections::HashMap;

    use parking_lot::Mutex;
    use tessera_ui::{DesktopHost, PanelPreferences, PanelSnapshot, PanelWindow};
    use tessera_windows::ActivationTarget;

    use crate::settings::{Preferences, SettingsStore, Theme};

    struct AppHost {
        targets: Mutex<HashMap<String, ActivationTarget>>,
        settings: Option<SettingsStore>,
    }

    impl DesktopHost for AppHost {
        fn observe(&self) -> Result<PanelSnapshot, String> {
            let snapshot = tessera_windows::observe().map_err(|error| error.to_string())?;
            let mut targets = HashMap::new();
            let mut windows = Vec::new();
            for window in snapshot.windows() {
                if let Some(target) = ActivationTarget::from_window(window) {
                    // An opaque snapshot-local key, never a persisted application identity.
                    let key = format!("{}:{}", target.window_id().value(), target.process_id());
                    targets.insert(key.clone(), target);
                    windows.push(PanelWindow::new(
                        key,
                        window.title().to_owned(),
                        window.minimized(),
                    ));
                }
            }
            *self.targets.lock() = targets;
            Ok(PanelSnapshot::new(
                snapshot.monitors().len(),
                windows,
                snapshot.warnings().len(),
            ))
        }

        fn activate(&self, key: &str) -> Result<(), String> {
            let target = self.targets.lock().get(key).copied().ok_or_else(|| {
                "This window is no longer in the observation. Refresh and retry".to_owned()
            })?;
            // No registry lock is held during Win32 work. Native validation happens at the click.
            tessera_windows::activate(target).map_err(|error| error.to_string())
        }

        fn save_preferences(&self, preferences: &PanelPreferences) -> Result<(), String> {
            let store = self.settings.as_ref().ok_or_else(|| {
                "User settings location is unavailable; appearance preview still works".to_owned()
            })?;
            let theme = match preferences.theme() {
                tessera_ui::Theme::System => Theme::System,
                tessera_ui::Theme::Light => Theme::Light,
                tessera_ui::Theme::Dark => Theme::Dark,
            };
            store
                .save(&Preferences::new(theme, preferences.compact()))
                .map_err(|error| error.to_string())
        }
    }

    pub(super) fn run() -> Result<(), Box<dyn std::error::Error>> {
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
        let host = AppHost {
            targets: Mutex::new(HashMap::new()),
            settings,
        };
        tessera_ui::run(
            host,
            PanelPreferences::new(theme, preferences.compact()),
            notice,
        )?;
        Ok(())
    }
}

/// Platform facts and commands become a portable presentation host.
#[cfg(windows)]
pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    desktop::run()
}

#[cfg(not(windows))]
pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    Err(tessera_windows::ObservationError::UnsupportedPlatform.into())
}
