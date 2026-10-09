// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Read-only paired Bluetooth observations in one independently leased popup.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, PhysicalPosition, PhysicalSize};
use tessera_system::bluetooth::{
    BluetoothError, BluetoothErrorKind, BluetoothHost, BluetoothSnapshot,
};

use crate::generated::{BluetoothMenu, PopoverMotion, TileBounds};
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
}

impl State {
    fn loading(&self) -> bool {
        self.read_requested
            || self
                .flight
                .is_some_and(|token| token.session == self.session)
    }
}

struct Completion {
    token: ReadToken,
    result: Result<BluetoothSnapshot, BluetoothError>,
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
    result: Result<BluetoothSnapshot, BluetoothError>,
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
            state.session = state.session.wrapping_add(1);
            state.read_requested = false;
            state.snapshot = None;
            state.notice.clear();
            // No cancellation or joining. Only the accepted completion can
            // retire flight, even after a subsequent session has opened.
        }
        self.placement.set(None);
        self.rect.borrow_mut().take();
        self.surface.set_refresh_enabled(false);
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
        self.is_open() && self.state.borrow().session == session
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
        }
        self.project_and_fit();
        self.pump();
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
        if let Err(error) = provider.read(Box::new(move |result| {
            complete(&mailbox, &root, token, result)
        })) {
            complete(&self.mailbox, &self.surface.as_weak(), token, Err(error));
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
                if visible && state.session == completion.token.session {
                    match completion.result {
                        Ok(snapshot) => {
                            state.snapshot = Some(snapshot);
                            state.notice.clear();
                        }
                        Err(error) => {
                            state.snapshot = None;
                            state.notice = failure("Bluetooth", &error);
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
                projection.notice = state.notice.clone();
            }
            (
                state.session,
                projection,
                state.loading(),
                state.snapshot.is_some(),
            )
        };
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
