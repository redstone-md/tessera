// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shell runtime: takeover supervision and safe restoration of the user's
//! previous shell.
//!
//! This module is the native side of the shared installer/native contract:
//! the installer writes the backup record and redirects `Shell`; the
//! supervisor binary ([`run_shell`]) verifies that it is the recorded shell,
//! spawns exactly one Tessera child, watches its heartbeat, and restores the
//! original shell before returning to Explorer. [`restore_explorer`] and
//! [`open_file_manager`] are the explicit recovery/escape paths.
//!
//! Error dialogs are never shown here: the module returns contextual errors
//! and the GUI entry point reports them after the Explorer fallback.

pub use error::ShellRuntimeError;
#[cfg(windows)]
pub use heartbeat::ShellHeartbeat;

#[cfg(windows)]
use crate::shell_recovery::{OriginalShellState, ShellRecoveryBackup};

pub(crate) mod error;
#[cfg(windows)]
mod heartbeat;
#[cfg(windows)]
#[allow(unsafe_code)]
mod supervisor;

#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod native;

/// Verifies the deployment conditions for launching the supervisor:
/// an active, valid backup exists, and the current executable is the exact
/// fully qualified command recorded as the current `Shell` value.
///
/// Manual execution of the supervisor with no takeover in place is a no-op
/// refusal; it never touches the registry.
#[cfg(windows)]
fn validate_takeover(current_exe: &std::path::Path) -> Result<(), ShellRuntimeError> {
    let backup =
        crate::shell_recovery::native::read_backup()?.ok_or(ShellRuntimeError::NotDeployed {
            reason: "no active recovery backup record".into(),
        })?;
    let live = crate::shell_recovery::native::read_live_shell()?;
    validate_takeover_record(current_exe, &backup, live.as_ref())
}

#[cfg(windows)]
fn validate_takeover_record(
    current_exe: &std::path::Path,
    backup: &ShellRecoveryBackup,
    live: Option<&OriginalShellState>,
) -> Result<(), ShellRuntimeError> {
    let path = current_exe.to_str().ok_or(ShellRuntimeError::NotDeployed {
        reason: "the supervisor path is not valid Unicode".into(),
    })?;
    if backup.shell_command() != format!("\"{path}\"")
        || Some(std::path::Path::new(backup.install_directory())) != current_exe.parent()
    {
        return Err(ShellRuntimeError::NotDeployed {
            reason: "this supervisor does not own the recorded installation".into(),
        });
    }
    if !matches!(live, Some(OriginalShellState::RawString(value)) if value == backup.shell_command())
    {
        return Err(ShellRuntimeError::NotDeployed {
            reason: "the live Shell is not this exact REG_SZ supervisor command".into(),
        });
    }
    Ok(())
}

/// Verifies that the production UI can actually start under the current
/// unsigned-launch policy: resolves the sibling Tessera.exe, creates the
/// heartbeat event, spawns the real GUI with `--shell-heartbeat`, waits for
/// the first heartbeat (UI event loop ready) and then a second fresh
/// heartbeat within the 30-second window (timer retention proof), and
/// terminates and reaps only the diagnostic child. No backup record or
/// Winlogon value is read or written and no Explorer is started.
///
/// This is the preflight the source installer invokes with
/// `tessera-shell.exe --verify-runtime` before writing any backup record; a
/// blocked or heartbeat-less run returns an error and the installer must
/// refuse the takeover. Passing here proves the process can start and pulse,
/// but does not guarantee future logins: an unsigned supervisor may still be
/// blocked later by policy changes.
///
/// Success is `Ok(())` only when both pulses arrived and cleanup succeeded.
pub fn verify_runtime() -> Result<(), ShellRuntimeError> {
    #[cfg(windows)]
    {
        crate::shell_runtime::native::verify_runtime_impl()
    }
    #[cfg(not(windows))]
    {
        Err(ShellRuntimeError::UnsupportedPlatform)
    }
}

/// Launches the installed supervisor as the active shell: verifies the
/// takeover, spawns exactly one Tessera child with the heartbeat argument,
/// waits for a normal exit or a heartbeat loss, restores the user's original
/// shell, and finally starts Explorer for this session.
///
/// The caller reports failures after attempting Explorer fallback. An unsigned
/// or externally blocked supervisor cannot repair a session it never starts;
/// independent PowerShell/emergency recovery must remain available.
pub fn run_shell() -> Result<(), ShellRuntimeError> {
    #[cfg(windows)]
    {
        crate::shell_runtime::native::run_shell_impl()
    }
    #[cfg(not(windows))]
    {
        Err(ShellRuntimeError::UnsupportedPlatform)
    }
}

/// Owned persisted restore: reads the active backup, verifies it against the
/// live `Shell` value, restores the recorded original state, and clears the
/// backup's `Active` flag. The clear happens only after the restore verified;
/// a refused restore keeps the backup intact and returns
/// [`ShellRuntimeError::RestoreRefusedClobber`].
///
/// Regardless of the restore outcome, Explorer is then started from the
/// absolute Windows directory so the current session stays usable; the
/// restore error is returned to the caller on top of that.
pub fn restore_explorer() -> Result<(), ShellRuntimeError> {
    #[cfg(windows)]
    {
        crate::shell_runtime::native::restore_explorer_impl()
    }
    #[cfg(not(windows))]
    {
        Err(ShellRuntimeError::UnsupportedPlatform)
    }
}

/// Opens the file manager: launches Explorer with an explicit user folder so
/// a file-management window appears. The Explorer shell may reappear safely
/// as a side effect; this function makes no effort to prevent that.
pub fn open_file_manager() -> Result<(), ShellRuntimeError> {
    #[cfg(windows)]
    {
        crate::shell_runtime::native::open_file_manager_impl()
    }
    #[cfg(not(windows))]
    {
        Err(ShellRuntimeError::UnsupportedPlatform)
    }
}

/// Opens Task Manager from the absolute system directory.
pub fn open_task_manager() -> Result<(), ShellRuntimeError> {
    #[cfg(windows)]
    {
        crate::shell_runtime::native::open_task_manager_impl()
    }
    #[cfg(not(windows))]
    {
        Err(ShellRuntimeError::UnsupportedPlatform)
    }
}

#[cfg(all(windows, test))]
#[path = "shell_runtime/runtime_tests.rs"]
mod tests;
