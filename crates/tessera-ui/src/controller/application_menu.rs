// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Root-owned source lifetime, separate from native command targets and receipts.

use slint::ComponentHandle;

use super::{Cell, PanelController, Rc, RefCell, SurfaceKind};
use crate::generated::Dock;

#[derive(Default)]
pub(super) struct ApplicationMenuSource {
    epoch: RefCell<Rc<()>>,
    projection_depth: Cell<usize>,
    exhausted: Cell<bool>,
}

impl ApplicationMenuSource {
    pub(super) fn retire(&self) {
        self.epoch.replace(Rc::new(()));
    }

    pub(super) fn begin_projection(self: &Rc<Self>) -> ApplicationProjection {
        self.retire();
        if let Some(depth) = self.projection_depth.get().checked_add(1) {
            self.projection_depth.set(depth);
        } else {
            self.exhausted.set(true);
        }
        ApplicationProjection(Rc::clone(self))
    }

    fn current(&self, epoch: &Rc<()>) -> bool {
        !self.exhausted.get()
            && self.projection_depth.get() == 0
            && Rc::ptr_eq(epoch, &self.epoch.borrow())
    }
}

pub(super) struct ApplicationProjection(Rc<ApplicationMenuSource>);

impl Drop for ApplicationProjection {
    fn drop(&mut self) {
        self.0.retire();
        self.0
            .projection_depth
            .set(self.0.projection_depth.get().saturating_sub(1));
    }
}

impl PanelController {
    /// Readonly admission: never observe, refit, project or retire popup input.
    /// Popup input is fenced separately; this source stays checkable after local hide.
    pub(super) fn dock_application_menu_admission(&self, dock: &Dock) -> Rc<dyn Fn() -> bool> {
        let position = dock.window().position();
        let size = dock.window().size();
        let scale = dock.window().scale_factor();
        let dock = dock.as_weak();
        let root = Rc::downgrade(&self.admission);
        let power = Rc::downgrade(&self.power_admission_closed);
        let saving = Rc::downgrade(&self.preference_saving);
        let geometry = Rc::downgrade(&self.visibility_geometry);
        let leases = Rc::downgrade(&self.leases);
        let core = std::sync::Arc::downgrade(&self.core);
        let source = Rc::downgrade(&self.application_menu_source);
        let epoch = self.application_menu_source.epoch.borrow().clone();
        Rc::new(move || {
            let Some(dock) = dock.upgrade() else {
                return false;
            };
            let Some(core) = core.upgrade() else {
                return false;
            };
            let Some(leases) = leases.upgrade() else {
                return false;
            };
            source
                .upgrade()
                .is_some_and(|source| source.current(&epoch))
                && root.upgrade().is_some_and(|root| root.alive.get())
                && power.upgrade().is_some_and(|closed| !closed.get())
                && saving.upgrade().is_some_and(|saving| !saving.get())
                && geometry
                    .upgrade()
                    .is_some_and(|geometry| geometry.borrow().desired_visible(SurfaceKind::Dock))
                && core
                    .dock_context()
                    .is_some_and(|context| !context.fullscreen_active())
                && scale.is_finite()
                && scale > 0.0
                && size.width > 0
                && size.height > 0
                && dock.window().is_visible()
                && dock.window().position() == position
                && dock.window().size() == size
                && dock.window().scale_factor() == scale
                && leases
                    .borrow()
                    .attachments
                    .get(&SurfaceKind::Dock)
                    .is_some_and(|lease| {
                        lease.rect == (position.x, position.y, size.width, size.height)
                    })
        })
    }
}
