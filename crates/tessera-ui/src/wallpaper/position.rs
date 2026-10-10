// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit global policy operations, with no image or preference-draft authority.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::ComponentHandle;
use tessera_system::wallpaper::position::{
    Observation, Position, ReadCompletion, WriteCompletion, WriteDisposition, WriteOutcome,
};
use tessera_system::wallpaper::{WallpaperError, WallpaperHost};

use super::{Panel, ResetFlag, Session, WallpaperController, WindowFrame};

mod presentation;

#[derive(Clone, PartialEq, Eq)]
enum Command {
    Read,
    Write {
        issued: Observation,
        desired: Position,
        index: i32,
    },
}

#[derive(Clone)]
struct Flight {
    ticket: u64,
    session: Session,
    provider: Arc<dyn WallpaperHost>,
    command: Command,
}

enum Reply {
    Read(Result<Observation, WallpaperError>),
    Written(Result<WriteOutcome, WallpaperError>),
}

struct Receipt {
    ticket: u64,
    reply: Reply,
}

#[derive(Default)]
struct State {
    sequence: u64,
    exhausted: bool,
    provider: Option<Arc<dyn WallpaperHost>>,
    session: Option<Session>,
    observation: Option<Observation>,
    desired: Option<Position>,
    notice: String,
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

    fn retire(&mut self) -> Option<Observation> {
        self.desired = None;
        self.observation.take()
    }
}

pub(super) struct PositionController {
    panel: slint::Weak<Panel>,
    admission: Rc<dyn Fn() -> bool>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Option<Receipt>>>,
    projecting: Cell<bool>,
    submitting: Cell<bool>,
}

