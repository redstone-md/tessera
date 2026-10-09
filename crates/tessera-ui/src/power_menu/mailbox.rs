// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::{LockResult, PowerMenuSurface, PowerUpdateHint, PowerUpdatesError, ReadResult, Token};
use parking_lot::Mutex;
use std::sync::Arc;

type UpdatesResult = Result<PowerUpdateHint, PowerUpdatesError>;

pub(super) struct Slot<T> {
    pub(super) expected: Option<Token>,
    pub(super) terminal: Option<(Token, T)>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            expected: None,
            terminal: None,
        }
    }
}

impl<T> Slot<T> {
    fn complete(&mut self, token: Token, result: T) -> bool {
        if self.expected != Some(token) || self.terminal.is_some() {
            return false;
        }
        self.terminal = Some((token, result));
        true
    }

    pub(super) fn take(&mut self) -> Option<(Token, T)> {
        let terminal = self.terminal.take();
        if terminal.is_some() {
            self.expected = None;
        }
        terminal
    }

    pub(super) fn clear(&mut self, token: Token) {
        if self.expected == Some(token) {
            self.expected = None;
            self.terminal = None;
        }
    }
}

#[derive(Clone)]
struct WakeTarget {
    epoch: u64,
    revision: u64,
    root: slint::Weak<PowerMenuSurface>,
}

// Three independent, first-terminal-wins slots. Workers own only this bounded
// Send mailbox, never a strong generated component, actor or root controller.
#[derive(Default)]
pub(super) struct Mailbox {
    pub(super) read: Slot<ReadResult>,
    pub(super) lock: Slot<LockResult>,
    pub(super) updates: Slot<UpdatesResult>,
    pub(super) wake_queued: bool,
    pub(super) command_wake_queued: bool,
    pub(super) command_target: Option<slint::Weak<crate::generated::Panel>>,
    revision: u64,
    target: Option<WakeTarget>,
}

impl Mailbox {
    pub(super) fn install(
        &mut self,
        epoch: u64,
        root: slint::Weak<PowerMenuSurface>,
    ) -> Result<(), String> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or("Power menu wake identity is exhausted.")?;
        self.revision = revision;
        self.target = Some(WakeTarget {
            epoch,
            revision,
            root,
        });
        self.wake_queued = false;
        Ok(())
    }

    pub(super) fn retire(&mut self, epoch: u64) {
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.epoch == epoch)
        {
            self.target = None;
            self.wake_queued = false;
        }
    }

    pub(super) fn close(&mut self) {
        self.target = None;
        self.wake_queued = false;
        self.command_target = None;
        self.command_wake_queued = false;
        self.read = Slot::default();
        self.lock = Slot::default();
        self.updates = Slot::default();
    }

    pub(super) fn pending(&self) -> bool {
        self.read.terminal.is_some()
            || self.lock.terminal.is_some()
            || self.updates.terminal.is_some()
    }
}

// A failed enqueue for retired A must not clear an already queued wake for B.
pub(super) fn wake_failed(mailbox: &Arc<Mutex<Mailbox>>, revision: u64) {
    let mut mailbox = mailbox.lock();
    if mailbox
        .target
        .as_ref()
        .is_some_and(|target| target.revision == revision)
    {
        mailbox.wake_queued = false;
    }
}

pub(super) fn wake(mailbox: &Arc<Mutex<Mailbox>>) {
    let target = {
        let mut mailbox = mailbox.lock();
        if mailbox.wake_queued || !mailbox.pending() {
            return;
        }
        let Some(target) = mailbox.target.clone() else {
            return;
        };
        mailbox.wake_queued = true;
        target
    };
    if target
        .root
        .upgrade_in_event_loop(|root| root.invoke_power_event_ready())
        .is_err()
    {
        wake_failed(mailbox, target.revision);
    }
}

pub(super) fn complete_read(mailbox: &Arc<Mutex<Mailbox>>, token: Token, result: ReadResult) {
    let delivered = mailbox.lock().read.complete(token, result);
    if delivered {
        wake(mailbox);
    }
}

fn wake_command(mailbox: &Arc<Mutex<Mailbox>>) {
    let target = {
        let mut state = mailbox.lock();
        if state.command_wake_queued || state.lock.terminal.is_none() {
            return;
        }
        let Some(target) = state.command_target.clone() else {
            drop(state);
            wake(mailbox);
            return;
        };
        state.command_wake_queued = true;
        target
    };
    if target
        .upgrade_in_event_loop(|panel| panel.invoke_power_command_event_ready())
        .is_err()
    {
        mailbox.lock().command_wake_queued = false;
    }
}

pub(super) fn complete_lock(mailbox: &Arc<Mutex<Mailbox>>, token: Token, result: LockResult) {
    let delivered = mailbox.lock().lock.complete(token, result);
    if delivered {
        wake_command(mailbox);
    }
}

pub(super) fn complete_updates(mailbox: &Arc<Mutex<Mailbox>>, token: Token, result: UpdatesResult) {
    let delivered = mailbox.lock().updates.complete(token, result);
    if delivered {
        wake(mailbox);
    }
}

#[cfg(test)]
pub(super) fn revision(mailbox: &Arc<Mutex<Mailbox>>) -> u64 {
    mailbox.lock().revision
}

#[cfg(test)]
pub(super) fn exhaust_revision(mailbox: &Arc<Mutex<Mailbox>>) {
    mailbox.lock().revision = u64::MAX;
}
