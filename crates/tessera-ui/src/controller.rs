// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::generated::{AppRow, Dock, DockApp, DockStatus, DockWindow, Palette, Row};
use crate::projection::{AppProjection, PanelProjection, RowProjection};
use crate::state::{Routes, SurfaceCore};
use crate::{
    DesktopHost, Panel, PanelPreferences, PanelSnapshot, RunOptions, SystemAction, sanitize,
    system_action_from_index,
};

type Host = dyn DesktopHost;

/// One controller drives both surfaces.
///
/// In panel mode only the panel window exists. In dock mode a slim icon strip
/// (`Dock`) is the always-present surface and the panel is the launcher and
/// settings window, opened from the strip's Applications button and hidden —
/// not closed — by its close button, so the dock keeps running.
///
/// The panel's `refreshing`/`stale`/`has-snapshot` properties remain the
/// single source of truth for both surfaces; the dock status is a mirror kept
/// in sync by [`PanelController::render`]. There is exactly one observation
/// worker, one notification bus, and one retained snapshot — never two
/// independent observers.
///
/// Retained snapshots, the catalog, and pins live in the shared
/// [`SurfaceCore`]; the controller owns only the two weak handles.
#[derive(Clone)]
pub(crate) struct PanelController {
    panel: slint::Weak<Panel>,
    dock: Option<slint::Weak<Dock>>,
    core: Arc<SurfaceCore>,
    icon_cache: Arc<parking_lot::Mutex<crate::icons::IconCache>>,
}

impl PanelController {
    pub(crate) fn new(panel: &Panel, core: Arc<SurfaceCore>) -> Self {
        let controller = Self {
            panel: panel.as_weak(),
            dock: None,
            core,
            icon_cache: Arc::default(),
        };
        controller.wire_panel(panel);
        controller
    }

    /// Dock mode: the same controller, plus a weak strip handle.
    pub(crate) fn new_with_dock(panel: &Panel, dock: &Dock, core: Arc<SurfaceCore>) -> Self {
        let controller = Self {
            panel: panel.as_weak(),
            dock: Some(dock.as_weak()),
            core,
            icon_cache: Arc::default(),
        };
        controller.wire_panel(panel);
        controller.wire_dock(dock);
        controller
    }

    fn wire_panel(&self, panel: &Panel) {
        let weak = self.clone();
        panel.on_refresh_requested(move || {
            let _ = weak.refresh();
        });
        let weak = self.clone();
        panel.on_activate_requested(move |key| weak.activate(&key));
        let weak = self.clone();
        panel.on_filter_requested(move || weak.apply_filter());
        let weak = self.clone();
        panel.on_save_preferences_requested(move || {
            weak.save_preferences();
        });
        let weak = self.clone();
        panel.on_launch_requested(move |key| weak.launch(&key));
        let weak = self.clone();
        panel.on_pin_toggle_requested(move |key, pinned| weak.toggle_pin(&key, pinned));
        let weak = self.clone();
        panel.on_system_action_requested(move |action| {
            weak.system_action(system_action_from_index(action));
        });
        // The dock's own Palette global follows the panel's live theme preview
        // so both surfaces share one appearance at all times (dock mode only;
        // the sync is a no-op without a live strip).
        let weak = self.clone();
        panel.on_theme_changed(move || weak.sync_dock_theme());
    }

    fn wire_dock(&self, dock: &Dock) {
        let weak = self.clone();
        dock.on_launch_requested(move |key| weak.launch(&key));
        let weak = self.clone();
        dock.on_window_activate_requested(move |key| weak.activate(&key));
        let weak = self.clone();
        dock.on_system_action_requested(move |action| {
            weak.system_action(system_action_from_index(action));
        });
        let weak = self.clone();
        dock.on_open_applications_requested(move || weak.open_applications());
        let weak = self.clone();
        dock.on_refresh_requested(move || {
            let _ = weak.refresh();
        });
        let weak = self.clone();
        dock.on_exit_requested(move || {
            // Explicit exit: quitting the loop ends the run; the retained
            // watcher guard and heartbeat timer drop together with it.
            let _ = slint::quit_event_loop();
            let _ = weak;
        });
    }

