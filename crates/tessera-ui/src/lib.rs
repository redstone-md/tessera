// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native alpha window-switcher presentation for Tessera.
//!
//! The panel is an ordinary, closable utility window. It shows windows and
//! counts supplied by a [`DesktopHost`]. Observation is read-only; activation
//! and preference saves are explicit host commands. Independent native capability
//! notifications do not add desktop-observation polling.
//!
//! [`PanelSnapshot`] is a portable UI view of one observation. [`run`] opens
//! the panel and blocks on the UI thread until the window closes. Observation
//! runs on a worker thread; activating a window and saving preferences are
//! explicit user actions performed directly on the UI thread.

#![deny(unsafe_code)]

pub(crate) mod bluetooth;
#[cfg(test)]
mod bluetooth_render_tests;
pub(crate) mod calendar;
#[cfg(test)]
mod calendar_render_tests;
pub(crate) mod context_menu;
pub(crate) mod controller;
pub(crate) mod dock;
pub(crate) mod dock_media;
#[cfg(test)]
mod dock_media_render_tests;
pub(crate) mod dock_utilities;
#[cfg(test)]
mod dock_utilities_render_tests;
pub(crate) mod dto;
pub(crate) mod icons;
pub(crate) mod image_mask;
pub(crate) mod input_language;
#[cfg(test)]
mod input_language_render_tests;
pub(crate) mod launcher;
pub(crate) mod motion;
#[cfg(test)]
mod native_typography_oracle;
pub(crate) mod network_menu;
#[cfg(test)]
mod network_menu_render_tests;
pub(crate) mod popup_placement;
pub(crate) mod power_menu;
#[cfg(test)]
mod power_menu_render_tests;
pub(crate) mod projection;
pub(crate) mod quick_settings;
pub(crate) mod recycle_bin;
#[cfg(test)]
mod recycle_bin_render_tests;
pub(crate) mod shortcuts;
#[cfg(test)]
mod shortcuts_render_tests;
pub(crate) mod user_menu;
pub(crate) mod visibility;
// Renderer-backed tests: test-only (they need the software renderer and the
// testing backend's element introspection; see build.rs debug info).
#[cfg(all(test, target_os = "linux"))]
mod gl_render_tests;
#[cfg(test)]
mod quick_settings_render_tests;
#[cfg(test)]
mod render_tests;
pub(crate) mod sanitize;
pub(crate) mod state;
pub(crate) mod theme;
#[cfg(test)]
mod tile_input_tests;
pub(crate) mod tooltip;
pub(crate) mod transient_window;
#[cfg(test)]
mod user_menu_render_tests;

// Slint owns its generated code; handwritten presentation remains safe Rust.
#[allow(unsafe_code)]
mod generated {
    slint::include_modules!();
}
use generated::Panel;

pub use calendar::preferences::{GeneralPreferences, StartOfWeek};
pub use dto::{
    DockContext, DockEdge, MAX_PINS, PanelApplication, PixelIcon, RunOptions, ShellIdentity,
    SurfaceKind, SurfaceMode, SystemAction, WindowAction,
};
pub use launcher::{LauncherDisplayMode, LauncherPreferences};
pub use tessera_core::SourceSeed;

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

