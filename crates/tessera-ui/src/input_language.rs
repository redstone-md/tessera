// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Session-bound input-profile intentions; native observations alone own selection.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, PhysicalPosition, PhysicalSize, SharedString};
use tessera_system::input_language::{
    InputLanguageAction, InputLanguageError, InputLanguageErrorKind, InputLanguageHost,
    InputLanguageOutcome, InputLanguageSnapshot, ProfileId,
};

use crate::generated::{InputLanguageMenu, InputProfileRow, PopoverMotion, TileBounds};
use crate::popup_placement::{self as placement, PopupRect};
use crate::sanitize::bounded_text;
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::transient_window::{TransientComponent, TransientWindow};
use crate::{DesktopHost, DockContext, SurfaceKind};

mod mailbox;
mod state;
#[cfg(test)]
mod tests;
use mailbox::{Mailbox, Outcome, complete};
use state::{Keys, Operation, State, Token};

impl TransientComponent for InputLanguageMenu {
    fn motion(&self) -> PopoverMotion<'_> {
        self.global::<PopoverMotion>()
    }
    fn set_presentation_opacity(&self, opacity: f32) {
        self.invoke_set_presentation_opacity(opacity);
    }
}

#[derive(Clone, Copy)]
struct Placement {
    anchor: PhysicalPosition,
    context: DockContext,
    scale: f32,
}

struct Guard<'a>(&'a Cell<bool>);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub(crate) struct InputLanguageController {
    surface: TransientWindow<InputLanguageMenu>,
    host: Arc<dyn DesktopHost>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
    projecting: Cell<bool>,
    presenting: Cell<bool>,
    placement: Cell<Option<Placement>>,
    rect: RefCell<Option<PopupRect>>,
    fit_timer: slint::Timer,
    focus_watch: slint::Timer,
    refresh_timer: slint::Timer,
    focus_seen: Cell<bool>,
}

