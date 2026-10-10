// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Passive paired-device inventory and opt-in, exact-incarnation radio control.
//!
//! An inventory entry is cached pairing information, not evidence of presence,
//! service availability, or permission to connect. Reads neither discover devices
//! nor request consent, open device sessions, or change radio/pairing state.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BluetoothTransport {
    Classic,
    LowEnergy,
}

/// A nonempty opaque native identity, preserved verbatim.
///
/// Never interpret this value as a path or address, display it as a device name,
/// or use a name as identity. A device's key is `(transport, id)`.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BluetoothDeviceId(String);

impl BluetoothDeviceId {
    pub fn new(value: impl Into<String>) -> Result<Self, BluetoothError> {
        let value = value.into();
        if value.is_empty() {
            return Err(BluetoothError::new(
                BluetoothErrorKind::InvalidData,
                "Bluetooth device identity must not be empty",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothRadioState {
    On,
    Off,
    Disabled,
    /// Includes unrecognized native enum values; never implies `Off`.
    Unknown,
}

/// Observed display name and state; no guessed stable radio identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothRadioObservation {
    pub name: String,
    pub state: BluetoothRadioState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothConnectionState {
    Connected,
    Disconnected,
    /// The optional connection property was absent or not provided.
    /// A supplied property of the wrong type is an acquisition error instead.
    Unknown,
}

/// Pairing is checked while acquiring this row; confirmed unpaired races are
/// excluded. Empty native names remain empty and need an unnamed-device label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothPairedDevice {
    pub id: BluetoothDeviceId,
    pub name: String,
    pub connection: BluetoothConnectionState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothTransportInventory {
    pub transport: BluetoothTransport,
    /// A successful empty inventory is distinct from any failed observation.
    pub devices: Result<Vec<BluetoothPairedDevice>, BluetoothError>,
}

/// Independently acquired observations, not an atomic hardware snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothSnapshot {
    /// Empty means no Bluetooth radios were exposed by this API. In particular,
    /// architecture-sensitive enumeration does not prove no adapter exists.
    pub radios: Result<Vec<BluetoothRadioObservation>, BluetoothError>,
    pub classic: BluetoothTransportInventory,
    pub low_energy: BluetoothTransportInventory,
}

impl BluetoothSnapshot {
    /// Fixes transport tags while retaining all independent failures and rows.
    pub fn new(
        radios: Result<Vec<BluetoothRadioObservation>, BluetoothError>,
        classic: Result<Vec<BluetoothPairedDevice>, BluetoothError>,
        low_energy: Result<Vec<BluetoothPairedDevice>, BluetoothError>,
    ) -> Self {
        Self {
            radios,
            classic: BluetoothTransportInventory {
                transport: BluetoothTransport::Classic,
                devices: classic,
            },
            low_energy: BluetoothTransportInventory {
                transport: BluetoothTransport::LowEnergy,
                devices: low_energy,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothErrorKind {
    Unsupported,
    AccessDenied,
    Busy,
    Stopped,
    InvalidData,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothError {
    pub kind: BluetoothErrorKind,
    /// Diagnostic provider text: consumers bound and sanitize it before display.
    pub message: String,
    /// Unsigned native HRESULT bits, if available; do not truncate/sign-extend.
    pub native_code: Option<u32>,
}

impl BluetoothError {
    pub fn new(kind: BluetoothErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            native_code: None,
        }
    }
}

impl fmt::Display for BluetoothError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BluetoothError {}

pub type BluetoothReadCompletion =
    Box<dyn FnOnce(Result<BluetoothSnapshot, BluetoothError>) + Send + 'static>;

/// An owner-issued incarnation. Equality is token identity, not display text,
/// a native address, or an enumeration index. Clones do not extend native life.
#[derive(Clone)]
pub struct BluetoothRadioKey(std::sync::Arc<()>);

impl BluetoothRadioKey {
    pub fn issue() -> Self {
        Self(std::sync::Arc::new(()))
    }
}

impl PartialEq for BluetoothRadioKey {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for BluetoothRadioKey {}
impl fmt::Debug for BluetoothRadioKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BluetoothRadioKey(redacted)")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothControlledRadio {
    pub key: BluetoothRadioKey,
    pub revision: u64,
    pub observation: BluetoothRadioObservation,
}

/// Additional inventory without changing any legacy snapshot literal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothControlSnapshot {
    pub snapshot: BluetoothSnapshot,
    pub radios: Result<Vec<BluetoothControlledRadio>, BluetoothError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothRadioPower {
    On,
    Off,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothRadioCommand {
    pub radio: BluetoothControlledRadio,
    pub power: BluetoothRadioPower,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothRadioAccess {
    Allowed,
    DeniedByUser,
    DeniedBySystem,
    Unspecified,
}

/// Allowed is native acceptance, not confirmation of the requested power.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothRadioOutcome {
    pub access: BluetoothRadioAccess,
    pub observed: Result<BluetoothRadioObservation, BluetoothError>,
}

pub type BluetoothControlReadCompletion =
    Box<dyn FnOnce(Result<BluetoothControlSnapshot, BluetoothError>) + Send + 'static>;
pub type BluetoothRadioCompletion =
    Box<dyn FnOnce(Result<BluetoothRadioOutcome, BluetoothError>) + Send + 'static>;

/// Opt-in owned inventory. Admission/completion rules match `BluetoothHost`.
/// Unlike legacy reads, native radios/events/apartment stay on the command
/// owner until retirement. No native handles cross this interface.
pub trait BluetoothRadioControlHost: Send + Sync {
    fn read_controls(
        &self,
        completion: BluetoothControlReadCompletion,
    ) -> Result<(), BluetoothError>;
    /// Invoke only from the current user's GUI/valid consent context. Admission
    /// may start nonblocking consent there; the owned worker awaits it. Never
    /// retry/replay automatically. State events, a new inventory, or retirement
    /// revoke captured revisions before submission. Accepted work outlives UI.
    fn set_radio(
        &self,
        command: BluetoothRadioCommand,
        completion: BluetoothRadioCompletion,
    ) -> Result<(), BluetoothError>;
}

/// Prompt admission of one independent read, without a queued request backlog.
///
/// Immediate `Ok` accepts exactly one completion, possibly inline; immediate
/// `Err` accepts zero callbacks. Native resources and the apartment retire,
/// then the flight releases, before completion is invoked outside locks. The
/// consumer may reenter `read`. UI callers always post and generation-gate it.
///
/// Outer completion failure means owner initialization/global acquisition
/// failed. Radio, Classic, and LE query failures remain in their snapshot fields
/// so successful observations survive partial failure. No consent, discovery,
/// native sessions, commands, URI opening, or event tokens cross this seam.
///
/// Accepted reads outlive host/UI drops without a GUI-thread join. There is no
/// cancellation, native execution timeout, live-update or atomic-snapshot
/// promise. Stalled native operations and process termination remain possible;
/// unwind containment cannot recover from aborts or double cleanup panics.
pub trait BluetoothHost: Send + Sync + 'static {
    fn read(&self, completion: BluetoothReadCompletion) -> Result<(), BluetoothError>;

    /// Cheap optional lookup; old read-only adapters incur no extra requests.
    fn radio_controls(&self) -> Option<&dyn BluetoothRadioControlHost> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, name: &str) -> BluetoothPairedDevice {
        BluetoothPairedDevice {
            id: BluetoothDeviceId::new(id).unwrap(),
            name: name.to_owned(),
            connection: BluetoothConnectionState::Unknown,
        }
    }

    #[test]
    fn bluetooth_id_rejects_only_empty_and_preserves_opaque_contents() {
        assert_eq!(
            BluetoothDeviceId::new("").unwrap_err().kind,
            BluetoothErrorKind::InvalidData
        );
        for value in [" ", "opaque\\native#ID", "\u{202e}not/a/path", "a\0b"] {
            assert_eq!(BluetoothDeviceId::new(value).unwrap().as_str(), value);
        }
        assert_ne!(
            BluetoothDeviceId::new("opaqueA").unwrap(),
            BluetoothDeviceId::new("opaquea").unwrap()
        );
    }

    #[test]
    fn bluetooth_snapshot_fixes_tags_without_name_deduplication() {
        let snapshot = BluetoothSnapshot::new(
            Ok(vec![]),
            Ok(vec![device("one", "Headset"), device("two", "Headset")]),
            Ok(vec![device("one", "")]),
        );
        assert_eq!(snapshot.classic.transport, BluetoothTransport::Classic);
        assert_eq!(snapshot.low_energy.transport, BluetoothTransport::LowEnergy);
        let classic = snapshot.classic.devices.unwrap();
        let low_energy = snapshot.low_energy.devices.unwrap();
        assert_eq!(classic.len(), 2);
        assert_ne!(classic[0].id, classic[1].id);
        assert_eq!(classic[0].id, low_energy[0].id);
        assert_eq!(low_energy[0].name, "");
        assert_eq!(low_energy[0].connection, BluetoothConnectionState::Unknown);
    }

    #[test]
    fn bluetooth_partial_failure_does_not_replace_successful_empty_inventory() {
        let denied = BluetoothError::new(BluetoothErrorKind::AccessDenied, "Denied");
        let invalid = BluetoothError::new(BluetoothErrorKind::InvalidData, "Wrong property type");
        let snapshot =
            BluetoothSnapshot::new(Err(denied.clone()), Ok(vec![]), Err(invalid.clone()));
        assert_eq!(snapshot.radios, Err(denied));
        assert_eq!(snapshot.classic.devices, Ok(vec![]));
        assert_eq!(snapshot.low_energy.devices, Err(invalid));
    }

    #[test]
    fn bluetooth_error_preserves_unsigned_hresult_and_diagnostic_message() {
        let mut error = BluetoothError::new(BluetoothErrorKind::AccessDenied, "Denied");
        assert_eq!(error.native_code, None);
        error.native_code = Some(0x8007_0005);
        assert_eq!(error.clone().native_code, Some(0x8007_0005));
        assert_eq!(error.to_string(), "Denied");
    }

    #[test]
    fn bluetooth_observations_are_portable_and_unknown_is_not_disconnected() {
        fn portable<T: Clone + Eq + Send + Sync + 'static>() {}
        portable::<BluetoothSnapshot>();
        portable::<BluetoothError>();
        portable::<BluetoothDeviceId>();
        assert_ne!(BluetoothRadioState::Unknown, BluetoothRadioState::Off);
        assert_ne!(
            BluetoothConnectionState::Unknown,
            BluetoothConnectionState::Disconnected
        );
    }
}
