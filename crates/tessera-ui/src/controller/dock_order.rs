// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native pinned-only input and measured-slot targeting. The displayed order
//! stays authoritative until the existing complete-record saver succeeds.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use slint::language::{DragAction, DropEvent, PointerEvent, PointerEventButton, PointerEventKind};
use slint::{ComponentHandle, DataTransfer, Model};

use super::PanelController;
use crate::SurfaceKind;
use crate::generated::{Dock, DockDragVisual, DockOrderMetrics, TileBounds};
use crate::launcher::order::{Bounds, Point, RelativePlacement, ReorderIntent, apply_reorder};

#[derive(Default)]
struct Marker;

struct Token {
    owner: Rc<Marker>,
    epoch: u64,
}

struct Gesture {
    token: Rc<Token>,
    projection: u64,
    saved: Vec<String>,
    resolved: Vec<String>,
    catalog: Vec<String>,
    slots: Vec<Bounds>,
    metrics: DockOrderMetrics,
    scale: f32,
    press: Point,
    visual: DockDragVisual,
    target: usize,
}

#[derive(Default)]
pub(super) struct DockOrder {
    marker: Rc<Marker>,
    gesture: RefCell<Option<Gesture>>,
    slots: RefCell<HashMap<String, Bounds>>,
    projection: Cell<u64>,
    epoch: Cell<u64>,
    callbacks: Cell<usize>,
    pending_exit: Cell<Option<u64>>,
    exit_dispatching: Cell<bool>,
    exit_timer: slint::Timer,
}

struct NativeCallback(Rc<DockOrder>);

impl Drop for NativeCallback {
    fn drop(&mut self) {
        let owner = &self.0;
        owner.callbacks.set(owner.callbacks.get() - 1);
        if owner.callbacks.get() == 0
            && owner.pending_exit.get().is_some()
            && !owner.exit_dispatching.get()
            && !owner.exit_timer.running()
        {
            owner.exit_timer.restart();
        }
    }
}

fn transfer<T: 'static>(payload: Rc<T>) -> DataTransfer {
    let mut data = DataTransfer::default();
    data.set_user_data(payload);
    data
}

fn bounds(value: &TileBounds) -> Bounds {
    Bounds {
        x: value.origin.x,
        y: value.origin.y,
        width: value.width,
        height: value.height,
    }
}

fn contains(rect: Bounds, point: Point) -> bool {
    [rect.x, rect.y, rect.width, rect.height, point.x, point.y]
        .into_iter()
        .all(f32::is_finite)
        && rect.width > 0.0
        && rect.height > 0.0
        && point.x >= rect.x
        && point.x < rect.x + rect.width
        && point.y >= rect.y
        && point.y < rect.y + rect.height
}

fn same_metrics(left: &DockOrderMetrics, right: &DockOrderMetrics) -> bool {
    bounds(&left.viewport) == bounds(&right.viewport)
        && left.vertical == right.vertical
        && left.edge == right.edge
}

fn keys(dock: &Dock) -> Vec<String> {
    dock.get_pinned_apps()
        .iter()
        .map(|app| app.key.to_string())
        .collect()
}

fn pointer(event: &DropEvent, origin: slint::LogicalPosition) -> Point {
    Point {
        x: origin.x + event.position.x,
        y: origin.y + event.position.y,
    }
}

impl PanelController {
    fn dock_callback(&self) -> NativeCallback {
        self.dock_order
            .callbacks
            .set(self.dock_order.callbacks.get() + 1);
        NativeCallback(Rc::clone(&self.dock_order))
    }

    fn dock_order_ready(&self, dock: &Dock) -> bool {
        self.dock_order.epoch.get() != u64::MAX
            && i32::try_from(self.dock_order.projection.get()).is_ok()
            && self.root_current()
            && self.bar_input_ready(SurfaceKind::Dock)
            && self.panel.upgrade().is_some_and(|panel| {
                panel.get_has_snapshot() && !panel.get_refreshing() && !panel.get_stale()
            })
            && !dock.get_surface_status().refreshing
            && !dock.get_surface_status().stale
    }

