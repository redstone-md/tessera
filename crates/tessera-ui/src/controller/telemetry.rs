// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Optional passive statistics adopt only the complete saved preference record.

use slint::ComponentHandle;

use super::{PanelController, Rc};
use crate::telemetry::{TelemetryController, TelemetryFrame};
use crate::transient_window::TransientCache;

pub(super) type TelemetryRoots = TransientCache<TelemetryController>;

impl PanelController {
    pub(super) fn sync_telemetry_root(&self) {
        let enabled = self.core.applied_preferences().telemetry_enabled();
        let existing = self.telemetry.borrow().clone();
        if !enabled {
            if let Some(actor) = existing {
                actor.set_enabled(false);
            }
            return;
        }
        let Some(toolbar) = self.toolbar_and_upgrade() else {
            self.stop_telemetry_root();
            return;
        };
        let admitted = self.passive_toolbar_admission(&toolbar);
        if !admitted() {
            self.stop_telemetry_root();
            return;
        }
        let actor = match existing {
            Some(actor) => actor,
            None => {
                let source = toolbar.as_weak();
                let current = Rc::clone(&admitted);
                let frame = Rc::new(move || {
                    if !current() {
                        return None;
                    }
                    let toolbar = source.upgrade()?;
                    let scale = toolbar.window().scale_factor();
                    if !scale.is_finite() || scale <= 0.0 {
                        return None;
                    }
                    Some(TelemetryFrame {
                        position: toolbar.window().position(),
                        size: toolbar.window().size(),
                        scale,
                    })
                });
                let source = toolbar.as_weak();
                let current = Rc::clone(&admitted);
                let project = Rc::new(move |view: crate::telemetry::TelemetryProjection| {
                    let Some(toolbar) = source.upgrade() else {
                        return;
                    };
                    if !view.visible || !current() {
                        toolbar.set_telemetry_visible(false);
                        return;
                    }
                    toolbar.set_telemetry_text(view.text);
                    if !current() {
                        toolbar.set_telemetry_visible(false);
                        return;
                    }
                    toolbar.set_telemetry_label(view.accessible_label);
                    toolbar.set_telemetry_visible(current());
                });
                let actor = TelemetryController::new(
                    self.core.host().clone(),
                    toolbar.as_weak(),
                    frame,
                    project,
                );
                let published = {
                    let mut cache = self.telemetry.borrow_mut();
                    if cache.is_none() {
                        *cache = Some(Rc::clone(&actor));
                        true
                    } else {
                        false
                    }
                };
                if !published {
                    actor.stop_root();
                    return;
                }
                actor
            }
        };
        actor.set_enabled(self.core.applied_preferences().telemetry_enabled());
    }

    pub(super) fn stop_telemetry_root(&self) {
        let actor = self.telemetry.borrow().clone();
        if let Some(actor) = actor {
            actor.stop_root();
        }
    }
}
