// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Application composition shared by the desktop launcher and developer CLI.

#![forbid(unsafe_code)]

mod panel;
#[cfg(any(windows, test))]
mod settings;

/// Opens the native alpha panel on the UI main thread.
pub fn run_panel() -> Result<(), Box<dyn std::error::Error>> {
    panel::run()
}