/// Maps the UI's system-action combo index (0..=2) to the dispatched action.
pub(crate) fn system_action_from_index(index: i32) -> SystemAction {
    match index {
        1 => SystemAction::OpenTaskManager,
        2 => SystemAction::RestoreExplorer,
        _ => SystemAction::OpenFileManager,
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

/// Inverse of [`dock_edge_to_index`]; out-of-range values fall back to [`DockEdge::Bottom`].
pub(crate) fn dock_edge_from_index(index: i32) -> DockEdge {
    match index {
        1 => DockEdge::Top,
        2 => DockEdge::Left,
        3 => DockEdge::Right,
        _ => DockEdge::Bottom,
    }
}

/// Retained heartbeat control: pulses only once the surface startup is
/// provably ready (valid real geometry, shown windows, attached leases), then
/// keeps the two-second repeated timer alive until `run` returns so the
/// watchdog never fires into a dropped loop. It never observes the desktop.
pub(crate) struct Heartbeat {
    closure: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    timer: slint::Timer,
}

impl Heartbeat {
    /// Builds an unarmed heartbeat; without [`Heartbeat::arm`] nothing is
    /// ever pulsed (a failed startup exits cleanly and silently).
    pub(crate) fn unarmed(run_options: &RunOptions) -> Self {
        Self {
            closure: run_options.heartbeat.clone(),
            timer: slint::Timer::default(),
        }
    }

    /// Queues one heartbeat tick right away (it drains as soon as the event
    /// loop spins, i.e. directly after the surfaces are shown) and starts the
    /// two-second repeated timer. Idempotent: a second call does nothing.
    pub(crate) fn arm(&mut self) {
        if let Some(heartbeat) = self.closure.take() {
            let initial = std::sync::Arc::clone(&heartbeat);
            // Queued first tick: runs once the event loop is spinning, right
            // after the surfaces have been shown.
            let _ = slint::invoke_from_event_loop(move || initial());
            self.timer.start(
                slint::TimerMode::Repeated,
                std::time::Duration::from_secs(2),
                move || heartbeat(),
            );
        }
    }
}

/// Starts the clock-only timer: at most one tick per minute, each tick
/// refreshing only the clock text through [`DesktopHost::clock_text`] (never
/// windows, foreground, or accent). Retained until `run` returns.
pub(crate) fn start_clock_timer(
    core: &std::sync::Arc<crate::state::SurfaceCore>,
    sink: std::sync::Arc<dyn Fn(String) + Send + Sync>,
) -> slint::Timer {
    let core = std::sync::Arc::clone(core);
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(60),
        move || {
            if let Some(clock) = core.refresh_clock() {
                sink(clock);
            }
        },
    );
    timer
}

/// Complete panel preferences, persisted by the host on explicit user actions.
///
/// Dock pins and ordered launcher favorites are independent collections.
/// Immediate pin/favorite/display-mode changes carry the last saved appearance,
/// never a live preview. Dock pins are bounded by [`MAX_PINS`]; favorite builders
/// validate identities without truncating the collection. The host enforces
/// the aggregate storage budget on save.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PanelPreferences {
    theme: Theme,
    source_seed: SourceSeed,
    compact: bool,
    dock_edge: DockEdge,
    pins: Vec<String>,
    launcher: LauncherPreferences,
    general: GeneralPreferences,
    media_enabled: bool,
    shortcuts: tessera_system::shortcuts::ShortcutConfig,
}

impl PanelPreferences {
    /// Builds preferences from an already-validated theme and density choice.
    pub fn new(theme: Theme, compact: bool) -> Self {
        Self {
            theme,
            source_seed: SourceSeed::default(),
            compact,
            dock_edge: DockEdge::default(),
            pins: Vec::new(),
            launcher: LauncherPreferences::default(),
            general: GeneralPreferences::default(),
            media_enabled: false,
            shortcuts: tessera_system::shortcuts::ShortcutConfig::default(),
        }
    }

    /// Changes appearance and dock placement without changing the launcher group or pins.
    pub fn with_appearance(mut self, theme: Theme, compact: bool, edge: DockEdge) -> Self {
        self.theme = theme;
        self.compact = compact;
        self.dock_edge = edge;
        self
    }

    /// Applied source intent, independent of generated light/dark presentation paint.
    pub fn source_seed(&self) -> SourceSeed {
        self.source_seed
    }

    /// Changes source intent without persisting generated color roles.
    pub fn with_source_seed(mut self, seed: SourceSeed) -> Self {
        self.source_seed = seed;
        self
    }

    /// Complete saved applications-menu choices.
    pub fn launcher(&self) -> &LauncherPreferences {
        &self.launcher
    }

    /// Changes applications-menu presentation without changing any other saved choice.
    pub fn with_launcher_display_mode(mut self, mode: LauncherDisplayMode) -> Self {
        self.launcher = self.launcher.with_display_mode(mode);
        self
    }

    /// Applied calendar policy, independent of the settings draft.
    pub fn general(&self) -> GeneralPreferences {
        self.general
    }

    pub fn with_general(mut self, general: GeneralPreferences) -> Self {
        self.general = general;
        self
    }

    /// Optional Dock media starts disabled and is adopted only after a save.
    pub fn media_enabled(&self) -> bool {
        self.media_enabled
    }

