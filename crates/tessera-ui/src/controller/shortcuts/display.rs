// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A native trigger waits for one genuine containing monitor; it never moves bars.
//! A newer input/source revision revokes the old presentation instead of replaying it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::ComponentHandle;
use tessera_system::display_context::{DisplayContextError, DisplayLayout, DisplaySelection};
use tessera_system::shortcuts::{ShortcutAction, ShortcutTrigger};

use super::super::PanelController;
use super::ShortcutPresentation;
use crate::{DockContext, Panel};

struct Flight {
    token: u64,
    trigger: ShortcutTrigger,
    source: Option<DockContext>,
    source_scale: Option<f32>,
    input: Rc<()>,
}

#[derive(Default)]
struct Mailbox {
    expected: Option<u64>,
    result: Option<(u64, Result<Option<DisplayLayout>, DisplayContextError>)>,
}

#[derive(Default)]
pub(super) struct DisplayTargeting {
    sequence: Cell<u64>,
    flight: RefCell<Option<Flight>>,
    mailbox: Arc<Mutex<Mailbox>>,
    pub launcher_layout: Cell<Option<DisplayLayout>>,
    input: RefCell<Rc<()>>,
}

impl DisplayTargeting {
    pub fn retire_input(&self) {
        self.input.replace(Rc::new(()));
    }

    pub fn close(&self) {
        self.retire_input();
        self.launcher_layout.set(None);
        self.mailbox.lock().expected = None;
    }
}

fn complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    panel: &slint::Weak<Panel>,
    token: u64,
    result: Result<Option<DisplayLayout>, DisplayContextError>,
) {
    {
        let mut slot = mailbox.lock();
        if slot.expected != Some(token) || slot.result.is_some() {
            return;
        }
        slot.result = Some((token, result));
    }
    let _ = panel.upgrade_in_event_loop(|panel| panel.invoke_shortcuts_event_ready());
}

impl PanelController {
    fn shortcut_source_scale(&self) -> Option<f32> {
        self.dock_and_upgrade()
            .map(|dock| dock.window().scale_factor())
            .or_else(|| {
                self.panel
                    .upgrade()
                    .map(|panel| panel.window().scale_factor())
            })
    }

    pub(in super::super) fn shortcut_presentation_current(
        &self,
        scope: Option<&ShortcutPresentation>,
    ) -> bool {
        self.root_current()
            && scope.is_none_or(|scope| {
                self.shortcut_generation_current(&scope.trigger)
                    && Rc::ptr_eq(&scope.input, &self.shortcuts.display.input.borrow())
                    && self.core.dock_context() == scope.source
                    && self.shortcut_source_scale() == scope.source_scale
            })
    }

    pub(in super::super) fn request_shortcut_display(&self, trigger: ShortcutTrigger) {
        let Some(point) = trigger.cursor else {
            self.report_message(
                "Shortcut cursor position is unavailable; no monitor was selected.",
            );
            return;
        };
        let display = &self.shortcuts.display;
        display.retire_input();
        if display.flight.borrow().is_some() {
            return;
        }
        let Some(token) = display.sequence.get().checked_add(1) else {
            self.report_message("Shortcut display scope is exhausted.");
            return;
        };
        let input = display.input.borrow().clone();
        let source = self.core.dock_context();
        let source_scale = self.shortcut_source_scale();
        display.sequence.set(token);
        // Reserve before the factory: its callback can synchronously enter Root.
        display.flight.replace(Some(Flight {
            token,
            trigger,
            source,
            source_scale,
            input: input.clone(),
        }));
        display.mailbox.lock().expected = Some(token);
        let provider = self.core.host().display_context_host();
        let admitted = self.root_current()
            && self.shortcuts.enabled.get()
            && Rc::ptr_eq(&input, &display.input.borrow())
            && self.core.dock_context() == source
            && self.shortcut_source_scale() == source_scale
            && self.shortcut_generation_current(&trigger);
        if !admitted {
            display.flight.borrow_mut().take();
            display.mailbox.lock().expected = None;
            return;
        }
        let provider = match provider {
            Ok(Some(provider)) => provider,
            Ok(None) => {
                complete(
                    &display.mailbox,
                    &self.panel,
                    token,
                    Err(DisplayContextError::Unsupported),
                );
                return;
            }
            Err(error) => {
                complete(&display.mailbox, &self.panel, token, Err(error));
                return;
            }
        };
        let mailbox = display.mailbox.clone();
        let panel = self.panel.clone();
        let result = provider.read_selected(
            DisplaySelection::AtPoint {
                x: point.x,
                y: point.y,
            },
            Box::new(move |result| complete(&mailbox, &panel, token, result)),
        );
        if let Err(error) = result {
            complete(&display.mailbox, &self.panel, token, Err(error));
        }
    }