    /// Busy means a worker runs right now; the panel property is the truth.
    fn busy(&self) -> bool {
        self.panel
            .upgrade()
            .map(|panel| panel.get_refreshing())
            .unwrap_or(true)
    }

    fn guarded(&self) -> bool {
        self.panel
            .upgrade()
            .map(|panel| panel.get_refreshing() || panel.get_stale())
            .unwrap_or(true)
    }

    /// Applies the current search filter to the last successful snapshot.
    /// Filtering never re-observes the desktop and never renumbers keys.
    pub(crate) fn apply_filter(&self) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let search = panel.get_search().to_string();
        let Some(snapshot) = self.core.retained_snapshot() else {
            return;
        };
        let projection = crate::projection::project(&snapshot, &search);
        show(&panel, &projection);
        self.show_apps(&panel, &search);
        if !panel.get_stale() && !panel.get_refreshing() {
            panel.set_status(projection.status.as_str().into());
        }
    }

    /// Renders the launcher rows for the current search over the retained
    /// catalog (bounded by the projection module's `MAX_APPS`).
    fn show_apps(&self, panel: &Panel, search: &str) {
        let apps = crate::projection::project_apps(&self.core.catalog(), &self.core.pins(), search);
        panel.set_apps(ModelRc::new(VecModel::from(
            apps.iter().map(app_model).collect::<Vec<_>>(),
        )));
    }

    /// Starts one single-flight observation on a worker thread. Returns
    /// `None` (without queuing) when already refreshing, both surfaces are
    /// gone, or the worker could not be spawned.
    pub(crate) fn refresh(&self) -> Option<std::thread::JoinHandle<()>> {
        let panel = self.panel.upgrade()?;
        if panel.get_refreshing() {
            return None;
        }
        self.set_busy(
            true,
            if panel.get_has_snapshot() {
                "Refreshing; activation is paused..."
            } else {
                "Reading the desktop..."
            },
        );
        // While refreshing, activation is disabled even on retained data.
        panel.set_stale(true);

        let weak = self.panel.clone();
        let controller = self.clone();
        let core = Arc::clone(&self.core);
        let spawned = std::thread::Builder::new()
            .name("tessera-observation".into())
            .spawn(move || {
                let result = core.observe_safely();
                // Closing the window/event loop legitimately drops this
                // result. On the creating thread (tests have no event loop)
                // the upgrade succeeds directly; otherwise the result is
                // marshalled to the event loop.
                if let Some(panel) = weak.upgrade() {
                    controller.apply_observation_on(&panel, result);
                } else {
                    let _ = weak.upgrade_in_event_loop(move |panel| {
                        controller.apply_observation_on(&panel, result)
                    });
                }
            });
        match spawned {
            Ok(worker) => Some(worker),
            Err(error) => {
                self.apply_observation_on(
                    &panel,
                    Err(format!("Could not start observation: {error}")),
                );
                None
            }
        }
    }

    /// Sets the busy flag and status text on every live surface.
    fn set_busy(&self, busy: bool, message: &str) {
        if let Some(panel) = self.panel.upgrade() {
            panel.set_refreshing(busy);
            if !message.is_empty() {
                panel.set_status(message.into());
            }
        }
        if let Some(dock) = self.dock_and_upgrade() {
            let mut status = dock.get_surface_status();
            status.refreshing = busy;
            if !message.is_empty() {
                status.status = message.into();
            }
            dock.set_surface_status(status);
        }
    }

    /// Writes one status message to every live surface.
    fn report_message(&self, message: &str) {
        if let Some(panel) = self.panel.upgrade() {
            panel.set_status(message.into());
        }
        if let Some(dock) = self.dock_and_upgrade() {
            let mut status = dock.get_surface_status();
            status.status = message.into();
            dock.set_surface_status(status);
        }
    }

    /// Reports one host-command outcome to every live surface.
    fn report<F: FnOnce(String) -> String>(
        &self,
        result: Result<(), String>,
        success: &str,
        failure: F,
    ) {
        let message = match result {
            Ok(()) => success.to_string(),
            Err(error) => failure(sanitize::bounded_text(&error, 200)),
        };
        if let Some(panel) = self.panel.upgrade() {
            panel.set_status(message.as_str().into());
        }
        if let Some(dock) = self.dock_and_upgrade() {
            let mut status = dock.get_surface_status();
            status.status = message.as_str().into();
            dock.set_surface_status(status);
        }
    }

    fn dock_and_upgrade(&self) -> Option<Dock> {
        self.dock.as_ref().and_then(|dock| dock.upgrade())
    }

    /// Applies one finished observation: panel rows and launcher when the
    /// panel is live, otherwise the dock strip alone (a dropped panel cannot
    /// render, but the dock still needs the result).
    fn apply_observation_on(&self, panel: &Panel, result: Result<PanelSnapshot, String>) {
        if let Some(dock) = self.dock_and_upgrade() {
            apply_result_to_both(self, panel, &dock, result);
        } else {
            apply_result(self, panel, result);
        }
    }

    /// Mirrors the panel's busy/stale/status state into the dock and repaints
    /// the strip models. Called after every state change.
    fn render(&self) {
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let mut status = dock.get_surface_status();
        status.refreshing = panel.get_refreshing();
        status.stale = panel.get_stale();
        status.status = panel.get_status();
        dock.set_surface_status(status);

        self.refresh_strip(&dock);
    }

    fn refresh_strip(&self, dock: &Dock) {
        let pins = self.core.pins();
        let catalog = self.core.catalog();
        let pinned_apps: Vec<DockApp> = pins
            .iter()
            // Unknown stored keys are skipped for display and can never be
            // launched; they stay persisted for a future session.
            .filter_map(|pin| catalog.iter().find(|app| app.key() == pin))
            .map(|app| DockApp {
                key: app.key().into(),
                label: sanitize::caption(app.title()).into(),
                icon: dock_icon(self, app.icon()),
                pinned: true,
            })
            .collect();
        dock.set_pinned_apps(ModelRc::new(VecModel::from(pinned_apps)));

        let windows = self
            .core
            .retained_snapshot()
            .map(|snapshot| {
                snapshot
                    .windows()
                    .iter()
                    .map(|window| DockWindow {
                        key: window.key().into(),
                        caption: sanitize::caption(window.title()).into(),
                        icon: dock_icon(self, window.icon()),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        dock.set_running_windows(ModelRc::new(VecModel::from(windows)));
    }

    /// Places the strip inside the work area and applies the fullscreen hide.
    /// Called after show (so the monitor's DPI scale is known), after every
    /// successful observation, and immediately after an explicit Save that
    /// changed the edge. A fullscreen work area hides the strip entirely; the
    /// UI, subscription, and heartbeat keep running, and the next passive
    /// notification-driven observation shows it again when fullscreen ends.
    fn apply_geometry(&self, context: Option<crate::DockContext>) {
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        let Some(context) = context else {
            return;
        };
        if context.fullscreen_active() {
            let _ = dock.hide();
            return;
        }
        let edge = self.core.applied_dock_edge();
        // Left/right strips lay out vertically; bottom/top horizontally.
        dock.set_vertical(matches!(
            edge,
            crate::DockEdge::Left | crate::DockEdge::Right
        ));
        let compact = dock.get_compact();
        let base_thickness = if compact {
            crate::dock::DOCK_THICKNESS_COMPACT
        } else {
            crate::dock::DOCK_THICKNESS
        };
        // Vertical command labels need room beside a stock scrollbar.
        let thickness = base_thickness + if dock.get_vertical() { 24 } else { 0 };
        let rect = crate::dock::dock_rect(context, edge, thickness, dock.window().scale_factor());
        dock.window()
            .set_size(slint::WindowSize::Physical(slint::PhysicalSize::new(
                rect.width,
                rect.height,
            )));
        dock.window().set_position(slint::WindowPosition::Physical(
            slint::PhysicalPosition::new(rect.x, rect.y),
        ));
        let _ = dock.show();
    }

    /// Opens (creates not; already constructed) and shows the launcher and
    /// settings window in dock mode. Its close button hides only the window.
    fn open_applications(&self) {
        if let Some(panel) = self.panel.upgrade() {
            let _ = panel.show();
        }
    }

    /// Copies the panel's current theme preview into the dock's Palette.
    fn sync_dock_theme(&self) {
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let scheme = match crate::theme_from_index(panel.get_theme_index()) {
            crate::Theme::Light => slint::language::ColorScheme::Light,
            crate::Theme::Dark => slint::language::ColorScheme::Dark,
            crate::Theme::System => slint::language::ColorScheme::Unknown,
        };
        dock.global::<Palette>().set_color_scheme(scheme);
    }

    /// Activates a window through the host. Guarded twice: the panel refresh
    /// state and this callback re-check, so a queued or synthetic activation
    /// can never act on a stale snapshot or during an observation.
    pub(crate) fn activate(&self, key: &str) {
        if self.guarded() {
            return;
        }
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let resolved = displayed_key(&panel.get_rows(), key).or_else(|| {
            self.dock_and_upgrade().and_then(|dock| {
                model_key(&dock.get_running_windows(), key, |window| {
                    window.key.to_string()
                })
            })
        });
        let Some(key) = resolved else {
            panel.set_status("That window is no longer in the current observation.".into());
            return;
        };
        let result = self.core.host().activate(&key);
        self.report(result, "Window activated", |detail| {
            format!("Activation failed: {detail}")
        });
    }

    /// Launches an application through the host. Only a key currently shown
    /// in the launcher rows or the dock strip reaches the host; stale catalog
    /// data cannot launch.
    pub(crate) fn launch(&self, key: &str) {
        if self.guarded() {
            return;
        }
        let Some(key) = self.resolve_app_key(key) else {
            self.report_message("That application is no longer in the current catalog.");
            return;
        };
        let result = self.core.host().launch(&key);
        self.report(result, "Application launched", |detail| {
            format!("Launch failed: {detail}")
        });
    }

    /// Toggles one pin and persists it immediately — reusing the last saved
    /// appearance values so the live appearance preview is never persisted by
    /// a pin click. On save failure the pin state reverts and stays visible.
    pub(crate) fn toggle_pin(&self, key: &str, pin: bool) {
        // `pin` is the *new* state: true means "pin this application".
        let Some(resolved) = self.resolve_app_key(key) else {
            self.report_message("That application is no longer in the current catalog.");
            return;
        };
        let mut pins = self.core.pins();
        let pinned_already = pins.iter().any(|existing| existing == &resolved);
        if pin == pinned_already {
            return;
        }
        if pin && self.core.pin_capacity_full(&pins) {
            self.report_message(&format!("Pin limit reached ({} apps)", crate::MAX_PINS));
            return;
        }
        if pin {
            pins.push(resolved.clone());
        } else {
            pins.retain(|existing| existing != &resolved);
        }
        let appearance = self.core.applied_appearance();
        let preferences = PanelPreferences::new(appearance.theme, appearance.compact)
            .with_dock(self.core.applied_dock_edge(), pins.clone());
        match self.core.host().save_preferences(&preferences) {
            Ok(()) => {
                self.core.record_applied(&preferences);
                self.report_message(if pin {
                    "Application pinned"
                } else {
                    "Pin removed"
                });
                if let Some(panel) = self.panel.upgrade() {
                    self.show_apps(&panel, &panel.get_search());
                }
                self.render();
            }
            Err(error) => self.report_message(&format!(
                "Could not save pins: {}",
                sanitize::bounded_text(&error, 200)
            )),
        }
    }

    /// Dispatches one explicit system action to the host.
    pub(crate) fn system_action(&self, action: SystemAction) {
        let description = match action {
            SystemAction::OpenFileManager => "Opening File Explorer",
            SystemAction::OpenTaskManager => "Opening Task Manager",
            SystemAction::RestoreExplorer => "Restoring the Windows Explorer shell",
        };
        let result = self.core.host().system_action(action);
        self.report(result, description, |detail| {
            format!("{description} failed: {detail}")
        });
    }

    /// Saves the currently previewed preferences through the host. Dock edge
    /// and pins are preview/live state; this records them on success and
    /// re-applies the dock geometry immediately (edge change is visible).
    pub(crate) fn save_preferences(&self) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let preferences = PanelPreferences::new(
            crate::theme_from_index(panel.get_theme_index()),
            panel.get_compact(),
        )
        .with_dock(
            crate::dock_edge_from_index(panel.get_dock_edge_index()),
            self.core.pins(),
        );
        match self.core.host().save_preferences(&preferences) {
            Ok(()) => {
                self.core.record_applied(&preferences);
                panel.set_status("Preferences saved".into());
                let context = self
                    .core
                    .retained_snapshot()
                    .and_then(|snapshot| snapshot.dock_context());
                self.apply_geometry(context);
                self.render();
            }
            Err(error) => {
                let detail = sanitize::bounded_text(&error, 200);
                panel.set_status(format!("Could not save preferences: {detail}").into());
            }
        }
    }

    /// Resolves a clicked application key against the launcher rows and, in
    /// dock mode, the pinned strip actually on screen.
    pub(crate) fn resolve_app_key(&self, key: &str) -> Option<String> {
        let panel_key = self
            .panel
            .upgrade()
            .and_then(|panel| model_key(&panel.get_apps(), key, |row| row.key.to_string()));
        panel_key.or_else(|| {
            self.dock_and_upgrade()
                .and_then(|dock| model_key(&dock.get_pinned_apps(), key, |app| app.key.to_string()))
        })
    }

    /// UI-thread routes for the shared notification bus. A gone panel reports
    /// busy so the bus never spawns work after the surface is dropped.
    pub(crate) fn routes(&self) -> Routes {
        let busy_controller = self.clone();
        let refresh_controller = self.clone();
        Routes {
            is_refreshing: Arc::new(move || busy_controller.busy()),
            start_refresh: Arc::new(move || {
                let _ = refresh_controller.refresh();
            }),
        }
    }
}

/// Builds the strip icon through the controller's shared icon cache.
fn dock_icon(controller: &PanelController, icon: Option<&crate::PixelIcon>) -> slint::Image {
    controller
        .icon_cache
        .lock()
        .optional(icon)
        .unwrap_or_default()
}

/// Resolves a clicked key against the rows actually on screen. A stale,
/// filtered-out, or fabricated key (or a display index passed as a key) is
/// rejected here, so activation always maps to a visible row.
pub(crate) fn displayed_key(model: &ModelRc<Row>, key: &str) -> Option<String> {
    model_key(model, key, |row| row.key.to_string())
}

pub(crate) fn model_key<M: Clone + 'static>(
    model: &ModelRc<M>,
    key: &str,
    key_of: impl Fn(&M) -> String,
) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    (0..model.row_count()).find_map(|index| {
        let row: M = model.row_data(index)?;
        let candidate = key_of(&row);
        (candidate == key).then_some(candidate)
    })
}

