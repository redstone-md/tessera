// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

#[derive(Default)]
pub(super) struct Mailbox {
    pub read_expected: Option<u64>,
    pub settings_expected: Option<SettingsToken>,
    pub watch_expected: Option<u64>,
    read: Option<(u64, Result<BatterySnapshot, BatteryError>)>,
    settings: Option<(SettingsToken, Result<BatterySettingsAccepted, BatteryError>)>,
    changed: bool,
    watch: Option<BatteryEvent>,
    scheduled: bool,
}
pub(super) struct Delivery {
    pub read: Option<(u64, Result<BatterySnapshot, BatteryError>)>,
    pub settings: Option<(SettingsToken, Result<BatterySettingsAccepted, BatteryError>)>,
    pub changed: bool,
    pub watch: Option<BatteryEvent>,
}
impl Mailbox {
    pub fn take(&mut self) -> Delivery {
        self.scheduled = false;
        Delivery {
            read: self.read.take(),
            settings: self.settings.take(),
            changed: std::mem::take(&mut self.changed),
            watch: self.watch.take(),
        }
    }
    pub fn close_watch(&mut self) {
        self.watch_expected = None;
        self.changed = false;
        self.watch = None;
    }
}
fn wake(mailbox: &Arc<Mutex<Mailbox>>, root: &slint::Weak<BatteryMenu>) {
    let schedule = {
        let mut state = mailbox.lock();
        if state.scheduled {
            false
        } else {
            state.scheduled = true;
            true
        }
    };
    if schedule
        && root
            .upgrade_in_event_loop(|root| root.invoke_battery_event_ready())
            .is_err()
    {
        mailbox.lock().scheduled = false;
    }
}
pub(super) fn read_complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<BatteryMenu>,
    id: u64,
    result: Result<BatterySnapshot, BatteryError>,
) {
    {
        let mut state = mailbox.lock();
        if state.read_expected != Some(id) {
            return;
        }
        state.read = Some((id, result));
    }
    wake(mailbox, root);
}
pub(super) fn settings_complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<BatteryMenu>,
    token: SettingsToken,
    result: Result<BatterySettingsAccepted, BatteryError>,
) {
    {
        let mut state = mailbox.lock();
        if state.settings_expected != Some(token) {
            return;
        }
        state.settings = Some((token, result));
    }
    wake(mailbox, root);
}
pub(super) fn event(
    mailbox: &Arc<Mutex<Mailbox>>,
    root: &slint::Weak<BatteryMenu>,
    epoch: u64,
    event: BatteryEvent,
) {
    {
        let mut state = mailbox.lock();
        if state.watch_expected != Some(epoch) {
            return;
        }
        match event {
            BatteryEvent::Changed => state.changed = true,
            event => state.watch = Some(event),
        }
    }
    wake(mailbox, root);
}
