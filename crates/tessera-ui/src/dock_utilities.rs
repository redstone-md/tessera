// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One explicit Shell desktop-toggle intent, independent of application observation.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::ComponentHandle;
use tessera_system::dock_utilities::{DockUtilitiesHost, DockUtilityErrorKind};

use crate::DesktopHost;
use crate::generated::{Dock, DockReservedAction};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq)]
struct RequestToken(u64);

#[derive(Default)]
struct State {
    sequence: u64,
    provider: Option<Arc<dyn DockUtilitiesHost>>,
    flight: Option<RequestToken>,
}

struct Completion {
    token: RequestToken,
    result: Result<(), DockUtilityErrorKind>,
}

/// One accepted intent owns one slot; duplicates never queue a follow-up toggle.
#[derive(Default)]
struct Mailbox {
    expected: Option<RequestToken>,
    completion: Option<Completion>,
    wake_queued: bool,
}

fn complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    dock: &slint::Weak<Dock>,
    token: RequestToken,
    result: Result<(), DockUtilityErrorKind>,
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
    let weak_mailbox = Arc::downgrade(mailbox);
    if dock
        .upgrade_in_event_loop(move |dock| {
            let Some(mailbox) = weak_mailbox.upgrade() else {
                return;
            };
            // Closing between completion and delivery must not revive a root callback.
            let current = {
                let mailbox = mailbox.lock();
                mailbox.expected == Some(token)
                    && mailbox
                        .completion
                        .as_ref()
                        .is_some_and(|c| c.token == token)
            };
            if current {
                dock.invoke_utility_event_ready();
            }
        })
        .is_err()
    {
        // Portable tests may invoke this exact production wake without an event loop.
        mailbox.lock().wake_queued = false;
    }
}

struct Guard<'a>(&'a Cell<bool>);

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub(crate) struct DockUtilitiesController {
    dock: slint::Weak<Dock>,
    host: Arc<dyn DesktopHost>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
    entering: Cell<bool>,
    closed: Cell<bool>,
}

impl DockUtilitiesController {
    /// Root wiring owns callbacks; construction neither acquires a capability nor a window.
    pub(crate) fn new(host: Arc<dyn DesktopHost>, dock: &Dock) -> Rc<Self> {
        Rc::new(Self {
            dock: dock.as_weak(),
            host,
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            entering: Cell::new(false),
            closed: Cell::new(false),
        })
    }

    pub(crate) fn request(self: &Rc<Self>, action: DockReservedAction) {
        if action != DockReservedAction::ShowDesktop || self.closed.get() || self.entering.get() {
            return;
        }
        let Some(dock) = self.dock.upgrade() else {
            return;
        };
        if !dock.window().is_visible() {
            return;
        }
        self.entering.set(true);
        let _guard = Guard(&self.entering);
        let (token, provider) = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() {
                return;
            }
            // Exhaustion rejects new intents rather than recycling an old authority token.
            let Some(sequence) = state.sequence.checked_add(1) else {
                return;
            };
            state.sequence = sequence;
            let token = RequestToken(sequence);
            state.flight = Some(token);
            (token, state.provider.clone())
        };
        {
            let mut mailbox = self.mailbox.lock();
            mailbox.expected = Some(token);
            mailbox.completion = None;
            mailbox.wake_queued = false;
        }
        dock.set_show_desktop_busy(true);
        dock.set_show_desktop_notice("Desktop toggle request pending.".into());

        // Factory and dispatch may reenter. Both flight and mailbox already own this intent;
        // no state borrow or lock crosses a host call, and its callback holds only a weak Dock.
        let provider = match provider {
            Some(provider) => provider,
            None => match self
                .host
                .dock_utilities_host()
                .map_err(|error| error.kind)
                .and_then(|provider| provider.ok_or(DockUtilityErrorKind::Unsupported))
            {
                Ok(provider) => {
                    if !self.current(token) {
                        return;
                    }
                    self.state.borrow_mut().provider = Some(provider.clone());
                    provider
                }
                Err(kind) => {
                    complete(&self.mailbox, &self.dock, token, Err(kind));
                    return;
                }
            },
        };
        if !self.current(token) {
            return;
        }
        let mailbox = self.mailbox.clone();
        let weak_dock = self.dock.clone();
        if let Err(error) = provider.toggle_desktop(Box::new(move |result| {
            complete(
                &mailbox,
                &weak_dock,
                token,
                result.map_err(|error| error.kind),
            );
        })) {
            complete(&self.mailbox, &self.dock, token, Err(error.kind));
        }
    }

    /// Called only by the root's utility-event-ready wake; results are safe for root reporting.
    pub(crate) fn process_events(&self) -> Option<Result<(), String>> {
        if self.closed.get() || self.entering.replace(true) {
            return None;
        }
        let _guard = Guard(&self.entering);
        let completion = {
            let mut mailbox = self.mailbox.lock();
            mailbox.wake_queued = false;
            let completion = mailbox.completion.take()?;
            mailbox.expected = None;
            completion
        };
        {
            let mut state = self.state.borrow_mut();
            if state.flight != Some(completion.token) {
                return None;
            }
            state.flight = None;
        }
        let result = completion.result.map_err(|kind| failure(kind).to_owned());
        if let Some(dock) = self.dock.upgrade() {
            // Visibility changes never cancel or resubmit an already accepted native action.
            dock.set_show_desktop_busy(false);
            dock.set_show_desktop_notice(match &result {
                Ok(()) => "Desktop toggle requested.".into(),
                Err(message) => message.as_str().into(),
            });
        }
        Some(result)
    }

    /// Retire presentation only: accepted native work still drains, without a GUI-thread join.
    pub(crate) fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        let provider = {
            let mut state = self.state.borrow_mut();
            state.flight = None;
            state.provider.take()
        };
        *self.mailbox.lock() = Mailbox::default();
        if let Some(dock) = self.dock.upgrade() {
            dock.set_show_desktop_busy(false);
            dock.set_show_desktop_notice("".into());
        }
        drop(provider);
    }

    fn current(&self, token: RequestToken) -> bool {
        !self.closed.get() && self.state.borrow().flight == Some(token)
    }
}

impl Drop for DockUtilitiesController {
    fn drop(&mut self) {
        self.close();
    }
}

fn failure(kind: DockUtilityErrorKind) -> &'static str {
    match kind {
        DockUtilityErrorKind::Unsupported => "Show desktop is not supported by this host.",
        DockUtilityErrorKind::AccessDenied => "Desktop toggle was denied.",
        DockUtilityErrorKind::Unavailable => "Desktop toggle is unavailable. Try again.",
        DockUtilityErrorKind::Busy => "Desktop toggle provider is busy. Try again.",
        DockUtilityErrorKind::Stopped => "Desktop toggle provider has stopped. Try again.",
        DockUtilityErrorKind::Other => "Desktop toggle could not be requested.",
    }
}
