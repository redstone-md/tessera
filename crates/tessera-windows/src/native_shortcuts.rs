// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One installing thread owns the hook, hotkey IDs and message pump. Consumers
//! run on a separate bounded relay, never in the hook or native owner thread.
//! Start-menu/key-balance behavior still needs real Windows VM certification;
//! this is not completion of the full native source shortcut matrix.

mod actor;
mod reducer;
#[cfg(windows)]
mod sdk;
#[cfg(all(test, windows))]
mod sdk_tests;
#[cfg(test)]
mod tests;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize},
};
#[cfg(windows)]
use tessera_system::shortcuts::ShortcutHost;
use tessera_system::shortcuts::{
    KeyChord, ShortcutAction, ShortcutError, ShortcutErrorKind, TriggerPoint,
};

pub(super) struct Gate {
    generation: AtomicU64,
    listening: AtomicBool,
    closed: AtomicBool,
    retirement: AtomicU64,
    events: AtomicUsize,
}

pub(super) type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RawTrigger {
    generation: u64,
    action: ShortcutAction,
    reserved: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BackendEvent {
    Triggered(RawTrigger),
    RearmRequired,
}

/// Internal recording seam; all methods run on the installing owner thread.
pub(super) trait Backend: 'static {
    fn wake(&self) -> Wake;
    fn install_launcher(&mut self, generation: u64) -> Result<(), ShortcutError>;
    fn register_settings(&mut self, chord: &KeyChord, generation: u64)
    -> Result<(), ShortcutError>;
    fn cleanup(&mut self) -> Result<(), ShortcutError>;
    fn wait(&mut self) -> Result<Option<BackendEvent>, ShortcutError>;
    fn cursor(&mut self) -> Option<TriggerPoint>;
}

pub(super) fn unavailable() -> ShortcutError {
    ShortcutError::new(
        ShortcutErrorKind::Other,
        None,
        "Native shortcut owner is unavailable",
    )
}

#[cfg(windows)]
pub(crate) fn create_host() -> Result<Arc<dyn ShortcutHost>, ShortcutError> {
    actor::create(sdk::SdkBackend::new)
}
