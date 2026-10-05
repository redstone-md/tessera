// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::generated::Row;
use crate::projection::{PanelProjection, RowProjection};
use crate::{DesktopHost, Panel, PanelPreferences, PanelSnapshot, sanitize};

type Host = dyn DesktopHost;

/// All request state belongs to the UI thread, including queued completions.
/// There is deliberately no separate atomic busy flag: `refreshing` on the
/// panel is the single source of truth, so UI state and callback guarding
/// cannot drift apart.
///
/// The last successful full snapshot is kept here (not derived from displayed
/// rows) so filtering works without re-observing the desktop.
#[derive(Clone)]
pub(crate) struct PanelController {
    panel: slint::Weak<Panel>,
    host: Arc<Host>,
    last_snapshot: Arc<Mutex<Option<PanelSnapshot>>>,
}

impl PanelController {
    pub(crate) fn new(panel: &Panel, host: Arc<Host>) -> Self {
        let controller = Self {
            panel: panel.as_weak(),
            host,
            last_snapshot: Arc::new(Mutex::new(None)),
        };
        let weak = controller.clone();
        panel.on_refresh_requested(move || {
            let _ = weak.refresh();
        });
        let weak = controller.clone();
        panel.on_activate_requested(move |key| weak.activate(&key));
        let weak = controller.clone();
        panel.on_filter_requested(move || weak.apply_filter());
        let weak = controller.clone();
        panel.on_save_preferences_requested(move || {
            weak.save_preferences();
        });
        controller
    }

    /// Applies the current search filter to the last successful snapshot.
    /// Filtering never re-observes the desktop and never renumbers keys.
    pub(crate) fn apply_filter(&self) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let Some(projection) = self
            .last_snapshot
            .lock()
            .as_ref()
            .map(|snapshot| crate::projection::project(snapshot, &panel.get_search()))
        else {
            return;
        };
        show(&panel, &projection);
        if !panel.get_stale() && !panel.get_refreshing() {
            panel.set_status(projection.status.as_str().into());
        }
    }

    /// Starts one single-flight observation on a worker thread. Returns
    /// `None` (without queuing) when already refreshing, the panel is gone,
    /// or the worker could not be spawned.
    pub(crate) fn refresh(&self) -> Option<std::thread::JoinHandle<()>> {
        let panel = self.panel.upgrade()?;
        if panel.get_refreshing() {
            return None;
        }
        panel.set_refreshing(true);
        // While refreshing, activation is disabled even on retained data.
        panel.set_stale(true);
        panel.set_status(if panel.get_has_snapshot() {
            "Refreshing; activation is paused...".into()
        } else {
            "Reading the desktop...".into()
        });

        let weak = self.panel.clone();
        let controller = self.clone();
        let host = Arc::clone(&self.host);
        let spawned = std::thread::Builder::new()
            .name("tessera-observation".into())
            .spawn(move || {
                // A failed worker must not leave the UI permanently busy.
                // Keep private panic payloads out of the panel's error text.
                let result = catch_unwind(AssertUnwindSafe(|| host.observe()))
                    .unwrap_or_else(|_| Err("Observation failed unexpectedly".into()));
                // Closing the window/event loop legitimately drops this result.
                let _ = weak
                    .upgrade_in_event_loop(move |panel| apply_result(&controller, &panel, result));
            });
        match spawned {
            Ok(worker) => Some(worker),
            Err(error) => {
                apply_result(
                    self,
                    &panel,
                    Err(format!("Could not start observation: {error}")),
                );
                None
            }
        }
    }

    /// Activates a window through the host. Guarded twice: the panel refresh
    /// state and this callback re-check, so a queued or synthetic activation
    /// can never act on a stale snapshot or during an observation.
    pub(crate) fn activate(&self, key: &str) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        if panel.get_refreshing() || panel.get_stale() || !panel.get_has_snapshot() {
            return;
        }
        let Some(key) = activation_key(&panel, key) else {
            panel.set_status("That window is no longer in the current observation.".into());
            return;
        };
        match self.host.activate(&key) {
            Ok(()) => panel.set_status("Window activated".into()),
            Err(error) => {
                let detail = sanitize::bounded_text(&error, 200);
                panel.set_status(format!("Activation failed: {detail}").into());
            }
        }
    }

    /// Saves the currently previewed preferences through the host.
    pub(crate) fn save_preferences(&self) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let preferences = PanelPreferences::new(
            crate::theme_from_index(panel.get_theme_index()),
            panel.get_compact(),
        );
        match self.host.save_preferences(&preferences) {
            Ok(()) => panel.set_status("Preferences saved".into()),
            Err(error) => {
                let detail = sanitize::bounded_text(&error, 200);
                panel.set_status(format!("Could not save preferences: {detail}").into());
            }
        }
    }
}

/// Resolves a clicked row key against the rows actually on screen. A stale,
/// filtered-out, or fabricated key (or a display index passed as a key) is
/// rejected here, so activation always maps to a visible row.
fn activation_key(panel: &Panel, key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    let model = panel.get_rows();
    (0..model.row_count()).find_map(|index| {
        let row: Row = model.row_data(index)?;
        (row.key.as_str() == key).then(|| row.key.to_string())
    })
}

fn show(panel: &Panel, projection: &PanelProjection) {
    panel.set_rows(ModelRc::new(VecModel::from(
        projection.rows.iter().map(row_model).collect::<Vec<_>>(),
    )));
    panel.set_summary(projection.summary.as_str().into());
}

fn row_model(row: &RowProjection) -> Row {
    Row {
        key: row.key.as_str().into(),
        caption: row.caption.as_str().into(),
        minimized: row.minimized,
    }
}

fn apply_result(
    controller: &PanelController,
    panel: &Panel,
    result: Result<PanelSnapshot, String>,
) {
    match result {
        Ok(snapshot) => {
            let projection = crate::projection::project(&snapshot, &panel.get_search());
            show(panel, &projection);
            panel.set_status(projection.status.as_str().into());
            panel.set_has_snapshot(true);
            panel.set_stale(false);
            // Retain the full snapshot for filtering without new observation.
            *controller.last_snapshot.lock() = Some(snapshot);
        }
        Err(error) => {
            let prefix = if panel.get_has_snapshot() {
                "Refresh failed; retained data is stale"
            } else {
                "Could not observe the desktop"
            };
            let detail = sanitize::bounded_text(&error, 200);
            panel.set_status(format!("{prefix}: {detail}. Try Refresh.").into());
            // A failed refresh means the desktop may have changed; activation
            // stays paused until the next successful observation confirms it.
            panel.set_stale(true);
        }
    }
    // Release single-flight only after applying the result on the UI thread.
    panel.set_refreshing(false);
}

#[cfg(test)]
mod tests;
