// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

#![forbid(unsafe_code)]
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args_os().nth(1).is_some() {
        tessera_windows::show_startup_error(
            "The desktop launcher takes no arguments. Use tessera-cli.exe for developer commands.",
        );
        return ExitCode::from(2);
    }
    match tessera::run_panel() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tessera_windows::show_startup_error(&error.to_string());
            ExitCode::FAILURE
        }
    }
}
