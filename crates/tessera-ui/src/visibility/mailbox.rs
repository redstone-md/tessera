// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use parking_lot::Mutex;
use std::sync::Arc;
use tessera_system::visibility::{
    PhysicalPoint, PointerEnvironment, PointerWatchError, PointerWatchEvent,
};

pub(super) type Wake = Arc<dyn Fn() -> Result<(), ()> + Send + Sync + 'static>;

#[derive(Default)]
pub(super) struct Mailbox {
    pub epoch: u64,
    pub live: bool,
    pub wake: Option<Wake>,
    pub queued: bool,
    pub ready: Option<Result<(), PointerWatchError>>,
    ready_received: bool,
    pub position: Option<(Option<u64>, PhysicalPoint)>,
    pub geometry_revision: Option<u64>,
    pub environment: Option<PointerEnvironment>,
    pub failure: Option<PointerWatchError>,
}

impl Mailbox {
    pub fn activate(&mut self, epoch: u64) {
        self.epoch = epoch;
        self.live = true;
        self.queued = false;
        self.ready = None;
        self.ready_received = false;
        self.position = None;
        self.environment = None;
        self.failure = None;
    }

    pub fn retire(&mut self) {
        self.live = false;
        self.queued = false;
        self.ready = None;
        self.position = None;
        self.environment = None;
        self.failure = None;
    }
}

pub(super) fn ready(
    mailbox: &Arc<Mutex<Mailbox>>,
    epoch: u64,
    result: Result<(), PointerWatchError>,
) {
    publish(mailbox, epoch, |slot| {
        if !slot.ready_received {
            slot.ready_received = true;
            slot.ready = Some(result);
        }
    });
}

pub(super) fn event(mailbox: &Arc<Mutex<Mailbox>>, epoch: u64, event: PointerWatchEvent) {
    publish(mailbox, epoch, |slot| match event {
        PointerWatchEvent::Position(point) if slot.failure.is_none() => {
            slot.position = Some((slot.geometry_revision, point));
        }
        PointerWatchEvent::Environment(environment) if slot.failure.is_none() => {
            slot.environment = Some(environment);
        }
        PointerWatchEvent::Failed(error) if slot.failure.is_none() => {
            slot.failure = Some(error);
            slot.position = None;
            slot.environment = None;
        }
        _ => {}
    });
}

fn publish(mailbox: &Arc<Mutex<Mailbox>>, epoch: u64, update: impl FnOnce(&mut Mailbox)) {
    let wake = {
        let mut slot = mailbox.lock();
        // Without an explicitly attached weak GUI route callbacks are inert.
        if !slot.live || slot.epoch != epoch || slot.wake.is_none() {
            return;
        }
        update(&mut slot);
        if slot.queued {
            return;
        }
        slot.queued = true;
        slot.wake.clone()
    };
    // No consumer, UI effect or dispatch runs with the mailbox lock held.
    if let Some(wake) = wake
        && wake().is_err()
    {
        let mut slot = mailbox.lock();
        if slot.epoch == epoch {
            slot.queued = false;
        }
        // Never directly fall back to GUI effects. Results remain bounded for
        // a later valid UI drain; unavailable dispatch cannot manufacture ready.
    }
}
