// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Toolbar-origin placement and lazy ownership of independent audio controls.

use super::{PanelController, Rc};
use crate::generated::TileBounds;
use crate::popup_placement::physical_anchor;
use crate::quick_settings::QuickSettingsController;
use crate::theme::ThemedComponent;
use crate::transient_window::TransientCache;
use slint::ComponentHandle;

pub(super) type QuickPopups = TransientCache<QuickSettingsController>;

impl PanelController {
    pub(super) fn open_quick_settings(&self, bounds: TileBounds) {
        self.dismiss_tooltip(false);
        let Some(toolbar) = self.toolbar_and_upgrade() else {
            return;
        };
        if !toolbar.window().is_visible()
            || !bounds.width.is_finite()
            || !bounds.height.is_finite()
            || bounds.width <= 0.0
            || bounds.height <= 0.0
        {
            return;
        }
        let Some(context) = self.core.dock_context() else {
            self.report_message("Quick settings needs the toolbar's real monitor geometry.");
            return;
        };
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let origin = toolbar.window().position();
        let scale = toolbar.window().scale_factor();
        let mut anchor = match physical_anchor(
            origin,
            scale,
            (bounds.origin.x + bounds.width / 2.0, bounds.origin.y),
        ) {
            Ok(anchor) => anchor,
            Err(error) => {
                self.report_message(error);
                return;
            }
        };
        // The reference starts at the real toolbar bottom, not at the tile's
        // lower edge. Stay in physical pixels here; do not round-trip DPI.
        anchor.y =
            match i32::try_from(i64::from(origin.y) + i64::from(toolbar.window().size().height)) {
                Ok(bottom) => bottom,
                Err(_) => {
                    self.report_message("The toolbar's physical bottom is invalid.");
                    return;
                }
            };
        let menu = self.menus.borrow().clone();
        if let Some(menu) = menu {
            menu.hide();
        }
        let existing = self.quick_settings.borrow().clone();
        let quick = match existing {
            Some(quick) => quick,
            None => match QuickSettingsController::new(self.core.host().clone(), &panel) {
                Ok(quick) => {
                    *self.quick_settings.borrow_mut() = Some(Rc::clone(&quick));
                    quick
                }
                Err(error) => {
                    self.report_message(&format!("Could not create quick settings: {error}"));
                    return;
                }
            },
        };
        if let Err(error) = quick.show(toolbar.presentation_theme(), anchor, context, scale) {
            self.report_message(&format!("Quick settings: {error}"));
        }
    }
}
