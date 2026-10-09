// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Watch lifetime is independent of the Power presentation's 30-second retirement.
//! Native callbacks enqueue one ready result and one coalesced dirty hint only.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use parking_lot::Mutex;
use tessera_system::display_context::{
    DisplayContextError, DisplayContextHost, DisplayContextWatchEvent, DisplayContextWatchGuard,
};

use super::PanelController;
use crate::generated::Panel;
use crate::power_menu::PowerMenuController;

#[derive(Default)]
struct Mailbox {
    closed: bool,
    wake_queued: bool,
    ready: Option<Result<(), DisplayContextError>>,
    started: bool,
    dirty: bool,
    terminal: Option<DisplayContextError>,
}

fn publish(
    mailbox: &Arc<Mutex<Mailbox>>,
    panel: &slint::Weak<Panel>,
    update: impl FnOnce(&mut Mailbox),
) {
    let post = {
        let mut mailbox = mailbox.lock();
        if mailbox.closed {
            return;
        }
        update(&mut mailbox);
        if mailbox.wake_queued {
            false
        } else {
            mailbox.wake_queued = true;
            true
        }
    };
    if post
        && panel
            .upgrade_in_event_loop(|panel| panel.invoke_power_display_event_ready())
            .is_err()
    {
        mailbox.lock().wake_queued = false;
    }
}

pub(super) struct PowerDisplayWatch {
    actor: Weak<PowerMenuController>,
    mailbox: Arc<Mutex<Mailbox>>,
    guard: RefCell<Option<Box<dyn DisplayContextWatchGuard>>>,
}

impl PowerDisplayWatch {
    fn new(
        panel: slint::Weak<Panel>,
        actor: &Rc<PowerMenuController>,
        provider: Arc<dyn DisplayContextHost>,
    ) -> Rc<Self> {
        let watch = Rc::new(Self {
            actor: Rc::downgrade(actor),
            mailbox: Arc::new(Mutex::default()),
            guard: RefCell::default(),
        });
        let events_mailbox = watch.mailbox.clone();
        let events_panel = panel.clone();
        let ready_mailbox = watch.mailbox.clone();
        let result = provider.watch(
            Arc::new(move |event| {
                publish(&events_mailbox, &events_panel, |mailbox| match event {
                    DisplayContextWatchEvent::Changed => mailbox.dirty = true,
                    DisplayContextWatchEvent::Unavailable(error) => mailbox.terminal = Some(error),
                })
            }),
            Box::new(move |result| {
                publish(&ready_mailbox, &panel, |mailbox| {
                    if !mailbox.started && mailbox.ready.is_none() {
                        mailbox.ready = Some(result);
                    }
                })
            }),
        );
        match result {
            Ok(guard) => *watch.guard.borrow_mut() = Some(guard),
            Err(_) => watch.close(), // Unsupported is silent; independent reads still work.
        }
        watch
    }

    fn matches(&self, actor: &Rc<PowerMenuController>) -> bool {
        self.actor
            .upgrade()
            .is_some_and(|current| Rc::ptr_eq(&current, actor))
    }

    fn active(&self) -> bool {
        !self.mailbox.lock().closed
    }

    /// Terminal watch failure never fabricates an empty layout or disables reads.
    fn take_dirty(&self) -> bool {
        let (dirty, terminal) = {
            let mut mailbox = self.mailbox.lock();
            mailbox.wake_queued = false;
            if mailbox.closed {
                return false;
            }
            let mut failed = false;
            if let Some(ready) = mailbox.ready.take() {
                match ready {
                    Ok(()) => mailbox.started = true,
                    Err(_) => failed = true,
                }
            }
            let terminal = failed || mailbox.terminal.take().is_some();
            let dirty = mailbox.started && std::mem::take(&mut mailbox.dirty) && !terminal;
            (dirty, terminal)
        };
        if terminal {
            self.close();
        }
        dirty
    }

    pub(super) fn close(&self) {
        {
            let mut mailbox = self.mailbox.lock();
            mailbox.closed = true;
            mailbox.dirty = false;
            mailbox.ready = None;
            mailbox.terminal = None;
        }
        let guard = self.guard.borrow_mut().take();
        drop(guard);
    }
}

impl Drop for PowerDisplayWatch {
    fn drop(&mut self) {
        self.close();
    }
}

impl PanelController {
    pub(super) fn start_power_display_watch(
        &self,
        actor: &Rc<PowerMenuController>,
        explicit_open: bool,
    ) {
        if self.power_admission_closed.get() {
            return;
        }
        let cached = self.power_menu.borrow().clone();
        if cached
            .as_ref()
            .is_none_or(|cached| !Rc::ptr_eq(cached, actor))
        {
            return;
        }
        let existing = self.power_display.borrow().clone();
        if existing
            .as_ref()
            .is_some_and(|watch| watch.matches(actor) && (watch.active() || !explicit_open))
        {
            return;
        }
        let Some(provider) = actor.display_provider() else {
            return;
        };
        let watch = PowerDisplayWatch::new(self.panel.clone(), actor, provider);
        let cached = self.power_menu.borrow().clone();
        if self.power_admission_closed.get()
            || cached
                .as_ref()
                .is_none_or(|cached| !Rc::ptr_eq(cached, actor))
        {
            watch.close();
            return;
        }
        // An external watch factory may reenter and publish a newer relay.
        let replacement = self.power_display.borrow().clone();
        let unchanged = match (&existing, &replacement) {
            (None, None) => true,
            (Some(old), Some(current)) => Rc::ptr_eq(old, current),
            _ => false,
        };
        if !unchanged {
            watch.close();
            return;
        }
        let retired = self.power_display.borrow_mut().replace(watch);
        if let Some(retired) = retired {
            retired.close();
        }
    }

    pub(super) fn power_display_event_ready(&self) {
        if self.power_admission_closed.get() {
            return;
        }
        let watch = self.power_display.borrow().clone();
        let actor = self.power_menu.borrow().clone();
        let (Some(watch), Some(actor)) = (watch, actor) else {
            return;
        };
        if !watch.matches(&actor) || !watch.take_dirty() || self.power_admission_closed.get() {
            return;
        }
        let current = self.power_menu.borrow().clone();
        let relay = self.power_display.borrow().clone();
        if current
            .as_ref()
            .is_some_and(|current| Rc::ptr_eq(current, &actor))
            && relay
                .as_ref()
                .is_some_and(|current| Rc::ptr_eq(current, &watch))
        {
            actor.refresh_display();
        }
    }
}
