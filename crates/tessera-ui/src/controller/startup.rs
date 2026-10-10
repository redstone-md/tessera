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
        let admitted = self.settings_source_admission(panel, Panel::get_startup_general_visible);
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
