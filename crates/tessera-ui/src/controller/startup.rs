// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native startup registration is OS state, independent of preference drafts.

use slint::ComponentHandle;

use super::{PanelController, Rc};
use crate::generated::Panel;
use crate::startup::StartupController;
use crate::transient_window::TransientCache;

pub(super) type StartupRoots = TransientCache<StartupController>;

impl PanelController {
    pub(super) fn wire_startup(&self, panel: &Panel) {
        let source = panel.as_weak();
        let root = Rc::downgrade(&self.admission);
        let power = Rc::downgrade(&self.power_admission_closed);
        let saving = Rc::downgrade(&self.preference_saving);
        let admitted = Rc::new(move || {
            let Some(panel) = source.upgrade() else {
                return false;
            };
            let size = panel.window().size();
            let scale = panel.window().scale_factor();
            root.upgrade().is_some_and(|root| root.alive.get())
                && power.upgrade().is_some_and(|closed| !closed.get())
                && saving.upgrade().is_some_and(|saving| !saving.get())
                && panel.window().is_visible()
                && panel.get_startup_general_visible()
                && size.width > 0
                && size.height > 0
                && scale.is_finite()
                && scale > 0.0
        });
        let actor = StartupController::new(self.core.host().clone(), panel.as_weak(), admitted);
        let published = {
            let mut slot = self.startup.borrow_mut();
            if slot.is_none() {
                *slot = Some(Rc::clone(&actor));
                true
            } else {
                false
            }
        };
        if !published {
            actor.stop_root();
            return;
        }
        let controller = self.clone();
        panel.on_startup_view_changed(move || controller.sync_startup_root());
        self.sync_startup_root();
    }

    pub(super) fn sync_startup_root(&self) {
        let actor = self.startup.borrow().clone();
        if let Some(actor) = actor {
            actor.refresh_root();
        }
    }

    pub(super) fn stop_startup_root(&self) {
        let actor = self.startup.borrow().clone();
        if let Some(actor) = actor {
            actor.stop_root();
        }
    }
}
