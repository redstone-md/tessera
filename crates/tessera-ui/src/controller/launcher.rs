// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::cell::Cell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc};

use super::PanelController;
use crate::generated::{
    Launcher, LauncherDisplayMode as UiDisplayMode, LauncherNavigation, LauncherView,
};
use crate::launcher::{LauncherInventory, LauncherRows, LauncherSelection, Navigation};
use crate::{LauncherDisplayMode, SurfaceKind};

#[derive(Default)]
pub(super) struct LauncherState {
    view: LauncherView,
    selection: LauncherSelection,
    query: String,
    inventory: Rc<LauncherInventory>,
    session: Rc<LauncherSession>,
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

/// Only launcher presentation is serialized: native calls may synchronously
/// hide/reopen it. Logical inventory never stays borrowed across those calls.
#[derive(Default)]
struct LauncherSession {
    generation: Cell<u64>,
    visible: Cell<bool>,
    presenting: Cell<bool>,
    reopen: Cell<bool>,
    refit: Cell<bool>,
    saving: Cell<bool>,
}

impl LauncherSession {
    fn advance(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
    }
}

struct LauncherPresentation<'a> {
    controller: &'a PanelController,
    launcher: &'a Launcher,
    session: Rc<LauncherSession>,
    generation: u64,
    completed: bool,
}

impl LauncherPresentation<'_> {
    fn current(&self) -> bool {
        self.session.visible.get() && self.session.generation.get() == self.generation
    }
}

impl Drop for LauncherPresentation<'_> {
    fn drop(&mut self) {
        if !self.completed && self.session.generation.get() == self.generation {
            self.session.visible.set(false);
            self.session.advance();
        }
        if !self.completed || !self.current() {
            self.controller.detach_lease(SurfaceKind::Launcher);
            // A cancelled local attachment has already dropped before this
            // guard. Keep native hide deferred until then; reopening queues.
            let _ = self.launcher.hide();
        }
        self.session.presenting.set(false);
        if self.session.reopen.replace(false) && self.session.visible.get() {
            self.session.refit.set(false);
            self.controller.open_launcher();
        } else if self.session.refit.replace(false) && self.session.visible.get() {
            self.controller.update_launcher_geometry();
        }
    }
}

struct ModeSave<'a>(&'a Cell<bool>);
impl Drop for ModeSave<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

