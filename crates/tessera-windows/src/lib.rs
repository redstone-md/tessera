// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Read-only desktop observation for Windows.
//!
//! `observe()` enumerates physical monitors and visible top-level desktop
//! windows without moving, resizing, focusing, or otherwise mutating any
//! window. It performs no process-global DPI mutation; the per-monitor-aware
//! V2 context is scoped to the observing thread for the duration of the call.

#![deny(unsafe_op_in_unsafe_fn)]

mod error;
#[cfg(any(windows, test))]
mod helpers;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native;
mod snapshot;

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
