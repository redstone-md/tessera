// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One captured native gesture owns a complete group request; no per-member repaint.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::language::{PointerEvent, PointerEventButton, PointerEventKind};
use slint::{ComponentHandle, Model};

use super::PanelController;
use crate::generated::{Dock, TileBounds};
use crate::{DockContext, DockMiddleClickAction, SurfaceKind, WindowAction, sanitize};

#[derive(Default)]
pub(super) struct DockMiddleClick {
    epoch: RefCell<Rc<()>>,
    gesture: RefCell<Option<Gesture>>,
    running: Cell<bool>,
    projection_depth: Cell<usize>,
}

impl DockMiddleClick {
    fn retire(&self) {
        self.epoch.replace(Rc::new(()));
        self.gesture.borrow_mut().take();
    }
}

pub(super) struct DockMiddleProjection(Rc<DockMiddleClick>);

impl Drop for DockMiddleProjection {
    fn drop(&mut self) {
        self.0.retire();
        self.0
            .projection_depth
            .set(self.0.projection_depth.get().saturating_sub(1));
    }
}

#[derive(PartialEq)]
struct Frame {
    rect: (i32, i32, u32, u32),
    scale: f32,
    viewport: [f32; 4],
    scroll: [f32; 2],
    compact: bool,
    vertical: bool,
    edge: i32,
    projection: i32,
    context: DockContext,
}

#[derive(PartialEq, Eq)]
struct Targets {
    launch: Option<String>,
    members: Vec<String>,
}

struct Gesture {
    epoch: Rc<()>,
    key: String,
    pinned: bool,
    action: DockMiddleClickAction,
    frame: Frame,
    slot: [f32; 4],
    targets: Targets,
    released: bool,
}

struct Flight<'a>(&'a DockMiddleClick);

impl Drop for Flight<'_> {
    fn drop(&mut self) {
        self.0.running.set(false);
    }
}

fn bounds(value: &TileBounds) -> [f32; 4] {
    [value.origin.x, value.origin.y, value.width, value.height]
}

fn contains(rect: [f32; 4], point: slint::LogicalPosition) -> bool {
    rect.iter().all(|value| value.is_finite())
        && rect[2] > 0.0
        && rect[3] > 0.0
        && point.x.is_finite()
        && point.y.is_finite()
        && point.x >= rect[0]
        && point.y >= rect[1]
        && point.x < rect[0] + rect[2]
        && point.y < rect[1] + rect[3]
}

fn admitted_point(frame: &Frame, slot: [f32; 4], point: slint::LogicalPosition) -> bool {
    contains(slot, point)
        && contains(frame.viewport, point)
        && contains(
            [
                0.0,
                0.0,
                frame.rect.2 as f32 / frame.scale,
                frame.rect.3 as f32 / frame.scale,
            ],
            point,
        )
}

impl PanelController {
    pub(super) fn cancel_dock_middle_click(&self) {
        self.dock_middle_click.retire();
    }

    pub(super) fn begin_dock_middle_click_projection(&self) -> DockMiddleProjection {
        self.cancel_dock_middle_click();
        let state = &self.dock_middle_click;
        state
            .projection_depth
            .set(state.projection_depth.get().saturating_add(1));
        DockMiddleProjection(Rc::clone(state))
    }

    /// Fence every projection replacement, including replacements with identical keys.
    pub(super) fn sync_dock_middle_click(&self) {
        self.cancel_dock_middle_click();
        let epoch = self.dock_middle_click.epoch.borrow().clone();
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        if !self.root_current() {
            return;
        }
        let label = match self.core.applied_preferences().dock_middle_click() {
            DockMiddleClickAction::NewInstance => "New instance",
            DockMiddleClickAction::Minimize => "Minimize windows",
            DockMiddleClickAction::Close => "Close windows",
        };
        dock.set_middle_click_label(label.into());
        if self.root_current() && Rc::ptr_eq(&epoch, &self.dock_middle_click.epoch.borrow()) {
            dock.set_middle_click_available(true);
        }
    }

    pub(super) fn wire_dock_middle_click(&self, dock: &Dock) {
        let root = self.clone();
        dock.on_middle_click_pointer(move |key, pinned, event, slot, point| {
            root.dock_middle_pointer(&key, pinned, event, slot, point);
        });
        let root = self.clone();
        dock.on_middle_click_requested(move |key, pinned| {
            root.request_dock_middle_click(&key, pinned);
        });
        let root = self.clone();
        dock.on_middle_click_frame_changed(move || root.cancel_dock_middle_click());
        self.sync_dock_middle_click();
    }

