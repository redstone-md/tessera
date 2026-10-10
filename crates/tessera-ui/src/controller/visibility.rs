// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Existing observation owns uncapped window facts. Pointer hints never observe
//! windows, change focus, or own bar leases; revision-validated root placement does.

use slint::ComponentHandle;
use tessera_core::Rect;
use tessera_system::visibility::{Edge, VisibilityFacts, any_window_overlaps};

use super::{PanelController, Rc};
use crate::visibility::{
    BarFacts, BarVisibility, VisibilityConfig, VisibilityController, VisibilityGeometry,
    VisibilityObservations,
};
use crate::{DockContext, DockEdge, SurfaceKind};

#[derive(Clone, Copy, PartialEq)]
struct Placement {
    monitor: Rect,
    dock: Rect,
    toolbar: Rect,
    dock_edge: Edge,
    scale: f32,
}

#[derive(Default)]
pub(super) struct RootGeometry {
    revision: u64,
    placement: Option<Placement>,
    last_valid: Option<Placement>,
    desired: BarVisibility,
    attempted: bool,
}

impl RootGeometry {
    pub(super) fn desired_visible(&self, kind: SurfaceKind) -> bool {
        self.placement.is_none()
            || match kind {
                SurfaceKind::Dock => self.desired.dock,
                SurfaceKind::Toolbar => self.desired.toolbar,
                _ => true,
            }
    }
}

fn edge(edge: DockEdge) -> Edge {
    match edge {
        DockEdge::Bottom => Edge::Bottom,
        DockEdge::Top => Edge::Top,
        DockEdge::Left => Edge::Left,
        DockEdge::Right => Edge::Right,
    }
}

#[cfg(any(windows, test))]
fn own_focus(window: &slint::Window) -> Option<bool> {
    use slint::winit_030::WinitWindowAccessor;
    window.with_winit_window(|window| window.has_focus())
}

#[cfg(not(any(windows, test)))]
fn own_focus(_window: &slint::Window) -> Option<bool> {
    None
}

impl PanelController {
    fn bar_desired_visible(&self, kind: SurfaceKind) -> bool {
        if self.power_admission_closed.get() {
            return false;
        }
        self.visibility_geometry.borrow().desired_visible(kind)
    }
    pub(super) fn bar_input_ready(&self, kind: SurfaceKind) -> bool {
        self.bar_desired_visible(kind)
            && match kind {
                SurfaceKind::Dock => self
                    .dock_and_upgrade()
                    .is_some_and(|dock| dock.window().is_visible()),
                SurfaceKind::Toolbar => self
                    .toolbar_and_upgrade()
                    .is_some_and(|toolbar| toolbar.window().is_visible()),
                _ => true,
            }
    }
    pub(super) fn wire_visibility(&self, panel: &crate::generated::Panel) {
        let controller = self.clone();
        panel.on_visibility_event_ready(move || {
            if controller.power_admission_closed.get() {
                return;
            }
            let actor = controller.visibility.borrow().clone();
            if let Some(actor) = actor {
                actor.process_events();
            }
            if !controller.power_admission_closed.get() {
                controller.update_geometry();
            }
        });
    }

    fn initialize_visibility(&self) {
        let attempted = {
            let mut geometry = self.visibility_geometry.borrow_mut();
            let attempted = geometry.attempted;
            geometry.attempted = true;
            attempted
        };
        if attempted || self.power_admission_closed.get() {
            return;
        }
        let provider = match self.core.host().pointer_host() {
            Ok(Some(provider)) => provider,
            Ok(None) => return,
            Err(error) => {
                self.report_message(&format!(
                    "Passive pointer notifications unavailable: {error}"
                ));
                return;
            }
        };
        if self.power_admission_closed.get() {
            return;
        }
        let root = Rc::downgrade(&self.visibility_geometry);
        let closed = Rc::downgrade(&self.power_admission_closed);
        let panel = self.panel.clone();
        let actor = VisibilityController::new(provider, move |desired, revision| {
            let Some(closed) = closed.upgrade() else {
                return;
            };
            if closed.get() {
                return;
            }
            let Some(root) = root.upgrade() else {
                return;
            };
            let accepted = {
                let mut root = root.borrow_mut();
                if root.placement.is_none() || root.revision != revision {
                    false
                } else {
                    root.desired = desired;
                    true
                }
            };
            if accepted && let Some(panel) = panel.upgrade() {
                panel.invoke_visibility_event_ready();
            }
        });
        if let Some(panel) = self.panel.upgrade() {
            actor.attach_wake(&panel, |panel| panel.invoke_visibility_event_ready());
        } else {
            actor.close();
            return;
        }
        let published = {
            let mut cache = self.visibility.borrow_mut();
            if cache.is_none() {
                *cache = Some(Rc::clone(&actor));
                true
            } else {
                false
            }
        };
        if !published {
            actor.close();
        }
    }

