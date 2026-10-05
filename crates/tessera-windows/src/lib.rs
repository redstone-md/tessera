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
//! input thread. See the `activation` module docs for the exact contract.

#![deny(unsafe_op_in_unsafe_fn)]

mod activation;
mod error;
#[cfg(any(windows, test))]
mod helpers;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native_activation;
mod snapshot;

pub use activation::{ActivationError, ActivationTarget, activate, show_startup_error};

pub use error::{ObservationError, ObservationWarning};
pub use snapshot::{DesktopSnapshot, MonitorId, ObservedMonitor, ObservedWindow};

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
