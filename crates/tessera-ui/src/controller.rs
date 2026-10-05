// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{Panel, PanelSnapshot, sanitize};

type Source = dyn Fn() -> Result<PanelSnapshot, String> + Send + Sync;
const MAX_ROWS: usize = 128;

/// All request state belongs to the UI thread, including queued completions.
#[derive(Clone)]
pub(crate) struct PanelController {
    panel: slint::Weak<Panel>,
    source: Arc<Source>,
}

impl PanelController {
    pub(crate) fn new(
        panel: &Panel,
        source: impl Fn() -> Result<PanelSnapshot, String> + Send + Sync + 'static,
    ) -> Self {
        let controller = Self {
            panel: panel.as_weak(),
            source: Arc::new(source),
        };
        let callback = controller.clone();
        panel.on_refresh_requested(move || {
            let _ = callback.refresh();
        });
        controller
    }

    pub(crate) fn refresh(&self) -> Option<std::thread::JoinHandle<()>> {
        let panel = self.panel.upgrade()?;
        if panel.get_refreshing() {
            return None;
        }
        panel.set_refreshing(true);
        panel.set_status(if panel.get_has_snapshot() {
            "Refreshing; showing the last observation...".into()
        } else {
            "Reading the desktop...".into()
        });

        let weak = self.panel.clone();
        let source = Arc::clone(&self.source);
        let spawned = std::thread::Builder::new()
            .name("tessera-observation".into())
            .spawn(move || {
                // A failed worker must not leave the UI permanently busy.
                // Keep private panic payloads out of the panel's error text.
                let result = catch_unwind(AssertUnwindSafe(|| source()))
                    .unwrap_or_else(|_| Err("Observation failed unexpectedly".into()));
                // Closing the window/event loop legitimately drops this result.
                let _ = weak.upgrade_in_event_loop(move |panel| apply_result(&panel, result));
            });
        match spawned {
            Ok(worker) => Some(worker),
            Err(error) => {
                apply_result(&panel, Err(format!("Could not start observation: {error}")));
                None
            }
        }
    }
}

fn apply_result(panel: &Panel, result: Result<PanelSnapshot, String>) {
    match result {
        Ok(snapshot) => {
            let total = snapshot.window_titles.len();
            let rows: Vec<SharedString> = snapshot
                .window_titles
                .iter()
                .take(MAX_ROWS)
                .map(|title| sanitize::caption(title).into())
                .collect();
            let shown = rows.len();
            panel.set_rows(ModelRc::new(VecModel::from(rows)));
            panel.set_summary(
                format!(
                    "Monitors: {} | Windows: {total} | Warnings: {}",
                    snapshot.monitor_count, snapshot.warning_count
                )
                .into(),
            );
            panel.set_status(if total == 0 {
                "No visible windows observed".into()
            } else {
                format!("Showing {shown} of {total} windows; refresh to update").into()
            });
            panel.set_has_snapshot(true);
        }
        Err(error) => {
            let prefix = if panel.get_has_snapshot() {
                "Refresh failed; retained data is stale"
            } else {
                "Could not observe the desktop"
            };
            let detail = sanitize::bounded_text(&error, 200);
            panel.set_status(format!("{prefix}: {detail}. Try Refresh.").into());
        }
    }
    // Release single-flight only after applying the result on the UI thread.
    panel.set_refreshing(false);
}

#[cfg(test)]
mod tests;
