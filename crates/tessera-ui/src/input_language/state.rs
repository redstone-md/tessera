// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Token {
    pub session: u64,
    pub sequence: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Operation {
    Read,
    Activate,
    Settings,
}

pub(super) struct Flight {
    pub token: Token,
    pub operation: Operation,
}

#[derive(Default)]
pub(super) struct Keys {
    pub profiles: Vec<(SharedString, ProfileId)>,
    pub settings: SharedString,
}

#[derive(Default)]
pub(super) struct State {
    pub session: u64,
    sequence: u64,
    key_sequence: u64,
    pub closed: bool,
    pub acquiring: bool,
    pub provider: Option<Arc<dyn InputLanguageHost>>,
    pub flight: Option<Flight>,
    pub read_requested: bool,
    pub pending_action: Option<InputLanguageAction>,
    pub snapshot: Option<InputLanguageSnapshot>,
    pub keys: Keys,
    pub notice: String,
    pub action_notice: String,
}

impl State {
    pub fn loading(&self) -> bool {
        self.acquiring
            || self.read_requested
            || self.flight.as_ref().is_some_and(|flight| {
                flight.token.session == self.session && flight.operation == Operation::Read
            })
    }
    pub fn idle(&self) -> bool {
        !self.closed
            && !self.acquiring
            && !self.read_requested
            && self.flight.is_none()
            && self.pending_action.is_none()
    }

    pub fn action_pending(&self) -> bool {
        self.pending_action.is_some()
            || self
                .flight
                .as_ref()
                .is_some_and(|flight| flight.operation != Operation::Read)
    }

    pub fn begin(&mut self, operation: Operation) -> Token {
        self.sequence = self.sequence.wrapping_add(1);
        let token = Token {
            session: self.session,
            sequence: self.sequence,
        };
        self.flight = Some(Flight { token, operation });
        token
    }

    pub fn key(&mut self) -> SharedString {
        self.key_sequence = self.key_sequence.wrapping_add(1);
        format!("input-{:x}-{:x}", self.session, self.key_sequence).into()
    }

    pub fn profile(&self, key: &SharedString) -> Option<ProfileId> {
        if !self.idle() || key.is_empty() {
            return None;
        }
        self.keys
            .profiles
            .iter()
            .find(|(issued, _)| issued == key)
            .map(|(_, profile)| profile.clone())
    }

    pub fn accepts_settings(&self, key: &SharedString) -> bool {
        !self.closed
            && !self.acquiring
            && !self.action_pending()
            && self.provider.is_some()
            && !key.is_empty()
            && self.keys.settings == *key
    }
}
