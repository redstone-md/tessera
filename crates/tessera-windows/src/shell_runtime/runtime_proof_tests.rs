// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

struct RecordedChild {
    waits: VecDeque<Result<ProofWait, ShellRuntimeError>>,
    exit: Option<Result<u32, ShellRuntimeError>>,
    cleanup: Option<ShellRuntimeError>,
    stderr: StderrRecord,
    capture: Option<StderrCapture>,
    steps: Vec<&'static str>,
}

impl RecordedChild {
    fn new(waits: impl IntoIterator<Item = ProofWait>) -> Self {
        Self {
            waits: waits.into_iter().map(Ok).collect(),
            exit: Some(Ok(0xC000_0005)),
            cleanup: None,
            stderr: StderrRecord {
                bytes: b"thread 'main' panicked at diagnostic startup".to_vec(),
                status: "complete",
                ..StderrRecord::default()
            },
            capture: None,
            steps: Vec::new(),
        }
    }
}

impl DiagnosticProcess for RecordedChild {
    fn wait(&mut self) -> Result<ProofWait, ShellRuntimeError> {
        self.steps.push("wait");
        self.waits.pop_front().unwrap()
    }

    fn exit_code(&mut self) -> Result<u32, ShellRuntimeError> {
        self.steps.push("exit");
        self.exit.take().unwrap()
    }

    fn stop(&mut self) -> Result<(), ShellRuntimeError> {
        self.steps.push("stop");
        self.cleanup.take().map_or(Ok(()), Err)
    }

    fn finish_stderr(&mut self) -> StderrRecord {
        self.steps.push("stderr");
        match self.capture.as_mut() {
            Some(capture) => capture.finish(),
            None => std::mem::take(&mut self.stderr),
        }
    }
}

#[test]
fn immediate_gui_exit_keeps_native_status_stderr_and_owned_cleanup() {
    let mut child = RecordedChild::new([ProofWait::Exited]);
    let error = verify_child(&mut child).unwrap_err();
    assert_eq!(child.steps, ["wait", "exit", "stop", "stderr"]);
    let record = error.to_string();
    assert!(record.starts_with("tessera-runtime stage=processExit error=heartbeat pulses=0 exit_code=0xC0000005 native_code=none cleanup_native_code=none"));
    assert!(record.contains("stderr_status=complete stderr_hint=panic stderr_native_code=none"));
    match error {
        ShellRuntimeError::DiagnosticFailure { stderr, .. } => {
            assert_eq!(stderr, b"thread 'main' panicked at diagnostic startup");
        }
        error => panic!("unexpected error: {error}"),
    }
}

#[test]
fn zero_or_one_pulse_timeout_never_skips_cleanup() {
    for (waits, stage, pulses) in [
        (vec![ProofWait::TimedOut], "wait1", 0),
        (vec![ProofWait::Pulse, ProofWait::TimedOut], "wait2", 1),
    ] {
        let mut child = RecordedChild::new(waits);
        let error = verify_child(&mut child).unwrap_err();
        assert_eq!(&child.steps[child.steps.len() - 2..], ["stop", "stderr"]);
        assert!(matches!(error, ShellRuntimeError::DiagnosticFailure {
            stage: actual_stage, pulses: actual_pulses, exit_code: None, ..
        } if actual_stage == stage && actual_pulses == pulses));
    }
}

#[test]
fn two_pulses_are_success_only_after_owned_cleanup_succeeds() {
    let mut child = RecordedChild::new([ProofWait::Pulse, ProofWait::Pulse]);
    verify_child(&mut child).unwrap();
    assert_eq!(child.steps, ["wait", "wait", "stop", "stderr"]);
    child = RecordedChild::new([ProofWait::Pulse, ProofWait::Pulse]);
    child.cleanup = Some(ShellRuntimeError::Windows {
        operation: "reap owned GUI",
        code: 5,
    });
    let error = verify_child(&mut child).unwrap_err();
    assert!(error.to_string().starts_with(
        "tessera-runtime stage=cleanup error=windows pulses=2 exit_code=none native_code=0x00000005"
    ));
    assert_eq!(child.steps, ["wait", "wait", "stop", "stderr"]);
}

#[test]
fn wait_and_exit_status_errors_keep_primary_code_and_secondary_cleanup_code() {
    let mut child = RecordedChild::new([]);
    child.waits.push_back(Err(ShellRuntimeError::Windows {
        operation: "WaitForMultipleObjects",
        code: 6,
    }));
    child.cleanup = Some(ShellRuntimeError::Windows {
        operation: "stop owned GUI",
        code: 5,
    });
    let record = verify_child(&mut child).unwrap_err().to_string();
    assert!(record.starts_with("tessera-runtime stage=wait1 error=windows pulses=0 exit_code=none native_code=0x00000006 cleanup_native_code=0x00000005"));
    assert_eq!(child.steps, ["wait", "stop", "stderr"]);
    let mut child = RecordedChild::new([ProofWait::Exited]);
    child.exit = Some(Err(ShellRuntimeError::Windows {
        operation: "wait for GUI",
        code: 6,
    }));
    assert!(verify_child(&mut child).unwrap_err().to_string().starts_with("tessera-runtime stage=processExit error=windows pulses=0 exit_code=none native_code=0x00000006"));
    assert_eq!(child.steps, ["wait", "exit", "stop", "stderr"]);
}

