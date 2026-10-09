// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

impl PowerMenuController {
    /// Genuine Launcher account-name data only. Retains bounded domain data even
    /// while presentationless; never reads shell identity or creates a window.
    pub(crate) fn set_user_name(&self, user_name: &str) {
        let name: String = user_name
            .trim()
            .chars()
            .filter(|character| !character.is_control())
            .take(256)
            .collect();
        let generation = {
            let mut state = self.state.borrow_mut();
            if state.closed {
                return;
            }
            state.user_name = name;
            state.generation
        };
        self.project(generation);
    }

    pub(super) fn choose_updates(&self, epoch: u64, choice: bool) {
        let Some((current_epoch, surface)) = self.surface_snapshot() else {
            return;
        };
        let generation = self.state.borrow().generation;
        if current_epoch != epoch
            || !self.is_visible()
            || !surface.get_action_enabled()
            || surface.get_lock_busy()
            || !self.scope_is(generation, epoch)
        {
            return;
        }
        let generation = {
            let mut state = self.state.borrow_mut();
            if !state.current(generation)
                || state.epoch != epoch
                || state.authorized != Some(state.generation)
                || state.lock.is_some()
                || state.updates_hint != Some(PowerUpdateHint::Pending)
            {
                return;
            }
            state.install_updates = choice;
            state.generation
        };
        self.project(generation);
    }

    pub(super) fn pump_updates(&self) {
        let reservation = {
            let mut state = self.state.borrow_mut();
            if state.closed
                || !state.desired
                || !state.updates_dirty
                || state.updates_read.is_some()
                || self.surface.borrow().is_none()
            {
                return;
            }
            let token = match state.token() {
                Ok(token) => token,
                Err(error) => {
                    drop(state);
                    self.report(Err(error));
                    self.close();
                    return;
                }
            };
            state.updates_dirty = false;
            state.updates_read = Some(Flight {
                token,
                phase: Phase::Reserved,
            });
            (token, state.updates.clone())
        };
        let (token, cached) = reservation;
        self.mailbox.lock().updates.expected = Some(token);
        let provider = match cached {
            Some(provider) => Some(provider),
            None => match self.host.power_updates_host() {
                Ok(Some(provider)) => {
                    {
                        let mut state = self.state.borrow_mut();
                        if !state.closed && state.updates.is_none() {
                            state.updates = Some(provider.clone());
                        }
                    }
                    Some(provider)
                }
                Ok(None) => {
                    complete_updates(&self.mailbox, token, Err(PowerUpdatesError::Unsupported));
                    None
                }
                Err(error) => {
                    complete_updates(&self.mailbox, token, Err(error));
                    None
                }
            },
        };
        let Some(provider) = provider else { return };
        if !self.updates_current(token) {
            self.cancel_updates(token);
            self.schedule_work();
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            let Some(flight) = state
                .updates_read
                .as_mut()
                .filter(|flight| flight.token == token)
            else {
                return;
            };
            flight.phase = Phase::Submitted;
        }
        let mailbox = self.mailbox.clone();
        let result = provider.read(Box::new(move |result| {
            complete_updates(&mailbox, token, result)
        }));
        if let Err(error) = result {
            complete_updates(&self.mailbox, token, Err(error));
        }
    }

    fn updates_current(&self, token: Token) -> bool {
        let state = self.state.borrow();
        state.current(token.generation)
            && state.epoch == token.epoch
            && state
                .updates_read
                .is_some_and(|flight| flight.token == token)
    }

    fn cancel_updates(&self, token: Token) {
        {
            let mut state = self.state.borrow_mut();
            if state
                .updates_read
                .is_some_and(|flight| flight.token == token && flight.phase == Phase::Reserved)
            {
                state.updates_read = None;
            }
        }
        self.mailbox.lock().updates.clear(token);
    }

    pub(super) fn observe_updates(
        &self,
        token: Token,
        result: Result<PowerUpdateHint, PowerUpdatesError>,
    ) {
        let current = {
            let mut state = self.state.borrow_mut();
            if !state
                .updates_read
                .is_some_and(|flight| flight.token == token)
            {
                return;
            }
            state.updates_read = None;
            if !state.current(token.generation) || state.epoch != token.epoch {
                return;
            }
            match result {
                Ok(hint) => {
                    state.updates_hint = Some(hint);
                    state.updates_status.clear();
                }
                Err(error) => {
                    state.updates_hint = None;
                    state.updates_status = format!("System update hint unavailable: {error}");
                }
            }
            if !state.initialized {
                state.initial_updates_generation = Some(token.generation);
            }
            true
        };
        if !current || !self.scope_is(token.generation, token.epoch) {
            return;
        }
        self.project(token.generation);
        if !self.current(token.generation) || !self.epoch_is(token.epoch) {
            return;
        }
        let layout = {
            let state = self.state.borrow();
            if state.opening { state.layout } else { None }
        };
        if let Some(layout) = layout {
            self.present_layout(token.generation, layout);
        }
    }
}
