// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use tessera_system::bluetooth::{
    BluetoothConnectionState, BluetoothError, BluetoothErrorKind, BluetoothRadioState,
    BluetoothSnapshot, BluetoothTransport,
};

use crate::generated::{
    BluetoothDeviceRow, BluetoothRadioRow, BluetoothRadioStatus, BluetoothTransportKind,
};
use crate::sanitize::bounded_text;

#[derive(Default)]
pub(super) struct Projection {
    pub(super) radios: Vec<BluetoothRadioRow>,
    pub(super) connected: Vec<BluetoothDeviceRow>,
    pub(super) paired: Vec<BluetoothDeviceRow>,
    pub(super) unknown: Vec<BluetoothDeviceRow>,
    pub(super) no_radios: bool,
    pub(super) empty_inventory: bool,
    pub(super) partial_inventory: bool,
    pub(super) notice: String,
}

impl Projection {
    pub(super) fn from_snapshot(snapshot: &BluetoothSnapshot) -> Self {
        let mut projection = Self::default();
        let mut failures = Vec::new();
        match &snapshot.radios {
            Ok(radios) => {
                projection.no_radios = radios.is_empty();
                projection.radios = radios
                    .iter()
                    .map(|radio| BluetoothRadioRow {
                        name: bounded_text(&radio.name, 128).into(),
                        state: match radio.state {
                            BluetoothRadioState::On => BluetoothRadioStatus::On,
                            BluetoothRadioState::Off => BluetoothRadioStatus::Off,
                            BluetoothRadioState::Disabled => BluetoothRadioStatus::Disabled,
                            BluetoothRadioState::Unknown => BluetoothRadioStatus::Unknown,
                        },
                    })
                    .collect();
            }
            Err(error) => failures.push(failure("Radios", error)),
        }
        let mut inventories_read = 0;
        for inventory in [&snapshot.classic, &snapshot.low_energy] {
            let (label, transport) = match inventory.transport {
                BluetoothTransport::Classic => ("Classic", BluetoothTransportKind::Classic),
                BluetoothTransport::LowEnergy => ("Low Energy", BluetoothTransportKind::LowEnergy),
            };
            match &inventory.devices {
                Ok(devices) => {
                    inventories_read += 1;
                    for device in devices {
                        // Preserve every (transport, ID) observation. Display names
                        // are bounded text, not identity or selection authority.
                        let row = BluetoothDeviceRow {
                            name: bounded_text(&device.name, 128).into(),
                            transport,
                        };
                        match device.connection {
                            BluetoothConnectionState::Connected => projection.connected.push(row),
                            BluetoothConnectionState::Disconnected => projection.paired.push(row),
                            BluetoothConnectionState::Unknown => projection.unknown.push(row),
                        }
                    }
                }
                Err(error) => failures.push(failure(label, error)),
            }
        }
        projection.partial_inventory = inventories_read < 2;
        projection.empty_inventory = inventories_read > 0
            && projection.connected.is_empty()
            && projection.paired.is_empty()
            && projection.unknown.is_empty();
        projection.notice = failures.join(" · ");
        projection
    }
}

pub(super) fn failure(context: &str, error: &BluetoothError) -> String {
    let status = match error.kind {
        BluetoothErrorKind::Unsupported => "Bluetooth is not supported on this platform.",
        BluetoothErrorKind::AccessDenied => "Bluetooth access was denied.",
        BluetoothErrorKind::Busy => "Bluetooth provider is busy. Try Refresh again.",
        BluetoothErrorKind::Stopped => "Bluetooth provider has stopped. Try Refresh again.",
        BluetoothErrorKind::InvalidData => "Bluetooth observations could not be validated.",
        BluetoothErrorKind::Unavailable => "Bluetooth observations are unavailable.",
    };
    let detail = bounded_text(&error.message, 160);
    if detail.is_empty() {
        format!("{context}: {status}")
    } else {
        format!("{context}: {status} {detail}")
    }
}
