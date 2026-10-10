// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Launcher entry and weak root feedback for the independent Power surface.

use slint::ComponentHandle;
use std::cell::Cell;

use super::popups::PopupKind;
use super::{Panel, PanelController, Rc};
use crate::power_menu::PowerMenuController;
use crate::theme::ThemedComponent;
use crate::transient_window::{TransientCache, TransientScope};

pub(super) type PowerPopups = TransientCache<PowerMenuController>;

/// Close root admission before the actor's scope drops callback-capable leases.
pub(super) struct PowerAdmissionScope {
    closed: Rc<Cell<bool>>,
    launcher: Option<slint::Weak<crate::generated::Launcher>>,
    _watch: TransientScope<super::power_display::PowerDisplayWatch>,
    _actor: TransientScope<PowerMenuController>,
}

impl PowerAdmissionScope {
    pub(super) fn new(controller: &PanelController) -> Self {
        Self {
            closed: Rc::clone(&controller.power_admission_closed),
            launcher: controller.launcher.clone(),
            _watch: TransientScope::new(
                Rc::clone(&controller.power_display),
                super::power_display::PowerDisplayWatch::close,
            ),
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
        if let Some(launcher) = self.launcher.as_ref().and_then(slint::Weak::upgrade) {
            launcher.invoke_cancel_input();
        }
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
                let operation = controller.popup_operation.borrow().clone();
                if power.is_visible() {
                    controller.start_power_display_watch(&power, false);
                }
                let cached = controller.power_menu.borrow().clone();
                if controller.power_admission_closed.get()
                    || !Rc::ptr_eq(&operation, &controller.popup_operation.borrow())
                    || cached
                        .as_ref()
                        .is_none_or(|cached| !Rc::ptr_eq(cached, &power))
                {
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
                controller.report_message(
                    "Power request accepted; OS state and update completion are not observed.",
                );
            } else {
                controller.report_message(&format!(
                    "Power: {}",
                    crate::sanitize::bounded_text(&error, 200)
                ));
            }
        });
        let controller = self.clone();
        panel.on_power_display_event_ready(move || controller.power_display_event_ready());
        let controller = self.clone();
        panel.on_power_command_event_ready(move || {
            if !controller.root_current() {
                return;
            }
            let power = controller.power_menu.borrow().clone();
            if let Some(power) = power {
                power.process_events();
            }
        });
        let controller = self.clone();
        panel.on_power_command_busy_changed(move |busy| {
            if !controller.root_current() {
                return;
            }
            let user = controller.user_menu.borrow().clone();
            if let Some(user) = user {
                user.set_logout_busy(busy);
            }
        });
    }

    /// Cache/command creation only. Native display/update reads and Power UI
    /// creation remain exclusively behind explicit Power show().
    pub(super) fn ensure_power_controller(
        &self,
        current: impl Fn() -> bool,
    ) -> Option<Rc<PowerMenuController>> {
        if !self.root_current() || !current() {
            return None;
        }
        if let Some(power) = self.power_menu.borrow().clone() {
            return Some(power);
        }
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
        let power =
            PowerMenuController::command_actor(self.core.host().clone(), on_opened, on_result);
        let panel = self.panel.clone();
        power.bind_command_relay(
            self.panel.clone(),
            Rc::new(move |busy| {
                if let Some(panel) = panel.upgrade() {
                    panel.invoke_power_command_busy_changed(busy);
                }
            }),
        );
        if !self.root_current() || !current() {
            power.close();
            return None;
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
        if published {
            Some(power)
        } else {
            power.close();
            None
        }
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
        // A genuine pending Power intent retires an older command transaction
        // without pretending the new surface is visible or dismissing User yet.
        let operation = Rc::new(());
        self.popup_operation.replace(Rc::clone(&operation));
        let current = || {
            self.root_current()
                && Rc::ptr_eq(&operation, &self.popup_operation.borrow())
                && self.launcher_popup_ready()
                && launcher.window().is_visible()
                && !launcher.get_reorder_dragging()
        };
        let Some(power) = self.ensure_power_controller(current) else {
            return;
        };
        let theme = launcher.presentation_theme();
        if !current() {
            return;
        }
        // Reuse the retained genuine account observation, not another desktop read.
        power.set_user_name(&launcher.get_user_name());
        if !current() {
            return;
        }
        // Scheduled read is not visible: coordinate only after real show.
        match power.show(theme) {
            Ok(true) if current() => self.start_power_display_watch(&power, true),
            Ok(_) => {}
            Err(error) if current() => self.report_message(&format!("Power menu: {error}")),
            Err(_) => {}
        }
    }

    pub(super) fn hide_power_menu(&self) {
        let power = self.power_menu.borrow().clone();
        if let Some(power) = power {
            power.hide();
        }
    }
}
