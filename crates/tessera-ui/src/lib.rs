// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native alpha window-switcher presentation for Tessera.
//!
//! The panel is an ordinary, closable utility window. It shows windows and
//! counts supplied by a [`DesktopHost`]. Observation is read-only; activation
//! and preference saves are explicit host commands. No hooks or polling.
//!
//! [`PanelSnapshot`] is a portable UI view of one observation. [`run`] opens
//! the panel and blocks on the UI thread until the window closes. Observation
//! runs on a worker thread; activating a window and saving preferences are
//! explicit user actions performed directly on the UI thread.

#![deny(unsafe_code)]

mod controller;
mod projection;
mod sanitize;

use slint::ComponentHandle;

// Slint owns its generated code; handwritten presentation remains safe Rust.
#[allow(unsafe_code)]
mod generated {
    slint::include_modules!();
}
use generated::Panel;

/// Preferred color scheme for the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// Follow the desktop setting.
    #[default]
    System,
    Light,
    Dark,
}

/// Maps a [`Theme`] to the panel combo box row index (System, Light, Dark).
pub(crate) fn theme_to_index(theme: Theme) -> i32 {
    match theme {
        Theme::System => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
    }
}

/// Inverse of [`theme_to_index`]; out-of-range values fall back to [`Theme::System`].
pub(crate) fn theme_from_index(index: i32) -> Theme {
    match index {
        1 => Theme::Light,
        2 => Theme::Dark,
        _ => Theme::System,
    }
}

/// Panel appearance preferences, persisted by the host on explicit save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PanelPreferences {
    theme: Theme,
    compact: bool,
}

impl PanelPreferences {
    /// Builds preferences from an already-validated theme and density choice.
    pub fn new(theme: Theme, compact: bool) -> Self {
        Self { theme, compact }
    }

    /// Selected color scheme.
    pub fn theme(&self) -> Theme {
        self.theme
    }

    /// Whether the panel uses the compact layout density.
    pub fn compact(&self) -> bool {
        self.compact
    }
}

/// One observed top-level window as shown to the user.
///
/// `key` is an opaque, host-chosen identifier. It is the only value ever sent
/// back to [`DesktopHost::activate`]; it must not be derived from a display
/// index or the (possibly truncated) title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelWindow {
    key: String,
    title: String,
    minimized: bool,
}

impl PanelWindow {
    /// Builds one observed window row from already-collected facts.
    pub fn new(key: String, title: String, minimized: bool) -> Self {
        Self {
            key,
            title,
            minimized,
        }
    }

    /// Opaque activation key; stable within one snapshot, never persisted.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Raw observed caption; display normalization belongs to presentation.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Whether the window is currently minimized.
    pub fn minimized(&self) -> bool {
        self.minimized
    }
}

/// A portable UI view of one desktop observation.
///
/// This is not an OS desktop snapshot; it carries only what the panel renders:
/// a monitor count, observed windows, and a warning count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelSnapshot {
    monitor_count: usize,
    windows: Vec<PanelWindow>,
    warning_count: usize,
}

impl PanelSnapshot {
    /// Builds a snapshot view from already-collected observation facts.
    pub fn new(monitor_count: usize, windows: Vec<PanelWindow>, warning_count: usize) -> Self {
        Self {
            monitor_count,
            windows,
            warning_count,
        }
    }

    /// Number of monitors observed.
    pub fn monitor_count(&self) -> usize {
        self.monitor_count
    }

    /// Observed windows in stable observation order.
    pub fn windows(&self) -> &[PanelWindow] {
        &self.windows
    }

    /// Number of non-fatal observation warnings reported by the source.
    pub fn warning_count(&self) -> usize {
        self.warning_count
    }
}

/// Adapter the panel drives: observation, explicit activation, preference save.
///
/// `observe` is called on a worker thread and must not mutate the desktop.
/// `activate` is called directly on the UI input thread (foreground
/// eligibility) and only for an explicit user action. `save_preferences` is
/// called only when the user presses Save.
pub trait DesktopHost: Send + Sync + 'static {
    /// Captures one observation pass.
    fn observe(&self) -> Result<PanelSnapshot, String>;
    /// Activates the window behind `key`; may restore a minimized window only
    /// for this explicit user action.
    fn activate(&self, key: &str) -> Result<(), String>;
    /// Persists preferences; invoked only on explicit Save.
    fn save_preferences(&self, preferences: &PanelPreferences) -> Result<(), String>;
}

/// Opens the panel window and runs the UI event loop until it closes.
///
/// The host's `observe` may be called again only after the previous call has
/// returned (single-flight; concurrent refresh requests are dropped, not
/// queued). `preferences` seeds the initial live preview; nothing is saved
/// without an explicit Save. `startup_notice`, when present, is shown as a
/// plain notice at the top of the panel.
///
/// This must be called on the UI main thread.
pub fn run(
    host: impl DesktopHost,
    preferences: PanelPreferences,
    startup_notice: Option<String>,
) -> Result<(), slint::PlatformError> {
    let panel = Panel::new()?;
    panel.set_theme_index(theme_to_index(preferences.theme()));
    panel.set_compact(preferences.compact());
    panel.set_version(env!("CARGO_PKG_VERSION").into());
    panel.set_startup_notice(
        startup_notice
            .map(|notice| sanitize::bounded_text(&notice, 200))
            .unwrap_or_default()
            .into(),
    );
    let controller = controller::PanelController::new(&panel, std::sync::Arc::new(host));
    controller.apply_filter();
    let _ = controller.refresh();
    panel.run()
}
