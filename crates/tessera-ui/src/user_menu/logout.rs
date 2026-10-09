// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! User input authority only. Execution and the shared command flight live in Power.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use slint::{ComponentHandle, SharedString};

use crate::generated::UserMenu;

#[cfg(test)]
mod tests;

/// Actor-local checked identity. Callers cannot manufacture a presentation session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UserLogoutSession(u64);

/// Issued only by the bound, current generated User input callback, never by a label.
#[derive(Clone)]
pub(crate) struct UserLogoutIntent {
    issuer: Weak<()>,
    session: UserLogoutSession,
    issuance: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepared,
    Active,
    Retired,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct View {
    session: Option<UserLogoutSession>,
    phase: Phase,
    key: Option<u64>,
    issued: Option<u64>,
    busy: bool,
    exhausted: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            session: None,
            phase: Phase::Retired,
            key: None,
            issued: None,
            busy: false,
            exhausted: false,
        }
    }
}

#[derive(Default)]
struct State {
    view: View,
    sessions: u64,
    keys: u64,
    issuances: u64,
}

impl State {
    fn exhaust(&mut self) {
        self.view.exhausted = true;
        self.view.phase = Phase::Retired;
        self.view.key = None;
        self.view.issued = None;
    }

    fn rotate_key(&mut self) {
        if let Some(next) = self.keys.checked_add(1) {
            self.keys = next;
            self.view.key = Some(next);
        } else {
            self.exhaust();
        }
    }

    fn retire(&mut self) -> View {
        self.view.phase = Phase::Retired;
        self.view.key = None;
        // Retain the issued receipt for exact post-hide validation. Prepare,
        // not a repeated hide or busy projection, ends that retirement identity.
        self.view
    }
}

type Admission = Rc<dyn Fn() -> bool>;
type Request = Rc<dyn Fn(UserLogoutIntent)>;

#[derive(Clone)]
struct Binding {
    current: Admission,
    request: Request,
}

pub(crate) struct UserLogoutController {
    root: slint::Weak<UserMenu>,
    identity: Rc<()>,
    binding: RefCell<Option<Binding>>,
    state: RefCell<State>,
}

impl UserLogoutController {
    pub(crate) fn new(root: &UserMenu) -> Rc<Self> {
        let actor = Rc::new(Self {
            root: root.as_weak(),
            identity: Rc::new(()),
            binding: RefCell::new(None),
            state: RefCell::default(),
        });
        let weak = Rc::downgrade(&actor);
        root.on_logout_requested(move |key| {
            if let Some(actor) = weak.upgrade() {
                actor.request(key);
            }
        });
        actor
    }

    /// One-time weak Root binding; no provider acquisition or action occurs here.
    pub(crate) fn bind_root(
        &self,
        current: impl Fn() -> bool + 'static,
        request: impl Fn(UserLogoutIntent) + 'static,
    ) -> bool {
        let mut binding = self.binding.borrow_mut();
        if binding.is_some() {
            return false;
        }
        *binding = Some(Binding {
            current: Rc::new(current),
            request: Rc::new(request),
        });
        true
    }

    /// Revoke/cancel the old gesture before a replacement session is projected.
    pub(crate) fn prepare(&self) -> Option<UserLogoutSession> {
        let retired = self.state.borrow_mut().retire();
        self.project(retired);
        if !self.local(retired) || retired.exhausted {
            return None;
        }
        let view = {
            let mut state = self.state.borrow_mut();
            if let Some(next) = state.sessions.checked_add(1) {
                state.sessions = next;
                state.view.session = Some(UserLogoutSession(next));
                state.view.phase = Phase::Prepared;
                state.view.issued = None;
            } else {
                state.exhaust();
            }
            state.view
        };
        self.project(view);
        (!view.exhausted && self.local(view))
            .then_some(view.session)
            .flatten()
    }

    /// Only after genuine native presentation and the already-bound Root admission.
    pub(crate) fn activate(&self, session: UserLogoutSession) -> bool {
        let view = self.state.borrow().view;
        if view.session != Some(session) || view.phase != Phase::Prepared || view.exhausted {
            return false;
        }
        let binding = self.binding.borrow().clone();
        let Some(binding) = binding else {
            return false;
        };
        if !(binding.current)() || !self.local(view) || !self.native_visible() {
            return false;
        }
        let view = {
            let mut state = self.state.borrow_mut();
            state.view.phase = Phase::Active;
            state.rotate_key();
            state.view
        };
        self.project(view);
        self.local(view) && !view.exhausted
    }

    /// Call before profile setters/native lease release; caller guards this session
    /// again after every callback-capable retirement stage, before touching a replacement.
    pub(crate) fn retire(&self) -> Option<UserLogoutSession> {
        let view = self.state.borrow_mut().retire();
        self.project(view);
        view.session
    }

