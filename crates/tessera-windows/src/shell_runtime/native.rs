// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native entry points; deployment state and child ownership stay separate.

use crate::shell_recovery::native::{read_backup, read_live_shell};
use crate::shell_recovery::{OriginalShellState, RestoreDecision};
use crate::shell_runtime::error::ShellRuntimeError;
use crate::shell_runtime::supervisor::{
    WaitOutcome, sibling_tessera_exe, spawn_session_child, verify_supervision,
};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use windows_sys::Win32::System::SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW};

fn system_path(file: &str, windows_directory: bool) -> Result<PathBuf, ShellRuntimeError> {
    let mut buffer = [0u16; 1024];
    // SAFETY: both APIs receive the actual writable buffer length.
    let written = unsafe {
        if windows_directory {
            GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32)
        } else {
            GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32)
        }
    } as usize;
    if written == 0 || written >= buffer.len() {
        return Err(ShellRuntimeError::UnusablePath {
            context: "system directory",
        });
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..written])).join(file))
}

// Absolute OS-resolved executable, separately quoted arguments, no shell/runas.
// Dropping this Child closes our handle but intentionally leaves Explorer/Tasks alive.
fn launch_system(
    file: &str,
    windows_directory: bool,
    arguments: &[OsString],
) -> Result<(), ShellRuntimeError> {
    let exe = system_path(file, windows_directory)?;
    if !exe.is_absolute() {
        return Err(ShellRuntimeError::UnusablePath {
            context: "system executable",
        });
    }
    Command::new(exe)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| ShellRuntimeError::SpawnFailed {
            code: error.raw_os_error().unwrap_or_default() as u32,
        })
}

pub(crate) fn verify_runtime_impl() -> Result<(), ShellRuntimeError> {
    let supervisor = crate::shell_recovery::native::supervisor_path()?;
    let child = sibling_tessera_exe(&supervisor).ok_or(ShellRuntimeError::UnusablePath {
        context: "sibling Tessera.exe not found",
    })?;
    verify_supervision(&child)
}

fn perform_restore() -> Result<(), ShellRuntimeError> {
    let _lock = crate::shell_recovery::native::DeploymentLock::acquire()?;
    // Reload under the shared lock: another session may have replaced/restored it.
    let Some(backup) = read_backup()? else {
        return Ok(());
    };
    let live = read_live_shell()?;
    let decision = crate::shell_recovery::decide_restore(Some(&backup), live.as_ref());
    crate::shell_recovery::native::apply_restore(&backup, &decision)?;
    match decision {
        RestoreDecision::RefuseClobber => Err(ShellRuntimeError::RestoreRefusedClobber),
        RestoreDecision::RefuseMalformed(reason) => {
            Err(ShellRuntimeError::BackupRejected(reason.to_string()))
        }
        _ => Ok(()),
    }
}

/// Only a validated active deployment supervises a child. If its backup is
/// damaged but live Winlogon still names this exact supervisor, start Explorer
/// for this session without guessing or overwriting the original registry state.
pub(crate) fn run_shell_impl() -> Result<(), ShellRuntimeError> {
    let supervisor = crate::shell_recovery::native::supervisor_path()?;
    if let Err(error) = super::validate_takeover(&supervisor) {
        let command = supervisor.to_str().map(|path| format!("\"{path}\""));
        if matches!((read_live_shell(), command), (Ok(Some(OriginalShellState::RawString(live))), Some(owned)) if live == owned)
        {
            let _ = launch_system("explorer.exe", true, &[]);
        }
        return Err(error);
    }
    // No early returns after validation: every child startup/wait failure
    // recovers. The takeover session shares the guarded presentation engine
    // (capture-before-hide, readiness-gated hide, exact restore, event-driven
    // re-hide) with the developer session; the persistent recovery step then
    // runs as before.
    let gui = sibling_tessera_exe(&supervisor).ok_or(ShellRuntimeError::UnusablePath {
        context: "sibling Tessera.exe not found",
    });
    let supervised = match gui {
        Ok(gui) => supervise_session(&gui),
        Err(error) => Err(error),
    };
    let restore = perform_restore();
    let explorer = launch_system("explorer.exe", true, &[]);
    supervised.and(restore).and(explorer)
}

