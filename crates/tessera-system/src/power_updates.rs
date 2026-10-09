// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Read-only existence hint for two known pending-update registry keys.

use std::fmt;

/// An observation of the configured keys, not exhaustive Windows Update state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerUpdateHint {
    /// At least one known key opened successfully and its handle was retired.
    Pending,
    /// Both known keys were absent. Other update/reboot requirements may exist.
    NotDetected,
}

/// Fixed safe diagnostics; no provider text or registry contents are exposed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerUpdatesError {
    Unsupported,
    Busy,
    Unavailable,
    AccessDenied,
    Native { code: u32 },
}

impl fmt::Display for PowerUpdatesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => {
                formatter.write_str("Pending-update hints are not supported on this platform")
            }
            Self::Busy => formatter.write_str("A pending-update hint read is already in progress"),
            Self::Unavailable => formatter.write_str("The pending-update hint is unavailable"),
            Self::AccessDenied => formatter.write_str("The pending-update hint read was denied"),
            Self::Native { code } => {
                write!(
                    formatter,
                    "The pending-update hint read failed (native code 0x{code:08x})"
                )
            }
        }
    }
}

impl std::error::Error for PowerUpdatesError {}

pub type PowerUpdatesCompletion =
    Box<dyn FnOnce(Result<PowerUpdateHint, PowerUpdatesError>) + Send + 'static>;

/// Prompt admission of one independent read, with no accepted queue or replay.
///
/// `Ok` accepts exactly one completion, possibly inline/reentrant; immediate
/// `Err` accepts zero callbacks. The read owner/resources retire (or cleanup is
/// attempted), then admission releases, before the consumer is called. The
/// consumer may submit another read. This gate is independent of power actions.
///
/// Accepted reads outlive host/UI drops. There is no GUI join, cancellation,
/// deadline, or completion-across-process-exit guarantee. Native cleanup can
/// fail or stall. Panic containment requires unwinding and cannot recover from
/// aborts, double panics during cleanup, or process termination. A cleanup error
/// is not converted into a successful hint and can leave a handle until exit.
pub trait PowerUpdatesHost: Send + Sync + 'static {
    fn read(&self, completion: PowerUpdatesCompletion) -> Result<(), PowerUpdatesError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_update_hint_and_errors_are_portable_copy_values() {
        fn portable<T: Copy + Eq + Send + Sync + 'static>() {}
        portable::<PowerUpdateHint>();
        portable::<PowerUpdatesError>();
        assert_ne!(PowerUpdateHint::Pending, PowerUpdateHint::NotDetected);
    }

    #[test]
    fn power_updates_diagnostics_are_fixed_and_preserve_full_native_codes() {
        for (error, expected) in [
            (
                PowerUpdatesError::Unsupported,
                "Pending-update hints are not supported on this platform",
            ),
            (
                PowerUpdatesError::Busy,
                "A pending-update hint read is already in progress",
            ),
            (
                PowerUpdatesError::Unavailable,
                "The pending-update hint is unavailable",
            ),
            (
                PowerUpdatesError::AccessDenied,
                "The pending-update hint read was denied",
            ),
            (
                PowerUpdatesError::Native { code: 0 },
                "The pending-update hint read failed (native code 0x00000000)",
            ),
            (
                PowerUpdatesError::Native { code: u32::MAX },
                "The pending-update hint read failed (native code 0xffffffff)",
            ),
        ] {
            assert_eq!(error.to_string(), expected);
        }
    }
}
