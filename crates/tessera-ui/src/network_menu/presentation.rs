// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::collections::{BTreeMap, BTreeSet};

use tessera_system::network::{
    InterfaceId, NetworkErrorKind, NetworkSnapshot, Observation, RadioState, WifiBss,
};

use crate::generated::NetworkRow;
use crate::sanitize::bounded_text;

#[derive(Default)]
pub(super) struct Projection {
    pub connected: Vec<NetworkRow>,
    pub saved: Vec<NetworkRow>,
    pub available: Vec<NetworkRow>,
    pub hidden: Vec<NetworkRow>,
    pub radio: String,
    pub summary: String,
}

pub(super) fn denied(snapshot: &NetworkSnapshot) -> bool {
    snapshot.interfaces.iter().any(|interface| {
        matches!(&interface.radio, Observation::Unavailable(error) if error.kind == NetworkErrorKind::AccessDenied)
            || matches!(&interface.discovery, Observation::Unavailable(error) if error.kind == NetworkErrorKind::AccessDenied)
    })
}

fn connection_unknown(snapshot: &NetworkSnapshot) -> bool {
    snapshot.interfaces.iter().any(|interface| {
        matches!(&interface.discovery, Observation::Ready(entries)
            if entries.iter().any(|entry| entry.connected.is_none()))
    })
}

pub(super) fn automatic_retry_blocked(snapshot: &NetworkSnapshot) -> bool {
    // The fixed portable snapshot cannot distinguish current-connection
    // privacy denial from unavailable/ambiguous metadata. Be conservative
    // without claiming that unknown data proves denial.
    denied(snapshot) || connection_unknown(snapshot)
}

pub(super) fn failure(kind: NetworkErrorKind) -> &'static str {
    match kind {
        NetworkErrorKind::Unsupported => "Wi-Fi information is not supported on this platform.",
        NetworkErrorKind::AccessDenied => {
            "Windows denied Wi-Fi access. Check location permissions in Settings, then Refresh."
        }
        NetworkErrorKind::ServiceUnavailable => {
            "Windows WLAN service is unavailable. Check Settings, then Refresh."
        }
        NetworkErrorKind::DeviceChanged => {
            "The Wi-Fi adapter changed. Refresh to read the current adapters."
        }
        NetworkErrorKind::Busy => "The network provider is busy. Try Refresh again.",
        NetworkErrorKind::Stopped => "The network provider has stopped. Try Refresh again.",
        NetworkErrorKind::InvalidData => {
            "Windows Wi-Fi information could not be validated. Try Refresh again."
        }
        NetworkErrorKind::Other => "Wi-Fi information could not be read. Try Refresh again.",
    }
}

fn adapter_name(name: &str) -> String {
    let name = bounded_text(name, 80);
    if name.is_empty() {
        "Wi-Fi adapter".into()
    } else {
        name
    }
}

