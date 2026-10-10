// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One native image selection and immediate wallpaper effect, outside preference drafts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::ComponentHandle;
use tessera_system::wallpaper::{
    WallpaperApplyCompletion, WallpaperApplyOutcome, WallpaperApplyScope,
    WallpaperChooseCompletion, WallpaperError, WallpaperHost, WallpaperImageTarget,
    WallpaperSelection,
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
    Apply {
        target: WallpaperImageTarget,
        scope: WallpaperApplyScope,
        index: i32,
        requested: u32,
    },
}

#[derive(Clone)]
struct Flight {
    ticket: u64,
    session: Session,
    operation: Operation,
    scope: Option<WallpaperApplyScope>,
    monitor_index: Option<i32>,
    requested: u32,
}

enum Reply {
    Chosen(Result<Option<WallpaperSelection>, WallpaperError>),
    Applied(Result<WallpaperApplyOutcome, WallpaperError>),
}

struct Receipt {
    ticket: u64,
    reply: Reply,
}

/// Display indices resolve only inside this validated, native-issued selection.
struct SelectedImage {
    image: WallpaperSelection,
    scope: WallpaperApplyScope,
}

impl SelectedImage {
    fn new(mut image: WallpaperSelection) -> Option<Self> {
        if !(1..=32).contains(&image.monitors.len())
            || image.monitors.iter().enumerate().any(|(index, monitor)| {
                image.monitors[..index]
                    .iter()
                    .any(|previous| previous.target == monitor.target)
            })
        {
            return None;
        }
        image.caption = bounded_caption(&image.caption, 96)?;
        for monitor in &mut image.monitors {
            // Native ID plus RECT metadata is bounded; never invent a label
            // for an empty or malformed descriptor.
            if monitor.caption.chars().count() > 256 {
                return None;
            }
            monitor.caption = bounded_caption(&monitor.caption, 256)?;
        }
        Some(Self {
            image,
            scope: WallpaperApplyScope::AllCaptured,
        })
    }

    fn scope_at(&self, index: i32) -> Option<WallpaperApplyScope> {
        if index == 0 {
            return Some(WallpaperApplyScope::AllCaptured);
        }
        let index = usize::try_from(index).ok()?.checked_sub(1)?;
        self.image
            .monitors
            .get(index)
            .map(|monitor| WallpaperApplyScope::Monitor(monitor.target.clone()))
    }

    fn captions(&self) -> Vec<slint::SharedString> {
        std::iter::once("All captured displays".into())
            .chain(
                self.image
                    .monitors
                    .iter()
                    .map(|monitor| monitor.caption.as_str().into()),
            )
            .collect()
    }

    fn requested(&self) -> Option<u32> {
        match &self.scope {
            WallpaperApplyScope::AllCaptured => u32::try_from(self.image.monitors.len()).ok(),
            WallpaperApplyScope::Monitor(target) => self
                .image
                .monitors
                .iter()
                .any(|monitor| &monitor.target == target)
                .then_some(1),
        }
    }

    fn notice(&self, scope: &WallpaperApplyScope) -> Option<String> {
        let scope = match scope {
            WallpaperApplyScope::AllCaptured => {
                format!("all {} captured displays", self.image.monitors.len())
            }
            WallpaperApplyScope::Monitor(target) => {
                let monitor = self
                    .image
                    .monitors
                    .iter()
                    .find(|monitor| &monitor.target == target)?;
                format!("captured display {}", monitor.caption)
            }
        };
        Some(format!(
            "Selected: {}. Scope: {}. Image usability is checked by Windows.",
            self.image.caption, scope
        ))
    }
}

#[derive(Default)]
struct State {
    sequence: u64,
    exhausted: bool,
    provider_checked: bool,
    provider: Option<Arc<dyn WallpaperHost>>,
    session: Option<Session>,
    selection: Option<SelectedImage>,
    monitor_captions: Vec<slint::SharedString>,
    monitor_index: i32,
    notice: String,
    flight: Option<Flight>,
}

