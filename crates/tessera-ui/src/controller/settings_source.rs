// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Readonly admission shared by independent OS-state Settings actors.

use slint::ComponentHandle;

use super::{PanelController, Rc};
use crate::generated::Panel;

impl PanelController {
    pub(super) fn settings_source_admission(
        &self,
        panel: &Panel,
        page_visible: fn(&Panel) -> bool,
    ) -> Rc<dyn Fn() -> bool> {
        let source = panel.as_weak();
        let root = Rc::downgrade(&self.admission);
        let power = Rc::downgrade(&self.power_admission_closed);
        let saving = Rc::downgrade(&self.preference_saving);
        Rc::new(move || {
            let Some(panel) = source.upgrade() else {
                return false;
            };
            let size = panel.window().size();
            let scale = panel.window().scale_factor();
            root.upgrade().is_some_and(|root| root.alive.get())
                && power.upgrade().is_some_and(|closed| !closed.get())
                && saving.upgrade().is_some_and(|saving| !saving.get())
                && panel.window().is_visible()
                && page_visible(&panel)
                && size.width > 0
                && size.height > 0
                && scale.is_finite()
                && scale > 0.0
        })
    }
}
