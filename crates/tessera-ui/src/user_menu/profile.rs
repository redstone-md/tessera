// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Memory-only profile projection. Folder state, tokens and repeaters are never touched.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{Image, SharedString};
use tessera_system::profile::{
    ProfileCommand, ProfileError, ProfileErrorKind, ProfileHost, ProfilePhotoState, ProfileSnapshot,
};

use super::UserMenuController;
use crate::DesktopHost;
use crate::generated::{UserMenu, UserProfileAction};
use crate::sanitize::bounded_text;

pub(super) mod avatar;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Token {
    session: u64,
    sequence: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Scope {
    session: u64,
    revision: u64,
}

#[derive(Clone, Copy)]
enum Request {
    Read,
    Open { onedrive: bool },
}

struct Flight {
    token: Token,
    request: Request,
}

enum Outcome {
    Read(Result<ProfileSnapshot, ProfileError>),
    Open(Result<(), ProfileError>),
}

struct Completion {
    token: Token,
    outcome: Outcome,
}

struct Projection {
    snapshot: ProfileSnapshot,
    image: Image,
    key: SharedString,
    onedrive_valid: bool,
}

/// One immutable UI-thread view; no actor borrow crosses generated setters.
struct View {
    scope: Scope,
    name: String,
    email: String,
    image: Image,
    has_photo: bool,
    fallback: bool,
    loading: bool,
    opening: bool,
    ready: bool,
    onedrive: bool,
    action_key: SharedString,
    retry: bool,
    retry_key: SharedString,
    notice: String,
}

#[derive(Default)]
struct State {
    session: u64,
    revision: u64,
    sequence: u64,
    exhausted: bool,
    active: bool,
    requested: bool,
    acquiring: bool,
    provider: Option<Arc<dyn ProfileHost>>,
    flight: Option<Flight>,
    confirmed: Option<Projection>,
    notice: String,
    retry: bool,
    key: SharedString,
}

impl State {
    fn scope(&self) -> Scope {
        Scope {
            session: self.session,
            revision: self.revision,
        }
    }

    fn advance(&mut self) {
        if let Some(revision) = self.revision.checked_add(1) {
            self.revision = revision;
            self.key = format!("profile-{:x}-{:x}", self.session, revision).into();
        } else {
            self.exhausted = true;
            self.active = false;
            self.key = SharedString::default();
        }
    }

    fn retire(&mut self) {
        if let Some(session) = self.session.checked_add(1) {
            self.session = session;
        } else {
            self.exhausted = true;
        }
        self.active = false;
        self.requested = false;
        self.confirmed = None;
        self.notice.clear();
        self.retry = false;
        self.advance();
        // An accepted read/open persists until its completion, even across Hide.
    }

    fn token(&mut self) -> Option<Token> {
        self.sequence = match self.sequence.checked_add(1) {
            Some(sequence) => sequence,
            None => {
                self.exhausted = true;
                self.active = false;
                return None;
            }
        };
        Some(Token {
            session: self.session,
            sequence: self.sequence,
        })
    }

    fn busy(&self) -> bool {
        self.requested || self.acquiring || self.flight.is_some()
    }
}

/// At most one accepted operation and one completion; no accumulating event queue.
#[derive(Default)]
struct Mailbox {
    expected: Option<Token>,
    completion: Option<Completion>,
    wake_queued: bool,
}

fn complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<UserMenu>,
    token: Token,
    outcome: Outcome,
) {
    {
        let mut mailbox = mailbox.lock();
        if mailbox.expected != Some(token) || mailbox.completion.is_some() {
            return;
        }
        mailbox.completion = Some(Completion { token, outcome });
        if mailbox.wake_queued {
            return;
        }
        mailbox.wake_queued = true;
    }
    if root
        .upgrade_in_event_loop(|root| root.invoke_profile_event_ready())
        .is_err()
    {
        mailbox.lock().wake_queued = false;
    }
}

