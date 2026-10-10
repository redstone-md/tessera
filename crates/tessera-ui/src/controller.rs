// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::generated::{
    AppRow, Dock, DockApp, DockGroup, DockStatus, DockSystemCommand, DockWindow, DockWindowCommand,
    Launcher, Row, Toolbar,
};
use crate::projection::{AppProjection, PanelProjection, RowProjection};
use crate::state::{Routes, SurfaceCore};
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::{
    DesktopHost, Panel, PanelPreferences, PanelSnapshot, RunOptions, SurfaceKind, SystemAction,
    WindowAction, sanitize, system_action_from_index,
};

mod actions;
mod application_menu;
mod bar_visibility;
mod battery;
mod calendar;
mod context_menu;
mod dock_middle_click;
mod dock_order;
mod dock_utilities;
mod geometry;
mod launcher;
mod native_toolbar;
mod passive_toolbar;
mod popups;
mod power_display;
mod power_menu;
mod quick_settings;
mod recycle_bin;
mod shortcuts;
mod startup;
mod telemetry;
mod tooltip;
mod user_menu;
mod visibility;
use battery::BatteryPopups;
use calendar::CalendarPopups;
use context_menu::Menus;
use dock_utilities::DockUtilities;
use native_toolbar::{BluetoothPopups, InputLanguagePopups, NetworkPopups};
use power_menu::PowerPopups;
use quick_settings::QuickPopups;
use recycle_bin::RecycleBins;
use tooltip::Tooltips;
use user_menu::UserPopups;

type Host = dyn DesktopHost;

// Presentation choices, not persisted indices; valid non-preset RGB stays intact.
const SOURCE_PRESETS: [u32; 5] = [0x7ca45c, 0xffbd59, 0x4d90fe, 0xff7849, 0xe78cba];

/// Native leases, keyed by [`SurfaceKind`], retained by the controller.
///
/// A lease is the `Box<dyn Any>` returned by
/// [`DesktopHost::configure_surface`] (the adapter's RAII native attachment).
/// Dropping it is the detach: it happens strictly before a hide, before a
/// geometry re-reservation, and before the HWND closes (run teardown).
///
/// The registry never leaves the UI thread. Worker completion carries only a
/// weak component signal; the component callback accesses this registry locally.
///
/// Each surface also remembers the physical rect it was last attached with,
/// so a geometry change is detected in the UI and re-reservation (a real
/// drop + attach cycle against the new RECT) happens only when the geometry
/// actually changed — never on unchanged foreground observations.
#[derive(Default)]
pub(crate) struct SurfaceLeases {
    attachments: std::collections::HashMap<SurfaceKind, SurfaceAttachment>,
}

struct SurfaceAttachment {
    _lease: Option<Box<dyn std::any::Any>>,
    rect: (i32, i32, u32, u32),
}

impl SurfaceLeases {
    /// Pure storage: native configuration and attachment destruction must run
    /// outside the registry's RefCell borrow, since both can synchronously reenter.
    fn store(
        &mut self,
        kind: SurfaceKind,
        lease: Option<Box<dyn std::any::Any>>,
        rect: (i32, i32, u32, u32),
    ) -> Option<SurfaceAttachment> {
        self.attachments.insert(
            kind,
            SurfaceAttachment {
                _lease: lease,
                rect,
            },
        )
    }

    fn take(&mut self, kind: SurfaceKind) -> Option<SurfaceAttachment> {
        self.attachments.remove(&kind)
    }

    /// Whether `rect` differs from the last attached rect for `kind`.
    pub(crate) fn geometry_changed(&self, kind: SurfaceKind, rect: (i32, i32, u32, u32)) -> bool {
        self.attachments.get(&kind).map(|attached| attached.rect) != Some(rect)
    }
}

struct SurfaceLeaseScope(Rc<RefCell<SurfaceLeases>>);

impl Drop for SurfaceLeaseScope {
    fn drop(&mut self) {
        let leases = std::mem::take(&mut *self.0.borrow_mut());
        drop(leases);
    }
}

/// One controller drives both surfaces.
///
/// In panel mode only the panel window exists. In dock mode the always-present
/// native Seelen-style surfaces are the dock strip and the top toolbar; the
/// frameless launcher opens from the dock's start tile; a separately owned
/// quick-settings popup opens from the toolbar's audio/settings entry.
/// The framed Panel remains the settings/recovery window and developer mode.
///
/// The panel's `refreshing`/`stale`/`has-snapshot` properties remain the
/// single source of truth for observational surfaces; dock/toolbar/launcher
/// mirrors are kept in sync by [`PanelController::render`]. Independent audio
/// never observes the desktop or borrows its busy/stale state. There is one
/// observation worker, one notification bus, and one retained snapshot —
/// never two independent observers.
///
/// Retained snapshots, the catalog, and pins live in the shared
/// [`SurfaceCore`]; the controller owns only the weak handles. Native leases
/// live in the controller's UI-thread [`SurfaceLeases`] registry: bar leases
/// attach on the first real-geometry observation (and re-attach on actual
/// geometry changes — the native reservation binds to the attach-time RECT),
/// and the launcher lease attaches on show and drops before hide.
#[derive(Clone)]
pub(crate) struct PanelController {
    panel: slint::Weak<Panel>,
    dock: Option<slint::Weak<Dock>>,
    toolbar: Option<slint::Weak<Toolbar>>,
    launcher: Option<slint::Weak<Launcher>>,
    core: Arc<SurfaceCore>,
    icon_cache: Rc<RefCell<crate::icons::IconCache>>,
    leases: Rc<RefCell<SurfaceLeases>>,
    launcher_state: Rc<RefCell<launcher::LauncherState>>,
    dock_order: Rc<dock_order::DockOrder>,
    dock_middle_click: Rc<dock_middle_click::DockMiddleClick>,
    application_menu_source: Rc<application_menu::ApplicationMenuSource>,
    preference_saving: Rc<Cell<bool>>,
    surface_failure: Rc<RefCell<Option<String>>>,
    popup_operation: Rc<RefCell<Rc<()>>>,
    power_admission_closed: Rc<Cell<bool>>,
    menus: Menus,
    quick_settings: QuickPopups,
    user_menu: UserPopups,
    calendar: CalendarPopups,
    power_menu: PowerPopups,
    dock_utilities: DockUtilities,
    recycle_bin: RecycleBins,
    tooltips: Tooltips,
    battery: BatteryPopups,
    telemetry: telemetry::TelemetryRoots,
    startup: startup::StartupRoots,
    network_menu: NetworkPopups,
    bluetooth: BluetoothPopups,
    input_language: InputLanguagePopups,
    launcher_app_menu: crate::transient_window::TransientCache<crate::launcher::LauncherAppMenu>,
    dock_media: crate::transient_window::TransientCache<crate::dock_media::DockMediaController>,
    visibility: crate::transient_window::TransientCache<crate::visibility::VisibilityController>,
    visibility_geometry: Rc<RefCell<visibility::RootGeometry>>,
    power_display: crate::transient_window::TransientCache<power_display::PowerDisplayWatch>,
    geometry: Rc<geometry::GeometryUpdates>,
    admission: Rc<shortcuts::RootAdmission>,
    shortcuts: Rc<shortcuts::ShortcutIntegration>,
}

/// Complete-record writes are single-flight even when a host callback re-enters
/// a different surface's save action. Never hold a state borrow during the write.
struct PreferenceSave<'a>(&'a PanelController);

impl Drop for PreferenceSave<'_> {
    fn drop(&mut self) {
        self.0.preference_saving.set(false);
        self.0.sync_launcher_reorder();
        self.0.sync_dock_reorder();
        self.0.sync_dock_middle_click();
        self.0.sync_startup_root();
    }
}

