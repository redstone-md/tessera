// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

#![forbid(unsafe_code)]
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let heartbeat = match arguments.as_slice() {
        [] => None,
        [flag, event] if flag == "--shell-heartbeat" => match event.to_str() {
            Some(event) => Some(event),
            None => {
                tessera_windows::show_startup_error("The supervisor heartbeat name is invalid.");
                return ExitCode::from(2);
            }
        },
        _ => {
            tessera_windows::show_startup_error(
                "Start Tessera normally. Shell activation uses Install-Tessera.ps1; developer commands use tessera-cli.exe.",
            );
            return ExitCode::from(2);
        }
    };
    match tessera::run_desktop(heartbeat) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tessera_windows::show_startup_error(&error.to_string());
            ExitCode::FAILURE
        }
    }
}
