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
//! The developer session path ([`start_desktop_session`] /
//! [`run_desktop_session`]) owns the reversible taskbar presentation for one
//! ordinary (non-elevated, non-service) supervisor run: hide only after GUI
//! readiness, primary monitor only, exact captured restore, event-driven
//! re-hide, single presentation owner per session. Diagnostics
//! ([`verify_runtime`], `--verify-heartbeat`) never mutate the taskbar or
//! appbar state.
//!
//! Error dialogs are never shown here: the module returns contextual errors
//! and the GUI entry point reports them after the Explorer fallback.

pub use error::ShellRuntimeError;
#[cfg(windows)]
pub use heartbeat::ShellHeartbeat;

#[cfg(windows)]
pub use identity::DesktopIdentity;
#[cfg(windows)]
pub use surface::{OwnedShellSurface, request_owned_foreground};

/// Native shell surface role, available on all targets for typed host adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellSurfaceKind {
    /// Floating dock; never reserves the work area.
    Dock,
    /// The only appbar surface: a 32-logical-pixel top toolbar.
    Toolbar,
    /// Activatable tool window for search.
    Launcher,
    /// Activatable tool window for shell-owned menus and module popups.
    Popup,
}

#[cfg(not(windows))]
pub struct OwnedShellSurface {
    kind: ShellSurfaceKind,
}

#[cfg(not(windows))]
impl OwnedShellSurface {
    pub fn attach(_handle: isize, _kind: ShellSurfaceKind) -> Result<Self, ShellRuntimeError> {
        Err(ShellRuntimeError::UnsupportedPlatform)
    }

    pub fn reserve(&self) -> Result<(), ShellRuntimeError> {
        Err(ShellRuntimeError::UnsupportedPlatform)
    }

    pub fn kind(&self) -> ShellSurfaceKind {
        self.kind
    }
}

#[cfg(not(windows))]
pub fn request_owned_foreground(_handle: isize) -> Result<bool, ShellRuntimeError> {
    Err(ShellRuntimeError::UnsupportedPlatform)
}

use crate::apps::ApplicationError;
#[cfg(windows)]
use crate::shell_recovery::{OriginalShellState, ShellRecoveryBackup};

#[cfg(not(windows))]
pub use identity_stub::DesktopIdentity;
#[cfg(not(windows))]
mod identity_stub {
    /// Non-Windows stub: desktop identity requires Windows; never fabricated.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DesktopIdentity {
        pub user_name: String,
        pub clock: String,
        pub language: String,
        pub foreground_window: Option<tessera_core::WindowId>,
    }
}

pub(crate) mod error;
#[cfg(windows)]
pub(crate) mod heartbeat;
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod supervisor;

#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod native;

#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod identity;
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod native_presentation;
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) mod placement;
#[cfg(any(windows, test))]
pub(crate) mod session_guard;
#[cfg(test)]
#[path = "shell_runtime/session_guard_tests.rs"]
mod session_guard_tests;
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod surface;

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
/// heartbeat event, spawns the real GUI with `--verify-heartbeat`, waits for
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

/// Bootstraps the ordinary desktop session: from the GUI process, spawns the
/// sibling `tessera-shell.exe` supervisor with
/// `--desktop-session <absolute-current-GUI-path>` and returns. No
/// elevation, no shell command, no registry/Winlogon access.
pub fn start_desktop_session() -> Result<(), ShellRuntimeError> {
    #[cfg(windows)]
    {
        crate::shell_runtime::native::start_desktop_session_impl()
    }
    #[cfg(not(windows))]
    {
        Err(ShellRuntimeError::UnsupportedPlatform)
    }
}

/// Runs the developer session supervisor in-process: owns exactly one GUI
/// child, watches its heartbeat with the same guarded supervision as
/// [`run_shell`], reversibly hides the primary-monitor taskbar only after
/// the GUI readiness pulse, and restores the exact original visibility and
/// appbar auto-hide state on child exit/crash/timeout/errors. Event-driven
/// re-hide on taskbar re-creation; no Explorer kill/restart, no services,
/// no periodic polling, no input hooks. A second presentation owner in this
/// session is refused without touching the running one.
pub fn run_desktop_session(gui: &std::path::Path) -> Result<(), ShellRuntimeError> {
    #[cfg(windows)]
    {
        crate::shell_runtime::native::run_desktop_session_impl(gui)
    }
    #[cfg(not(windows))]
    {
        let _ = gui;
        Err(ShellRuntimeError::UnsupportedPlatform)
    }
}

/// Bounded genuine desktop identity for the shell surfaces: user name,
/// locale clock text, language display name, and the raw-HWND identity of
/// the eligible foreground window (when any). No simulated telemetry; off
/// Windows this reports an error instead of fabricated data.
pub fn desktop_identity() -> Result<DesktopIdentity, ApplicationError> {
    #[cfg(windows)]
    {
        crate::shell_runtime::native::desktop_identity_impl()
    }
    #[cfg(not(windows))]
    {
        Err(ApplicationError::UnsupportedPlatform)
    }
}

/// Current native clock text (user locale, minutes granularity). Reads only
/// date/time APIs; the UI calls this at most once per minute for clock text.
pub fn clock_text() -> Result<String, ApplicationError> {
    #[cfg(windows)]
    {
        crate::shell_runtime::native::clock_text_impl()
    }
    #[cfg(not(windows))]
    {
        Err(ApplicationError::UnsupportedPlatform)
    }
}

#[cfg(all(windows, test))]
#[path = "shell_runtime/runtime_tests.rs"]
mod tests;
