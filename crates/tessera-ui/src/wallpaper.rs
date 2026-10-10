// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One native image selection and immediate wallpaper effect, outside preference drafts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::ComponentHandle;
use tessera_system::wallpaper::{
    WallpaperApplyCompletion, WallpaperApplyOutcome, WallpaperChooseCompletion, WallpaperError,
    WallpaperHost, WallpaperImageTarget, WallpaperSelection,
};

use crate::DesktopHost;
use crate::application_menu::WindowFrame;
use crate::generated::Panel;

#[derive(Clone, Copy, PartialEq)]
struct Session {
    id: u64,
    frame: WindowFrame,
}

#[derive(Clone, Copy, PartialEq)]
enum Operation {
    Choose,
    Apply,
}

enum Request {
    Choose,
    Apply(WallpaperImageTarget),
}

#[derive(Clone, Copy)]
struct Flight {
    ticket: u64,
    session: Session,
    operation: Operation,
}

enum Reply {
    Chosen(Result<Option<WallpaperSelection>, WallpaperError>),
    Applied(Result<WallpaperApplyOutcome, WallpaperError>),
}

struct Receipt {
    ticket: u64,
    reply: Reply,
}

#[derive(Default)]
struct State {
    sequence: u64,
    exhausted: bool,
    provider_checked: bool,
    provider: Option<Arc<dyn WallpaperHost>>,
    session: Option<Session>,
    selection: Option<WallpaperImageTarget>,
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
}

struct ResetFlag<'a>(&'a Cell<bool>);

impl Drop for ResetFlag<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub(crate) struct WallpaperController {
    host: Arc<dyn DesktopHost>,
    panel: slint::Weak<Panel>,
    admission: Rc<dyn Fn() -> bool>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Option<Receipt>>>,
    projecting: Cell<bool>,
    acquiring: Cell<bool>,
    submitting: Cell<bool>,
}