    fn dock_order_resolved(&self, dock: &Dock) -> Option<Vec<String>> {
        let resolved = keys(dock);
        let catalog = self.core.catalog();
        let saved = self.core.pins();
        let current: Vec<_> = saved
            .iter()
            .filter(|key| catalog.iter().any(|app| app.key() == key.as_str()))
            .cloned()
            .collect();
        (resolved.len() >= 2
            && resolved == current
            && dock.get_pinned_apps().iter().all(|app| app.pinned)
            && apply_reorder(
                &saved,
                &resolved,
                &ReorderIntent {
                    source_key: resolved.first().cloned().unwrap_or_default(),
                    anchor_key: resolved.first().cloned().unwrap_or_default(),
                    placement: RelativePlacement::Before,
                },
            )
            .is_ok())
        .then_some(resolved)
    }

    pub(super) fn sync_dock_reorder(&self) {
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        let locked = self.core.applied_preferences().dock_locked();
        dock.set_reorder_locked(locked);
        dock.set_reorder_lock_enabled(
            self.dock_order_ready(&dock) && !self.preference_saving.get(),
        );
        let token = self
            .dock_order
            .gesture
            .borrow()
            .as_ref()
            .map(|gesture| Rc::clone(&gesture.token));
        if token
            .as_ref()
            .is_some_and(|token| !self.dock_token_current(&dock, token))
        {
            self.cancel_dock_reorder();
            return;
        }
        dock.set_reorder_projection(
            i32::try_from(self.dock_order.projection.get()).unwrap_or(i32::MAX),
        );
        let enabled = self.dock_order_ready(&dock)
            && !locked
            && !self.preference_saving.get()
            && self.dock_order.pending_exit.get().is_none()
            && !self.dock_order.exit_dispatching.get()
            && self.dock_order_resolved(&dock).is_some();
        dock.set_reorder_enabled(enabled);
        if self.dock_order.gesture.borrow().is_none() {
            dock.set_reorder_data(transfer(Rc::clone(&self.dock_order.marker)));
        }
    }

    pub(super) fn dock_reorder_active(&self) -> bool {
        self.dock_and_upgrade()
            .is_some_and(|dock| dock.get_reorder_dragging())
    }

    /// Fence both ends of a model batch: native menu retirement may reenter
    /// and arm a new gesture while the old model is still being replaced.
    pub(super) fn invalidate_dock_projection(&self) {
        self.dock_order
            .projection
            .set(self.dock_order.projection.get().saturating_add(1));
        self.dock_order.slots.borrow_mut().clear();
        self.cancel_dock_reorder();
    }

    pub(super) fn cancel_dock_reorder(&self) {
        self.end_dock_reorder(true);
    }

