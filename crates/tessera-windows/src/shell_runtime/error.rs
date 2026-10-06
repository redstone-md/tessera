// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shell runtime error type.

use std::fmt;

/// Why a shell-runtime operation failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum ShellRuntimeError {
    /// The current OS is not supported by this module.
    UnsupportedPlatform,
    /// A Win32 operation failed; `operation` names the failed call and
    /// `code` carries the last error or NTSTATUS.
    Windows { operation: &'static str, code: u32 },
    /// The environment is not a real takeover: no active backup, or the
    /// running binary is not the installed supervisor recorded as the
    /// current shell. Manual execution has no registry side effects.
    NotDeployed { reason: String },
    /// A backup was found but refused by the policy layer.
    BackupRejected(String),
    /// The recorded backup authorizes restoration, but another program now
    /// owns the `Shell` value. The backup is retained and nothing changed.
    RestoreRefusedClobber,
    /// Spawning the single Tessera child failed.
    SpawnFailed { code: u32 },
    /// The Tessera child violated the heartbeat protocol.
    HeartbeatViolation { reason: &'static str },
    /// A required path (system directory, Windows directory, supervisor)
    /// could not be resolved or is not absolute.
    UnusablePath { context: &'static str },
    /// A value could not be decoded as bounded UTF-16.
    InvalidUtf16 { context: &'static str },
    /// Another presentation owner already runs for this user/session. The
    /// existing owner is left untouched; nothing was hidden or restored.
    PresentationOwnerBusy,
}

impl fmt::Display for ShellRuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "shell runtime requires Windows"),
            Self::Windows { operation, code } => {
                write!(
                    f,
                    "Windows operation `{operation}` failed (code 0x{code:X})"
                )
            }
            Self::NotDeployed { reason } => {
                write!(f, "tessera-shell is not the active shell: {reason}")
            }
            Self::BackupRejected(reason) => write!(f, "backup rejected: {reason}"),
            Self::RestoreRefusedClobber => write!(
                f,
                "restoration refused: the Shell value is no longer owned by Tessera; backup retained"
            ),
            Self::SpawnFailed { code } => write!(f, "failed to spawn Tessera (code 0x{code:X})"),
            Self::HeartbeatViolation { reason } => {
                write!(f, "heartbeat protocol violation: {reason}")
            }
            Self::UnusablePath { context } => write!(f, "path unusable: {context}"),
            Self::InvalidUtf16 { context } => write!(f, "value at `{context}` is not valid UTF-16"),
            Self::PresentationOwnerBusy => write!(
                f,
                "another presentation owner already runs in this session; nothing was changed"
            ),
        }
    }
}

impl std::error::Error for ShellRuntimeError {}

#[cfg(windows)]
use windows_sys::Win32::Foundation::ERROR_NO_MATCH;

#[cfg(windows)]
impl From<crate::shell_recovery::native::DeploymentError> for ShellRuntimeError {
    fn from(error: crate::shell_recovery::native::DeploymentError) -> Self {
        match error {
            crate::shell_recovery::native::DeploymentError::UnsupportedPlatform => {
                Self::UnsupportedPlatform
            }
            crate::shell_recovery::native::DeploymentError::Windows { operation, code } => {
                Self::Windows { operation, code }
            }
            crate::shell_recovery::native::DeploymentError::Rejected(reason) => {
                Self::BackupRejected(reason.to_string())
            }
            crate::shell_recovery::native::DeploymentError::ShellValueNotString { kind } => {
                Self::Windows {
                    operation: "Shell value type check",
                    code: kind,
                }
            }
            crate::shell_recovery::native::DeploymentError::ShellValueUnreadable => {
                Self::HeartbeatViolation {
                    reason: "Shell value is unreadable",
                }
            }
            crate::shell_recovery::native::DeploymentError::ShellOwnershipChanged => {
                Self::RestoreRefusedClobber
            }
            crate::shell_recovery::native::DeploymentError::LockBusy => Self::Windows {
                operation: "DeploymentLock::acquire",
                code: 0xB7, // ERROR_ALREADY_EXISTS
            },
            crate::shell_recovery::native::DeploymentError::UnusablePath { context } => {
                Self::UnusablePath { context }
            }
            crate::shell_recovery::native::DeploymentError::InvalidUtf16 { context } => {
                Self::InvalidUtf16 { context }
            }
            crate::shell_recovery::native::DeploymentError::RestoreVerificationFailed {
                value: _,
            } => Self::Windows {
                operation: "restore readback verification",
                code: ERROR_NO_MATCH,
            },
        }
    }
}
