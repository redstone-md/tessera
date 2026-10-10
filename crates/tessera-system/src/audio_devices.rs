// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Device and mixer capabilities on the existing audio owner. Labels are display
//! data only; every write carries the exact native endpoint/session incarnation.

use crate::audio::{AudioError, AudioFlow, EndpointId, Volume};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AudioDeviceKey {
    pub id: EndpointId,
    pub incarnation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AudioSessionKey {
    pub endpoint: AudioDeviceKey,
    pub instance_id: String,
    pub incarnation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioRole {
    Console,
    Multimedia,
    Communications,
}

/// Independent observations: an error never becomes a false default-role fact.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioDefaultRoles {
    pub console: Result<Option<EndpointId>, AudioError>,
    pub multimedia: Result<Option<EndpointId>, AudioError>,
    pub communications: Result<Option<EndpointId>, AudioError>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioLevel {
    pub volume: Volume,
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioSession {
    pub key: AudioSessionKey,
    pub name: String,
    pub system_sounds: bool,
    pub active: bool,
    pub level: Result<AudioLevel, AudioError>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioDevice {
    pub key: AudioDeviceKey,
    pub flow: AudioFlow,
    pub name: String,
    pub level: Result<AudioLevel, AudioError>,
    pub watch: Result<(), AudioError>,
    /// Input endpoints have an empty list; only output has app/system mixers.
    pub sessions: Result<Vec<AudioSession>, AudioError>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioDeviceFlow {
    pub devices: Result<Vec<AudioDevice>, AudioError>,
    pub roles: AudioDefaultRoles,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioDevicesSnapshot {
    pub output: AudioDeviceFlow,
    pub input: AudioDeviceFlow,
    /// Unavailable policy support is distinct from unavailable volume control.
    pub default_role_control: Result<(), AudioError>,
    pub device_settings: Result<(), AudioError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioDefaultTarget {
    /// Applied in Multimedia then Console order; both results remain independent.
    MultimediaAndConsole,
    Communications,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AudioDeviceCommand {
    SetVolume {
        endpoint: AudioDeviceKey,
        volume: Volume,
    },
    SetMuted {
        endpoint: AudioDeviceKey,
        muted: bool,
    },
    SetSessionVolume {
        session: AudioSessionKey,
        volume: Volume,
    },
    SetSessionMuted {
        session: AudioSessionKey,
        muted: bool,
    },
    SetDefault {
        endpoint: AudioDeviceKey,
        target: AudioDefaultTarget,
    },
    /// The native host dispatches only its fixed, trusted Sound Settings target.
    OpenDeviceSettings { endpoint: AudioDeviceKey },
}

impl AudioDeviceCommand {
    pub fn endpoint(&self) -> &AudioDeviceKey {
        match self {
            Self::SetVolume { endpoint, .. }
            | Self::SetMuted { endpoint, .. }
            | Self::SetDefault { endpoint, .. }
            | Self::OpenDeviceSettings { endpoint } => endpoint,
            Self::SetSessionVolume { session, .. } | Self::SetSessionMuted { session, .. } => {
                &session.endpoint
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioRoleResult {
    pub role: AudioRole,
    pub result: Result<(), AudioError>,
}

/// A role batch may partially succeed. The fresh snapshot and each individual
/// role result are returned together, never collapsed to optimistic success.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioDevicesResult {
    pub snapshot: AudioDevicesSnapshot,
    pub roles: Vec<AudioRoleResult>,
}

pub type AudioDevicesCompletion =
    Box<dyn FnOnce(Result<AudioDevicesResult, AudioError>) + Send + 'static>;
