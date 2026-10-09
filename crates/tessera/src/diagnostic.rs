// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Installer-only GUI startup fixture, not a source of desktop health or identity.
//! No settings, native observations, providers, surface leases, or watchers are acquired.

#[cfg(windows)]
use std::sync::Arc;

use tessera_ui::{
    DesktopHost, DockContext, PanelPreferences, PanelSnapshot, SystemAction, WindowAction,
};
#[cfg(windows)]
use tessera_ui::{RunOptions, SurfaceMode};

const NOTICE: &str =
    "Internal GUI startup diagnostic: synthetic geometry; desktop data and actions unavailable.";
const ACTION_UNAVAILABLE: &str = "Desktop actions are unavailable in the internal GUI diagnostic";

struct DiagnosticHost {
    snapshot: PanelSnapshot,
}

impl DiagnosticHost {
    fn new() -> Result<Self, String> {
        // This deliberately labelled fixture supplies only the viewport needed by
        // the production bars' readiness path, never fabricated native metadata.
        let bounds = tessera_core::Rect::new(0, 0, 1000, 800).map_err(|error| error.to_string())?;
        let context = DockContext::new(
            bounds.x(),
            bounds.y(),
            bounds.width(),
            bounds.height(),
            false,
        )
        .ok_or("Invalid internal GUI diagnostic geometry")?;
        Ok(Self {
            snapshot: PanelSnapshot::new(1, Vec::new(), 0)
                .with_applications(Vec::new())
                .with_dock_context(context),
        })
    }
}

impl DesktopHost for DiagnosticHost {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        Ok(self.snapshot.clone())
    }

    fn activate(&self, _key: &str) -> Result<(), String> {
        Err(ACTION_UNAVAILABLE.into())
    }

    fn window_action(&self, _key: &str, _action: WindowAction) -> Result<(), String> {
        Err(ACTION_UNAVAILABLE.into())
    }

    fn launch(&self, _key: &str) -> Result<(), String> {
        Err(ACTION_UNAVAILABLE.into())
    }

    fn system_action(&self, _action: SystemAction) -> Result<(), String> {
        Err(ACTION_UNAVAILABLE.into())
    }

    fn save_preferences(&self, _preferences: &PanelPreferences) -> Result<(), String> {
        Err(ACTION_UNAVAILABLE.into())
    }

    // Current DesktopHost defaults return no capabilities/subscriptions/leases,
    // empty identity and clock, disabled motion, and no native focus request.
}

#[cfg(windows)]
pub(crate) fn run(
    heartbeat: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Result<(), Box<dyn std::error::Error>> {
    tessera_ui::run(
        DiagnosticHost::new()?,
        PanelPreferences::default(),
        Some(NOTICE.into()),
        RunOptions {
            surface: SurfaceMode::Dock,
            heartbeat,
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    #[test]
    fn observation_is_only_the_fixed_explicit_internal_fixture() {
        let host = DiagnosticHost::new().unwrap();
        let expected = PanelSnapshot::new(1, Vec::new(), 0)
            .with_applications(Vec::new())
            .with_dock_context(DockContext::new(0, 0, 1000, 800, false).unwrap());
        assert_eq!(host.observe().unwrap(), expected);
        assert_eq!(host.observe().unwrap(), expected);
        assert_eq!(
            host.shell_identity().unwrap(),
            tessera_ui::ShellIdentity::default()
        );
        assert!(host.clock_text().unwrap().is_empty());
        assert!(!host.ui_animations_enabled());
        assert!(NOTICE.contains("synthetic geometry"));
    }

    #[test]
    fn capabilities_and_subscriptions_remain_unavailable_without_callbacks() {
        let host = DiagnosticHost::new().unwrap();
        assert!(host.audio_host().unwrap().is_none());
        assert!(host.folder_host().unwrap().is_none());
        assert!(host.calendar_host().unwrap().is_none());
        assert!(host.display_context_host().unwrap().is_none());
        assert!(host.power_host().unwrap().is_none());
        assert!(host.dock_utilities_host().unwrap().is_none());
        assert!(host.recycle_bin_host().unwrap().is_none());
        assert!(host.recycle_bin_mutation_host().unwrap().is_none());

        let calls = Arc::new(AtomicUsize::new(0));
        let desktop_calls = Arc::clone(&calls);
        assert!(
            host.subscribe(Arc::new(move || {
                desktop_calls.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap()
            .is_none()
        );
        let motion_calls = Arc::clone(&calls);
        assert!(
            host.subscribe_ui_motion(Arc::new(move |_| {
                motion_calls.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap()
            .is_none()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(Arc::strong_count(&calls), 1);
    }

    #[test]
    fn input_cannot_activate_launch_save_or_dispatch_desktop_effects() {
        let host = DiagnosticHost::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let original = b"existing user settings must not be read or changed";
        std::fs::write(&path, original).unwrap();
        let preferences = PanelPreferences::default();
        assert_eq!(preferences.theme(), tessera_ui::Theme::System);
        assert!(!preferences.compact());
        assert!(preferences.pinned_apps().is_empty());
        assert!(preferences.launcher().favorites().is_empty());
        assert_eq!(
            host.save_preferences(&preferences).unwrap_err(),
            ACTION_UNAVAILABLE
        );
        let edited = preferences
            .with_appearance(tessera_ui::Theme::Dark, true, tessera_ui::DockEdge::Left)
            .with_dock(tessera_ui::DockEdge::Left, vec!["stale-pin".into()])
            .with_launcher_display_mode(tessera_ui::LauncherDisplayMode::Fullscreen)
            .with_launcher_favorites(vec!["stale-favorite".into()])
            .unwrap();
        assert_eq!(
            host.save_preferences(&edited).unwrap_err(),
            ACTION_UNAVAILABLE
        );
        for key in ["", "stale-window", "internal-fixture"] {
            assert_eq!(host.activate(key).unwrap_err(), ACTION_UNAVAILABLE);
            assert_eq!(host.launch(key).unwrap_err(), ACTION_UNAVAILABLE);
            for action in [
                WindowAction::Activate,
                WindowAction::ActivateOrMinimize,
                WindowAction::Minimize,
                WindowAction::Close,
            ] {
                assert_eq!(
                    host.window_action(key, action).unwrap_err(),
                    ACTION_UNAVAILABLE
                );
            }
        }
        for action in [
            SystemAction::OpenFileManager,
            SystemAction::OpenTaskManager,
            SystemAction::RestoreExplorer,
        ] {
            assert_eq!(host.system_action(action).unwrap_err(), ACTION_UNAVAILABLE);
        }
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
