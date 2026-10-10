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
    Battery,
    Network,
    Bluetooth,
    InputLanguage,
    LauncherAppMenu,
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
        if !self.root_current() {
            return;
        }
        if is_open && !self.dismiss_popups_except(Some(kind)) {
            return;
        }
        if let Err(error) = result {
            let label = match kind {
                PopupKind::DockMenu => "Context menu",
                PopupKind::QuickSettings => "Quick settings",
                PopupKind::User => "User menu",
                PopupKind::Calendar => "Calendar",
                PopupKind::Power => "Power menu",
                PopupKind::Battery => "Battery",
                PopupKind::Network => "Network",
                PopupKind::Bluetooth => "Bluetooth",
                PopupKind::InputLanguage => "Keyboard selector",
                PopupKind::LauncherAppMenu => "Application menu",
            };
            self.report_message(&format!("{label}: {error}"));
        }
    }

    /// Clone all caches before callbacks: hiding can synchronously reenter UI.
    pub(super) fn dismiss_popups_except(&self, keep: Option<PopupKind>) -> bool {
        if !self.root_current() {
            return false;
        }
        // Holding the old identity prevents allocator reuse: a reentrant newer
        // presentation wins without wrapping a counter or keeping a cache borrow.
        let operation = Rc::new(());
        self.admission.active_popup.set(keep);
        self.popup_operation.replace(Rc::clone(&operation));
        let current =
            || self.root_current() && Rc::ptr_eq(&operation, &self.popup_operation.borrow());
        let menu = self.menus.borrow().clone();
        let quick = self.quick_settings.borrow().clone();
        let user = self.user_menu.borrow().clone();
        let calendar = self.calendar.borrow().clone();
        let power = self.power_menu.borrow().clone();
        let battery = self.battery.borrow().clone();
        let network = self.network_menu.borrow().clone();
        let bluetooth = self.bluetooth.borrow().clone();
        let input = self.input_language.borrow().clone();
        let launcher_app = self.launcher_app_menu.borrow().clone();
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
        if keep != Some(PopupKind::Battery)
            && current()
            && let Some(actor) = battery
        {
            actor.hide();
        }
        if keep != Some(PopupKind::Network)
            && current()
            && let Some(actor) = network
        {
            actor.hide();
        }
        if keep != Some(PopupKind::Bluetooth)
            && current()
            && let Some(actor) = bluetooth
        {
            actor.hide();
        }
        if keep != Some(PopupKind::InputLanguage)
            && current()
            && let Some(actor) = input
        {
            actor.hide();
        }
        if keep != Some(PopupKind::LauncherAppMenu)
            && current()
            && let Some(actor) = launcher_app
        {
            actor.hide();
        }
        current()
    }
}
