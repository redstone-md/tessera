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
    DockWindowCommand,
};
use crate::theme::ThemedComponent;
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
    focus_seen: Cell<bool>,
    focus_watch: slint::Timer,
}

impl ContextMenuController {
    pub(crate) fn is_open(&self) -> bool {
        self.surface.is_visible()
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
                host,
                ContextMenuSurface::new_with_metrics()?,
                SurfaceKind::Popup,
            ),
            dock: dock.as_weak(),
            key: RefCell::default(),
            scope_generation: Cell::new(Some(0)),
            focus_seen: Cell::new(false),
            focus_watch: slint::Timer::default(),
        });
        let weak = Rc::downgrade(&menu);
        menu.surface.on_action_requested(move |action| {
            if let Some(menu) = weak.upgrade() {
                menu.execute(action);
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
        self.surface.set_recycle_empty_enabled(false);
        self.focus_watch.stop();
        self.surface.hide();
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
        ),
        DockMenuKind::Pinned => matches!(action, DockMenuAction::Launch | DockMenuAction::Unpin),
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
