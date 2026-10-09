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

#[derive(Clone, Copy, Debug)]
pub(crate) enum StartupPhase {
    HeartbeatEventConnect,
    DiagnosticFixture,
    ProductionUiConstructionAndEventLoop,
}

impl StartupPhase {
    fn label(self) -> &'static str {
        match self {
            Self::HeartbeatEventConnect => "heartbeatEventConnect",
            Self::DiagnosticFixture => "diagnosticFixture",
            Self::ProductionUiConstructionAndEventLoop => "productionUiConstructionAndEventLoop",
        }
    }
}

#[derive(Debug)]
struct StartupError {
    phase: StartupPhase,
    cause: Box<dyn std::error::Error>,
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "diagnostic phase={}: {}",
            self.phase.label(),
            self.cause
        )
    }
}

impl std::error::Error for StartupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

/// Context only for the inert diagnostic; ordinary session errors stay untouched.
/// The operation runs exactly once, without retries, queries, or panic interception.
pub(crate) fn startup_phase<T>(
    diagnostic: bool,
    phase: StartupPhase,
    operation: impl FnOnce() -> Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    operation().map_err(|cause| {
        if diagnostic {
            Box::new(StartupError { phase, cause }) as Box<dyn std::error::Error>
        } else {
            cause
        }
    })
}

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
    let host = startup_phase(true, StartupPhase::DiagnosticFixture, || {
        DiagnosticHost::new().map_err(Into::into)
    })?;
    // The public production runner owns both component construction and the loop;
    // do not claim a more specific internal stage than this boundary can observe.
    startup_phase(
        true,
        StartupPhase::ProductionUiConstructionAndEventLoop,
        || {
            tessera_ui::run(
                host,
                PanelPreferences::default(),
                Some(NOTICE.into()),
                RunOptions {
                    surface: SurfaceMode::Dock,
                    heartbeat,
                },
            )
            .map_err(Into::into)
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
    fn diagnostic_phase_preserves_each_original_typed_cause() {
        for (phase, label) in [
            (StartupPhase::HeartbeatEventConnect, "heartbeatEventConnect"),
            (StartupPhase::DiagnosticFixture, "diagnosticFixture"),
            (
                StartupPhase::ProductionUiConstructionAndEventLoop,
                "productionUiConstructionAndEventLoop",
            ),
        ] {
            let calls = AtomicUsize::new(0);
            let error = startup_phase::<()>(true, phase, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(
                    std::io::Error::new(std::io::ErrorKind::PermissionDenied, "original cause")
                        .into(),
                )
            })
            .unwrap_err();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                error.to_string(),
                format!("diagnostic phase={label}: original cause")
            );
            let cause = error
                .source()
                .unwrap()
                .downcast_ref::<std::io::Error>()
                .unwrap();
            assert_eq!(cause.kind(), std::io::ErrorKind::PermissionDenied);
            assert_eq!(cause.to_string(), "original cause");
        }
    }

    #[test]
    fn ordinary_session_error_is_not_wrapped_or_replaced() {
        let calls = AtomicUsize::new(0);
        let cause: Box<dyn std::error::Error> =
            std::io::Error::new(std::io::ErrorKind::NotFound, "ordinary cause").into();
        let original = cause.as_ref() as *const dyn std::error::Error;
        let error = startup_phase::<()>(false, StartupPhase::HeartbeatEventConnect, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(cause)
        })
        .unwrap_err();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(std::ptr::eq(original, error.as_ref()));
        assert_eq!(error.to_string(), "ordinary cause");
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn phase_success_runs_only_the_given_operation_once() {
        for diagnostic in [false, true] {
            let calls = AtomicUsize::new(0);
            let result = startup_phase(diagnostic, StartupPhase::DiagnosticFixture, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            })
            .unwrap();
            assert_eq!(result, 42);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn observation_is_only_the_fixed_explicit_internal_fixture() {
        let host = DiagnosticHost::new().unwrap();
        let expected = PanelSnapshot::new(1, Vec::new(), 0)
            .with_applications(Vec::new())
            .with_dock_context(DockContext::new(0, 0, 1000, 800, false).unwrap());
        assert_eq!(host.observe().unwrap(), expected);
        assert_eq!(host.observe().unwrap(), expected);
        assert!(host.observe().unwrap().visibility_facts().is_none());
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
        assert!(host.power_updates_host().unwrap().is_none());
        assert!(host.network_host().unwrap().is_none());
        assert!(host.bluetooth_host().unwrap().is_none());
        assert!(host.input_language_host().unwrap().is_none());
        assert!(host.media_host().unwrap().is_none());
        assert!(host.pointer_host().unwrap().is_none());
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
        let preferences = PanelPreferences::default();
        assert_eq!(preferences.theme(), tessera_ui::Theme::System);
        assert!(!preferences.compact());
        assert!(preferences.pinned_apps().is_empty());
        assert!(preferences.launcher().favorites().is_empty());
        assert_eq!(preferences.dock_edge(), tessera_ui::DockEdge::Bottom);
        assert!(!preferences.media_enabled());
        assert_eq!(
            preferences.general().start_of_week(),
            tessera_ui::StartOfWeek::Monday
        );
        assert_eq!(
            host.save_preferences(&preferences).unwrap_err(),
            ACTION_UNAVAILABLE
        );
        let edited = preferences
            .with_appearance(tessera_ui::Theme::Dark, true, tessera_ui::DockEdge::Left)
            .with_dock(tessera_ui::DockEdge::Left, vec!["stale-pin".into()])
            .with_launcher_display_mode(tessera_ui::LauncherDisplayMode::Fullscreen)
            .with_general(
                tessera_ui::GeneralPreferences::default()
                    .with_start_of_week(tessera_ui::StartOfWeek::Sunday),
            )
            .with_media_enabled(true)
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
    }
}
