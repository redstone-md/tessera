// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Root owns policy and full-record Save; the shortcut actor owns native flight.
//! All actor callbacks return through weak component relays, never a Root cycle.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use slint::ComponentHandle;
use slint::language::KeyEvent;
use tessera_system::shortcuts::{
    ShortcutAction, ShortcutConfig, ShortcutError, ShortcutErrorKind, ShortcutTrigger,
};

use super::PanelController;
use super::popups::PopupKind;
use crate::Panel;
use crate::shortcuts::{ShortcutsBindings, ShortcutsController, ShortcutsView};

mod display;

pub(super) struct RootAdmission {
    pub alive: Cell<bool>,
    pub active_popup: Cell<Option<PopupKind>>,
}

impl Default for RootAdmission {
    fn default() -> Self {
        Self {
            alive: Cell::new(true),
            active_popup: Cell::new(None),
        }
    }
}

#[derive(Default)]
pub(super) struct ShortcutIntegration {
    actor: RefCell<Option<Rc<ShortcutsController>>>,
    started: Cell<bool>,
    enabled: Cell<bool>,
    backend_absent: Cell<bool>,
    capture_requested: Cell<bool>,
    trigger: RefCell<Option<ShortcutTrigger>>,
    presentation: RefCell<Option<Rc<ShortcutPresentation>>>,
    display: display::DisplayTargeting,
}

/// Each native presentation keeps its own revocable intent and source geometry.
/// Clearing the ambient callback context never turns an old shortcut into a manual open.
pub(super) struct ShortcutPresentation {
    trigger: ShortcutTrigger,
    input: Rc<()>,
    source: Option<crate::DockContext>,
    source_scale: Option<f32>,
}

/// Declared after construction and retired before any other native teardown.
pub(super) struct RootCapabilityScope {
    admission: Rc<RootAdmission>,
    shortcuts: Rc<ShortcutIntegration>,
    power_closed: Rc<Cell<bool>>,
}

impl RootCapabilityScope {
    pub fn new(root: &PanelController) -> Self {
        Self {
            admission: root.admission.clone(),
            shortcuts: root.shortcuts.clone(),
            power_closed: root.power_admission_closed.clone(),
        }
    }
}

impl Drop for RootCapabilityScope {
    fn drop(&mut self) {
        // Preserve existing Power revocation before callback-capable shortcut cleanup.
        self.power_closed.set(true);
        self.admission.alive.set(false);
        self.admission.active_popup.set(None);
        self.shortcuts.enabled.set(false);
        self.shortcuts.capture_requested.set(false);
        self.shortcuts.trigger.borrow_mut().take();
        self.shortcuts.display.close();
        let actor = self.shortcuts.actor.borrow_mut().take();
        if let Some(actor) = actor {
            actor.cancel_current();
        }
    }
}

fn current(
    panel: &slint::Weak<Panel>,
    admission: &Weak<RootAdmission>,
    power: &Weak<Cell<bool>>,
) -> bool {
    panel.upgrade().is_some()
        && admission.upgrade().is_some_and(|scope| scope.alive.get())
        && power.upgrade().is_some_and(|closed| !closed.get())
}

fn view_is_current(
    panel: &slint::Weak<Panel>,
    state: &ShortcutIntegration,
    expected: &ShortcutsView,
) -> bool {
    let actor = state.actor.borrow().clone();
    let unchanged = actor.is_some_and(|actor| actor.view() == *expected);
    if !unchanged {
        // A setter-reentered draft edit cannot recursively project through the actor.
        let _ = panel.upgrade_in_event_loop(|panel| panel.invoke_shortcuts_event_ready());
    }
    unchanged
}

fn project_view(panel: &Panel, view: &ShortcutsView, message: &str, admitted: impl Fn() -> bool) {
    macro_rules! set {
        ($call:expr) => {
            if !admitted() {
                return;
            }
            $call;
        };
    }
    set!(panel.set_shortcuts_enabled(view.enabled));
    set!(panel.set_shortcuts_settings_label(view.settings_label.as_str().into()));
    set!(panel.set_shortcuts_capturing(view.capturing));
    set!(panel.set_shortcuts_dirty(view.dirty));
    set!(panel.set_shortcuts_applying(view.applying));
    set!(panel.set_shortcuts_launcher_status(view.launcher_status.as_str().into()));
    set!(panel.set_shortcuts_settings_status(view.settings_status.as_str().into()));
    set!(panel.set_shortcuts_message(message.into()));
}

impl PanelController {
    pub(super) fn root_current(&self) -> bool {
        self.admission.alive.get()
            && !self.power_admission_closed.get()
            && self.panel.upgrade().is_some()
    }

