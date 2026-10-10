// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Separately owned native context-menu window; no modal platform menu loop.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelExt, SharedString};

use crate::generated::{
    ContextMenuSurface, Dock, DockMenuAction, DockMenuKind, DockRecycleAction, DockSystemCommand,
    DockWindowCommand, MenuEntry,
};
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::transient_window::TransientWindow;
use crate::{DesktopHost, DockContext, SurfaceKind};

use crate::popup_placement as placement;
#[cfg(test)]
mod tests;

impl ContextMenuSurface {
    pub(crate) fn new_with_metrics() -> Result<Self, slint::PlatformError> {
        let surface = Self::new()?;
        surface.on_metric_labels(|entries, decorated| {
            entries.model_tracker().track_row_count_changes();
            let mut labels = String::new();
            for row in 0..entries.row_count() {
                if let Some(entry) = entries
                    .row_data_tracked(row)
                    .filter(|entry| (entry.icon.size().width > 0) == decorated)
                {
                    labels.push_str(entry.label.as_str());
                    labels.push('\n');
                }
            }
            labels.into()
        });
        Ok(surface)
    }
}

pub(crate) struct ContextMenuController {
    surface: TransientWindow<ContextMenuSurface>,
    dock: slint::Weak<Dock>,
    key: RefCell<SharedString>,
    scope_generation: Cell<Option<u64>>,
    pin_order: RefCell<Vec<String>>,
    lock_desired: Cell<Option<bool>>,
    focus_seen: Cell<bool>,
    focus_watch: slint::Timer,
    host: Arc<dyn DesktopHost>,
    preview: RefCell<Option<Box<dyn std::any::Any>>>,
    preview_key: RefCell<SharedString>,
    preview_bounds: Cell<Option<tessera_core::Rect>>,
    preview_generation: Cell<Option<u64>>,
}

impl ContextMenuController {
    pub(crate) fn is_open(&self) -> bool {
        self.surface.is_visible()
    }

    pub(crate) fn retire_window_scope(&self) {
        if self.is_open()
            && matches!(
                self.surface.get_kind(),
                DockMenuKind::Window | DockMenuKind::Pinned
            )
        {
            self.hide();
        }
    }

