// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use tessera_system::shortcuts::{
    ShortcutError, ShortcutErrorKind, ShortcutEvent, ShortcutSnapshot,
};

pub(super) type Wake = Arc<dyn Fn() + Send + Sync>;
pub(super) type Outcome = Result<ShortcutSnapshot, ShortcutError>;
const EVENT_LIMIT: usize = 8;

#[derive(Default)]
pub(super) struct Mailbox {
    pub alive: bool,
    pub expected: Option<u64>,
    pub source: u64,
    pub completion: Option<(u64, Outcome)>,
    pub generation: Option<u64>,
    pub events: VecDeque<ShortcutEvent>,
    pub wake_queued: bool,
}

impl Mailbox {
    pub fn retire_delivery(&mut self) {
        self.generation = None;
        self.events.clear();
    }

    pub fn close(&mut self) {
        self.alive = false;
        self.expected = None;
        self.completion = None;
        self.retire_delivery();
    }

    fn wake(&mut self) -> bool {
        if self.wake_queued {
            false
        } else {
            self.wake_queued = true;
            true
        }
    }
}

pub(super) fn complete(mailbox: &Arc<Mutex<Mailbox>>, wake: &Wake, token: u64, result: Outcome) {
    let notify = {
        let mut state = mailbox.lock();
        if !state.alive || state.expected != Some(token) || state.completion.is_some() {
            return;
        }
        state.completion = Some((token, result));
        state.wake()
    };
    if notify {
        wake();
    }
}

pub(super) fn event(mailbox: &Arc<Mutex<Mailbox>>, wake: &Wake, source: u64, event: ShortcutEvent) {
    let notify = {
        let mut state = mailbox.lock();
        if !state.alive || state.source != source {
            return;
        }
        match &event {
            ShortcutEvent::Triggered(trigger) if state.generation != Some(trigger.generation) => {
                return;
            }
            ShortcutEvent::Unavailable(_) if state.generation.is_none() => return,
            _ => {}
        }
        if matches!(&event, ShortcutEvent::Unavailable(_)) {
            // Async parent lookups must fail authority checks immediately on a
            // provider fault, even before the UI event loop drains this status.
            state.retire_delivery();
        }
        if state.events.len() == EVENT_LIMIT {
            // Saturation cannot replay a gesture later. Retire delivery and
            // report one safe fault; storage and scheduled wakes stay bounded.
            state.retire_delivery();
            state
                .events
                .push_back(ShortcutEvent::Unavailable(ShortcutError::new(
                    ShortcutErrorKind::Busy,
                    None,
                    "Shortcut delivery is busy.",
                )));
        } else {
            state.events.push_back(event);
        }
        state.wake()
    };
    if notify {
        wake();
    }
}

pub(super) fn notify(mailbox: &Arc<Mutex<Mailbox>>, wake: &Wake) {
    let notify = {
        let mut state = mailbox.lock();
        state.alive && state.wake()
    };
    if notify {
        wake();
    }
}
