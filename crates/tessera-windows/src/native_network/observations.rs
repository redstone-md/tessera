// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::mem::offset_of;

use tessera_system::network::{NetworkError, RadioState, Ssid, WifiBss};
use windows::Win32::NetworkManagement::WiFi::*;
use windows::core::GUID;

use super::buffer::Buffer;
use super::calls::{NativeCalls, invalid};

pub(super) fn interfaces<C: NativeCalls>(
    calls: &C,
    handle: usize,
) -> Result<Vec<WLAN_INTERFACE_INFO>, NetworkError> {
    let buffer = Buffer::acquire(
        calls,
        calls.interfaces(handle),
        "WLAN interface enumeration",
    )?;
    let count = buffer.read::<u32>(offset_of!(WLAN_INTERFACE_INFO_LIST, dwNumberOfItems))?;
    buffer.list(
        offset_of!(WLAN_INTERFACE_INFO_LIST, InterfaceInfo),
        count,
        256,
    )
}

pub(super) fn radio<C: NativeCalls>(
    calls: &C,
    handle: usize,
    id: &GUID,
) -> Result<RadioState, NetworkError> {
    // Aggregate facts do not grant per-PHY mutation authority.
    Ok(radio_from_phys(&read_radio_phys(calls, handle, id)?))
}

pub(super) fn radio_phys<C: NativeCalls>(
    calls: &C,
    handle: usize,
    id: &GUID,
) -> Result<Vec<WLAN_PHY_RADIO_STATE>, NetworkError> {
    let mut phys = read_radio_phys(calls, handle, id)?;
    phys.sort_by_key(|phy| phy.dwPhyIndex);
    if phys
        .windows(2)
        .any(|pair| pair[0].dwPhyIndex == pair[1].dwPhyIndex)
    {
        return Err(invalid("WLAN returned duplicate radio PHY identifiers"));
    }
    Ok(phys)
}

fn read_radio_phys<C: NativeCalls>(
    calls: &C,
    handle: usize,
    id: &GUID,
) -> Result<Vec<WLAN_PHY_RADIO_STATE>, NetworkError> {
    let buffer = Buffer::acquire(
        calls,
        calls.query(handle, id, wlan_intf_opcode_radio_state),
        "WLAN radio observation",
    )?;
    let count = buffer.read::<u32>(offset_of!(WLAN_RADIO_STATE, dwNumberOfPhys))?;
    buffer.list(offset_of!(WLAN_RADIO_STATE, PhyRadioState), count, 64)
}

fn radio_from_phys(phys: &[WLAN_PHY_RADIO_STATE]) -> RadioState {
    if phys.iter().any(|phy| {
        phy.dot11SoftwareRadioState == dot11_radio_state_on
            && phy.dot11HardwareRadioState == dot11_radio_state_on
    }) {
        RadioState::Enabled
    } else if !phys.is_empty()
        && phys.iter().all(|phy| {
            phy.dot11SoftwareRadioState == dot11_radio_state_off
                || phy.dot11HardwareRadioState == dot11_radio_state_off
        })
    {
        RadioState::Disabled
    } else {
        RadioState::Unknown
    }
}

struct Metadata {
    ssid: Ssid,
    bss_type: DOT11_BSS_TYPE,
    secured: bool,
    known: bool,
    auth: DOT11_AUTH_ALGORITHM,
}

fn available<C: NativeCalls>(
    calls: &C,
    handle: usize,
    id: &GUID,
) -> Result<Vec<Metadata>, NetworkError> {
    let buffer = Buffer::acquire(
        calls,
        calls.available(handle, id, 0),
        "WLAN available-cache observation",
    )?;
    let count = buffer.read::<u32>(offset_of!(WLAN_AVAILABLE_NETWORK_LIST, dwNumberOfItems))?;
    buffer
        .list::<WLAN_AVAILABLE_NETWORK>(
            offset_of!(WLAN_AVAILABLE_NETWORK_LIST, Network),
            count,
            4096,
        )?
        .into_iter()
        .map(|entry| {
            Ok(Metadata {
                ssid: ssid(&entry.dot11Ssid)?,
                bss_type: entry.dot11BssType,
                secured: entry.bSecurityEnabled.as_bool(),
                known: entry.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE != 0,
                auth: entry.dot11DefaultAuthAlgorithm,
            })
        })
        .collect()
}

