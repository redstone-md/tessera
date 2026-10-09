// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Root authorization and lifecycle for independent, lazily opened toolbar modules.

use slint::ComponentHandle;

use super::popups::PopupKind;
use super::{PanelController, Rc};
use crate::bluetooth::BluetoothController;
use crate::generated::TileBounds;
use crate::input_language::InputLanguageController;
use crate::network_menu::NetworkController;
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::transient_window::{TransientCache, TransientScope};

pub(super) type NetworkPopups = TransientCache<NetworkController>;
pub(super) type BluetoothPopups = TransientCache<BluetoothController>;
pub(super) type InputLanguagePopups = TransientCache<InputLanguageController>;

pub(super) fn valid_bounds(bounds: &TileBounds) -> bool {
    bounds.origin.x.is_finite()
        && bounds.origin.y.is_finite()
        && bounds.width.is_finite()
        && bounds.height.is_finite()
        && bounds.width > 0.0
        && bounds.height > 0.0
}

macro_rules! toolbar_popup {
    ($method:ident, $cache:ident, $actor:ident, $kind:ident) => {
        pub(super) fn $method(&self, bounds: TileBounds) {
            let Some(toolbar) = self.toolbar_and_upgrade() else {
                return;
            };
            let Some(context) = self.core.dock_context() else {
                return;
            };
            let operation = self.popup_operation.borrow().clone();
            let position = toolbar.window().position();
            let size = toolbar.window().size();
            let scale = toolbar.window().scale_factor();
            let current = || {
                !self.power_admission_closed.get()
                    && self.bar_input_ready(crate::SurfaceKind::Toolbar)
                    && Rc::ptr_eq(&operation, &self.popup_operation.borrow())
                    && toolbar.window().is_visible()
                    && toolbar.window().position() == position
                    && toolbar.window().size() == size
                    && toolbar.window().scale_factor() == scale
                    && self.core.dock_context() == Some(context)
                    && !context.fullscreen_active()
                    && size.width > 0
                    && size.height > 0
                    && valid_bounds(&bounds)
            };
            if !current() {
                return;
            }
            self.dismiss_tooltip(false);
            if !current() {
                return;
            }
            let existing = self.$cache.borrow().clone();
            let actor = match existing {
                Some(actor) => actor,
                None => {
                    let actor = match $actor::new(self.core.host().clone()) {
                        Ok(actor) => actor,
                        Err(error) => {
                            if current() {
                                self.report_message(&format!(
                                    "Could not create toolbar popup: {error}"
                                ));
                            }
                            return;
                        }
                    };
                    if !current() {
                        actor.hide();
                        return;
                    }
                    let published = {
                        let mut cache = self.$cache.borrow_mut();
                        if cache.is_none() {
                            *cache = Some(Rc::clone(&actor));
                            true
                        } else {
                            false
                        }
                    };
                    if !published {
                        actor.hide();
                        return;
                    }
                    actor
                }
            };
            actor.apply_theme(toolbar.presentation_theme());
            if !current() {
                return;
            }
            let result = actor.show(toolbar.window(), bounds.clone(), context);
            let cached = self.$cache.borrow().clone();
            if current()
                && cached
                    .as_ref()
                    .is_some_and(|cached| Rc::ptr_eq(cached, &actor))
            {
                self.popup_presentation_finished(PopupKind::$kind, actor.is_open(), result);
            }
        }
    };
}

impl PanelController {
    toolbar_popup!(open_network_menu, network_menu, NetworkController, Network);
    toolbar_popup!(open_bluetooth, bluetooth, BluetoothController, Bluetooth);
    toolbar_popup!(
        open_input_language,
        input_language,
        InputLanguageController,
        InputLanguage
    );

    pub(super) fn toolbar_popup_theme(&self, theme: PresentationTheme) {
        let network = self.network_menu.borrow().clone();
        let bluetooth = self.bluetooth.borrow().clone();
        let input = self.input_language.borrow().clone();
        if let Some(actor) = network {
            actor.apply_theme(theme);
        }
        if let Some(actor) = bluetooth {
            actor.apply_theme(theme);
        }
        if let Some(actor) = input {
            actor.apply_theme(theme);
        }
    }

    pub(super) fn toolbar_popup_motion_disabled(&self) {
        let network = self.network_menu.borrow().clone();
        let bluetooth = self.bluetooth.borrow().clone();
        let input = self.input_language.borrow().clone();
        if let Some(actor) = network {
            actor.disable_motion();
        }
        if let Some(actor) = bluetooth {
            actor.disable_motion();
        }
        if let Some(actor) = input {
            actor.disable_motion();
        }
    }

    pub(super) fn toolbar_popup_geometry(&self, context: crate::DockContext, scale: f32) {
        let network = self.network_menu.borrow().clone();
        let bluetooth = self.bluetooth.borrow().clone();
        let input = self.input_language.borrow().clone();
        if let Some(actor) = network {
            actor.close_if_geometry_changed(context, scale);
        }
        if let Some(actor) = bluetooth {
            actor.close_if_geometry_changed(context, scale);
        }
        if let Some(actor) = input {
            actor.close_if_geometry_changed(context, scale);
        }
    }
}

pub(super) struct ToolbarPopupScope {
    _network: TransientScope<NetworkController>,
    _bluetooth: TransientScope<BluetoothController>,
    _input: TransientScope<InputLanguageController>,
}

impl ToolbarPopupScope {
    pub(super) fn new(controller: &PanelController) -> Self {
        Self {
            _network: TransientScope::new(
                Rc::clone(&controller.network_menu),
                NetworkController::hide,
            ),
            _bluetooth: TransientScope::new(
                Rc::clone(&controller.bluetooth),
                BluetoothController::hide,
            ),
            _input: TransientScope::new(
                Rc::clone(&controller.input_language),
                InputLanguageController::close,
            ),
        }
    }
}
