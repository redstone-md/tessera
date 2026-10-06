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
mod dock;
mod dto;
mod icons;
mod projection;
mod sanitize;
mod state;

// Slint owns its generated code; handwritten presentation remains safe Rust.
#[allow(unsafe_code)]
mod generated {
    slint::include_modules!();
}
use generated::Panel;

pub use dto::{
    DockContext, DockEdge, MAX_PINS, PanelApplication, PixelIcon, RunOptions, SurfaceMode,
    SystemAction,
};

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

/// Maps a [`DockEdge`] to the dock combo box row index (Bottom, Top, Left, Right).
pub(crate) fn dock_edge_to_index(edge: DockEdge) -> i32 {
    match edge {
        DockEdge::Bottom => 0,
        DockEdge::Top => 1,
        DockEdge::Left => 2,
        DockEdge::Right => 3,
    }
}

/// Maps the UI's system-action combo index (0..=2) to the dispatched action.
pub(crate) fn system_action_from_index(index: i32) -> SystemAction {
    match index {
        1 => SystemAction::OpenTaskManager,
        2 => SystemAction::RestoreExplorer,
        _ => SystemAction::OpenFileManager,
    }
}

/// Inverse of [`dock_edge_to_index`]; out-of-range values fall back to [`DockEdge::Bottom`].
pub(crate) fn dock_edge_from_index(index: i32) -> DockEdge {
    match index {
        1 => DockEdge::Top,
        2 => DockEdge::Left,
        3 => DockEdge::Right,
        _ => DockEdge::Bottom,
    }
}

/// Starts the UI-thread heartbeat, if the host supplied one: one queued
/// invocation right after the surface shows, then a two-second repeated Slint
/// timer owned by the caller. The timer is retained until `run` returns so the
/// watchdog never fires into a dropped loop; it never observes the desktop.
pub(crate) fn start_heartbeat(run_options: &RunOptions) -> Option<slint::Timer> {
    let heartbeat = run_options.heartbeat.clone()?;
    // Queued first tick: it runs once the event loop is spinning, right
    // after the surface has been shown.
    let initial = std::sync::Arc::clone(&heartbeat);
    let _ = slint::invoke_from_event_loop(move || initial());
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(2),
        move || heartbeat(),
    );
    Some(timer)
}

/// Panel appearance preferences, persisted by the host on explicit save.
///
/// `dock_edge` and `pins` ride along in the same persisted record: the dock
/// edge follows the appearance Save button, while pin changes are persisted
/// immediately using the last saved appearance values (see the controller).
/// Pins are bounded (`MAX_PINS`) and length-limited by `with_dock`, so this
/// small cloneable value stays cheap to move across the trait boundary.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PanelPreferences {
    theme: Theme,
    compact: bool,
    dock_edge: DockEdge,
    pins: Vec<String>,
}

impl PanelPreferences {
    /// Builds preferences from an already-validated theme and density choice.
    pub fn new(theme: Theme, compact: bool) -> Self {
        Self {
            theme,
            compact,
            dock_edge: DockEdge::default(),
            pins: Vec::new(),
        }
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
    icon: Option<PixelIcon>,
}

impl PanelWindow {
    /// Builds one observed window row from already-collected facts.
    pub fn new(key: String, title: String, minimized: bool) -> Self {
        Self {
            key,
            title,
            minimized,
            icon: None,
        }
    }