    pub(crate) fn apply_theme(&self, theme: PresentationTheme) {
        let generation = self.scope_generation.get();
        self.surface.apply_presentation_theme_scoped(theme, || {
            generation.is_some()
                && self.scope_generation.get() == generation
                && self.dock.upgrade().is_some()
        });
    }
    #[cfg(test)]
    pub(crate) fn component(&self) -> &ContextMenuSurface {
        &self.surface
    }
    pub(crate) fn new(
        host: Arc<dyn DesktopHost>,
        dock: &Dock,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let menu = Rc::new(Self {
            surface: TransientWindow::new(
                Arc::clone(&host),
                ContextMenuSurface::new_with_metrics()?,
                SurfaceKind::Popup,
            ),
            dock: dock.as_weak(),
            key: RefCell::default(),
            scope_generation: Cell::new(Some(0)),
            pin_order: RefCell::default(),
            lock_desired: Cell::new(None),
            focus_seen: Cell::new(false),
            focus_watch: slint::Timer::default(),
            host,
            preview: RefCell::default(),
            preview_key: RefCell::default(),
            preview_bounds: Cell::new(None),
            preview_generation: Cell::new(Some(0)),
        });
        let weak = Rc::downgrade(&menu);
        menu.surface.on_action_requested(move |action| {
            if let Some(menu) = weak.upgrade() {
                menu.execute(action);
            }
        });
        let weak = Rc::downgrade(&menu);
        menu.surface.on_window_action_requested(move |key, action| {
            if let Some(menu) = weak.upgrade() {
                menu.execute_member(&key, action);
            }
        });
        let weak = Rc::downgrade(&menu);
        menu.surface.on_preview_changed(move || {
            if let Some(menu) = weak.upgrade() {
                menu.update_preview();
            }
        });
        let weak = Rc::downgrade(&menu);
        menu.surface.on_dismiss_requested(move || {
            if let Some(menu) = weak.upgrade() {
                menu.hide();
            }
        });
        let weak = Rc::downgrade(&menu);
        menu.surface.window().on_close_requested(move || {
            if let Some(menu) = weak.upgrade() {
                menu.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        Ok(menu)
    }

    pub(crate) fn show(
        self: &Rc<Self>,
        kind: DockMenuKind,
        key: SharedString,
        anchor: slint::PhysicalPosition,
        context: DockContext,
    ) -> Result<(), String> {
        self.show_scope(kind, key, anchor, context, false)
    }

    /// Context for the displayed optional module, not a playback command.
    pub(crate) fn show_media(
        self: &Rc<Self>,
        dock: &Dock,
        logical_anchor: slint::LogicalPosition,
        context: DockContext,
    ) -> Result<(), String> {
        let owned = self
            .dock
            .upgrade()
            .is_some_and(|owned| std::ptr::eq(owned.window(), dock.window()));
        if !owned || !dock.window().is_visible() || !dock.get_media_view().enabled {
            return Err("The media module is no longer available.".into());
        }
        let anchor = placement::physical_anchor(
            dock.window().position(),
            dock.window().scale_factor(),
            (logical_anchor.x, logical_anchor.y),
        )?;
        self.show_scope(
            DockMenuKind::Bar,
            SharedString::default(),
            anchor,
            context,
            true,
        )
    }

    fn show_scope(
        self: &Rc<Self>,
        kind: DockMenuKind,
        key: SharedString,
        anchor: slint::PhysicalPosition,
        context: DockContext,
        media_scope: bool,
    ) -> Result<(), String> {
        let retired_generation = self
            .scope_generation
            .get()
            .and_then(|generation| generation.checked_add(1));
        self.hide();
        // Lease Drop may publish a newer scope before this payload is applied.
        // At exhaustion, a visible replacement still wins; ordinary shows remain usable.
        if self.scope_generation.get() != retired_generation || self.is_open() {
            return Ok(());
        }
        let dock = self
            .dock
            .upgrade()
            .ok_or("The dock is no longer available.")?;
        if media_scope && (!dock.window().is_visible() || !dock.get_media_view().enabled) {
            return Err("The media module is no longer available.".into());
        }
        let key = if kind == DockMenuKind::Recycle {
            SharedString::default()
        } else {
            key
        };
        self.surface.set_kind(kind);
        self.surface.set_launcher_favorite_scope(false);
        self.surface.set_media_scope(media_scope);
        let lock_available =
            kind == DockMenuKind::Bar && !media_scope && dock.get_reorder_lock_available();
        self.surface.set_dock_lock_available(lock_available);
        self.surface
            .set_dock_lock_enabled(dock.get_reorder_lock_enabled());
        self.surface.set_dock_locked(dock.get_reorder_locked());
        self.lock_desired
            .set(lock_available.then_some(!dock.get_reorder_locked()));
        let pin_order: Vec<_> = dock
            .get_pinned_apps()
            .iter()
            .map(|app| app.key.to_string())
            .collect();
        let pin_index = pin_order.iter().position(|pin| pin == key.as_str());
        self.surface.set_pin_move_earlier_enabled(
            kind == DockMenuKind::Pinned
                && dock.get_reorder_enabled()
                && pin_index.is_some_and(|index| index > 0),
        );
        self.surface.set_pin_move_later_enabled(
            kind == DockMenuKind::Pinned
                && dock.get_reorder_enabled()
                && pin_index.is_some_and(|index| index + 1 < pin_order.len()),
        );
        self.pin_order.replace(pin_order);
        let metadata = dock.get_group_metadata();
        let group = metadata.iter().find(|group| group.key == key);
        let members: Vec<_> = if matches!(kind, DockMenuKind::Pinned | DockMenuKind::Window) {
            dock.get_observed_windows()
                .iter()
                .filter(|window| {
                    group.as_ref().is_some_and(|group| {
                        metadata.iter().any(|member| {
                            member.key == window.key
                                && (member.key == group.key
                                    || (!group.identity.is_empty()
                                        && member.identity == group.identity))
                        })
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut entries = Vec::new();
        if !members.is_empty() {
            for member in members {
                for (action, prefix) in [
                    (DockMenuAction::Activate, ""),
                    (DockMenuAction::Minimize, "Minimize: "),
                    (DockMenuAction::Close, "Close: "),
                ] {
                    entries.push(MenuEntry {
                        label: format!("{prefix}{}", member.caption).into(),
                        action,
                        icon: member.icon.clone(),
                        application_image: true,
                        danger: action == DockMenuAction::Close,
                        window_key: member.key.clone(),
                    });
                }
            }
            if kind == DockMenuKind::Pinned {
                for (action, label) in [
                    (DockMenuAction::Launch, "Open new instance"),
                    (DockMenuAction::Unpin, "Unpin"),
                    (DockMenuAction::MoveEarlier, "Move earlier"),
                    (DockMenuAction::MoveLater, "Move later"),
                ] {
                    if matches!(
                        action,
                        DockMenuAction::MoveEarlier | DockMenuAction::MoveLater
                    ) && !self.surface.get_pin_move_earlier_enabled()
                        && !self.surface.get_pin_move_later_enabled()
                    {
                        continue;
                    }
                    entries.push(MenuEntry {
                        action,
                        label: label.into(),
                        ..Default::default()
                    });
                }
            }
        }
        self.surface
            .set_window_entries(slint::ModelRc::new(slint::VecModel::from(entries)));
        self.surface
            .set_media_enabled(kind == DockMenuKind::Bar && dock.get_media_view().enabled);
        // Reuse only already-resolved typed dock images; no host extraction.
        let target_icon = match kind {
            DockMenuKind::Pinned => dock
                .get_pinned_apps()
                .iter()
                .find(|app| app.key == key)
                .map(|app| app.icon),
            DockMenuKind::Window => dock
                .get_running_windows()
                .iter()
                .find(|window| window.key == key)
                .map(|window| window.icon),
            DockMenuKind::Bar | DockMenuKind::Recycle => None,
        };
        self.surface
            .set_target_icon(target_icon.unwrap_or_default());
        self.surface.set_recycle_empty_enabled(
            kind == DockMenuKind::Recycle && dock.get_recycle_empty_enabled(),
        );
        self.surface.set_selected_index(0);
        *self.key.borrow_mut() = key;
        self.surface
            .apply_presentation_theme(dock.presentation_theme());
        let rect = placement::place(
            context,
            anchor,
            (
                self.surface.get_menu_width(),
                self.surface.get_menu_height(),
            ),
            dock.window().scale_factor(),
        )?;
        let generation = self.scope_generation.get();
        if !self.surface.present(rect.position, rect.size)? {
            return Ok(());
        }
        if self.scope_generation.get() != generation {
            return Ok(());
        }
        if media_scope && (!dock.window().is_visible() || !dock.get_media_view().enabled) {
            self.hide();
            return Ok(());
        }
        self.refresh_recycle_actions();
        self.update_preview();
        if self.scope_generation.get() != generation || !self.is_open() {
            return Ok(());
        }
        self.surface.invoke_focus_menu();
        let focus = self.surface.request_focus();
        self.focus_seen.set(self.is_focused() == Some(true));
        // Only the owned popup's cached focus is sampled, only while open.
        // No desktop scan, foreign hook, forced focus, or idle observer work.
        if self.is_focused().is_some() {
            let weak = Rc::downgrade(self);
            self.focus_watch.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(100),
                move || {
                    if let Some(menu) = weak.upgrade() {
                        match menu.is_focused() {
                            Some(true) => menu.focus_seen.set(true),
                            Some(false) if menu.focus_seen.get() => menu.hide(),
                            _ => {}
                        }
                    }
                },
            );
        }
        focus.map_err(|error| format!("Menu opened, but keyboard focus was not granted: {error}"))
    }

    pub(crate) fn hide(&self) {
        self.scope_generation.set(
            self.scope_generation
                .get()
                .and_then(|generation| generation.checked_add(1)),
        );
        let generation = self.scope_generation.get();
        self.clear_preview();
        if self.scope_generation.get() != generation {
            return;
        }
        self.surface.set_recycle_empty_enabled(false);
        self.focus_watch.stop();
        self.surface.hide();
    }

    fn clear_preview(&self) {
        self.preview_generation.set(
            self.preview_generation
                .get()
                .and_then(|generation| generation.checked_add(1)),
        );
        self.preview_key.replace(SharedString::default());
        self.preview_bounds.set(None);
        // No RefCell borrow survives an arbitrary host lease's Drop.
        let lease = self.preview.borrow_mut().take();
        drop(lease);
    }

    fn selected_preview_key(&self) -> Option<SharedString> {
        let index = usize::try_from(self.surface.get_selected_index()).ok()?;
        self.surface
            .get_window_entries()
            .row_data(index)
            .map(|entry| entry.window_key)
            .filter(|key| !key.is_empty())
    }

    fn preview_current(
        &self,
        scope: u64,
        operation: u64,
        key: &str,
        bounds: tessera_core::Rect,
    ) -> bool {
        self.scope_generation.get() == Some(scope)
            && self.preview_generation.get() == Some(operation)
            && self.is_open()
            && self.surface.window().is_visible()
            && self.selected_preview_key().as_deref() == Some(key)
            && preview_rect(self.surface.window(), self.surface.get_preview_bounds()).ok()
                == Some(bounds)
            && self.dock.upgrade().is_some_and(|dock| {
                let status = dock.get_surface_status();
                dock.window().is_visible()
                    && !status.refreshing
                    && !status.stale
                    && dock.displayed_window_key(key).is_some()
            })
    }

    fn update_preview(&self) {
        let Some(scope) = self.scope_generation.get() else {
            return;
        };
        if !self.is_open() || !self.surface.window().is_visible() {
            return;
        }
        let key = self.selected_preview_key();
        let bounds = preview_rect(self.surface.window(), self.surface.get_preview_bounds());
        if key
            .as_ref()
            .is_some_and(|key| *self.preview_key.borrow() == *key)
            && bounds.as_ref().ok().copied() == self.preview_bounds.get()
            && self.preview.borrow().is_some()
        {
            return;
        }
        let operation = self
            .preview_generation
            .get()
            .and_then(|generation| generation.checked_add(1));
        self.clear_preview();
        if self.scope_generation.get() != Some(scope)
            || self.preview_generation.get() != operation
            || !self.is_open()
        {
            return;
        }
        let Some(key) = key else {
            self.surface
                .set_preview_notice("Select a window to preview.".into());
            return;
        };
        let bounds = match bounds {
            Ok(bounds) => bounds,
            Err(error) => {
                self.surface.set_preview_notice(error.into());
                return;
            }
        };
        let Some(operation) = operation else { return };
        if !self.preview_current(scope, operation, &key, bounds) {
            return;
        }
        let result = self
            .host
            .window_preview(&key, self.surface.window(), bounds);
        if !self.preview_current(scope, operation, &key, bounds) {
            drop(result);
            return;
        }
        match result {
            Ok(Some(lease)) => {
                self.preview_key.replace(key);
                self.preview_bounds.set(Some(bounds));
                self.preview.replace(Some(lease));
                self.surface.set_preview_notice(SharedString::default());
            }
            Ok(None) => self
                .surface
                .set_preview_notice("Window previews are unavailable.".into()),
            Err(error) => self.surface.set_preview_notice(
                format!(
                    "Preview unavailable: {}",
                    crate::sanitize::bounded_text(&error, 160)
                )
                .into(),
            ),
        }
    }

    /// Refresh retained recycle presentation without granting or presenting a scope.
    pub(crate) fn refresh_recycle_actions(&self) {
        if self.surface.get_kind() == DockMenuKind::Recycle {
            self.surface.set_recycle_empty_enabled(
                self.scope_generation.get().is_some()
                    && self.dock.upgrade().is_some_and(|dock| {
                        dock.window().is_visible() && dock.get_recycle_empty_enabled()
                    }),
            );
        }
    }
    pub(crate) fn disable_motion(&self) {
        self.surface.disable_motion();
    }

    fn execute_member(&self, key: &str, action: DockMenuAction) {
        let command = match action {
            DockMenuAction::Activate => DockWindowCommand::Activate,
            DockMenuAction::Minimize => DockWindowCommand::Minimize,
            DockMenuAction::Close => DockWindowCommand::Close,
            _ => return,
        };
        let Some(generation) = self.scope_generation.get() else {
            return;
        };
        if !self.is_open()
            || !self.surface.window().is_visible()
            || !matches!(
                self.surface.get_kind(),
                DockMenuKind::Window | DockMenuKind::Pinned
            )
            || !self
                .surface
                .get_window_entries()
                .iter()
                .any(|entry| entry.window_key == key && entry.action == action)
        {
            return;
        }
        let key = SharedString::from(key);
        // Release focus/lease before routing; reentrant scope replacement cancels.
        self.hide();
        if generation.checked_add(1) != self.scope_generation.get() || self.is_open() {
            return;
        }
        if let Some(dock) = self.dock.upgrade().filter(|dock| {
            let status = dock.get_surface_status();
            dock.window().is_visible()
                && !status.refreshing
                && !status.stale
                && dock.displayed_window_key(&key).is_some()
        }) {
            dock.invoke_window_command_requested(key, command);
        }
    }

    fn execute(&self, action: DockMenuAction) {
        if matches!(
            action,
            DockMenuAction::MediaBarAdd | DockMenuAction::MediaRemove
        ) {
            self.execute_media(action);
            return;
        }
        if !self.surface.is_visible()
            || self.surface.get_media_scope()
            || !allowed(self.surface.get_kind(), action)
        {
            return;
        }
        let key = self.key.borrow().clone();
        let dock = self.dock.upgrade();
        let scope_generation = self.scope_generation.get();
        if action == DockMenuAction::RecycleEmpty
            && (scope_generation.is_none()
                || !key.is_empty()
                || dock.as_ref().is_some_and(|dock| {
                    !dock.window().is_visible() || !dock.get_recycle_empty_enabled()
                }))
        {
            return;
        }
        let pin_order = self.pin_order.borrow().clone();
        // Capture the presented desired value before hide can reenter and
        // publish another scope. Never toggle whichever state is current later.
        let lock_desired = self.lock_desired.get();
        // Releasing our foreground/native role precedes any parent callback.
        self.hide();
        let Some(dock) = dock else { return };
        match action {
            DockMenuAction::Activate => {
                dock.invoke_window_command_requested(key, DockWindowCommand::Activate)
            }
            DockMenuAction::Minimize => {
                dock.invoke_window_command_requested(key, DockWindowCommand::Minimize)
            }
            DockMenuAction::Close => {
                dock.invoke_window_command_requested(key, DockWindowCommand::Close)
            }
            DockMenuAction::Launch => dock.invoke_launch_requested(key),
            DockMenuAction::Unpin => dock.invoke_pin_toggle_requested(key, false),
            DockMenuAction::MoveEarlier | DockMenuAction::MoveLater => {
                // A native lease Drop may replace even this same-key popup.
                // The old discrete intent cannot reorder its replacement.
                let same_scope = scope_generation
                    .and_then(|generation| generation.checked_add(1))
                    .is_some_and(|generation| self.scope_generation.get() == Some(generation))
                    && !self.is_open()
                    && self.surface.get_kind() == DockMenuKind::Pinned
                    && *self.key.borrow() == key;
                let current_order: Vec<_> = dock
                    .get_pinned_apps()
                    .iter()
                    .map(|app| app.key.to_string())
                    .collect();
                if same_scope
                    && dock.window().is_visible()
                    && dock.get_reorder_enabled()
                    && current_order == pin_order
                {
                    dock.invoke_reorder_move_requested(key, action == DockMenuAction::MoveLater);
                }
            }
            DockMenuAction::DockSetLocked => {
                let same_scope = scope_generation
                    .and_then(|generation| generation.checked_add(1))
                    .is_some_and(|generation| self.scope_generation.get() == Some(generation))
                    && !self.is_open()
                    && self.surface.get_kind() == DockMenuKind::Bar
                    && !self.surface.get_media_scope()
                    && key.is_empty()
                    && self.key.borrow().is_empty();
                if let Some(locked) = lock_desired.filter(|_| {
                    same_scope
                        && dock.window().is_visible()
                        && dock.get_reorder_lock_available()
                        && dock.get_reorder_lock_enabled()
                }) {
                    dock.invoke_reorder_lock_requested(locked);
                }
            }
            DockMenuAction::Settings => dock.invoke_open_settings_requested(),
            DockMenuAction::FileManager => {
                dock.invoke_system_command_requested(DockSystemCommand::FileManager)
            }
            DockMenuAction::TaskManager => {
                dock.invoke_system_command_requested(DockSystemCommand::TaskManager)
            }
            DockMenuAction::Restore => {
                dock.invoke_system_command_requested(DockSystemCommand::Restore)
            }
            DockMenuAction::Exit => dock.invoke_exit_requested(),
            DockMenuAction::RecycleEmpty => {
                // Lease Drop may reenter, replace this scope, or retire the dock.
                let same_scope = scope_generation
                    .and_then(|generation| generation.checked_add(1))
                    .is_some_and(|generation| self.scope_generation.get() == Some(generation))
                    && !self.is_open()
                    && self.surface.get_kind() == DockMenuKind::Recycle
                    && self.key.borrow().is_empty();
                if same_scope && dock.window().is_visible() && dock.get_recycle_empty_enabled() {
                    dock.invoke_recycle_action_requested(DockRecycleAction::Empty);
                }
            }
            DockMenuAction::RecycleRetry => {
                dock.invoke_recycle_action_requested(DockRecycleAction::Retry)
            }
            DockMenuAction::MediaBarAdd | DockMenuAction::MediaRemove => {}
            // These belong exclusively to the launcher's favorite scope.
            DockMenuAction::FavoriteAdd | DockMenuAction::FavoriteRemove => {}
        }
    }

    fn execute_media(&self, action: DockMenuAction) {
        if !self.surface.is_visible()
            || !self.surface.window().is_visible()
            || self.surface.get_kind() != DockMenuKind::Bar
            || self.surface.get_launcher_favorite_scope()
        {
            return;
        }
        let Some(generation) = self.scope_generation.get() else {
            return;
        };
        let media_scope = self.surface.get_media_scope();
        let enabled = self.surface.get_media_enabled();
        let desired = action == DockMenuAction::MediaBarAdd;
        if desired == enabled
            || (media_scope && desired)
            || !self.dock.upgrade().is_some_and(|dock| {
                dock.window().is_visible() && dock.get_media_view().enabled == enabled
            })
        {
            return;
        }
        // No strong Dock survives lease Drop: a reentrant owner drop or scope
        // replacement must cancel this intent before the root transaction.
        self.hide();
        let same_scope = generation
            .checked_add(1)
            .is_some_and(|retired| self.scope_generation.get() == Some(retired))
            && !self.is_open()
            && !self.surface.window().is_visible()
            && self.surface.get_kind() == DockMenuKind::Bar
            && self.surface.get_media_scope() == media_scope
            && self.surface.get_media_enabled() == enabled;
        if !same_scope {
            return;
        }
        if let Some(dock) = self
            .dock
            .upgrade()
            .filter(|dock| dock.window().is_visible() && dock.get_media_view().enabled == enabled)
        {
            dock.invoke_media_enabled_requested(desired);
        }
    }

    #[cfg(any(windows, test))]
    fn is_focused(&self) -> Option<bool> {
        use slint::winit_030::WinitWindowAccessor;
        self.surface
            .window()
            .with_winit_window(|window| window.has_focus())
    }

    #[cfg(not(any(windows, test)))]
    fn is_focused(&self) -> Option<bool> {
        None
    }
}

impl Drop for ContextMenuController {
    fn drop(&mut self) {
        self.hide();
    }
}

fn allowed(kind: DockMenuKind, action: DockMenuAction) -> bool {
    match kind {
        DockMenuKind::Bar => matches!(
            action,
            DockMenuAction::Settings
                | DockMenuAction::FileManager
                | DockMenuAction::TaskManager
                | DockMenuAction::Restore
                | DockMenuAction::Exit
                | DockMenuAction::DockSetLocked
        ),
        DockMenuKind::Pinned => matches!(
            action,
            DockMenuAction::Launch
                | DockMenuAction::Unpin
                | DockMenuAction::MoveEarlier
                | DockMenuAction::MoveLater
        ),
        DockMenuKind::Window => matches!(
            action,
            DockMenuAction::Activate | DockMenuAction::Minimize | DockMenuAction::Close
        ),
        DockMenuKind::Recycle => matches!(
            action,
            DockMenuAction::RecycleEmpty | DockMenuAction::RecycleRetry
        ),
    }
}

/// Convert the current measured client viewport inward at its actual scale.
/// Reject clipped/zero/invalid layouts instead of inventing a 96-DPI rectangle.
fn preview_rect(
    window: &slint::Window,
    bounds: crate::generated::TileBounds,
) -> Result<tessera_core::Rect, String> {
    let scale = f64::from(window.scale_factor());
    let values = [
        f64::from(bounds.origin.x),
        f64::from(bounds.origin.y),
        f64::from(bounds.width),
        f64::from(bounds.height),
    ];
    if !scale.is_finite()
        || scale <= 0.0
        || values.iter().any(|value| !value.is_finite())
        || values[0] < 0.0
        || values[1] < 0.0
        || values[2] <= 0.0
        || values[3] <= 0.0
    {
        return Err("The window preview viewport is unavailable.".into());
    }
    let left = (values[0] * scale).ceil();
    let top = (values[1] * scale).ceil();
    let right = ((values[0] + values[2]) * scale).floor();
    let bottom = ((values[1] + values[3]) * scale).floor();
    let size = window.size();
    if left >= right
        || top >= bottom
        || right > f64::from(i32::MAX)
        || bottom > f64::from(i32::MAX)
        || right > f64::from(size.width)
        || bottom > f64::from(size.height)
    {
        return Err("The window preview viewport is clipped or unavailable.".into());
    }
    tessera_core::Rect::new(
        left as i32,
        top as i32,
        (right - left) as u32,
        (bottom - top) as u32,
    )
    .map_err(|_| "The window preview viewport is unavailable.".into())
}
