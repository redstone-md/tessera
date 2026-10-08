// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc};

use super::PanelController;
use crate::SurfaceKind;
use crate::generated::{Launcher, LauncherNavigation, LauncherView};
use crate::launcher::{LauncherInventory, LauncherRows, LauncherSelection, Navigation};

#[derive(Default)]
pub(super) struct LauncherState {
    view: LauncherView,
    selection: LauncherSelection,
    query: String,
    inventory: Rc<LauncherInventory>,
}

impl LauncherState {
    fn effective_view(&self, query: &str) -> LauncherView {
        // Raw whitespace opens All, but does not implicitly select an app.
        if query.is_empty() {
            self.view
        } else {
            LauncherView::All
        }
    }

    fn project(
        &mut self,
        inventory: Rc<LauncherInventory>,
        query: &str,
        view: LauncherView,
    ) -> Option<usize> {
        self.inventory = inventory;
        let keys = self.inventory.keys();
        let active_query = view == LauncherView::All && !query.trim().is_empty();
        // Slint can deliver an earlier changed callback after a projection.
        // Only a genuinely different query/view discards selected identity.
        if self.view != view || self.query != query {
            self.view = view;
            self.query = query.to_owned();
            self.selection.search_changed(keys, active_query)
        } else {
            self.selection.reconcile(keys, active_query)
        }
    }

    fn select(&mut self, key: &str) -> Option<usize> {
        self.selection.select(self.inventory.keys(), key)
    }

    fn navigate(&mut self, direction: Navigation, columns: usize) -> Option<usize> {
        self.selection
            .navigate(self.inventory.keys(), direction, columns)
    }

    fn resolve(&self, key: &str) -> Option<String> {
        self.inventory
            .keys()
            .iter()
            .find(|current| current.as_str() == key)
            .cloned()
    }

    fn switch(&mut self, view: LauncherView) {
        self.view = view;
        self.selection.reset();
        self.query.clear();
    }

    fn reopen(&mut self) {
        self.switch(LauncherView::Favorites);
    }
}

impl PanelController {
    pub(super) fn wire_launcher(&self, launcher: &Launcher) {
        let weak = self.clone();
        launcher.on_launch_requested(move |key| weak.launch_launcher(&key));
        let weak = self.clone();
        launcher.on_favorite_toggle_requested(move |key, favorite| {
            weak.toggle_launcher_favorite(&key, favorite);
        });
        let weak = self.clone();
        launcher.on_view_requested(move |view| weak.switch_launcher_view(view));
        let weak = self.clone();
        launcher.on_search_changed(move || weak.apply_launcher_filter());
        let weak = self.clone();
        launcher.on_navigate_requested(move |direction| weak.navigate_launcher(direction));
        let weak = self.clone();
        launcher.on_select_requested(move |key| weak.select_launcher(&key));
        let weak = self.clone();
        launcher.on_activate_selected_requested(move || weak.activate_launcher_selection());
        let weak = self.clone();
        launcher.on_open_settings_requested(move || weak.open_panel());
        let weak = self.clone();
        launcher.on_refresh_requested(move || {
            let _ = weak.refresh();
        });
        let weak = self.clone();
        // Escape hides the launcher (dropping its lease first, because a
        // hidden window may lose its HWND); the loop keeps running.
        launcher.on_hide_requested(move || weak.hide_launcher());
        let weak = self.clone();
        // The footer's explicit Exit button quits the run (the host
        // supervisor follows by restoring the Explorer shell).
        launcher.on_exit_requested(move || {
            let _ = slint::quit_event_loop();
            let _ = weak;
        });
    }

    /// Applies the launcher's own search to All, resolving Favorites separately.
    /// Filtering never re-observes the desktop and never renumbers keys.
    pub(crate) fn apply_launcher_filter(&self) {
        self.show_launcher_tiles();
    }

