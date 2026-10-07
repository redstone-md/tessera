// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Lazy popup ownership and conversion of real dock-relative input anchors.

use super::{PanelController, Rc, model_key};
use crate::context_menu::ContextMenuController;
use crate::generated::DockMenuKind;
use crate::popup_placement::physical_anchor;
use crate::transient_window::TransientCache;
use slint::ComponentHandle;

pub(super) type Menus = TransientCache<ContextMenuController>;

impl PanelController {
    pub(super) fn open_dock_menu(&self, kind: DockMenuKind, key: &str, point: (f32, f32)) {
        self.dismiss_tooltip(false);
        if kind != DockMenuKind::Bar && self.guarded() {
            return;
        }
        let Some(dock) = self.dock_and_upgrade() else {
            return;
        };
        let key = match kind {
            DockMenuKind::Bar => Some(String::new()),
            DockMenuKind::Pinned => {
                model_key(&dock.get_pinned_apps(), key, |app| app.key.to_string())
            }
            DockMenuKind::Window => model_key(&dock.get_running_windows(), key, |window| {
                window.key.to_string()
            }),
        };
        let Some(key) = key else {
            self.report_message("That item is no longer displayed in the dock.");
            return;
        };
        let Some(context) = self.core.dock_context() else {
            self.open_panel();
            self.report_message("Popup monitor geometry is unavailable. Settings and recovery remain available here.");
            return;
        };
        let anchor = match physical_anchor(
            dock.window().position(),
            dock.window().scale_factor(),
            point,
        ) {
            Ok(anchor) => anchor,
            Err(message) => {
                self.report_message(message);
                return;
            }
        };
        let existing = self.menus.borrow().clone();
        let menu = match existing {
            Some(menu) => menu,
            None => match ContextMenuController::new(self.core.host().clone(), &dock) {
                Ok(menu) => {
                    *self.menus.borrow_mut() = Some(Rc::clone(&menu));
                    menu
                }
                Err(error) => {
                    self.report_message(&format!("Could not create context menu: {error}"));
                    return;
                }
            },
        };
        if let Err(error) = menu.show(kind, key.into(), anchor, context) {
            self.report_message(&format!("Context menu: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::physical_anchor;
    use slint::PhysicalPosition;

    #[test]
    fn input_anchor_uses_real_origin_and_scale_and_rejects_invalid_geometry() {
        let origin = PhysicalPosition::new(-1920, -1080);
        assert_eq!(
            physical_anchor(origin, 1.5, (8.0, 40.0)).unwrap(),
            PhysicalPosition::new(-1908, -1020)
        );
        assert_eq!(
            physical_anchor(origin, 2.0, (7.25, 20.25)).unwrap(),
            PhysicalPosition::new(-1906, -1040)
        );
        for (scale, point) in [
            (0.0, (1.0, 1.0)),
            (-1.0, (1.0, 1.0)),
            (f32::NAN, (1.0, 1.0)),
            (1.0, (f32::INFINITY, 0.0)),
            (1.0, (0.0, f32::NAN)),
            (f32::MAX, (2.0, 2.0)),
        ] {
            assert!(physical_anchor(origin, scale, point).is_err());
        }
        assert!(physical_anchor(PhysicalPosition::new(i32::MAX, 0), 1.0, (1.0, 0.0)).is_err());
    }
}
