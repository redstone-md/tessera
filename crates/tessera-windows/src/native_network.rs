// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Worker-only WLAN cache and explicit scoped connection adapter. Construction
//! performs no native calls; reads do not scan and credentials are not saved.
//! A failed unregister barrier intentionally retains its callback context rather
//! than risking use-after-free; Settings dispatch acknowledges only Shell launch.

use std::sync::Arc;

use tessera_system::network::{
    NetworkCommand, NetworkCommandOutcome, NetworkError, NetworkSnapshot, NetworkView,
};

use crate::network::worker::NetworkOwner;

#[path = "native_network/buffer.rs"]
mod buffer;
#[path = "native_network/callback.rs"]
mod callback;
#[path = "native_network/calls.rs"]
mod calls;
#[path = "native_network/controls.rs"]
mod controls;
#[path = "native_network/observations.rs"]
mod observations;
#[path = "native_network/owner.rs"]
mod owner;
#[path = "native_network/sdk.rs"]
mod sdk;

pub(crate) struct NativeOwner {
    owner: owner::Owner<sdk::WindowsCalls>,
}

impl NativeOwner {
    /// Pure factory: handles and callback registration are acquired on the worker.
    pub(crate) fn new() -> Self {
        Self {
            owner: owner::Owner::new(sdk::WindowsCalls),
        }
    }
}

impl NetworkOwner for NativeOwner {
    fn read(&mut self) -> Result<NetworkSnapshot, NetworkError> {
        self.owner.read()
    }

    fn read_view(&mut self) -> Result<NetworkView, NetworkError> {
        self.owner.read_view()
    }

    fn command(&mut self, command: NetworkCommand) -> Result<(), NetworkError> {
        self.owner.command(command)
    }

    fn command_readback(&mut self) -> Result<Option<NetworkCommandOutcome>, NetworkError> {
        self.owner.command_readback()
    }

    fn command_retired(&mut self) {
        self.owner.command_retired();
    }

    fn open_settings(&mut self) -> Result<(), NetworkError> {
        self.owner.open_settings()
    }

    fn register(&mut self, wake: Arc<dyn Fn() + Send + Sync>) -> Result<(), NetworkError> {
        self.owner.register(wake)
    }

    fn unregister(&mut self) -> Result<(), NetworkError> {
        self.owner.unregister()
    }
}

#[cfg(test)]
#[path = "native_network/sdk_tests.rs"]
mod sdk_tests;
#[cfg(test)]
#[path = "native_network/tests.rs"]
mod tests;