fn show(panel: &Panel, projection: &PanelProjection) {
    panel.set_rows(ModelRc::new(VecModel::from(
        projection.rows.iter().map(row_model).collect::<Vec<_>>(),
    )));
    panel.set_summary(projection.summary.as_str().into());
}

fn row_model(row: &RowProjection) -> Row {
    Row {
        key: row.key.as_str().into(),
        caption: row.caption.as_str().into(),
        minimized: row.minimized,
    }
}

fn app_model(app: &AppProjection) -> AppRow {
    AppRow {
        key: app.key.as_str().into(),
        label: app.label.as_str().into(),
        pinned: app.pinned,
    }
}

/// Panel-only surface completion (used by tests and panel mode).
pub(crate) fn apply_result(
    controller: &PanelController,
    panel: &Panel,
    result: Result<PanelSnapshot, String>,
) {
    match result {
        Ok(snapshot) => {
            let projection = crate::projection::project(&snapshot, &panel.get_search());
            show(panel, &projection);
            controller.show_apps(panel, &panel.get_search());
            panel.set_status(projection.status.as_str().into());
            panel.set_has_snapshot(true);
            panel.set_stale(false);
            // Retain the full snapshot for filtering without new observation.
            controller.core.store_snapshot(snapshot);
        }
        Err(error) => {
            let prefix = if panel.get_has_snapshot() {
                "Refresh failed; retained data is stale"
            } else {
                "Could not observe the desktop"
            };
            let detail = sanitize::bounded_text(&error, 200);
            panel.set_status(format!("{prefix}: {detail}. Try Refresh.").into());
            // A failed refresh means the desktop may have changed; activation
            // stays paused until the next successful observation confirms it.
            panel.set_stale(true);
        }
    }
    // Release single-flight only after applying the result on the UI thread.
    panel.set_refreshing(false);
    // A notification that arrived while this worker ran queues exactly one
    // follow-up refresh; none means the burst is fully drained.
    if controller.core.take_dirty() {
        let _ = controller.refresh();
    }
}