pub(crate) fn restore_explorer_impl() -> Result<(), ShellRuntimeError> {
    let restore = perform_restore();
    let explorer = launch_system("explorer.exe", true, &[]);
    restore.and(explorer)
}

pub(crate) fn open_file_manager_impl() -> Result<(), ShellRuntimeError> {
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_Downloads, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };
    let mut pointer = std::ptr::null_mut();
    // SAFETY: valid out-parameter; success transfers a NUL-terminated allocation.
    let status = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_Downloads,
            KF_FLAG_DEFAULT as u32,
            std::ptr::null_mut(),
            &mut pointer,
        )
    };
    if status != 0 || pointer.is_null() {
        return Err(ShellRuntimeError::UnusablePath {
            context: "Downloads folder",
        });
    }
    let mut length = 0;
    // SAFETY: SDK-owned string is NUL-terminated; cap consumption at the native path bound.
    while length < 32768 && unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    let folder = if length < 32768 {
        // SAFETY: measured units are within the returned string allocation.
        Some(OsString::from_wide(unsafe {
            std::slice::from_raw_parts(pointer, length)
        }))
    } else {
        None
    };
    // SAFETY: exactly one free with the API's allocator.
    unsafe { windows_sys::Win32::System::Com::CoTaskMemFree(pointer.cast()) };
    let folder = folder.filter(|path| Path::new(path).is_absolute()).ok_or(
        ShellRuntimeError::UnusablePath {
            context: "Downloads folder",
        },
    )?;
    launch_system("explorer.exe", true, &[folder])
}

pub(crate) fn open_task_manager_impl() -> Result<(), ShellRuntimeError> {
    launch_system("Taskmgr.exe", false, &[])
}

/// Validates the supervisor for `--desktop-session`: the sibling
/// `tessera-shell.exe` in this GUI's directory, an absolute existing file.
/// No shell command, no elevation, no registry access.
fn owned_session_supervisor(gui: &Path) -> Result<(PathBuf, &'static str), ShellRuntimeError> {
    let (supervisor, flag) =
        super::placement::session_command(gui).ok_or(ShellRuntimeError::UnusablePath {
            context: "current GUI is not an allowed Tessera executable",
        })?;
    if !supervisor.is_file() {
        return Err(ShellRuntimeError::UnusablePath {
            context: "owned sibling supervisor (tessera-shell.exe)",
        });
    }
    Ok((supervisor, flag))
}

pub(crate) fn start_desktop_session_impl() -> Result<(), ShellRuntimeError> {
    // This process is the GUI (parent app adapter calls this seam); the
    // absolute current GUI path is the validated argument, never a shell
    // command string.
    let gui = std::env::current_exe().map_err(|error| ShellRuntimeError::Windows {
        operation: "resolve current GUI path",
        code: error.raw_os_error().unwrap_or_default() as u32,
    })?;
    if !gui.is_absolute() {
        return Err(ShellRuntimeError::UnusablePath {
            context: "current GUI path is relative",
        });
    }
    let (supervisor, flag) = owned_session_supervisor(&gui)?;
    // The supervisor (tessera-shell.exe) is spawned with
    // `--desktop-session <absolute-current-GUI-path>`; the GUI never
    // receives this flag.
    let _ = Command::new(&supervisor)
        .arg(flag)
        .arg(&gui)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| ShellRuntimeError::SpawnFailed {
            code: error.raw_os_error().unwrap_or_default() as u32,
        })?;
    Ok(())
}