impl PanelController {
    fn begin_preference_save(&self) -> Option<PreferenceSave<'_>> {
        if self.power_admission_closed.get() || self.preference_saving.replace(true) {
            return None;
        }
        let save = PreferenceSave(self);
        self.cancel_launcher_input();
        self.cancel_launcher_reorder();
        self.cancel_dock_reorder();
        self.hide_launcher_app_menu();
        let menu = self.menus.borrow().clone();
        if let Some(menu) = menu {
            menu.hide();
        }
        if self.power_admission_closed.get() {
            None
        } else {
            Some(save)
        }
    }

    pub(crate) fn new(panel: &Panel, core: Arc<SurfaceCore>) -> Self {
        let controller = Self {
            panel: panel.as_weak(),
            dock: None,
            toolbar: None,
            launcher: None,
            core,
            icon_cache: Rc::default(),
            leases: Rc::default(),
            launcher_state: Rc::default(),
            dock_order: Rc::default(),
            dock_middle_click: Rc::default(),
            application_menu_source: Rc::default(),
            preference_saving: Rc::default(),
            surface_failure: Rc::default(),
            popup_operation: Rc::default(),
            power_admission_closed: Rc::default(),
            menus: Rc::default(),
            quick_settings: Rc::default(),
            user_menu: Rc::default(),
            calendar: Rc::default(),
            power_menu: Rc::default(),
            dock_utilities: Rc::default(),
            recycle_bin: Rc::default(),
            tooltips: Rc::default(),
            battery: Rc::default(),
            telemetry: Rc::default(),
            startup: Rc::default(),
            network_menu: Rc::default(),
            bluetooth: Rc::default(),
            input_language: Rc::default(),
            launcher_app_menu: Rc::default(),
            dock_media: Rc::default(),
            visibility: Rc::default(),
            visibility_geometry: Rc::default(),
            power_display: Rc::default(),
            geometry: Rc::default(),
            admission: Rc::default(),
            shortcuts: Rc::default(),
        };
        controller.wire_panel(panel);
        controller.wire_completion(panel);
        controller.wire_motion(panel);
        controller.wire_shortcuts(panel);
        controller
    }

    /// Dock mode: the same controller, plus weak handles for the three
    /// native surfaces (dock strip, toolbar, launcher).
    pub(crate) fn new_with_dock(
        panel: &Panel,
        dock: &Dock,
        toolbar: &Toolbar,
        launcher: &Launcher,
        core: Arc<SurfaceCore>,
    ) -> Self {
        let controller = Self {
            panel: panel.as_weak(),
            dock: Some(dock.as_weak()),
            toolbar: Some(toolbar.as_weak()),
            launcher: Some(launcher.as_weak()),
            core,
            icon_cache: Rc::default(),
            leases: Rc::default(),
            launcher_state: Rc::default(),
            preference_saving: Rc::default(),
            surface_failure: Rc::default(),
            popup_operation: Rc::default(),
            power_admission_closed: Rc::default(),
            menus: Rc::default(),
            quick_settings: Rc::default(),
            user_menu: Rc::default(),
            calendar: Rc::default(),
            power_menu: Rc::default(),
            dock_utilities: Rc::default(),
            dock_order: Rc::default(),
            dock_middle_click: Rc::default(),
            application_menu_source: Rc::default(),
            recycle_bin: Rc::default(),
            tooltips: Rc::default(),
            battery: Rc::default(),
            telemetry: Rc::default(),
            startup: Rc::default(),
            network_menu: Rc::default(),
            bluetooth: Rc::default(),
            input_language: Rc::default(),
            launcher_app_menu: Rc::default(),
            dock_media: Rc::default(),
            visibility: Rc::default(),
            visibility_geometry: Rc::default(),
            power_display: Rc::default(),
            geometry: Rc::default(),
            admission: Rc::default(),
            shortcuts: Rc::default(),
        };
        controller.wire_panel(panel);
        controller.wire_dock(dock);
        controller.wire_toolbar(toolbar);
        controller.wire_launcher(launcher);
        controller.wire_completion(panel);
        controller.wire_motion(panel);
        controller.initialize_media(dock);
        controller.wire_visibility(panel);
        controller.wire_shortcuts(panel);
        controller
    }

    fn wire_completion(&self, panel: &Panel) {
        let controller = self.clone();
        panel.on_observation_result_ready(move || {
            let Some(panel) = controller.panel.upgrade() else {
                return;
            };
            let Some(result) = controller.core.take_observation() else {
                return;
            };
            if controller.dock.is_some() {
                apply_result_to_both(&controller, &panel, result);
            } else {
                apply_result(&controller, &panel, result);
            }
        });
        let panel = panel.as_weak();
        self.core.install_apply_route(Arc::new(move || {
            if let Some(panel) = panel.upgrade() {
                panel.invoke_observation_result_ready();
            }
        }));
    }

    fn wire_motion(&self, panel: &Panel) {
        let controller = self.clone();
        panel.on_motion_policy_changed(move |enabled| {
            if enabled {
                return;
            }
            let tooltip = controller.tooltips.borrow().clone();
            if let Some(tooltip) = tooltip {
                tooltip.disable_motion();
            }
            let menu = controller.menus.borrow().clone();
            if let Some(menu) = menu {
                menu.disable_motion();
            }
            let quick = controller.quick_settings.borrow().clone();
            if let Some(quick) = quick {
                quick.disable_motion();
            }
            let user = controller.user_menu.borrow().clone();
            if let Some(user) = user {
                user.disable_motion();
            }
            let calendar = controller.calendar.borrow().clone();
            if let Some(calendar) = calendar {
                calendar.disable_motion();
            }
            let power = controller.power_menu.borrow().clone();
            if let Some(power) = power {
                power.disable_motion();
            }
            controller.toolbar_popup_motion_disabled();
            let menu = controller.launcher_app_menu.borrow().clone();
            if let Some(menu) = menu {
                menu.disable_motion();
            }
        });
    }

    /// Arms the retained heartbeat through a UI-thread component callback.
    fn pulse_readiness(&self) {
        if let Some(panel) = self.panel.upgrade() {
            panel.invoke_readiness_pulse();
        }
    }

    fn wire_panel(&self, panel: &Panel) {
        let applied = self.core.applied_preferences();
        panel.set_dock_middle_click_index(applied.dock_middle_click().index());
        panel.set_telemetry_enabled(applied.telemetry_enabled());
        self.project_bar_visibility_preferences(panel);
        let visibility_available = self.visibility.borrow().is_some();
        panel.set_bar_visibility_available(visibility_available);
        panel.set_dock_preferences_available(self.dock.is_some());
        let telemetry_available = self.core.host().telemetry_host().is_some();
        panel.set_telemetry_available(telemetry_available);
        self.wire_startup(panel);
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
        // All surfaces preview one appearance without writing preferences.
        let weak = self.clone();
        panel.on_appearance_changed(move || weak.sync_appearance());
        let weak = self.clone();
        panel.on_source_preset_requested(move |index| {
            if !weak.root_current() {
                return;
            }
            let Some(rgb) = usize::try_from(index)
                .ok()
                .and_then(|index| SOURCE_PRESETS.get(index))
                .copied()
            else {
                return;
            };
            if let Some(panel) = weak.panel.upgrade() {
                panel.set_source_rgb(rgb as i32);
            }
        });
        self.wire_power(panel);
    }

    fn wire_dock(&self, dock: &Dock) {
        self.wire_dock_reorder(dock);
        self.wire_dock_middle_click(dock);
        dock.on_group_count(|metadata, key| {
            metadata
                .iter()
                .find(|group| group.key == key)
                .map(|group| group.count)
                .unwrap_or(0)
        });
        dock.on_group_tooltip(|windows, metadata, key| {
            let Some(group) = metadata.iter().find(|group| group.key == key) else {
                return slint::SharedString::default();
            };
            let titles: Vec<_> = windows
                .iter()
                .filter(|window| {
                    metadata
                        .iter()
                        .any(|member| member.key == window.key && member.identity == group.identity)
                })
                .map(|window| window.caption.to_string())
                .collect();
            format!("\n{} windows\n{}", titles.len(), titles.join("\n")).into()
        });
        let weak = self.clone();
        dock.on_launch_requested(move |key| {
            if weak.bar_input_ready(SurfaceKind::Dock) {
                weak.launch(&key);
            }
        });
        let controller = self.clone();
        dock.on_activate_app_requested(move |key| {
            if !controller.bar_input_ready(SurfaceKind::Dock) || controller.guarded() {
                return;
            }
            let Some(dock) = controller.dock_and_upgrade() else {
                return;
            };
            if model_key(&dock.get_pinned_apps(), &key, |app| app.key.to_string()).is_none() {
                return;
            }
            let identity = format!("aumid:{}", crate::projection::catalog_aumid(&key));
            let metadata = dock.get_group_metadata();
            let matching: Vec<_> = dock
                .get_observed_windows()
                .iter()
                .filter(|window| {
                    metadata
                        .iter()
                        .any(|group| group.key == window.key && group.identity == identity.as_str())
                })
                .collect();
            if matching.len() == 1 {
                controller.window_action(&matching[0].key, WindowAction::ActivateOrMinimize);
            } else if matching.is_empty() {
                controller.launch(&key);
            }
        });
        let weak = self.clone();
        dock.on_window_command_requested(move |key, command| {
            if !weak.bar_input_ready(SurfaceKind::Dock) {
                return;
            }
            let action = match command {
                DockWindowCommand::Activate => WindowAction::Activate,
                DockWindowCommand::Toggle => WindowAction::ActivateOrMinimize,
                DockWindowCommand::Minimize => WindowAction::Minimize,
                DockWindowCommand::Close => WindowAction::Close,
            };
            weak.window_action(&key, action);
        });
        let weak = self.clone();
        dock.on_open_settings_requested(move || {
            if weak.bar_input_ready(SurfaceKind::Dock) {
                weak.open_panel();
            }
        });
        let weak = self.clone();
        dock.on_pin_toggle_requested(move |key, pin| {
            if weak.bar_input_ready(SurfaceKind::Dock) {
                weak.toggle_pin(&key, pin);
            }
        });
        let weak = self.clone();
        dock.on_system_command_requested(move |command| {
            if !weak.bar_input_ready(SurfaceKind::Dock) {
                return;
            }
            let action = match command {
                DockSystemCommand::FileManager => SystemAction::OpenFileManager,
                DockSystemCommand::TaskManager => SystemAction::OpenTaskManager,
                DockSystemCommand::Restore => SystemAction::RestoreExplorer,
            };
            weak.system_action(action);
        });
        let weak = self.clone();
        dock.on_reserved_action_requested(move |action| {
            if weak.bar_input_ready(SurfaceKind::Dock) {
                weak.request_dock_utility(action);
            }
        });
        let weak = self.clone();
        dock.on_utility_event_ready(move || weak.dock_utility_event_ready());
        let weak = self.clone();
        dock.on_recycle_action_requested(move |action| {
            if weak.bar_input_ready(SurfaceKind::Dock) {
                weak.request_recycle_bin(action);
            }
        });
        let weak = self.clone();
        dock.on_recycle_event_ready(move || weak.recycle_bin_event_ready());
        let weak = self.clone();
        dock.on_recycle_projection_changed(move || weak.refresh_recycle_actions());
        let weak = self.clone();
        self.wire_media(dock);
        dock.on_context_menu_requested(move |kind, key, point| {
            if weak.bar_input_ready(SurfaceKind::Dock)
                && !weak.preference_saving.get()
                && weak
                    .dock_and_upgrade()
                    .is_some_and(|dock| dock.window().is_visible())
            {
                weak.open_dock_menu(kind, &key, (point.x, point.y));
            }
        });
        let controller = self.clone();
        let owner = dock.as_weak();
        dock.on_tooltip_requested(move |content, bounds| {
            if !controller.bar_input_ready(SurfaceKind::Dock) {
                return;
            }
            if let Some(dock) = owner.upgrade() {
                let side = match crate::dock_edge_from_index(dock.get_edge()) {
                    crate::DockEdge::Bottom => crate::tooltip::Side::Top,
                    crate::DockEdge::Top => crate::tooltip::Side::Bottom,
                    crate::DockEdge::Left => crate::tooltip::Side::Right,
                    crate::DockEdge::Right => crate::tooltip::Side::Left,
                };
                controller.show_tooltip(&dock, SurfaceKind::Dock, &content, bounds, side);
            }
        });
        let controller = self.clone();
        dock.on_tooltip_dismissed(move |delayed, origin| {
            controller.dismiss_hover_tooltip(SurfaceKind::Dock, origin, delayed);
        });
        let weak = self.clone();
        dock.on_open_applications_requested(move || {
            if weak.bar_input_ready(SurfaceKind::Dock) {
                weak.toggle_launcher();
            }
        });
        let weak = self.clone();
        dock.on_exit_requested(move || {
            if !weak.bar_input_ready(SurfaceKind::Dock) {
                return;
            }
            // Explicit exit: quitting the loop ends the run; the retained
            // watcher guard and heartbeat timer drop together with it.
            let _ = slint::quit_event_loop();
        });
    }

    fn initialize_media(&self, dock: &Dock) {
        let media = crate::dock_media::DockMediaController::new(self.core.host().clone(), dock);
        *self.dock_media.borrow_mut() = Some(Rc::clone(&media));
        media.update_applications(&self.core.catalog());
        media.set_enabled(self.core.applied_preferences().media_enabled());
    }

    fn media_input_ready(&self) -> bool {
        !self.power_admission_closed.get()
            && self.bar_input_ready(SurfaceKind::Dock)
            && self.core.applied_preferences().media_enabled()
            && self
                .dock_and_upgrade()
                .is_some_and(|dock| dock.window().is_visible() && dock.get_media_view().enabled)
            && self
                .core
                .dock_context()
                .is_some_and(|context| !context.fullscreen_active())
    }

    fn wire_media(&self, dock: &Dock) {
        let controller = self.clone();
        dock.on_media_action_requested(move |action| {
            if !controller.media_input_ready() {
                return;
            }
            let media = controller.dock_media.borrow().clone();
            if let Some(media) = media {
                media.request(action);
            }
        });
        let controller = self.clone();
        dock.on_media_retry_requested(move || {
            if !controller.media_input_ready() {
                return;
            }
            let media = controller.dock_media.borrow().clone();
            if let Some(media) = media {
                media.retry();
            }
        });
        let controller = self.clone();
        dock.on_media_event_ready(move || {
            if controller.power_admission_closed.get() {
                return;
            }
            let media = controller.dock_media.borrow().clone();
            if let Some(media) = media {
                media.process_events();
            }
        });
        let controller = self.clone();
        dock.on_media_enabled_requested(move |enabled| controller.set_media_enabled(enabled));
        let controller = self.clone();
        dock.on_media_context_requested(move |point| controller.open_media_menu(point));
    }

    fn set_media_enabled(&self, enabled: bool) {
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        let applied = self.core.applied_preferences();
        if self.power_admission_closed.get()
            || self.preference_saving.get()
            || !dock.window().is_visible()
            || self
                .core
                .dock_context()
                .is_none_or(|context| context.fullscreen_active())
            || applied.media_enabled() != dock.get_media_view().enabled
            || applied.media_enabled() == enabled
        {
            return;
        }
        let Some(_save) = self.begin_preference_save() else {
            return;
        };
        let preferences = applied.with_media_enabled(enabled);
        match self.core.host().save_preferences(&preferences) {
            Ok(()) => {
                self.core.record_applied(&preferences);
                let media = self.dock_media.borrow().clone();
                if let Some(media) = media {
                    media.set_enabled(enabled);
                }
                self.update_geometry();
                self.report_message(if enabled {
                    "Media module added"
                } else {
                    "Media module removed"
                });
            }
            Err(error) => self.report_message(&format!(
                "Could not save media module: {}",
                sanitize::bounded_text(&error, 200)
            )),
        }
    }

    fn open_media_menu(&self, point: slint::LogicalPosition) {
        if !self.media_input_ready()
            || self.preference_saving.get()
            || !point.x.is_finite()
            || !point.y.is_finite()
        {
            return;
        }
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        let Some(context) = self.core.dock_context() else {
            return;
        };
        let operation = self.popup_operation.borrow().clone();
        self.dismiss_tooltip(false);
        let current = || {
            self.media_input_ready()
                && !self.preference_saving.get()
                && self.core.dock_context() == Some(context)
                && Rc::ptr_eq(&operation, &self.popup_operation.borrow())
        };
        self.hide_launcher_app_menu();
        if !current() {
            return;
        }
        let existing = self.menus.borrow().clone();
        let menu = match existing {
            Some(menu) => menu,
            None => {
                let menu = match crate::context_menu::ContextMenuController::new(
                    self.core.host().clone(),
                    &dock,
                ) {
                    Ok(menu) => menu,
                    Err(error) => {
                        self.report_message(&format!("Could not create media menu: {error}"));
                        return;
                    }
                };
                if !current() {
                    menu.hide();
                    return;
                }
                let published = {
                    let mut cache = self.menus.borrow_mut();
                    if cache.is_none() {
                        *cache = Some(Rc::clone(&menu));
                        true
                    } else {
                        false
                    }
                };
                if !published {
                    menu.hide();
                    return;
                }
                menu
            }
        };
        let result = menu.show_media(&dock, point, context);
        if current() {
            self.popup_presentation_finished(popups::PopupKind::DockMenu, menu.is_open(), result);
        }
    }

    fn wire_toolbar(&self, toolbar: &Toolbar) {
        let weak = self.clone();
        toolbar.on_quick_settings_requested(move |bounds| {
            if weak.bar_input_ready(SurfaceKind::Toolbar) {
                weak.open_quick_settings(bounds);
            }
        });
        let weak = self.clone();
        toolbar.on_calendar_requested(move |bounds| {
            if weak.bar_input_ready(SurfaceKind::Toolbar) {
                weak.open_calendar(bounds);
            }
        });
        let controller = self.clone();
        let owner = toolbar.as_weak();
        toolbar.on_battery_requested(move |bounds| {
            let cached = controller.battery.borrow().is_some();
            if let Some(toolbar) = owner.upgrade()
                && toolbar.get_battery_visible()
                && !toolbar.get_battery_activation_key().is_empty()
                && cached
            {
                controller.open_battery(bounds);
            }
        });
        let weak = self.clone();
        toolbar.on_network_requested(move |bounds| weak.open_network_menu(bounds));
        let weak = self.clone();
        toolbar.on_bluetooth_requested(move |bounds| weak.open_bluetooth(bounds));
        let weak = self.clone();
        toolbar.on_keyboard_requested(move |bounds| weak.open_input_language(bounds));
        let controller = self.clone();
        let owner = toolbar.as_weak();
        toolbar.on_tooltip_requested(move |content, bounds| {
            if !controller.bar_input_ready(SurfaceKind::Toolbar) {
                return;
            }
            if let Some(toolbar) = owner.upgrade() {
                controller.show_tooltip(
                    &toolbar,
                    SurfaceKind::Toolbar,
                    &content,
                    bounds,
                    crate::tooltip::Side::Bottom,
                );
            }
        });
        let controller = self.clone();
        toolbar.on_tooltip_dismissed(move |delayed, origin| {
            controller.dismiss_hover_tooltip(SurfaceKind::Toolbar, origin, delayed);
        });
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
        self.show_launcher_tiles();
    }

    /// Native calls and lease destruction never retain a registry borrow.
    fn detach_lease(&self, kind: SurfaceKind) {
        if kind == SurfaceKind::Dock {
            self.application_menu_source.retire();
        }
        let attachment = self.leases.borrow_mut().take(kind);
        drop(attachment);
    }

    #[cfg(test)]
    fn configure_lease(
        &self,
        kind: SurfaceKind,
        window: &slint::Window,
        rect: (i32, i32, u32, u32),
    ) -> Result<(), String> {
        self.configure_lease_if(kind, window, rect, || !self.power_admission_closed.get())
    }

    fn configure_lease_if(
        &self,
        kind: SurfaceKind,
        window: &slint::Window,
        rect: (i32, i32, u32, u32),
        current: impl Fn() -> bool,
    ) -> Result<(), String> {
        self.detach_lease(kind);
        if !current() {
            return Ok(());
        }
        let lease = self.core.host().configure_surface(kind, window)?;
        if !current() {
            drop(lease);
            return Ok(());
        }
        let replaced = self.leases.borrow_mut().store(kind, lease, rect);
        drop(replaced);
        Ok(())
    }

    fn fail_surface(&self, error: String) {
        let message = format!(
            "Surface attach failed: {}",
            sanitize::bounded_text(&error, 200)
        );
        self.report_message(&message);
        *self.surface_failure.borrow_mut() = Some(message);
        let _ = slint::quit_event_loop();
    }

    /// Shows the framed Panel: the native settings/recovery window.
    pub(crate) fn open_panel(&self) {
        if !self.root_current() {
            return;
        }
        let operation = Rc::new(());
        self.popup_operation.replace(Rc::clone(&operation));
        self.retire_shortcut_presentation();
        if self.root_current() && Rc::ptr_eq(&operation, &self.popup_operation.borrow()) {
            self.present_panel(None, &operation);
        }
    }

    fn open_panel_at_shortcut_monitor(&self) {
        let scope = self.shortcut_presentation_scope();
        if !self.shortcut_presentation_current(scope.as_deref()) {
            return;
        }
        let operation = Rc::new(());
        self.popup_operation.replace(Rc::clone(&operation));
        self.present_panel(scope, &operation);
    }

    fn present_panel(
        &self,
        scope: Option<Rc<shortcuts::ShortcutPresentation>>,
        operation: &Rc<()>,
    ) {
        let current = || {
            self.shortcut_presentation_current(scope.as_deref())
                && Rc::ptr_eq(operation, &self.popup_operation.borrow())
        };
        if !current() {
            return;
        }
        self.dismiss_tooltip(false);
        if !current() {
            return;
        }
        if let Some(panel) = self.panel.upgrade() {
            if !panel.window().is_visible() {
                self.stop_startup_root();
                self.prepare_shortcut_draft();
            }
            if !current() {
                return;
            }
            match panel.show() {
                Ok(()) if current() => {
                    if let Err(error) = self.core.host().request_ui_focus(panel.window())
                        && current()
                    {
                        self.report_message(&sanitize::bounded_text(&error, 200));
                        if current() {
                            self.sync_launcher_status();
                        }
                    }
                    if current() {
                        self.sync_startup_root();
                    }
                }
                Ok(()) => {}
                Err(error) if current() => {
                    self.report_message(&format!("Could not show settings: {error}"));
                }
                Err(_) => {}
            }
        }
    }

    fn request_ui_focus(&self, window: &slint::Window) {
        let scope = self.shortcut_presentation_scope();
        if let Err(error) = self.core.host().request_ui_focus(window) {
            if !self.shortcut_presentation_current(scope.as_deref()) {
                return;
            }
            self.report_message(&sanitize::bounded_text(&error, 200));
            self.sync_launcher_status();
        }
    }

    fn launcher_and_upgrade(&self) -> Option<Launcher> {
        self.launcher
            .as_ref()
            .and_then(|launcher| launcher.upgrade())
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
        self.cancel_launcher_input();
        self.hide_launcher_app_menu();
        let panel = self.panel.upgrade()?;
        let worker = Self::refresh_from(&panel, &self.core);
        self.render();
        worker
    }

    /// The single-flight refresh body over Send pieces only (panel weak
    /// handle + core): shared by the controller entry and the notification
    /// bus so the busy/stale/worker logic exists exactly once.
    fn refresh_from(panel: &Panel, core: &Arc<SurfaceCore>) -> Option<std::thread::JoinHandle<()>> {
        if panel.get_refreshing() {
            return None;
        }
        Self::set_busy_on(
            Some(panel),
            true,
            if panel.get_has_snapshot() {
                "Refreshing; activation is paused..."
            } else {
                "Reading the desktop..."
            },
        );
        // While refreshing, activation is disabled even on retained data.
        panel.set_stale(true);

        let worker_core = Arc::clone(core);
        let spawned = std::thread::Builder::new()
            .name("tessera-observation".into())
            .spawn(move || {
                let result = worker_core.observe_safely();
                worker_core.apply_observation(result);
            });
        match spawned {
            Ok(worker) => Some(worker),
            Err(error) => {
                core.apply_observation(Err(format!("Could not start observation: {error}")));
                None
            }
        }
    }

    /// The Send-safe busy setter over the panel alone (used by the
    /// notification-bus refresh entry; mirrors are refreshed by `render`).
    fn set_busy_on(panel: Option<&Panel>, busy: bool, message: &str) {
        let Some(panel) = panel else {
            return;
        };
        panel.set_refreshing(busy);
        if !message.is_empty() {
            panel.set_status_is_feedback(false);
            panel.set_status(message.into());
        }
    }

    /// Writes one status message to every live surface.
    fn report_message(&self, message: &str) {
        if let Some(panel) = self.panel.upgrade() {
            panel.set_status_is_feedback(true);
            panel.set_status(message.into());
        }
        if let Some(dock) = self.dock_and_upgrade() {
            let mut status = dock.get_surface_status();
            status.status = message.into();
            dock.set_surface_status(status);
        }
        self.sync_launcher_status();
        self.sync_toolbar_status();
    }

    /// Reports one host-command outcome to every live surface.
    fn report<F: FnOnce(String) -> String>(
        &self,
        result: Result<(), String>,
        success: &str,
        failure: F,
    ) {
        let feedback_visible = result.is_err();
        let message = match result {
            Ok(()) => success.to_string(),
            Err(error) => failure(sanitize::bounded_text(&error, 200)),
        };
        if let Some(panel) = self.panel.upgrade() {
            panel.set_status_is_feedback(feedback_visible);
            panel.set_status(message.as_str().into());
        }
        if let Some(dock) = self.dock_and_upgrade() {
            let mut status = dock.get_surface_status();
            status.status = message.as_str().into();
            dock.set_surface_status(status);
        }
        self.sync_launcher_status();
        self.sync_toolbar_status();
    }

    /// Mirrors refreshing/stale/status into the launcher surface.
    fn sync_launcher_status(&self) {
        let Some(launcher) = self.launcher_and_upgrade() else {
            return;
        };
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        if panel.get_refreshing() || panel.get_stale() {
            launcher.invoke_cancel_input();
            self.hide_launcher_app_menu();
            self.cancel_launcher_reorder();
        }
        launcher.set_refreshing(panel.get_refreshing());
        launcher.set_stale(panel.get_stale());
        launcher.set_status(panel.get_status());
        launcher.set_feedback_visible(panel.get_status_is_feedback());
        launcher.set_notice(panel.get_startup_notice());
        self.sync_launcher_reorder();
    }

    /// Mirrors status into the toolbar surface.
    fn sync_toolbar_status(&self) {
        let Some(toolbar) = self.toolbar_and_upgrade() else {
            return;
        };
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        toolbar.set_surface_status(DockStatus {
            notice: "".into(),
            status: panel.get_status(),
            refreshing: panel.get_refreshing(),
            stale: panel.get_stale(),
        });
    }

    fn dock_and_upgrade(&self) -> Option<Dock> {
        self.dock.as_ref().and_then(|dock| dock.upgrade())
    }

    fn toolbar_and_upgrade(&self) -> Option<Toolbar> {
        self.toolbar.as_ref().and_then(|toolbar| toolbar.upgrade())
    }

    /// Mirrors the panel's busy/stale/feedback state into the live shell
    /// surfaces and repaints the strip models after every state change.
    fn render(&self) {
        self.dismiss_tooltip(false);
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        self.sync_launcher_status();
        let mut status = dock.get_surface_status();
        status.refreshing = panel.get_refreshing();
        status.stale = panel.get_stale();
        status.status = panel.get_status();
        dock.set_surface_status(status);

        // Identity text on the toolbar; the focused key drives the dock's
        // accent indicator (only keys present in the snapshot are eligible).
        let identity = self.core.identity();
        let focused_key = identity
            .focused_window_key
            .as_deref()
            .filter(|key| {
                self.core.retained_snapshot().is_some_and(|snapshot| {
                    snapshot.windows().iter().any(|window| window.key() == *key)
                })
            })
            .unwrap_or_default();
        if let Some(launcher) = self.launcher_and_upgrade() {
            launcher.set_user_name(identity.user_name.as_str().into());
        }
        if let Some(toolbar) = self.toolbar_and_upgrade() {
            toolbar.set_user_name(identity.user_name.as_str().into());
            toolbar.set_focused_app(
                self.core
                    .retained_snapshot()
                    .and_then(|snapshot| {
                        identity.focused_window_key.as_deref().and_then(|key| {
                            snapshot
                                .windows()
                                .iter()
                                .find(|window| window.key() == key)
                                .map(|window| sanitize::caption(window.title()))
                        })
                    })
                    .unwrap_or_default()
                    .as_str()
                    .into(),
            );
            toolbar.set_clock(identity.clock.as_str().into());
            toolbar.set_language(identity.language.as_str().into());
        }
        dock.set_focused_key(focused_key.into());

        self.refresh_strip(&dock);
        let media = self.dock_media.borrow().clone();
        if let Some(media) = media {
            media.update_applications(&self.core.catalog());
        }
        self.show_launcher_tiles();
        self.update_launcher_geometry();
        self.sync_telemetry_root();
        self.sync_startup_root();
    }

    fn refresh_strip(&self, dock: &Dock) {
        let _application_projection = self.application_menu_source.begin_projection();
        let _middle_projection = self.begin_dock_middle_click_projection();
        self.invalidate_dock_projection();
        let menu = self.menus.borrow().clone();
        if let Some(menu) = menu {
            menu.retire_window_scope();
        }
        let pins = self.core.pins();
        let catalog = self.core.catalog();
        let snapshot = self.core.retained_snapshot();
        let observed: Vec<DockWindow> = snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .windows()
                    .iter()
                    .map(|window| DockWindow {
                        key: window.key().into(),
                        caption: sanitize::caption(window.title()).into(),
                        icon: dock_icon(self, window.icon()),
                    })
                    .collect()
            })
            .unwrap_or_default();
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

        let groups = snapshot
            .as_ref()
            .map(crate::projection::dock_groups)
            .unwrap_or_default();
        let mut metadata: Vec<DockGroup> = Vec::new();
        for members in &groups {
            let first = members[0];
            for member in members {
                metadata.push(DockGroup {
                    key: member.key().into(),
                    identity: member.application_identity().unwrap_or_default().into(),
                    count: members.len().try_into().unwrap_or(i32::MAX),
                    representative: first.key().into(),
                });
            }
        }
        for pin in &pins {
            let identity = format!("aumid:{}", crate::projection::catalog_aumid(pin));
            let matching: Vec<_> = metadata
                .iter()
                .filter(|group| group.identity == identity.as_str())
                .collect();
            if let Some(first) = matching.first() {
                let entry = DockGroup {
                    key: pin.as_str().into(),
                    identity: identity.into(),
                    count: matching.len().try_into().unwrap_or(i32::MAX),
                    representative: first.representative.clone(),
                };
                metadata.push(entry);
            }
        }
        dock.set_group_metadata(ModelRc::new(VecModel::from(metadata)));
        let focused = self.core.identity().focused_window_key;
        let windows: Vec<DockWindow> = groups
            .into_iter()
            .filter_map(|members| {
                let first = members[0];
                let pinned = first.application_identity().and_then(|identity| {
                    pins.iter().find(|pin| {
                        identity == format!("aumid:{}", crate::projection::catalog_aumid(pin))
                            && catalog.iter().any(|app| app.key() == pin.as_str())
                    })
                });
                if let Some(pin) = pinned {
                    if focused
                        .as_deref()
                        .is_some_and(|key| members.iter().any(|window| window.key() == key))
                    {
                        dock.set_focused_key(pin.as_str().into());
                    }
                    return None;
                }
                let mut tile = observed
                    .iter()
                    .find(|window| window.key == first.key())?
                    .clone();
                if members.len() > 1 {
                    tile.caption = format!(
                        "{} windows\n{}",
                        members.len(),
                        members
                            .iter()
                            .map(|window| sanitize::caption(window.title()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                    .into();
                }
                if focused
                    .as_deref()
                    .is_some_and(|key| members.iter().any(|window| window.key() == key))
                {
                    dock.set_focused_key(first.key().into());
                }
                Some(tile)
            })
            .collect();
        // Every member stays in the retained model used for action admission.
        dock.set_observed_windows(ModelRc::new(VecModel::from(observed)));
        dock.set_running_windows(ModelRc::new(VecModel::from(windows)));
        self.invalidate_dock_projection();
        self.sync_dock_middle_click();
    }

    /// Places the dock and toolbar windows inside the monitor bounds and
    /// applies the fullscreen hide. Called after show (so the monitor's DPI
    /// scale is known), after every successful observation, and immediately
    /// after an explicit Save that changed the edge.
    ///
    /// Re-reservation is change-driven: only when the desired physical rect
    /// differs from the last attached one is the old lease dropped and the
    /// surface re-attached against the new RECT (the native side reserves at
    /// attach time only). Unchanged-geometry observations touch nothing
    /// native. Visibility is independent of placement: unchanged observations
    /// must not reshow an overlap-hidden dock. Fullscreen still suppresses both.
    fn place_geometry(&self, context: crate::DockContext) -> Result<bool, String> {
        self.dismiss_tooltip(false);
        let Some(dock) = self.dock_and_upgrade() else {
            return Ok(false);
        };
        let scale = dock.window().scale_factor();
        let tile_count =
            // A 136px module occupies exactly three 40px/8px virtual slots.
            dock.get_pinned_apps().row_count() + dock.get_running_windows().row_count()
                + if dock.get_media_view().enabled { 3 } else { 0 };
        // Read the live draft directly: an already visible bar must not need
        // another show() merely to flush a deferred Panel.changed handler.
        let compact = self
            .panel
            .upgrade()
            .map(|panel| panel.get_compact())
            .unwrap_or_else(|| self.core.applied_preferences().compact());
        dock.set_compact(compact);
        // Preview positioning uses the current UI edge; pin saves still use
        // the independent last-saved edge from the core.
        let edge = self
            .panel
            .upgrade()
            .map(|panel| crate::dock_edge_from_index(panel.get_dock_edge_index()))
            .unwrap_or_else(|| self.core.applied_dock_edge());
        dock.set_edge(crate::dock_edge_to_index(edge));
        let rect = crate::dock::dock_rect(context, edge, tile_count, compact, scale);
        let quick = self.quick_settings.borrow().clone();
        if let Some(quick) = quick {
            quick.close_if_geometry_changed(context, scale);
        }
        let user = self.user_menu.borrow().clone();
        if let Some(user) = user
            && let Some(launcher) = self.launcher_and_upgrade()
            && let Some(context) = self.launcher_context()
        {
            user.close_if_geometry_changed(context, launcher.window().scale_factor());
        }
        let calendar = self.calendar.borrow().clone();
        if let Some(calendar) = calendar
            && let Some(toolbar) = self.toolbar_and_upgrade()
        {
            calendar.close_if_geometry_changed(context, toolbar.window().scale_factor());
        }
        if let Some(toolbar) = self.toolbar_and_upgrade() {
            self.toolbar_popup_geometry(context, toolbar.window().scale_factor());
        }

        let toolbar_rect = crate::dock::toolbar_rect(context, scale);
        let visible = self.visibility_for_geometry(context, rect, toolbar_rect, edge, scale)?;
        self.place_bar(SurfaceKind::Dock, dock.window(), rect, visible.dock, None)?;
        if let Some(toolbar) = self.toolbar_and_upgrade() {
            self.place_bar(
                SurfaceKind::Toolbar,
                toolbar.window(),
                toolbar_rect,
                visible.toolbar,
                None,
            )?;
        }
        Ok(true)
    }

    fn update_geometry(&self) {
        if let Err(error) = self.apply_geometry(self.core.dock_context()) {
            self.fail_surface(error);
        }
        self.update_launcher_geometry();
    }

    /// Apply the shared live color/density/edge preview without saving it.
    fn sync_appearance(&self) {
        if !self.root_current() {
            return;
        }
        let current_theme = crate::theme_from_index(
            self.panel
                .upgrade()
                .map(|panel| panel.get_theme_index())
                .unwrap_or(0),
        );
        let scheme = match current_theme {
            crate::Theme::Light => slint::language::ColorScheme::Light,
            crate::Theme::Dark => slint::language::ColorScheme::Dark,
            crate::Theme::System => slint::language::ColorScheme::Unknown,
        };
        let seed = self
            .panel
            .upgrade()
            .and_then(|panel| u32::try_from(panel.get_source_rgb()).ok())
            .and_then(crate::SourceSeed::from_rgb);
        let Some(seed) = seed else {
            return;
        };
        let current = || {
            self.root_current()
                && self.panel.upgrade().is_some_and(|panel| {
                    panel.get_source_rgb() == seed.rgb() as i32
                        && crate::theme_from_index(panel.get_theme_index()) == current_theme
                })
        };
        macro_rules! apply {
            ($effect:expr) => {
                if !current() {
                    return;
                }
                $effect;
                if !current() {
                    return;
                }
            };
        }
        let theme = PresentationTheme::from_source(scheme, seed);
        if let Some(panel) = self.panel.upgrade() {
            apply!(panel.set_source_hex(format!("#{:06x}", seed.rgb()).into()));
            apply!(
                panel.set_source_preset_index(
                    SOURCE_PRESETS
                        .iter()
                        .position(|rgb| *rgb == seed.rgb())
                        .map_or(-1, |index| index as i32)
                )
            );
            panel.apply_presentation_theme_scoped(theme.clone(), current);
        }
        if let Some(dock) = self.dock_and_upgrade() {
            dock.apply_presentation_theme_scoped(theme.clone(), current);
            if let Some(panel) = self.panel.upgrade() {
                apply!(dock.set_compact(panel.get_compact()));
            }
        }
        if let Some(toolbar) = self.toolbar_and_upgrade() {
            toolbar.apply_presentation_theme_scoped(theme.clone(), current);
        }
        if let Some(launcher) = self.launcher_and_upgrade() {
            launcher.apply_presentation_theme_scoped(theme.clone(), current);
        }
        let quick = self.quick_settings.borrow().clone();
        if let Some(quick) = quick {
            apply!(quick.apply_theme(theme.clone()));
        }
        let user = self.user_menu.borrow().clone();
        if let Some(user) = user {
            apply!(user.apply_theme(theme.clone()));
        }
        let calendar = self.calendar.borrow().clone();
        if let Some(calendar) = calendar {
            apply!(calendar.apply_theme(theme.clone()));
        }
        let power = self.power_menu.borrow().clone();
        if let Some(power) = power {
            apply!(power.set_theme(theme.clone()));
            apply!(power.update_motion());
        }
        let menu = self.menus.borrow().clone();
        if let Some(menu) = menu {
            apply!(menu.apply_theme(theme.clone()));
        }
        let tooltip = self.tooltips.borrow().clone();
        if let Some(tooltip) = tooltip {
            apply!(tooltip.apply_theme(theme.clone()));
        }
        apply!(self.toolbar_popup_theme(theme));
        apply!(self.hide_launcher_app_menu());
        apply!(self.update_geometry());
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
        self.launch_resolved_app(&key);
    }

    fn launch_resolved_app(&self, key: &str) -> bool {
        let result = self.core.host().launch(key);
        let accepted = result.is_ok();
        self.report(result, "Application launched", |detail| {
            format!("Launch failed: {detail}")
        });
        accepted
    }

    /// Toggles one pin and persists it immediately — reusing the last saved
    /// appearance values so the live appearance preview is never persisted by
    /// a pin click. On save failure the pin state reverts and stays visible.
    pub(crate) fn toggle_pin(&self, key: &str, pin: bool) {
        let Some(_save) = self.begin_preference_save() else {
            return;
        };
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
        let preferences = self
            .core
            .applied_preferences()
            .with_dock(self.core.applied_dock_edge(), pins);
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
                self.show_launcher_tiles();
                self.render();
                self.update_geometry();
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

    /// Saves appearance, General policy and module drafts as one complete
    /// applied record. Native modules adopt changes only after storage succeeds.
    pub(crate) fn save_preferences(&self) {
        let Some(_save) = self.begin_preference_save() else {
            return;
        };
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let Some(seed) = u32::try_from(panel.get_source_rgb())
            .ok()
            .and_then(crate::SourceSeed::from_rgb)
        else {
            self.report_message("Could not save preferences: invalid RGB source.");
            return;
        };
        let Some(start) = crate::StartOfWeek::from_index(panel.get_start_of_week_index()) else {
            self.report_message("Could not save preferences: invalid start-of-week choice.");
            return;
        };
        let Some(middle) =
            crate::DockMiddleClickAction::from_index(panel.get_dock_middle_click_index())
        else {
            self.report_message("Could not save preferences: invalid middle-click choice.");
            return;
        };
        let Some(visibility) = self.draft_bar_visibility_preferences(&panel) else {
            self.report_message("Could not save preferences: invalid auto-hide choice.");
            return;
        };
        let preferences = self
            .core
            .applied_preferences()
            .with_appearance(
                crate::theme_from_index(panel.get_theme_index()),
                panel.get_compact(),
                crate::dock_edge_from_index(panel.get_dock_edge_index()),
            )
            .with_source_seed(seed)
            .with_general(crate::GeneralPreferences::default().with_start_of_week(start))
            .with_dock_middle_click(middle)
            .with_telemetry_enabled(panel.get_telemetry_enabled())
            .with_bar_visibility(visibility)
            .with_shortcuts(self.shortcut_draft());
        match self.core.host().save_preferences(&preferences) {
            Ok(()) => {
                self.core.record_applied(&preferences);
                if !self.root_current() {
                    return;
                }
                self.shortcuts_saved(&preferences);
                if !self.root_current() {
                    return;
                }
                let calendar = self.calendar.borrow().clone();
                if let Some(calendar) = calendar {
                    calendar.set_start_of_week(start);
                }
                if !self.root_current() {
                    return;
                }
                panel.set_status("Preferences saved".into());
                panel.set_status_is_feedback(false);
                if !self.root_current() {
                    return;
                }
                self.update_geometry();
                if !self.root_current() {
                    return;
                }
                self.render();
            }
            Err(error) => {
                if !self.root_current() {
                    return;
                }
                let detail = sanitize::bounded_text(&error, 200);
                panel.set_status(format!("Could not save preferences: {detail}").into());
                panel.set_status_is_feedback(true);
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
        panel_key
            .or_else(|| self.resolve_launcher_key(key))
            .or_else(|| {
                self.dock_and_upgrade().and_then(|dock| {
                    model_key(&dock.get_pinned_apps(), key, |app| app.key.to_string())
                })
            })
    }

    /// UI-thread routes for the shared notification bus. A gone panel reports
    /// busy so the bus never spawns work after the surface is dropped.
    ///
    /// Send+Sync closures capture only weak component handles. The scheduled
    /// UI callback owns the controller; its thread-affine state never crosses
    /// a thread, and the core never strongly captures itself in stored routes.
    pub(crate) fn routes(&self) -> Routes {
        let busy_panel = self.panel.clone();
        let refresh_panel = self.panel.clone();
        // Route back through the component callback; never capture the core
        // strongly inside its own stored routes or send UI-only leases.
        Routes {
            is_refreshing: Arc::new(move || {
                busy_panel
                    .upgrade()
                    .map(|panel| panel.get_refreshing())
                    .unwrap_or(true)
            }),
            start_refresh: Arc::new(move || {
                if let Some(panel) = refresh_panel.upgrade() {
                    panel.invoke_refresh_requested();
                }
            }),
        }
    }
}

/// Builds the strip icon through the controller's shared icon cache.
fn dock_icon(controller: &PanelController, icon: Option<&crate::PixelIcon>) -> slint::Image {
    controller
        .icon_cache
        .borrow_mut()
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
    controller.cancel_launcher_reorder();
    match result {
        Ok(snapshot) => {
            // Identity text arrives with each successful observation; the
            // focused key is validated against this snapshot in `render`.
            controller.core.refresh_identity();
            let projection = crate::projection::project(&snapshot, &panel.get_search());
            // Publish the fresh catalog before projecting application rows.
            controller.core.store_snapshot(snapshot);
            show(panel, &projection);
            controller.show_apps(panel, &panel.get_search());
            panel.set_status(projection.status.as_str().into());
            panel.set_status_is_feedback(false);
            panel.set_has_snapshot(true);
            panel.set_stale(false);
        }
        Err(error) => {
            let prefix = if panel.get_has_snapshot() {
                "Refresh failed; retained data is stale"
            } else {
                "Could not observe the desktop"
            };
            let detail = sanitize::bounded_text(&error, 200);
            panel.set_status(format!("{prefix}: {detail}. Try Refresh.").into());
            panel.set_status_is_feedback(true);
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

/// Apply a fresh observation on the UI thread and announce readiness only
/// after real monitor geometry and native bar leases are established.
fn apply_result_to_both(
    controller: &PanelController,
    panel: &Panel,
    result: Result<PanelSnapshot, String>,
) {
    let fresh = result.is_ok();
    apply_result(controller, panel, result);
    controller.render();
    if !fresh {
        return;
    }
    let context = controller.core.dock_context();
    match controller.apply_geometry(context) {
        Err(error) => {
            controller.fail_surface(error);
        }
        Ok(applied) => {
            if applied {
                // Real monitor geometry applied + bar leases attached:
                // the immediate readiness pulse (steps 4-5 of the startup
                // order; arm() is one-shot so later observations are no-ops).
                controller.pulse_readiness();
            }
        }
    }
}

/// Builds the requested surfaces, installs callbacks, and runs the loop.
///
/// Panel mode runs the panel normally (closing it quits). Dock mode shows the
/// dock strip, the top toolbar, and the frameless launcher, keeps the loop
/// alive with `run_event_loop_until_quit`: hiding any surface (fullscreen,
/// Escape in the launcher) must never terminate the UI or the watchdog; exit
/// is an explicit Exit button, which the host supervisor follows by restoring
/// the Explorer shell.
///
/// The heartbeat stays unarmed until startup is provably ready: the first
/// successful observation whose real monitor geometry applied and whose bar
/// leases attached. Setup failure exits for immediate taskbar restoration;
/// never-arriving geometry instead reaches the supervisor's startup timeout.
pub(crate) fn run(
    host: Arc<Host>,
    preferences: PanelPreferences,
    startup_notice: Option<String>,
    run_options: RunOptions,
) -> Result<(), slint::PlatformError> {
    let panel = Panel::new()?;
    panel.set_theme_index(crate::theme_to_index(preferences.theme()));
    panel.set_source_rgb(preferences.source_seed().rgb() as i32);
    panel.set_source_hex(format!("#{:06x}", preferences.source_seed().rgb()).into());
    panel.set_compact(preferences.compact());
    panel.set_dock_edge_index(crate::dock_edge_to_index(preferences.dock_edge()));
    panel.set_start_of_week_index(preferences.general().start_of_week().index());
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

    // The heartbeat is created but never armed on failure paths; the timer
    // stays retained until the loop ends (the drop below makes that
    // explicit). In dock mode the controller arms it after the first
    // successful observation with real geometry and both bar leases.
    let heartbeat = std::rc::Rc::new(std::cell::RefCell::new(crate::Heartbeat::unarmed(
        &run_options,
    )));
    // The readiness-pulse armer: a UI-thread panel callback (the heartbeat
    // is deliberately not Send; the Send observation route reaches it only
    // through this indirection). `Heartbeat::arm` is one-shot (it takes the
    // closure), so repeated invocations are harmless.
    {
        let heartbeat = std::rc::Rc::clone(&heartbeat);
        panel.on_readiness_pulse(move || heartbeat.borrow_mut().arm());
    }

    match run_options.surface {
        crate::SurfaceMode::Panel => {
            let controller = PanelController::new(&panel, Arc::clone(&core));
            controller.sync_appearance();
            core.install_routes(controller.routes());
            controller.apply_filter();
            let _capability_scope = shortcuts::RootCapabilityScope::new(&controller);
            controller.start_shortcuts(run_options.global_shortcuts_enabled);
            // Panel mode pulses immediately: its only surface is ready.
            heartbeat.borrow_mut().arm();
            let _ = controller.refresh();
            panel.run()?;
        }
        crate::SurfaceMode::Dock => {
            let dock = Dock::new()?;
            let toolbar = Toolbar::new()?;
            let launcher = Launcher::new()?;
            dock.set_compact(preferences.compact());
            // Single-flight starts idle; refresh() below owns the busy flag.
            let initial_status = DockStatus {
                notice: "".into(),
                status: "Ready to observe".into(),
                refreshing: false,
                stale: false,
            };
            dock.set_surface_status(initial_status.clone());
            toolbar.set_surface_status(initial_status);
            launcher.set_version(env!("CARGO_PKG_VERSION").into());
            launcher.set_status("Ready to observe".into());
            launcher.set_notice(panel.get_startup_notice());
            let controller = PanelController::new_with_dock(
                &panel,
                &dock,
                &toolbar,
                &launcher,
                Arc::clone(&core),
            );
            // Drop native leases before any component/HWND is destroyed, even
            // if another component callback still retains the controller.
            let _surface_scope = SurfaceLeaseScope(Rc::clone(&controller.leases));
            let _dock_utility_scope = crate::transient_window::TransientScope::new(
                Rc::clone(&controller.dock_utilities),
                crate::dock_utilities::DockUtilitiesController::close,
            );
            let _recycle_bin_scope = crate::transient_window::TransientScope::new(
                Rc::clone(&controller.recycle_bin),
                crate::recycle_bin::RecycleBinController::close,
            );
            let _menu_scope = crate::transient_window::TransientScope::new(
                Rc::clone(&controller.menus),
                crate::context_menu::ContextMenuController::hide,
            );
            let _quick_scope = crate::transient_window::TransientScope::new(
                Rc::clone(&controller.quick_settings),
                crate::quick_settings::QuickSettingsController::hide,
            );
            let _user_scope = crate::transient_window::TransientScope::new(
                Rc::clone(&controller.user_menu),
                crate::user_menu::UserMenuController::hide,
            );
            let _calendar_scope = crate::transient_window::TransientScope::new(
                Rc::clone(&controller.calendar),
                crate::calendar::CalendarController::hide,
            );
            let _tooltip_scope = crate::transient_window::TransientScope::new(
                Rc::clone(&controller.tooltips),
                crate::tooltip::TooltipController::hide,
            );
            let _toolbar_popup_scope = native_toolbar::ToolbarPopupScope::new(&controller);
            let _launcher_app_scope = crate::transient_window::TransientScope::new(
                Rc::clone(&controller.launcher_app_menu),
                crate::launcher::LauncherAppMenu::hide,
            );
            let _media_scope = crate::transient_window::TransientScope::new(
                Rc::clone(&controller.dock_media),
                crate::dock_media::DockMediaController::close,
            );
            let _visibility_scope = crate::transient_window::TransientScope::new(
                Rc::clone(&controller.visibility),
                crate::visibility::VisibilityController::close,
            );
            let _motion_scope = match crate::motion::MotionSubscription::new(
                core.host().as_ref(),
                &panel,
                Panel::invoke_motion_policy_changed,
            ) {
                Ok(scope) => Some(scope),
                Err(error) => {
                    controller.report_message(&format!(
                        "System motion notifications unavailable: {error}"
                    ));
                    None
                }
            };
            core.install_routes(controller.routes());
            controller.apply_filter();
            controller.sync_appearance();
            // The launcher starts hidden; the dock's start tile shows it.
            // Toolbar settings has its own independently loaded audio popup.
            // Closing a bar ends the session; hiding a launcher drops its
            // thread-affine native lease before the HWND can disappear.
            for window in [dock.window(), toolbar.window()] {
                window.on_close_requested(|| {
                    let _ = slint::quit_event_loop();
                    slint::CloseRequestResponse::KeepWindowShown
                });
            }
            // The clock timer refreshes only the toolbar's clock text.
            let _clock_guard = crate::start_clock_timer(&core, {
                let toolbar = toolbar.as_weak();
                std::sync::Arc::new(move |clock| {
                    if let Some(toolbar) = toolbar.upgrade() {
                        toolbar.set_clock(clock.as_str().into());
                    }
                })
            });
            // Exact startup order (contract): (1) show the recovery-capable
            // bars with NO leases and NO pulse; (2) the single-flight first
            // observation runs asynchronously (the UI thread stays
            // responsive); (3) its completion applies the REAL monitor
            // position/size and then (4) attaches both bar leases through
            // the UI-thread registry, and (5) pulses and arms the timer
            // immediately. No provisional/default appbar registration ever
            // happens. Attach errors exit for immediate restoration; geometry
            // that never arrives instead reaches the supervisor's timeout.
            // Retire Root/global delivery and the existing Power gate before native teardown.
            let _power_scope = power_menu::PowerAdmissionScope::new(&controller);
            let _capability_scope = shortcuts::RootCapabilityScope::new(&controller);
            controller.start_shortcuts(run_options.global_shortcuts_enabled);
            dock.show()?;
            toolbar.show()?;
            // (2) Async first observation: its completion handler does
            // steps (3)-(5): real geometry, bar leases, immediate pulse.
            let _ = controller.refresh();
            slint::run_event_loop_until_quit()?;
            if let Some(error) = controller.surface_failure.borrow_mut().take() {
                return Err(slint::PlatformError::Other(error));
            }
        }
    }
    // The heartbeat timer is retained until the loop has ended; dropping it
    // earlier would silently stop the two-second watchdog signal.
    drop(heartbeat);
    Ok(())
}

#[cfg(test)]
mod tests;
