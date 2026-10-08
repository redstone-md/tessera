// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use slint::language::{DragAction, DropEvent, PointerEvent, PointerEventButton, PointerEventKind};
use slint::{ComponentHandle, DataTransfer, ModelRc};

use super::PanelController;
use crate::generated::{
    LaunchTile, Launcher, LauncherDisplayMode as UiDisplayMode, LauncherDragVisual,
    LauncherGridMetrics, LauncherNavigation, LauncherView, TileBounds,
};
use crate::launcher::order::{
    Bounds, FavoriteDrag, GridMetrics, Point, apply_reorder, autoscroll_delta,
};
use crate::launcher::{LauncherInventory, LauncherRows, LauncherSelection, Navigation};
use crate::{LauncherDisplayMode, SurfaceKind};

#[derive(Default)]
pub(super) struct LauncherState {
    view: LauncherView,
    selection: LauncherSelection,
    query: String,
    inventory: Rc<LauncherInventory>,
    catalog_keys: Vec<String>,
    catalog_membership: std::collections::HashSet<String>,
    session: Rc<LauncherSession>,
    projection: u64,
    reorder: Rc<ReorderOwner>,
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

/// Only immutable typed payloads authorize an operation. An armed owner marker
/// makes the ancestor eligible before Down; the tile replaces it before the
/// SDK snapshots its initial DropEvent. Old cloned payloads never gain authority.
#[derive(Default)]
struct ReorderMarker;

struct ReorderToken {
    owner: Rc<ReorderMarker>,
    session: u64,
    projection: u64,
    epoch: u64,
    source: String,
}

struct ReorderGesture {
    token: Rc<ReorderToken>,
    drag: FavoriteDrag,
    // One captured image, independent of virtual rows and the bounded cache.
    source_visual: LaunchTile,
    source_scale: f32,
    saved: Vec<String>,
    resolved: Vec<String>,
    catalog: Vec<String>,
    metrics: GridMetrics,
    scale: f32,
    pointer: Option<Point>,
    active: bool,
}

#[derive(Default)]
struct ReorderOwner {
    marker: Rc<ReorderMarker>,
    gesture: RefCell<Option<ReorderGesture>>,
    epoch: Cell<u64>,
    pending_exit: Cell<Option<u64>>,
    callbacks: Cell<usize>,
    timer: slint::Timer,
    exit_timer: slint::Timer,
}

struct LauncherCallback(Rc<ReorderOwner>);

impl Drop for LauncherCallback {
    fn drop(&mut self) {
        self.0.callbacks.set(self.0.callbacks.get() - 1);
        if self.0.callbacks.get() == 0
            && self.0.pending_exit.get().is_some()
            && !self.0.exit_timer.running()
        {
            self.0.exit_timer.restart();
        }
    }
}

fn transfer<T: 'static>(payload: Rc<T>) -> DataTransfer {
    let mut data = DataTransfer::default();
    data.set_user_data(payload);
    data
}

fn grid_metrics(metrics: LauncherGridMetrics) -> GridMetrics {
    GridMetrics {
        viewport: tile_bounds(metrics.viewport),
        source_width: metrics.source_width,
        tile: metrics.tile,
        gap: metrics.gap,
        gutter: metrics.gutter,
        columns: metrics.columns.max(0) as usize,
        content_y: metrics.content_y,
        content_height: metrics.content_height,
    }
}

fn tile_bounds(bounds: TileBounds) -> Bounds {
    Bounds {
        x: bounds.origin.x,
        y: bounds.origin.y,
        width: bounds.width,
        height: bounds.height,
    }
}

fn same_geometry(left: &GridMetrics, right: &GridMetrics) -> bool {
    left.viewport == right.viewport
        && left.source_width == right.source_width
        && left.tile == right.tile
        && left.gap == right.gap
        && left.gutter == right.gutter
        && left.columns == right.columns
}

fn contains_pointer(metrics: &GridMetrics, point: Point) -> bool {
    point.x.is_finite()
        && point.y.is_finite()
        && point.x >= metrics.viewport.x
        && point.x < metrics.viewport.x + metrics.source_width
        && point.y >= metrics.viewport.y
        && point.y < metrics.viewport.y + metrics.viewport.height
}