    fn end_dock_reorder(&self, cancel_native: bool) {
        let Some(dock) = self.dock_and_upgrade() else {
            self.dock_order.gesture.borrow_mut().take();
            return;
        };
        let owner = &self.dock_order;
        let had_gesture = owner.gesture.borrow_mut().take().is_some();
        if !cancel_native {
            owner.pending_exit.set(None);
            owner.exit_timer.stop();
        }
        if had_gesture || (cancel_native && dock.get_reorder_dragging()) {
            let epoch = owner.epoch.get().saturating_add(1);
            owner.epoch.set(epoch);
            // Dispatch outside native callbacks: descendant Cancel is part of
            // DragArea takeover, and cannot safely trigger nested SDK teardown.
            if cancel_native && owner.pending_exit.get().is_none() {
                owner.pending_exit.set(Some(epoch));
                let weak_order = Rc::downgrade(owner);
                let weak_dock = dock.as_weak();
                owner
                    .exit_timer
                    .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                        let (Some(owner), Some(dock)) = (weak_order.upgrade(), weak_dock.upgrade())
                        else {
                            return;
                        };
                        if owner.pending_exit.get() == Some(epoch)
                            && owner.gesture.borrow().is_none()
                            && owner.callbacks.get() == 0
                        {
                            owner.exit_dispatching.set(true);
                            dock.window()
                                .dispatch_event(slint::platform::WindowEvent::PointerExited);
                            owner.exit_dispatching.set(false);
                            if owner.pending_exit.get() == Some(epoch) {
                                owner.pending_exit.set(None);
                            }
                            dock.invoke_reorder_metrics_changed();
                        }
                    });
            }
        }
        let epoch = owner.epoch.get();
        dock.set_reorder_visual(DockDragVisual::default());
        if owner.epoch.get() != epoch || owner.gesture.borrow().is_some() {
            return;
        }
        dock.set_reorder_target_key(Default::default());
        if owner.epoch.get() != epoch || owner.gesture.borrow().is_some() {
            return;
        }
        self.sync_dock_reorder();
    }

    fn dock_token_current(&self, dock: &Dock, token: &Rc<Token>) -> bool {
        if self.core.applied_preferences().dock_locked()
            || !self.dock_order_ready(dock)
            || self.preference_saving.get()
        {
            return false;
        }
        let catalog: Vec<_> = self
            .core
            .catalog()
            .iter()
            .map(|app| app.key().to_owned())
            .collect();
        let saved = self.core.pins();
        let resolved = keys(dock);
        let slots = self.dock_order.slots.borrow();
        self.dock_order
            .gesture
            .borrow()
            .as_ref()
            .is_some_and(|gesture| {
                Rc::ptr_eq(&token.owner, &self.dock_order.marker)
                    && Rc::ptr_eq(token, &gesture.token)
                    && token.epoch == self.dock_order.epoch.get()
                    && gesture.projection == self.dock_order.projection.get()
                    && gesture.saved == saved
                    && gesture.resolved == resolved
                    && gesture.catalog == catalog
                    && same_metrics(&gesture.metrics, &dock.get_reorder_metrics())
                    && gesture.scale == dock.window().scale_factor()
                    && gesture
                        .resolved
                        .iter()
                        .zip(&gesture.slots)
                        .all(|(key, captured)| slots.get(key) == Some(captured))
            })
    }

    fn dock_reorder_origin(
        &self,
        mut visual: DockDragVisual,
        event: PointerEvent,
        source_bounds: TileBounds,
        press: slint::LogicalPosition,
    ) {
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        if event.kind == PointerEventKind::Up && !dock.get_reorder_dragging() {
            self.end_dock_reorder(false);
            return;
        }
        // Child Cancel precedes native dragging=true. Only native finished,
        // explicit lifecycle cancellation, or a changed layout retires it.
        if event.kind != PointerEventKind::Down {
            return;
        }
        if self.dock_order.exit_dispatching.get() || self.dock_order.callbacks.get() > 1 {
            return;
        }
        self.cancel_dock_reorder();
        if event.button != PointerEventButton::Left
            || dock.get_reorder_dragging()
            || self.dock_order.pending_exit.get().is_some()
            || !self.dock_order_ready(&dock)
            || self.preference_saving.get()
            || self.core.applied_preferences().dock_locked()
        {
            return;
        }
        let Some(resolved) = self.dock_order_resolved(&dock) else {
            return;
        };
        let Some(source) = resolved
            .iter()
            .position(|key| key == visual.source.key.as_str())
        else {
            return;
        };
        let Some(app) = dock.get_pinned_apps().row_data(source) else {
            return;
        };
        if !visual.source.pinned || app.key != visual.source.key || app.label != visual.source.label
        {
            return;
        }
        let point = Point {
            x: press.x,
            y: press.y,
        };
        let Some(slots) = resolved
            .iter()
            .map(|key| self.dock_order.slots.borrow().get(key).copied())
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        if slots[source] != bounds(&source_bounds)
            || !contains(slots[source], point)
            || !contains(bounds(&dock.get_reorder_metrics().viewport), point)
            || slots.iter().any(|slot| {
                !contains(
                    *slot,
                    Point {
                        x: slot.x,
                        y: slot.y,
                    },
                )
            })
        {
            return;
        }
        let epoch = self.dock_order.epoch.get().saturating_add(1);
        self.dock_order.epoch.set(epoch);
        if epoch == u64::MAX {
            self.sync_dock_reorder();
            return;
        }
        let token = Rc::new(Token {
            owner: Rc::clone(&self.dock_order.marker),
            epoch,
        });
        visual.visible = false;
        visual.bounds = source_bounds;
        *self.dock_order.gesture.borrow_mut() = Some(Gesture {
            token: Rc::clone(&token),
            projection: self.dock_order.projection.get(),
            saved: self.core.pins(),
            resolved,
            catalog: self
                .core
                .catalog()
                .iter()
                .map(|app| app.key().to_owned())
                .collect(),
            slots,
            metrics: dock.get_reorder_metrics(),
            scale: dock.window().scale_factor(),
            press: point,
            visual,
            target: source,
        });
        dock.set_reorder_data(transfer(token));
    }

    fn dock_reorder_hover(
        &self,
        event: &DropEvent,
        origin: slint::LogicalPosition,
        targeting: bool,
    ) -> DragAction {
        let Some(dock) = self.dock_and_upgrade() else {
            return DragAction::None;
        };
        let Some(token) = event
            .data
            .user_data()
            .and_then(|data| data.downcast::<Token>().ok())
        else {
            return DragAction::None;
        };
        if !dock.get_reorder_dragging() || !self.dock_token_current(&dock, &token) {
            if Rc::ptr_eq(&token.owner, &self.dock_order.marker) {
                self.cancel_dock_reorder();
            }
            return DragAction::None;
        }
        let point = pointer(event, origin);
        if !point.x.is_finite() || !point.y.is_finite() {
            return DragAction::None;
        }
        let (visual, target) = {
            let mut gesture = self.dock_order.gesture.borrow_mut();
            let gesture = gesture.as_mut().expect("validated native gesture");
            let mut visual = gesture.visual.clone();
            visual.visible = true;
            visual.bounds.origin.x += point.x - gesture.press.x;
            visual.bounds.origin.y += point.y - gesture.press.y;
            let target = targeting
                .then(|| {
                    gesture
                        .slots
                        .iter()
                        .enumerate()
                        .min_by(|(_, left), (_, right)| {
                            let distance = |rect: &&Bounds| {
                                if gesture.metrics.vertical {
                                    (point.y - rect.y - rect.height / 2.0).abs()
                                } else {
                                    (point.x - rect.x - rect.width / 2.0).abs()
                                }
                            };
                            distance(left).total_cmp(&distance(right))
                        })
                        .map(|(index, _)| index)
                })
                .flatten()
                .filter(|_| contains(bounds(&gesture.metrics.viewport), point));
            if let Some(index) = target {
                gesture.target = index;
            }
            (visual, target.map(|index| gesture.resolved[index].clone()))
        };
        dock.set_reorder_visual(visual);
        dock.set_reorder_target_key(target.clone().unwrap_or_default().into());
        self.dismiss_tooltip(false);
        if target.is_some() {
            DragAction::Move
        } else {
            DragAction::None
        }
    }

    fn persist_dock_order(
        &self,
        projection: u64,
        saved: Vec<String>,
        resolved: Vec<String>,
        intent: ReorderIntent,
    ) -> DragAction {
        let Ok(Some(pins)) = apply_reorder(&saved, &resolved, &intent) else {
            return DragAction::None;
        };
        let Some(_save) = self.begin_preference_save() else {
            return DragAction::None;
        };
        // Closing a menu/native hint can reenter and replace the entire root.
        let Some(dock) = self.dock_and_upgrade() else {
            return DragAction::None;
        };
        if !self.dock_order_ready(&dock)
            || self.core.applied_preferences().dock_locked()
            || self.dock_order.projection.get() != projection
            || self.core.pins() != saved
            || self.dock_order_resolved(&dock).as_ref() != Some(&resolved)
        {
            return DragAction::None;
        }
        let preferences = self
            .core
            .applied_preferences()
            .with_dock(self.core.applied_dock_edge(), pins);
        match self.core.host().save_preferences(&preferences) {
            Ok(()) => {
                self.core.record_applied(&preferences);
                if self.root_current() {
                    self.report_message("Pinned application order saved");
                    if self.root_current() {
                        self.render();
                    }
                }
                DragAction::Move
            }
            Err(error) => {
                // No optimistic model was published; prior/current real order
                // is still displayed even if the host replaced the projection.
                if self.root_current() {
                    self.report_message(&format!(
                        "Could not save pin order: {}",
                        crate::sanitize::bounded_text(&error, 200)
                    ));
                }
                DragAction::None
            }
        }
    }

    fn dock_reorder_dropped(&self, event: DropEvent, origin: slint::LogicalPosition) -> DragAction {
        if self.dock_reorder_hover(&event, origin, true) != DragAction::Move {
            self.cancel_dock_reorder();
            return DragAction::None;
        }
        let completed = self.dock_order.gesture.borrow().as_ref().map(|gesture| {
            let source = gesture
                .resolved
                .iter()
                .position(|key| key == gesture.visual.source.key.as_str())
                .expect("captured pinned source");
            (
                gesture.projection,
                gesture.saved.clone(),
                gesture.resolved.clone(),
                ReorderIntent {
                    source_key: gesture.visual.source.key.to_string(),
                    anchor_key: gesture.resolved[gesture.target].clone(),
                    placement: if gesture.target > source {
                        RelativePlacement::After
                    } else {
                        RelativePlacement::Before
                    },
                },
            )
        });
        self.end_dock_reorder(false);
        let Some((projection, saved, resolved, intent)) = completed else {
            return DragAction::None;
        };
        self.persist_dock_order(projection, saved, resolved, intent)
    }

    fn move_dock_pin(&self, key: &str, later: bool) {
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        if self.core.applied_preferences().dock_locked()
            || !self.dock_order_ready(&dock)
            || self.preference_saving.get()
        {
            return;
        }
        let Some(resolved) = self.dock_order_resolved(&dock) else {
            return;
        };
        let Some(index) = resolved.iter().position(|candidate| candidate == key) else {
            return;
        };
        let target = if later {
            index.checked_add(1).filter(|index| *index < resolved.len())
        } else {
            index.checked_sub(1)
        };
        let Some(target) = target else { return };
        let intent = ReorderIntent {
            source_key: key.to_owned(),
            anchor_key: resolved[target].clone(),
            placement: if later {
                RelativePlacement::After
            } else {
                RelativePlacement::Before
            },
        };
        self.persist_dock_order(
            self.dock_order.projection.get(),
            self.core.pins(),
            resolved,
            intent,
        );
    }

    fn set_dock_locked(&self, locked: bool) {
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        if !dock.get_reorder_lock_available()
            || !self.dock_order_ready(&dock)
            || self.preference_saving.get()
        {
            return;
        }
        let Some(_save) = self.begin_preference_save() else {
            return;
        };
        // Native popup retirement can replace the source/root during admission.
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        if !dock.get_reorder_lock_available() || !self.dock_order_ready(&dock) {
            return;
        }
        let applied = self.core.applied_preferences();
        if applied.dock_locked() == locked {
            return;
        }
        let preferences = applied.with_dock_locked(locked);
        match self.core.host().save_preferences(&preferences) {
            Ok(()) => {
                self.core.record_applied(&preferences);
                self.cancel_dock_reorder();
                if self.root_current() {
                    self.report_message(if locked {
                        "Dock locked"
                    } else {
                        "Dock unlocked"
                    });
                    if self.root_current() {
                        self.render();
                    }
                }
            }
            Err(error) => {
                if self.root_current() {
                    self.report_message(&format!(
                        "Could not save Dock lock: {}",
                        crate::sanitize::bounded_text(&error, 200)
                    ));
                }
            }
        }
    }

    pub(super) fn wire_dock_reorder(&self, dock: &Dock) {
        dock.set_reorder_lock_available(true);
        let controller = self.clone();
        dock.on_reorder_lock_requested(move |locked| controller.set_dock_locked(locked));
        let controller = self.clone();
        dock.on_reorder_origin(move |visual, event, bounds, press| {
            let _callback = controller.dock_callback();
            controller.dock_reorder_origin(visual, event, bounds, press);
        });
        let controller = self.clone();
        dock.on_reorder_slot_measured(move |key, value, revision| {
            let _callback = controller.dock_callback();
            if !controller.root_current()
                || i32::try_from(controller.dock_order.projection.get()).ok() != Some(revision)
            {
                return;
            }
            let changed = controller
                .dock_order
                .gesture
                .borrow()
                .as_ref()
                .is_some_and(|gesture| {
                    gesture
                        .resolved
                        .iter()
                        .position(|candidate| candidate == key.as_str())
                        .is_some_and(|index| gesture.slots[index] != bounds(&value))
                });
            controller
                .dock_order
                .slots
                .borrow_mut()
                .insert(key.to_string(), bounds(&value));
            if changed {
                controller.cancel_dock_reorder();
            }
        });
        let controller = self.clone();
        dock.on_reorder_can_drop(move |event, origin| {
            let _callback = controller.dock_callback();
            controller.dock_reorder_hover(&event, origin, true)
        });
        let controller = self.clone();
        dock.on_reorder_window_hover(move |event, origin| {
            let _callback = controller.dock_callback();
            controller.dock_reorder_hover(&event, origin, false);
        });
        let controller = self.clone();
        dock.on_reorder_dropped(move |event, origin| {
            let _callback = controller.dock_callback();
            controller.dock_reorder_dropped(event, origin)
        });
        let controller = self.clone();
        dock.on_reorder_dragging_changed(move |active| {
            let _callback = controller.dock_callback();
            if active {
                let token = controller
                    .dock_order
                    .gesture
                    .borrow()
                    .as_ref()
                    .map(|gesture| Rc::clone(&gesture.token));
                if controller.dock_and_upgrade().is_none_or(|dock| {
                    token
                        .as_ref()
                        .is_none_or(|token| !controller.dock_token_current(&dock, token))
                }) {
                    controller.cancel_dock_reorder();
                }
            }
            if controller.root_current() {
                controller.update_geometry();
            }
        });
        let controller = self.clone();
        dock.on_reorder_finished(move |_| {
            let _callback = controller.dock_callback();
            controller.end_dock_reorder(false);
        });
        let controller = self.clone();
        dock.on_reorder_metrics_changed(move || {
            let _callback = controller.dock_callback();
            let token = controller
                .dock_order
                .gesture
                .borrow()
                .as_ref()
                .map(|gesture| Rc::clone(&gesture.token));
            if controller.dock_and_upgrade().is_none_or(|dock| {
                token
                    .as_ref()
                    .is_some_and(|token| !controller.dock_token_current(&dock, token))
            }) {
                controller.cancel_dock_reorder();
            }
            controller.sync_dock_reorder();
        });
        let controller = self.clone();
        dock.on_reorder_move_requested(move |key, later| {
            let _callback = controller.dock_callback();
            controller.move_dock_pin(&key, later);
        });
        self.sync_dock_reorder();
    }
}
