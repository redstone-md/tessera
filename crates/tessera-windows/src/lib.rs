// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Read-only desktop observation for Windows.
//!
//! `observe()` enumerates physical monitors and visible top-level desktop
//! windows without moving, resizing, focusing, or otherwise mutating any
//! window. It performs no process-global DPI mutation; the per-monitor-aware
//! V2 context is scoped to the observing thread for the duration of the call.
//!
//! Activation is deliberately narrow: [`ActivationTarget::from_window`]
//! captures one qualified candidate from an observation pass and [`activate`]
//! brings it to the foreground on an explicit user action from the panel's UI
//! input thread. [`window_action`] applies further explicit user actions
//! (minimize, close request, foreground toggle) behind the same live
//! validation. See the `activation` and `window_actions` module docs for the
//! exact contracts.

#![deny(unsafe_op_in_unsafe_fn)]

mod activation;
mod apps;
mod audio;
pub mod calendar;
mod diagnostics;
pub mod dock_utilities;
mod error;
mod events;
pub mod folders;
#[cfg(any(windows, test))]
mod helpers;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native_activation;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native_apps;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native_audio;
#[cfg(any(windows, test))]
#[cfg_attr(windows, allow(unsafe_code))]
mod native_calendar;
#[cfg(any(windows, test))]
#[cfg_attr(windows, allow(unsafe_code))]
mod native_dock_utilities;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native_events;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native_folders;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native_window_actions;
#[cfg(any(windows, test))]
mod shell_recovery;
mod shell_runtime;
mod snapshot;
mod ui_preferences;
mod window_actions;

pub use activation::{ActivationError, ActivationTarget, activate, show_startup_error};
pub use audio::AudioService;
pub use ui_preferences::{
    UiMotionCallback, UiMotionWatcher, ui_animations_enabled, watch_ui_motion,
};
pub use window_actions::{WindowAction, WindowActionError, window_action};

pub use apps::{
    Application, ApplicationError, IconPixels, catalog, foreground_window_id, launch, window_icon,
};

pub use diagnostics::{DiagnosticError, WindowDiagnostics, diagnose_window};
pub use error::{ObservationError, ObservationWarning};
pub use events::{DesktopEventCallback, DesktopWatcher, watch_desktop};
pub use snapshot::{DesktopSnapshot, MonitorId, ObservedMonitor, ObservedWindow};

#[cfg(windows)]
pub use shell_runtime::ShellHeartbeat;
pub use shell_runtime::{DesktopIdentity, run_desktop_session, start_desktop_session};
pub use shell_runtime::{OwnedShellSurface, ShellSurfaceKind, request_owned_foreground};
pub use shell_runtime::{
    ShellRuntimeError, clock_text, desktop_identity, open_file_manager, open_task_manager,
    restore_explorer, run_shell, verify_runtime,
};

/// Captures one best-effort, read-only observation pass; this is not an atomic
/// snapshot. Invisible and calling-process windows are excluded.
///
/// On non-Windows platforms this always returns
/// [`ObservationError::UnsupportedPlatform`]; a fake empty snapshot is never
/// produced.
pub fn observe() -> Result<DesktopSnapshot, ObservationError> {
    #[cfg(windows)]
    {
        crate::native::observe()
    }
    #[cfg(not(windows))]
    {
        Err(ObservationError::UnsupportedPlatform)
    }
}