fn bssid(value: &[u8; 6]) -> String {
    value
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn row(name: &str, raw_ssid: &[u8], entries: &[&WifiBss]) -> NetworkRow {
    let strongest = entries
        .iter()
        .max_by_key(|entry| entry.signal_percent)
        .expect("nonempty BSS group");
    let label = if raw_ssid.is_empty() {
        format!("Hidden Network ({})", entries.len())
    } else {
        let decoded = bounded_text(&strongest.ssid.display_name(), 96);
        if decoded.is_empty() {
            "Unnamed network".into()
        } else {
            decoded
        }
    };
    let mut bands = BTreeSet::new();
    let mut auth = BTreeSet::new();
    let mut connected = Vec::new();
    let mut access_points = Vec::new();
    for entry in entries {
        match entry.frequency_khz {
            2_400_000..=2_484_000 => {
                bands.insert("2.4G");
            }
            5_000_000..=5_850_000 => {
                bands.insert("5G");
            }
            5_925_000..=7_125_000 => {
                bands.insert("6G");
            }
            _ => {}
        }
        if let Some(value) = &entry.auth {
            let value = bounded_text(value, 48);
            if !value.is_empty() {
                auth.insert(value);
            }
        }
        if entry.connected == Some(true) {
            connected.push(bssid(&entry.bssid));
        }
        access_points.push(format!(
            "{}: {}%, {} dBm, {} kHz, {}",
            bssid(&entry.bssid),
            entry.signal_percent.min(100),
            entry.rssi_dbm,
            entry.frequency_khz,
            match entry.connected {
                Some(true) => "current AP",
                Some(false) => "not current AP",
                None => "connection unknown",
            }
        ));
    }
    let security = if entries.iter().all(|entry| entry.secured == Some(true)) {
        "Secured"
    } else if entries.iter().all(|entry| entry.secured == Some(false)) {
        "Open"
    } else {
        "Security unknown or mixed"
    };
    let profile = if entries.iter().any(|entry| entry.known == Some(true)) {
        "Saved profile"
    } else if entries.iter().all(|entry| entry.known == Some(false)) {
        "No saved profile"
    } else {
        "Saved profile unknown"
    };
    let connection = if !connected.is_empty() {
        format!("Current AP {}", connected.join(", "))
    } else if entries.iter().all(|entry| entry.connected == Some(false)) {
        "Not connected".into()
    } else {
        "Connection unknown".into()
    };
    let auth_text = if auth.is_empty() {
        "Authentication unknown".into()
    } else {
        let known = auth.into_iter().collect::<Vec<_>>().join(", ");
        if entries.iter().any(|entry| {
            entry
                .auth
                .as_ref()
                .is_none_or(|value| bounded_text(value, 48).is_empty())
        }) {
            format!("{known}; some AP authentication unknown")
        } else {
            known
        }
    };
    let details = format!(
        "{name} · {}% signal · {security} · {profile} · {connection}",
        strongest.signal_percent.min(100)
    );
    NetworkRow {
        label: label.clone().into(),
        details: details.clone().into(),
        bands: bands.into_iter().collect::<Vec<_>>().join(" / ").into(),
        description: format!(
            "{label}; {details}; {auth_text}; {}",
            access_points.join("; ")
        )
        .into(),
    }
}

// Interface plus raw SSID is the identity. Lossy/sanitized labels never join
// groups, and only exact per-BSS connected observations select this section.
pub(super) fn project(snapshot: &NetworkSnapshot) -> Projection {
    let mut result = Projection::default();
    if snapshot.interfaces.is_empty() {
        result.radio = "No Wi-Fi adapter found".into();
        result.summary = "Windows confirmed that no Wi-Fi adapter is available.".into();
        return result;
    }
    let mut radios = Vec::new();
    let mut errors = Vec::new();
    let mut rows = Vec::new();
    let mut ready = 0;
    let mut off = 0;
    let mut groups = BTreeMap::<(InterfaceId, Vec<u8>), (String, Vec<&WifiBss>)>::new();
    for interface in &snapshot.interfaces {
        let name = adapter_name(&interface.name);
        let radio = match &interface.radio {
            Observation::Ready(RadioState::Enabled) => "On",
            Observation::Ready(RadioState::Disabled) => {
                off += 1;
                "Off"
            }
            Observation::Ready(RadioState::Unknown) => "Unknown",
            Observation::Unavailable(error) => {
                errors.push(format!("{name}: {}", failure(error.kind)));
                "Unavailable"
            }
        };
        radios.push(format!("{name}: Wi-Fi {radio}"));
        match &interface.discovery {
            Observation::Ready(entries) => {
                ready += 1;
                // Confirmed off adapters do not expose previously cached rows as
                // currently available, while their raw snapshot remains retained.
                if matches!(&interface.radio, Observation::Ready(RadioState::Disabled)) {
                    continue;
                }
                for entry in entries {
                    groups
                        .entry((interface.id, entry.ssid.as_bytes().to_vec()))
                        .or_insert_with(|| (name.clone(), Vec::new()))
                        .1
                        .push(entry);
                }
            }
            Observation::Unavailable(error) => {
                errors.push(format!("{name}: {}", failure(error.kind)))
            }
        }
    }
    for ((id, ssid), (name, entries)) in groups {
        let category = if ssid.is_empty() {
            3
        } else if entries.iter().any(|entry| entry.connected == Some(true)) {
            0
        } else if entries.iter().any(|entry| entry.known == Some(true)) {
            1
        } else {
            2
        };
        let signal = entries
            .iter()
            .map(|entry| entry.signal_percent)
            .max()
            .unwrap_or(0);
        let item = row(&name, &ssid, &entries);
        rows.push((category, std::cmp::Reverse(signal), id, ssid, item));
    }
    rows.sort_by(|left, right| {
        (&left.0, &left.1, &left.2, &left.3).cmp(&(&right.0, &right.1, &right.2, &right.3))
    });
    for (category, _, _, _, row) in rows {
        match category {
            0 => result.connected.push(row),
            1 => result.saved.push(row),
            2 => result.available.push(row),
            _ => result.hidden.push(row),
        }
    }
    result.radio = radios.join(" · ");
    if !errors.is_empty() {
        result.summary = format!(
            "{}{}",
            if ready > 0 {
                "Partial cache read. "
            } else {
                ""
            },
            errors.join(" ")
        );
    } else if off == snapshot.interfaces.len() {
        result.summary = "Wi-Fi is off. Enable it in Windows Settings, then Refresh.".into();
    } else if result.connected.is_empty()
        && result.saved.is_empty()
        && result.available.is_empty()
        && result.hidden.is_empty()
    {
        result.summary =
            "No Wi-Fi networks in the Windows cache. Refresh re-reads the cache; it does not scan."
                .into();
    } else {
        result.summary = "Saved networks lists discovered networks with saved profiles, not all saved profiles. No Internet connectivity is asserted.".into();
    }
    if connection_unknown(snapshot) {
        result.summary.push_str(" Connection metadata is unavailable or ambiguous; automatic cache rereads are paused. Use Refresh.");
    }
    result
}
