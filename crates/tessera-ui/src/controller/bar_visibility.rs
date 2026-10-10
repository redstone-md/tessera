// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Bar visibility controls stay drafts until the complete preferences are saved.

use tessera_system::visibility::AutoHideMode;

use super::PanelController;
use crate::{BarVisibilityPreferences, Panel};

impl PanelController {
    pub(super) fn project_bar_visibility_preferences(&self, panel: &Panel) {
        let preferences = self.core.applied_preferences().bar_visibility();
        panel.set_dock_auto_hide_index(mode_index(preferences.dock()));
        if !self.root_current() {
            return;
        }
        panel.set_toolbar_auto_hide_index(mode_index(preferences.toolbar()));
    }

    pub(super) fn draft_bar_visibility_preferences(
        &self,
        panel: &Panel,
    ) -> Option<BarVisibilityPreferences> {
        Some(BarVisibilityPreferences::new(
            mode_from_index(panel.get_dock_auto_hide_index())?,
            mode_from_index(panel.get_toolbar_auto_hide_index())?,
        ))
    }
}

fn mode_index(mode: AutoHideMode) -> i32 {
    match mode {
        AutoHideMode::Never => 0,
        AutoHideMode::Always => 1,
        AutoHideMode::OnOverlap => 2,
    }
}

fn mode_from_index(index: i32) -> Option<AutoHideMode> {
    match index {
        0 => Some(AutoHideMode::Never),
        1 => Some(AutoHideMode::Always),
        2 => Some(AutoHideMode::OnOverlap),
        _ => None,
    }
}
