// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit native power requests, separate from desktop observation and Exit.

use std::fmt;

/// Explicit update-installation intent, captured before admitting a request.
///
/// Omitting the explicit flag is not a guarantee that Windows installs nothing.
/// Requesting installation is not proof that updates exist or later complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerUpdatePolicy {
    OmitExplicitInstallation,
    RequestInstallation,
}

/// Closed native requests; callers cannot supply force, reason or target flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerAction {
    LockSession,
    LogOut,
    PowerOff { updates: PowerUpdatePolicy },
    Reboot { updates: PowerUpdatePolicy },
    Suspend,
    Hibernate,
}

/// Native initiation receipt, not proof of eventual OS state or update completion.
///
/// Lock, logoff and shutdown can be asynchronous. Suspend/hibernate may block
/// until resume. This marker makes no claim about observed session/power state,
/// installed updates, or the lifetime of the UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerRequestAccepted;

/// Safe fixed diagnostics; native codes are preserved without provider text.
///
/// Any completion error can follow native initiation (for example, checked
/// cleanup failed). It does not prove the request was uninitiated: never replay.
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
            Self::AccessDenied => {
                formatter.write_str("The power request or its cleanup was denied")
            }
            Self::Native { code } => {
                write!(
                    formatter,
                    "The power request or its cleanup failed (native code 0x{code:08x})"
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
/// `Err` accepts zero callbacks. Native owners retire before admission releases
/// and the consumer runs. The completion may submit another request. Success
/// reports only initiation, never observed OS state or completed installation.
///
/// Accepted work outlives host/UI drops. There is no GUI-thread join, deadline,
/// native cancellation, queue, retry, or completion-across-process-exit promise.
/// Suspend/hibernate and native cleanup can stall. The Windows privileged child
/// must actually terminate before consumer delivery, including if reversion
/// fails. An unexpected failure to prove child retirement quarantines its
/// background owner: admission remains busy and no completion is delivered.
///
/// Panic containment requires unwinding; aborts, TLS/DLL teardown failures,
/// double panics, global panic hooks and process termination are not recoverable
/// guarantees. A failed token close may leak its handle until process exit.
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
        copy_eq::<PowerUpdatePolicy>();
        copy_eq::<PowerRequestAccepted>();
        copy_eq::<PowerError>();
        send::<PowerCompletion>();
        send_sync::<std::sync::Arc<dyn PowerHost>>();
        error::<PowerError>();
        assert_eq!(PowerAction::LockSession, PowerAction::LockSession);
        assert_eq!(std::mem::size_of::<PowerRequestAccepted>(), 0);
        let actions = [
            PowerAction::LockSession,
            PowerAction::LogOut,
            PowerAction::PowerOff {
                updates: PowerUpdatePolicy::OmitExplicitInstallation,
            },
            PowerAction::Reboot {
                updates: PowerUpdatePolicy::RequestInstallation,
            },
            PowerAction::Suspend,
            PowerAction::Hibernate,
        ];
        assert_eq!(actions.len(), 6);
        assert_ne!(
            PowerAction::PowerOff {
                updates: PowerUpdatePolicy::OmitExplicitInstallation,
            },
            PowerAction::PowerOff {
                updates: PowerUpdatePolicy::RequestInstallation,
            },
        );
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
            (
                PowerError::AccessDenied,
                "The power request or its cleanup was denied",
            ),
            (
                PowerError::Native { code: 0 },
                "The power request or its cleanup failed (native code 0x00000000)",
            ),
            (
                PowerError::Native { code: u32::MAX },
                "The power request or its cleanup failed (native code 0xffffffff)",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
            assert!(error.source().is_none());
        }
    }
}
