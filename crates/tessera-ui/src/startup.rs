// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Immediate current-user startup registration, independent of preference drafts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::ComponentHandle;
use tessera_system::startup::{
    StartupCompletion, StartupError, StartupHost, StartupRegistration, StartupSnapshot,
    StartupTarget,
};

use crate::DesktopHost;
use crate::application_menu::WindowFrame;
use crate::generated::Panel;

#[derive(Clone, Copy, PartialEq)]
struct Session {
    id: u64,
    frame: WindowFrame,
}

#[derive(Clone, Copy)]
struct Flight {
    ticket: u64,
    session: Session,
    writing: bool,
}

struct Receipt {
    ticket: u64,
    result: Result<StartupSnapshot, StartupError>,
}

#[derive(Default)]
struct State {
    sequence: u64,
    exhausted: bool,
    provider_checked: bool,
    provider: Option<Arc<dyn StartupHost>>,
    session: Option<Session>,
    read_started: bool,
    snapshot: Option<StartupSnapshot>,
    error: Option<StartupError>,
    flight: Option<Flight>,
}

impl State {
    fn next(&mut self) -> Option<u64> {
        let Some(next) = self.sequence.checked_add(1) else {
            self.exhausted = true;
            return None;
        };
        self.sequence = next;
        Some(next)
    }
}

struct ResetFlag<'a>(&'a Cell<bool>);
impl Drop for ResetFlag<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub(crate) struct StartupController {
    host: Arc<dyn DesktopHost>,
    panel: slint::Weak<Panel>,
    admission: Rc<dyn Fn() -> bool>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Option<Receipt>>>,
    projecting: Cell<bool>,
    acquiring: Cell<bool>,
}