/// Short-lived UI-thread access. Timer captures only weak pieces, never this
/// access or its controller; native calls happen after all gesture borrows end.
struct ReorderAccess {
    launcher: Launcher,
    state: Rc<RefCell<LauncherState>>,
    core: std::sync::Arc<crate::state::SurfaceCore>,
    icons: Rc<RefCell<crate::icons::IconCache>>,
    saving: Rc<Cell<bool>>,
    panel: slint::Weak<crate::Panel>,
}

impl ReorderAccess {
    fn owner(&self) -> Rc<ReorderOwner> {
        Rc::clone(&self.state.borrow().reorder)
    }

    fn eligible(&self) -> bool {
        let ready = self.panel.upgrade().is_some_and(|panel| {
            panel.get_has_snapshot() && !panel.get_refreshing() && !panel.get_stale()
        });
        let visible = self.launcher.window().is_visible();
        let search_empty = self.launcher.get_search().is_empty();
        let saved = self.core.launcher_favorites();
        let state = self.state.borrow();
        ready
            && visible
            && search_empty
            && !self.saving.get()
            && state.session.visible.get()
            && !state.session.presenting.get()
            && state.view == LauncherView::Favorites
            && state.inventory.len() >= 2
            && state.inventory.keys().iter().eq(saved
                .iter()
                .filter(|key| state.catalog_membership.contains(*key)))
    }

    fn current(&self, token: &Rc<ReorderToken>) -> bool {
        if !self.eligible() {
            return false;
        }
        let catalog: Vec<_> = self
            .core
            .catalog()
            .iter()
            .map(|app| app.key().to_owned())
            .collect();
        let saved = self.core.launcher_favorites();
        let state = self.state.borrow();
        let owner = &state.reorder;
        let gesture = owner.gesture.borrow();
        gesture.as_ref().is_some_and(|gesture| {
            Rc::ptr_eq(&owner.marker, &token.owner)
                && Rc::ptr_eq(&gesture.token, token)
                && owner.epoch.get() == token.epoch
                && state.session.generation.get() == token.session
                && state.projection == token.projection
                && gesture.saved == saved
                && gesture.resolved == state.inventory.keys()
                && gesture.catalog == catalog
                && saved.contains(&token.source)
                && catalog.contains(&token.source)
        })
    }

    fn publish(&self, keys: Option<&[String]>) {
        let (inventory, selected) = {
            let state = self.state.borrow();
            (
                Rc::clone(&state.inventory),
                state.selection.key().unwrap_or_default().to_owned(),
            )
        };
        let inventory = match keys {
            Some(keys) => {
                let Some(preview) = inventory.reordered(keys) else {
                    return;
                };
                Rc::new(preview)
            }
            None => inventory,
        };
        let content_y = self.launcher.get_reorder_metrics().content_y;
        let icons = Rc::clone(&self.icons);
        let rows = LauncherRows::new(
            inventory,
            self.core.launcher_favorites(),
            self.launcher.get_grid_columns().max(1) as usize,
            move |icon| icons.borrow_mut().optional(icon).unwrap_or_default(),
        );
        self.launcher.set_rows(ModelRc::new(rows));
        self.launcher.set_selected_key(selected.into());
        let shifted = self.launcher.get_reorder_metrics().content_y;
        if shifted != content_y {
            self.launcher.invoke_scroll_reorder(content_y - shifted);
        }
    }

    fn sync(&self) {
        let owner = self.owner();
        let enabled = self.eligible() && owner.pending_exit.get().is_none();
        self.launcher.set_reorder_enabled(enabled);
        if owner.gesture.borrow().is_none() {
            self.launcher
                .set_reorder_data(transfer(Rc::clone(&owner.marker)));
        }
    }

    fn cancel(&self) {
        self.end(true);
    }

    fn finish(&self) {
        let owner = self.owner();
        owner.pending_exit.set(None);
        owner.exit_timer.stop();
        self.end(false);
    }

