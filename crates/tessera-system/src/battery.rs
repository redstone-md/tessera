// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Independent cached power facts; no power policy or session mutations.

use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryError {
    Unsupported,
    Busy,
    Stopped,
    Unavailable,
    AccessDenied,
    InvalidData,
    Exhausted,
    Native { code: u32 },
}

impl fmt::Display for BatteryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("Battery observations are not supported"),
            Self::Busy => formatter.write_str("A battery request is already in progress"),
            Self::Stopped => formatter.write_str("The battery worker has stopped"),
            Self::Unavailable => formatter.write_str("The battery observation is unavailable"),
            Self::AccessDenied => formatter.write_str("The battery observation was denied"),
            Self::InvalidData => formatter.write_str("Windows returned an invalid battery value"),
            Self::Exhausted => {
                formatter.write_str("Battery request identifiers exhausted; restart required")
            }
            Self::Native { code } => write!(
                formatter,
                "Battery request failed (native code 0x{code:08x})"
            ),
        }
    }
}
impl std::error::Error for BatteryError {}

/// Unknown is a successful read without a usable value, not absence or an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryFact<T> {
    Known(T),
    Unknown,
    Unavailable(BatteryError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryState {
    NotPresent,
    Discharging,
    Idle,
    Charging,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerSupplyState {
    NotPresent,
    Inadequate,
    Adequate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnergySaverState {
    Disabled,
    Off,
    On,
}

/// Sequential, best-effort OS-cache reads, not an atomic or live-state assertion.
/// Percentage outside 0..=100 is invalid, never clamped. Remaining runtime is a
/// Windows estimate in seconds; unknown and failed reads stay independent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatterySnapshot {
    pub battery: BatteryFact<BatteryState>,
    pub supply: BatteryFact<PowerSupplyState>,
    pub percent: BatteryFact<u8>,
    pub remaining_seconds: BatteryFact<u32>,
    pub energy_saver: BatteryFact<EnergySaverState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryEvent {
    Changed,
    WatchReady,
    WatchUnavailable(BatteryError),
}

pub type BatteryCompletion =
    Box<dyn FnOnce(Result<BatterySnapshot, BatteryError>) + Send + 'static>;
/// This receipt only acknowledges SDK initiation, not a visible Settings window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatterySettingsAccepted;
pub type BatterySettingsCompletion =
    Box<dyn FnOnce(Result<BatterySettingsAccepted, BatteryError>) + Send + 'static>;

/// One shared lazy native read/watch owner for toolbar and popup. A successful
/// admission accepts exactly one callback (possibly inline); immediate failure
/// accepts none. Accepted reads/Settings work outlive UI and host retirement.
/// No GUI joins, replay, idle polling, scan, or power policy mutation.
pub trait BatteryHost: Send + Sync + 'static {
    fn read(&self, completion: BatteryCompletion) -> Result<(), BatteryError>;
    fn subscribe(
        &self,
        _events: Arc<dyn Fn(BatteryEvent) + Send + Sync>,
    ) -> Result<Option<Box<dyn Send + 'static>>, BatteryError> {
        Ok(None)
    }
    /// Fixed local Power & battery Settings route; no caller-supplied URI.
    fn open_settings(&self, _completion: BatterySettingsCompletion) -> Result<(), BatteryError> {
        Err(BatteryError::Unsupported)
    }
}
