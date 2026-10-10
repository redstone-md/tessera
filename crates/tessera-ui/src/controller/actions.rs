// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit dock window intents; neither rendering nor native window policy.

use super::{PanelController, model_key};
use crate::WindowAction;
use crate::generated::Dock;
use slint::Model;

impl PanelController {
    /// Routes individual observed members only while their tile/group is displayed.
    /// Group metadata admits presentation, never substitutes for the native key.
    pub(crate) fn window_action(&self, key: &str, action: WindowAction) {
        if self.guarded() {
            return;
        }
        let resolved = self
            .dock_and_upgrade()
            .and_then(|dock| dock.displayed_window_key(key));
        let Some(key) = resolved else {
            self.report_message("That window is no longer displayed in the dock.");
            return;
        };
        let description = match action {
            WindowAction::ActivateOrMinimize => "Window switch/minimize requested",
            WindowAction::Activate => "Window activation requested",
            WindowAction::Minimize => "Window minimize requested",
            WindowAction::Close => "Window close requested",
        };
        let result = self.core.host().window_action(&key, action);
        self.report(result, description, |detail| {
            format!("Window command failed: {detail}")
        });
    }
}

impl Dock {
    /// One source rule for direct actions, grouped members and live previews.
    pub(crate) fn displayed_window_key(&self, key: &str) -> Option<String> {
        let observed = model_key(&self.get_observed_windows(), key, |window| {
            window.key.to_string()
        })?;
        let running = self.get_running_windows();
        if model_key(&running, key, |window| window.key.to_string()).is_some() {
            return Some(observed);
        }
        let groups = self.get_group_metadata();
        let member = groups.iter().find(|group| group.key == key)?;
        let visible = running
            .iter()
            .any(|window| window.key == member.representative)
            || self.get_pinned_apps().iter().any(|app| {
                groups.iter().any(|group| {
                    group.key == app.key
                        && group.representative == member.representative
                        && !group.identity.is_empty()
                        && group.identity == member.identity
                })
            });
        visible.then_some(observed)
    }
}