impl State {
    fn clear_selection(&mut self) -> Option<SelectedImage> {
        self.monitor_captions.clear();
        self.monitor_index = -1;
        self.selection.take()
    }

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
            panel.on_wallpaper_monitor_changed(move |index| {
                if let Some(actor) = weak.upgrade() {
                    actor.monitor_changed(index);
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
        let (session, retired) = {
            let mut state = self.state.borrow_mut();
            if state.exhausted {
                (None, state.clear_selection())
            } else if state.session.is_none_or(|session| session.frame != frame) {
                let retired = state.clear_selection();
                state.notice.clear();
                state.session = state.next().map(|id| Session { id, frame });
                (state.session, retired)
            } else {
                (state.session, None)
            }
        };
        // Native selection retirement can enqueue work; no state borrow survives it.
        drop(retired);
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
        let (revision, retired) = {
            let mut state = self.state.borrow_mut();
            if state.session.take().is_some() || self.acquiring.get() {
                let _ = state.next();
            }
            let retired = state.clear_selection();
            state.notice.clear();
            (state.sequence, retired)
        };
        let already_projecting = self.projecting.replace(true);
        drop(retired);
        if already_projecting {
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
        clear!(set_wallpaper_monitor_selection_available, false);
        clear!(set_wallpaper_monitors, slint::ModelRc::default());
        clear!(set_wallpaper_monitor_index, -1);
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
                        && self.scope_current(&panel)
                }
            }
    }

    fn scope_current(&self, panel: &Panel) -> bool {
        let index = panel.get_wallpaper_monitor_index();
        let state = self.state.borrow();
        if state.monitor_index != index {
            return false;
        }
        if let Some(flight) = state.flight.as_ref() {
            return state.session == Some(flight.session)
                && flight.operation == Operation::Apply
                && flight.monitor_index == Some(index)
                && flight.scope.is_some();
        }
        state
            .selection
            .as_ref()
            .is_some_and(|selection| selection.scope_at(index).as_ref() == Some(&selection.scope))
    }

    fn monitor_changed(self: &Rc<Self>, index: i32) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
            return;
        }
        let Some((panel, _)) = self.source() else {
            return;
        };
        let session = self.state.borrow().session;
        let Some(session) = session else { return };
        if !self.current(session) {
            return;
        }
        let ui_index = panel.get_wallpaper_monitor_index();
        let choice = {
            let state = self.state.borrow();
            if state.flight.is_some() {
                return;
            }
            let Some(selection) = state.selection.as_ref() else {
                return;
            };
            if ui_index == index {
                selection
                    .scope_at(index)
                    .and_then(|scope| selection.notice(&scope).map(|notice| (scope, notice)))
            } else {
                None
            }
        };
        let (retired, exhausted) = {
            let mut state = self.state.borrow_mut();
            let retired = if let Some((scope, notice)) = choice {
                let Some(selection) = state.selection.as_mut() else {
                    return;
                };
                selection.scope = scope;
                state.monitor_index = index;
                state.notice = notice;
                None
            } else {
                let retired = state.clear_selection();
                state.notice = "The display scope is invalid. Choose a fresh image; no wallpaper change was requested.".into();
                retired
            };
            (retired, state.next().is_none())
        };
        drop(retired);
        // A fresh scope choice retires held Apply gestures, but never invokes Windows.
        if exhausted || !self.project(session, None) {
            self.stop_root();
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
        let (provider, flight, request, retired) = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || state.session != Some(session) {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            // Both a new picker and an apply retire the old selection. Even
            // rejection cannot restore it or turn a held gesture into a retry.
            let (request, retired) = match operation {
                Operation::Choose => (Request::Choose, state.clear_selection()),
                Operation::Apply => {
                    let Some(selection) = state.selection.as_ref() else {
                        return;
                    };
                    if selection.scope_at(state.monitor_index).as_ref() != Some(&selection.scope) {
                        return;
                    }
                    let Some(requested) = selection.requested() else {
                        return;
                    };
                    let request = Request::Apply {
                        target: selection.image.target.clone(),
                        scope: selection.scope.clone(),
                        index: state.monitor_index,
                        requested,
                    };
                    // Keep only readonly display projection until the flight
                    // completes; no image authority remains in actor state.
                    (request, state.selection.take())
                }
            };
            let Some(ticket) = state.next() else {
                drop(state);
                drop(retired);
                drop(request);
                self.stop_root();
                return;
            };
            let (scope, monitor_index, requested) = match &request {
                Request::Choose => (None, None, 0),
                Request::Apply {
                    scope,
                    index,
                    requested,
                    ..
                } => (Some(scope.clone()), Some(*index), *requested),
            };
            let flight = Flight {
                ticket,
                session,
                operation,
                scope,
                monitor_index,
                requested,
            };
            state.flight = Some(flight.clone());
            state.notice.clear();
            (provider, flight, request, retired)
        };
        drop(retired);
        if !self.project(session, Some(operation))
            || !self.intent_current(session, operation)
            || !self.provider_current(Some(&provider))
            || !self.request_current(&request, flight.ticket)
            || self
                .state
                .borrow()
                .flight
                .as_ref()
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
                Request::Apply { target, scope, .. } => {
                    let completion: WallpaperApplyCompletion = Box::new(move |result| {
                        let _owner = owner;
                        deliver(&mailbox, &panel, flight.ticket, Reply::Applied(result));
                    });
                    provider.apply(target, scope, completion)
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

    fn request_current(&self, request: &Request, ticket: u64) -> bool {
        let state = self.state.borrow();
        let Some(flight) = state.flight.as_ref() else {
            return false;
        };
        if flight.ticket != ticket {
            return false;
        }
        match request {
            Request::Choose => flight.operation == Operation::Choose && flight.scope.is_none(),
            Request::Apply {
                scope,
                index,
                requested,
                ..
            } => {
                flight.operation == Operation::Apply
                    && flight.scope.as_ref() == Some(scope)
                    && flight.monitor_index == Some(*index)
                    && state.monitor_index == *index
                    && flight.requested == *requested
            }
        }
    }

    fn receive(self: &Rc<Self>) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
            return;
        }
        let receipt = self.mailbox.lock().take();
        let Some(receipt) = receipt else { return };
        let flight = {
            let mut state = self.state.borrow_mut();
            let Some(current) = state.flight.as_ref() else {
                return;
            };
            if current.ticket != receipt.ticket {
                return;
            }
            let Some(flight) = state.flight.take() else {
                return;
            };
            flight
        };
        if !self.current(flight.session) {
            // Discard every old result/target before independently projecting
            // current availability. Releasing a flight does not revive its scope.
            drop(receipt);
            self.refresh_root();
            return;
        }
        // Validation and rejected target destruction happen outside RefCell borrows.
        let (selection, notice) = match (flight.operation, receipt.reply) {
            (Operation::Choose, Reply::Chosen(Ok(Some(image)))) => {
                match SelectedImage::new(image).and_then(|selection| {
                    let notice = selection.notice(&selection.scope)?;
                    Some((selection, notice))
                }) {
                    Some((selection, notice)) => (Some(selection), notice),
                    None => (
                        None,
                        "The native image selection contains invalid display metadata. No image is selected; choose again.".into(),
                    ),
                }
            }
            (Operation::Choose, Reply::Chosen(Ok(None))) => {
                (None, "Image picker cancelled. No image is selected.".into())
            }
            (Operation::Apply, Reply::Applied(Ok(outcome))) => {
                (None, outcome_notice(outcome, flight.requested))
            }
            (Operation::Choose, Reply::Chosen(Err(error)))
            | (Operation::Apply, Reply::Applied(Err(error))) => {
                (None, error_notice(flight.operation, error).into())
            }
            _ => (
                None,
                "The provider returned an invalid response. Actual Windows wallpaper is unknown; choose a fresh image.".into(),
            ),
        };
        if !self.current(flight.session) {
            drop(selection);
            self.refresh_root();
            return;
        }
        let captions = selection
            .as_ref()
            .map(SelectedImage::captions)
            .unwrap_or_default();
        let index = if selection.is_some() { 0 } else { -1 };
        {
            let mut state = self.state.borrow_mut();
            state.selection = selection;
            state.monitor_captions = captions;
            state.monitor_index = index;
            state.notice = notice;
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
        let (provider, busy, apply_enabled, captions, index, key, status) = {
            let state = self.state.borrow();
            let status = match state.flight.as_ref() {
                Some(flight) if flight.session != session => {
                    "A previous wallpaper request is pending. Its result will not be shown in this view.".into()
                }
                Some(flight) if flight.operation == Operation::Choose => {
                    "Waiting for the Windows image picker… Choosing does not apply the image.".into()
                }
                Some(flight) => format!(
                    "Requesting wallpaper on {} selected captured display(s)… Awaiting native path/file readback; rendered pixels are not checked.",
                    flight.requested
                ),
                None if state.notice.is_empty() => {
                    "Current Windows wallpaper has not been read. Choose an image to begin.".into()
                }
                None => state.notice.clone(),
            };
            (
                state.provider.clone(),
                state.flight.is_some(),
                state.selection.is_some(),
                state.monitor_captions.clone(),
                if state.monitor_captions.is_empty() {
                    -1
                } else {
                    state.monitor_index
                },
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
        // Hide the stock selector as soon as its image authority is consumed.
        // Pending readonly captions/index preserve final scope agreement only.
        publish!(set_wallpaper_monitor_selection_available, apply_enabled);
        publish!(
            set_wallpaper_monitors,
            slint::ModelRc::new(slint::VecModel::from(captions))
        );
        publish!(set_wallpaper_monitor_index, index);
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

fn outcome_notice(outcome: WallpaperApplyOutcome, requested: u32) -> String {
    let sum = outcome
        .accepted
        .checked_add(outcome.failed)
        .and_then(|count| count.checked_add(outcome.not_submitted));
    if !(1..=32).contains(&requested)
        || outcome.requested != requested
        || outcome.accepted > 32
        || outcome.confirmed > outcome.accepted
        || outcome.failed > 32
        || outcome.not_submitted > 32
        || sum != Some(outcome.requested)
    {
        return "The provider returned invalid monitor counts. Actual Windows wallpaper is unknown; choose a fresh image.".into();
    }
    format!(
        "Requested captured displays: {}. Accepted: {}; path-confirmed: {}; unconfirmed: {}; failed: {}; not submitted: {}. Path/file readback does not confirm rendered pixels. Choose a fresh image to apply again.",
        outcome.requested,
        outcome.accepted,
        outcome.confirmed,
        outcome.accepted - outcome.confirmed,
        outcome.failed,
        outcome.not_submitted,
    )
}

fn bounded_caption(value: &str, limit: usize) -> Option<String> {
    let caption: String = value
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(*character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(limit)
        .collect();
    let caption = caption.trim();
    (!caption.is_empty()).then(|| caption.to_owned())
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
