// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Installed shell supervisor entry point.
//!
//! GUI-subsystem binary. With no arguments it verifies that this executable
//! is the recorded active shell, supervises exactly one Tessera child with a
//! heartbeat, restores the user's original shell, and starts Explorer for
//! the current session. With `--verify-runtime` it runs the installer's
//! diagnostic preflight: the production UI starts, pulses twice, and is
//! cleaned up, with no registry or Explorer side effects. Diagnostic failures
//! return a nonzero exit code without a blocking dialog; shell failures are
//! reported through a bounded dialog after the fallback attempt.

#![cfg_attr(windows, windows_subsystem = "windows")]
#![forbid(unsafe_code)]

use std::process::ExitCode;

use tessera_windows::{ShellRuntimeError, run_shell, show_startup_error, verify_runtime};

fn main() -> ExitCode {
    // The only accepted argument is the diagnostic preflight used by the
    // source installer before takeover; diagnostic errors must not block on a dialog.
    match std::env::args_os().skip(1).collect::<Vec<_>>().as_slice() {
        [only] if only == "--verify-runtime" => match verify_runtime() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Tessera runtime verification failed: {error}");
                ExitCode::FAILURE
            }
        },
        [] => match run_shell() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                report(&error);
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("tessera-shell: takes no arguments except --verify-runtime");
            ExitCode::from(2)
        }
    }
}

/// Reports a startup failure after the Explorer fallback has run (the
/// takeover path) or after the diagnostic child has been cleaned up (the
/// preflight path).
fn report(error: &ShellRuntimeError) {
    show_startup_error(&format!("Tessera shell could not run: {error}"));
}
