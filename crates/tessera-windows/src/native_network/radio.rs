// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Software-only PHY writes on the existing WLAN owner, never a radio/connection toggle.
use super::callback::Context;
use super::calls::{NativeCalls, native_error};
use super::observations;
use tessera_system::network::{
    NetworkError, NetworkErrorKind, NetworkRadioInitiation, NetworkRadioPhy, NetworkRadioPhyResult,
    NetworkRadioResult, NetworkTarget, Observation, RadioState,
};
use windows::Win32::NetworkManagement::WiFi::{
    DOT11_RADIO_STATE, WLAN_INTERFACE_INFO, WLAN_PHY_RADIO_STATE, dot11_radio_state_off,
    dot11_radio_state_on,
};
use windows::core::GUID;

#[derive(Clone)]
pub(super) struct Record {
    pub target: NetworkTarget,
    pub interface: GUID,
    pub description: [u16; 256],
    pub incarnation: u64,
    pub revision: u64,
    pub phys: Vec<WLAN_PHY_RADIO_STATE>,
    pub source_revision: u64,
}

pub(super) fn observed(phy: &WLAN_PHY_RADIO_STATE) -> NetworkRadioPhy {
    let software = state(phy.dot11SoftwareRadioState);
    let hardware = state(phy.dot11HardwareRadioState);
    let effective = if software == RadioState::Disabled || hardware == RadioState::Disabled {
        RadioState::Disabled
    } else if software == RadioState::Enabled && hardware == RadioState::Enabled {
        RadioState::Enabled
    } else {
        RadioState::Unknown
    };
    NetworkRadioPhy {
        id: phy.dwPhyIndex,
        software,
        hardware,
        effective,
    }
}

fn state(value: DOT11_RADIO_STATE) -> RadioState {
    if value == dot11_radio_state_on {
        RadioState::Enabled
    } else if value == dot11_radio_state_off {
        RadioState::Disabled
    } else {
        RadioState::Unknown
    }
}

fn changed() -> NetworkError {
    NetworkError::new(
        NetworkErrorKind::DeviceChanged,
        "The selected Wi-Fi radio source changed. Refresh and choose it again.",
    )
}

fn source<C: NativeCalls>(
    calls: &C,
    handle: usize,
    record: &Record,
) -> Result<Vec<WLAN_PHY_RADIO_STATE>, NetworkError> {
    let entries = observations::interfaces(calls, handle)?;
    let mut matching = entries
        .into_iter()
        .filter(|entry| entry.InterfaceGuid == record.interface);
    let entry: WLAN_INTERFACE_INFO = matching.next().ok_or_else(changed)?;
    if matching.next().is_some() {
        return Err(changed());
    }
    if entry.strInterfaceDescription != record.description {
        return Err(changed());
    }
    let phys = observations::radio_phys(calls, handle, &record.interface)?;
    if phys.len() != record.phys.len()
        || phys
            .iter()
            .zip(&record.phys)
            .any(|(fresh, original)| fresh.dwPhyIndex != original.dwPhyIndex)
    {
        return Err(changed());
    }
    Ok(phys)
}

fn readback<C: NativeCalls>(
    calls: &C,
    handle: usize,
    context: &Context,
    record: &Record,
) -> Result<Vec<WLAN_PHY_RADIO_STATE>, NetworkError> {
    let valid = || {
        context.is_admitted()
            && context.incarnation() == record.incarnation
            && context.source_revision() == record.source_revision
    };
    if !valid() {
        return Err(changed());
    }
    let phys = source(calls, handle, record)?;
    if !valid() {
        return Err(changed());
    }
    Ok(phys)
}

// A successful driver write can change linked PHYs and emit ACM notifications.
// Admit only forward transitions to this request, with the same exact PHY IDs,
// hardware facts and retained watch incarnation. Never accept a reverse change.
fn applied_transition(
    fresh: &[WLAN_PHY_RADIO_STATE],
    expected: &[WLAN_PHY_RADIO_STATE],
    requested: DOT11_RADIO_STATE,
    accepted: bool,
) -> bool {
    fresh.len() == expected.len()
        && fresh.iter().zip(expected).all(|(fresh, previous)| {
            fresh.dwPhyIndex == previous.dwPhyIndex
                && fresh.dot11HardwareRadioState == previous.dot11HardwareRadioState
                && (fresh.dot11SoftwareRadioState == previous.dot11SoftwareRadioState
                    || (accepted
                        && previous.dot11SoftwareRadioState != requested
                        && fresh.dot11SoftwareRadioState == requested))
        })
}

