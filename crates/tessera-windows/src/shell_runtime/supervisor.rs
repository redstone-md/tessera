// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One owned GUI child and one local heartbeat event. Windows kernel waits
//! avoid polling; Rust's process module owns handles and quotes arguments.

use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

use super::error::ShellRuntimeError;
use super::heartbeat::{SupervisorEvent, WaitOutcome, wait_any};

const HEARTBEAT_TIMEOUT_MS: u32 = 30_000;
const CLEANUP_TIMEOUT_MS: u32 = 5_000;

struct OwnedChild {
    process: Child,
    finished: bool,
}

impl OwnedChild {
    fn spawn(executable: &Path, event: &SupervisorEvent) -> Result<Self, ShellRuntimeError> {
        if !executable.is_absolute() {
            return Err(ShellRuntimeError::UnusablePath {
                context: "child executable is not absolute",
            });
        }
        let process = Command::new(executable)
            .arg("--shell-heartbeat")
            .arg(event.name())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| ShellRuntimeError::SpawnFailed {
                code: error.raw_os_error().unwrap_or_default() as u32,
            })?;
        Ok(Self {
            process,
            finished: false,
        })
    }

    fn handle(&self) -> HANDLE {
        self.process.as_raw_handle()
    }

    fn wait(&self, event: &SupervisorEvent) -> Result<WaitOutcome, ShellRuntimeError> {
        wait_any(&[self.handle(), event.raw_handle()], HEARTBEAT_TIMEOUT_MS)
    }

    fn exit_code(&mut self) -> Result<u32, ShellRuntimeError> {
        let status = self
            .process
            .wait()
            .map_err(|error| process_error("wait for GUI", error))?;
        self.finished = true;
        Ok(status.code().unwrap_or(1) as u32)
    }

    fn stop(&mut self) -> Result<(), ShellRuntimeError> {
        self.process
            .kill()
            .map_err(|error| process_error("stop owned GUI", error))?;
        // SAFETY: this handle belongs to our live Child and remains owned during the wait.
        let wait = unsafe { WaitForSingleObject(self.handle(), CLEANUP_TIMEOUT_MS) };
        if wait != WAIT_OBJECT_0 {
            return Err(ShellRuntimeError::Windows {
                operation: "reap owned GUI",
                code: wait,
            });
        }
        self.exit_code().map(|_| ())
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.finished {
            // Every early-return path attempts bounded cleanup of only our own child.
            // Failure cannot authorize killing an unrelated PID or blocking indefinitely.
            let _ = self.stop();
        }
    }
}

fn process_error(operation: &'static str, error: std::io::Error) -> ShellRuntimeError {
    ShellRuntimeError::Windows {
        operation,
        code: error.raw_os_error().unwrap_or_default() as u32,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildOutcome {
    Exited(u32),
    HeartbeatTimeout,
}

pub(crate) fn supervise(executable: &Path) -> Result<ChildOutcome, ShellRuntimeError> {
    let event = SupervisorEvent::create(std::process::id())?;
    let mut child = OwnedChild::spawn(executable, &event)?;
    loop {
        match child.wait(&event)? {
            WaitOutcome::Signalled(0) => return child.exit_code().map(ChildOutcome::Exited),
            WaitOutcome::Signalled(1) => {}
            WaitOutcome::TimedOut => {
                child.stop()?;
                return Ok(ChildOutcome::HeartbeatTimeout);
            }
            WaitOutcome::Signalled(_) => {
                return Err(ShellRuntimeError::HeartbeatViolation {
                    reason: "invalid wait result",
                });
            }
        }
    }
}

/// Uses the exact production GUI. No registry/backup reads or writes, and no
/// Explorer launch: two fresh pulses prove event-loop and timer readiness.
pub(crate) fn verify_supervision(executable: &Path) -> Result<(), ShellRuntimeError> {
    let event = SupervisorEvent::create(std::process::id())?;
    let mut child = OwnedChild::spawn(executable, &event)?;
    for _ in 0..2 {
        match child.wait(&event)? {
            WaitOutcome::Signalled(1) => {}
            WaitOutcome::Signalled(0) => {
                child.exit_code()?;
                return Err(ShellRuntimeError::HeartbeatViolation {
                    reason: "diagnostic GUI exited before two heartbeats",
                });
            }
            _ => {
                return Err(ShellRuntimeError::HeartbeatViolation {
                    reason: "diagnostic GUI did not produce two heartbeats",
                });
            }
        }
    }
    child.stop()
}

pub(crate) fn sibling_tessera_exe(supervisor: &Path) -> Option<PathBuf> {
    let candidate = supervisor.parent()?.join("Tessera.exe");
    candidate.is_file().then_some(candidate)
}
