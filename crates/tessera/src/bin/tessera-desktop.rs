// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

#![forbid(unsafe_code)]
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

#[derive(Debug, PartialEq, Eq)]
enum LaunchRequest<'a> {
    Desktop,
    Supervised(&'a str),
    Diagnostic(&'a str),
}

fn launch_request(arguments: &[std::ffi::OsString]) -> Result<LaunchRequest<'_>, &'static str> {
    match arguments {
        [] => Ok(LaunchRequest::Desktop),
        [flag, event] if flag == "--shell-heartbeat" || flag == "--verify-heartbeat" => {
            let event = event
                .to_str()
                .filter(|name| !name.is_empty() && !name.contains('\0'))
                .ok_or("The supervisor heartbeat name is invalid.")?;
            Ok(if flag == "--verify-heartbeat" {
                LaunchRequest::Diagnostic(event)
            } else {
                LaunchRequest::Supervised(event)
            })
        }
        _ => Err(
            "Start Tessera normally. Shell activation uses Install-Tessera.ps1; developer commands use tessera-cli.exe.",
        ),
    }
}

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    // A supervised child must exit promptly: its supervisor restores Explorer
    // before presenting errors. A blocking child dialog delays that cleanup.
    let supervised = arguments
        .first()
        .is_some_and(|flag| flag == "--verify-heartbeat" || flag == "--shell-heartbeat");
    let request = match launch_request(&arguments) {
        Ok(request) => request,
        Err(message) => {
            if supervised {
                eprintln!("Tessera supervised startup was refused: {message}");
            } else {
                tessera_windows::show_startup_error(message);
            }
            return ExitCode::from(2);
        }
    };
    let result = match request {
        LaunchRequest::Desktop => tessera::run_desktop(None),
        LaunchRequest::Supervised(event) => tessera::run_desktop(Some(event)),
        LaunchRequest::Diagnostic(event) => tessera::run_desktop_diagnostic(event),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if supervised {
                eprintln!("Tessera supervised GUI could not start: {error}");
            } else {
                tessera_windows::show_startup_error(&error.to_string());
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_and_diagnostic_startup_are_distinct() {
        assert_eq!(launch_request(&[]), Ok(LaunchRequest::Desktop));
        assert_eq!(
            launch_request(&["--verify-heartbeat".into(), "event".into()]),
            Ok(LaunchRequest::Diagnostic("event"))
        );
        assert_eq!(
            launch_request(&["--shell-heartbeat".into(), "event".into()]),
            Ok(LaunchRequest::Supervised("event"))
        );
    }

    #[test]
    fn arbitrary_commands_and_incomplete_heartbeat_arguments_are_refused() {
        assert!(launch_request(&["--shell-heartbeat".into()]).is_err());
        assert!(launch_request(&["--enable-shell".into()]).is_err());
        assert!(launch_request(&["--shell-heartbeat".into(), "".into()]).is_err());
        assert!(launch_request(&["--verify-heartbeat".into(), "invalid\0event".into()]).is_err());
        assert!(
            launch_request(&["--verify-heartbeat".into(), "event".into(), "extra".into()]).is_err()
        );
    }
}
