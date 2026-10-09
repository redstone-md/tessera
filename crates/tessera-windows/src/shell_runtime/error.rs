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
    /// Diagnostic preflight failure, retaining the exact phase, child status,
    /// cleanup error and at most 16 KiB of the owned GUI's stderr tail.
    DiagnosticFailure {
        stage: &'static str,
        pulses: u8,
        exit_code: Option<u32>,
        source: Box<ShellRuntimeError>,
        cleanup_error: Option<Box<ShellRuntimeError>>,
        stderr: Vec<u8>,
        stderr_truncated: bool,
        stderr_status: &'static str,
        stderr_native_code: Option<u32>,
    },
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
            Self::DiagnosticFailure {
                stage,
                pulses,
                exit_code,
                source,
                cleanup_error,
                stderr,
                stderr_truncated,
                stderr_status,
                stderr_native_code,
            } => {
                let stderr_hint = if stderr.is_empty() {
                    "empty"
                } else if stderr
                    .windows(b"panicked at".len())
                    .any(|part| part == b"panicked at")
                {
                    "panic"
                } else {
                    "other"
                };
                writeln!(
                    f,
                    "tessera-runtime stage={stage} error={} pulses={pulses} exit_code={} native_code={} cleanup_native_code={} stderr_bytes={} stderr_truncated={stderr_truncated} stderr_status={stderr_status} stderr_hint={stderr_hint} stderr_native_code={}",
                    source.category(),
                    NativeCode(*exit_code),
                    NativeCode(source.native_code()),
                    NativeCode(cleanup_error.as_deref().and_then(Self::native_code)),
                    stderr.len(),
                    NativeCode(*stderr_native_code),
                )?;
                write!(f, "runtime cause: {source}")?;
                if let Some(error) = cleanup_error {
                    write!(f, "; owned cleanup also failed: {error}")?;
                }
                if !stderr.is_empty() {
                    // Debug escaping keeps GUI text off the machine-record line,
                    // including embedded newlines/control characters.
                    write!(
                        f,
                        "\nGUI stderr tail: {:?}",
                        String::from_utf8_lossy(stderr)
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl ShellRuntimeError {
    fn native_code(&self) -> Option<u32> {
        match self {
            Self::Windows { code, .. } | Self::SpawnFailed { code } => Some(*code),
            _ => None,
        }
    }

    fn category(&self) -> &'static str {
        match self {
            Self::Windows { .. } => "windows",
            Self::SpawnFailed { .. } => "spawn",
            Self::HeartbeatViolation { .. } => "heartbeat",
            Self::UnusablePath { .. } => "path",
            _ => "other",
        }
    }
}

struct NativeCode(Option<u32>);

impl fmt::Display for NativeCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(code) => write!(f, "0x{code:08X}"),
            None => write!(f, "none"),
        }
    }
}

impl std::error::Error for ShellRuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DiagnosticFailure { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

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
