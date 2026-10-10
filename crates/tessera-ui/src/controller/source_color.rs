// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Custom RGB input joins the existing appearance preview, never persistence.

use slint::ComponentHandle;

use super::PanelController;
use crate::{Panel, SourceSeed};

impl PanelController {
    pub(super) fn wire_custom_source_color(&self, panel: &Panel) {
        let controller = self.clone();
        panel.on_source_custom_requested(move |value| {
            let Some(panel) = controller.panel.upgrade() else {
                return false;
            };
            if !controller.root_current()
                || controller.power_admission_closed.get()
                || controller.preference_saving.get()
                || !panel.window().is_visible()
                || !panel.get_source_custom_visible()
                || !panel.get_source_custom_input_active()
            {
                return false;
            }
            let Some(seed) = source_seed(&value) else {
                return false;
            };
            panel.set_source_rgb(seed.rgb() as i32);
            true
        });
    }
}

fn source_seed(value: &str) -> Option<SourceSeed> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16)
        .ok()
        .and_then(SourceSeed::from_rgb)
}