    fn shortcut_actor(&self) -> Option<Rc<ShortcutsController>> {
        self.shortcuts.actor.borrow().clone()
    }

    /// Bind once after the complete Root value exists. Construction does no native work.
    pub(super) fn wire_shortcuts(&self, panel: &Panel) {
        let admission = Rc::downgrade(&self.admission);
        let power = Rc::downgrade(&self.power_admission_closed);
        let owner = self.panel.clone();
        let core = Arc::downgrade(&self.core);
        let state = Rc::downgrade(&self.shortcuts);
        let factory = Rc::new(move || {
            let Some(state) = state.upgrade() else {
                return Ok(None);
            };
            if !state.enabled.get() || !current(&owner, &admission, &power) {
                return Ok(None);
            }
            let Some(core) = core.upgrade() else {
                return Ok(None);
            };
            let result = core.host().shortcuts_host();
            if !state.enabled.get() || !current(&owner, &admission, &power) {
                return Err(ShortcutError::new(
                    ShortcutErrorKind::Unsupported,
                    None,
                    "Shortcut Root retired",
                ));
            }
            state.backend_absent.set(matches!(&result, Ok(None)));
            result
        });
        let admission = Rc::downgrade(&self.admission);
        let power = Rc::downgrade(&self.power_admission_closed);
        let owner = self.panel.clone();
        let state = Rc::downgrade(&self.shortcuts);
        let is_current = Rc::new(move || {
            current(&owner, &admission, &power)
                && state.upgrade().is_some_and(|state| state.enabled.get())
        });
        let admission = Rc::downgrade(&self.admission);
        let power = Rc::downgrade(&self.power_admission_closed);
        let owner = self.panel.clone();
        let state = Rc::downgrade(&self.shortcuts);
        let project = Rc::new(move |view: ShortcutsView| {
            let Some(state) = state.upgrade() else {
                return;
            };
            let Some(panel) = owner.upgrade() else {
                return;
            };
            let admitted = || {
                current(&owner, &admission, &power)
                    && state.enabled.get()
                    && view_is_current(&owner, &state, &view)
            };
            project_view(&panel, &view, &view.message, admitted);
            if admitted()
                && state.capture_requested.get()
                && (view.pause_applied || state.backend_absent.get())
            {
                let _ = owner.upgrade_in_event_loop(|panel| panel.invoke_shortcuts_event_ready());
            }
        });
        let owner = self.panel.clone();
        let admission = Rc::downgrade(&self.admission);
        let power = Rc::downgrade(&self.power_admission_closed);
        let state = Rc::downgrade(&self.shortcuts);
        let deliver = Rc::new(move |trigger| {
            if !current(&owner, &admission, &power) {
                return;
            }
            let Some(state) = state.upgrade() else {
                return;
            };
            if !state.enabled.get() {
                return;
            }
            state.trigger.replace(Some(trigger));
            if let Some(panel) = owner.upgrade() {
                panel.invoke_shortcut_trigger_ready();
            }
        });
        let owner = self.panel.clone();
        let wake = Arc::new(move || {
            let _ = owner.upgrade_in_event_loop(|panel| panel.invoke_shortcuts_event_ready());
        });
        let actor = Rc::new(ShortcutsController::new(
            self.core.applied_preferences().shortcuts().clone(),
            ShortcutsBindings {
                factory,
                is_current,
                project,
                deliver,
                wake,
            },
        ));
        self.shortcuts.actor.replace(Some(actor));

        let root = self.clone();
        panel.on_shortcuts_event_ready(move || root.process_shortcuts());
        let root = self.clone();
        panel.on_shortcut_trigger_ready(move || root.deliver_shortcut());
        let root = self.clone();
        panel.on_shortcuts_enabled_edited(move |enabled| {
            if !root.shortcut_editor_ready() {
                return;
            }
            root.end_shortcut_capture();
            if let Some(actor) = root.shortcut_actor() {
                actor.set_draft_enabled(enabled);
            }
            root.project_shortcut_draft();
        });
        let root = self.clone();
        panel.on_shortcuts_capture_requested(move || root.request_shortcut_capture());
        let root = self.clone();
        panel.on_shortcuts_capture_cancelled(move || root.end_shortcut_capture());
        let root = self.clone();
        panel.on_shortcuts_reset_requested(move || {
            if !root.shortcut_editor_ready() {
                return;
            }
            root.end_shortcut_capture();
            if let Some(actor) = root.shortcut_actor() {
                actor.reset_settings();
            }
            root.project_shortcut_draft();
        });
        let root = self.clone();
        panel.on_shortcuts_capture_pressed(move |event| root.shortcut_key(event, true));
        let root = self.clone();
        panel.on_shortcuts_capture_released(move |event| root.shortcut_key(event, false));
        let root = self.clone();
        panel.on_cancel_preferences_requested(move || root.cancel_preferences());
        let root = self.clone();
        panel.window().on_close_requested(move || {
            root.reset_shortcut_draft();
            slint::CloseRequestResponse::HideWindow
        });
    }

