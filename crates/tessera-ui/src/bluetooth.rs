// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Passive paired observations and explicit owner-issued Bluetooth radio power.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use slint::{ComponentHandle, Model, ModelRc, PhysicalPosition, PhysicalSize};
use tessera_system::bluetooth::{
    BluetoothControlSnapshot, BluetoothControlledRadio, BluetoothError, BluetoothErrorKind,
    BluetoothHost, BluetoothRadioAccess, BluetoothRadioCommand, BluetoothRadioOutcome,
    BluetoothRadioPower, BluetoothRadioState, BluetoothSnapshot,
};

use crate::generated::{
    BluetoothMenu, BluetoothRadioControlRow, BluetoothRadioStatus, PopoverMotion, TileBounds,
};
use crate::popup_placement::{self as placement, PopupRect};
use crate::sanitize::bounded_text;
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::transient_window::{TransientComponent, TransientWindow};
use crate::{DesktopHost, DockContext, SurfaceKind};

mod presentation;
use presentation::{Projection, failure};

#[cfg(test)]
mod tests;

impl TransientComponent for BluetoothMenu {
    fn motion(&self) -> PopoverMotion<'_> {
        self.global::<PopoverMotion>()
    }

    fn set_presentation_opacity(&self, opacity: f32) {
        self.invoke_set_presentation_opacity(opacity);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReadToken {
    session: u64,
    sequence: u64,
}

#[derive(Default)]
struct State {
    session: u64,
    sequence: u64,
    provider: Option<Arc<dyn BluetoothHost>>,
    read_requested: bool,
    flight: Option<ReadToken>,
    snapshot: Option<BluetoothSnapshot>,
    notice: String,
    controls: Vec<BluetoothControlledRadio>,
    command_pending: bool,
}

impl State {
    fn loading(&self) -> bool {
        self.read_requested
            || self
                .flight
                .is_some_and(|token| token.session == self.session && !self.command_pending)
    }

    fn next_token(&mut self) -> Option<ReadToken> {
        self.sequence = self.sequence.checked_add(1)?;
        Some(ReadToken {
            session: self.session,
            sequence: self.sequence,
        })
    }
}

enum EventResult {
    Read(Result<BluetoothControlSnapshot, BluetoothError>),
    Radio {
        radio: BluetoothControlledRadio,
        power: BluetoothRadioPower,
        result: Result<BluetoothRadioOutcome, BluetoothError>,
    },
}

struct Completion {
    token: ReadToken,
    result: EventResult,
}

/// One accepted flight survives close/reopen. Even an inline native callback
/// enters this bounded Send mailbox and posts to the event loop, never to Rust UI.
#[derive(Default)]
struct Mailbox {
    expected: Option<ReadToken>,
    completion: Option<Completion>,
    wake_queued: bool,
}

fn complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<BluetoothMenu>,
    token: ReadToken,
    result: EventResult,
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
        .upgrade_in_event_loop(|root| root.invoke_bluetooth_event_ready())
        .is_err()
    {
        // Headless recording tests deliver this same production wake manually.
        // Never fall back to executing a completion on the submitting thread.
        mailbox.lock().wake_queued = false;
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

pub(crate) struct BluetoothController {
    surface: TransientWindow<BluetoothMenu>,
    host: Arc<dyn DesktopHost>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
    projecting: Cell<bool>,
    presenting: Cell<bool>,
    admitting: Cell<bool>,
    placement: Cell<Option<Placement>>,
    rect: RefCell<Option<PopupRect>>,
    fit_timer: slint::Timer,
    drain_timer: slint::Timer,
    focus_watch: slint::Timer,
    focus_seen: Cell<bool>,
    control_generation: Cell<u64>,
    control_frame: RefCell<String>,
    control_model: RefCell<ModelRc<BluetoothRadioControlRow>>,
    control_rows: RefCell<Vec<BluetoothRadioControlRow>>,
    control_scale: Cell<f32>,
}

impl BluetoothController {
    pub(crate) fn new(host: Arc<dyn DesktopHost>) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            surface: TransientWindow::new(host.clone(), BluetoothMenu::new()?, SurfaceKind::Popup),
            host,
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            projecting: Cell::new(false),
            presenting: Cell::new(false),
            admitting: Cell::new(false),
            placement: Cell::new(None),
            rect: RefCell::default(),
            fit_timer: slint::Timer::default(),
            drain_timer: slint::Timer::default(),
            focus_watch: slint::Timer::default(),
            focus_seen: Cell::new(false),
            control_generation: Cell::new(0),
            control_frame: RefCell::default(),
            control_model: RefCell::default(),
            control_rows: RefCell::default(),
            control_scale: Cell::new(1.0),
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_bluetooth_event_ready(move || {
            if let Some(controller) = weak.upgrade() {
                controller.drain();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_refresh_requested(move || {
            if let Some(controller) = weak.upgrade() {
                controller.refresh();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller
            .surface
            .on_radio_power_requested(move |frame, index, on| {
                if let Some(controller) = weak.upgrade() {
                    controller.set_radio(frame.as_str(), index, on);
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

    pub(crate) fn component(&self) -> &BluetoothMenu {
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
            return Err("Bluetooth presentation is already in progress.".into());
        }
        self.hide();
        if !source.is_visible() || context.fullscreen_active() {
            return Err("Bluetooth needs a visible source and usable monitor geometry.".into());
        }
        if self.state.borrow().session == u64::MAX {
            return Err("Bluetooth presentation counter exhausted; restart required.".into());
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
        // A generated property callback may have retired this session already.
        if self.state.borrow().session != session {
            return Ok(());
        }
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
                return Err(bounded_text(&error, 240));
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
        // Native visibility was accepted even if foreground focus was denied.
        // Keep its lease and observation alive so root routing can coordinate
        // the actual visible popup while still reporting focus denial honestly.
        focus.map_err(|error| {
            bounded_text(
                &format!("Bluetooth keyboard focus was not granted: {error}"),
                240,
            )
        })
    }

    pub(crate) fn hide(&self) {
        self.fit_timer.stop();
        self.drain_timer.stop();
        self.focus_watch.stop();
        self.focus_seen.set(false);
        {
            let mut state = self.state.borrow_mut();
            state.session = state.session.saturating_add(1);
            state.read_requested = false;
            state.snapshot = None;
            state.controls.clear();
            state.notice.clear();
            // No cancellation or joining. Only the accepted completion can
            // retire flight, even after a subsequent session has opened.
        }
        self.placement.set(None);
        self.rect.borrow_mut().take();
        self.surface.set_refresh_enabled(false);
        self.control_frame.borrow_mut().clear();
        self.surface.hide();
    }

    pub(crate) fn disable_motion(&self) {
        self.surface.disable_motion();
    }

    pub(crate) fn apply_theme(&self, theme: PresentationTheme) {
        self.component().apply_presentation_theme(theme);
        let _ = self.refit();
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
        session != u64::MAX && self.is_open() && self.state.borrow().session == session
    }

    fn refresh(self: &Rc<Self>) {
        if !self.is_open() || self.projecting.get() || self.presenting.get() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            // A bool coalesces every press into one fresh observation, not a queue.
            state.read_requested = true;
            state.notice.clear();
            state.controls.clear();
        }
        self.project_and_fit();
        self.pump();
    }

    fn set_radio(self: &Rc<Self>, frame: &str, index: i32, on: bool) {
        if !self.is_open()
            || self.projecting.get()
            || self.presenting.get()
            || self.admitting.get()
            || frame != self.control_frame.borrow().as_str()
            || !self.controls_current()
            || self.component().window().scale_factor() != self.control_scale.get()
        {
            return;
        }
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        let issuer_scale = self.control_scale.get();
        let (provider, issuer_session) = {
            let state = self.state.borrow();
            if state.flight.is_some() || state.read_requested {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            (provider, state.session)
        };
        self.admitting.set(true);
        let _guard = Guard(&self.admitting);
        // Capability access can reenter presentation. Never hold UI state
        // across host calls, and retain this exact capability for submission.
        let Some(controls) = provider.radio_controls() else {
            return;
        };
        if !self.current(issuer_session)
            || frame != self.control_frame.borrow().as_str()
            || !self.controls_current()
            || self.component().window().scale_factor() != issuer_scale
        {
            return;
        }
        let (radio, token) = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some()
                || state.read_requested
                || !state
                    .provider
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &provider))
            {
                return;
            }
            let Some(radio) = state.controls.get(index).cloned() else {
                return;
            };
            let requested = if on {
                BluetoothRadioState::On
            } else {
                BluetoothRadioState::Off
            };
            if radio.observation.state == requested
                || !matches!(
                    radio.observation.state,
                    BluetoothRadioState::On | BluetoothRadioState::Off
                )
            {
                return;
            }
            let Some(token) = state.next_token() else {
                return;
            };
            state.flight = Some(token);
            state.command_pending = true;
            state.notice.clear();
            (radio, token)
        };
        self.mailbox.lock().expected = Some(token);
        self.project_and_fit();
        // Generated model/geometry callbacks can retire the reserved source.
        // Consent has not started yet: abandon this unsubmitted intent only.
        if !self.current(token.session)
            || !self.controls_current()
            || self.component().window().scale_factor() != issuer_scale
            || self.state.borrow().controls.get(index) != Some(&radio)
        {
            let mut state = self.state.borrow_mut();
            state.flight = None;
            state.command_pending = false;
            self.mailbox.lock().expected = None;
            drop(state);
            self.schedule_drain();
            return;
        }
        let power = if on {
            BluetoothRadioPower::On
        } else {
            BluetoothRadioPower::Off
        };
        let command = BluetoothRadioCommand {
            radio: radio.clone(),
            power,
        };
        let mailbox = self.mailbox.clone();
        let root = self.surface.as_weak();
        let captured = radio.clone();
        // Consent initiation stays on the GUI; only native awaits use the owner.
        let admission = controls.set_radio(
            command,
            Box::new(move |result| {
                complete(
                    &mailbox,
                    &root,
                    token,
                    EventResult::Radio {
                        radio: captured,
                        power,
                        result,
                    },
                );
            }),
        );
        if let Err(error) = admission {
            complete(
                &self.mailbox,
                &self.surface.as_weak(),
                token,
                EventResult::Radio {
                    radio,
                    power,
                    result: Err(error),
                },
            );
        }
    }

    fn controls_current(&self) -> bool {
        if self.control_generation.get() == u64::MAX {
            return false;
        }
        let model = self.component().get_radio_controls();
        let rows = self.control_rows.borrow();
        model == *self.control_model.borrow()
            && model.row_count() == rows.len()
            && rows
                .iter()
                .enumerate()
                .all(|(index, row)| model.row_data(index).as_ref() == Some(row))
    }

    fn pump(self: &Rc<Self>) {
        if !self.is_open() || self.admitting.get() {
            return;
        }
        let acquire = {
            let state = self.state.borrow();
            if state.flight.is_some() || !state.read_requested {
                return;
            }
            state.provider.is_none().then_some(state.session)
        };
        self.admitting.set(true);
        let _guard = Guard(&self.admitting);
        if let Some(session) = acquire {
            // The getter is a cheap lazy capability lookup. SDK work is host.read.
            let result = self.host.bluetooth_host();
            {
                let mut state = self.state.borrow_mut();
                match result {
                    Ok(Some(provider)) => state.provider = Some(provider),
                    result if state.session == session => {
                        state.read_requested = false;
                        state.notice = match result {
                            Err(error) => failure("Bluetooth", &error),
                            Ok(None) => failure(
                                "Bluetooth",
                                &BluetoothError::new(BluetoothErrorKind::Unsupported, ""),
                            ),
                            Ok(Some(_)) => unreachable!(),
                        };
                    }
                    _ => {}
                }
            }
            self.project_and_fit();
            if !self.current(session) {
                self.schedule_drain();
                return;
            }
        }
        let submission = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || !state.read_requested {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            state.read_requested = false;
            state.next_token().map(|token| {
                state.flight = Some(token);
                (provider, token)
            })
        };
        let Some((provider, token)) = submission else {
            self.state.borrow_mut().notice =
                "Bluetooth request counter exhausted; restart required.".into();
            self.project_and_fit();
            return;
        };
        self.mailbox.lock().expected = Some(token);
        self.project_and_fit();
        if !self.current(token.session) {
            // Not accepted yet; no worker owns this reserved slot.
            self.state.borrow_mut().flight = None;
            self.mailbox.lock().expected = None;
            self.schedule_drain();
            return;
        }
        let mailbox = self.mailbox.clone();
        let root = self.surface.as_weak();
        let controls = provider.radio_controls();
        if !self.current(token.session)
            || !self
                .state
                .borrow()
                .provider
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &provider))
        {
            self.state.borrow_mut().flight = None;
            self.mailbox.lock().expected = None;
            self.schedule_drain();
            return;
        }
        let admission = if let Some(controls) = controls {
            controls.read_controls(Box::new(move |result| {
                complete(&mailbox, &root, token, EventResult::Read(result));
            }))
        } else {
            provider.read(Box::new(move |result| {
                complete(
                    &mailbox,
                    &root,
                    token,
                    EventResult::Read(result.map(|snapshot| BluetoothControlSnapshot {
                        snapshot,
                        radios: Ok(Vec::new()),
                    })),
                );
            }))
        };
        if let Err(error) = admission {
            complete(
                &self.mailbox,
                &self.surface.as_weak(),
                token,
                EventResult::Read(Err(error)),
            );
        }
        // No inline drain: even immediate rejection is event-loop delivered.
    }

    fn drain(self: &Rc<Self>) {
        if self.projecting.get() || self.presenting.get() || self.admitting.get() {
            self.schedule_drain();
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
                state.command_pending = false;
                if visible && state.session == completion.token.session {
                    match completion.result {
                        EventResult::Read(Ok(observation)) => {
                            state.snapshot = Some(observation.snapshot);
                            match observation.radios {
                                Ok(radios) => {
                                    state.controls = radios;
                                    state.notice.clear();
                                }
                                Err(error) => {
                                    state.controls.clear();
                                    state.notice = failure("Radio control", &error);
                                }
                            }
                        }
                        EventResult::Read(Err(error)) => {
                            state.snapshot = None;
                            state.controls.clear();
                            state.notice = failure("Bluetooth", &error);
                        }
                        EventResult::Radio {
                            radio,
                            power,
                            result,
                        } => {
                            state.notice = radio_feedback(power, &result);
                            let radio_index = state
                                .controls
                                .iter()
                                .position(|source| source.key == radio.key);
                            // Preserve Classic/LE observations. Readback only
                            // updates this exact command's retained radio row.
                            if let Ok(outcome) = result
                                && let Ok(observed) = outcome.observed
                                && let Some(snapshot) = state.snapshot.as_mut()
                                && let Ok(radios) = snapshot.radios.as_mut()
                                && let Some(index) = radio_index
                                && let Some(row) = radios.get_mut(index)
                            {
                                *row = observed;
                            }
                            state.controls.clear();
                        }
                    }
                }
            }
        }
        if visible {
            self.project_and_fit();
            self.pump();
        }
    }

    fn project(&self) {
        if self.projecting.replace(true) {
            return;
        }
        let _guard = Guard(&self.projecting);
        let (session, projection, loading, has_snapshot) = {
            let state = self.state.borrow();
            let mut projection = state
                .snapshot
                .as_ref()
                .map(Projection::from_snapshot)
                .unwrap_or_default();
            if !state.notice.is_empty() {
                projection.notice = if projection.notice.is_empty() {
                    state.notice.clone()
                } else {
                    format!("{} · {}", projection.notice, state.notice)
                };
            }
            (
                state.session,
                projection,
                state.loading(),
                state.snapshot.is_some(),
            )
        };
        self.control_generation
            .set(self.control_generation.get().saturating_add(1));
        let frame = format!("{session}:{}", self.control_generation.get());
        let (control_rows, command_pending) = {
            let state = self.state.borrow();
            let enabled = self.is_open()
                && state.flight.is_none()
                && !state.read_requested
                && state.session != u64::MAX
                && state.sequence != u64::MAX
                && self.control_generation.get() != u64::MAX;
            let rows = state
                .controls
                .iter()
                .enumerate()
                .map(|(index, radio)| BluetoothRadioControlRow {
                    name: bounded_text(&radio.observation.name, 128).into(),
                    state: radio_status(radio.observation.state),
                    frame: frame.clone().into(),
                    index: i32::try_from(index).unwrap_or(-1),
                    enabled: enabled
                        && matches!(
                            radio.observation.state,
                            BluetoothRadioState::On | BluetoothRadioState::Off
                        ),
                })
                .collect::<Vec<_>>();
            (
                rows,
                state.command_pending && state.flight.is_some_and(|token| token.session == session),
            )
        };
        *self.control_rows.borrow_mut() = control_rows.clone();
        let control_model = ModelRc::new(slint::VecModel::from(control_rows));
        *self.control_frame.borrow_mut() = frame;
        *self.control_model.borrow_mut() = control_model.clone();
        self.control_scale
            .set(self.component().window().scale_factor());
        let root = self.component();
        // A property callback can hide/reopen. Do not finish projecting the old
        // session over its replacement; Rust input is gated during projection.
        macro_rules! set {
            ($method:ident, $value:expr) => {
                if self.state.borrow().session != session {
                    return;
                }
                root.$method($value);
            };
        }
        set!(set_loading, loading);
        set!(set_has_snapshot, has_snapshot);
        set!(set_radio_command_pending, command_pending);
        set!(set_radio_controls, control_model);
        set!(
            set_radios,
            ModelRc::new(slint::VecModel::from(projection.radios))
        );
        set!(
            set_connected,
            ModelRc::new(slint::VecModel::from(projection.connected))
        );
        set!(
            set_paired,
            ModelRc::new(slint::VecModel::from(projection.paired))
        );
        set!(
            set_unknown,
            ModelRc::new(slint::VecModel::from(projection.unknown))
        );
        set!(set_no_radios, projection.no_radios);
        set!(set_empty_inventory, projection.empty_inventory);
        set!(set_partial_inventory, projection.partial_inventory);
        set!(set_notice, projection.notice.into());
        set!(set_refresh_enabled, self.is_open());
    }

    fn preferred_rect(&self) -> Result<PopupRect, String> {
        let placement = self
            .placement
            .get()
            .ok_or("Bluetooth placement is unavailable.")?;
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
        if !self.is_open() || self.fit_timer.running() {
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

    fn schedule_drain(self: &Rc<Self>) {
        if self.drain_timer.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.drain_timer
            .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                if let Some(controller) = weak.upgrade() {
                    controller.drain();
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
                    controller.focus_observed(controller.is_focused());
                }
            },
        );
    }

    fn focus_observed(&self, focused: Option<bool>) {
        match focused {
            Some(true) => self.focus_seen.set(true),
            Some(false) if self.focus_seen.get() => self.hide(),
            _ => {}
        }
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

impl Drop for BluetoothController {
    fn drop(&mut self) {
        self.hide();
    }
}

fn radio_status(state: BluetoothRadioState) -> BluetoothRadioStatus {
    match state {
        BluetoothRadioState::On => BluetoothRadioStatus::On,
        BluetoothRadioState::Off => BluetoothRadioStatus::Off,
        BluetoothRadioState::Disabled => BluetoothRadioStatus::Disabled,
        BluetoothRadioState::Unknown => BluetoothRadioStatus::Unknown,
    }
}

fn radio_feedback(
    power: BluetoothRadioPower,
    result: &Result<BluetoothRadioOutcome, BluetoothError>,
) -> String {
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => return failure("Radio power", error),
    };
    match outcome.access {
        BluetoothRadioAccess::DeniedByUser => {
            return "Radio power: access denied by the user.".into();
        }
        BluetoothRadioAccess::DeniedBySystem => {
            return "Radio power: access denied by the system or policy.".into();
        }
        BluetoothRadioAccess::Unspecified => {
            return "Radio power: permission is unavailable; no confirmed power change.".into();
        }
        BluetoothRadioAccess::Allowed => {}
    }
    match &outcome.observed {
        Ok(observed) => {
            let confirmed = matches!(
                (power, observed.state),
                (BluetoothRadioPower::On, BluetoothRadioState::On)
                    | (BluetoothRadioPower::Off, BluetoothRadioState::Off)
            );
            let state = match observed.state {
                BluetoothRadioState::On => "On",
                BluetoothRadioState::Off => "Off",
                BluetoothRadioState::Disabled => "Disabled",
                BluetoothRadioState::Unknown => "Unknown",
            };
            if confirmed {
                format!("Radio power: native state confirmed {state}.")
            } else {
                format!(
                    "Radio power request accepted; native state is {state}. The requested state is not confirmed. Refresh to read it again."
                )
            }
        }
        Err(error) => format!(
            "Radio power request accepted; {}",
            failure("Native readback", error)
        ),
    }
}

// Native toolbar geometry owns Y; the trigger's genuine bounds own only X.
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
        return Err("Bluetooth needs positive native geometry and valid trigger bounds.".into());
    }
    let bottom = i32::try_from(i64::from(origin.y) + i64::from(size.height))
        .map_err(|_| "Bluetooth source bottom is outside native coordinates.")?;
    placement::physical_anchor(
        PhysicalPosition::new(origin.x, bottom),
        scale,
        (bounds.origin.x + bounds.width / 2.0, -10.0),
    )
    .map_err(str::to_owned)
}
