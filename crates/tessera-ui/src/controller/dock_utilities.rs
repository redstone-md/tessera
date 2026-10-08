// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::rc::Rc;
use std::sync::Arc;

use slint::ComponentHandle;

use crate::dock_utilities::DockUtilitiesController;
use crate::generated::DockReservedAction;

pub(super) type DockUtilities = crate::transient_window::TransientCache<DockUtilitiesController>;

impl super::PanelController {
    pub(super) fn request_dock_utility(&self, action: DockReservedAction) {
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        if !dock.window().is_visible() {
            return;
        }
        let cached = self.dock_utilities.borrow().clone();
        let utility = cached.unwrap_or_else(|| {
            let utility = DockUtilitiesController::new(Arc::clone(self.core.host()), &dock);
            *self.dock_utilities.borrow_mut() = Some(Rc::clone(&utility));
            utility
        });
        utility.request(action);
    }

    pub(super) fn dock_utility_event_ready(&self) {
        let cached = self.dock_utilities.borrow().clone();
        if let Some(result) = cached.and_then(|utility| utility.process_events()) {
            self.report(result, "Desktop toggle requested", |error| {
                format!("Desktop toggle failed: {error}")
            });
        }
    }
}
