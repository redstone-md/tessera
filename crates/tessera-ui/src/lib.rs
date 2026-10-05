// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native read-only desktop panel for Tessera.
//!
//! The panel is an ordinary, closable utility window. It shows the window
//! titles observed by an adapter supplied by the caller, monitor and warning
//! counts, and nothing else. It manages only its own window: no hooks,
//! foreign-window mutation, or polling.
//!
//! [`PanelSnapshot`] is a portable UI view of one observation. [`run`] opens
//! the panel and blocks on the UI thread until the window closes.

#![deny(unsafe_code)]

mod controller;
mod sanitize;

use slint::ComponentHandle;

// Slint owns its generated code; handwritten presentation remains safe Rust.
#[allow(unsafe_code)]
mod generated {
    slint::include_modules!();
}
use generated::Panel;

/// A portable UI view of one desktop observation.
///
/// This is not an OS desktop snapshot; it carries only what the panel renders:
/// a monitor count, raw captions (cleaned at presentation), and a warning count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelSnapshot {
    monitor_count: usize,
    window_titles: Vec<String>,
    warning_count: usize,
}

impl PanelSnapshot {
    /// Builds a snapshot view from already-collected observation facts.
    pub fn new(monitor_count: usize, window_titles: Vec<String>, warning_count: usize) -> Self {
        Self {
            monitor_count,
            window_titles,
            warning_count,
        }
    }

    /// Number of monitors observed.
    pub fn monitor_count(&self) -> usize {
        self.monitor_count
    }

    /// Raw observed captions; display normalization belongs to presentation.
    pub fn window_titles(&self) -> &[String] {
        &self.window_titles
    }

    /// Number of non-fatal observation warnings reported by the source.
    pub fn warning_count(&self) -> usize {
        self.warning_count
    }
}

/// Opens the panel window and runs the UI event loop until it closes.
///
/// `source` captures one observation pass and must be `Send + Sync`; the
/// controller invokes it on a worker thread so the UI never blocks on the
/// desktop, both for the initial load and for each explicit Refresh. The
/// source may be called again only after the previous call has returned
/// (single-flight; concurrent refresh requests are dropped, not queued).
///
/// This must be called on the UI main thread.
pub fn run(
    source: impl Fn() -> Result<PanelSnapshot, String> + Send + Sync + 'static,
) -> Result<(), slint::PlatformError> {
    let panel = Panel::new()?;
    let controller = controller::PanelController::new(&panel, source);
    let _ = controller.refresh();
    panel.run()
}
