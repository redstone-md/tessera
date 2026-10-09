// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One active interactive shell popup, without coupling its provider state.

use super::PanelController;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PopupKind {
    DockMenu,
    QuickSettings,
    User,
    Calendar,
    Power,
}

impl PanelController {
    pub(super) fn popup_presentation_finished(
        &self,
        kind: PopupKind,
        is_open: bool,
        result: Result<(), String>,
    ) {
        // Foreground denial can leave an honest visible popup. Conversely,
        // invalid or cancelled presentation must retain the previous popup.
        if is_open {
            self.dismiss_popups_except(Some(kind));
        }
        if let Err(error) = result {
            let label = match kind {
                PopupKind::DockMenu => "Context menu",
                PopupKind::QuickSettings => "Quick settings",
                PopupKind::User => "User menu",
                PopupKind::Calendar => "Calendar",
                PopupKind::Power => "Power menu",
            };
            self.report_message(&format!("{label}: {error}"));
        }
    }

    /// Clone all caches before callbacks: hiding can synchronously reenter UI.
    pub(super) fn dismiss_popups_except(&self, keep: Option<PopupKind>) {
        // Holding the old identity prevents allocator reuse: a reentrant newer
        // presentation wins without wrapping a counter or keeping a cache borrow.
        let operation = Rc::new(());
        self.popup_operation.replace(Rc::clone(&operation));
        let current = || Rc::ptr_eq(&operation, &self.popup_operation.borrow());
        let menu = self.menus.borrow().clone();
        let quick = self.quick_settings.borrow().clone();
        let user = self.user_menu.borrow().clone();
        let calendar = self.calendar.borrow().clone();
        let power = self.power_menu.borrow().clone();
        if keep != Some(PopupKind::DockMenu)
            && current()
            && let Some(menu) = menu
        {
            menu.hide();
        }
        if keep != Some(PopupKind::QuickSettings)
            && current()
            && let Some(quick) = quick
        {
            quick.hide();
        }
        if keep != Some(PopupKind::User)
            && current()
            && let Some(user) = user
        {
            user.hide();
        }
        if keep != Some(PopupKind::Calendar)
            && current()
            && let Some(calendar) = calendar
        {
            calendar.hide();
        }
        if keep != Some(PopupKind::Power)
            && current()
            && let Some(power) = power
        {
            power.hide();
        }
    }
}