/// Both-surfaces completion: panel state first (it is the truth), then dock
/// mirror, strip, and geometry. Stale/busy handling is identical.
fn apply_result_to_both(
    controller: &PanelController,
    panel: &Panel,
    _dock: &Dock,
    result: Result<PanelSnapshot, String>,
) {
    apply_result(controller, panel, result);
    controller.render();
    let context = controller
        .core
        .retained_snapshot()
        .and_then(|snapshot| snapshot.dock_context());
    controller.apply_geometry(context);
}

/// Builds the requested surface pair, installs callbacks, and runs the loop.
///
/// Panel mode runs the panel normally (closing it quits). Dock mode shows the
/// strip and keeps the loop alive with `run_event_loop_until_quit`: hiding
/// the strip (fullscreen work area) or the launcher panel must never
/// terminate the UI or the watchdog; exit is the strip's explicit Exit
/// button, which the host supervisor follows by restoring the Explorer shell.
pub(crate) fn run(
    host: Arc<Host>,
    preferences: PanelPreferences,
    startup_notice: Option<String>,
    run_options: RunOptions,
) -> Result<(), slint::PlatformError> {
    let panel = Panel::new()?;
    panel.set_theme_index(crate::theme_to_index(preferences.theme()));
    panel.set_compact(preferences.compact());
    panel.set_dock_edge_index(crate::dock_edge_to_index(preferences.dock_edge()));
    panel.set_version(env!("CARGO_PKG_VERSION").into());

    let mut subscription_error = None;
    // The watcher guard is retained for the whole run: dropping it would stop
    // the native watcher, and a hidden dock must keep receiving notifications.
    let (core, _watcher_guard) = SurfaceCore::new(host, &preferences, &mut subscription_error);

    // Subscription failure leaves manual refresh and a plain notice; it is
    // never a fatal startup condition.
    let combined = match (subscription_error, startup_notice) {
        (Some(subscription), Some(startup)) => Some(format!("{startup} {subscription}")),
        (Some(subscription), None) => Some(subscription),
        (None, startup) => startup,
    };
    panel.set_startup_notice(
        combined
            .as_deref()
            .map(|notice| sanitize::bounded_text(notice, 400))
            .unwrap_or_default()
            .into(),
    );

    let heartbeat_timer = crate::start_heartbeat(&run_options);

    match run_options.surface {
        crate::SurfaceMode::Panel => {
            let controller = PanelController::new(&panel, Arc::clone(&core));
            core.install_routes(controller.routes());
            controller.apply_filter();
            let _ = controller.refresh();
            panel.run()?;
        }
        crate::SurfaceMode::Dock => {
            let dock = Dock::new()?;
            dock.set_compact(preferences.compact());
            // Single-flight starts idle; refresh() below owns the busy flag.
            dock.set_surface_status(DockStatus {
                notice: "".into(),
                status: "Ready to observe".into(),
                refreshing: false,
                stale: false,
            });
            let controller = PanelController::new_with_dock(&panel, &dock, Arc::clone(&core));
            core.install_routes(controller.routes());
            controller.apply_filter();
            controller.sync_dock_theme();
            // The launcher panel starts hidden in dock mode; the strip's
            // Applications button shows it, and its close button only hides.
            dock.window().on_close_requested(|| {
                // The strip leaves the screen only via the explicit Exit
                // button (which quits the loop); a system close request just
                // hides the window and the loop keeps running.
                slint::CloseRequestResponse::HideWindow
            });
            panel.window().on_close_requested(|| {
                // In dock mode the panel is a secondary launcher window:
                // closing it hides only the panel, the strip keeps running.
                slint::CloseRequestResponse::HideWindow
            });
            let _ = controller.refresh();
            // Show the strip after the loop data is set; geometry (which may
            // hide it for a fullscreen work area) runs on each observation.
            dock.show()?;
            slint::run_event_loop_until_quit()?;
        }
    }
    // The heartbeat timer is retained until the loop has ended; dropping it
    // earlier would silently stop the two-second watchdog signal.
    drop(heartbeat_timer);
    Ok(())
}

#[cfg(test)]
mod tests;
