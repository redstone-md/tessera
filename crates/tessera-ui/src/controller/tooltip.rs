// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::{PanelController, Rc};
use crate::generated::TileBounds;
use crate::tooltip::{Side, TooltipController};
use crate::transient_window::TransientCache;
use slint::ComponentHandle;

pub(super) type Tooltips = TransientCache<TooltipController>;

impl PanelController {
    pub(super) fn show_tooltip<C: ComponentHandle + 'static>(
        &self,
        owner: &C,
        source: crate::SurfaceKind,
        content: &str,
        bounds: TileBounds,
        side: Side,
    ) {
        if self
            .menus
            .borrow()
            .as_ref()
            .is_some_and(|menu| menu.is_open())
        {
            self.dismiss_tooltip(false);
            return;
        }
        let Some(context) = self.core.dock_context() else {
            self.dismiss_tooltip(false);
            return;
        };
        if context.fullscreen_active() {
            self.dismiss_tooltip(false);
            return;
        }
        let existing = self.tooltips.borrow().clone();
        let tooltip = match existing {
            Some(tooltip) => tooltip,
            None => {
                let panel = self.panel.clone();
                let dock = self.dock.clone();
                let on_error = Rc::new(move |error: String| {
                    let error: slint::SharedString =
                        crate::sanitize::bounded_text(&error, 400).into();
                    if let Some(panel) = panel.upgrade() {
                        panel.set_status(error.clone());
                    }
                    if let Some(dock) = dock.as_ref().and_then(slint::Weak::upgrade) {
                        let mut status = dock.get_surface_status();
                        status.status = error;
                        dock.set_surface_status(status);
                    }
                });
                match TooltipController::new(self.core.host().clone(), on_error) {
                    Ok(tooltip) => {
                        *self.tooltips.borrow_mut() = Some(Rc::clone(&tooltip));
                        tooltip
                    }
                    Err(error) => {
                        self.report_message(&format!("Could not create tooltip: {error}"));
                        return;
                    }
                }
            }
        };
        if let Err(error) = tooltip.schedule(owner, source, content, bounds, context, side) {
            self.report_message(&format!("Tooltip: {error}"));
        }
    }

    pub(super) fn dismiss_tooltip(&self, delayed: bool) {
        let tooltip = self.tooltips.borrow().clone();
        if let Some(tooltip) = tooltip {
            tooltip.dismiss(delayed);
        }
    }

    pub(super) fn dismiss_hover_tooltip(
        &self,
        source: crate::SurfaceKind,
        origin: slint::LogicalPosition,
        delayed: bool,
    ) {
        let tooltip = self.tooltips.borrow().clone();
        if let Some(tooltip) = tooltip {
            tooltip.dismiss_for(source, origin, delayed);
        }
    }
}
