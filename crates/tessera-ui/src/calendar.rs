// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Read-only calendar effects: one native surface, one accepted read, private authority.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, PhysicalPosition, PhysicalSize, SharedString};
use tessera_system::calendar::{
    CalendarDirection, CalendarError, CalendarErrorKind, CalendarHost, CalendarSnapshot,
    CalendarState, CalendarView, CivilDate,
};

use crate::generated::{
    CalendarDayCell, CalendarMenu, CalendarMode, CalendarMonthCell, CalendarMonthRow,
    CalendarWeekRow, PopoverMotion, TileBounds,
};
use crate::popup_placement::{self as placement, PopupRect};
use crate::sanitize::bounded_text;
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::transient_window::{TransientComponent, TransientWindow};
use crate::{DesktopHost, DockContext, SurfaceKind};

pub(crate) mod preferences;

use preferences::StartOfWeek;

#[cfg(test)]
mod preferences_control_tests;
#[cfg(test)]
mod tests;

impl TransientComponent for CalendarMenu {
    fn motion(&self) -> PopoverMotion<'_> {
        self.global::<PopoverMotion>()
    }

    fn set_presentation_opacity(&self, opacity: f32) {
        self.invoke_set_presentation_opacity(opacity);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ReadToken {
    session: u64,
    sequence: u64,
}

/// Keys are opaque capabilities issued by this projection, never dates from UI text.
#[derive(Default)]
struct Keys {
    action: SharedString,
    days: Vec<(SharedString, CivilDate)>,
    months: Vec<(SharedString, u32)>,
}

#[derive(Default)]
struct State {
    session: u64,
    sequence: u64,
    key_sequence: u64,
    provider: Option<Arc<dyn CalendarHost>>,
    acquiring: bool,
    read_requested: bool,
    flight: Option<ReadToken>,
    calendar: Option<CalendarState>,
    start_of_week: StartOfWeek,
    keys: Keys,
    notice: String,
}

impl State {
    fn loading(&self) -> bool {
        self.read_requested
            || self.acquiring
            || self.flight.is_some_and(|t| t.session == self.session)
    }

    fn interactive(&self) -> bool {
        !self.loading() && self.notice.is_empty() && self.calendar.is_some()
    }

    fn key(&mut self) -> SharedString {
        self.key_sequence = self.key_sequence.wrapping_add(1);
        format!("calendar-{:x}-{:x}", self.session, self.key_sequence).into()
    }
}

struct Completion {
    token: ReadToken,
    result: Result<CalendarSnapshot, CalendarError>,
}

/// Capacity one is sufficient: accepted work remains in flight across hide/reopen.
#[derive(Default)]
struct Mailbox {
    expected: Option<ReadToken>,
    completion: Option<Completion>,
    wake_queued: bool,
}

fn complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<CalendarMenu>,
    token: ReadToken,
    result: Result<CalendarSnapshot, CalendarError>,
) {
    {
        let mut mailbox = mailbox.lock();
        if mailbox.expected != Some(token) || mailbox.completion.is_some() {
            return;
        }
        mailbox.completion = Some(Completion { token, result });
        if mailbox.wake_queued {
            return;
        }
        mailbox.wake_queued = true;
    }
    if root
        .upgrade_in_event_loop(|root| root.invoke_calendar_event_ready())
        .is_err()
    {
        // A headless backend may manually invoke this same production wakeup.
        mailbox.lock().wake_queued = false;
    }
}

#[derive(Clone, Copy)]
struct Placement {
    anchor: PhysicalPosition,
    context: DockContext,
    scale: f32,
}

/// Native toolbar geometry owns Y; only the genuine clock tile owns X.
fn toolbar_anchor(
    origin: PhysicalPosition,
    size: PhysicalSize,
    scale: f32,
    bounds: &TileBounds,
) -> Result<PhysicalPosition, String> {
    if size.width == 0
        || size.height == 0
        || !bounds.origin.x.is_finite()
        || !bounds.origin.y.is_finite()
        || !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
    {
        return Err("Calendar needs positive native geometry and valid input bounds.".into());
    }
    let bottom = i32::try_from(i64::from(origin.y) + i64::from(size.height))
        .map_err(|_| "Calendar source bottom is outside native coordinates.")?;
    // Reserve the same ten-logical-pixel shadow gutter as other toolbar popups.
    placement::physical_anchor(
        PhysicalPosition::new(origin.x, bottom),
        scale,
        (bounds.origin.x + bounds.width / 2.0, -10.0),
    )
    .map_err(str::to_owned)
}

struct Guard<'a>(&'a Cell<bool>);

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