    pub fn with_media_enabled(mut self, enabled: bool) -> Self {
        self.media_enabled = enabled;
        self
    }

    /// Complete typed global bindings. Display labels never authorize native input.
    pub fn shortcuts(&self) -> &tessera_system::shortcuts::ShortcutConfig {
        &self.shortcuts
    }

    /// Changes global bindings without changing any other saved preference group.
    pub fn with_shortcuts(mut self, shortcuts: tessera_system::shortcuts::ShortcutConfig) -> Self {
        self.shortcuts = shortcuts;
        self
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
    visibility: Option<(Vec<tessera_system::visibility::VisibilityWindow>, bool)>,
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
            visibility: None,
        }
    }

    /// Builder for the capped launchable-application catalog the host mapped
    /// from its native source. Absent means the source exposes no catalog.
    pub fn with_applications(mut self, applications: Vec<PanelApplication>) -> Self {
        self.applications = Some(applications);
        self
    }

    /// Builder for the primary-monitor bounds viewport the dock needs.
    pub fn with_dock_context(mut self, dock_context: DockContext) -> Self {
        self.dock_context = Some(dock_context);
        self
    }

    /// Complete native eligibility/geometry facts, kept separate from UI row caps.
    /// Missing evidence is unknown, never an empty/no-overlap observation.
    pub fn with_visibility_windows(
        mut self,
        windows: Vec<tessera_system::visibility::VisibilityWindow>,
        foreground_interactable: bool,
    ) -> Self {
        self.visibility = Some((windows, foreground_interactable));
        self
    }

    pub fn visibility_facts(
        &self,
    ) -> Option<(&[tessera_system::visibility::VisibilityWindow], bool)> {
        self.visibility
            .as_ref()
            .map(|(windows, foreground)| (windows.as_slice(), *foreground))
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

    /// Primary-monitor bounds viewport carried by this observation, if any.
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
/// `save_preferences` is called only when the user presses Save or changes a
/// dock pin, launcher favorite, or display mode (see [`DesktopHost::save_preferences`]).
pub trait DesktopHost: Send + Sync + 'static {
    /// Captures one observation pass, including the capped application
    /// catalog and dock viewport when the source provides them.
    fn observe(&self) -> Result<PanelSnapshot, String>;
    /// Activates the window behind `key`; may restore a minimized window only
    /// for this explicit user action.
    fn activate(&self, key: &str) -> Result<(), String>;
    /// Performs one explicit command on an eligible, currently displayed
    /// observed window. The platform revalidates its live identity; an
    /// accepted asynchronous request is not proof that it completed.
    fn window_action(&self, _key: &str, _action: WindowAction) -> Result<(), String> {
        Err("This host does not support window commands".into())
    }
    /// Launches the application behind `key` — an identity the host itself
    /// enumerated. The presentation only ever forwards keys currently shown.
    fn launch(&self, key: &str) -> Result<(), String>;
    /// Performs one explicit desktop action on the host's own authority.
    fn system_action(&self, action: SystemAction) -> Result<(), String>;
    /// Persists the complete record on explicit Save or pin/favorite/display-mode changes.
    /// Immediate launcher and pin changes carry last-saved appearance, never preview.
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

    /// Returns an optional independent audio capability, initialized on demand.
    /// This hook and the capability's requests return promptly; hardware I/O
    /// belongs to its native worker, never observation or the UI input thread.
    fn audio_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::audio::AudioHost>>,
        tessera_system::audio::AudioError,
    > {
        Ok(None)
    }

    /// Independent known-folder capability. Discovery and shell dispatch run
    /// on its native worker, not the UI or desktop observation path.
    fn folder_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::folders::FolderHost>>,
        tessera_system::folders::FolderError,
    > {
        Ok(None)
    }

    /// Current-user identity and closed trusted actions, acquired only on popup intent.
    fn profile_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::profile::ProfileHost>>,
        tessera_system::profile::ProfileError,
    > {
        Ok(None)
    }

    /// Independent global bindings, acquired only by admitted ordinary startup.
    /// Diagnostics disable this entire path, including factory and subscription.
    fn shortcuts_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::shortcuts::ShortcutHost>>,
        tessera_system::shortcuts::ShortcutError,
    > {
        Ok(None)
    }

    /// Supplies genuine local-date/locale reads independently of application
    /// observation. Native work runs on the capability's bounded worker.
    fn calendar_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::calendar::CalendarHost>>,
        tessera_system::calendar::CalendarError,
    > {
        Ok(None)
    }

    /// Independent reserved Dock effects; native Shell calls run only after
    /// explicit user intent on the capability's bounded worker.
    fn dock_utilities_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::dock_utilities::DockUtilitiesHost>>,
        tessera_system::dock_utilities::DockUtilityError,
    > {
        Ok(None)
    }

    /// Genuine Recycle Bin reads, fixed opening and scoped changes, separate
    /// from application observation and destructive operation authority.
    fn recycle_bin_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::recycle_bin::RecycleBinHost>>,
        tessera_system::dock_utilities::DockUtilityError,
    > {
        Ok(None)
    }

    /// Explicit native-confirmed mutation with its own dialog-owner lifetime.
    /// Acquisition is lazy user intent, never part of read/open/watch startup.
    fn recycle_bin_mutation_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::recycle_bin_mutation::RecycleBinMutationHost>>,
        tessera_system::dock_utilities::DockUtilityError,
    > {
        Ok(None)
    }

    /// Fresh independent display reads for desktop-spanning presentation.
    /// Query infrastructure and DPI/text-scale work belong to its worker.
    fn display_context_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::display_context::DisplayContextHost>>,
        tessera_system::display_context::DisplayContextError,
    > {
        Ok(None)
    }

    /// Explicit typed session/power capability, acquired only on user intent.
    /// Acceptance means native initiation, never observed session state.
    fn power_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::power::PowerHost>>,
        tessera_system::power::PowerError,
    > {
        Ok(None)
    }

    /// Read-only known pending-update hints, independent of power mutation.
    /// Query work and registry resources belong to the capability's worker.
    fn power_updates_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::power_updates::PowerUpdatesHost>>,
        tessera_system::power_updates::PowerUpdatesError,
    > {
        Ok(None)
    }

    /// Independent read-only network capability, acquired on popup intent.
    fn network_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::network::NetworkHost>>,
        tessera_system::network::NetworkError,
    > {
        Ok(None)
    }

    /// Independent read-only Bluetooth capability, acquired on popup intent.
    fn bluetooth_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::bluetooth::BluetoothHost>>,
        tessera_system::bluetooth::BluetoothError,
    > {
        Ok(None)
    }

    /// Enabled language profiles and explicitly selected native activation.
    fn input_language_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::input_language::InputLanguageHost>>,
        tessera_system::input_language::InputLanguageError,
    > {
        Ok(None)
    }

    /// Current native media sessions and explicit transport requests.
    fn media_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::media::MediaHost>>,
        tessera_system::media::MediaError,
    > {
        Ok(None)
    }

    /// Passive physical pointer hints; only the production desktop host opts in.
    fn pointer_host(
        &self,
    ) -> Result<
        Option<std::sync::Arc<dyn tessera_system::visibility::PointerHost>>,
        tessera_system::visibility::PointerWatchError,
    > {
        Ok(None)
    }

    /// Refreshes only the clock text. Called on the UI thread by the
    /// low-frequency clock timer (at most once per minute); it must never
    /// observe windows, foreground state, or anything else — one cheap
    /// display-only query, not desktop polling.
    fn clock_text(&self) -> Result<String, String> {
        Ok(String::new())
    }

    /// Reads the current system/user permission for optional UI motion.
    /// Called on the UI thread for a presentation, never for desktop polling.
    /// Unknown/unsupported hosts conservatively skip animations; production
    /// adapters must also skip them when the system requests reduced motion.
    fn ui_animations_enabled(&self) -> bool {
        false
    }

    /// Subscribes only to effective motion-permission changes. Delivery may
    /// originate on another thread; it must never trigger desktop observation.
    /// The returned guard owns the registration. `None` means unavailable;
    /// presentations still query [`DesktopHost::ui_animations_enabled`].
    fn subscribe_ui_motion(
        &self,
        _callback: std::sync::Arc<dyn Fn(bool) + Send + Sync>,
    ) -> Result<Option<Box<dyn Send>>, String> {
        Ok(None)
    }

    /// Attaches one shown UI window to its native shell surface.
    ///
    /// Called on the main UI thread after the window is shown and native
    /// window creation completed — for the bars once real monitor geometry
    /// is applied (never with a provisional/default rect), for the launcher
    /// on each show. The default returns no lease and performs no native
    /// action (the developer Panel mode and diagnostics hosts); `None` is
    /// a success answer for such hosts. A production adapter converts the
    /// window handle, validates it as an owned live surface, attaches it
    /// (tool-window styling: the bars no-activate, the launcher activatable;
    /// toolbar work-area reservation when available), and returns a
    /// lease the UI retains until the attachment must end (before hide,
    /// before a changed-geometry re-reservation, before HWND close). An
    /// `Err` terminates the UI session so its supervisor can restore Explorer.
    fn configure_surface(
        &self,
        _kind: SurfaceKind,
        _window: &slint::Window,
    ) -> Result<Option<Box<dyn std::any::Any>>, String> {
        Ok(None)
    }

    /// Requests foreground for an explicitly user-opened launcher or popup.
    ///
    /// Windows may refuse activation; the adapter must respect that decision
    /// without synthetic input or focus-policy workarounds. Bars never call it.
    fn request_ui_focus(&self, _window: &slint::Window) -> Result<(), String> {
        Ok(())
    }

    /// Bounded genuine identity sample on successful observation application
    /// (UI thread). May read the foreground identity, but must not rescan the
    /// desktop/catalog or mutate system state. The minute timer uses only
    /// [`DesktopHost::clock_text`], never this method.
    fn shell_identity(&self) -> Result<ShellIdentity, String> {
        Ok(ShellIdentity::default())
    }
}