struct RecordedPipe {
    bytes: Vec<u8>,
    offset: usize,
    stay_open: bool,
    drained: Option<mpsc::Sender<()>>,
    dropped: mpsc::Sender<()>,
    failure: Option<u32>,
    panic_after_drain: bool,
}

impl DiagnosticPipe for RecordedPipe {
    fn read_available(&mut self, buffer: &mut [u8]) -> io::Result<PipeRead> {
        let count = buffer.len().min(self.bytes.len() - self.offset);
        if count > 0 {
            buffer[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
            self.offset += count;
            return Ok(PipeRead::Bytes(count));
        }
        assert!(!self.panic_after_drain, "recorded reader panic");
        if let Some(sender) = self.drained.take() {
            let _ = sender.send(());
        }
        if let Some(code) = self.failure {
            return Err(io::Error::from_raw_os_error(code as i32));
        }
        Ok(if self.stay_open {
            PipeRead::Idle
        } else {
            PipeRead::Closed
        })
    }
}

impl Drop for RecordedPipe {
    fn drop(&mut self) {
        let _ = self.dropped.send(());
    }
}

#[test]
fn stderr_is_drained_while_running_and_tail_is_bounded_without_waiting_for_eof() {
    let (drained, drained_receiver) = mpsc::channel();
    let (dropped, dropped_receiver) = mpsc::channel();
    let mut bytes = vec![b'x'; STDERR_LIMIT * 4];
    bytes.extend_from_slice(b"last useful error");
    let expected = bytes[bytes.len() - STDERR_LIMIT..].to_vec();
    let mut capture = StderrCapture::start(RecordedPipe {
        bytes,
        offset: 0,
        stay_open: true,
        drained: Some(drained),
        dropped,
        failure: None,
        panic_after_drain: false,
    })
    .unwrap();
    // This happens before finish/cleanup: output larger than the retained buffer
    // must be consumed rather than leaving a full pipe blocking the real child.
    drained_receiver
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let start = Instant::now();
    let record = capture.finish();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(record.bytes, expected);
    assert!(record.truncated);
    assert_eq!(record.status, "incomplete");
    dropped_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
}

#[test]
fn reader_error_retains_tail_and_releases_its_owned_pipe() {
    let (dropped, dropped_receiver) = mpsc::channel();
    let mut capture = StderrCapture::start(RecordedPipe {
        bytes: b"startup error".to_vec(),
        offset: 0,
        stay_open: false,
        drained: None,
        dropped,
        failure: Some(5),
        panic_after_drain: false,
    })
    .unwrap();
    let record = capture.finish();
    assert_eq!(record.bytes, b"startup error");
    assert_eq!(record.status, "readError");
    assert_eq!(record.native_code, Some(5));
    dropped_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
}

#[test]
fn reader_panic_cannot_skip_child_cleanup_or_lose_the_owned_pipe() {
    let (dropped, dropped_receiver) = mpsc::channel();
    let capture = StderrCapture::start(RecordedPipe {
        bytes: b"before reader panic".to_vec(),
        offset: 0,
        stay_open: false,
        drained: None,
        dropped,
        failure: None,
        panic_after_drain: true,
    })
    .unwrap();
    let mut child = RecordedChild::new([ProofWait::Exited]);
    child.capture = Some(capture);
    let error = verify_child(&mut child).unwrap_err();
    assert_eq!(child.steps, ["wait", "exit", "stop", "stderr"]);
    assert!(error.to_string().contains("stderr_status=panic"));
    assert!(
        matches!(error, ShellRuntimeError::DiagnosticFailure { stderr, .. }
        if stderr == b"before reader panic")
    );
    dropped_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
}

#[test]
fn preflight_path_errors_get_a_record_without_double_wrapping_owned_child_failures() {
    let error = preflight_error(ShellRuntimeError::UnusablePath {
        context: "sibling Tessera.exe not found",
    });
    assert!(error.to_string().starts_with(
        "tessera-runtime stage=spawn error=path pulses=0 exit_code=none native_code=none"
    ));
    assert_eq!(
        std::error::Error::source(&error).unwrap().to_string(),
        "path unusable: sibling Tessera.exe not found"
    );
    let mut child = RecordedChild::new([ProofWait::Exited]);
    let error = verify_child(&mut child).unwrap_err();
    let original = error.to_string();
    assert_eq!(preflight_error(error).to_string(), original);
}
