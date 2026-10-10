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
    /// Only a unique catalog-backed pin admits application capability inspection.
    /// Running groups, captions and process metadata are never launch authority.
    pub(crate) fn catalog_pinned_key(&self, key: &str) -> Option<String> {
        let pins = self.get_pinned_apps();
        let mut matches = pins.iter().filter(|pin| pin.key == key);
        let pin = matches.next()?;
        (pin.pinned && matches.next().is_none()).then(|| pin.key.to_string())
    }

    /// Complete admitted membership, never the visible representative slice.
    pub(crate) fn displayed_group_window_keys(
        &self,
        key: &str,
        pinned: bool,
    ) -> Option<Vec<String>> {
        let displayed = if pinned {
            model_key(&self.get_pinned_apps(), key, |app| app.key.to_string())
        } else {
            model_key(&self.get_running_windows(), key, |window| {
                window.key.to_string()
            })
        }?;
        let metadata = self.get_group_metadata();
        let mut anchors = metadata.iter().filter(|group| group.key == displayed);
        let Some(anchor) = anchors.next() else {
            return pinned.then(Vec::new);
        };
        if anchors.next().is_some() || (pinned && anchor.identity.is_empty()) {
            return None;
        }
        let mut keys = Vec::new();
        for window in self.get_observed_windows().iter() {
            let mut entries = metadata.iter().filter(|member| member.key == window.key);
            let Some(member) = entries.next() else {
                continue;
            };
            if entries.next().is_some() {
                return None;
            }
            let belongs = member.representative == anchor.representative
                && (member.key == anchor.key
                    || (!anchor.identity.is_empty() && member.identity == anchor.identity));
            if !belongs {
                continue;
            }
            let exact = self.displayed_window_key(&window.key)?;
            if keys.contains(&exact) {
                return None;
            }
            keys.push(exact);
        }
        (usize::try_from(anchor.count).ok() == Some(keys.len())).then_some(keys)
    }

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