/// Opens the requested surface and runs the UI event loop until it closes.
///
/// The host's `observe` may be called again only after the previous call has
/// returned (single-flight). Desktop-change notifications that arrive while a
/// worker runs are coalesced into exactly one queued follow-up refresh;
/// concurrent manual refresh requests are dropped, not queued.
/// `preferences` seeds the initial live preview and complete applied record;
/// nothing is saved without explicit Save or a pin/favorite/display-mode action.
/// `startup_notice`, when present, is shown as a plain notice
/// at the top of the launcher panel. One desktop-change subscription is
/// created; a subscription failure just leaves manual refresh as the update
/// path.
///
/// In dock mode the loop runs until explicit Exit or a bar close request.
/// The heartbeat pulses only once startup is provably ready — a successful
/// initial observation with real monitor bounds and both bar leases attached.
/// Setup failure exits for immediate Explorer restoration; geometry that
/// never arrives reaches the supervisor's startup timeout. The readiness
/// pulse routes through the panel's `readiness-pulse` callback (the heartbeat
/// is UI-thread-only; the observation route is Send).
///
/// This must be called on the UI main thread.
pub fn run(
    host: impl DesktopHost,
    preferences: PanelPreferences,
    startup_notice: Option<String>,
    run_options: RunOptions,
) -> Result<(), slint::PlatformError> {
    #[cfg(windows)]
    if run_options.surface == SurfaceMode::Dock {
        // The hook runs before native creation; titles are not available yet.
        // User-opened windows request activation explicitly through the host.
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .with_winit_window_attributes_hook(nonactivating_window_attributes)
            .select()?;
    }
    controller::run(
        std::sync::Arc::new(host),
        preferences,
        startup_notice,
        run_options,
    )
}

#[cfg(any(windows, test))]
fn nonactivating_window_attributes(
    attributes: slint::winit_030::winit::window::WindowAttributes,
) -> slint::winit_030::winit::window::WindowAttributes {
    attributes.with_active(false)
}

#[cfg(test)]
mod activation_tests {
    #[test]
    fn first_native_show_is_nonactivating_without_changing_surface_options() {
        let attributes = slint::winit_030::winit::window::WindowAttributes::default()
            .with_active(true)
            .with_visible(false)
            .with_transparent(true);
        let attributes = super::nonactivating_window_attributes(attributes);
        assert!(!attributes.active);
        assert!(!attributes.visible);
        assert!(attributes.transparent);
    }
}