    /// Publishes complete logical results with lazy native row presentation.
    /// Key authority never retrieves rows or converts offscreen images.
    pub(super) fn show_launcher_tiles(&self) {
        let Some(launcher) = self.launcher_and_upgrade() else {
            return;
        };
        let search = launcher.get_search().to_string();
        let catalog = self.core.catalog();
        let favorites = self.core.launcher_favorites();
        let pins = self.core.pins();
        let view = self.launcher_state.borrow().effective_view(&search);
        let view_changed = self.launcher_state.borrow().view != view;
        let apps = match view {
            LauncherView::Favorites => {
                crate::projection::project_favorite_apps(&catalog, &favorites, &pins)
            }
            LauncherView::All => crate::projection::project_launcher_apps(&catalog, &pins, &search),
        };
        let inventory = Rc::new(LauncherInventory::new(apps));
        let count = i32::try_from(inventory.len())
            .expect("retained native catalog fits the Slint model index range");
        let index = self
            .launcher_state
            .borrow_mut()
            .project(Rc::clone(&inventory), &search, view);
        let icons = Rc::clone(&self.icon_cache);
        let saved_favorites_present = !favorites.is_empty();
        let rows = LauncherRows::new(
            inventory,
            favorites,
            launcher.get_grid_columns().max(1) as usize,
            move |icon| icons.borrow_mut().optional(icon).unwrap_or_default(),
        );
        launcher.set_view(view);
        launcher.set_saved_favorites_present(saved_favorites_present);
        launcher.set_application_count(count);
        launcher.set_rows(ModelRc::new(rows));
        if view_changed {
            launcher.invoke_reset_scroll();
        }
        self.project_launcher_selection(&launcher, index);
    }

    fn switch_launcher_view(&self, view: LauncherView) {
        let Some(launcher) = self.interactive_launcher() else {
            return;
        };
        if self.launcher_state.borrow().view == view {
            return;
        }
        self.launcher_state.borrow_mut().switch(view);
        launcher.set_search("".into());
        self.show_launcher_tiles();
        launcher.invoke_reset_scroll();
        launcher.invoke_focus_search();
    }

    /// Resolves current logical result membership, not materialized delegates.
    /// The caller separately guards visibility/state and revalidates the target.
    pub(super) fn resolve_launcher_key(&self, key: &str) -> Option<String> {
        self.launcher_and_upgrade()?;
        self.launcher_state.borrow().resolve(key)
    }

    /// Favorites are immediate saves of the complete applied record, never
    /// dock pins or an appearance preview. Stored IDs are not launch authority.
    fn toggle_launcher_favorite(&self, key: &str, favorite: bool) {
        let Some(launcher) = self.interactive_launcher() else {
            return;
        };
        let Some(key) = self
            .resolve_launcher_key(key)
            .filter(|key| self.core.catalog().iter().any(|app| app.key() == key))
        else {
            self.report_message("That application is no longer in the current launcher results.");
            return;
        };
        let mut favorites = self.core.launcher_favorites();
        if favorites.iter().any(|existing| existing == &key) == favorite {
            return;
        }
        if favorite {
            favorites.push(key);
        } else {
            favorites.retain(|existing| existing != &key);
        }
        let preferences = match self
            .core
            .applied_preferences()
            .with_launcher_favorites(favorites)
        {
            Ok(preferences) => preferences,
            Err(error) => {
                self.report_message(&format!(
                    "Could not save favorites: {}",
                    crate::sanitize::bounded_text(&error, 200)
                ));
                return;
            }
        };
        match self.core.host().save_preferences(&preferences) {
            Ok(()) => {
                self.core.record_applied(&preferences);
                self.report_message(if favorite {
                    "Added to favorites"
                } else {
                    "Removed from favorites"
                });
                self.show_launcher_tiles();
                if launcher.get_selected_key().is_empty()
                    && self.launcher_state.borrow().view == LauncherView::Favorites
                {
                    launcher.invoke_focus_search();
                }
            }
            Err(error) => self.report_message(&format!(
                "Could not save favorites: {}",
                crate::sanitize::bounded_text(&error, 200)
            )),
        }
    }

