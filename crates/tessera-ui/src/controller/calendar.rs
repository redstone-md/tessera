// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Toolbar-origin ownership of the real local-date/locale popup.

use super::popups::PopupKind;
use super::{PanelController, Rc};
use crate::calendar::CalendarController;
use crate::generated::TileBounds;
use crate::theme::ThemedComponent;
use crate::transient_window::TransientCache;
use slint::ComponentHandle;

pub(super) type CalendarPopups = TransientCache<CalendarController>;

impl PanelController {
    pub(super) fn open_calendar(&self, bounds: TileBounds) {
        self.dismiss_tooltip(false);
        let Some(toolbar) = self.toolbar_and_upgrade() else {
            return;
        };
        if !toolbar.window().is_visible()
            || toolbar.get_clock().is_empty()
            || !bounds.origin.x.is_finite()
            || !bounds.origin.y.is_finite()
            || !bounds.width.is_finite()
            || !bounds.height.is_finite()
            || bounds.width <= 0.0
            || bounds.height <= 0.0
        {
            return;
        }
        let Some(context) = self.core.dock_context() else {
            self.report_message("The calendar needs the toolbar's real monitor geometry.");
            return;
        };
        let existing = self.calendar.borrow().clone();
        let calendar = match existing {
            Some(calendar) => calendar,
            None => match CalendarController::new(self.core.host().clone()) {
                Ok(calendar) => {
                    *self.calendar.borrow_mut() = Some(Rc::clone(&calendar));
                    calendar
                }
                Err(error) => {
                    self.report_message(&format!("Could not create the calendar: {error}"));
                    return;
                }
            },
        };
        calendar.apply_theme(toolbar.presentation_theme());
        calendar.set_start_of_week(self.core.applied_preferences().general().start_of_week());
        let result = calendar.show(toolbar.window(), bounds, context);
        self.popup_presentation_finished(PopupKind::Calendar, calendar.is_open(), result);
    }
}