pub(super) fn execute<C: NativeCalls>(
    calls: &C,
    handle: usize,
    context: &Context,
    record: Record,
    enabled: bool,
) -> NetworkRadioResult {
    let requested = if enabled {
        dot11_radio_state_on
    } else {
        dot11_radio_state_off
    };
    let mut expected = record.phys.clone();
    let mut accepted = false;
    let mut results = Vec::with_capacity(expected.len());
    for original in &record.phys {
        let revision = context.revision();
        let validation = (|| {
            if !context.has_authority()
                || context.incarnation() != record.incarnation
                || context.source_revision() != record.source_revision
                || (!accepted && revision != record.revision)
            {
                return Err(changed());
            }
            let fresh = source(calls, handle, &record)?;
            if !applied_transition(&fresh, &expected, requested, accepted)
                || context.revision() != revision
                || !context.has_authority()
                || context.source_revision() != record.source_revision
            {
                return Err(changed());
            }
            Ok(fresh)
        })();
        let initiation = match validation {
            Ok(fresh) => {
                expected = fresh;
                // Lookup uses the actual native identifier, never the row position.
                let phy = expected
                    .iter()
                    .find(|phy| phy.dwPhyIndex == original.dwPhyIndex)
                    .expect("validated full PHY identity set");
                if phy.dot11SoftwareRadioState == requested {
                    NetworkRadioInitiation::AlreadyObserved
                } else {
                    let write = WLAN_PHY_RADIO_STATE {
                        dwPhyIndex: phy.dwPhyIndex,
                        dot11SoftwareRadioState: requested,
                        // WlanSetInterface explicitly ignores this read-only field.
                        dot11HardwareRadioState: phy.dot11HardwareRadioState,
                    };
                    if context.revision() != revision
                        || !context.has_authority()
                        || context.source_revision() != record.source_revision
                    {
                        NetworkRadioInitiation::Unavailable(changed())
                    } else {
                        let status = calls.set_radio(handle, &record.interface, &write);
                        if status == 0 {
                            accepted = true;
                            NetworkRadioInitiation::Accepted
                        } else {
                            NetworkRadioInitiation::Unavailable(native_error(
                                "WLAN software radio initiation",
                                status,
                            ))
                        }
                    }
                }
            }
            Err(error) => NetworkRadioInitiation::Unavailable(error),
        };
        // Readback is independent even after denied/stale initiation. A native
        // success without requested software readback remains unconfirmed.
        let readback = match readback(calls, handle, context, &record) {
            Ok(fresh) => {
                let result = fresh
                    .iter()
                    .find(|phy| phy.dwPhyIndex == original.dwPhyIndex)
                    .map(observed)
                    .ok_or_else(changed);
                if applied_transition(&fresh, &expected, requested, accepted) {
                    expected = fresh;
                }
                match result {
                    Ok(phy) => Observation::Ready(phy),
                    Err(error) => Observation::Unavailable(error),
                }
            }
            Err(error) => Observation::Unavailable(error),
        };
        results.push(NetworkRadioPhyResult {
            id: original.dwPhyIndex,
            initiation,
            readback,
        });
    }
    // Final same-source readback observes linked PHY updates from later writes,
    // without another write, retry loop or fabricated effective/hardware state.
    let final_readback = readback(calls, handle, context, &record);
    let readback_error = final_readback.as_ref().err().cloned();
    if let Ok(phys) = final_readback {
        for result in &mut results {
            result.readback = phys
                .iter()
                .find(|phy| phy.dwPhyIndex == result.id)
                .map(|phy| Observation::Ready(observed(phy)))
                .unwrap_or_else(|| Observation::Unavailable(changed()));
        }
    }
    NetworkRadioResult {
        target: record.target,
        interface: tessera_system::network::InterfaceId::new(
            record.interface.to_u128().to_be_bytes(),
        ),
        requested: enabled,
        phys: results,
        readback_error,
    }
}