type Admission = Rc<dyn Fn() -> bool>;

pub(super) struct ProfileController {
    owner: Weak<UserMenuController>,
    root: slint::Weak<UserMenu>,
    host: Arc<dyn DesktopHost>,
    admission: RefCell<Option<Admission>>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
}

impl ProfileController {
    pub(super) fn new(
        owner: Weak<UserMenuController>,
        root: slint::Weak<UserMenu>,
        host: Arc<dyn DesktopHost>,
    ) -> Self {
        Self {
            owner,
            root,
            host,
            // Parent must bind its weak Root/current-popup/source-session scope.
            admission: RefCell::new(None),
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
        }
    }

    pub(super) fn bind(&self, admission: impl Fn() -> bool + 'static) {
        *self.admission.borrow_mut() = Some(Rc::new(admission));
    }

    pub(super) fn bind_callbacks(owner: &Rc<UserMenuController>) {
        let weak = Rc::downgrade(owner);
        owner.component().on_profile_event_ready(move || {
            if let Some(owner) = weak.upgrade() {
                owner.profile.drain();
            }
        });
        let weak = Rc::downgrade(owner);
        owner
            .component()
            .on_profile_open_requested(move |action, key| {
                if let Some(owner) = weak.upgrade() {
                    let command = match action {
                        UserProfileAction::Home => ProfileCommand::OpenHome,
                        UserProfileAction::Accounts => ProfileCommand::OpenAccountsSettings,
                    };
                    owner.profile.open(command, key);
                }
            });
        let weak = Rc::downgrade(owner);
        owner.component().on_onedrive_open_requested(move |key| {
            if let Some(owner) = weak.upgrade() {
                owner.profile.open_onedrive(key);
            }
        });
        let weak = Rc::downgrade(owner);
        owner.component().on_profile_retry_requested(move |key| {
            if let Some(owner) = weak.upgrade() {
                owner.profile.retry(key);
            }
        });
    }

    pub(super) fn prepare(&self) -> u64 {
        let (session, scope) = {
            let mut state = self.state.borrow_mut();
            state.retire();
            (state.session, state.scope())
        };
        self.clear();
        let bound = self.admission.borrow().is_some();
        if bound
            && self.local(scope)
            && let Some(root) = self.root.upgrade()
        {
            // Until the admitted provider reports Absent/Unavailable, Unknown
            // has no generic image masquerading as an observation.
            root.set_profile_fallback(false);
        }
        session
    }

    pub(super) fn activate(&self, session: u64) {
        let scope = self.state.borrow().scope();
        if scope.session != session || !self.admitted(scope) {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.active = true;
            state.requested = true;
            state.advance();
        }
        self.project();
        self.pump();
    }

    pub(super) fn retire(&self) -> bool {
        self.state.borrow_mut().retire();
        let scope = self.state.borrow().scope();
        self.clear();
        // A setter callback may replace the popup session on this same component.
        self.state.borrow().scope() == scope
    }

    pub(super) fn same_session(&self, session: u64) -> bool {
        self.state.borrow().session == session
    }

    fn local(&self, scope: Scope) -> bool {
        let state = self.state.borrow();
        state.scope() == scope && self.root.upgrade().is_some()
    }

    fn admitted(&self, scope: Scope) -> bool {
        if self.state.borrow().exhausted || !self.local(scope) {
            return false;
        }
        let Some(owner) = self.owner.upgrade() else {
            return false;
        };
        if !owner.is_open() {
            return false;
        }
        // Clone outside the borrow: an admission callback may itself re-enter.
        let Some(admission) = self.admission.borrow().clone() else {
            return false;
        };
        admission() && self.local(scope) && owner.is_open()
    }

    fn current(&self, scope: Scope) -> bool {
        let active = self.state.borrow().active;
        active && self.admitted(scope)
    }

