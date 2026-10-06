// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Installed shell supervisor entry point.
//!
//! GUI-subsystem binary. With no arguments it verifies that this executable
//! is the recorded active shell, supervises exactly one Tessera child with a
//! heartbeat, restores the user's original shell, and starts Explorer for
//! the current session. With `--desktop-session <owned-gui>` it supervises
//! a temporary desktop presentation without changing the sign-in shell.
//! With `--verify-runtime` it runs the installer's diagnostic preflight:
//! the production UI starts, pulses twice, and is cleaned up, without taskbar,
//! work-area, registry, or Explorer side effects. Diagnostic failures return
//! a nonzero exit code without a blocking dialog; presentation failures are
//! reported through a bounded dialog after cleanup and fallback.

#![cfg_attr(windows, windows_subsystem = "windows")]
#![forbid(unsafe_code)]

use std::process::ExitCode;

use tessera_windows::{
    ShellRuntimeError, run_desktop_session, run_shell, show_startup_error, verify_runtime,
};

fn main() -> ExitCode {
    // Diagnostics are read-only; a desktop session owns only transient taskbar
    // presentation, while the no-argument sign-in shell owns persistent recovery.
    match std::env::args_os().skip(1).collect::<Vec<_>>().as_slice() {
        [only] if only == "--verify-runtime" => match verify_runtime() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Tessera runtime verification failed: {error}");
                ExitCode::FAILURE
            }
        },
        [flag, gui] if flag == "--desktop-session" => {
            match run_desktop_session(std::path::Path::new(gui)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    report(&error);
                    ExitCode::FAILURE
                }
            }
        }
        [] => match run_shell() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                report(&error);
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!(
                "tessera-shell: takes --verify-runtime or --desktop-session <owned-gui>, or no arguments for the installed shell"
            );
            ExitCode::from(2)
        }
    }
}

/// Reports a presentation failure only after owned-child cleanup and the
/// applicable transient/persistent restoration have run.
fn report(error: &ShellRuntimeError) {
    show_startup_error(&format!("Tessera shell could not run: {error}"));
}
