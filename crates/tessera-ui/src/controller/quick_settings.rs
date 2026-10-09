// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Toolbar-origin placement and lazy ownership of audio plus shared current-player projection.

use super::popups::PopupKind;
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
        let Some(toolbar) = self.toolbar_and_upgrade() else {
            return;
        };
        if !super::native_toolbar::valid_bounds(&bounds) {
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
        let size = toolbar.window().size();
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
        let operation = Rc::new(());
        self.popup_operation.replace(Rc::clone(&operation));
        let source_current = || {
            self.root_current()
                && self.bar_input_ready(crate::SurfaceKind::Toolbar)
                && toolbar.window().is_visible()
                && toolbar.window().position() == origin
                && toolbar.window().size() == size
                && toolbar.window().scale_factor() == scale
                && self.core.dock_context() == Some(context)
                && !context.fullscreen_active()
        };
        let current = || source_current() && Rc::ptr_eq(&operation, &self.popup_operation.borrow());
        if !current() {
            return;
        }
        self.dismiss_tooltip(false);
        if !current() {
            return;
        }
        let existing = self.quick_settings.borrow().clone();
        let quick = match existing {
            Some(quick) => quick,
            None => {
                let Some(media) = self.dock_media.borrow().clone() else {
                    return;
                };
                let quick =
                    match QuickSettingsController::new(self.core.host().clone(), &panel, media) {
                        Ok(quick) => quick,
                        Err(error) => {
                            if current() {
                                self.report_message(&format!(
                                    "Could not create quick settings: {error}"
                                ));
                            }
                            return;
                        }
                    };
                if !current() {
                    quick.hide();
                    return;
                }
                let published = {
                    let mut cache = self.quick_settings.borrow_mut();
                    if cache.is_none() {
                        *cache = Some(Rc::clone(&quick));
                        true
                    } else {
                        false
                    }
                };
                if !published {
                    quick.hide();
                    return;
                }
                quick
            }
        };
        let result = quick.show_scoped(
            toolbar.presentation_theme(),
            anchor,
            context,
            scale,
            current,
        );
        if !current()
            || !self
                .quick_settings
                .borrow()
                .as_ref()
                .is_some_and(|cached| Rc::ptr_eq(cached, &quick))
        {
            return;
        }
        if quick.is_open() && self.dismiss_popups_except(Some(PopupKind::QuickSettings)) {
            let origin_scope = self.popup_operation.borrow().clone();
            let core = std::sync::Arc::downgrade(&self.core);
            let admission = Rc::downgrade(&self.admission);
            let power_closed = Rc::downgrade(&self.power_admission_closed);
            let operation_slot = Rc::downgrade(&self.popup_operation);
            let quick_cache = Rc::downgrade(&self.quick_settings);
            let visibility = Rc::downgrade(&self.visibility_geometry);
            let panel = self.panel.clone();
            let expected = Rc::downgrade(&quick);
            let source_owner = expected.clone();
            let toolbar = toolbar.as_weak();
            let source_admission: Rc<dyn Fn() -> bool> = Rc::new(move || {
                let (
                    Some(core),
                    Some(admission),
                    Some(power_closed),
                    Some(operation_slot),
                    Some(quick_cache),
                    Some(visibility),
                ) = (
                    core.upgrade(),
                    admission.upgrade(),
                    power_closed.upgrade(),
                    operation_slot.upgrade(),
                    quick_cache.upgrade(),
                    visibility.upgrade(),
                )
                else {
                    return false;
                };
                let Some(quick) = source_owner.upgrade() else {
                    return false;
                };
                let toolbar_ready = toolbar.upgrade().is_some_and(|toolbar| {
                    toolbar.window().is_visible()
                        && toolbar.window().position() == origin
                        && toolbar.window().size() == size
                        && toolbar.window().scale_factor() == scale
                });
                admission.alive.get()
                    && !power_closed.get()
                    && panel.upgrade().is_some()
                    && quick.is_open()
                    && quick.component().window().is_visible()
                    && visibility
                        .borrow()
                        .desired_visible(crate::SurfaceKind::Toolbar)
                    && toolbar_ready
                    && core.dock_context() == Some(context)
                    && !context.fullscreen_active()
                    && admission.active_popup.get() == Some(PopupKind::QuickSettings)
                    && Rc::ptr_eq(&origin_scope, &operation_slot.borrow())
                    && quick_cache
                        .borrow()
                        .as_ref()
                        .is_some_and(|cached| Rc::ptr_eq(cached, &quick))
            });
            let input_source = Rc::clone(&source_admission);
            let admitted = Rc::new(move || {
                let quick = expected.upgrade()?;
                if quick.media_input_ready() && input_source() {
                    Some(quick)
                } else {
                    None
                }
            });
            let action_admission = Rc::clone(&admitted);
            quick
                .component()
                .on_media_action_requested(move |action, identity| {
                    if let Some(quick) = action_admission() {
                        quick.request_media(action, identity.as_str());
                    }
                });
            let seek_admission = Rc::clone(&admitted);
            quick.component().on_media_seek_requested(move |fraction| {
                if let Some(quick) = seek_admission() {
                    quick.request_seek(fraction);
                }
            });
            quick.component().on_media_refresh_requested(move || {
                if let Some(quick) = admitted() {
                    quick.retry_media();
                }
            });
            if source_current()
                && self.admission.active_popup.get() == Some(PopupKind::QuickSettings)
            {
                quick.attach_media_scoped(Rc::clone(&source_admission));
            }
        }
        if let Err(error) = result
            && source_current()
        {
            self.report_message(&format!("Quick settings: {error}"));
        }
    }
}