    pub(super) fn start_shortcuts(&self, enabled: bool) {
        if self.shortcuts.started.replace(true) || !self.root_current() {
            return;
        }
        self.shortcuts.enabled.set(enabled);
        if enabled && let Some(actor) = self.shortcut_actor() {
            actor.apply_saved(self.core.applied_preferences().shortcuts().clone());
        }
        self.project_shortcut_draft();
    }

    fn shortcut_editor_ready(&self) -> bool {
        self.root_current()
            && !self.preference_saving.get()
            && self
                .panel
                .upgrade()
                .is_some_and(|panel| panel.window().is_visible())
    }

    pub(super) fn shortcut_draft(&self) -> ShortcutConfig {
        self.shortcut_actor().map_or_else(
            || self.core.applied_preferences().shortcuts().clone(),
            |actor| actor.draft_config(),
        )
    }

    pub(super) fn shortcuts_saved(&self, preferences: &crate::PanelPreferences) {
        if !self.root_current() || self.core.applied_preferences() != *preferences {
            return;
        }
        self.shortcuts.capture_requested.set(false);
        self.shortcuts.display.retire_input();
        if let Some(actor) = self.shortcut_actor() {
            if self.shortcuts.enabled.get() {
                actor.apply_saved(preferences.shortcuts().clone());
            } else {
                actor.reset_draft(preferences.shortcuts().clone());
            }
        }
        self.project_shortcut_draft();
    }

    pub(super) fn reset_shortcut_draft(&self) {
        self.shortcuts.display.retire_input();
        self.prepare_shortcut_draft();
    }

    /// Opening Settings initializes the editor without cancelling its own admitted input.
    pub(super) fn prepare_shortcut_draft(&self) {
        self.end_shortcut_capture();
        if self.root_current()
            && let Some(actor) = self.shortcut_actor()
        {
            actor.reset_draft(self.core.applied_preferences().shortcuts().clone());
        }
        self.project_shortcut_draft();
    }

    fn cancel_preferences(&self) {
        if !self.shortcut_editor_ready() {
            return;
        }
        self.reset_shortcut_draft();
        let saved = self.core.applied_preferences();
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        panel.set_theme_index(crate::theme_to_index(saved.theme()));
        if !self.root_current() {
            return;
        }
        panel.set_compact(saved.compact());
        if !self.root_current() {
            return;
        }
        panel.set_dock_edge_index(crate::dock_edge_to_index(saved.dock_edge()));
        if !self.root_current() {
            return;
        }
        panel.set_start_of_week_index(saved.general().start_of_week().index());
        if self.dock.is_some() {
            let _ = panel.hide();
        }
    }

    fn request_shortcut_capture(&self) {
        if !self.shortcut_editor_ready() {
            return;
        }
        let Some(actor) = self.shortcut_actor() else {
            return;
        };
        if !actor.draft_config().enabled() {
            return;
        }
        self.shortcuts.display.retire_input();
        self.shortcuts.capture_requested.set(true);
        if self.shortcuts.enabled.get() {
            actor.set_paused(true);
        }
        self.continue_shortcut_capture();
    }

    fn continue_shortcut_capture(&self) {
        if !self.shortcuts.capture_requested.get() {
            return;
        }
        if !self.shortcut_editor_ready() {
            self.end_shortcut_capture();
            return;
        }
        let Some(actor) = self.shortcut_actor() else {
            return;
        };
        let view = actor.view();
        let local_only = !self.shortcuts.enabled.get() || self.shortcuts.backend_absent.get();
        if view.applying || (!view.pause_applied && !local_only) {
            return;
        }
        self.shortcuts.capture_requested.set(false);
        actor.begin_capture();
        self.project_shortcut_draft();
        if self.shortcut_editor_ready()
            && actor.view().capturing
            && let Some(panel) = self.panel.upgrade()
        {
            panel.invoke_focus_shortcut_capture();
        }
    }

