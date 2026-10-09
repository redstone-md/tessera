// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One owned GUI child and one local heartbeat event. Windows kernel waits
//! supervise readiness; only diagnostic stderr is drained by a finite reader.
//! Rust's process module owns handles and quotes arguments.

use std::io::{self, Read};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};

use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, GetLastError, HANDLE, WAIT_FAILED, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

use super::error::ShellRuntimeError;
use super::heartbeat::wait_any;
pub(crate) use super::heartbeat::{SupervisorEvent, WaitOutcome};
use super::runtime_proof::{
    DiagnosticPipe, DiagnosticProcess, PipeRead, ProofWait, StderrCapture, StderrRecord,
    diagnostic_error, preflight_error, verify_child,
};

pub(crate) const HEARTBEAT_TIMEOUT_MS: u32 = 30_000;
const CLEANUP_TIMEOUT_MS: u32 = 5_000;

pub(crate) struct OwnedChild {
    process: Child,
    finished: bool,
    stderr: Option<StderrCapture>,
}

/// The supervisor->GUI heartbeat argument for ordinary session children.
const SESSION_HEARTBEAT_ARG: &str = "--shell-heartbeat";
/// The distinct diagnostic argument: a child started with it must never
/// hide the taskbar, change appbar state, or touch Winlogon.
const DIAGNOSTIC_HEARTBEAT_ARG: &str = "--verify-heartbeat";

