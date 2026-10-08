// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Launcher-origin ownership of the independent current-user folder popup.

use super::{PanelController, Rc};
use crate::generated::TileBounds;
use crate::theme::ThemedComponent;
use crate::transient_window::TransientCache;
use crate::user_menu::UserMenuController;
use slint::ComponentHandle;

pub(super) type UserPopups = TransientCache<UserMenuController>;

impl PanelController {
    pub(super) fn open_user_menu(&self, bounds: TileBounds) {
        self.dismiss_tooltip(false);
        let Some(launcher) = self.launcher_and_upgrade() else {
            return;
        };
        if !self.launcher_popup_ready()
            || !launcher.window().is_visible()
            || launcher.get_reorder_dragging()
            || !bounds.origin.x.is_finite()
            || !bounds.origin.y.is_finite()
            || !bounds.width.is_finite()
            || !bounds.height.is_finite()
            || bounds.width <= 0.0
            || bounds.height <= 0.0
        {
            return;
        }
        let Some(context) = self.core.dock_context() else {
            self.report_message("The user menu needs the launcher's real monitor geometry.");
            return;
        };
        let menu = self.menus.borrow().clone();
        if let Some(menu) = menu {
            menu.hide();
        }
        let quick = self.quick_settings.borrow().clone();
        if let Some(quick) = quick {
            quick.hide();
        }
        let existing = self.user_menu.borrow().clone();
        let user = match existing {
            Some(user) => user,
            None => match UserMenuController::new(self.core.host().clone()) {
                Ok(user) => {
                    *self.user_menu.borrow_mut() = Some(Rc::clone(&user));
                    user
                }
                Err(error) => {
                    self.report_message(&format!("Could not create the user menu: {error}"));
                    return;
                }
            },
        };
        user.apply_theme(launcher.presentation_theme());
        if let Err(error) = user.show(
            launcher.window(),
            bounds,
            context,
            &launcher.get_user_name(),
        ) {
            self.report_message(&format!("User menu: {error}"));
        }
    }

    pub(super) fn hide_user_menu(&self) {
        let user = self.user_menu.borrow().clone();
        if let Some(user) = user {
            user.hide();
        }
    }
}