#[derive(Clone, Copy)]
enum Action {
    Navigate(CalendarDirection),
    Toggle,
    Today,
}

pub(crate) struct CalendarController {
    surface: TransientWindow<CalendarMenu>,
    host: Arc<dyn DesktopHost>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
    projecting: Cell<bool>,
    policy_projection_pending: Cell<bool>,
    presenting: Cell<bool>,
    placement: Cell<Option<Placement>>,
    rect: RefCell<Option<PopupRect>>,
    fit_timer: slint::Timer,
    focus_watch: slint::Timer,
    refresh_timer: slint::Timer,
    focus_seen: Cell<bool>,
}

impl CalendarController {
    pub(crate) fn new(host: Arc<dyn DesktopHost>) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            surface: TransientWindow::new(host.clone(), CalendarMenu::new()?, SurfaceKind::Popup),
            host,
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            projecting: Cell::new(false),
            policy_projection_pending: Cell::new(false),
            presenting: Cell::new(false),
            placement: Cell::new(None),
            rect: RefCell::default(),
            fit_timer: slint::Timer::default(),
            focus_watch: slint::Timer::default(),
            refresh_timer: slint::Timer::default(),
            focus_seen: Cell::new(false),
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_calendar_event_ready(move || {
            if let Some(controller) = weak.upgrade() {
                controller.drain();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller
            .surface
            .on_navigate_requested(move |forward, key| {
                if let Some(controller) = weak.upgrade() {
                    controller.action(
                        key,
                        Action::Navigate(if forward {
                            CalendarDirection::Next
                        } else {
                            CalendarDirection::Previous
                        }),
                    );
                }
            });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_toggle_view_requested(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.action(key, Action::Toggle);
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_today_requested(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.action(key, Action::Today);
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_day_selected(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.select_day(key);
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_month_selected(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.select_month(key);
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_retry_requested(move || {
            if let Some(controller) = weak.upgrade() {
                controller.retry();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_hide_requested(move || {
            if let Some(controller) = weak.upgrade() {
                controller.hide();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_preferred_size_changed(move || {
            if let Some(controller) = weak.upgrade() {
                controller.schedule_fit();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.window().on_close_requested(move || {
            if let Some(controller) = weak.upgrade() {
                controller.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        Ok(controller)
    }

    pub(crate) fn component(&self) -> &CalendarMenu {
        &self.surface
    }

    pub(crate) fn is_open(&self) -> bool {
        self.surface.is_visible() && self.component().window().is_visible()
    }

    pub(crate) fn show(
        self: &Rc<Self>,
        source: &slint::Window,
        bounds: TileBounds,
        context: DockContext,
    ) -> Result<(), String> {
        if self.presenting.get() {
            return Err("Calendar presentation is already in progress.".into());
        }
        self.hide();
        if !source.is_visible() || context.fullscreen_active() {
            return Err("Calendar needs a visible source and valid input bounds.".into());
        }
        let scale = source.scale_factor();
        let anchor = toolbar_anchor(source.position(), source.size(), scale, &bounds)?;
        self.placement.set(Some(Placement {
            anchor,
            context,
            scale,
        }));
        let session = {
            let mut state = self.state.borrow_mut();
            state.read_requested = true;
            state.session
        };
        self.project();
        let presentation = {
            self.presenting.set(true);
            let _guard = Guard(&self.presenting);
            self.preferred_rect().and_then(|rect| {
                self.surface
                    .present(rect.position, rect.size)
                    .map(|shown| (rect, shown))
            })
        };
        match presentation {
            Ok((rect, true)) if self.current(session) => *self.rect.borrow_mut() = Some(rect),
            Ok(_) => {
                self.hide();
                return Ok(());
            }
            Err(error) => {
                self.hide();
                return Err(error);
            }
        }
        self.surface.invoke_focus_content();
        if !self.current(session) {
            return Ok(());
        }
        let focus = self.surface.request_focus();
        if !self.current(session) {
            return Ok(());
        }
        self.watch_focus();
        // Retire a mailbox completion whose wake arrived during native attachment.
        self.drain();
        focus.map_err(|error| {
            bounded_text(
                &format!("Calendar opened, but keyboard focus was not granted: {error}"),
                240,
            )
        })
    }

    pub(crate) fn hide(&self) {
        self.fit_timer.stop();
        self.focus_watch.stop();
        self.refresh_timer.stop();
        self.focus_seen.set(false);
        {
            let mut state = self.state.borrow_mut();
            state.session = state.session.wrapping_add(1);
            state.read_requested = false;
            state.keys = Keys::default();
            state.notice.clear();
            // Accepted work is not cancelled: retirement owns its original token.
            // Pure browsing state survives native-window closure; only metadata
            // changes or an explicit Today action may reset it on the next read.
        }
        self.placement.set(None);
        self.rect.borrow_mut().take();
        self.surface.set_action_key(SharedString::default());
        self.surface.set_weeks(ModelRc::default());
        self.surface.set_month_rows(ModelRc::default());
        self.surface.set_retry_enabled(false);
        self.surface.hide();
    }

    pub(crate) fn disable_motion(&self) {
        self.surface.disable_motion();
    }

    pub(crate) fn apply_theme(&self, theme: PresentationTheme) {
        self.component().apply_presentation_theme(theme);
        let _ = self.refit();
    }

    /// Apply a saved policy without acquiring metadata or reopening the native surface.
    pub(crate) fn set_start_of_week(&self, start: StartOfWeek) {
        {
            let mut state = self.state.borrow_mut();
            if state.start_of_week == start {
                return;
            }
            state.start_of_week = start;
            if let Some(calendar) = state.calendar.as_mut() {
                calendar.set_week_start(start.into());
            }
            // A previously queued input must not act on the newly aligned grid.
            state.keys = Keys::default();
        }
        if self.is_open() {
            if self.projecting.get() {
                self.policy_projection_pending.set(true);
            } else {
                self.project_and_fit();
            }
        }
    }

    pub(crate) fn close_if_geometry_changed(&self, context: DockContext, scale: f32) {
        if let Some(previous) = self.placement.get() {
            let old = previous.context;
            if context.fullscreen_active()
                || previous.scale != scale
                || (old.x(), old.y(), old.width(), old.height())
                    != (context.x(), context.y(), context.width(), context.height())
            {
                self.hide();
            }
        }
    }

    fn current(&self, session: u64) -> bool {
        self.is_open() && self.state.borrow().session == session
    }

    fn accepts_input(&self) -> bool {
        self.is_open() && !self.projecting.get() && !self.presenting.get()
    }

    fn action(&self, key: SharedString, action: Action) {
        if !self.accepts_input() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if !state.interactive() || key.is_empty() || state.keys.action != key {
                return;
            }
            let Some(calendar) = state.calendar.as_mut() else {
                return;
            };
            match action {
                Action::Navigate(direction) => {
                    calendar.navigate(direction);
                }
                Action::Toggle => calendar.toggle_view(),
                Action::Today => calendar.today(),
            }
        }
        self.project_and_fit();
    }

    fn select_day(&self, key: SharedString) {
        if !self.accepts_input() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if !state.interactive() {
                return;
            }
            let Some(date) = state
                .keys
                .days
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, d)| *d)
            else {
                return;
            };
            if let Some(calendar) = state.calendar.as_mut() {
                calendar.select_day(date);
            }
        }
        self.project_and_fit();
    }

    fn select_month(&self, key: SharedString) {
        if !self.accepts_input() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if !state.interactive() {
                return;
            }
            let Some(month) = state
                .keys
                .months
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, m)| *m)
            else {
                return;
            };
            if let Some(calendar) = state.calendar.as_mut() {
                calendar.select_month(month);
            }
        }
        self.project_and_fit();
    }

    fn retry(self: &Rc<Self>) {
        if !self.accepts_input() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if state.loading() || state.flight.is_some() || state.notice.is_empty() {
                return;
            }
            state.notice.clear();
            state.read_requested = true;
        }
        self.pump();
    }

    /// Factories and provider calls never run under a UI-state or native-lease borrow.
    fn pump(self: &Rc<Self>) {
        if !self.is_open() {
            return;
        }
        let acquire = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || state.acquiring || !state.read_requested {
                return;
            }
            if state.provider.is_none() {
                state.acquiring = true;
                Some(state.session)
            } else {
                None
            }
        };
        if let Some(session) = acquire {
            let result = self.host.calendar_host();
            {
                let mut state = self.state.borrow_mut();
                state.acquiring = false;
                match result {
                    Ok(Some(provider)) => state.provider = Some(provider),
                    result if state.session == session => {
                        state.read_requested = false;
                        state.notice = match result {
                            Err(error) => failure(error.kind),
                            Ok(None) => failure(CalendarErrorKind::Unsupported),
                            Ok(Some(_)) => unreachable!(),
                        }
                        .into();
                    }
                    _ => {}
                }
            }
            self.project_and_fit();
            if !self.current(session) {
                self.pump();
                return;
            }
        }
        let (provider, token) = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || !state.read_requested {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            state.read_requested = false;
            state.sequence = state.sequence.wrapping_add(1);
            let token = ReadToken {
                session: state.session,
                sequence: state.sequence,
            };
            state.flight = Some(token);
            (provider, token)
        };
        self.refresh_timer.stop();
        self.mailbox.lock().expected = Some(token);
        self.project_and_fit();
        if !self.current(token.session) {
            self.state.borrow_mut().flight = None;
            self.mailbox.lock().expected = None;
            self.pump();
            return;
        }
        let mailbox = self.mailbox.clone();
        let root = self.surface.as_weak();
        if let Err(error) = provider.read(Box::new(move |result| {
            complete(&mailbox, &root, token, result)
        })) {
            complete(&self.mailbox, &self.surface.as_weak(), token, Err(error));
        }
    }

    fn drain(self: &Rc<Self>) {
        if self.projecting.get() || self.presenting.get() {
            return;
        }
        let completion = {
            let mut mailbox = self.mailbox.lock();
            mailbox.wake_queued = false;
            let completion = mailbox.completion.take();
            if completion.is_some() {
                mailbox.expected = None;
            }
            completion
        };
        let visible = self.is_open();
        {
            let mut state = self.state.borrow_mut();
            if let Some(completion) = completion
                && state.flight == Some(completion.token)
            {
                state.flight = None;
                if visible && state.session == completion.token.session {
                    // Resolve the policy now, not when the asynchronous read was accepted.
                    match completion.result.and_then(|snapshot| {
                        preferences::adapt_snapshot(snapshot, state.start_of_week)
                    }) {
                        Ok(snapshot) => {
                            if let Some(calendar) = state.calendar.as_mut() {
                                calendar.refresh(snapshot);
                            } else {
                                state.calendar = Some(CalendarState::new(snapshot));
                            }
                            state.notice.clear();
                        }
                        Err(error) => state.notice = failure(error.kind).into(),
                    }
                }
            }
        }
        if visible {
            self.project_and_fit();
            self.pump();
            self.schedule_refresh();
        }
    }

    fn project(&self) {
        if self.projecting.replace(true) {
            return;
        }
        let _guard = Guard(&self.projecting);
        self.policy_projection_pending.set(false);
        let (session, projection, keys, loading, notice, retry) = {
            let mut state = self.state.borrow_mut();
            let loading = state.loading();
            let projection = state.calendar.as_ref().map(CalendarState::projection);
            let mut keys = Keys::default();
            if self.is_open() && state.interactive() {
                keys.action = state.key();
                if let Some(projection) = &projection {
                    for day in &projection.days {
                        if let Some(date) = day.date {
                            keys.days.push((state.key(), date));
                        }
                    }
                    if projection.view == CalendarView::Year {
                        for month in &projection.months {
                            keys.months.push((state.key(), month.month));
                        }
                    }
                }
            }
            state.keys = Keys {
                action: keys.action.clone(),
                days: keys.days.clone(),
                months: keys.months.clone(),
            };
            let retry = !loading && state.flight.is_none() && !state.notice.is_empty();
            (
                state.session,
                projection,
                keys,
                loading,
                state.notice.clone(),
                retry,
            )
        };
        let root = self.component();
        // Disable UI actions before any other property changes; Rust also guards callbacks.
        root.set_action_key(SharedString::default());
        root.set_loading(loading);
        root.set_notice(notice.into());
        root.set_retry_enabled(retry);
        if let Some(projection) = projection {
            root.set_title_text(projection.title.into());
            root.set_view_mode(match projection.view {
                CalendarView::Month => CalendarMode::Month,
                CalendarView::Year => CalendarMode::Year,
            });
            root.set_can_previous(projection.can_previous);
            root.set_can_next(projection.can_next);
            root.set_weekdays(ModelRc::new(slint::VecModel::from(
                projection
                    .weekdays
                    .into_iter()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            )));
            let days = projection
                .days
                .into_iter()
                .map(|day| CalendarDayCell {
                    key: day
                        .date
                        .and_then(|date| {
                            keys.days
                                .iter()
                                .find(|(_, d)| *d == date)
                                .map(|(k, _)| k.clone())
                        })
                        .unwrap_or_default(),
                    label: day.label.into(),
                    description: day.description.into(),
                    off_month: day.off_month,
                    today: day.today,
                    selected: day.selected,
                })
                .collect::<Vec<_>>();
            root.set_weeks(ModelRc::new(slint::VecModel::from(
                days.chunks(7)
                    .map(|week| CalendarWeekRow {
                        days: ModelRc::new(slint::VecModel::from(week.to_vec())),
                    })
                    .collect::<Vec<_>>(),
            )));
            let months = if projection.view == CalendarView::Year {
                projection
                    .months
                    .into_iter()
                    .map(|month| CalendarMonthCell {
                        key: keys
                            .months
                            .iter()
                            .find(|(_, m)| *m == month.month)
                            .map(|(k, _)| k.clone())
                            .unwrap_or_default(),
                        label: month.label.into(),
                        current: month.current,
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            root.set_month_rows(ModelRc::new(slint::VecModel::from(
                months
                    .chunks(3)
                    .map(|row| CalendarMonthRow {
                        months: ModelRc::new(slint::VecModel::from(row.to_vec())),
                    })
                    .collect::<Vec<_>>(),
            )));
        } else {
            root.set_title_text("Calendar".into());
            root.set_view_mode(CalendarMode::Month);
            root.set_can_previous(false);
            root.set_can_next(false);
            root.set_weekdays(ModelRc::default());
            root.set_weeks(ModelRc::default());
            root.set_month_rows(ModelRc::default());
        }
        root.set_action_key(if self.current(session) {
            keys.action
        } else {
            SharedString::default()
        });
        drop(_guard);
        if self.policy_projection_pending.replace(false) && self.is_open() {
            self.project_and_fit();
        }
    }

    fn schedule_refresh(self: &Rc<Self>) {
        self.refresh_timer.stop();
        if !self.is_open() || !self.state.borrow().interactive() {
            return;
        }
        let session = self.state.borrow().session;
        let weak = Rc::downgrade(self);
        self.refresh_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(60),
            move || {
                if let Some(controller) = weak.upgrade()
                    && controller.current(session)
                {
                    let interactive = controller.state.borrow().interactive();
                    if interactive {
                        controller.state.borrow_mut().read_requested = true;
                        controller.pump();
                    }
                }
            },
        );
    }

    fn preferred_rect(&self) -> Result<PopupRect, String> {
        let placement = self
            .placement
            .get()
            .ok_or("Calendar placement is unavailable.")?;
        placement::place_centered(
            placement.context,
            placement.anchor,
            (
                self.component().get_popup_content_width(),
                self.component().get_popup_content_height(),
            ),
            placement.scale,
        )
    }

    fn project_and_fit(&self) {
        self.project();
        let _ = self.refit();
    }

    pub(crate) fn refit(&self) -> Result<(), String> {
        if !self.is_open() {
            return Ok(());
        }
        match self.preferred_rect() {
            Ok(rect) => {
                let changed = self.rect.borrow().as_ref() != Some(&rect);
                if changed && self.surface.reposition(rect.position, rect.size) {
                    *self.rect.borrow_mut() = Some(rect);
                }
                Ok(())
            }
            Err(error) => {
                self.hide();
                Err(error)
            }
        }
    }

    fn schedule_fit(self: &Rc<Self>) {
        if !self.is_open() || self.placement.get().is_none() || self.fit_timer.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        let session = self.state.borrow().session;
        self.fit_timer
            .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                if let Some(controller) = weak.upgrade()
                    && controller.current(session)
                {
                    let _ = controller.refit();
                }
            });
    }

    fn watch_focus(self: &Rc<Self>) {
        let focused = self.is_focused();
        self.focus_seen.set(focused == Some(true));
        if focused.is_none() || !self.is_open() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.focus_watch.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || {
                if let Some(controller) = weak.upgrade() {
                    if !controller.is_open() {
                        controller.focus_watch.stop();
                        return;
                    }
                    match controller.is_focused() {
                        Some(true) => controller.focus_seen.set(true),
                        Some(false) if controller.focus_seen.get() => controller.hide(),
                        _ => {}
                    }
                }
            },
        );
    }

    #[cfg(any(windows, test))]
    fn is_focused(&self) -> Option<bool> {
        use slint::winit_030::WinitWindowAccessor;
        self.surface
            .window()
            .with_winit_window(|window| window.has_focus())
    }

    #[cfg(not(any(windows, test)))]
    fn is_focused(&self) -> Option<bool> {
        None
    }
}

impl Drop for CalendarController {
    fn drop(&mut self) {
        self.hide();
    }
}

fn failure(kind: CalendarErrorKind) -> &'static str {
    match kind {
        CalendarErrorKind::Unsupported => "Calendar is not supported on this platform.",
        CalendarErrorKind::Unavailable => "Calendar information is unavailable.",
        CalendarErrorKind::InvalidData => "Calendar information could not be validated.",
        CalendarErrorKind::Busy => "Calendar provider is busy. Try again.",
        CalendarErrorKind::Stopped => "Calendar provider has stopped. Try again.",
        CalendarErrorKind::Other => "Calendar information could not be read.",
    }
}
