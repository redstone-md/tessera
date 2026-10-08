// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::rc::Rc;
use std::sync::Arc;

use slint::ComponentHandle;

use crate::generated::DockRecycleAction;
use crate::recycle_bin::RecycleBinController;

pub(super) type RecycleBins = crate::transient_window::TransientCache<RecycleBinController>;

impl super::PanelController {
    pub(super) fn recycle_bin_shown(&self) {
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        if !dock.window().is_visible() {
            return;
        }
        let cached = self.recycle_bin.borrow().clone();
        let recycle = cached.unwrap_or_else(|| {
            let recycle = RecycleBinController::new(Arc::clone(self.core.host()), &dock);
            *self.recycle_bin.borrow_mut() = Some(Rc::clone(&recycle));
            recycle
        });
        recycle.shown();
    }

    pub(super) fn recycle_bin_hidden(&self) {
        let cached = self.recycle_bin.borrow().clone();
        if let Some(recycle) = cached {
            recycle.hidden();
        }
    }

    pub(super) fn request_recycle_bin(&self, action: DockRecycleAction) {
        // Only real geometry/show creates the presenter. Provisional startup
        // input cannot acquire a provider or replay intent after placement.
        let cached = self.recycle_bin.borrow().clone();
        if let Some(recycle) = cached {
            recycle.request(action);
        }
    }

    pub(super) fn recycle_bin_event_ready(&self) {
        let cached = self.recycle_bin.borrow().clone();
        if let Some(result) = cached.and_then(|recycle| recycle.process_events()) {
            self.report(result, "Recycle Bin open requested", |error| {
                format!("Recycle Bin open failed: {error}")
            });
        }
    }
}