impl PositionController {
    pub(super) fn new(panel: slint::Weak<Panel>, admission: Rc<dyn Fn() -> bool>) -> Rc<Self> {
        let actor = Rc::new(Self {
            panel,
            admission,
            state: RefCell::new(State::default()),
            mailbox: Arc::default(),
            projecting: Cell::new(false),
            submitting: Cell::new(false),
        });
        if let Some(panel) = actor.panel.upgrade() {
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_position_read_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.request(false);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_position_apply_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.request(true);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_position_desired_changed(move |index| {
                if let Some(actor) = weak.upgrade() {
                    actor.desired_changed(index);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_position_event_ready(move || {
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
        if !panel.get_wallpaper_page_visible() {
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
        !state.exhausted && state.session == Some(session) && session.frame == frame
    }

    fn provider_current(&self, provider: Option<&Arc<dyn WallpaperHost>>) -> bool {
        let state = self.state.borrow();
        same_provider(state.provider.as_ref(), provider)
    }

    /// The image actor supplies its exact discovered provider; this never reads SDK state.
    pub(super) fn refresh_root(&self, provider: Option<Arc<dyn WallpaperHost>>) {
        if self.projecting.get() || self.submitting.get() {
            return;
        }
        let Some((_, frame)) = self.source() else {
            self.stop_root();
            return;
        };
        let (session, retired) = {
            let mut state = self.state.borrow_mut();
            let changed = !same_provider(state.provider.as_ref(), provider.as_ref());
            state.provider = provider;
            if state.exhausted {
                (None, state.retire())
            } else if changed || state.session.is_none_or(|session| session.frame != frame) {
                let retired = state.retire();
                state.notice.clear();
                state.session = state.next().map(|id| Session { id, frame });
                (state.session, retired)
            } else {
                (state.session, None)
            }
        };
        drop(retired);
        let Some(session) = session else {
            self.stop_root();
            return;
        };
        // Update source identity before draining: late data must not enter this Root.
        self.receive();
        if !self.project(session, None) {
            self.stop_root();
        }
    }

    /// Accepted flights outlive Root; all issued UI authority and held gestures retire.
    pub(super) fn stop_root(&self) {
        let (revision, retired) = {
            let mut state = self.state.borrow_mut();
            if state.session.take().is_some() {
                let _ = state.next();
            }
            state.notice.clear();
            (state.sequence, state.retire())
        };
        drop(retired);
        self.clear_projection(revision);
    }

    fn stopped(&self, revision: u64) -> bool {
        let state = self.state.borrow();
        state.sequence == revision && state.session.is_none()
    }

    fn intent_current(&self, session: Session, command: &Command) -> bool {
        if !self.current(session) {
            return false;
        }
        let Some((panel, frame)) = self.source() else {
            return false;
        };
        if frame != session.frame || !WallpaperController::focused(&panel) {
            return false;
        }
        match command {
            Command::Read => {
                panel.get_wallpaper_position_read_input_active()
                    && panel.get_wallpaper_position_read_control_visible()
            }
            Command::Write { index, desired, .. } => {
                panel.get_wallpaper_position_apply_input_active()
                    && panel.get_wallpaper_position_apply_control_visible()
                    && panel.get_wallpaper_position_desired_index() == *index
                    && position_at(*index) == Some(*desired)
            }
        }
    }

    fn flight_current(&self, flight: &Flight) -> bool {
        let state = self.state.borrow();
        state.session == Some(flight.session)
            && state.sequence == flight.ticket
            && same_provider(state.provider.as_ref(), Some(&flight.provider))
            && state.flight.as_ref().is_some_and(|current| {
                current.ticket == flight.ticket
                    && current.session == flight.session
                    && Arc::ptr_eq(&current.provider, &flight.provider)
                    && current.command == flight.command
            })
    }

    fn desired_changed(&self, index: i32) {
        if self.projecting.get() || self.submitting.get() {
            return;
        }
        let Some((panel, _)) = self.source() else {
            return;
        };
        let Some(session) = self.state.borrow().session else {
            return;
        };
        if !self.current(session) {
            self.stop_root();
            return;
        }
        let desired =
            position_at(index).filter(|_| panel.get_wallpaper_position_desired_index() == index);
        if desired.is_some() && !WallpaperController::focused(&panel) {
            return;
        }
        let retired = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || state.observation.is_none() {
                return;
            }
            let retired = if let Some(desired) = desired {
                state.desired = Some(desired);
                None
            } else {
                state.notice =
                    "Invalid position choice. Read current again; no change was requested.".into();
                state.retire()
            };
            let _ = state.next();
            retired
        };
        drop(retired);
        if !self.project(session, None) {
            self.stop_root();
        }
    }

    fn request(self: &Rc<Self>, write: bool) {
        if self.projecting.get() || self.submitting.get() {
            return;
        }
        let Some(session) = self.state.borrow().session else {
            return;
        };
        if write {
            let Some((panel, _)) = self.source() else {
                return;
            };
            let index = panel.get_wallpaper_position_desired_index();
            if position_at(index).is_none() {
                self.desired_changed(index);
                return;
            }
        }
        let command = {
            let state = self.state.borrow();
            if state.flight.is_some() {
                return;
            }
            if write {
                let (Some(issued), Some(desired)) = (state.observation.as_ref(), state.desired)
                else {
                    return;
                };
                Command::Write {
                    issued: issued.clone(),
                    desired,
                    index: position_index(desired),
                }
            } else {
                Command::Read
            }
        };
        if !self.intent_current(session, &command) {
            return;
        }
        let (flight, retired) = {
            let mut state = self.state.borrow_mut();
            if state.session != Some(session) || state.flight.is_some() {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            if let Command::Write {
                issued, desired, ..
            } = &command
                && (state.observation.as_ref() != Some(issued) || state.desired != Some(*desired))
            {
                return;
            }
            let Some(ticket) = state.next() else {
                drop(state);
                self.stop_root();
                return;
            };
            let retired = state.retire();
            let flight = Flight {
                ticket,
                session,
                provider,
                command,
            };
            state.flight = Some(flight.clone());
            state.notice.clear();
            (flight, retired)
        };
        drop(retired);
        if !self.project(session, Some(&flight))
            || !self.intent_current(session, &flight.command)
            || !self.flight_current(&flight)
        {
            self.state.borrow_mut().flight = None;
            self.stop_root();
            return;
        }
        let mailbox = self.mailbox.clone();
        let panel = self.panel.clone();
        let owner = flight.provider.clone();
        let ticket = flight.ticket;
        let admitted = {
            self.submitting.set(true);
            let _guard = ResetFlag(&self.submitting);
            // Final guards are immediately before entering the foreign provider.
            if !self.intent_current(session, &flight.command) || !self.flight_current(&flight) {
                drop(_guard);
                self.state.borrow_mut().flight = None;
                self.stop_root();
                return;
            }
            match &flight.command {
                Command::Read => {
                    let completion: ReadCompletion = Box::new(move |result| {
                        let _owner = owner;
                        deliver(&mailbox, &panel, ticket, Reply::Read(result));
                    });
                    flight.provider.read_position(completion)
                }
                Command::Write {
                    issued, desired, ..
                } => {
                    let completion: WriteCompletion = Box::new(move |result| {
                        let _owner = owner;
                        deliver(&mailbox, &panel, ticket, Reply::Written(result));
                    });
                    flight
                        .provider
                        .set_position(issued.target.clone(), *desired, completion)
                }
            }
        };
        if let Err(error) = admitted {
            let reply = match flight.command {
                Command::Read => Reply::Read(Err(error)),
                Command::Write { .. } => Reply::Written(Err(error)),
            };
            *self.mailbox.lock() = Some(Receipt { ticket, reply });
        }
        self.receive();
    }

    fn receive(&self) {
        if self.projecting.get() || self.submitting.get() {
            return;
        }
        let Some(receipt) = self.mailbox.lock().take() else {
            return;
        };
        let flight = {
            let mut state = self.state.borrow_mut();
            if state
                .flight
                .as_ref()
                .is_none_or(|flight| flight.ticket != receipt.ticket)
            {
                return;
            }
            state.flight.take().expect("matching position flight")
        };
        if !self.current(flight.session) || !self.provider_current(Some(&flight.provider)) {
            // Discard old data/authority before a fresh readonly current-source
            // refresh. Reopening while a request is pending must not stay busy.
            drop(receipt);
            drop(flight);
            let provider = self.state.borrow().provider.clone();
            self.refresh_root(provider);
            return;
        }
        let (observation, notice) = match (&flight.command, receipt.reply) {
            (Command::Read, Reply::Read(Ok(observation))) => (
                Some(observation), "Actual global Windows position read. Choose a proposal, then Apply globally; Save and Cancel are unrelated.".into(),
            ),
            (Command::Write { issued, desired, .. }, Reply::Written(Ok(outcome))) => {
                outcome_projection(issued, *desired, outcome)
            }
            (_, Reply::Read(Err(error)) | Reply::Written(Err(error))) => (None, error_notice(error).into()),
            _ => (None, invalid_notice().into()),
        };
        if !self.current(flight.session) || !self.provider_current(Some(&flight.provider)) {
            drop(observation);
            drop(flight);
            let provider = self.state.borrow().provider.clone();
            self.refresh_root(provider);
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.desired = observation.as_ref().map(|observation| observation.position);
            state.observation = observation;
            state.notice = notice;
            let _ = state.next();
        }
        if !self.project(flight.session, None) {
            self.stop_root();
        }
    }
}

fn same_provider(
    left: Option<&Arc<dyn WallpaperHost>>,
    right: Option<&Arc<dyn WallpaperHost>>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        (None, None) => true,
        _ => false,
    }
}

fn position_at(index: i32) -> Option<Position> {
    usize::try_from(index)
        .ok()
        .and_then(|index| Position::ALL.get(index))
        .copied()
}

fn position_index(position: Position) -> i32 {
    Position::ALL
        .iter()
        .position(|value| *value == position)
        .expect("closed position") as i32
}

fn caption(position: Position) -> &'static str {
    match position {
        Position::Center => "Center",
        Position::Tile => "Tile",
        Position::Stretch => "Stretch",
        Position::Fit => "Fit",
        Position::Fill => "Fill",
        Position::Span => "Span",
    }
}

fn deliver(
    mailbox: &Mutex<Option<Receipt>>,
    panel: &slint::Weak<Panel>,
    ticket: u64,
    reply: Reply,
) {
    *mailbox.lock() = Some(Receipt { ticket, reply });
    let _ = panel.upgrade_in_event_loop(|panel| panel.invoke_wallpaper_position_event_ready());
}

fn outcome_projection(
    issued: &Observation,
    desired: Position,
    outcome: WriteOutcome,
) -> (Option<Observation>, String) {
    let fresh = outcome.observation.as_ref();
    if fresh.is_some_and(|observation| observation.target == issued.target)
        || (outcome.disposition == WriteDisposition::AlreadyCurrent
            && (issued.position != desired
                || fresh.is_none_or(|observation| observation.position != desired)))
    {
        return (None, invalid_notice().into());
    }
    let receipt = match outcome.disposition {
        WriteDisposition::AlreadyCurrent => {
            "Already current: a fresh native check matched; no setter was called."
        }
        WriteDisposition::Accepted => "Windows accepted the global position setter.",
        WriteDisposition::Rejected => {
            "Windows rejected the global position setter; state may still have changed."
        }
    };
    let readback = match fresh {
        Some(observation) if observation.position == desired => {
            format!("Fresh native readback confirms {}.", caption(desired))
        }
        Some(observation) => format!(
            "Fresh native readback is {}, not the requested {}.",
            caption(observation.position),
            caption(desired)
        ),
        None => {
            "Fresh native readback unavailable; current position is unknown. Read current again."
                .into()
        }
    };
    (
        outcome.observation,
        format!(
            "{receipt} {readback} Rendered pixels are not checked; external changes can race the native read/set pair."
        ),
    )
}

fn invalid_notice() -> &'static str {
    "Invalid native position response. Current position is unknown; Read current again. No retry was requested."
}

fn error_notice(error: WallpaperError) -> &'static str {
    match error {
        WallpaperError::Busy => {
            "Windows wallpaper is busy with another image or position request. No work was accepted; Read current explicitly when ready."
        }
        WallpaperError::Unavailable => {
            "Native global Windows position is unavailable or unknown. Read current explicitly; no default is assumed."
        }
        WallpaperError::InvalidTarget => {
            "The issued position observation is stale or invalid. No position setter was requested; Read current again."
        }
    }
}