impl WallpaperController {
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
            submitting: Cell::new(false),
        });
        if let Some(panel) = actor.panel.upgrade() {
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_choose_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.request(Operation::Choose);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_apply_requested(move || {
                if let Some(actor) = weak.upgrade() {
                    actor.request(Operation::Apply);
                }
            });
            let weak = Rc::downgrade(&actor);
            panel.on_wallpaper_event_ready(move || {
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
        !state.exhausted && state.session == Some(session) && frame == session.frame
    }

    fn provider_current(&self, provider: Option<&Arc<dyn WallpaperHost>>) -> bool {
        let state = self.state.borrow();
        match (state.provider.as_ref(), provider) {
            (Some(current), Some(provider)) => Arc::ptr_eq(current, provider),
            (None, None) => true,
            _ => false,
        }
    }

    /// Capability discovery is lazy and admitted; refreshing never reads Windows wallpaper.
    pub(crate) fn refresh_root(self: &Rc<Self>) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
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
            let provider = self.host.wallpaper_host();
            let mut state = self.state.borrow_mut();
            state.provider_checked = true;
            state.provider = provider;
        }
        if self.state.borrow().sequence != revision {
            return;
        }
        // Capability getters can reenter or retire the Root.
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
                None
            } else if state.session.is_none_or(|session| session.frame != frame) {
                state.selection = None;
                state.notice.clear();
                state.session = state.next().map(|id| Session { id, frame });
                state.session
            } else {
                state.session
            }
        };
        let Some(session) = session else {
            self.stop_root();
            return;
        };
        if !self.project(session, None) {
            self.stop_root();
        }
    }

    /// Retire selection and input, but never cancel or replay an accepted native effect.
    pub(crate) fn stop_root(&self) {
        let revision = {
            let mut state = self.state.borrow_mut();
            if state.session.take().is_some() || self.acquiring.get() {
                let _ = state.next();
            }
            state.selection = None;
            state.notice.clear();
            state.sequence
        };
        if self.projecting.replace(true) {
            return;
        }
        let _guard = ResetFlag(&self.projecting);
        let busy = self.state.borrow().flight.is_some();
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        macro_rules! clear {
            ($setter:ident, $value:expr) => {
                if !self.stopped(revision) {
                    return;
                }
                panel.$setter($value);
                if !self.stopped(revision) {
                    return;
                }
            };
        }
        clear!(set_wallpaper_controls_enabled, false);
        clear!(set_wallpaper_apply_enabled, false);
        clear!(set_wallpaper_input_key, slint::SharedString::default());
        clear!(set_wallpaper_available, false);
        clear!(set_wallpaper_busy, busy);
        clear!(
            set_wallpaper_status,
            "Selection is no longer current. Choose a fresh image.".into()
        );
    }

    fn stopped(&self, revision: u64) -> bool {
        let state = self.state.borrow();
        state.sequence == revision && state.session.is_none()
    }

    // A genuine TileButton callback supplies intent. Re-read actual clipping
    // independently of enabled/busy after every potentially reflowing setter.
    fn intent_current(&self, session: Session, operation: Operation) -> bool {
        if !self.current(session) {
            return false;
        }
        let Some((panel, frame)) = self.source() else {
            return false;
        };
        frame == session.frame
            && Self::focused(&panel)
            && match operation {
                Operation::Choose => {
                    panel.get_wallpaper_choose_input_active()
                        && panel.get_wallpaper_choose_control_visible()
                }
                Operation::Apply => {
                    panel.get_wallpaper_apply_input_active()
                        && panel.get_wallpaper_apply_control_visible()
                }
            }
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

    fn request(self: &Rc<Self>, operation: Operation) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
            return;
        }
        let session = self.state.borrow().session;
        let Some(session) = session else { return };
        if !self.intent_current(session, operation) {
            return;
        }
        let (provider, flight, request) = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || state.session != Some(session) {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            // Both a new picker and an apply retire the old selection. Even
            // rejection cannot restore it or turn a held gesture into a retry.
            let request = match operation {
                Operation::Choose => {
                    state.selection = None;
                    Request::Choose
                }
                Operation::Apply => {
                    let Some(target) = state.selection.take() else {
                        return;
                    };
                    Request::Apply(target)
                }
            };
            let Some(ticket) = state.next() else { return };
            let flight = Flight {
                ticket,
                session,
                operation,
            };
            state.flight = Some(flight);
            state.notice.clear();
            (provider, flight, request)
        };
        if !self.project(session, Some(operation))
            || !self.intent_current(session, operation)
            || !self.provider_current(Some(&provider))
            || self
                .state
                .borrow()
                .flight
                .is_none_or(|current| current.ticket != flight.ticket)
        {
            self.state.borrow_mut().flight = None;
            self.stop_root();
            return;
        }
        let mailbox = self.mailbox.clone();
        let panel = self.panel.clone();
        // Accepted work retains its provider independently of this actor/Root.
        let owner = provider.clone();
        let admitted = {
            self.submitting.set(true);
            let _guard = ResetFlag(&self.submitting);
            match request {
                Request::Choose => {
                    let completion: WallpaperChooseCompletion = Box::new(move |result| {
                        let _owner = owner;
                        deliver(&mailbox, &panel, flight.ticket, Reply::Chosen(result));
                    });
                    provider.choose(completion)
                }
                Request::Apply(target) => {
                    let completion: WallpaperApplyCompletion = Box::new(move |result| {
                        let _owner = owner;
                        deliver(&mailbox, &panel, flight.ticket, Reply::Applied(result));
                    });
                    provider.apply(target, completion)
                }
            }
        };
        if let Err(error) = admitted {
            // Immediate errors accepted no work and owe no completion.
            let reply = match operation {
                Operation::Choose => Reply::Chosen(Err(error)),
                Operation::Apply => Reply::Applied(Err(error)),
            };
            *self.mailbox.lock() = Some(Receipt {
                ticket: flight.ticket,
                reply,
            });
        }
        // Synchronous/reentrant providers cannot recursively start another flight.
        self.receive();
    }

    fn receive(self: &Rc<Self>) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
            return;
        }
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
        if !self.current(flight.session) {
            // Discard every old result/target before independently projecting
            // current availability. Releasing a flight does not revive its scope.
            drop(receipt);
            self.refresh_root();
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            match (flight.operation, receipt.reply) {
                (Operation::Choose, Reply::Chosen(Ok(Some(selection)))) => {
                    let caption: String = selection
                        .caption
                        .chars()
                        .filter(|character| !character.is_control())
                        .take(96)
                        .collect();
                    state.notice = if caption.trim().is_empty() {
                        "Image selected (name unavailable). Apply requests its captured target; image usability is checked by Windows.".into()
                    } else {
                        format!(
                            "Selected: {}. Apply requests this image on its captured monitors; image usability is checked by Windows.",
                            caption.trim()
                        )
                    };
                    state.selection = Some(selection.target);
                }
                (Operation::Choose, Reply::Chosen(Ok(None))) => {
                    state.notice = "Image picker cancelled. No image is selected.".into();
                }
                (Operation::Apply, Reply::Applied(Ok(outcome))) => {
                    state.notice = outcome_notice(outcome);
                }
                (Operation::Choose, Reply::Chosen(Err(error)))
                | (Operation::Apply, Reply::Applied(Err(error))) => {
                    state.notice = error_notice(flight.operation, error).into();
                }
                _ => {
                    state.notice = "The provider returned an invalid response. Actual Windows wallpaper is unknown; choose a fresh image.".into();
                }
            }
        }
        if !self.project(flight.session, None) {
            self.stop_root();
        }
    }

    fn project(&self, session: Session, intent: Option<Operation>) -> bool {
        if self.projecting.replace(true) {
            return false;
        }
        let _guard = ResetFlag(&self.projecting);
        let (provider, busy, apply_enabled, key, status) = {
            let state = self.state.borrow();
            let status = match state.flight {
                Some(flight) if flight.session != session => {
                    "A previous wallpaper request is pending. Its result will not be shown in this view.".into()
                }
                Some(flight) if flight.operation == Operation::Choose => {
                    "Waiting for the Windows image picker… Choosing does not apply the image.".into()
                }
                Some(_) => {
                    "Requesting Windows wallpaper on the captured monitors… Awaiting native path readback; rendered pixels are not checked.".into()
                }
                None if state.notice.is_empty() => {
                    "Current Windows wallpaper has not been read. Choose an image to begin.".into()
                }
                None => state.notice.clone(),
            };
            (
                state.provider.clone(),
                state.flight.is_some(),
                state.selection.is_some(),
                state.sequence.to_string(),
                status,
            )
        };
        let Some(panel) = self.panel.upgrade() else {
            return false;
        };
        let current = || {
            self.current(session)
                && self.provider_current(provider.as_ref())
                && intent.is_none_or(|operation| self.intent_current(session, operation))
        };
        // No borrow crosses a setter; every setter is a possible reentry point.
        macro_rules! publish {
            ($setter:ident, $value:expr) => {
                if !current() {
                    return false;
                }
                panel.$setter($value);
                if !current() {
                    return false;
                }
            };
        }
        publish!(set_wallpaper_controls_enabled, false);
        publish!(set_wallpaper_apply_enabled, false);
        publish!(set_wallpaper_available, provider.is_some());
        publish!(set_wallpaper_busy, busy);
        publish!(set_wallpaper_input_key, key.into());
        publish!(set_wallpaper_status, status.into());
        publish!(
            set_wallpaper_apply_enabled,
            provider.is_some() && !busy && apply_enabled
        );
        publish!(set_wallpaper_controls_enabled, provider.is_some() && !busy);
        true
    }
}

