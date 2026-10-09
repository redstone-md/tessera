// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::cell::Cell;

use super::PanelController;
use crate::DockContext;

#[derive(Default)]
pub(super) struct GeometryUpdates {
    running: Cell<bool>,
    pending: Cell<Option<Option<DockContext>>>,
}

struct GeometryScope<'a>(&'a GeometryUpdates);
impl Drop for GeometryScope<'_> {
    fn drop(&mut self) {
        self.0.running.set(false);
        self.0.pending.set(None);
    }
}

impl PanelController {
    /// Showing a Slint window flushes pending property-change callbacks.
    /// A nested request records the latest context instead of borrowing native
    /// leases again. The outer pass applies it before reporting readiness.
    pub(super) fn apply_geometry(&self, mut context: Option<DockContext>) -> Result<bool, String> {
        if self.geometry.running.replace(true) {
            self.geometry.pending.set(Some(context));
            return Ok(false);
        }
        let _scope = GeometryScope(&self.geometry);
        loop {
            self.geometry.pending.set(None);
            let placed = match context {
                Some(context) => self.place_geometry(context)?,
                None => {
                    self.retire_visibility_geometry()?;
                    false
                }
            };
            match self.geometry.pending.take() {
                Some(next) => context = next,
                None => return Ok(placed),
            }
        }
    }
}