/// Validates the passed GUI for the session run: same folder as this
/// supervisor, exactly `Tessera.exe` (packaged) or `tessera-desktop.exe`
/// (developer build), an absolute existing file. An arbitrary path is
/// refused.
fn validated_session_gui(gui: &Path, supervisor: &Path) -> Result<PathBuf, ShellRuntimeError> {
    let expected = super::placement::session_command(gui).map(|(executable, _)| executable);
    if expected.as_deref() != Some(supervisor) || !gui.is_file() {
        return Err(ShellRuntimeError::UnusablePath {
            context: "session GUI is not an allowed same-directory Tessera executable",
        });
    }
    Ok(gui.to_path_buf())
}

/// The shared guarded session engine for one GUI child (used by both the
/// takeover session and the developer session): presentation capture,
/// readiness-gated hide, event-driven re-hide, and stop-reap-then-restore
/// ordering on every outcome.
fn supervise_session(gui: &Path) -> Result<(), ShellRuntimeError> {
    // Both ordinary and persistent paths acquire this before spawning or hiding.
    let _owner = super::native_presentation::PresentationOwner::acquire()?;
    let mut guard = super::native_presentation::PresentationGuard::new()?;
    guard.prepare()?;
    let event = super::supervisor::SupervisorEvent::create(std::process::id())?;
    // Declaration order is intentional: RAII reaps child before guard restores.
    let mut child = spawn_session_child(gui, &event)?;
    let result = (|| {
        match child.wait(&event)? {
            WaitOutcome::Signalled(1) => {}
            WaitOutcome::Signalled(0) => {
                return Err(ShellRuntimeError::HeartbeatViolation {
                    reason: "GUI exited before readiness",
                });
            }
            _ => {
                return Err(ShellRuntimeError::HeartbeatViolation {
                    reason: "GUI never pulsed readiness",
                });
            }
        }
        guard.hide_after_ready()?;
        let mut last_heartbeat = std::time::Instant::now();
        // The presentation event is first, so a queued failure is not starved by
        // heartbeat pulses. It coalesces taskbar changes instead of queueing them.
        loop {
            let remaining = std::time::Duration::from_millis(u64::from(
                super::supervisor::HEARTBEAT_TIMEOUT_MS,
            ))
            .saturating_sub(last_heartbeat.elapsed());
            if remaining.is_zero() {
                return Err(ShellRuntimeError::HeartbeatViolation {
                    reason: "GUI heartbeat lost",
                });
            }
            match child.wait_with_presentation(
                &event,
                guard.signal_handle(),
                remaining.as_millis() as u32,
            )? {
                WaitOutcome::Signalled(0) => guard.process_signal()?,
                WaitOutcome::Signalled(1) => {
                    let code = child.exit_code()?;
                    return if code == 0 {
                        Ok(())
                    } else {
                        Err(ShellRuntimeError::Windows {
                            operation: "GUI exited",
                            code,
                        })
                    };
                }
                WaitOutcome::Signalled(2) => last_heartbeat = std::time::Instant::now(),
                _ => {
                    return Err(ShellRuntimeError::HeartbeatViolation {
                        reason: "GUI heartbeat lost",
                    });
                }
            }
        }
    })();
    guard.stop_watcher();
    super::session_guard::finish_session(result, || child.stop(), || guard.restore())
}

/// Validates the same-directory GUI, then uses the persistent path's session engine.
pub(crate) fn run_desktop_session_impl(gui: &Path) -> Result<(), ShellRuntimeError> {
    let supervisor = crate::shell_recovery::native::supervisor_path()?;
    let gui = validated_session_gui(gui, &supervisor)?;
    supervise_session(&gui)
}

pub(crate) fn desktop_identity_impl()
-> Result<super::identity::DesktopIdentity, crate::apps::ApplicationError> {
    super::identity::desktop_identity()
}

pub(crate) fn clock_text_impl() -> Result<String, crate::apps::ApplicationError> {
    super::identity::clock_text()
}