    fn end(&self, cancel_native: bool) {
        let owner = self.owner();
        let gesture = owner.gesture.borrow_mut().take();
        owner.timer.stop();
        self.launcher
            .set_reorder_visual(LauncherDragVisual::default());
        let native_active = self.launcher.get_reorder_dragging();
        if gesture.is_none()
            && (!cancel_native || !native_active || owner.pending_exit.get().is_some())
        {
            self.sync();
            return;
        }
        let epoch = owner.epoch.get().wrapping_add(1);
        owner.epoch.set(epoch);
        if cancel_native && (native_active || gesture.is_some()) {
            owner.pending_exit.set(Some(epoch));
            let state = Rc::downgrade(&self.state);
            let core = std::sync::Arc::downgrade(&self.core);
            let icons = Rc::downgrade(&self.icons);
            let saving = Rc::downgrade(&self.saving);
            let launcher = self.launcher.as_weak();
            let panel = self.panel.clone();
            owner
                .exit_timer
                .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                    let (Some(state), Some(core), Some(icons), Some(saving), Some(launcher)) = (
                        state.upgrade(),
                        core.upgrade(),
                        icons.upgrade(),
                        saving.upgrade(),
                        launcher.upgrade(),
                    ) else {
                        return;
                    };
                    let access = ReorderAccess {
                        launcher,
                        state,
                        core,
                        icons,
                        saving,
                        panel: panel.clone(),
                    };
                    let owner = access.owner();
                    let pending = owner.pending_exit.get() == Some(epoch)
                        && owner.epoch.get() == epoch
                        && owner.gesture.borrow().is_none();
                    if pending && owner.callbacks.get() == 0 {
                        owner.pending_exit.set(None);
                        access
                            .launcher
                            .window()
                            .dispatch_event(slint::platform::WindowEvent::PointerExited);
                        access.sync();
                    }
                });
        }
        let session_visible = self.state.borrow().session.visible.get();
        let visible = session_visible && self.launcher.window().is_visible();
        if visible
            && gesture
                .as_ref()
                .is_some_and(|gesture| gesture.drag.preview_keys() != gesture.resolved)
        {
            self.publish(None);
        }
        self.sync();
    }

    /// Pure presentation: no collision, grid pointer, autoscroll or save authority.
    /// SDK state is read synchronously because changed callbacks can be queued.
    fn present_source(&self, token: &Rc<ReorderToken>, pointer: Point) {
        if !self.launcher.get_reorder_dragging() || !self.current(token) {
            return;
        }
        let owner = self.owner();
        let metrics = grid_metrics(self.launcher.get_reorder_metrics());
        let scale = self.launcher.window().scale_factor();
        let changed_geometry = owner.gesture.borrow().as_ref().is_some_and(|gesture| {
            !same_geometry(&gesture.metrics, &metrics) || gesture.scale != scale
        });
        if changed_geometry {
            self.cancel();
            return;
        }
        let visual = {
            let mut gesture = owner.gesture.borrow_mut();
            let Some(gesture) = gesture.as_mut() else {
                return;
            };
            let Some(bounds) = gesture.drag.translated_bounds(pointer) else {
                return;
            };
            gesture.active = true;
            LauncherDragVisual {
                visible: true,
                source: gesture.source_visual.clone(),
                source_scale: gesture.source_scale,
                bounds: TileBounds {
                    origin: slint::LogicalPosition::new(bounds.x, bounds.y),
                    width: bounds.width,
                    height: bounds.height,
                },
            }
        };
        self.launcher.set_reorder_visual(visual);
    }

    fn hover(&self, pointer: Point) -> bool {
        let owner = self.owner();
        let token = owner
            .gesture
            .borrow()
            .as_ref()
            .map(|gesture| Rc::clone(&gesture.token));
        let Some(token) = token else {
            return false;
        };
        if !self.current(&token) {
            self.cancel();
            return false;
        }
        let metrics = grid_metrics(self.launcher.get_reorder_metrics());
        let scale = self.launcher.window().scale_factor();
        let changed_geometry = owner.gesture.borrow().as_ref().is_some_and(|gesture| {
            !same_geometry(&gesture.metrics, &metrics) || gesture.scale != scale
        });
        if changed_geometry {
            self.cancel();
            return false;
        }
        self.present_source(&token, pointer);
        let preview = {
            let mut gesture = owner.gesture.borrow_mut();
            let gesture = gesture.as_mut().expect("validated gesture");
            gesture.active = true;
            gesture.pointer = Some(pointer);
            gesture.metrics = metrics;
            gesture
                .drag
                .hover(&gesture.metrics, pointer)
                .then(|| gesture.drag.preview_keys().to_vec())
        };
        if let Some(preview) = preview {
            self.publish(Some(&preview));
        }
        let inside = contains_pointer(&metrics, pointer);
        if inside && autoscroll_delta(&metrics, pointer) != 0.0 {
            self.start_timer();
        } else {
            owner.timer.stop();
        }
        inside
    }

    fn start_timer(&self) {
        let owner = self.owner();
        if owner.timer.running() {
            return;
        }
        let state = Rc::downgrade(&self.state);
        let core = std::sync::Arc::downgrade(&self.core);
        let icons = Rc::downgrade(&self.icons);
        let saving = Rc::downgrade(&self.saving);
        let launcher = self.launcher.as_weak();
        let panel = self.panel.clone();
        let weak_owner = Rc::downgrade(&owner);
        owner.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(10),
            move || {
                let (Some(state), Some(core), Some(icons), Some(saving), Some(launcher)) = (
                    state.upgrade(),
                    core.upgrade(),
                    icons.upgrade(),
                    saving.upgrade(),
                    launcher.upgrade(),
                ) else {
                    if let Some(owner) = weak_owner.upgrade() {
                        owner.timer.stop();
                    }
                    return;
                };
                ReorderAccess {
                    launcher,
                    state,
                    core,
                    icons,
                    saving,
                    panel: panel.clone(),
                }
                .tick();
            },
        );
    }

    fn tick(&self) {
        let owner = self.owner();
        let current = owner.gesture.borrow().as_ref().and_then(|gesture| {
            if gesture.active {
                gesture
                    .pointer
                    .map(|pointer| (Rc::clone(&gesture.token), pointer))
            } else {
                None
            }
        });
        let Some((token, pointer)) = current else {
            owner.timer.stop();
            return;
        };
        if !self.current(&token) {
            self.cancel();
            return;
        }
        let metrics = grid_metrics(self.launcher.get_reorder_metrics());
        let scale = self.launcher.window().scale_factor();
        let changed = owner.gesture.borrow().as_ref().is_some_and(|gesture| {
            !same_geometry(&gesture.metrics, &metrics) || gesture.scale != scale
        });
        if changed {
            self.cancel();
            return;
        }
        let delta = autoscroll_delta(&metrics, pointer);
        if delta == 0.0 {
            owner.timer.stop();
            return;
        }
        self.launcher.invoke_scroll_reorder(delta);
        self.hover(pointer);
    }
}

