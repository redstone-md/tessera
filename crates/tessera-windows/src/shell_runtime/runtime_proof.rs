// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Diagnostic-only proof and bounded stderr drain. No desktop or registry APIs.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use super::error::ShellRuntimeError;

pub(crate) const STDERR_LIMIT: usize = 16 * 1024;
const DRAIN_GRACE: Duration = Duration::from_millis(200);
const READER_WAIT: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(crate) struct StderrRecord {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
    pub(crate) status: &'static str,
    pub(crate) native_code: Option<u32>,
}

pub(crate) enum ProofWait {
    Pulse,
    Exited,
    TimedOut,
}

pub(crate) trait DiagnosticProcess {
    fn wait(&mut self) -> Result<ProofWait, ShellRuntimeError>;
    fn exit_code(&mut self) -> Result<u32, ShellRuntimeError>;
    fn stop(&mut self) -> Result<(), ShellRuntimeError>;
    fn finish_stderr(&mut self) -> StderrRecord;
}

pub(crate) fn diagnostic_error(
    stage: &'static str,
    pulses: u8,
    exit_code: Option<u32>,
    source: ShellRuntimeError,
    cleanup_error: Option<ShellRuntimeError>,
    stderr: StderrRecord,
) -> ShellRuntimeError {
    ShellRuntimeError::DiagnosticFailure {
        stage,
        pulses,
        exit_code,
        source: Box::new(source),
        cleanup_error: cleanup_error.map(Box::new),
        stderr: stderr.bytes,
        stderr_truncated: stderr.truncated,
        stderr_status: if stderr.status.is_empty() {
            "complete"
        } else {
            stderr.status
        },
        stderr_native_code: stderr.native_code,
    }
}

/// Path resolution happens before an owned child exists. Preserve richer
/// failures unchanged when this also wraps the outer diagnostic entry point.
pub(crate) fn preflight_error(error: ShellRuntimeError) -> ShellRuntimeError {
    match error {
        ShellRuntimeError::DiagnosticFailure { .. } => error,
        error => diagnostic_error("spawn", 0, None, error, None, StderrRecord::default()),
    }
}

/// Both real waits and cleanup must succeed. Every acquired child is stopped,
/// even when a wait or exit-status read fails; stderr is finalized only after it.
pub(crate) fn verify_child(child: &mut impl DiagnosticProcess) -> Result<(), ShellRuntimeError> {
    let mut pulses = 0;
    let mut exit_code = None;
    let mut failure = None;
    for stage in ["wait1", "wait2"] {
        match child.wait() {
            Ok(ProofWait::Pulse) => pulses += 1,
            Ok(ProofWait::Exited) => {
                let source = match child.exit_code() {
                    Ok(code) => {
                        exit_code = Some(code);
                        ShellRuntimeError::HeartbeatViolation {
                            reason: "diagnostic GUI exited before two heartbeats",
                        }
                    }
                    Err(error) => error,
                };
                failure = Some(("processExit", source));
                break;
            }
            Ok(ProofWait::TimedOut) => {
                failure = Some((
                    stage,
                    ShellRuntimeError::HeartbeatViolation {
                        reason: "diagnostic GUI did not produce two heartbeats",
                    },
                ));
                break;
            }
            Err(error) => {
                failure = Some((stage, error));
                break;
            }
        }
    }
    let cleanup = child.stop();
    let stderr = child.finish_stderr();
    match (failure, cleanup) {
        (None, Ok(())) => Ok(()),
        (None, Err(error)) => Err(diagnostic_error(
            "cleanup", pulses, exit_code, error, None, stderr,
        )),
        (Some((stage, error)), cleanup) => Err(diagnostic_error(
            stage,
            pulses,
            exit_code,
            error,
            cleanup.err(),
            stderr,
        )),
    }
}

pub(crate) enum PipeRead {
    Bytes(usize),
    Idle,
    Closed,
}

/// Implementations must never wait for new bytes: return Idle instead. The
/// Windows adapter owns the only read handle and reads only bytes already peeked.
pub(crate) trait DiagnosticPipe: Send + 'static {
    fn read_available(&mut self, buffer: &mut [u8]) -> io::Result<PipeRead>;
}

struct Tail {
    bytes: VecDeque<u8>,
    truncated: bool,
    status: &'static str,
    native_code: Option<u32>,
}

impl Tail {
    fn append(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if self.bytes.len() == STDERR_LIMIT {
                self.bytes.pop_front();
                self.truncated = true;
            }
            self.bytes.push_back(*byte);
        }
    }

    fn snapshot(&self) -> StderrRecord {
        StderrRecord {
            bytes: self.bytes.iter().copied().collect(),
            truncated: self.truncated,
            status: self.status,
            native_code: self.native_code,
        }
    }
}

pub(crate) struct StderrCapture {
    tail: Arc<Mutex<Tail>>,
    stop: Arc<AtomicBool>,
    done: mpsc::Receiver<()>,
}

impl StderrCapture {
    pub(crate) fn start(mut pipe: impl DiagnosticPipe) -> io::Result<Self> {
        let tail = Arc::new(Mutex::new(Tail {
            bytes: VecDeque::with_capacity(STDERR_LIMIT),
            truncated: false,
            status: "incomplete",
            native_code: None,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, done) = mpsc::channel();
        let reader_tail = Arc::clone(&tail);
        let reader_stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("tessera-diagnostic-stderr".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut buffer = [0; 2048];
                    let mut deadline = None;
                    loop {
                        if reader_stop.load(Ordering::Acquire) {
                            let end = *deadline.get_or_insert_with(|| Instant::now() + DRAIN_GRACE);
                            if Instant::now() >= end {
                                break;
                            }
                        }
                        match pipe.read_available(&mut buffer) {
                            Ok(PipeRead::Bytes(count)) => reader_tail
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .append(&buffer[..count]),
                            Ok(PipeRead::Closed) => {
                                reader_tail
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .status = "complete";
                                break;
                            }
                            Ok(PipeRead::Idle) => {
                                if deadline.is_some() {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                            Err(error) => {
                                let mut tail = reader_tail
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner());
                                tail.status = "readError";
                                tail.native_code = error.raw_os_error().map(|code| code as u32);
                                break;
                            }
                        }
                    }
                }));
                if result.is_err() {
                    reader_tail
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .status = "panic";
                }
                // Drop the owned pipe before notifying the supervisor. Never join
                // on EOF: a descendant might still hold an inherited write handle.
                drop(pipe);
                let _ = sender.send(());
            })?;
        Ok(Self { tail, stop, done })
    }

    /// Called after owned-child cleanup. The reader stops within its 200 ms
    /// drain budget even if an inherited writer never closes; the caller waits
    /// at most 250 ms for notification, never joins a potentially blocked read.
    pub(crate) fn finish(&mut self) -> StderrRecord {
        self.stop.store(true, Ordering::Release);
        let _ = self.done.recv_timeout(READER_WAIT);
        self.tail
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .snapshot()
    }
}

impl Drop for StderrCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
#[path = "runtime_proof_tests.rs"]
mod tests;
