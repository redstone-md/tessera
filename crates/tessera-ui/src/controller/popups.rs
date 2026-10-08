// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One active interactive shell popup, without coupling its provider state.

use super::PanelController;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PopupKind {
    DockMenu,
    QuickSettings,
    User,
    Calendar,
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
            };
            self.report_message(&format!("{label}: {error}"));
        }
    }

    /// Clone all caches before callbacks: hiding can synchronously reenter UI.
    pub(super) fn dismiss_popups_except(&self, keep: Option<PopupKind>) {
        let menu = self.menus.borrow().clone();
        let quick = self.quick_settings.borrow().clone();
        let user = self.user_menu.borrow().clone();
        let calendar = self.calendar.borrow().clone();
        if keep != Some(PopupKind::DockMenu)
            && let Some(menu) = menu
        {
            menu.hide();
        }
        if keep != Some(PopupKind::QuickSettings)
            && let Some(quick) = quick
        {
            quick.hide();
        }
        if keep != Some(PopupKind::User)
            && let Some(user) = user
        {
            user.hide();
        }
        if keep != Some(PopupKind::Calendar)
            && let Some(calendar) = calendar
        {
            calendar.hide();
        }
    }
}