impl OwnedChild {
    fn spawn_with(
        executable: &Path,
        event: &SupervisorEvent,
        heartbeat_arg: &str,
    ) -> Result<Self, ShellRuntimeError> {
        if !executable.is_absolute() {
            return Err(ShellRuntimeError::UnusablePath {
                context: "child executable is not absolute",
            });
        }
        let diagnostic = heartbeat_arg == DIAGNOSTIC_HEARTBEAT_ARG;
        let process = Command::new(executable)
            .arg(heartbeat_arg)
            .arg(event.name())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(if diagnostic {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .spawn()
            .map_err(|error| ShellRuntimeError::SpawnFailed {
                code: error.raw_os_error().unwrap_or_default() as u32,
            })?;
        let mut child = Self {
            process,
            finished: false,
            stderr: None,
        };
        if diagnostic {
            let capture = child
                .process
                .stderr
                .take()
                .ok_or(ShellRuntimeError::HeartbeatViolation {
                    reason: "diagnostic GUI stderr pipe missing",
                })
                .and_then(|pipe| {
                    StderrCapture::start(DiagnosticStderr(pipe))
                        .map_err(|error| process_error("start diagnostic stderr reader", error))
                });
            match capture {
                Ok(capture) => child.stderr = Some(capture),
                Err(error) => {
                    let cleanup = child.stop();
                    return Err(diagnostic_error(
                        "spawn",
                        0,
                        None,
                        error,
                        cleanup.err(),
                        StderrRecord::default(),
                    ));
                }
            }
        }
        Ok(child)
    }

    pub(crate) fn spawn(
        executable: &Path,
        event: &SupervisorEvent,
    ) -> Result<Self, ShellRuntimeError> {
        Self::spawn_with(executable, event, SESSION_HEARTBEAT_ARG)
    }

    fn handle(&self) -> HANDLE {
        self.process.as_raw_handle()
    }

    pub(crate) fn wait(&self, event: &SupervisorEvent) -> Result<WaitOutcome, ShellRuntimeError> {
        wait_any(&[self.handle(), event.raw_handle()], HEARTBEAT_TIMEOUT_MS)
    }

    pub(crate) fn wait_with_presentation(
        &self,
        event: &SupervisorEvent,
        presentation: HANDLE,
        timeout_ms: u32,
    ) -> Result<WaitOutcome, ShellRuntimeError> {
        wait_any(
            &[presentation, self.handle(), event.raw_handle()],
            timeout_ms,
        )
    }

    pub(crate) fn exit_code(&mut self) -> Result<u32, ShellRuntimeError> {
        let status = self
            .process
            .wait()
            .map_err(|error| process_error("wait for GUI", error))?;
        self.finished = true;
        Ok(status.code().unwrap_or(1) as u32)
    }

    pub(crate) fn stop(&mut self) -> Result<(), ShellRuntimeError> {
        if self.finished {
            return Ok(());
        }
        if matches!(self.process.try_wait(), Ok(Some(_))) {
            return self.exit_code().map(|_| ());
        }
        let killed = self
            .process
            .kill()
            .map_err(|error| process_error("stop owned GUI", error));
        // SAFETY: this handle belongs to our live Child and remains owned during the wait.
        let wait = unsafe { WaitForSingleObject(self.handle(), CLEANUP_TIMEOUT_MS) };
        if wait != WAIT_OBJECT_0 {
            return killed.and(Err(ShellRuntimeError::Windows {
                operation: "reap owned GUI",
                code: if wait == WAIT_FAILED {
                    // SAFETY: queried immediately after the failed kernel wait.
                    unsafe { GetLastError() }
                } else {
                    wait
                },
            }));
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

/// Uses the exact production GUI. No registry/backup reads or writes, and no
/// Explorer launch: two fresh pulses prove event-loop and timer readiness.
pub(crate) fn verify_supervision(executable: &Path) -> Result<(), ShellRuntimeError> {
    let event = SupervisorEvent::create(std::process::id()).map_err(|error| {
        diagnostic_error("event", 0, None, error, None, StderrRecord::default())
    })?;
    let mut child = OwnedChild::spawn_with(executable, &event, DIAGNOSTIC_HEARTBEAT_ARG)
        .map_err(preflight_error)?;
    verify_child(&mut DiagnosticChild {
        child: &mut child,
        event: &event,
    })
}

struct DiagnosticChild<'a> {
    child: &'a mut OwnedChild,
    event: &'a SupervisorEvent,
}

impl DiagnosticProcess for DiagnosticChild<'_> {
    fn wait(&mut self) -> Result<ProofWait, ShellRuntimeError> {
        match self.child.wait(self.event)? {
            WaitOutcome::Signalled(1) => Ok(ProofWait::Pulse),
            WaitOutcome::Signalled(0) => Ok(ProofWait::Exited),
            WaitOutcome::TimedOut => Ok(ProofWait::TimedOut),
            WaitOutcome::Signalled(index) => Err(ShellRuntimeError::Windows {
                operation: "unexpected diagnostic wait index",
                code: index as u32,
            }),
        }
    }

    fn exit_code(&mut self) -> Result<u32, ShellRuntimeError> {
        self.child.exit_code()
    }

    fn stop(&mut self) -> Result<(), ShellRuntimeError> {
        self.child.stop()
    }

    fn finish_stderr(&mut self) -> StderrRecord {
        self.child
            .stderr
            .as_mut()
            .map_or_else(StderrRecord::default, StderrCapture::finish)
    }
}

struct DiagnosticStderr(ChildStderr);

impl DiagnosticPipe for DiagnosticStderr {
    fn read_available(&mut self, buffer: &mut [u8]) -> io::Result<PipeRead> {
        let mut available = 0;
        // SAFETY: this worker exclusively owns the live ChildStderr read handle.
        // No other thread reads/closes it, so synchronous pipe IO cannot contend
        // with another read. Only the available-byte count is requested.
        let peek = unsafe {
            PeekNamedPipe(
                self.0.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if peek == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                Ok(PipeRead::Closed)
            } else {
                Err(error)
            };
        }
        if available == 0 {
            return Ok(PipeRead::Idle);
        }
        // One reader, and at most the bytes already in this anonymous byte pipe:
        // read never waits for the GUI/descendants to produce additional output.
        let count = buffer.len().min(available as usize);
        match self.0.read(&mut buffer[..count]) {
            Ok(0) => Ok(PipeRead::Closed),
            Ok(count) => Ok(PipeRead::Bytes(count)),
            Err(error) if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => {
                Ok(PipeRead::Closed)
            }
            Err(error) => Err(error),
        }
    }
}

pub(crate) fn sibling_tessera_exe(supervisor: &Path) -> Option<PathBuf> {
    let candidate = supervisor.parent()?.join("Tessera.exe");
    candidate.is_file().then_some(candidate)
}

/// Spawns exactly one session GUI child with the heartbeat argument. Same
/// single-owned-child invariant as the takeover path; `gui` must be absolute.
pub(crate) fn spawn_session_child(
    gui: &Path,
    event: &SupervisorEvent,
) -> Result<OwnedChild, ShellRuntimeError> {
    if !gui.is_absolute() {
        return Err(ShellRuntimeError::UnusablePath {
            context: "session GUI path is not absolute",
        });
    }
    OwnedChild::spawn(gui, event)
}
