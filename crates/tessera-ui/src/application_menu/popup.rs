// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Application capabilities reuse the owned popup, rows, placement and input routes.

use std::cell::Cell;
use std::rc::Rc;

use slint::{ComponentHandle, Model};

use super::ContextMenuController;
use crate::application_menu::{
    ApplicationReply, ApplicationScope, FlightKind, WindowFrame, action_row, application_action,
    error_notice, outcome_notice,
};
use crate::generated::{DockApplicationNotice, DockMenuAction, DockMenuKind, TileBounds};
use crate::{DockContext, popup_placement as placement};

struct Projection<'a> {
    slot: &'a Cell<Option<u64>>,
    generation: u64,
}

impl Drop for Projection<'_> {
    fn drop(&mut self) {
        if self.slot.get() == Some(self.generation) {
            self.slot.set(None);
        }
    }
}

impl ContextMenuController {
    pub(crate) fn set_application_admission(&self, admission: Rc<dyn Fn() -> bool>) {
        let old = self.application_admission.replace(Some(admission));
        drop(old);
    }

    pub(super) fn invalidate_application_source(&self) {
        self.application_source_generation.set(
            self.application_source_generation
                .get()
                .and_then(|source| source.checked_add(1)),
        );
        let source = self.application_source_generation.get();
        if self.application.scope().is_some() {
            self.hide();
        }
        if self.application_source_generation.get() == source
            && let Some(dock) = self.dock.upgrade()
        {
            dock.set_application_menu_notice(DockApplicationNotice::default());
        }
    }

    fn application_source_current(&self, scope: &ApplicationScope) -> bool {
        if self.application_source_generation.get() != Some(scope.source) {
            return false;
        }
        let admitted = self.dock.upgrade().is_some_and(|dock| {
            let status = dock.get_surface_status();
            WindowFrame::capture(dock.window()) == Some(scope.dock_frame)
                && !status.refreshing
                && !status.stale
                && dock.catalog_pinned_key(&scope.key).as_deref() == Some(scope.key.as_str())
        });
        admitted && self.application_source_generation.get() == Some(scope.source)
    }

    fn application_current(&self, scope: &ApplicationScope) -> bool {
        let local = || {
            let key = self.key.borrow().clone();
            self.scope_generation.get() == Some(scope.generation)
                && self.is_open()
                && self.surface.window().is_visible()
                && self.surface.get_kind() == DockMenuKind::Pinned
                && !self.surface.get_launcher_favorite_scope()
                && !self.surface.get_media_scope()
                && key.as_str() == scope.key
                && WindowFrame::capture(self.surface.window()) == Some(scope.popup_frame)
                && self.application_source_current(scope)
        };
        // The Root callback may synchronously retire or replace this popup.
        local() && (scope.admission)() && local()
    }

    pub(super) fn inspect_application(
        &self,
        anchor: slint::PhysicalPosition,
        context: DockContext,
    ) {
        if self.surface.get_kind() != DockMenuKind::Pinned
            || self.surface.get_launcher_favorite_scope()
            || self.surface.get_media_scope()
        {
            return;
        }
        let generation = self.scope_generation.get();
        let key = self.key.borrow().to_string();
        if self
            .dock
            .upgrade()
            .is_none_or(|dock| dock.catalog_pinned_key(&key).is_none())
        {
            return;
        }
        // The optional getter is called only for an explicitly opened catalog pin.
        // No-provider hosts retain exactly their old rows and measured geometry.
        let Some(provider) = self.host.application_menu_host() else {
            return;
        };
        if self.scope_generation.get() != generation || !self.is_open() {
            return;
        }
        let admission = self.application_admission.borrow().clone();
        let (Some(generation), Some(source), Some(admission), Some(dock)) = (
            generation,
            self.application_source_generation.get(),
            admission,
            self.dock.upgrade(),
        ) else {
            return;
        };
        let (Some(dock_frame), Some(popup_frame)) = (
            WindowFrame::capture(dock.window()),
            WindowFrame::capture(self.surface.window()),
        ) else {
            return;
        };
        let scope = ApplicationScope {
            generation,
            source,
            key,
            dock_frame,
            popup_frame,
            anchor,
            context,
            admission,
        };
        if !self.application_current(&scope) {
            return;
        }
        self.application
            .prepare(scope.clone(), std::sync::Arc::clone(&provider));
        let Some(scope) =
            self.project_application(&scope, Vec::new(), "Checking application actions…")
        else {
            if self.scope_generation.get() == Some(scope.generation) && self.is_open() {
                self.hide();
            }
            return;
        };
        if !self.application_current(&scope) {
            return;
        }
        if let Err(error) = self.application.inspect(scope.clone(), provider) {
            self.project_application(&scope, Vec::new(), error_notice(error));
        }
    }