impl InputLanguageController {
    pub(crate) fn new(host: Arc<dyn DesktopHost>) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            surface: TransientWindow::new(
                host.clone(),
                InputLanguageMenu::new()?,
                SurfaceKind::Popup,
            ),
            host,
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            projecting: Cell::new(false),
            presenting: Cell::new(false),
            placement: Cell::new(None),
            rect: RefCell::default(),
            fit_timer: slint::Timer::default(),
            focus_watch: slint::Timer::default(),
            refresh_timer: slint::Timer::default(),
            focus_seen: Cell::new(false),
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_input_event_ready(move || {
            if let Some(controller) = weak.upgrade() {
                controller.drain();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_select_profile(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.select_profile(key);
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_open_settings(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.open_settings(key);
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

    pub(crate) fn component(&self) -> &InputLanguageMenu {
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
        if self.state.borrow().closed {
            return Err("Keyboard selector has closed.".into());
        }
        if self.presenting.get() {
            return Err("Keyboard selector presentation is already in progress.".into());
        }
        let expected_session = self.state.borrow().session.wrapping_add(1);
        self.hide();
        if self.state.borrow().closed || self.state.borrow().session != expected_session {
            return Ok(());
        }
        if !source.is_visible() || context.fullscreen_active() {
            return Err("Keyboard selector needs a visible source.".into());
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
        self.drain();
        focus.map_err(|error| {
            bounded_text(
                &format!("Keyboard selector opened, but keyboard focus was not granted: {error}"),
                240,
            )
        })
    }

    pub(crate) fn hide(&self) {
        self.fit_timer.stop();
        self.focus_watch.stop();
        self.refresh_timer.stop();
        self.focus_seen.set(false);
        self.component().invoke_cancel_input();
        {
            let mut state = self.state.borrow_mut();
            state.session = state.session.wrapping_add(1);
            state.read_requested = false;
            state.pending_action = None;
            state.snapshot = None;
            state.keys = Keys::default();
            state.notice.clear();
            state.action_notice.clear();
            // Accepted native work keeps its slot until its original completion retires.
        }
        self.placement.set(None);
        self.rect.borrow_mut().take();
        self.surface.set_settings_key(SharedString::default());
        self.surface.set_rows(ModelRc::default());
        self.surface.set_retry_enabled(false);
        self.surface.hide();
    }

    /// Terminal close revokes mailbox authority without joining accepted native work.
    pub(crate) fn close(&self) {
        self.state.borrow_mut().closed = true;
        *self.mailbox.lock() = Mailbox::default();
        self.hide();
        self.state.borrow_mut().flight = None;
    }

    pub(crate) fn apply_theme(&self, theme: PresentationTheme) {
        self.component().apply_presentation_theme(theme);
        let _ = self.refit();
    }
    pub(crate) fn disable_motion(&self) {
        self.surface.disable_motion();
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
        self.is_open() && !self.state.borrow().closed && self.state.borrow().session == session
    }
    fn accepts_input(&self) -> bool {
        self.is_open()
            && !self.projecting.get()
            && !self.presenting.get()
            && !self.state.borrow().closed
    }

    fn select_profile(self: &Rc<Self>, key: SharedString) {
        if !self.accepts_input() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            let Some(profile) = state.profile(&key) else {
                return;
            };
            state.pending_action = Some(InputLanguageAction::Activate { profile });
            state.keys = Keys::default();
            state.action_notice.clear();
        }
        self.pump();
    }

    fn open_settings(self: &Rc<Self>, key: SharedString) {
        if !self.accepts_input() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if !state.accepts_settings(&key) {
                return;
            }
            // Settings remains usable during a read, but waits behind that accepted
            // read rather than overlapping native calls or cancelling its completion.
            state.pending_action = Some(InputLanguageAction::OpenKeyboardSettings);
            state.read_requested = false;
            state.keys = Keys::default();
            state.action_notice.clear();
        }
        self.project_and_fit();
        self.pump();
    }

    fn retry(self: &Rc<Self>) {
        if !self.accepts_input() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if !state.idle() || state.notice.is_empty() && state.action_notice.is_empty() {
                return;
            }
            state.notice.clear();
            state.action_notice.clear();
            state.read_requested = true;
        }
        self.pump();
    }

    /// No state, mailbox or lease borrow crosses a provider factory or provider call.
    fn pump(self: &Rc<Self>) {
        if !self.is_open() || self.state.borrow().closed {
            return;
        }
        let acquire = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || state.acquiring {
                return;
            }
            if !state.read_requested && state.pending_action.is_none() {
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
            let result = self.host.input_language_host();
            {
                let mut state = self.state.borrow_mut();
                state.acquiring = false;
                if !state.closed {
                    match result {
                        Ok(Some(provider)) => state.provider = Some(provider),
                        result if state.session == session => {
                            state.read_requested = false;
                            state.pending_action = None;
                            state.notice = match result {
                                Err(error) => failure(error.kind).into(),
                                Ok(None) => failure(InputLanguageErrorKind::Unsupported).into(),
                                Ok(Some(_)) => unreachable!(),
                            };
                        }
                        _ => {}
                    }
                }
            }
            self.project_and_fit();
            if !self.current(session) {
                self.pump();
                return;
            }
        }
        let (provider, token, action) = {
            let mut state = self.state.borrow_mut();
            if state.closed || state.flight.is_some() || state.acquiring {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let action = state.pending_action.take();
            let operation = match &action {
                Some(InputLanguageAction::Activate { .. }) => Operation::Activate,
                Some(InputLanguageAction::OpenKeyboardSettings) => Operation::Settings,
                None if state.read_requested => Operation::Read,
                None => return,
            };
            state.read_requested = false;
            let token = state.begin(operation);
            (provider, token, action)
        };
        self.refresh_timer.stop();
        self.mailbox.lock().expected = Some(token);
        self.project_and_fit();
        if !self.current(token.session) {
            let mut state = self.state.borrow_mut();
            if state
                .flight
                .as_ref()
                .is_some_and(|flight| flight.token == token)
            {
                state.flight = None;
            }
            drop(state);
            let mut mailbox = self.mailbox.lock();
            if mailbox.expected == Some(token) {
                mailbox.expected = None;
            }
            drop(mailbox);
            self.pump();
            return;
        }
        let mailbox = self.mailbox.clone();
        let root = self.surface.as_weak();
        match action {
            Some(action) => {
                if let Err(error) = provider.execute(
                    action,
                    Box::new(move |result| {
                        complete(&mailbox, &root, token, Outcome::Action(result));
                    }),
                ) {
                    complete(
                        &self.mailbox,
                        &self.surface.as_weak(),
                        token,
                        Outcome::Action(Err(error)),
                    );
                }
            }
            None => {
                if let Err(error) = provider.read(Box::new(move |result| {
                    complete(&mailbox, &root, token, Outcome::Read(result));
                })) {
                    complete(
                        &self.mailbox,
                        &self.surface.as_weak(),
                        token,
                        Outcome::Read(Err(error)),
                    );
                }
            }
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
                && state
                    .flight
                    .as_ref()
                    .is_some_and(|flight| flight.token == completion.token)
            {
                let flight = state
                    .flight
                    .take()
                    .expect("matched accepted input operation");
                if visible && !state.closed && state.session == completion.token.session {
                    match completion.outcome {
                        Outcome::Read(Ok(snapshot)) => {
                            state.snapshot = Some(snapshot);
                            state.notice.clear();
                        }
                        Outcome::Read(Err(error)) => {
                            state.snapshot = None;
                            state.notice = failure(error.kind).into();
                        }
                        Outcome::Action(result) => {
                            // Even failure may follow a partial native language change.
                            // Always observe again; never replay an activation automatically.
                            state.read_requested = true;
                            state.keys = Keys::default();
                            match result {
                                Ok(InputLanguageOutcome::Snapshot(snapshot)) => {
                                    state.snapshot = Some(snapshot);
                                    state.notice.clear();
                                }
                                Ok(InputLanguageOutcome::SettingsDispatched) => {
                                    state.action_notice =
                                        "Keyboard settings launch accepted.".into();
                                }
                                Err(error) => {
                                    state.snapshot = None;
                                    state.action_notice = format!(
                                        "{} {}",
                                        if flight.operation == Operation::Settings {
                                            "Keyboard settings could not be opened."
                                        } else {
                                            "Keyboard profile activation could not be confirmed."
                                        },
                                        failure(error.kind)
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        if visible && !self.state.borrow().closed {
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
        let visible = self.is_open();
        let (
            session,
            rows,
            settings,
            loading,
            action_pending,
            observed,
            unknown_active,
            notice,
            retry,
        ) = {
            let mut state = self.state.borrow_mut();
            let snapshot = state.snapshot.clone();
            let loading = state.loading();
            let action_pending = state.action_pending();
            let rows_interactive = visible && state.idle();
            let settings_interactive = visible
                && !state.closed
                && !state.acquiring
                && !action_pending
                && state.provider.is_some();
            let mut keys = Keys::default();
            let mut rows = Vec::new();
            let mut has_active = false;
            if let Some(snapshot) = &snapshot {
                for language in &snapshot.languages {
                    for profile in &language.profiles {
                        has_active |= profile.active;
                        let key = if rows_interactive {
                            let key = state.key();
                            keys.profiles.push((key.clone(), profile.id.clone()));
                            key
                        } else {
                            SharedString::default()
                        };
                        rows.push(InputProfileRow {
                            key,
                            language_name: language.name.clone().into(),
                            layout_name: profile.display_name.clone().into(),
                            active: profile.active,
                        });
                    }
                }
            }
            if settings_interactive {
                keys.settings = state.key();
            }
            let settings = keys.settings.clone();
            state.keys = keys;
            let notice = if state.action_notice.is_empty() {
                state.notice.clone()
            } else if state.notice.is_empty() {
                state.action_notice.clone()
            } else {
                format!("{} {}", state.action_notice, state.notice)
            };
            let retry = state.idle()
                && (!state.notice.is_empty()
                    || !state.action_notice.is_empty() && state.snapshot.is_none());
            let unknown_active = snapshot.is_some() && !rows.is_empty() && !has_active;
            (
                state.session,
                rows,
                settings,
                loading,
                action_pending,
                snapshot.is_some(),
                unknown_active,
                notice,
                retry,
            )
        };
        let root = self.component();
        root.set_settings_key(SharedString::default());
        root.set_loading(loading);
        root.set_action_pending(action_pending);
        root.set_observed(observed);
        root.set_unknown_active(unknown_active);
        root.set_notice(notice.into());
        root.set_retry_enabled(retry);
        root.set_rows(ModelRc::new(slint::VecModel::from(rows)));
        root.set_settings_key(if self.current(session) {
            settings
        } else {
            SharedString::default()
        });
    }

    fn schedule_refresh(self: &Rc<Self>) {
        self.refresh_timer.stop();
        if !self.is_open() || !self.state.borrow().idle() || self.state.borrow().provider.is_none()
        {
            return;
        }
        let session = self.state.borrow().session;
        let weak = Rc::downgrade(self);
        self.refresh_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(1),
            move || {
                if let Some(controller) = weak.upgrade()
                    && controller.current(session)
                    && controller.state.borrow().idle()
                {
                    controller.state.borrow_mut().read_requested = true;
                    controller.pump();
                }
            },
        );
    }

    fn preferred_rect(&self) -> Result<PopupRect, String> {
        let p = self
            .placement
            .get()
            .ok_or("Keyboard selector placement is unavailable.")?;
        placement::place_centered(
            p.context,
            p.anchor,
            (
                self.surface.get_popup_content_width(),
                self.surface.get_popup_content_height(),
            ),
            p.scale,
        )
    }
    fn project_and_fit(&self) {
        self.project();
        let _ = self.refit();
    }
    fn refit(&self) -> Result<(), String> {
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
        let session = self.state.borrow().session;
        let weak = Rc::downgrade(self);
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

impl Drop for InputLanguageController {
    fn drop(&mut self) {
        self.close();
    }
}

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
        return Err("Keyboard selector needs valid native geometry and input bounds.".into());
    }
    let bottom = i32::try_from(i64::from(origin.y) + i64::from(size.height))
        .map_err(|_| "Keyboard selector source bottom is outside native coordinates.")?;
    placement::physical_anchor(
        PhysicalPosition::new(origin.x, bottom),
        scale,
        (bounds.origin.x + bounds.width / 2.0, -10.0),
    )
    .map_err(str::to_owned)
}

fn failure(kind: InputLanguageErrorKind) -> &'static str {
    match kind {
        InputLanguageErrorKind::Unsupported => {
            "Keyboard profiles are not supported on this platform."
        }
        InputLanguageErrorKind::AccessDenied => "Access to keyboard profiles was denied.",
        InputLanguageErrorKind::Busy => "Keyboard profile provider is busy. Try again.",
        InputLanguageErrorKind::Stopped => "Keyboard profile provider has stopped. Try again.",
        InputLanguageErrorKind::ProfileChanged => {
            "The keyboard profile changed or is no longer enabled."
        }
        InputLanguageErrorKind::Unavailable => "Keyboard profile information is unavailable.",
        InputLanguageErrorKind::InvalidValue => {
            "Keyboard profile information could not be validated."
        }
        InputLanguageErrorKind::Other => "Keyboard profile information could not be read.",
    }
}
