// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit dock window intents; neither rendering nor native window policy.

use super::{PanelController, model_key};
use crate::WindowAction;

impl PanelController {
    /// Routes only keys still present in the displayed dock model. Native
    /// identity and current foreground are revalidated by the platform adapter.
    pub(crate) fn window_action(&self, key: &str, action: WindowAction) {
        if self.guarded() {
            return;
        }
        let resolved = self.dock_and_upgrade().and_then(|dock| {
            model_key(&dock.get_running_windows(), key, |window| {
                window.key.to_string()
            })
        });
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