    /// Refit the attached popup without replaying native show, focus or motion.
    fn project_application(
        &self,
        scope: &ApplicationScope,
        actions: Vec<tessera_system::application_menu::ApplicationAction>,
        notice: &str,
    ) -> Option<ApplicationScope> {
        if !self.application_current(scope) {
            return None;
        }
        self.application_projection.set(Some(scope.generation));
        let _projection = Projection {
            slot: &self.application_projection,
            generation: scope.generation,
        };
        let entries: Vec<_> = actions
            .into_iter()
            .map(|action| action_row(action, scope.generation))
            .collect();
        self.surface
            .set_application_entries(slint::ModelRc::new(slint::VecModel::from(entries)));
        if !self.application_current(scope) {
            return None;
        }
        self.surface.set_application_notice(notice.into());
        if !self.application_current(scope) {
            return None;
        }
        let dock = self.dock.upgrade()?;
        let rect = placement::place(
            scope.context,
            scope.anchor,
            (
                self.surface.get_menu_width(),
                self.surface.get_menu_height(),
            ),
            dock.window().scale_factor(),
        )
        .ok()?;
        if !self.application_current(scope) || !self.surface.reposition(rect.position, rect.size) {
            return None;
        }
        let mut next = scope.clone();
        next.popup_frame = WindowFrame::capture(self.surface.window())?;
        if !self.application_current(&next) {
            return None;
        }
        self.application.reframe(scope, next.popup_frame);
        Some(next)
    }

    pub(super) fn application_event_ready(&self) {
        let Some((scope, kind, reply)) = self.application.receive() else {
            return;
        };
        match (kind, reply) {
            (FlightKind::Inspect, ApplicationReply::Inspect(result)) => {
                if !self.application_current(&scope) {
                    return;
                }
                let result =
                    result.and_then(|snapshot| self.application.publish_targets(&scope, snapshot));
                match result {
                    Ok(actions) => {
                        let notice = if actions.is_empty() {
                            "No application actions are available."
                        } else {
                            ""
                        };
                        self.project_application(&scope, actions, notice);
                    }
                    Err(error) => {
                        self.project_application(&scope, Vec::new(), error_notice(error));
                    }
                }
            }
            (
                FlightKind::Request {
                    retired_generation,
                    action,
                },
                ApplicationReply::Request(result),
            ) => {
                let notice = match result {
                    Ok(outcome) => outcome_notice(action, outcome),
                    Err(error) => error_notice(error),
                };
                self.application_command_notice(&scope, retired_generation, notice);
            }
            _ => {}
        }
    }

    fn application_retired_current(
        &self,
        scope: &ApplicationScope,
        retired_generation: u64,
    ) -> bool {
        let key = self.key.borrow().clone();
        self.scope_generation.get() == Some(retired_generation)
            && !self.is_open()
            && !self.surface.window().is_visible()
            && self.surface.get_kind() == DockMenuKind::Pinned
            && key.as_str() == scope.key
            && self.application_source_current(scope)
    }

    fn application_retired_admitted(
        &self,
        scope: &ApplicationScope,
        retired_generation: u64,
    ) -> bool {
        self.application_retired_current(scope, retired_generation)
            && (scope.admission)()
            && self.application_retired_current(scope, retired_generation)
    }

    fn application_command_notice(
        &self,
        scope: &ApplicationScope,
        retired_generation: u64,
        notice: &str,
    ) {
        if !self.application_retired_admitted(scope, retired_generation) {
            return;
        }
        if let Some(dock) = self.dock.upgrade() {
            dock.set_application_menu_notice(DockApplicationNotice {
                key: scope.key.as_str().into(),
                text: notice.into(),
            });
        }
    }

    pub(super) fn execute_application(
        &self,
        token: &str,
        action: DockMenuAction,
        bounds: TileBounds,
    ) {
        let Some(action) = application_action(action) else {
            return;
        };
        let Some(scope) = self.application.scope() else {
            return;
        };
        if token != scope.generation.to_string()
            || self.application_projection.get().is_some()
            || !self.application_current(&scope)
            || self.is_focused() != Some(true)
            || !fully_visible_row(
                self.surface.window(),
                bounds.clone(),
                self.surface.get_application_viewport(),
            )
            || !self.surface.get_application_entries().iter().any(|entry| {
                entry.application_scope == token && application_action(entry.action) == Some(action)
            })
        {
            return;
        }
        let Some(command) = self.application.capture(scope.generation, action) else {
            return;
        };
        let Some(retired_generation) = scope.generation.checked_add(1) else {
            return;
        };
        // Capture the exact issuer target and Root approval before releasing focus.
        // Recheck Root/source retirement before dispatch; accepted work then survives hide.
        if !self.application_current(&scope)
            || self.is_focused() != Some(true)
            || !fully_visible_row(
                self.surface.window(),
                bounds,
                self.surface.get_application_viewport(),
            )
        {
            return;
        }
        self.hide();
        if !self.application_retired_admitted(&scope, retired_generation) {
            return;
        }
        self.application_command_notice(
            &scope,
            retired_generation,
            "Submitting application request…",
        );
        if !self.application_retired_admitted(&scope, retired_generation) {
            return;
        }
        if let Err(error) = self.application.request(command, retired_generation) {
            self.application_command_notice(&scope, retired_generation, error_notice(error));
        }
    }
}

fn fully_visible_row(window: &slint::Window, row: TileBounds, viewport: TileBounds) -> bool {
    let values = [
        row.origin.x,
        row.origin.y,
        row.width,
        row.height,
        viewport.origin.x,
        viewport.origin.y,
        viewport.width,
        viewport.height,
    ];
    values.iter().all(|value| value.is_finite())
        && row.width > 0.0
        && row.height > 0.0
        && viewport.width > 0.0
        && viewport.height > 0.0
        && row.origin.x >= viewport.origin.x
        && row.origin.y >= viewport.origin.y
        && row.origin.x + row.width <= viewport.origin.x + viewport.width
        && row.origin.y + row.height <= viewport.origin.y + viewport.height
        && super::preview_rect(window, row).is_ok()
        && super::preview_rect(window, viewport).is_ok()
}