fn deliver(
    mailbox: &Mutex<Option<Receipt>>,
    panel: &slint::Weak<Panel>,
    ticket: u64,
    reply: Reply,
) {
    *mailbox.lock() = Some(Receipt { ticket, reply });
    let _ = panel.upgrade_in_event_loop(|panel| panel.invoke_wallpaper_event_ready());
}

fn outcome_notice(outcome: WallpaperApplyOutcome) -> String {
    let sum = outcome
        .accepted
        .checked_add(outcome.failed)
        .and_then(|count| count.checked_add(outcome.not_submitted));
    if outcome.requested > 32
        || outcome.accepted > 32
        || outcome.confirmed > outcome.accepted
        || outcome.failed > 32
        || outcome.not_submitted > 32
        || sum != Some(outcome.requested)
    {
        return "The provider returned invalid monitor counts. Actual Windows wallpaper is unknown; choose a fresh image.".into();
    }
    format!(
        "Captured monitors: {}. Accepted: {}; path-confirmed: {}; unconfirmed: {}; failed: {}; not submitted: {}. Path/file readback does not confirm rendered pixels. Choose a fresh image to apply again.",
        outcome.requested,
        outcome.accepted,
        outcome.confirmed,
        outcome.accepted - outcome.confirmed,
        outcome.failed,
        outcome.not_submitted,
    )
}

fn error_notice(operation: Operation, error: WallpaperError) -> &'static str {
    match (operation, error) {
        (_, WallpaperError::Busy) => {
            "The wallpaper provider is busy. Nothing is confirmed; choose a fresh image before trying again."
        }
        (Operation::Choose, WallpaperError::Unavailable) => {
            "The Windows image picker or file validation failed. No image is selected. Choose again to try another image."
        }
        (Operation::Apply, WallpaperError::Unavailable) => {
            "The wallpaper request failed. Actual Windows wallpaper is unknown; choose a fresh image before trying again."
        }
        (_, WallpaperError::InvalidTarget) => {
            "The selected file, captured monitors, or target is no longer valid. Nothing is confirmed; choose a fresh image."
        }
    }
}