    pub(in super::super) fn process_shortcut_display(&self) {
        let display = &self.shortcuts.display;
        let result = display.mailbox.lock().result.take();
        let Some((token, result)) = result else {
            return;
        };
        let flight = {
            let mut flight = display.flight.borrow_mut();
            if flight.as_ref().is_none_or(|flight| flight.token != token) {
                return;
            }
            flight.take().expect("matching reserved display flight")
        };
        display.mailbox.lock().expected = None;
        if !self.root_current()
            || !self.shortcuts.enabled.get()
            || !Rc::ptr_eq(&flight.input, &display.input.borrow())
            || self.core.dock_context() != flight.source
            || self.shortcut_source_scale() != flight.source_scale
            || !self.shortcut_generation_current(&flight.trigger)
        {
            return;
        }
        let Some(point) = flight.trigger.cursor else {
            return;
        };
        let layout = match result {
            Ok(Some(layout))
                if layout.selection()
                    == (DisplaySelection::AtPoint {
                        x: point.x,
                        y: point.y,
                    })
                    && i64::from(point.x) >= i64::from(layout.selected_bounds().x())
                    && i64::from(point.y) >= i64::from(layout.selected_bounds().y())
                    && i64::from(point.x) < i64::from(layout.selected_bounds().right())
                    && i64::from(point.y) < i64::from(layout.selected_bounds().bottom())
                    && (layout.presentation_scale() as f32).is_finite()
                    && (layout.presentation_scale() as f32) > 0.0 =>
            {
                layout
            }
            Ok(None) => {
                self.report_message("No current monitor contains the shortcut cursor.");
                return;
            }
            Ok(Some(_)) => {
                self.report_message("Shortcut monitor result does not match its native cursor.");
                return;
            }
            Err(_) => {
                self.report_message("The shortcut cursor monitor is unavailable.");
                return;
            }
        };
        let scope = Rc::new(ShortcutPresentation {
            trigger: flight.trigger,
            input: flight.input,
            source: flight.source,
            source_scale: flight.source_scale,
        });
        self.run_shortcut_action(scope, layout);
    }

    fn run_shortcut_action(&self, scope: Rc<ShortcutPresentation>, layout: DisplayLayout) {
        let admitted = || self.shortcut_presentation_current(Some(&scope));
        if !admitted() {
            return;
        }
        self.with_shortcut_presentation(scope.clone(), || match scope.trigger.action {
            ShortcutAction::ToggleLauncher if self.launcher.is_some() => {
                self.shortcuts.display.launcher_layout.set(Some(layout));
                self.open_launcher_at_shortcut_monitor();
            }
            ShortcutAction::ToggleLauncher | ShortcutAction::OpenSettings => {
                if let Some(panel) = self.panel.upgrade() {
                    let bounds = layout.selected_bounds();
                    let size = panel.window().size();
                    let x = i64::from(bounds.x())
                        + (i64::from(bounds.width()) - i64::from(size.width)) / 2;
                    let y = i64::from(bounds.y())
                        + (i64::from(bounds.height()) - i64::from(size.height)) / 2;
                    let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
                        return;
                    };
                    panel
                        .window()
                        .set_position(slint::PhysicalPosition::new(x, y));
                    if !admitted() {
                        return;
                    }
                }
                self.open_panel_at_shortcut_monitor();
            }
        });
    }
}
