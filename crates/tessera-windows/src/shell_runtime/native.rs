// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native entry points; deployment state and child ownership stay separate.

use crate::shell_recovery::native::{read_backup, read_live_shell};
use crate::shell_recovery::{OriginalShellState, RestoreDecision};
use crate::shell_runtime::error::ShellRuntimeError;
use crate::shell_runtime::supervisor::{
    ChildOutcome, sibling_tessera_exe, supervise, verify_supervision,
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
    // No early returns after validation: every child startup/wait failure recovers.
    let supervised = sibling_tessera_exe(&supervisor)
        .ok_or(ShellRuntimeError::UnusablePath {
            context: "sibling Tessera.exe not found",
        })
        .and_then(|child| supervise(&child))
        .and_then(|outcome| match outcome {
            ChildOutcome::Exited(0) => Ok(()),
            ChildOutcome::Exited(code) => Err(ShellRuntimeError::Windows {
                operation: "GUI exited",
                code,
            }),
            ChildOutcome::HeartbeatTimeout => Err(ShellRuntimeError::HeartbeatViolation {
                reason: "GUI heartbeat stopped",
            }),
        });
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
