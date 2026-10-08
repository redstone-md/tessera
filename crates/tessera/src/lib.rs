// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Application composition shared by the desktop launcher and developer CLI.

#![forbid(unsafe_code)]

#[cfg(any(windows, test))]
mod audio_provider;
mod panel;
#[cfg(any(windows, test))]
mod settings;

/// Opens the native alpha panel on the UI main thread.
pub fn run_panel() -> Result<(), Box<dyn std::error::Error>> {
    panel::run()
}

/// Starts a supervised desktop session, or opens its native UI with the owner's heartbeat.
pub fn run_desktop(heartbeat: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    panel::run_desktop(heartbeat)
}

/// Opens the installer's diagnostic UI without taskbar/appbar/Winlogon mutation.
pub fn run_desktop_diagnostic(heartbeat: &str) -> Result<(), Box<dyn std::error::Error>> {
    panel::run_desktop_diagnostic(heartbeat)
}
