// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

#[derive(Default)]
pub(super) struct Mailbox {
    pub expected_read: Option<Token>,
    pub read: Option<(Token, Result<NetworkSnapshot, NetworkError>)>,
    pub controls: Option<(Token, Option<Observation<NetworkControlInventory>>)>,
    pub radios: Option<(Token, Option<Observation<NetworkRadioInventory>>)>,
    pub profiles: Option<(Token, Option<Observation<NetworkProfileInventory>>)>,
    pub expected_command: Option<Token>,
    pub command: Option<(Token, Result<NetworkCommandOutcome, NetworkError>)>,
    pub accepted: Option<Token>,
    pub expected_settings: Option<Token>,
    pub settings: Option<(Token, Result<(), NetworkError>)>,
    pub watch_session: Option<u64>,
    pub changed: bool,
    pub watch: Option<NetworkEvent>,
    wake_queued: bool,
}

fn wake(mailbox: &Arc<Mutex<Mailbox>>, root: &slint::Weak<NetworkMenu>) {
    {
        let mut slot = mailbox.lock();
        if slot.wake_queued {
            return;
        }
        slot.wake_queued = true;
    }
    if root
        .upgrade_in_event_loop(|root| root.invoke_network_event_ready())
        .is_err()
    {
        // Recording/headless backends drain through the same production wake.
        mailbox.lock().wake_queued = false;
    }
}

pub(super) fn read_complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<NetworkMenu>,
    token: Token,
    result: Result<NetworkSnapshot, NetworkError>,
) {
    {
        let mut slot = mailbox.lock();
        if slot.expected_read != Some(token) || slot.read.is_some() {
            return;
        }
        slot.read = Some((token, result));
    }
    wake(mailbox, root);
}

pub(super) fn control_view_complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<NetworkMenu>,
    token: Token,
    result: Result<NetworkControlView, NetworkError>,
) {
    {
        let mut slot = mailbox.lock();
        if slot.expected_read != Some(token) || slot.read.is_some() {
            return;
        }
        let result = result.map(|view| {
            slot.controls = Some((token, view.controls));
            slot.radios = Some((token, view.radios));
            slot.profiles = Some((token, view.profiles));
            view.snapshot
        });
        slot.read = Some((token, result));
    }
    wake(mailbox, root);
}

pub(super) fn command_accepted(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<NetworkMenu>,
    token: Token,
) {
    {
        let mut slot = mailbox.lock();
        if slot.expected_command != Some(token) || slot.command.is_some() {
            return;
        }
        slot.accepted = Some(token);
    }
    wake(mailbox, root);
}

pub(super) fn command_complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<NetworkMenu>,
    token: Token,
    result: Result<NetworkCommandOutcome, NetworkError>,
) {
    {
        let mut slot = mailbox.lock();
        if slot.expected_command != Some(token) || slot.command.is_some() {
            return;
        }
        slot.command = Some((token, result));
    }
    wake(mailbox, root);
}

pub(super) fn settings_complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<NetworkMenu>,
    token: Token,
    result: Result<(), NetworkError>,
) {
    {
        let mut slot = mailbox.lock();
        if slot.expected_settings != Some(token) || slot.settings.is_some() {
            return;
        }
        slot.settings = Some((token, result));
    }
    wake(mailbox, root);
}

pub(super) fn event(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<NetworkMenu>,
    session: u64,
    event: NetworkEvent,
) {
    {
        let mut slot = mailbox.lock();
        if slot.watch_session != Some(session) {
            return;
        }
        match event {
            NetworkEvent::Changed => slot.changed = true,
            // A Changed burst cannot displace a readiness/failure observation.
            event => slot.watch = Some(event),
        }
    }
    wake(mailbox, root);
}

pub(super) struct Delivery {
    pub read: Option<(Token, Result<NetworkSnapshot, NetworkError>)>,
    pub controls: Option<(Token, Option<Observation<NetworkControlInventory>>)>,
    pub radios: Option<(Token, Option<Observation<NetworkRadioInventory>>)>,
    pub profiles: Option<(Token, Option<Observation<NetworkProfileInventory>>)>,
    pub command: Option<(Token, Result<NetworkCommandOutcome, NetworkError>)>,
    pub accepted: Option<Token>,
    pub settings: Option<(Token, Result<(), NetworkError>)>,
    pub watch_session: Option<u64>,
    pub changed: bool,
    pub watch: Option<NetworkEvent>,
}

impl Mailbox {
    pub fn take(&mut self) -> Delivery {
        self.wake_queued = false;
        let read = self.read.take();
        let settings = self.settings.take();
        let command = self.command.take();
        if command.is_some() {
            self.expected_command = None;
        }
        if read.is_some() {
            self.expected_read = None;
        }
        if settings.is_some() {
            self.expected_settings = None;
        }
        Delivery {
            read,
            controls: self.controls.take(),
            radios: self.radios.take(),
            profiles: self.profiles.take(),
            command,
            accepted: self.accepted.take(),
            settings,
            watch_session: self.watch_session,
            changed: std::mem::take(&mut self.changed),
            watch: self.watch.take(),
        }
    }
    pub fn close_watch(&mut self) {
        self.watch_session = None;
        self.changed = false;
        self.watch = None;
    }
}