    fn end_shortcut_capture(&self) {
        self.shortcuts.capture_requested.set(false);
        let Some(actor) = self.shortcut_actor() else {
            return;
        };
        actor.cancel_capture();
        if self.root_current() && self.shortcuts.enabled.get() {
            actor.set_paused(false);
        }
        self.project_shortcut_draft();
    }

    fn shortcut_key(&self, event: KeyEvent, pressed: bool) -> bool {
        if !self.shortcut_editor_ready() {
            return false;
        }
        let Some(actor) = self.shortcut_actor() else {
            return false;
        };
        if !actor.view().capturing {
            return false;
        }
        let result = if pressed {
            actor.key_pressed(event)
        } else {
            actor.key_released(event)
        };
        if !actor.view().capturing {
            self.end_shortcut_capture();
        }
        self.project_shortcut_draft();
        result
    }

    fn project_shortcut_draft(&self) {
        if !self.root_current() {
            return;
        }
        let Some(actor) = self.shortcut_actor() else {
            return;
        };
        if self.shortcuts.enabled.get() {
            actor.project();
            return;
        }
        // Inert/diagnostic drafts are local: no apply_saved, factory or subscription.
        let view = actor.view();
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let admitted = || {
            self.root_current()
                && !self.shortcuts.enabled.get()
                && view_is_current(&self.panel, &self.shortcuts, &view)
        };
        project_view(
            &panel,
            &view,
            "Global input is disabled for this presentation.",
            admitted,
        );
    }

    pub(super) fn process_shortcuts(&self) {
        if !self.root_current() {
            return;
        }
        if self
            .panel
            .upgrade()
            .is_none_or(|panel| !panel.window().is_visible())
            && self
                .shortcut_actor()
                .is_some_and(|actor| actor.view().capturing)
        {
            self.end_shortcut_capture();
        }
        if self.shortcuts.enabled.get()
            && let Some(actor) = self.shortcut_actor()
        {
            actor.process_pending();
        }
        self.continue_shortcut_capture();
        self.process_shortcut_display();
        if !self.shortcuts.enabled.get() {
            self.project_shortcut_draft();
        }
    }

    fn deliver_shortcut(&self) {
        let trigger = self.shortcuts.trigger.borrow_mut().take();
        if !self.root_current() || !self.shortcuts.enabled.get() {
            return;
        }
        let Some(trigger) = trigger else {
            return;
        };
        if !self.shortcut_generation_current(&trigger) {
            return;
        }
        self.shortcuts.display.retire_input();
        if trigger.action == ShortcutAction::ToggleLauncher && self.launcher_visible() {
            self.hide_launcher();
            return;
        }
        self.request_shortcut_display(trigger);
    }

    pub(super) fn shortcut_generation_current(&self, trigger: &ShortcutTrigger) -> bool {
        self.root_current()
            && self.shortcuts.enabled.get()
            && self
                .shortcut_actor()
                .is_some_and(|actor| actor.accepts_trigger(trigger))
    }

    pub(super) fn shortcut_presentation_scope(&self) -> Option<Rc<ShortcutPresentation>> {
        self.shortcuts.presentation.borrow().clone()
    }

    pub(super) fn with_shortcut_presentation(
        &self,
        scope: Rc<ShortcutPresentation>,
        present: impl FnOnce(),
    ) {
        self.shortcuts.presentation.replace(Some(scope.clone()));
        present();
        let current = self.shortcuts.presentation.borrow().clone();
        if current
            .as_ref()
            .is_some_and(|current| Rc::ptr_eq(current, &scope))
        {
            self.shortcuts.presentation.replace(None);
        }
    }

    pub(super) fn launcher_context(&self) -> Option<crate::DockContext> {
        let source = self.core.dock_context();
        self.shortcuts
            .display
            .launcher_layout
            .get()
            .and_then(|layout| {
                let bounds = layout.selected_bounds();
                crate::DockContext::new(
                    bounds.x(),
                    bounds.y(),
                    bounds.width(),
                    bounds.height(),
                    source.is_some_and(|source| source.fullscreen_active()),
                )
            })
            .or(source)
    }

    pub(super) fn launcher_presentation_scale(&self, current: f32) -> f32 {
        self.shortcuts
            .display
            .launcher_layout
            .get()
            .map_or(current, |layout| layout.presentation_scale() as f32)
    }

    pub(super) fn retire_shortcut_presentation(&self) {
        self.shortcuts.presentation.replace(None);
        self.shortcuts.display.retire_input();
    }

    pub(super) fn clear_shortcut_launcher_monitor(&self) {
        self.retire_shortcut_presentation();
        self.shortcuts.display.launcher_layout.set(None);
    }
}