    fn dock_middle_frame(&self, dock: &Dock) -> Option<Frame> {
        if !self.root_current()
            || !self.bar_input_ready(SurfaceKind::Dock)
            || self.preference_saving.get()
            || self.dock_middle_click.projection_depth.get() != 0
            || self.dock_reorder_active()
            || !dock.get_middle_click_available()
            || dock.get_surface_status().refreshing
            || dock.get_surface_status().stale
            || !self.panel.upgrade().is_some_and(|panel| {
                panel.get_has_snapshot() && !panel.get_refreshing() && !panel.get_stale()
            })
            || !self
                .dock_and_upgrade()
                .is_some_and(|owned| std::ptr::eq(owned.window(), dock.window()))
        {
            return None;
        }
        let context = self.core.dock_context()?;
        let position = dock.window().position();
        let size = dock.window().size();
        let scale = dock.window().scale_factor();
        let rect = (position.x, position.y, size.width, size.height);
        if context.fullscreen_active()
            || size.width == 0
            || size.height == 0
            || !scale.is_finite()
            || scale <= 0.0
            || self
                .leases
                .borrow()
                .geometry_changed(SurfaceKind::Dock, rect)
        {
            return None;
        }
        let metrics = dock.get_reorder_metrics();
        let viewport = bounds(&metrics.viewport);
        let scroll = dock.get_middle_click_scroll();
        if !viewport.iter().all(|value| value.is_finite())
            || viewport[2] <= 0.0
            || viewport[3] <= 0.0
            || !scroll.x.is_finite()
            || !scroll.y.is_finite()
        {
            return None;
        }
        Some(Frame {
            rect,
            scale,
            viewport,
            scroll: [scroll.x, scroll.y],
            compact: dock.get_compact(),
            vertical: metrics.vertical,
            edge: metrics.edge,
            projection: dock.get_reorder_projection(),
            context,
        })
    }

    /// Resolve only the retained proven grouping and exact current trusted catalog.
    fn dock_middle_targets(&self, dock: &Dock, key: &str, pinned: bool) -> Option<Targets> {
        let snapshot = self.core.retained_snapshot()?;
        let groups = crate::projection::dock_groups(&snapshot);
        let catalog = self.core.catalog();
        let (members, launch) = if pinned {
            if !self.core.pins().iter().any(|pin| pin == key)
                || dock
                    .get_pinned_apps()
                    .iter()
                    .filter(|app| app.key == key && app.pinned)
                    .count()
                    != 1
            {
                return None;
            }
            let mut matches = catalog.iter().filter(|app| app.key() == key);
            let app = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            let identity = format!("aumid:{}", crate::projection::catalog_aumid(app.key()));
            let members = groups
                .iter()
                .find(|members| members[0].application_identity() == Some(identity.as_str()));
            (members, Some(app.key().to_owned()))
        } else {
            if dock
                .get_running_windows()
                .iter()
                .filter(|window| window.key == key)
                .count()
                != 1
            {
                return None;
            }
            let members = groups.iter().find(|members| members[0].key() == key)?;
            let launch = members[0].application_identity().and_then(|identity| {
                let mut matches = catalog.iter().filter(|app| {
                    identity == format!("aumid:{}", crate::projection::catalog_aumid(app.key()))
                });
                let app = matches.next()?;
                matches.next().is_none().then(|| app.key().to_owned())
            });
            (Some(members), launch)
        };
        let members: Vec<_> = members
            .into_iter()
            .flatten()
            .map(|window| window.key().to_owned())
            .collect();
        (dock.displayed_group_window_keys(key, pinned)? == members)
            .then_some(Targets { launch, members })
    }

    fn dock_middle_current(&self, dock: &Dock, gesture: &Gesture) -> bool {
        Rc::ptr_eq(&gesture.epoch, &self.dock_middle_click.epoch.borrow())
            && self.core.applied_preferences().dock_middle_click() == gesture.action
            && self.dock_middle_frame(dock).as_ref() == Some(&gesture.frame)
            && self
                .dock_middle_targets(dock, &gesture.key, gesture.pinned)
                .as_ref()
                == Some(&gesture.targets)
            && self.root_current()
            && Rc::ptr_eq(&gesture.epoch, &self.dock_middle_click.epoch.borrow())
    }

