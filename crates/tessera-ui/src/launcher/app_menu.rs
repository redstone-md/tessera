// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native Pin/Unpin presentation only. Catalog authority and preference
//! transactions stay with the launcher controller; opening performs neither.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, SharedString};

use crate::generated::{ContextMenuSurface, DockMenuAction, Launcher};
use crate::popup_placement as placement;
use crate::theme::ThemedComponent;
use crate::transient_window::TransientWindow;
use crate::{DesktopHost, DockContext, SurfaceKind};

#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod tests;

#[derive(Clone)]
struct Scope {
    launcher: slint::Weak<Launcher>,
    key: SharedString,
    desired_favorite: bool,
    token: Rc<()>,
}

pub(crate) struct LauncherAppMenu {
    surface: TransientWindow<ContextMenuSurface>,
    token: RefCell<Rc<()>>,
    scope: RefCell<Option<Scope>>,
    focus_seen: Cell<bool>,
    focus_watch: slint::Timer,
}

impl LauncherAppMenu {
    pub(crate) fn new(host: Arc<dyn DesktopHost>) -> Result<Rc<Self>, slint::PlatformError> {
        let menu = Rc::new(Self {
            surface: TransientWindow::new(
                host,
                ContextMenuSurface::new_with_metrics()?,
                SurfaceKind::Popup,
            ),
            token: RefCell::new(Rc::new(())),
            scope: RefCell::default(),
            focus_seen: Cell::new(false),
            focus_watch: slint::Timer::default(),
        });
        menu.install_callbacks();
        Ok(menu)
    }

    pub(crate) fn is_open(&self) -> bool {
        self.surface.is_visible() && self.surface.window().is_visible()
    }

    #[cfg(test)]
    pub(crate) fn component(&self) -> &ContextMenuSurface {
        &self.surface
    }

    /// Caller validates catalog/results membership and controller busy state.
    /// Caller must retire this scope on query/view/catalog/projection changes,
    /// even when the resulting key list happens to be unchanged.
    pub(crate) fn show(
        self: &Rc<Self>,
        launcher: &Launcher,
        key: SharedString,
        favorite: bool,
        logical_anchor: slint::LogicalPosition,
        context: DockContext,
    ) -> Result<(), String> {
        let token = self.retire();
        // Native lease Drop may synchronously issue a newer presentation.
        if !self.current(&token) || self.is_open() {
            return Ok(());
        }
        if !source_available(launcher) || key.is_empty() {
            return Err("The launcher application is no longer available.".into());
        }
        let scale = launcher.window().scale_factor();
        let anchor = placement::physical_anchor(
            launcher.window().position(),
            scale,
            (logical_anchor.x, logical_anchor.y),
        )?;
        self.surface.set_launcher_favorite_scope(true);
        self.surface.set_launcher_favorite(favorite);
        self.surface.set_target_icon(Default::default());
        self.surface.set_recycle_empty_enabled(false);
        self.surface.set_selected_index(0);
        self.surface
            .apply_presentation_theme(launcher.presentation_theme());
        let rect = placement::place(
            context,
            anchor,
            (
                self.surface.get_menu_width(),
                self.surface.get_menu_height(),
            ),
            scale,
        )?;
        *self.scope.borrow_mut() = Some(Scope {
            launcher: launcher.as_weak(),
            key,
            desired_favorite: !favorite,
            token: Rc::clone(&token),
        });
        let presented = match self.surface.present(rect.position, rect.size) {
            Ok(presented) => presented,
            Err(error) => {
                if self.current(&token) {
                    self.hide();
                }
                return Err(error);
            }
        };
        if !self.current(&token) {
            return Ok(());
        }
        if !presented {
            self.hide();
            return Ok(());
        }
        if !source_available(launcher) {
            self.hide();
            return Ok(());
        }
        self.surface.invoke_focus_menu();
        let focus = self.surface.request_focus();
        // Focus dispatch is also outward/reentrant; never watch a retired scope.
        if self.current(&token) && self.is_open() {
            self.focus_seen.set(self.is_focused() == Some(true));
            if self.is_focused().is_some() {
                let weak = Rc::downgrade(self);
                self.focus_watch.start(
                    slint::TimerMode::Repeated,
                    Duration::from_millis(100),
                    move || {
                        if let Some(menu) = weak.upgrade() {
                            if !menu.current(&token) {
                                return;
                            }
                            match menu.is_focused() {
                                Some(true) => menu.focus_seen.set(true),
                                Some(false) if menu.focus_seen.get() => menu.hide(),
                                _ => {}
                            }
                        }
                    },
                );
            }
        }
        focus.map_err(|error| format!("Menu opened, but keyboard focus was not granted: {error}"))
    }

    pub(crate) fn hide(&self) {
        self.retire();
    }

    pub(crate) fn disable_motion(&self) {
        self.surface.disable_motion();
    }

    fn current(&self, token: &Rc<()>) -> bool {
        Rc::ptr_eq(&self.token.borrow(), token)
    }

    /// Publish retirement before releasing the lease, outside every borrow.
    fn retire(&self) -> Rc<()> {
        let token = Rc::new(());
        *self.token.borrow_mut() = Rc::clone(&token);
        let _ = self.scope.borrow_mut().take();
        self.focus_watch.stop();
        self.focus_seen.set(false);
        self.surface.hide();
        token
    }

    /// Generated handlers cannot be replaced while one is executing. Install
    /// once, then snapshot the private issued authority at callback entry.
    fn install_callbacks(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.surface.on_action_requested(move |action| {
            if let Some(menu) = weak.upgrade() {
                let scope = menu.scope.borrow().clone();
                if let Some(scope) = scope {
                    menu.execute(&scope.token, action);
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.surface.on_dismiss_requested(move || {
            if let Some(menu) = weak.upgrade() {
                menu.hide();
            }
        });
        let weak = Rc::downgrade(self);
        self.surface.window().on_close_requested(move || {
            if let Some(menu) = weak.upgrade() {
                menu.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
    }

    fn execute(&self, issued: &Rc<()>, action: DockMenuAction) {
        if !self.current(issued) || !self.is_open() || !self.surface.get_launcher_favorite_scope() {
            return;
        }
        let Some(scope) = self.scope.borrow().clone() else {
            return;
        };
        if !Rc::ptr_eq(&scope.token, issued)
            || action
                != if scope.desired_favorite {
                    DockMenuAction::FavoriteAdd
                } else {
                    DockMenuAction::FavoriteRemove
                }
        {
            return;
        }
        if !scope
            .launcher
            .upgrade()
            .is_some_and(|launcher| source_available(&launcher))
        {
            self.hide();
            return;
        }
        let retired = self.retire();
        // Detach may hide/reopen the launcher, replace the popup, or drop its
        // owner. That supersedes this request even if the key still exists.
        if !self.current(&retired) || self.is_open() {
            return;
        }
        let Some(launcher) = scope.launcher.upgrade().filter(source_available) else {
            return;
        };
        launcher.invoke_favorite_toggle_requested(scope.key, scope.desired_favorite);
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

fn source_available(launcher: &Launcher) -> bool {
    launcher.window().is_visible()
        && launcher.get_input_available()
        && !launcher.get_refreshing()
        && !launcher.get_stale()
        && !launcher.get_reorder_dragging()
        && !launcher.get_reorder_visual().visible
}

impl Drop for LauncherAppMenu {
    fn drop(&mut self) {
        self.hide();
    }
}
