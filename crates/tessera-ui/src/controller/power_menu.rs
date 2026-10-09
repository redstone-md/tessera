// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Launcher entry and weak root feedback for the independent Power surface.

use slint::ComponentHandle;
use std::cell::Cell;

use super::popups::PopupKind;
use super::{Panel, PanelController, Rc};
use crate::power_menu::PowerMenuController;
use crate::transient_window::{TransientCache, TransientScope};

pub(super) type PowerPopups = TransientCache<PowerMenuController>;

/// Close root admission before the actor's scope drops callback-capable leases.
pub(super) struct PowerAdmissionScope {
    closed: Rc<Cell<bool>>,
    _actor: TransientScope<PowerMenuController>,
}

impl PowerAdmissionScope {
    pub(super) fn new(controller: &PanelController) -> Self {
        Self {
            closed: Rc::clone(&controller.power_admission_closed),
            _actor: TransientScope::new(
                Rc::clone(&controller.power_menu),
                PowerMenuController::close,
            ),
        }
    }
}

impl Drop for PowerAdmissionScope {
    fn drop(&mut self) {
        self.closed.set(true);
    }
}

impl PanelController {
    pub(super) fn wire_power(&self, panel: &Panel) {
        let controller = self.clone();
        panel.on_power_popup_opened(move || {
            let power = controller.power_menu.borrow().clone();
            if let Some(power) = power {
                if controller.power_admission_closed.get() {
                    power.close();
                    return;
                }
                controller.popup_presentation_finished(
                    PopupKind::Power,
                    power.is_visible(),
                    Ok(()),
                );
            }
        });
        let controller = self.clone();
        panel.on_power_action_result(move |accepted, error| {
            if controller.power_admission_closed.get() {
                return;
            }
            if accepted {
                controller.report_message("Lock request accepted; session state is not observed.");
            } else {
                controller.report_message(&format!(
                    "Power: {}",
                    crate::sanitize::bounded_text(&error, 200)
                ));
            }
        });
    }

    pub(super) fn open_power_menu(&self) {
        let Some(launcher) = self.launcher_and_upgrade() else {
            return;
        };
        let operation = self.popup_operation.borrow().clone();
        let current = || {
            !self.power_admission_closed.get()
                && Rc::ptr_eq(&operation, &self.popup_operation.borrow())
                && self.launcher_popup_ready()
                && launcher.window().is_visible()
                && !launcher.get_reorder_dragging()
        };
        if !current() {
            return;
        }
        self.dismiss_tooltip(false);
        if !current() {
            return;
        }
        let existing = self.power_menu.borrow().clone();
        let power = match existing {
            Some(power) => power,
            None => {
                // Weak component relays avoid cache -> actor -> root -> cache.
                let panel = self.panel.clone();
                let on_opened = Rc::new(move || {
                    if let Some(panel) = panel.upgrade() {
                        panel.invoke_power_popup_opened();
                    }
                });
                let panel = self.panel.clone();
                let on_result = Rc::new(move |result: Result<(), String>| {
                    if let Some(panel) = panel.upgrade() {
                        match result {
                            Ok(()) => panel.invoke_power_action_result(true, "".into()),
                            Err(error) => panel.invoke_power_action_result(false, error.into()),
                        }
                    }
                });
                match PowerMenuController::new(self.core.host().clone(), on_opened, on_result) {
                    Ok(power) => {
                        if !current() {
                            power.close();
                            return;
                        }
                        let published = {
                            let mut cache = self.power_menu.borrow_mut();
                            if cache.is_none() {
                                *cache = Some(Rc::clone(&power));
                                true
                            } else {
                                false
                            }
                        };
                        if !published {
                            power.close();
                            return;
                        }
                        power
                    }
                    Err(error) => {
                        if current() {
                            self.report_message(&format!(
                                "Could not create the power menu: {error}"
                            ));
                        }
                        return;
                    }
                }
            }
        };
        let theme = self
            .panel
            .upgrade()
            .map(|panel| crate::theme_from_index(panel.get_theme_index()))
            .unwrap_or_default();
        if !current() {
            return;
        }
        // Scheduled read is not visible: coordinate only after real show.
        if let Err(error) = power.show(theme) {
            self.report_message(&format!("Power menu: {error}"));
        }
    }

    pub(super) fn hide_power_menu(&self) {
        let power = self.power_menu.borrow().clone();
        if let Some(power) = power {
            power.hide();
        }
    }
}