impl StartupController {
    pub(crate) fn new(
        host: Arc<dyn DesktopHost>,
        panel: slint::Weak<Panel>,
        admission: Rc<dyn Fn() -> bool>,
    ) -> Rc<Self> {
        let actor = Rc::new(Self {
            host,
            panel,
            admission,
            state: RefCell::new(State::default()),
            mailbox: Arc::default(),
            projecting: Cell::new(false),
            acquiring: Cell::new(false),
        });
        if let Some(panel) = actor.panel.upgrade() {
            let weak = Rc::downgrade(&actor);
            panel.on_startup_requested(move |registered| {
                if let Some(actor) = weak.upgrade() {
                    actor.request(registered);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_startup_refresh_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.refresh_requested();
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_startup_event_ready(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.receive();
                }
            });
        }
        actor
    }

    fn source(&self) -> Option<(Panel, WindowFrame)> {
        if !(self.admission)() {
            return None;
        }
        let panel = self.panel.upgrade()?;
        if !panel.get_startup_general_visible() {
            return None;
        }
        let frame = WindowFrame::capture(panel.window())?;
        Some((panel, frame))
    }

    fn current(&self, session: Session) -> bool {
        let Some((_, frame)) = self.source() else {
            return false;
        };
        let state = self.state.borrow();
        !state.exhausted && state.session == Some(session) && frame == session.frame
    }

    /// Called only for an admitted, shown General presentation. No polling.
    pub(crate) fn refresh_root(self: &Rc<Self>) {
        if self.projecting.get() || self.acquiring.get() {
            return;
        }
        let Some((_, frame)) = self.source() else {
            self.stop_root();
            return;
        };
        let revision = self.state.borrow().sequence;
        if !self.state.borrow().provider_checked {
            self.acquiring.set(true);
            let _guard = ResetFlag(&self.acquiring);
            let provider = self.host.startup_host();
            let mut state = self.state.borrow_mut();
            state.provider_checked = true;
            state.provider = provider;
        }
        if self.state.borrow().sequence != revision {
            return;
        }
        // Capability getters may reenter, hide, or retire the Root.
        let Some((_, current_frame)) = self.source() else {
            self.stop_root();
            return;
        };
        if frame != current_frame {
            self.stop_root();
            return;
        }
        let session = {
            let mut state = self.state.borrow_mut();
            if state.exhausted {
                return;
            }
            if state.session.is_none_or(|session| session.frame != frame) {
                let Some(id) = state.next() else { return };
                state.session = Some(Session { id, frame });
                state.read_started = false;
                state.snapshot = None;
                state.error = None;
            }
            state.session.unwrap()
        };
        self.project(session);
        let read = {
            let state = self.state.borrow();
            state.provider.is_some() && !state.read_started && state.flight.is_none()
        };
        if read {
            self.submit(session, None, None);
        }
    }

    /// Retire unsubmitted demand and projection, never an accepted native effect.
    pub(crate) fn stop_root(&self) {
        {
            let mut state = self.state.borrow_mut();
            if state.session.take().is_some() || self.acquiring.get() {
                let _ = state.next();
            }
            state.snapshot = None;
            state.error = None;
            state.read_started = false;
        }
        if self.projecting.replace(true) {
            return;
        }
        let _guard = ResetFlag(&self.projecting);
        let busy = self.state.borrow().flight.is_some();
        if let Some(panel) = self.panel.upgrade() {
            panel.set_startup_controls_enabled(false);
            panel.set_startup_input_key(slint::SharedString::default());
            panel.set_startup_known(false);
            panel.set_startup_busy(busy);
            if !busy {
                panel.set_startup_status(
                    "Registration is no longer current. Refresh to read it.".into(),
                );
            }
        }
    }

    fn input(&self, refresh: bool) -> Option<Session> {
        if self.projecting.get() || self.acquiring.get() {
            return None;
        }
        let (panel, _) = self.source()?;
        if !Self::focused(&panel)
            || !(if refresh {
                panel.get_startup_refresh_input_active()
            } else {
                panel.get_startup_input_active() && panel.get_startup_control_visible()
            })
        {
            return None;
        }
        let session = self.state.borrow().session?;
        self.current(session).then_some(session)
    }

    #[cfg(any(windows, test))]
    fn focused(panel: &Panel) -> bool {
        use slint::winit_030::WinitWindowAccessor;
        panel
            .window()
            .with_winit_window(|window| window.has_focus())
            == Some(true)
    }

    #[cfg(not(any(windows, test)))]
    fn focused(_panel: &Panel) -> bool {
        false
    }

    fn refresh_requested(self: &Rc<Self>) {
        let Some((panel, frame)) = self.source() else {
            return;
        };
        if !Self::focused(&panel) || !panel.get_startup_refresh_input_active() {
            return;
        }
        if self
            .state
            .borrow()
            .session
            .is_none_or(|session| session.frame != frame)
        {
            // A real Refresh is allowed to establish a new native frame; an
            // old toggle never gets this privilege or its old target restored.
            self.stop_root();
            self.refresh_root();
            return;
        }
        if let Some(session) = self.input(true) {
            self.submit(session, None, Some(true));
        }
    }

    fn request(self: &Rc<Self>, registered: bool) {
        let Some(session) = self.input(false) else {
            return;
        };
        let target = {
            let state = self.state.borrow();
            if state.flight.is_some() || state.error.is_some() {
                return;
            }
            let Some(snapshot) = state.snapshot.as_ref() else {
                return;
            };
            if snapshot.registration == StartupRegistration::Foreign
                || (snapshot.registration == StartupRegistration::Registered) == registered
            {
                return;
            }
            snapshot.target.clone()
        };
        if let Some(target) = target {
            self.submit(session, Some((target, registered)), Some(false));
        }
    }

    fn submit(
        self: &Rc<Self>,
        session: Session,
        write: Option<(StartupTarget, bool)>,
        require_input: Option<bool>,
    ) {
        if !self.current(session) {
            self.stop_root();
            return;
        }
        let (provider, flight) = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || state.session != Some(session) {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let Some(ticket) = state.next() else { return };
            let flight = Flight {
                ticket,
                session,
                writing: write.is_some(),
            };
            state.flight = Some(flight);
            state.read_started = true;
            state.error = None;
            (provider, flight)
        };
        self.project(session);
        // Every UI setter above is a reentry point. Check the complete source
        // again immediately before admission; never borrow state across host code.
        if !self.current(session)
            || require_input.is_some_and(|refresh| self.input(refresh) != Some(session))
        {
            self.state.borrow_mut().flight = None;
            self.stop_root();
            return;
        }
        let mailbox = self.mailbox.clone();
        let panel = self.panel.clone();
        // Accepted work owns its provider even if the actor/Root disappears.
        let owner = provider.clone();
        let completion: StartupCompletion = Box::new(move |result| {
            let _owner = owner;
            *mailbox.lock() = Some(Receipt {
                ticket: flight.ticket,
                result,
            });
            let _ = panel.upgrade_in_event_loop(|panel| panel.invoke_startup_event_ready());
        });
        let admitted = match write {
            Some((target, registered)) => provider.set(target, registered, completion),
            None => provider.read(completion),
        };
        if let Err(error) = admitted {
            // Immediate rejection accepts no work and owes no callback.
            *self.mailbox.lock() = Some(Receipt {
                ticket: flight.ticket,
                result: Err(error),
            });
        }
        // Also drains a synchronous fake completion without recursive submission.
        self.receive();
    }

    fn receive(self: &Rc<Self>) {
        let receipt = self.mailbox.lock().take();
        let Some(receipt) = receipt else { return };
        let flight = {
            let mut state = self.state.borrow_mut();
            let Some(flight) = state.flight else { return };
            if flight.ticket != receipt.ticket {
                return;
            }
            state.flight = None;
            flight
        };
        if self.current(flight.session) {
            let mut state = self.state.borrow_mut();
            match receipt.result {
                Ok(snapshot) => {
                    state.snapshot = Some(snapshot);
                    state.error = None;
                }
                Err(error) => {
                    // Preserve the last checked fact, but never preserve authority
                    // after a failed read or unconfirmed accepted mutation.
                    if let Some(snapshot) = state.snapshot.as_mut() {
                        snapshot.target = None;
                    }
                    state.error = Some(error);
                }
            }
            drop(state);
            self.project(flight.session);
        } else {
            // A late receipt only frees the flight. It cannot revive its view.
            self.refresh_root();
        }
    }

    fn project(&self, session: Session) {
        if self.projecting.replace(true) {
            return;
        }
        let _guard = ResetFlag(&self.projecting);
        let (available, busy, known, registered, enabled, status) = {
            let state = self.state.borrow();
            let registration = state
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.registration);
            let busy = state.flight.is_some();
            let enabled = !busy
                && state.error.is_none()
                && state.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.registration != StartupRegistration::Foreign
                        && snapshot.target.is_some()
                });
            let status = if busy {
                if state.flight.is_some_and(|flight| flight.writing) {
                    "Updating registration; waiting for actual registry readback…"
                } else {
                    "Reading current-user startup registration…"
                }
            } else if let Some(error) = state.error.as_ref() {
                match error {
                    StartupError::Busy => {
                        "Startup provider is busy. Refresh to read the actual registration."
                    }
                    StartupError::Unavailable => {
                        "Registration could not be read. Refresh to try again."
                    }
                    StartupError::InvalidTarget => {
                        "The registration changed or this target expired. Refresh before changing it."
                    }
                    StartupError::Unconfirmed => {
                        "The change was accepted but readback failed. Actual registration is unknown; Refresh to check. No rollback was attempted."
                    }
                }
            } else {
                match registration {
                    Some(StartupRegistration::Absent) => {
                        "No Tessera startup registration is present."
                    }
                    Some(StartupRegistration::Registered) => {
                        "This installation is registered. This does not prove Windows will run it at sign-in."
                    }
                    Some(StartupRegistration::Foreign) => {
                        "Another or malformed Tessera entry exists. Read-only: it will not be replaced or removed."
                    }
                    None => "Registration has not been read.",
                }
            };
            (
                state.provider.is_some(),
                busy,
                registration.is_some() && state.error.is_none(),
                registration == Some(StartupRegistration::Registered),
                enabled,
                status,
            )
        };
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        // Check after each setter, including setters which happen to be same-state.
        macro_rules! publish {
            ($setter:ident, $value:expr) => {
                if !self.current(session) {
                    return;
                }
                panel.$setter($value);
            };
        }
        publish!(set_startup_controls_enabled, false);
        publish!(set_startup_available, available);
        publish!(set_startup_busy, busy);
        publish!(set_startup_input_key, session.id.to_string().into());
        publish!(set_startup_known, known);
        publish!(set_startup_registered, registered);
        if !self.current(session) {
            return;
        }
        publish!(set_startup_status, status.into());
        publish!(set_startup_controls_enabled, enabled);
    }
}
