// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One passive battery domain follows the admitted toolbar, not popup selection.

use slint::ComponentHandle;

use super::{PanelController, Rc};
use crate::battery::BatteryController;
use crate::transient_window::TransientCache;

pub(super) type BatteryPopups = TransientCache<BatteryController>;

impl PanelController {
    pub(super) fn sync_battery_root(&self) {
        let Some(toolbar) = self.toolbar_and_upgrade() else {
            self.stop_battery_root();
            return;
        };
        let admitted = self.passive_toolbar_admission(&toolbar);
        if !admitted() {
            self.stop_battery_root();
            return;
        }
        let existing = self.battery.borrow().clone();
        let actor = match existing {
            Some(actor) => actor,
            None => {
                let actor = match BatteryController::new(self.core.host().clone()) {
                    Ok(actor) => actor,
                    Err(error) => {
                        if admitted() {
                            toolbar.set_battery_activation_key("".into());
                            toolbar.set_battery_percent_known(false);
                            toolbar.set_battery_text("Unavailable".into());
                            toolbar.set_battery_label("Battery presentation unavailable".into());
                            toolbar.set_battery_visible(true);
                            self.report_message(&format!(
                                "Could not create battery popup: {error}"
                            ));
                        }
                        return;
                    }
                };
                let current = Rc::clone(&admitted);
                actor.set_root_admission(move || current());
                let current = Rc::clone(&admitted);
                let source = toolbar.as_weak();
                actor.bind_toolbar(move |view| {
                    let Some(toolbar) = source.upgrade() else {
                        return;
                    };
                    // Set authority last; a source retired during projection stays inert.
                    macro_rules! project {
                        ($setter:ident, $value:expr) => {
                            if !current() {
                                return;
                            }
                            toolbar.$setter($value);
                        };
                    }
                    project!(set_battery_activation_key, "".into());
                    project!(set_battery_percent_known, false);
                    project!(set_battery_text, view.text);
                    project!(set_battery_label, view.accessible_label);
                    project!(set_battery_percent, i32::from(view.percent.unwrap_or(0)));
                    project!(set_battery_percent_known, view.percent.is_some());
                    project!(set_battery_visible, view.visible);
                    project!(set_battery_activation_key, view.activation_key);
                });
                if !admitted() {
                    actor.stop_root();
                    return;
                }
                let published = {
                    let mut cache = self.battery.borrow_mut();
                    if cache.is_none() {
                        *cache = Some(Rc::clone(&actor));
                        true
                    } else {
                        false
                    }
                };
                if !published {
                    actor.stop_root();
                    return;
                }
                actor
            }
        };
        if admitted() {
            actor.start_root();
        } else {
            actor.stop_root();
        }
    }

    pub(super) fn stop_battery_root(&self) {
        let actor = self.battery.borrow().clone();
        if let Some(actor) = actor {
            actor.stop_root();
            actor.hide();
        }
    }
}