fn ui_display_mode(mode: LauncherDisplayMode) -> UiDisplayMode {
    match mode {
        LauncherDisplayMode::Windowed => UiDisplayMode::Windowed,
        LauncherDisplayMode::Fullscreen => UiDisplayMode::Fullscreen,
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
        launcher.on_display_mode_requested(move |mode| {
            let mode = match mode {
                UiDisplayMode::Windowed => LauncherDisplayMode::Windowed,
                UiDisplayMode::Fullscreen => LauncherDisplayMode::Fullscreen,
            };
            weak.change_launcher_display_mode(mode);
        });
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
        launcher.window().on_close_requested({
            let controller = self.clone();
            move || {
                controller.hide_launcher();
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
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

    fn launcher_session(&self) -> Rc<LauncherSession> {
        Rc::clone(&self.launcher_state.borrow().session)
    }

    /// An absolute footer intent saves the complete applied record first.
    /// A failed save changes neither logical results nor native presentation.
    fn change_launcher_display_mode(&self, mode: LauncherDisplayMode) {
        if self.interactive_launcher().is_none() {
            return;
        }
        let session = self.launcher_session();
        let applied = self.core.applied_preferences();
        if applied.launcher().display_mode() == mode || session.saving.get() {
            return;
        }
        if mode == LauncherDisplayMode::Fullscreen && self.core.dock_context().is_none() {
            self.report_message(
                "Could not save launcher display mode: monitor bounds are unavailable.",
            );
            return;
        }
        session.saving.set(true);
        let _save = ModeSave(&session.saving);
        let preferences = applied.with_launcher_display_mode(mode);
        if let Err(error) = self.core.host().save_preferences(&preferences) {
            self.report_message(&format!(
                "Could not save launcher display mode: {}",
                crate::sanitize::bounded_text(&error, 200)
            ));
            return;
        }
        self.core.record_applied(&preferences);
        // Publish persistence before native callbacks. A nested cancelled refit
        // may report a presentation error; never overwrite it with late success.
        self.report_message("Launcher display mode saved");
        if let Err(error) = self.present_launcher(false) {
            self.report_message(&format!(
                "Launcher display mode saved, but could not present it: {}",
                crate::sanitize::bounded_text(&error, 200)
            ));
        }
    }

    /// Refit only an existing visible session. No show, focus, search reset,
    /// inventory projection, desktop observation or toolkit fullscreen setter.
    pub(super) fn update_launcher_geometry(&self) {
        if let Err(error) = self.present_launcher(false) {
            self.report_message(&format!(
                "Could not present saved launcher display mode: {}",
                crate::sanitize::bounded_text(&error, 200)
            ));
        }
    }

    fn present_launcher(&self, opening: bool) -> Result<bool, String> {
        let Some(launcher) = self.launcher_and_upgrade() else {
            return Ok(false);
        };
        let session = self.launcher_session();
        if opening {
            session.advance();
            session.visible.set(true);
            if session.presenting.get() {
                session.reopen.set(true);
                return Ok(false);
            }
        } else {
            if !session.visible.get() || !launcher.window().is_visible() {
                return Ok(false);
            }
            if session.presenting.get() {
                session.refit.set(true);
                return Ok(false);
            }
        }
        let mode = self.core.applied_preferences().launcher().display_mode();
        let context = self.core.dock_context();
        if mode == LauncherDisplayMode::Fullscreen && context.is_none() {
            self.hide_launcher();
            return Err("Monitor bounds are unavailable.".into());
        }
        let rect = context.map(|context| {
            crate::launcher::launcher_rect(context, launcher.window().scale_factor(), mode)
        });
        let position = launcher.window().position();
        let size = launcher.window().size();
        let rect = rect
            .map(|rect| (rect.x, rect.y, rect.width, rect.height))
            .unwrap_or((position.x, position.y, size.width, size.height));
        let geometry_changed = self
            .leases
            .borrow()
            .geometry_changed(SurfaceKind::Launcher, rect);
        let mode_changed = launcher.get_display_mode() != ui_display_mode(mode);
        if !opening && !geometry_changed && !mode_changed {
            return Ok(true);
        }
        session.presenting.set(true);
        let mut presentation = LauncherPresentation {
            controller: self,
            launcher: &launcher,
            generation: session.generation.get(),
            session,
            completed: false,
        };
        if opening {
            self.dismiss_tooltip(false);
            self.launcher_state.borrow_mut().reopen();
            launcher.set_search("".into());
            self.show_launcher_tiles();
            launcher.invoke_reset_scroll();
        }
        if opening || geometry_changed {
            self.detach_lease(SurfaceKind::Launcher);
            if !presentation.current() {
                return Ok(false);
            }
            if context.is_some() {
                launcher
                    .window()
                    .set_size(slint::PhysicalSize::new(rect.2, rect.3));
                if !presentation.current() {
                    return Ok(false);
                }
                launcher
                    .window()
                    .set_position(slint::PhysicalPosition::new(rect.0, rect.1));
            }
        }
        if !presentation.current() {
            return Ok(false);
        }
        if mode_changed {
            launcher.set_display_mode(ui_display_mode(mode));
        }
        if !presentation.current() {
            return Ok(false);
        }
        if opening {
            launcher.show().map_err(|error| error.to_string())?;
        }
        if !presentation.current() || !launcher.window().is_visible() {
            return Ok(false);
        }
        if opening || geometry_changed {
            // Configure outside every state/registry borrow. If its callback
            // cancels this generation, discard the local lease before hide.
            let attachment = self
                .core
                .host()
                .configure_surface(SurfaceKind::Launcher, launcher.window());
            if !presentation.current() || !launcher.window().is_visible() {
                drop(attachment);
                return Ok(false);
            }
            let attachment = attachment?;
            let rect = if context.is_some() {
                rect
            } else {
                let position = launcher.window().position();
                let size = launcher.window().size();
                (position.x, position.y, size.width, size.height)
            };
            let replaced = self
                .leases
                .borrow_mut()
                .store(SurfaceKind::Launcher, attachment, rect);
            drop(replaced);
            if !presentation.current() {
                return Ok(false);
            }
        }
        if opening {
            self.request_ui_focus(launcher.window());
            if !presentation.current() {
                return Ok(false);
            }
            launcher.invoke_focus_search();
            if !presentation.current() {
                return Ok(false);
            }
            launcher.invoke_reset_scroll();
        }
        if !presentation.current() || !launcher.window().is_visible() {
            return Ok(false);
        }
        presentation.completed = true;
        Ok(true)
    }

    /// Explicit opens reset Favorites/search; visible refits deliberately do not.
    pub(crate) fn open_launcher(&self) {
        if let Err(error) = self.present_launcher(true) {
            self.report_message(&format!(
                "Could not present saved launcher display mode: {}",
                crate::sanitize::bounded_text(&error, 200)
            ));
        }
    }

    /// Detach before hide; an in-flight configure must drop its late lease
    /// before native hide, and a synchronous reopen waits for that cleanup.
    pub(crate) fn hide_launcher(&self) {
        let session = self.launcher_session();
        session.advance();
        session.visible.set(false);
        session.reopen.set(false);
        session.refit.set(false);
        if session.presenting.get() {
            self.detach_lease(SurfaceKind::Launcher);
            return;
        }
        if let Some(launcher) = self.launcher_and_upgrade() {
            session.presenting.set(true);
            let _presentation = LauncherPresentation {
                controller: self,
                launcher: &launcher,
                generation: session.generation.get(),
                session,
                completed: false,
            };
        }
    }

    pub(super) fn toggle_launcher(&self) {
        if self.launcher_session().visible.get() {
            self.hide_launcher();
        } else {
            self.open_launcher();
        }
    }

    /// Queued signals from hidden, stale or busy surfaces cannot launch apps.
    fn interactive_launcher(&self) -> Option<Launcher> {
        let session = self.launcher_session();
        if self.guarded() || !session.visible.get() || session.presenting.get() {
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