    fn dock_middle_pointer(
        &self,
        key: &str,
        pinned: bool,
        event: PointerEvent,
        slot: TileBounds,
        point: slint::LogicalPosition,
    ) {
        if event.kind == PointerEventKind::Cancel {
            self.cancel_dock_middle_click();
            return;
        }
        if event.button != PointerEventButton::Middle {
            return;
        }
        let Some(dock) = self.dock_and_upgrade() else {
            self.cancel_dock_middle_click();
            return;
        };
        if event.kind == PointerEventKind::Down {
            self.cancel_dock_reorder();
            self.cancel_dock_middle_click();
            if self.dock_middle_click.running.get() {
                return;
            }
            let epoch = self.dock_middle_click.epoch.borrow().clone();
            let Some(frame) = self.dock_middle_frame(&dock) else {
                return;
            };
            let slot = bounds(&slot);
            if !admitted_point(&frame, slot, point) {
                return;
            }
            let Some(targets) = self.dock_middle_targets(&dock, key, pinned) else {
                return;
            };
            let gesture = Gesture {
                epoch,
                key: key.to_owned(),
                pinned,
                action: self.core.applied_preferences().dock_middle_click(),
                frame,
                slot,
                targets,
                released: false,
            };
            if self.dock_middle_current(&dock, &gesture) {
                self.dock_middle_click.gesture.replace(Some(gesture));
            }
        } else if event.kind == PointerEventKind::Up {
            let pending = self.dock_middle_click.gesture.borrow_mut().take();
            if let Some(mut gesture) = pending
                && gesture.key == key
                && gesture.pinned == pinned
                && gesture.slot == bounds(&slot)
                && admitted_point(&gesture.frame, gesture.slot, point)
                && self.dock_middle_current(&dock, &gesture)
            {
                gesture.released = true;
                self.dock_middle_click.gesture.replace(Some(gesture));
            }
        }
    }

    fn request_dock_middle_click(&self, key: &str, pinned: bool) {
        if self.dock_middle_click.running.get() {
            self.cancel_dock_middle_click();
            return;
        }
        let Some(gesture) = self.dock_middle_click.gesture.borrow_mut().take() else {
            return;
        };
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        if !gesture.released
            || gesture.key != key
            || gesture.pinned != pinned
            || !self.dock_middle_current(&dock, &gesture)
        {
            return;
        }
        self.dock_middle_click.running.set(true);
        let _flight = Flight(&self.dock_middle_click);
        let (label, action) = match gesture.action {
            DockMiddleClickAction::NewInstance => ("New instance", None),
            DockMiddleClickAction::Minimize => ("Minimize windows", Some(WindowAction::Minimize)),
            DockMiddleClickAction::Close => ("Close windows", Some(WindowAction::Close)),
        };
        let keys = action.map_or_else(
            || gesture.targets.launch.as_slice(),
            |_| gesture.targets.members.as_slice(),
        );
        let total = action.map_or(1, |_| keys.len());
        let mut accepted = 0;
        let mut failed = 0;
        let mut detail = (action.is_none() && keys.is_empty())
            .then(|| "No trusted catalog application for this tile.".to_owned());
        for key in keys {
            if !self.dock_middle_current(&dock, &gesture) {
                break;
            }
            let result = if let Some(action) = action {
                if dock.displayed_window_key(key).as_deref() != Some(key.as_str())
                    || !self.dock_middle_current(&dock, &gesture)
                {
                    break;
                }
                self.core.host().window_action(key, action)
            } else {
                self.core.host().launch(key)
            };
            match result {
                Ok(()) => accepted += 1,
                Err(error) => {
                    failed += 1;
                    if detail.is_none() {
                        detail = Some(sanitize::bounded_text(&error, 200));
                    }
                }
            }
        }
        let remaining = total - accepted - failed;
        let mut receipt = format!(
            "{label}: {accepted} requests accepted, {failed} failed, {remaining} not submitted."
        );
        if total == 0 {
            receipt.push_str(" No current admitted windows.");
        }
        if remaining > 0 && detail.is_none() {
            detail = Some("Dock scope changed; remaining requests were not submitted.".to_owned());
        }
        if let Some(detail) = detail {
            receipt.push(' ');
            receipt.push_str(&detail);
        }
        if self.root_current() {
            self.report_message(&receipt);
        }
    }
}
