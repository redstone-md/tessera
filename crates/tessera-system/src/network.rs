// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Best-effort WLAN cache observations and optional explicitly scoped controls.

use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkErrorKind {
    Unsupported,
    AccessDenied,
    ServiceUnavailable,
    DeviceChanged,
    Busy,
    Stopped,
    InvalidData,
    Other,
}

/// Diagnostics must not include SSIDs, BSSIDs, profiles or credentials by default.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkError {
    pub kind: NetworkErrorKind,
    pub message: String,
}

impl NetworkError {
    pub fn new(kind: NetworkErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for NetworkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for NetworkError {}

/// Opaque stable identity. The native adapter determines the GUID byte representation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InterfaceId([u8; 16]);

impl InterfaceId {
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// Raw IEEE 802.11 identity; presentation decoding never participates in joining.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ssid(Box<[u8]>);

impl Ssid {
    pub fn new(bytes: &[u8]) -> Result<Self, NetworkError> {
        if bytes.len() > 32 {
            return Err(NetworkError::new(
                NetworkErrorKind::InvalidData,
                "The network identity exceeds the WLAN limit.",
            ));
        }
        Ok(Self(bytes.into()))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Bounded readable text only. Control and directional formatting characters
    /// cannot turn native network names into invisible or misleading UI controls.
    pub fn display_name(&self) -> String {
        if self.0.is_empty() {
            return "Hidden network".into();
        }
        String::from_utf8_lossy(&self.0).chars().map(|character| {
            if character.is_control() || matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                '\u{fffd}'
            } else { character }
        }).collect()
    }
}

// Prevent accidentally logging private identities through snapshot Debug output.
impl fmt::Debug for Ssid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ssid")
            .field("length", &self.0.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioState {
    Enabled,
    Disabled,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Observation<T> {
    Ready(T),
    Unavailable(NetworkError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WifiInterface {
    pub id: InterfaceId,
    pub name: String,
    pub radio: Observation<RadioState>,
    pub discovery: Observation<Vec<WifiBss>>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct WifiBss {
    pub ssid: Ssid,
    pub bssid: [u8; 6],
    pub frequency_khz: u32,
    pub signal_percent: u8,
    pub rssi_dbm: i32,
    pub known: Option<bool>,
    pub secured: Option<bool>,
    /// True requires successful exact current-BSSID observation, not SSID equality.
    pub connected: Option<bool>,
    pub auth: Option<String>,
}

impl fmt::Debug for WifiBss {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WifiBss")
            .field("ssid", &self.ssid)
            .field("signal_percent", &self.signal_percent)
            .field("known", &self.known)
            .field("secured", &self.secured)
            .field("connected", &self.connected)
            .finish_non_exhaustive()
    }
}

/// Best-effort non-atomic OS-cache read. Never an active scan, a freshness guarantee,
/// or an assertion about Internet connectivity. Empty interfaces confirms absence
/// only after successful native enumeration; failures must remain unavailable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkSnapshot {
    pub interfaces: Vec<WifiInterface>,
}

pub type NetworkCompletion =
    Box<dyn FnOnce(Result<NetworkSnapshot, NetworkError>) + Send + 'static>;
pub type NetworkActionCompletion = Box<dyn FnOnce(Result<(), NetworkError>) + Send + 'static>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetworkEvent {
    Changed,
    WatchReady,
    WatchUnavailable(NetworkError),
}

/// Native-issued authority, scoped to one owner and observation revision.
/// Never derive this from a display name, SSID text or an adapter index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkTarget {
    owner: u64,
    key: u64,
    revision: u64,
}

impl NetworkTarget {
    pub const fn new(owner: u64, key: u64, revision: u64) -> Self {
        Self {
            owner,
            key,
            revision,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkConnectCapability {
    SavedProfile,
    Open,
    Wpa2Personal,
    Unavailable,
}

#[derive(Clone, Eq, PartialEq)]
pub struct NetworkControl {
    pub target: NetworkTarget,
    pub interface: InterfaceId,
    pub interface_name: String,
    pub ssid: Ssid,
    pub bssid: [u8; 6],
    pub connect: NetworkConnectCapability,
    pub connected: bool,
}

impl fmt::Debug for NetworkControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkControl")
            .field("target", &self.target)
            .field("connect", &self.connect)
            .field("connected", &self.connected)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkControlInventory {
    pub networks: Vec<NetworkControl>,
    /// Redacted independent failures; successful adapter observations survive.
    pub unavailable: Vec<NetworkError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkView {
    pub snapshot: NetworkSnapshot,
    /// None preserves legacy read-only providers. Partial per-interface failures
    /// do not discard independently successful snapshot/control observations.
    pub controls: Option<Observation<NetworkControlInventory>>,
}

/// Deliberately not Clone. Passwords are one-shot native inputs, never status.
pub struct NetworkPassword(Vec<u8>);

impl NetworkPassword {
    pub fn new(value: &str) -> Self {
        Self(value.as_bytes().to_vec())
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for NetworkPassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NetworkPassword([redacted])")
    }
}

impl Drop for NetworkPassword {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&mut self.0);
    }
}

#[derive(Debug)]
pub enum NetworkCommand {
    Connect {
        target: NetworkTarget,
        password: Option<NetworkPassword>,
    },
    Disconnect {
        target: NetworkTarget,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkCommandOutcome {
    Connected,
    Disconnected,
    /// Documented numeric WLAN reason only; no native profile/name diagnostics.
    Failed {
        reason: u32,
    },
    /// Native initiation was accepted but no conclusive readback arrived within
    /// the bounded observation window. This is NOT success or a retry request.
    AcceptedUnconfirmed,
}

pub type NetworkViewCompletion =
    Box<dyn FnOnce(Result<NetworkView, NetworkError>) + Send + 'static>;
pub type NetworkCommandCompletion =
    Box<dyn FnOnce(Result<NetworkCommandOutcome, NetworkError>) + Send + 'static>;
/// Optional one-shot native-initiation acknowledgement, never connected success.
pub type NetworkCommandAccepted = Box<dyn FnOnce() + Send + 'static>;

/// Prompt admission. `Ok` transfers exactly one completion (possibly inline or on
/// any thread); an immediate error transfers none. Accepted reads/actions outlive
/// UI retirement. Reads never actively scan; only explicit typed commands mutate.
///
/// A subscribe guard acknowledges startup, not readiness. WatchReady follows
/// successful native registration. Guard drop closes event admission immediately
/// and requests asynchronous cleanup; callers must generation-gate already posted
/// events. None means watch unsupported; read/settings remain independent.
///
/// Settings dispatch is restricted to ms-settings:network and acknowledges Shell
/// initiation, never visible Settings/focus or completion of an OS configuration.
pub trait NetworkHost: Send + Sync + 'static {
    fn read(&self, completion: NetworkCompletion) -> Result<(), NetworkError>;
    fn read_view(&self, completion: NetworkViewCompletion) -> Result<(), NetworkError> {
        self.read(Box::new(move |result| {
            completion(result.map(|snapshot| NetworkView {
                snapshot,
                controls: None,
            }))
        }))
    }
    fn command(
        &self,
        _command: NetworkCommand,
        _completion: NetworkCommandCompletion,
    ) -> Result<(), NetworkError> {
        Err(NetworkError::new(
            NetworkErrorKind::Unsupported,
            "Wi-Fi controls are unavailable.",
        ))
    }
    fn command_with_acceptance(
        &self,
        command: NetworkCommand,
        _accepted: NetworkCommandAccepted,
        completion: NetworkCommandCompletion,
    ) -> Result<(), NetworkError> {
        // Legacy/control hosts without native-initiation feedback stay truthful.
        self.command(command, completion)
    }
    fn open_settings(&self, completion: NetworkActionCompletion) -> Result<(), NetworkError>;
    fn subscribe(
        &self,
        events: Arc<dyn Fn(NetworkEvent) + Send + Sync>,
    ) -> Result<Option<Box<dyn Send>>, NetworkError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssid_boundaries_preserve_raw_identity() {
        assert!(Ssid::new(&[]).unwrap().as_bytes().is_empty());
        assert_eq!(Ssid::new(&[0; 32]).unwrap().as_bytes().len(), 32);
        assert_eq!(
            Ssid::new(&[0; 33]).unwrap_err().kind,
            NetworkErrorKind::InvalidData
        );
        assert_ne!(Ssid::new(&[0xff]).unwrap(), Ssid::new(&[0xfe]).unwrap());
        assert_eq!(
            Ssid::new(&[0xff]).unwrap().display_name(),
            Ssid::new(&[0xfe]).unwrap().display_name()
        );
    }

    #[test]
    fn display_is_bounded_and_cannot_change_identity_or_insert_controls() {
        let ssid = Ssid::new(b"a\0\nb").unwrap();
        assert_eq!(ssid.as_bytes(), b"a\0\nb");
        assert_eq!(ssid.display_name(), "a\u{fffd}\u{fffd}b");
        let rtl = Ssid::new("\u{202e}name".as_bytes()).unwrap();
        assert_eq!(rtl.display_name(), "\u{fffd}name");
        assert!(
            Ssid::new(&[0xff; 32])
                .unwrap()
                .display_name()
                .chars()
                .count()
                <= 32
        );
        assert!(
            !format!("{:?}", Ssid::new(b"private-network").unwrap()).contains("private-network")
        );
    }

    #[test]
    fn absence_is_distinct_from_failed_radio_and_discovery() {
        let absent = NetworkSnapshot { interfaces: vec![] };
        let denied = NetworkError::new(
            NetworkErrorKind::AccessDenied,
            "Location access is unavailable.",
        );
        let partial = NetworkSnapshot {
            interfaces: vec![WifiInterface {
                id: InterfaceId::new([1; 16]),
                name: "Adapter".into(),
                radio: Observation::Ready(RadioState::Unknown),
                discovery: Observation::Unavailable(denied),
            }],
        };
        assert_ne!(absent, partial);
        let bss = WifiBss {
            ssid: Ssid::new(b"name").unwrap(),
            bssid: [0; 6],
            frequency_khz: 0,
            signal_percent: 0,
            rssi_dbm: -100,
            known: None,
            secured: None,
            connected: None,
            auth: None,
        };
        assert_eq!(bss.connected, None);
        assert_eq!(bss.known, None);
    }
}
