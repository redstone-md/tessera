// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Separately owned native context-menu window; no modal platform menu loop.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, SharedString};

use crate::generated::{
    ContextMenuSurface, Dock, DockMenuAction, DockMenuKind, DockSystemCommand, DockWindowCommand,
    Palette, SeelenPalette,
};
use crate::transient_window::TransientWindow;
use crate::{DesktopHost, DockContext, SurfaceKind};

use crate::popup_placement as placement;
#[cfg(test)]
mod tests;

pub(crate) struct ContextMenuController {
    surface: TransientWindow<ContextMenuSurface>,
    dock: slint::Weak<Dock>,
    key: RefCell<SharedString>,
    focus_seen: Cell<bool>,
    focus_watch: slint::Timer,
}

impl ContextMenuController {
    pub(crate) fn is_open(&self) -> bool {
        self.surface.is_visible()
    }
    pub(crate) fn new(
        host: Arc<dyn DesktopHost>,
        dock: &Dock,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let menu = Rc::new(Self {
            surface: TransientWindow::new(host, ContextMenuSurface::new()?, SurfaceKind::Popup),
            dock: dock.as_weak(),
            key: RefCell::default(),
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
        self.hide();
        let dock = self
            .dock
            .upgrade()
            .ok_or("The dock is no longer available.")?;
        self.surface.set_kind(kind);
        self.surface.set_selected_index(0);
        *self.key.borrow_mut() = key;
        self.surface
            .global::<SeelenPalette>()
            .set_color_scheme(dock.global::<SeelenPalette>().get_color_scheme());
        self.surface
            .global::<Palette>()
            .set_color_scheme(dock.global::<Palette>().get_color_scheme());
        let rect = placement::place(
            context,
            anchor,
            (
                self.surface.get_menu_width(),
                self.surface.get_menu_height(),
            ),
            dock.window().scale_factor(),
        )?;
        if !self.surface.present(rect.position, rect.size)? {
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
        self.focus_watch.stop();
        self.surface.hide();
    }

    fn execute(&self, action: DockMenuAction) {
        if !self.surface.is_visible() || !allowed(self.surface.get_kind(), action) {
            return;
        }
        let key = self.key.borrow().clone();
        let dock = self.dock.upgrade();
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
    }
}
