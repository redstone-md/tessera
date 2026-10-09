// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit native power requests, separate from desktop observation and Exit.

use std::fmt;

/// Only genuinely implemented native requests are exposed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerAction {
    LockSession,
}

/// Native initiation receipt, not proof that the session is locked.
///
/// `LockWorkStation` returns asynchronously. This marker makes no claim about
/// the eventual OS state, session notifications, or the lifetime of the UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerRequestAccepted;

/// Safe fixed diagnostics; native codes are preserved without provider text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerError {
    Unsupported,
    Busy,
    Unavailable,
    AccessDenied,
    Native { code: u32 },
}

impl fmt::Display for PowerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => {
                formatter.write_str("Power requests are not supported on this platform")
            }
            Self::Busy => formatter.write_str("A power request is already in progress"),
            Self::Unavailable => formatter.write_str("The power request is unavailable"),
            Self::AccessDenied => formatter.write_str("The power request was denied"),
            Self::Native { code } => {
                write!(
                    formatter,
                    "The power request failed (native code 0x{code:08x})"
                )
            }
        }
    }
}

impl std::error::Error for PowerError {}

pub type PowerCompletion =
    Box<dyn FnOnce(Result<PowerRequestAccepted, PowerError>) + Send + 'static>;

/// Prompt admission of one explicit request, with no accepted queue or replay.
///
/// `Ok` accepts exactly one completion, possibly inline/reentrant; immediate
/// `Err` accepts zero callbacks. Native resources and drivers are retired (or
/// cleanup attempted), and admission is released, before calling the consumer.
/// The completion may submit another request. A successful completion reports
/// only native initiation, never an observed locked state.
///
/// Accepted work outlives host/UI drops. There is no GUI-thread join, deadline,
/// native cancellation, or completion-across-process-exit guarantee. Native
/// cleanup can fail or stall. Panic containment requires unwinding and cannot
/// recover from aborts, double panics during cleanup, or process termination.
pub trait PowerHost: Send + Sync + 'static {
    fn perform(&self, action: PowerAction, completion: PowerCompletion) -> Result<(), PowerError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn power_interface_is_typed_thread_safe_and_receipt_is_only_initiation() {
        fn copy_eq<T: Copy + Eq>() {}
        fn send<T: Send>() {}
        fn send_sync<T: Send + Sync>() {}
        fn error<T: std::error::Error>() {}
        copy_eq::<PowerAction>();
        copy_eq::<PowerRequestAccepted>();
        copy_eq::<PowerError>();
        send::<PowerCompletion>();
        send_sync::<std::sync::Arc<dyn PowerHost>>();
        error::<PowerError>();
        assert_eq!(PowerAction::LockSession, PowerAction::LockSession);
        assert_eq!(std::mem::size_of::<PowerRequestAccepted>(), 0);
        assert_eq!(format!("{PowerRequestAccepted:?}"), "PowerRequestAccepted");
    }

    #[test]
    fn power_errors_have_fixed_safe_messages_and_preserve_raw_codes() {
        let cases = [
            (
                PowerError::Unsupported,
                "Power requests are not supported on this platform",
            ),
            (PowerError::Busy, "A power request is already in progress"),
            (PowerError::Unavailable, "The power request is unavailable"),
            (PowerError::AccessDenied, "The power request was denied"),
            (
                PowerError::Native { code: 0 },
                "The power request failed (native code 0x00000000)",
            ),
            (
                PowerError::Native { code: u32::MAX },
                "The power request failed (native code 0xffffffff)",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
            assert!(error.source().is_none());
        }
    }
}