    /// Shows (creates not; already constructed) the frameless launcher
    /// window in dock mode. Its Escape key hides it; the loop keeps running.
    ///
    /// Lease lifecycle: the launcher window is a tool-window but activatable
    /// (no no-activate, unlike the bars); its HWND may be released while
    /// hidden, so the lease is attached after every show and dropped before
    /// every hide.
    pub(crate) fn open_launcher(&self) {
        self.dismiss_tooltip(false);
        if let Some(launcher) = self.launcher_and_upgrade() {
            self.launcher_state.borrow_mut().reopen();
            launcher.set_search("".into());
            self.show_launcher_tiles();
            launcher.invoke_reset_scroll();
            self.leases.borrow_mut().detach(SurfaceKind::Launcher);
            let rect = self.core.dock_context().map(|context| {
                crate::dock::launcher_rect(context, launcher.window().scale_factor())
            });
            if let Some(rect) = rect {
                launcher
                    .window()
                    .set_size(slint::PhysicalSize::new(rect.width, rect.height));
                launcher
                    .window()
                    .set_position(slint::PhysicalPosition::new(rect.x, rect.y));
            }
            if let Err(error) = launcher.show() {
                self.fail_surface(error.to_string());
                return;
            }
            let rect = rect
                .map(|rect| (rect.x, rect.y, rect.width, rect.height))
                .unwrap_or((0, 0, 0, 0));
            if self.attach_lease(SurfaceKind::Launcher, launcher.window(), rect) {
                self.request_ui_focus(launcher.window());
                launcher.invoke_focus_search();
                launcher.invoke_reset_scroll();
            }
        }
    }

    /// Hides the launcher (Escape path): the lease drops first because
    /// Slint/winit may release or recreate the HWND while hidden.
    pub(crate) fn hide_launcher(&self) {
        if let Some(launcher) = self.launcher_and_upgrade() {
            self.leases.borrow_mut().detach(SurfaceKind::Launcher);
            let _ = launcher.hide();
        }
    }

    pub(super) fn toggle_launcher(&self) {
        if self
            .launcher_and_upgrade()
            .is_some_and(|launcher| launcher.window().is_visible())
        {
            self.hide_launcher();
        } else {
            self.open_launcher();
        }
    }

    /// Queued signals from hidden, stale or busy surfaces cannot launch apps.
    fn interactive_launcher(&self) -> Option<Launcher> {
        if self.guarded() {
            return None;
        }
        self.launcher_and_upgrade()
            .filter(|launcher| launcher.window().is_visible())
    }

    fn select_launcher(&self, key: &str) {
        let Some(launcher) = self.interactive_launcher() else {
            return;
        };
        let index = self.launcher_state.borrow_mut().select(key);
        self.project_launcher_selection(&launcher, index);
    }

    fn navigate_launcher(&self, direction: LauncherNavigation) {
        let Some(launcher) = self.interactive_launcher() else {
            return;
        };
        let direction = match direction {
            LauncherNavigation::Up => Navigation::Up,
            LauncherNavigation::Down => Navigation::Down,
            LauncherNavigation::Left => Navigation::Left,
            LauncherNavigation::Right => Navigation::Right,
        };
        let columns = launcher.get_grid_columns().max(1) as usize;
        let index = self
            .launcher_state
            .borrow_mut()
            .navigate(direction, columns);
        self.project_launcher_selection(&launcher, index);
    }

    fn project_launcher_selection(&self, launcher: &Launcher, index: Option<usize>) {
        // Never retain a selection borrow across synchronous UI/focus callbacks.
        let key = self
            .launcher_state
            .borrow()
            .selection
            .key()
            .unwrap_or_default()
            .to_owned();
        launcher.set_selected_key(key.into());
        if let Some(index) = index.and_then(|index| i32::try_from(index).ok()) {
            launcher.invoke_ensure_visible(index);
        }
    }

    fn activate_launcher_selection(&self) {
        if self.interactive_launcher().is_none() {
            return;
        }
        let key = self
            .launcher_state
            .borrow()
            .selection
            .key()
            .map(str::to_owned);
        if let Some(key) = key {
            self.launch_launcher(&key);
        }
    }

    fn launch_launcher(&self, key: &str) {
        let Some(_launcher) = self.interactive_launcher() else {
            return;
        };
        let Some(key) = self.resolve_launcher_key(key) else {
            self.report_message("That application is no longer in the current launcher results.");
            return;
        };
        if self.launch_resolved_app(&key) {
            self.hide_launcher();
        }
    }
}