struct Current {
    ssid: Ssid,
    bssid: [u8; 6],
    bss_type: DOT11_BSS_TYPE,
}

fn current<C: NativeCalls>(
    calls: &C,
    handle: usize,
    id: &GUID,
) -> Result<Option<Current>, NetworkError> {
    let buffer = Buffer::acquire(
        calls,
        calls.query(handle, id, wlan_intf_opcode_current_connection),
        "WLAN current-connection observation",
    )?;
    let entry = buffer.read::<WLAN_CONNECTION_ATTRIBUTES>(0)?;
    match entry.isState {
        state
            if state == wlan_interface_state_connected
                || state == wlan_interface_state_ad_hoc_network_formed =>
        {
            let association = entry.wlanAssociationAttributes;
            Ok(Some(Current {
                ssid: ssid(&association.dot11Ssid)?,
                bssid: association.dot11Bssid,
                bss_type: association.dot11BssType,
            }))
        }
        state if state == wlan_interface_state_disconnected => Ok(None),
        _ => Err(invalid(
            "WLAN current connection is in a transitional or unknown state",
        )),
    }
}

pub(super) fn discovery<C: NativeCalls>(
    calls: &C,
    handle: usize,
    id: &GUID,
    interface_state: WLAN_INTERFACE_STATE,
) -> Result<Vec<WifiBss>, NetworkError> {
    let metadata = available(calls, handle, id);
    // Avoid following a privacy denial with two more location-sensitive calls.
    if let Err(error) = &metadata
        && error.kind == tessera_system::network::NetworkErrorKind::AccessDenied
    {
        return Err(error.clone());
    }
    let mut buffer = Buffer::acquire(calls, calls.bss(handle, id), "WLAN BSS-cache observation")?;
    let size = buffer.read::<u32>(offset_of!(WLAN_BSS_LIST, dwTotalSize))? as usize;
    let count = buffer.read::<u32>(offset_of!(WLAN_BSS_LIST, dwNumberOfItems))? as usize;
    let offset = offset_of!(WLAN_BSS_LIST, wlanBssEntries);
    if size < offset || count > 65536 {
        return Err(invalid("WLAN BSS header contains an invalid size or count"));
    }
    buffer.limit(size)?;
    buffer.span(offset, count, size_of::<WLAN_BSS_ENTRY>())?;
    // A successful empty cache is confirmed; do not access entry zero or query
    // location-sensitive current connection solely to decorate an empty list.
    if count == 0 {
        return Ok(Vec::new());
    }
    // A successful enumeration can confirm disconnected without the extra
    // location-sensitive query (which commonly returns ERROR_INVALID_STATE when
    // disconnected). This cache snapshot is deliberately non-atomic; transitional
    // or unknown enum states still require the actual current observation.
    let current = if interface_state == wlan_interface_state_disconnected {
        Ok(None)
    } else {
        current(calls, handle, id)
    };
    (0..count)
        .map(|index| {
            let start = offset + index * size_of::<WLAN_BSS_ENTRY>();
            // Read only fields used by this tranche, not the SDK struct's Rust bool,
            // beacon IE bytes, rate arrays or untrusted IE offsets.
            let raw_ssid =
                buffer.read::<DOT11_SSID>(start + offset_of!(WLAN_BSS_ENTRY, dot11Ssid))?;
            let ssid = ssid(&raw_ssid)?;
            let bssid = buffer.read::<[u8; 6]>(start + offset_of!(WLAN_BSS_ENTRY, dot11Bssid))?;
            let bss_type =
                buffer.read::<DOT11_BSS_TYPE>(start + offset_of!(WLAN_BSS_ENTRY, dot11BssType))?;
            let phy = buffer
                .read::<DOT11_PHY_TYPE>(start + offset_of!(WLAN_BSS_ENTRY, dot11BssPhyType))?;
            let privacy = buffer
                .read::<u16>(start + offset_of!(WLAN_BSS_ENTRY, usCapabilityInformation))?
                & 0x10
                != 0;
            let signal = buffer.read::<u32>(start + offset_of!(WLAN_BSS_ENTRY, uLinkQuality))?;
            if signal > 100 {
                return Err(invalid("WLAN signal quality is outside 0..=100"));
            }
            let matched: Vec<_> = metadata
                .as_ref()
                .ok()
                .into_iter()
                .flatten()
                .filter(|entry| {
                    !ssid.as_bytes().is_empty()
                        && entry.ssid == ssid
                        && entry.bss_type == bss_type
                        && entry.secured == privacy
                })
                .collect();
            let connected = match &current {
                Err(_) => None,
                Ok(None) => Some(false),
                Ok(Some(connection)) if connection.bssid != bssid => Some(false),
                Ok(Some(connection))
                    if connection.ssid == ssid && connection.bss_type == bss_type =>
                {
                    Some(true)
                }
                // Same AP address with conflicting cached identity/type can be a
                // hidden beacon or a non-atomic read, not proof of disconnection.
                Ok(Some(_)) => None,
            };
            Ok(WifiBss {
                ssid,
                bssid,
                // Windows documents this frequency field as invalid for FHSS.
                frequency_khz: if phy == dot11_phy_type_fhss {
                    0
                } else {
                    buffer.read(start + offset_of!(WLAN_BSS_ENTRY, ulChCenterFrequency))?
                },
                signal_percent: signal as u8,
                rssi_dbm: buffer.read(start + offset_of!(WLAN_BSS_ENTRY, lRssi))?,
                known: consensus(matched.iter().map(|entry| entry.known)),
                secured: consensus(matched.iter().map(|entry| entry.secured)),
                connected,
                auth: consensus(matched.iter().map(|entry| entry.auth))
                    .and_then(auth_label)
                    .map(str::to_owned),
            })
        })
        .collect()
}