    pub(super) fn visibility_for_geometry(
        &self,
        context: DockContext,
        dock_rect: crate::dock::DockRect,
        toolbar_rect: crate::dock::DockRect,
        dock_edge: DockEdge,
        scale: f32,
    ) -> Result<BarVisibility, String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err("Bar scale is not a finite positive native observation".into());
        }
        let rect =
            |x, y, width, height| Rect::new(x, y, width, height).map_err(|error| error.to_string());
        let placement = Placement {
            monitor: rect(context.x(), context.y(), context.width(), context.height())?,
            dock: rect(dock_rect.x, dock_rect.y, dock_rect.width, dock_rect.height)?,
            toolbar: rect(
                toolbar_rect.x,
                toolbar_rect.y,
                toolbar_rect.width,
                toolbar_rect.height,
            )?,
            dock_edge: edge(dock_edge),
            scale,
        };
        let placement_changed = self.visibility_geometry.borrow().placement != Some(placement);
        if placement_changed {
            self.cancel_dock_reorder();
        }
        let revision = {
            let mut root = self.visibility_geometry.borrow_mut();
            if root.placement != Some(placement) {
                root.revision = root
                    .revision
                    .checked_add(1)
                    .ok_or("Bar geometry revision exhausted")?;
                root.placement = Some(placement);
                root.last_valid = Some(placement);
                root.desired = BarVisibility::default();
            }
            root.revision
        };
        self.initialize_visibility();
        let actor = self.visibility.borrow().clone();
        let has_actor = actor.is_some();
        if let Some(actor) = actor {
            let snapshot = self.core.retained_snapshot();
            let overlap = |hitbox| {
                snapshot.as_ref().and_then(|snapshot| {
                    snapshot
                        .visibility_facts()
                        .map(|(windows, foreground_interactable)| {
                            any_window_overlaps(&VisibilityFacts {
                                monitor: placement.monitor,
                                hitbox,
                                foreground_interactable,
                                windows,
                            })
                        })
                })
            };
            let dragging = self.dock_reorder_active()
                || self
                    .launcher_and_upgrade()
                    .is_some_and(|launcher| launcher.get_reorder_dragging());
            let dock_focus = self
                .dock_and_upgrade()
                .and_then(|dock| own_focus(dock.window()));
            let toolbar_focus = self
                .toolbar_and_upgrade()
                .and_then(|toolbar| own_focus(toolbar.window()));
            let facts = |overlap, own_focus| BarFacts {
                overlap,
                confirmed_fullscreen: context.fullscreen_active(),
                own_focus,
                // Native environment readiness is merged by the owned actor.
                touch_primary: None,
                dragging,
                ..BarFacts::default()
            };
            actor.update(
                VisibilityConfig::default(),
                Some(VisibilityGeometry {
                    revision,
                    monitor: placement.monitor,
                    dock_edge: placement.dock_edge,
                    toolbar_edge: Edge::Top,
                }),
                VisibilityObservations {
                    dock: facts(overlap(placement.dock), dock_focus),
                    toolbar: facts(overlap(placement.toolbar), toolbar_focus),
                },
            );
            if actor.readiness() == crate::visibility::PointerReadiness::Inactive
                && let Err(error) = actor.start()
            {
                self.report_message(&format!(
                    "Passive pointer notifications unavailable: {error}"
                ));
            }
        }
        let mut root = self.visibility_geometry.borrow_mut();
        if root.revision != revision || root.placement != Some(placement) {
            return Ok(BarVisibility::default());
        }
        if context.fullscreen_active() {
            root.desired = BarVisibility {
                dock: false,
                toolbar: false,
            };
        } else if !has_actor {
            root.desired = BarVisibility::default();
        }
        Ok(root.desired)
    }

    /// Missing geometry revokes timers and popup scope; the retained validated
    /// frame is recovery placement only and must never count as fresh readiness.
    pub(super) fn retire_visibility_geometry(&self) -> Result<(), String> {
        self.stop_battery_root();
        self.cancel_dock_reorder();
        let (retired, frame, revision) = {
            let mut root = self.visibility_geometry.borrow_mut();
            let retired = root.placement.take().is_some();
            if retired {
                root.revision = root
                    .revision
                    .checked_add(1)
                    .ok_or("Bar geometry revision exhausted")?;
            }
            root.desired = BarVisibility::default();
            (retired, root.last_valid, root.revision)
        };
        let actor = self.visibility.borrow().clone();
        if let Some(actor) = actor {
            actor.update(
                VisibilityConfig::default(),
                None,
                VisibilityObservations::default(),
            );
        }
        if self.power_admission_closed.get() {
            return Ok(());
        }
        if retired {
            self.dismiss_tooltip(false);
            // Power has its own display provider and remains independent.
            self.dismiss_popups_except(Some(super::popups::PopupKind::Power));
        }
        let operation = self.popup_operation.borrow().clone();
        let current = || {
            !self.power_admission_closed.get()
                && self.visibility_geometry.borrow().revision == revision
                && self.visibility_geometry.borrow().placement.is_none()
                && Rc::ptr_eq(&operation, &self.popup_operation.borrow())
        };
        let Some(frame) = frame else {
            return Ok(());
        };
        let rect = |rect: Rect| crate::dock::DockRect {
            x: rect.x(),
            y: rect.y(),
            width: rect.width(),
            height: rect.height(),
        };
        if current()
            && let Some(dock) = self.dock_and_upgrade()
            && dock.window().scale_factor() == frame.scale
        {
            self.place_bar(
                SurfaceKind::Dock,
                dock.window(),
                rect(frame.dock),
                true,
                Some(Rc::clone(&operation)),
            )?;
        }
        if current()
            && let Some(toolbar) = self.toolbar_and_upgrade()
            && toolbar.window().scale_factor() == frame.scale
        {
            self.place_bar(
                SurfaceKind::Toolbar,
                toolbar.window(),
                rect(frame.toolbar),
                true,
                Some(operation.clone()),
            )?;
        }
        Ok(())
    }

    pub(super) fn place_bar(
        &self,
        kind: SurfaceKind,
        window: &slint::Window,
        rect: crate::dock::DockRect,
        visible: bool,
        operation: Option<Rc<()>>,
    ) -> Result<(), String> {
        if self.power_admission_closed.get() {
            return Ok(());
        }
        let revision = self.visibility_geometry.borrow().revision;
        let current = || {
            !self.power_admission_closed.get()
                && self.visibility_geometry.borrow().revision == revision
                && self.bar_desired_visible(kind) == visible
                && operation
                    .as_ref()
                    .is_none_or(|operation| Rc::ptr_eq(operation, &self.popup_operation.borrow()))
        };
        if !current() {
            return Ok(());
        }
        let tuple = (rect.x, rect.y, rect.width, rect.height);
        if !visible {
            // Revoke affected popup input and drop attachment before native hide.
            if kind == SurfaceKind::Toolbar {
                self.stop_battery_root();
            }
            self.popup_operation.replace(Rc::new(()));
            self.dismiss_tooltip(false);
            if kind == SurfaceKind::Dock {
                self.cancel_dock_reorder();
                if !current() {
                    return Ok(());
                }
                let menu = self.menus.borrow().clone();
                if let Some(menu) = menu {
                    menu.hide();
                }
                self.recycle_bin_hidden();
            } else {
                self.dismiss_popups_except(None);
            }
            if !current() {
                return Ok(());
            }
            self.detach_lease(kind);
            if !current() {
                return Ok(());
            }
            if window.is_visible() {
                window.hide().map_err(|error| error.to_string())?;
            }
            return Ok(());
        }
        let changed = self.leases.borrow().geometry_changed(kind, tuple);
        if changed {
            if kind == SurfaceKind::Dock {
                self.cancel_dock_reorder();
                if !current() {
                    return Ok(());
                }
            }
            self.detach_lease(kind);
            if !current() {
                return Ok(());
            }
            window.set_size(slint::PhysicalSize::new(rect.width, rect.height));
            window.set_position(slint::PhysicalPosition::new(rect.x, rect.y));
        }
        if !current() {
            return Ok(());
        }
        if !window.is_visible() {
            window.show().map_err(|error| error.to_string())?;
        }
        if !current() {
            return Ok(());
        }
        if changed {
            self.configure_lease_if(kind, window, tuple, current)?;
        }
        if kind == SurfaceKind::Dock {
            self.recycle_bin_shown();
            if !current() {
                return Ok(());
            }
            self.sync_dock_reorder();
        } else if kind == SurfaceKind::Toolbar && current() {
            self.sync_battery_root();
        }
        Ok(())
    }
}