fn ui_display_mode(mode: LauncherDisplayMode) -> UiDisplayMode {
    match mode {
        LauncherDisplayMode::Windowed => UiDisplayMode::Windowed,
        LauncherDisplayMode::Fullscreen => UiDisplayMode::Fullscreen,
    }
}

impl PanelController {
    fn reorder_access(&self) -> Option<ReorderAccess> {
        Some(ReorderAccess {
            launcher: self.launcher_and_upgrade()?,
            state: Rc::clone(&self.launcher_state),
            core: std::sync::Arc::clone(&self.core),
            icons: Rc::clone(&self.icon_cache),
            saving: Rc::clone(&self.preference_saving),
            panel: self.panel.clone(),
        })
    }

    fn launcher_callback(&self) -> LauncherCallback {
        let owner = Rc::clone(&self.launcher_state.borrow().reorder);
        owner.callbacks.set(owner.callbacks.get() + 1);
        LauncherCallback(owner)
    }

    pub(super) fn sync_launcher_reorder(&self) {
        if let Some(access) = self.reorder_access() {
            access.sync();
        }
    }

    pub(super) fn cancel_launcher_reorder(&self) {
        if let Some(access) = self.reorder_access() {
            access.cancel();
        }
    }

    fn launcher_reorder_active(&self) -> bool {
        self.launcher_state
            .borrow()
            .reorder
            .gesture
            .borrow()
            .as_ref()
            .is_some_and(|gesture| gesture.active)
    }