fn ssid(raw: &DOT11_SSID) -> Result<Ssid, NetworkError> {
    let length = raw.uSSIDLength as usize;
    if length > raw.ucSSID.len() {
        return Err(invalid("WLAN SSID exceeds the 32-byte identity limit"));
    }
    Ssid::new(&raw.ucSSID[..length])
}

fn consensus<T: Copy + PartialEq>(mut values: impl Iterator<Item = T>) -> Option<T> {
    let first = values.next()?;
    values.all(|value| value == first).then_some(first)
}

fn auth_label(auth: DOT11_AUTH_ALGORITHM) -> Option<&'static str> {
    match auth {
        value if value == DOT11_AUTH_ALGO_80211_OPEN => Some("Open"),
        value if value == DOT11_AUTH_ALGO_80211_SHARED_KEY => Some("Shared key"),
        value if value == DOT11_AUTH_ALGO_WPA => Some("WPA Enterprise"),
        value if value == DOT11_AUTH_ALGO_WPA_PSK => Some("WPA Personal"),
        value if value == DOT11_AUTH_ALGO_WPA_NONE => Some("WPA None"),
        value if value == DOT11_AUTH_ALGO_RSNA => Some("WPA2 Enterprise"),
        value if value == DOT11_AUTH_ALGO_RSNA_PSK => Some("WPA2 Personal"),
        value if value == DOT11_AUTH_ALGO_WPA3_ENT_192 => Some("WPA3 Enterprise 192-bit"),
        value if value == DOT11_AUTH_ALGO_WPA3_SAE => Some("WPA3 Personal"),
        value if value == DOT11_AUTH_ALGO_OWE => Some("Enhanced Open"),
        value if value == DOT11_AUTH_ALGO_WPA3_ENT => Some("WPA3 Enterprise"),
        _ => None,
    }
}
