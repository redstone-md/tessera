// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Readonly admission for passive data domains, independent of popup ownership.

use slint::ComponentHandle;

use super::{PanelController, Rc, SurfaceKind};
use crate::generated::Toolbar;

impl PanelController {
    pub(super) fn passive_toolbar_admission(&self, toolbar: &Toolbar) -> Rc<dyn Fn() -> bool> {
        let source = toolbar.as_weak();
        let root = Rc::downgrade(&self.admission);
        let power = Rc::downgrade(&self.power_admission_closed);
        let geometry = Rc::downgrade(&self.visibility_geometry);
        let leases = Rc::downgrade(&self.leases);
        let core = std::sync::Arc::downgrade(&self.core);
        Rc::new(move || {
            let Some(toolbar) = source.upgrade() else {
                return false;
            };
            let Some(core) = core.upgrade() else {
                return false;
            };
            let Some(leases) = leases.upgrade() else {
                return false;
            };
            let position = toolbar.window().position();
            let size = toolbar.window().size();
            root.upgrade().is_some_and(|root| root.alive.get())
                && power.upgrade().is_some_and(|closed| !closed.get())
                && geometry
                    .upgrade()
                    .is_some_and(|geometry| geometry.borrow().desired_visible(SurfaceKind::Toolbar))
                && core
                    .dock_context()
                    .is_some_and(|context| !context.fullscreen_active())
                && toolbar.window().is_visible()
                && size.width > 0
                && size.height > 0
                && leases
                    .borrow()
                    .attachments
                    .get(&SurfaceKind::Toolbar)
                    .is_some_and(|lease| {
                        lease.rect == (position.x, position.y, size.width, size.height)
                    })
        })
    }
}