    pub(crate) fn session_current(&self, session: UserLogoutSession) -> bool {
        let view = self.state.borrow().view;
        view.session == Some(session) && view.phase != Phase::Retired && !view.exhausted
    }

    /// Pure cleanup identity, including exhausted input, usable before native hide.
    /// Intent validation below separately rejects exhaustion and requires native hide.
    pub(crate) fn session_retired(&self, session: UserLogoutSession) -> bool {
        let view = self.state.borrow().view;
        view.session == Some(session) && view.phase == Phase::Retired && self.local(view)
    }

    #[cfg(test)]
    pub(crate) fn exhaust_counter_for_test(&self, counter: &'static str) {
        let mut state = self.state.borrow_mut();
        match counter {
            "issuance" => state.issuances = u64::MAX,
            "keys" => state.keys = u64::MAX,
            "sessions" => state.sessions = u64::MAX,
            _ => panic!("unknown Logout recording counter"),
        }
    }

    /// Busy blocks fresh input, not the captured receipt reserved by Power.
    pub(crate) fn set_busy(&self, busy: bool) {
        let view = {
            let mut state = self.state.borrow_mut();
            if state.view.busy == busy {
                return;
            }
            state.view.busy = busy;
            if state.view.phase == Phase::Active {
                state.rotate_key();
            }
            state.view
        };
        self.project(view);
    }

    pub(crate) fn is_current(&self, intent: &UserLogoutIntent) -> bool {
        let view = self.state.borrow().view;
        if view.phase != Phase::Active || !self.matches(intent, view) {
            return false;
        }
        let binding = self.binding.borrow().clone();
        binding
            .is_some_and(|binding| (binding.current)() && self.local(view) && self.native_visible())
    }

    pub(crate) fn is_exact_retired(&self, intent: &UserLogoutIntent) -> bool {
        let view = self.state.borrow().view;
        view.phase == Phase::Retired
            && self.matches(intent, view)
            && self.native_hidden()
            && self.local(view)
    }

    fn matches(&self, intent: &UserLogoutIntent, view: View) -> bool {
        !view.exhausted
            && Weak::ptr_eq(&intent.issuer, &Rc::downgrade(&self.identity))
            && view.session == Some(intent.session)
            && view.issued == Some(intent.issuance)
    }

    fn local(&self, view: View) -> bool {
        self.state.borrow().view == view && self.root.upgrade().is_some()
    }

    fn native_visible(&self) -> bool {
        self.root
            .upgrade()
            .is_some_and(|root| root.window().is_visible())
    }

    fn native_hidden(&self) -> bool {
        self.root
            .upgrade()
            .is_some_and(|root| !root.window().is_visible())
    }

    fn request(&self, key: SharedString) {
        let view = self.state.borrow().view;
        if view.phase != Phase::Active
            || view.busy
            || view.exhausted
            || view.key.is_none()
            || Self::key(view) != key
        {
            return;
        }
        // No actor/binding borrow crosses the source predicate or typed request sink.
        let binding = self.binding.borrow().clone();
        let Some(binding) = binding else {
            return;
        };
        if !(binding.current)() || !self.local(view) || !self.native_visible() {
            return;
        }
        let intent = {
            let mut state = self.state.borrow_mut();
            if let Some(next) = state.issuances.checked_add(1) {
                state.issuances = next;
                state.view.issued = Some(next);
                Some(UserLogoutIntent {
                    issuer: Rc::downgrade(&self.identity),
                    session: view.session.expect("active session"),
                    issuance: next,
                })
            } else {
                state.exhaust();
                None
            }
        };
        if let Some(intent) = intent {
            (binding.request)(intent);
        } else {
            let view = self.state.borrow().view;
            self.project(view);
        }
    }

    fn key(view: View) -> SharedString {
        match (view.session, view.key) {
            (Some(session), Some(key)) => format!("user-logout:{}:{key}", session.0).into(),
            _ => SharedString::default(),
        }
    }

    fn project(&self, view: View) {
        let Some(root) = self.root.upgrade() else {
            return;
        };
        macro_rules! set {
            ($call:expr) => {
                if !self.local(view) {
                    return;
                }
                $call;
            };
        }
        // Revocation already happened in State. Synchronous cancellation always
        // precedes readiness/key setters, including transient busy false→true→false.
        set!(root.invoke_cancel_logout_input());
        set!(root.set_logout_ready(false));
        set!(root.set_logout_key(Self::key(view)));
        set!(root.set_logout_busy(view.busy));
        set!(root.set_logout_ready(
            view.phase == Phase::Active && view.key.is_some() && !view.busy && !view.exhausted,
        ));
    }
}