    fn clear(&self) {
        let scope = self.state.borrow().scope();
        let Some(root) = self.root.upgrade() else {
            return;
        };
        macro_rules! set {
            ($call:expr) => {
                if !self.local(scope) {
                    return;
                }
                $call;
            };
        }
        // Synchronous gesture cancellation precedes all rebinding/hiding.
        set!(root.invoke_cancel_profile_input());
        set!(root.set_profile_actions_ready(false));
        set!(root.set_onedrive_ready(false));
        set!(root.set_profile_retry_enabled(false));
        set!(root.set_profile_key(SharedString::default()));
        set!(root.set_onedrive_key(SharedString::default()));
        set!(root.set_profile_retry_key(SharedString::default()));
        set!(root.set_has_photo(false));
        set!(root.set_profile_photo(Image::default()));
        set!(root.set_profile_name(SharedString::default()));
        set!(root.set_personal_email(SharedString::default()));
        set!(root.set_profile_loading(false));
        set!(root.set_profile_opening(false));
        set!(root.set_profile_status(SharedString::default()));
        set!(root.set_profile_fallback(true));
    }

    fn pump(&self) {
        let scope = self.state.borrow().scope();
        if !self.current(scope) {
            return;
        }
        let acquire = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || state.acquiring || !state.requested {
                return;
            }
            if state.provider.is_none() {
                state.acquiring = true;
                true
            } else {
                false
            }
        };
        if acquire {
            let result = self.host.profile_host();
            let current = self.current(scope);
            {
                let mut state = self.state.borrow_mut();
                state.acquiring = false;
                if current {
                    match result {
                        Ok(Some(provider)) => state.provider = Some(provider),
                        Ok(None) => {
                            state.requested = false;
                            state.notice =
                                "Profile details are not supported on this platform.".into();
                            state.retry = false;
                        }
                        Err(error) => {
                            state.requested = false;
                            state.notice = failure("Profile details unavailable", &error);
                            state.retry = true;
                        }
                    }
                    state.advance();
                }
            }
            // A late factory result is discarded, never applied/cached for a replacement session.
            let next = self.state.borrow().scope();
            if !self.current(next) {
                return;
            }
            self.project();
            self.pump();
            return;
        }
        let (provider, token) = {
            let mut state = self.state.borrow_mut();
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let Some(token) = state.token() else {
                return;
            };
            state.requested = false;
            state.retry = false;
            state.notice.clear();
            state.flight = Some(Flight {
                token,
                request: Request::Read,
            });
            state.advance();
            (provider, token)
        };
        self.mailbox.lock().expected = Some(token);
        self.project();
        let scope = self.state.borrow().scope();
        if token.session != scope.session || !self.current(scope) {
            self.cancel_unissued(token);
            return;
        }
        let mailbox = self.mailbox.clone();
        let root = self.root.clone();
        if let Err(error) = provider.read(Box::new(move |result| {
            complete(&mailbox, &root, token, Outcome::Read(result));
        })) {
            complete(&self.mailbox, &self.root, token, Outcome::Read(Err(error)));
        }
    }

    fn retry(&self, key: SharedString) {
        let scope = self.state.borrow().scope();
        if !self.current(scope) {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if !state.retry || state.busy() || state.key != key {
                return;
            }
            state.confirmed = None;
            state.notice.clear();
            state.retry = false;
            state.requested = true;
            state.advance();
        }
        self.project();
        self.pump();
    }

    fn open_onedrive(&self, key: SharedString) {
        let expected = {
            let state = self.state.borrow();
            let Some(projection) = &state.confirmed else {
                return;
            };
            if projection.key != key || !projection.onedrive_valid || state.busy() {
                return;
            }
            projection.snapshot.onedrive().cloned()
        };
        if let Some(expected) = expected {
            self.open(ProfileCommand::OpenOneDrive { expected }, key);
        }
    }

    fn open(&self, command: ProfileCommand, key: SharedString) {
        let scope = self.state.borrow().scope();
        if !self.current(scope) {
            return;
        }
        let onedrive = matches!(command, ProfileCommand::OpenOneDrive { .. });
        let (provider, token) = {
            let mut state = self.state.borrow_mut();
            if state.busy() {
                return;
            }
            let Some(projection) = &state.confirmed else {
                return;
            };
            if projection.key != key {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let Some(token) = state.token() else {
                return;
            };
            state.flight = Some(Flight {
                token,
                request: Request::Open { onedrive },
            });
            state.notice.clear();
            state.advance();
            (provider, token)
        };
        self.mailbox.lock().expected = Some(token);
        self.project();
        let scope = self.state.borrow().scope();
        if token.session != scope.session || !self.current(scope) {
            self.cancel_unissued(token);
            return;
        }
        let mailbox = self.mailbox.clone();
        let root = self.root.clone();
        if let Err(error) = provider.execute(
            command,
            Box::new(move |result| {
                complete(&mailbox, &root, token, Outcome::Open(result));
            }),
        ) {
            complete(&self.mailbox, &self.root, token, Outcome::Open(Err(error)));
        }
    }

    fn cancel_unissued(&self, token: Token) {
        {
            let mut state = self.state.borrow_mut();
            if state
                .flight
                .as_ref()
                .is_some_and(|flight| flight.token == token)
            {
                state.flight = None;
            }
        }
        {
            let mut mailbox = self.mailbox.lock();
            if mailbox.expected == Some(token) {
                mailbox.expected = None;
            }
        }
        self.project();
        self.pump();
    }

    fn drain(&self) {
        let completion = {
            let mut mailbox = self.mailbox.lock();
            mailbox.wake_queued = false;
            let completion = mailbox.completion.take();
            if completion.is_some() {
                mailbox.expected = None;
            }
            completion
        };
        let scope = self.state.borrow().scope();
        let current = self.current(scope);
        {
            let mut state = self.state.borrow_mut();
            if let Some(completion) = completion
                && state
                    .flight
                    .as_ref()
                    .is_some_and(|flight| flight.token == completion.token)
            {
                let flight = state.flight.take().expect("matching profile flight");
                if current && completion.token.session == state.session {
                    match (flight.request, completion.outcome) {
                        (Request::Read, Outcome::Read(Ok(snapshot))) => {
                            state.advance();
                            let image = match snapshot.photo() {
                                ProfilePhotoState::Ready(photo) => {
                                    super::prepare_profile_photo(photo)
                                }
                                _ => Image::default(),
                            };
                            let onedrive_valid = snapshot.onedrive().is_some();
                            state.retry =
                                matches!(snapshot.photo(), ProfilePhotoState::Unavailable(_));
                            state.confirmed = Some(Projection {
                                snapshot,
                                image,
                                key: state.key.clone(),
                                onedrive_valid,
                            });
                            state.notice.clear();
                        }
                        (Request::Read, Outcome::Read(Err(error))) => {
                            state.confirmed = None;
                            state.notice = failure("Could not read profile", &error);
                            state.retry = true;
                            state.advance();
                        }
                        (Request::Open { .. }, Outcome::Open(Ok(()))) => {
                            state.notice = "Profile open request accepted.".into();
                            state.advance();
                        }
                        (Request::Open { onedrive }, Outcome::Open(Err(error))) => {
                            state.notice = failure("Could not open profile destination", &error);
                            state.retry = true;
                            state.advance();
                            // Retire only the failed intent, not genuine name/email/photo.
                            let key = state.key.clone();
                            if let Some(projection) = &mut state.confirmed {
                                if onedrive {
                                    projection.onedrive_valid = false;
                                }
                                projection.key = key;
                            }
                        }
                        _ => unreachable!("typed profile completion matches request"),
                    }
                }
            }
        }
        self.project();
        self.pump();
    }

    fn project(&self) {
        let view = {
            let state = self.state.borrow();
            let busy = state.busy();
            let projection = state.confirmed.as_ref();
            let has_photo = projection
                .is_some_and(|p| matches!(p.snapshot.photo(), ProfilePhotoState::Ready(_)));
            let loading = state.requested
                || state.acquiring
                || state
                    .flight
                    .as_ref()
                    .is_some_and(|f| matches!(f.request, Request::Read));
            let opening = state
                .flight
                .as_ref()
                .is_some_and(|f| matches!(f.request, Request::Open { .. }));
            let name = projection
                .map(|p| bounded_text(p.snapshot.display_name(), 128))
                .unwrap_or_default();
            let email = projection
                .and_then(|p| p.snapshot.personal_email())
                .map(|s| bounded_text(s, 320))
                .unwrap_or_default();
            let notice = if !state.notice.is_empty() {
                state.notice.clone()
            } else if let Some(p) = projection {
                match p.snapshot.photo() {
                    ProfilePhotoState::Unavailable(kind) => {
                        format!("Profile photo unavailable: {}.", status(*kind))
                    }
                    _ => String::new(),
                }
            } else {
                String::new()
            };
            View {
                scope: state.scope(),
                name,
                email,
                image: projection.map(|p| p.image.clone()).unwrap_or_default(),
                has_photo,
                fallback: !has_photo && !loading,
                loading,
                opening,
                ready: projection.is_some() && !busy,
                onedrive: projection.is_some_and(|p| p.onedrive_valid) && !busy,
                action_key: projection.map(|p| p.key.clone()).unwrap_or_default(),
                retry: state.retry && !busy,
                retry_key: state.key.clone(),
                notice,
            }
        };
        let Some(root) = self.root.upgrade() else {
            return;
        };
        macro_rules! set {
            ($call:expr) => {
                if !self.current(view.scope) {
                    return;
                }
                $call;
            };
        }
        set!(root.invoke_cancel_profile_input());
        set!(root.set_profile_actions_ready(false));
        set!(root.set_onedrive_ready(false));
        set!(root.set_profile_retry_enabled(false));
        set!(root.set_has_photo(false));
        set!(root.set_profile_photo(view.image));
        set!(root.set_profile_fallback(view.fallback));
        set!(root.set_profile_name(view.name.into()));
        set!(root.set_personal_email(view.email.into()));
        set!(root.set_profile_loading(view.loading));
        set!(root.set_profile_opening(view.opening));
        set!(root.set_profile_status(view.notice.into()));
        set!(root.set_profile_key(view.action_key));
        set!(root.set_onedrive_key(if view.onedrive {
            root.get_profile_key()
        } else {
            SharedString::default()
        }));
        set!(root.set_profile_retry_key(view.retry_key));
        set!(root.set_has_photo(view.has_photo));
        set!(root.set_profile_actions_ready(view.ready));
        set!(root.set_onedrive_ready(view.onedrive));
        set!(root.set_profile_retry_enabled(view.retry));
        // Never rebuild the folder model or replay native show/focus/attachment.
        if self.current(view.scope)
            && let Some(owner) = self.owner.upgrade()
        {
            let _ = owner.refit();
        }
    }
}

fn status(kind: ProfileErrorKind) -> &'static str {
    match kind {
        ProfileErrorKind::Unsupported => "Not supported",
        ProfileErrorKind::AccessDenied => "Access denied",
        ProfileErrorKind::NotFound => "Not found",
        ProfileErrorKind::InvalidData => "Invalid data",
        ProfileErrorKind::Busy => "Profile service is busy",
        ProfileErrorKind::Other => "Unavailable",
    }
}

fn failure(context: &str, error: &ProfileError) -> String {
    // No raw provider message, paths, SID or personal email in status/diagnostics.
    format!("{context}: {}.", status(error.kind()))
}