    /// Builder for the window's class icon, when the host resolved one
    /// without blocking or cross-process messaging.
    pub fn with_icon(mut self, icon: Option<PixelIcon>) -> Self {
        self.icon = icon;
        self
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

    /// Icon pixels when the host resolved one for this window.
    pub fn icon(&self) -> Option<&PixelIcon> {
        self.icon.as_ref()
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
    applications: Option<Vec<PanelApplication>>,
    dock_context: Option<DockContext>,
}

impl PanelSnapshot {
    /// Builds a snapshot view from already-collected observation facts.
    pub fn new(monitor_count: usize, windows: Vec<PanelWindow>, warning_count: usize) -> Self {
        Self {
            monitor_count,
            windows,
            warning_count,
            applications: None,
            dock_context: None,
        }
    }

    /// Builder for the capped launchable-application catalog the host mapped
    /// from its native source. Absent means the source exposes no catalog.
    pub fn with_applications(mut self, applications: Vec<PanelApplication>) -> Self {
        self.applications = Some(applications);
        self
    }

    /// Builder for the primary-monitor work-area viewport the dock needs.
    pub fn with_dock_context(mut self, dock_context: DockContext) -> Self {
        self.dock_context = Some(dock_context);
        self
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

    /// Capped application catalog carried by this observation, if any.
    pub fn applications(&self) -> Option<&[PanelApplication]> {
        self.applications.as_deref()
    }

    /// Primary-monitor work-area viewport carried by this observation, if any.
    pub fn dock_context(&self) -> Option<DockContext> {
        self.dock_context
    }
}

/// Adapter the panel or dock drives: observation, explicit activation,
/// trusted launch, system actions, preference save, and change subscription.
///
/// `observe` is called on a worker thread and must not mutate the desktop.
/// `activate` and `launch` are called directly on the UI input thread (for
/// activation this preserves foreground eligibility) and only for an explicit
/// user action. `system_action` likewise runs on the UI thread for a click.
/// `save_preferences` is called only when the user presses Save or toggles a
/// pin (see [`DesktopHost::save_preferences`]).
pub trait DesktopHost: Send + Sync + 'static {
    /// Captures one observation pass, including the capped application
    /// catalog and dock viewport when the source provides them.
    fn observe(&self) -> Result<PanelSnapshot, String>;
    /// Activates the window behind `key`; may restore a minimized window only
    /// for this explicit user action.
    fn activate(&self, key: &str) -> Result<(), String>;
    /// Launches the application behind `key` — an identity the host itself
    /// enumerated. The presentation only ever forwards keys currently shown.
    fn launch(&self, key: &str) -> Result<(), String>;
    /// Performs one explicit desktop action on the host's own authority.
    fn system_action(&self, action: SystemAction) -> Result<(), String>;
    /// Persists preferences; invoked on explicit Save and on pin changes
    /// (which carry the last saved appearance values, never the live preview).
    fn save_preferences(&self, preferences: &PanelPreferences) -> Result<(), String>;

    /// Subscribes to desktop-change notifications (coalesced by the UI into
    /// refreshes). Called once at startup on the UI thread; the returned
    /// guard owns the native watcher and stopping it stops the notifications.
    /// Returning `None` is a valid polling-free "no watcher available" answer;
    /// an error leaves manual refresh as the only update path. The default
    /// implementation reports no watcher and never fails.
    fn subscribe(
        &self,
        _callback: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Option<Box<dyn Send>>, String> {
        Ok(None)
    }
}

/// Opens the requested surface and runs the UI event loop until it closes.
///
/// The host's `observe` may be called again only after the previous call has
/// returned (single-flight). Desktop-change notifications that arrive while a
/// worker runs are coalesced into exactly one queued follow-up refresh;
/// concurrent manual refresh requests are dropped, not queued.
/// `preferences` seeds the initial live preview; nothing is saved without an
/// explicit Save. `startup_notice`, when present, is shown as a plain notice
/// at the top of the launcher panel. One desktop-change subscription is
/// created; a subscription failure just leaves manual refresh as the update
/// path. In dock mode the loop runs until the strip's explicit Exit button.
///
/// This must be called on the UI main thread.
pub fn run(
    host: impl DesktopHost,
    preferences: PanelPreferences,
    startup_notice: Option<String>,
    run_options: RunOptions,
) -> Result<(), slint::PlatformError> {
    controller::run(
        std::sync::Arc::new(host),
        preferences,
        startup_notice,
        run_options,
    )
}