    /// Opening outside an input callback flushes an earlier queued Exit before
    /// another press. Nested host reopen defers it until the SDK dispatch ends.
    fn flush_launcher_reorder_exit(&self) {
        let Some(access) = self.reorder_access() else {
            return;
        };
        let owner = access.owner();
        if owner.callbacks.get() == 0 && owner.pending_exit.take().is_some() {
            owner.exit_timer.stop();
            access
                .launcher
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerExited);
        }
    }

    fn reorder_origin(&self, key: &str, event: PointerEvent, bounds: TileBounds, press: Point) {
        let Some(access) = self.reorder_access() else {
            return;
        };
        let owner = access.owner();
        match event.kind {
            PointerEventKind::Down => {
                access.cancel();
                let epoch = owner.epoch.get().wrapping_add(1);
                owner.epoch.set(epoch);
                // Invalidate queued old-generation teardown before arming a
                // fresh immutable token. Child Cancel is not an origin veto.
                owner.pending_exit.set(None);
                owner.exit_timer.stop();
                if event.button != PointerEventButton::Left || key.is_empty() || !access.eligible()
                {
                    access.sync();
                    return;
                }
                let metrics = grid_metrics(access.launcher.get_reorder_metrics());
                if !contains_pointer(&metrics, press) {
                    return;
                }
                let (resolved, session, projection) = {
                    let state = self.launcher_state.borrow();
                    (
                        state.inventory.keys().to_vec(),
                        state.session.generation.get(),
                        state.projection,
                    )
                };
                let saved = self.core.launcher_favorites();
                let catalog: Vec<_> = self
                    .core
                    .catalog()
                    .iter()
                    .map(|app| app.key().to_owned())
                    .collect();
                if !saved.iter().any(|saved| saved == key)
                    || !catalog.iter().any(|saved| saved == key)
                {
                    return;
                }
                let Some(drag) = FavoriteDrag::begin(&resolved, key, tile_bounds(bounds), press)
                else {
                    return;
                };
                let application = {
                    let state = self.launcher_state.borrow();
                    state.inventory.application(key).cloned()
                };
                let Some(application) = application else {
                    return;
                };
                let source_scale = access.launcher.get_source_appearance_scale();
                if !source_scale.is_finite() || source_scale <= 0.0 {
                    access.sync();
                    return;
                }
                // Exactly one source conversion, after all state borrows end.
                let icon = access
                    .icons
                    .borrow_mut()
                    .optional(application.icon.as_ref())
                    .unwrap_or_default();
                let source_visual = LaunchTile {
                    key: application.key.into(),
                    label: application.label.into(),
                    favorite: true,
                    icon,
                };
                let token = Rc::new(ReorderToken {
                    owner: Rc::clone(&owner.marker),
                    session,
                    projection,
                    epoch,
                    source: key.to_owned(),
                });
                let scale = access.launcher.window().scale_factor();
                *owner.gesture.borrow_mut() = Some(ReorderGesture {
                    token: Rc::clone(&token),
                    drag,
                    source_visual,
                    source_scale,
                    saved,
                    resolved,
                    catalog,
                    metrics,
                    scale,
                    pointer: None,
                    active: false,
                });
                access.launcher.set_reorder_data(transfer(token));
                access.sync();
            }
            PointerEventKind::Up => {
                let pending = owner
                    .gesture
                    .borrow()
                    .as_ref()
                    .is_some_and(|gesture| !gesture.active);
                if pending {
                    access.finish();
                }
            }
            // Ancestor takeover sends descendant Cancel before dragging=true.
            PointerEventKind::Cancel | PointerEventKind::Move => {}
            _ => {}
        }
    }

    fn reorder_dragging_changed(&self, dragging: bool) {
        let Some(access) = self.reorder_access() else {
            return;
        };
        if dragging && !access.launcher.get_reorder_dragging() {
            return;
        }
        let owner = access.owner();
        if !dragging {
            // dropped/finished own logical completion. Never erase the source
            // merely because the SDK cleared its flag before drag-finished.
            owner.timer.stop();
            return;
        }
        let token = owner
            .gesture
            .borrow()
            .as_ref()
            .map(|gesture| Rc::clone(&gesture.token));
        if token.as_ref().is_none_or(|token| !access.current(token)) {
            access.cancel();
        }
        // Only the SDK's immutable DropEvent payload activates presentation.
        // A queued changed=true has no payload and must not reveal a foreign
        // native drag, reset its latest point, or resurrect cleared feedback.
    }

    fn reorder_can_drop(&self, event: DropEvent, origin: Point) -> DragAction {
        let Some(access) = self.reorder_access() else {
            return DragAction::None;
        };
        if !access.launcher.get_reorder_dragging() {
            return DragAction::None;
        }
        let Some(token) = event
            .data
            .user_data()
            .and_then(|data| data.downcast::<ReorderToken>().ok())
        else {
            return DragAction::None;
        };
        if !access.current(&token) {
            return DragAction::None;
        }
        let pointer = Point {
            x: origin.x + event.position.x,
            y: origin.y + event.position.y,
        };
        if access.hover(pointer) {
            DragAction::Move
        } else {
            DragAction::None
        }
    }

    fn reorder_window_hover(&self, event: DropEvent, origin: Point) {
        let Some(access) = self.reorder_access() else {
            return;
        };
        let Some(token) = event
            .data
            .user_data()
            .and_then(|data| data.downcast::<ReorderToken>().ok())
        else {
            return;
        };
        access.present_source(
            &token,
            Point {
                x: origin.x + event.position.x,
                y: origin.y + event.position.y,
            },
        );
    }

    fn reorder_dropped(&self, event: DropEvent, origin: Point) -> DragAction {
        let Some(access) = self.reorder_access() else {
            return DragAction::None;
        };
        let Some(token) = event
            .data
            .user_data()
            .and_then(|data| data.downcast::<ReorderToken>().ok())
        else {
            return DragAction::None;
        };
        if !access.current(&token) {
            access.cancel();
            return DragAction::None;
        }
        let pointer = Point {
            x: origin.x + event.position.x,
            y: origin.y + event.position.y,
        };
        if !access.hover(pointer) {
            access.cancel();
            return DragAction::None;
        }
        let owner = access.owner();
        let completed = owner.gesture.borrow().as_ref().and_then(|gesture| {
            gesture
                .drag
                .final_intent()
                .map(|intent| (intent, gesture.resolved.clone()))
        });
        let Some((intent, resolved)) = completed else {
            access.cancel();
            return DragAction::None;
        };
        let favorites = match apply_reorder(&self.core.launcher_favorites(), &resolved, &intent) {
            Ok(Some(favorites)) => favorites,
            Ok(None) | Err(_) => {
                access.cancel();
                return DragAction::None;
            }
        };
        let Some(_save) = self.begin_preference_save() else {
            access.cancel();
            return DragAction::None;
        };
        let preferences = match self
            .core
            .applied_preferences()
            .with_launcher_favorites(favorites)
        {
            Ok(preferences) => preferences,
            Err(error) => {
                self.report_message(&format!(
                    "Could not save favorite order: {}",
                    crate::sanitize::bounded_text(&error, 200)
                ));
                return DragAction::None;
            }
        };
        match self.core.host().save_preferences(&preferences) {
            Ok(()) => {
                // Persist the completed intent, then project current core data
                // into whichever session is now visible, never the old preview.
                self.core.record_applied(&preferences);
                self.report_message("Favorite order saved");
                let session = self.launcher_session();
                if session.visible.get()
                    && !session.presenting.get()
                    && access.launcher.window().is_visible()
                {
                    self.publish_launcher_tiles(false);
                }
                DragAction::Move
            }
            Err(error) => {
                self.report_message(&format!(
                    "Could not save favorite order: {}",
                    crate::sanitize::bounded_text(&error, 200)
                ));
                DragAction::None
            }
        }
    }

    fn reorder_metrics_changed(&self) {
        let Some(access) = self.reorder_access() else {
            return;
        };
        let owner = access.owner();
        let metrics = grid_metrics(access.launcher.get_reorder_metrics());
        let scale = access.launcher.window().scale_factor();
        let changed = owner.gesture.borrow().as_ref().is_some_and(|gesture| {
            !same_geometry(&gesture.metrics, &metrics) || gesture.scale != scale
        });
        if changed {
            access.cancel();
        } else {
            let pointer = {
                let mut gesture = owner.gesture.borrow_mut();
                gesture.as_mut().and_then(|gesture| {
                    let moved = gesture.metrics.content_y != metrics.content_y
                        || gesture.metrics.content_height != metrics.content_height;
                    gesture.metrics = metrics;
                    moved.then_some(gesture.pointer).flatten()
                })
            };
            if let Some(pointer) = pointer {
                access.hover(pointer);
            }
        }
    }

    pub(super) fn wire_launcher(&self, launcher: &Launcher) {
        let weak = self.clone();
        launcher.on_launch_requested(move |key| {
            let _callback = weak.launcher_callback();
            weak.launch_launcher(&key);
        });
        let weak = self.clone();
        launcher.on_favorite_toggle_requested(move |key, favorite| {
            let _callback = weak.launcher_callback();
            weak.toggle_launcher_favorite(&key, favorite);
        });
        let weak = self.clone();
        launcher.on_view_requested(move |view| {
            let _callback = weak.launcher_callback();
            weak.switch_launcher_view(view);
        });
        let weak = self.clone();
        launcher.on_display_mode_requested(move |mode| {
            let _callback = weak.launcher_callback();
            let mode = match mode {
                UiDisplayMode::Windowed => LauncherDisplayMode::Windowed,
                UiDisplayMode::Fullscreen => LauncherDisplayMode::Fullscreen,
            };
            weak.change_launcher_display_mode(mode);
        });
        let weak = self.clone();
        launcher.on_search_changed(move || {
            let _callback = weak.launcher_callback();
            weak.apply_launcher_filter();
        });
        let weak = self.clone();
        launcher.on_navigate_requested(move |direction| {
            let _callback = weak.launcher_callback();
            weak.navigate_launcher(direction);
        });
        let weak = self.clone();
        launcher.on_select_requested(move |key| {
            let _callback = weak.launcher_callback();
            weak.select_launcher(&key);
        });
        let weak = self.clone();
        launcher.on_activate_selected_requested(move || {
            let _callback = weak.launcher_callback();
            weak.activate_launcher_selection();
        });
        let weak = self.clone();
        launcher.on_open_settings_requested(move || {
            let _callback = weak.launcher_callback();
            weak.open_panel();
        });
        let weak = self.clone();
        launcher.on_open_user_menu_requested(move |bounds| {
            let _callback = weak.launcher_callback();
            weak.open_user_menu(bounds);
        });
        let weak = self.clone();
        launcher.on_refresh_requested(move || {
            let _callback = weak.launcher_callback();
            let _ = weak.refresh();
        });
        let weak = self.clone();
        // Escape hides the launcher (dropping its lease first, because a
        // hidden window may lose its HWND); the loop keeps running.
        launcher.on_hide_requested(move || {
            let _callback = weak.launcher_callback();
            weak.hide_launcher();
        });
        launcher.window().on_close_requested({
            let controller = self.clone();
            move || {
                let _callback = controller.launcher_callback();
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
        let marker = Rc::clone(&self.launcher_state.borrow().reorder.marker);
        launcher.set_reorder_data(transfer(marker));
        let controller = self.clone();
        launcher.on_reorder_origin(move |key, event, bounds, press| {
            let _callback = controller.launcher_callback();
            controller.reorder_origin(
                &key,
                event,
                bounds,
                Point {
                    x: press.x,
                    y: press.y,
                },
            );
        });
        let controller = self.clone();
        launcher.on_reorder_dragging_changed(move |dragging| {
            let _callback = controller.launcher_callback();
            controller.reorder_dragging_changed(dragging);
        });
        let controller = self.clone();
        launcher.on_reorder_can_drop(move |event, origin| {
            let _callback = controller.launcher_callback();
            controller.reorder_can_drop(
                event,
                Point {
                    x: origin.x,
                    y: origin.y,
                },
            )
        });
        let controller = self.clone();
        launcher.on_reorder_window_hover(move |event, origin| {
            let _callback = controller.launcher_callback();
            controller.reorder_window_hover(
                event,
                Point {
                    x: origin.x,
                    y: origin.y,
                },
            );
        });
        let controller = self.clone();
        launcher.on_reorder_dropped(move |event, origin| {
            let _callback = controller.launcher_callback();
            controller.reorder_dropped(
                event,
                Point {
                    x: origin.x,
                    y: origin.y,
                },
            )
        });
        let controller = self.clone();
        launcher.on_reorder_finished(move |_| {
            let _callback = controller.launcher_callback();
            if let Some(access) = controller.reorder_access() {
                access.finish();
            }
        });
        let controller = self.clone();
        launcher.on_reorder_metrics_changed(move || {
            let _callback = controller.launcher_callback();
            controller.reorder_metrics_changed();
        });
        let controller = self.clone();
        launcher.on_reorder_target_changed(move |inside| {
            let _callback = controller.launcher_callback();
            if !inside {
                let owner = Rc::clone(&controller.launcher_state.borrow().reorder);
                owner.timer.stop();
                if let Some(gesture) = owner.gesture.borrow_mut().as_mut() {
                    gesture.pointer = None;
                }
            }
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
        self.publish_launcher_tiles(true);
    }

    fn publish_launcher_tiles(&self, ensure_selection: bool) {
        let Some(launcher) = self.launcher_and_upgrade() else {
            return;
        };
        let preserved_scroll_y =
            (!ensure_selection).then(|| launcher.get_reorder_metrics().content_y);
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
        let catalog_keys: Vec<_> = catalog.iter().map(|app| app.key().to_owned()).collect();
        let projection_changed = {
            let state = self.launcher_state.borrow();
            state.inventory.keys() != inventory.keys()
                || state.query != search
                || state.view != view
                || state.catalog_keys != catalog_keys
        };
        let conflicting_favorites = self
            .launcher_state
            .borrow()
            .reorder
            .gesture
            .borrow()
            .as_ref()
            .is_some_and(|gesture| gesture.saved != favorites);
        if projection_changed || conflicting_favorites {
            self.cancel_launcher_reorder();
            let mut state = self.launcher_state.borrow_mut();
            state.projection = state.projection.wrapping_add(1);
        } else if self
            .launcher_state
            .borrow()
            .reorder
            .gesture
            .borrow()
            .is_some()
        {
            self.sync_launcher_reorder();
            return;
        }
        {
            let mut state = self.launcher_state.borrow_mut();
            state.catalog_membership = catalog_keys.iter().cloned().collect();
            state.catalog_keys = catalog_keys;
        }
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
        if ensure_selection {
            self.project_launcher_selection(&launcher, index);
        } else {
            let key = self
                .launcher_state
                .borrow()
                .selection
                .key()
                .unwrap_or_default()
                .to_owned();
            launcher.set_selected_key(key.into());
        }
        if let Some(content_y) = preserved_scroll_y {
            let shifted = launcher.get_reorder_metrics().content_y;
            if shifted != content_y {
                launcher.invoke_scroll_reorder(content_y - shifted);
            }
        }
        self.sync_launcher_reorder();
    }

    fn switch_launcher_view(&self, view: LauncherView) {
        let Some(launcher) = self.interactive_launcher() else {
            return;
        };
        if self.launcher_state.borrow().view == view {
            return;
        }
        self.cancel_launcher_reorder();
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
        let Some(_save) = self.begin_preference_save() else {
            return;
        };
        let generation = self.launcher_session().generation.get();
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
                let session = self.launcher_session();
                if session.visible.get()
                    && !session.presenting.get()
                    && launcher.window().is_visible()
                {
                    if session.generation.get() == generation {
                        self.show_launcher_tiles();
                        if launcher.get_selected_key().is_empty()
                            && self.launcher_state.borrow().view == LauncherView::Favorites
                        {
                            launcher.invoke_focus_search();
                        }
                    } else {
                        self.publish_launcher_tiles(false);
                    }
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

    pub(super) fn launcher_popup_ready(&self) -> bool {
        let session = self.launcher_session();
        session.visible.get() && !session.presenting.get()
    }

    /// An absolute footer intent saves the complete applied record first.
    /// A failed save changes neither logical results nor native presentation.
    fn change_launcher_display_mode(&self, mode: LauncherDisplayMode) {
        if self.interactive_launcher().is_none() {
            return;
        }
        let applied = self.core.applied_preferences();
        if applied.launcher().display_mode() == mode || self.preference_saving.get() {
            return;
        }
        if mode == LauncherDisplayMode::Fullscreen && self.core.dock_context().is_none() {
            self.report_message(
                "Could not save launcher display mode: monitor bounds are unavailable.",
            );
            return;
        }
        let Some(_save) = self.begin_preference_save() else {
            return;
        };
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
            self.cancel_launcher_reorder();
            self.flush_launcher_reorder_exit();
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
        self.cancel_launcher_reorder();
        self.dismiss_popups_except(None);
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
        drop(presentation);
        self.sync_launcher_reorder();
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
        self.cancel_launcher_reorder();
        self.hide_user_menu();
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
        if self.guarded()
            || self.preference_saving.get()
            || !session.visible.get()
            || session.presenting.get()
        {
            return None;
        }
        self.launcher_and_upgrade()
            .filter(|launcher| launcher.window().is_visible())
    }

    fn select_launcher(&self, key: &str) {
        if self.launcher_reorder_active() {
            return;
        }
        let Some(launcher) = self.interactive_launcher() else {
            return;
        };
        let index = self.launcher_state.borrow_mut().select(key);
        self.project_launcher_selection(&launcher, index);
    }

    fn navigate_launcher(&self, direction: LauncherNavigation) {
        if self.launcher_reorder_active() {
            return;
        }
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
        if self.launcher_reorder_active() {
            return;
        }
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
