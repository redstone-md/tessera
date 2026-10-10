// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native target authority and documented, non-persistent WLAN connection profiles.
use super::buffer::Buffer;
use super::callback::Context;
use super::calls::{ConnectRequest, NativeCalls, invalid, native_error};
use super::observations;
use std::mem::offset_of;
use std::sync::atomic::{AtomicU64, Ordering};
use tessera_system::network::{
    InterfaceId, NetworkCommand, NetworkCommandOutcome, NetworkConnectCapability, NetworkControl,
    NetworkControlInventory, NetworkError, NetworkErrorKind, NetworkProfileInitiation,
    NetworkProfileInventory, NetworkProfileResult, NetworkRadioControl, NetworkRadioInitiation,
    NetworkRadioInventory, NetworkRadioResult, NetworkSnapshot, NetworkTarget, Observation, Ssid,
};
use windows::Win32::NetworkManagement::WiFi::*;
use windows::Win32::Storage::FileSystem::{
    FILE_EXECUTE, FILE_READ_DATA, STANDARD_RIGHTS_EXECUTE, STANDARD_RIGHTS_READ,
};
use windows::core::GUID;

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
// Published wlanapi.h macro, expressed through the pinned SDK access constants.
const WLAN_EXECUTE_ACCESS: u32 =
    STANDARD_RIGHTS_READ.0 | FILE_READ_DATA.0 | STANDARD_RIGHTS_EXECUTE.0 | FILE_EXECUTE.0;

#[derive(Clone, PartialEq)]
struct Record {
    target: NetworkTarget,
    interface: GUID,
    description: [u16; 256],
    ssid: Ssid,
    bssid: [u8; 6],
    auth: DOT11_AUTH_ALGORITHM,
    cipher: DOT11_CIPHER_ALGORITHM,
    secured: bool,
    profile: Vec<u16>,
    profile_xml: Vec<u16>,
    profile_flags: u32,
    granted_access: u32,
    connected: bool,
    capability: NetworkConnectCapability,
}

pub(super) struct Controls {
    owner: u64,
    sequence: u64,
    revision: u64,
    watch: u64,
    records: Vec<Record>,
    pending: Option<(Record, bool)>,
    radios: Vec<super::radio::Record>,
    radio_result: Option<NetworkRadioResult>,
    profiles: Vec<super::profiles::Record>,
    profile_result: Option<NetworkProfileResult>,
}

impl Controls {
    pub(super) fn new() -> Self {
        Self {
            owner: NEXT_OWNER
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                    (next != 0 && next < u64::MAX).then(|| next + 1)
                })
                .unwrap_or(0),
            sequence: 0,
            revision: 0,
            watch: 0,
            records: Vec::new(),
            pending: None,
            radios: Vec::new(),
            radio_result: None,
            profiles: Vec::new(),
            profile_result: None,
        }
    }

    pub(super) fn retire(&mut self) {
        self.pending = None;
        self.radio_result = None;
        self.profile_result = None;
    }

    fn has_authority(&self) -> bool {
        self.owner != 0 && self.owner != u64::MAX && self.sequence != u64::MAX
    }

    fn target(&mut self, revision: u64) -> Result<NetworkTarget, NetworkError> {
        if !self.has_authority() {
            return Err(watch_unavailable());
        }
        let Some(sequence) = self.sequence.checked_add(1).filter(|next| *next < u64::MAX) else {
            self.sequence = u64::MAX;
            self.records.clear();
            self.radios.clear();
            self.profiles.clear();
            return Err(watch_unavailable());
        };
        self.sequence = sequence;
        Ok(NetworkTarget::new(self.owner, sequence, revision))
    }

    pub(super) fn radio_inventory<C: NativeCalls>(
        &mut self,
        calls: &C,
        handle: usize,
        context: Option<&Context>,
    ) -> Result<NetworkRadioInventory, NetworkError> {
        self.radios.clear();
        let admitted = context.filter(|context| context.has_authority() && self.has_authority());
        let revision = admitted.map(Context::revision);
        let source_revision = admitted.map(Context::source_revision);
        let mut interfaces = Vec::new();
        let mut unavailable = Vec::new();
        if admitted.is_none() {
            unavailable.push(watch_unavailable());
        }
        for entry in observations::interfaces(calls, handle)? {
            let phys = match observations::radio_phys(calls, handle, &entry.InterfaceGuid) {
                Ok(phys) => phys,
                Err(error) => {
                    unavailable.push(error);
                    continue;
                }
            };
            let target = if !phys.is_empty() && admitted.is_some() && self.has_authority() {
                match self.target(revision.expect("admitted revision")) {
                    Ok(target) => Some(target),
                    Err(error) => {
                        unavailable.push(error);
                        None
                    }
                }
            } else {
                None
            };
            let length = entry
                .strInterfaceDescription
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(entry.strInterfaceDescription.len());
            interfaces.push(NetworkRadioControl {
                target,
                interface: InterfaceId::new(entry.InterfaceGuid.to_u128().to_be_bytes()),
                interface_name: String::from_utf16_lossy(&entry.strInterfaceDescription[..length]),
                phys: phys.iter().map(super::radio::observed).collect(),
            });
            if let Some(target) = target {
                let context = admitted.expect("admitted radio target");
                self.radios.push(super::radio::Record {
                    target,
                    interface: entry.InterfaceGuid,
                    description: entry.strInterfaceDescription,
                    incarnation: context.incarnation(),
                    revision: context.revision(),
                    source_revision: context.source_revision(),
                    phys,
                });
            }
        }
        if let Some(context) = admitted
            && (Some(context.revision()) != revision
                || Some(context.source_revision()) != source_revision
                || !context.has_authority()
                || !self.has_authority())
        {
            self.radios.clear();
            for interface in &mut interfaces {
                interface.target = None;
            }
            unavailable.push(changed());
        }
        Ok(NetworkRadioInventory {
            interfaces,
            unavailable,
        })
    }

    pub(super) fn profile_inventory<C: NativeCalls>(
        &mut self,
        calls: &C,
        handle: usize,
        context: Option<&Context>,
    ) -> Result<NetworkProfileInventory, NetworkError> {
        self.profiles.clear();
        let (mut inventory, records) =
            super::profiles::inventory(calls, handle, context, |revision| self.target(revision))?;
        if self.has_authority() {
            self.profiles = records;
        } else {
            for profile in &mut inventory.profiles {
                profile.target = None;
                profile.forget = Observation::Unavailable(watch_unavailable());
            }
            inventory.unavailable.push(watch_unavailable());
        }
        Ok(inventory)
    }

    pub(super) fn accepted(&self) -> bool {
        self.profile_result
            .as_ref()
            .is_some_and(|result| result.initiation == NetworkProfileInitiation::Accepted)
            || self.pending.is_some()
            || self.radio_result.as_ref().is_some_and(|result| {
                result
                    .phys
                    .iter()
                    .any(|phy| phy.initiation == NetworkRadioInitiation::Accepted)
            })
    }

    pub(super) fn inventory<C: NativeCalls>(
        &mut self,
        calls: &C,
        handle: usize,
        context: Option<&Context>,
        snapshot: &NetworkSnapshot,
    ) -> Result<NetworkControlInventory, NetworkError> {
        self.records.clear();
        if !self.has_authority() {
            return Err(watch_unavailable());
        }
        let context = context
            .filter(|context| context.has_authority())
            .ok_or_else(watch_unavailable)?;
        let revision = context.revision();
        if revision == 0 || revision == u64::MAX {
            return Err(watch_unavailable());
        }
        self.revision = revision;
        self.watch = context.incarnation();
        let interfaces = observations::interfaces(calls, handle)?;
        let mut result = Vec::new();
        let mut unavailable = Vec::new();
        for entry in interfaces {
            let id = InterfaceId::new(entry.InterfaceGuid.to_u128().to_be_bytes());
            let Some(interface) = snapshot
                .interfaces
                .iter()
                .find(|interface| interface.id == id)
            else {
                continue;
            };
            let bss = match &interface.discovery {
                Observation::Ready(bss) => bss,
                Observation::Unavailable(error) => {
                    unavailable.push(error.clone());
                    continue;
                }
            };
            // One failed adapter never removes successful observations of another.
            let networks = match available(calls, handle, &entry.InterfaceGuid) {
                Ok(networks) => networks,
                Err(error) => {
                    unavailable.push(error);
                    continue;
                }
            };
            for bss in bss {
                if bss.ssid.as_bytes().is_empty() || bss.connected.is_none() {
                    if !unavailable
                        .iter()
                        .any(|error: &NetworkError| error.kind == NetworkErrorKind::Unsupported)
                    {
                        unavailable.push(NetworkError::new(NetworkErrorKind::Unsupported, "Hidden or unconfirmed network identities are unavailable for connection controls."));
                    }
                    continue;
                }
                let mut record = match record(calls, handle, &entry, bss, &networks) {
                    Ok(record) => record,
                    Err(error) => {
                        if !unavailable.contains(&error) {
                            unavailable.push(error);
                        }
                        continue;
                    }
                };
                record.target = self.target(revision)?;
                result.push(NetworkControl {
                    target: record.target,
                    interface: id,
                    interface_name: interface.name.clone(),
                    ssid: record.ssid.clone(),
                    connect: record.capability,
                    connected: record.connected,
                    bssid: record.bssid,
                });
                self.records.push(record);
            }
        }
        if context.revision() != revision || !context.has_authority() {
            self.records.clear();
            return Err(changed());
        }
        Ok(NetworkControlInventory {
            networks: result,
            unavailable,
        })
    }

    pub(super) fn begin<C: NativeCalls>(
        &mut self,
        calls: &C,
        handle: usize,
        context: Option<&Context>,
        command: NetworkCommand,
    ) -> Result<(), NetworkError> {
        if !self.has_authority() || self.sequence == 0 {
            return Err(watch_unavailable());
        }
        if let NetworkCommand::ForgetProfile { target } = &command {
            let record = self
                .profiles
                .iter()
                .find(|record| record.target == *target)
                .cloned()
                .ok_or_else(changed)?;
            self.profile_result = Some(super::profiles::execute(calls, handle, context, record));
            self.records.clear();
            self.radios.clear();
            self.profiles.clear();
            return Ok(());
        }
        let context = context
            .filter(|context| context.has_authority())
            .ok_or_else(watch_unavailable)?;
        if let NetworkCommand::SetRadio { target, enabled } = &command {
            let record = self
                .radios
                .iter()
                .find(|record| record.target == *target)
                .cloned()
                .ok_or_else(changed)?;
            // Admission failure before a write cannot become a native ACK.
            if record.revision != context.revision()
                || record.incarnation != context.incarnation()
                || record.source_revision != context.source_revision()
            {
                return Err(changed());
            }
            self.radio_result = Some(super::radio::execute(
                calls, handle, context, record, *enabled,
            ));
            self.records.clear();
            self.radios.clear();
            return Ok(());
        }
        let (target, disconnect, password) = match command {
            NetworkCommand::Connect { target, password } => (target, false, password),
            NetworkCommand::Disconnect { target } => (target, true, None),
            NetworkCommand::SetRadio { .. } | NetworkCommand::ForgetProfile { .. } => {
                unreachable!("independent native controls handled above")
            }
        };
        let previous = self
            .records
            .iter()
            .find(|record| record.target == target)
            .cloned()
            .ok_or_else(changed)?;
        let revision = context.revision();
        if self.revision != revision || self.watch != context.incarnation() {
            return Err(changed());
        }
        let entry = observations::interfaces(calls, handle)?
            .into_iter()
            .find(|entry| entry.InterfaceGuid == previous.interface)
            .ok_or_else(changed)?;
        if entry.strInterfaceDescription != previous.description {
            return Err(changed());
        }
        let bss = observations::discovery(calls, handle, &entry.InterfaceGuid, entry.isState)?;
        let bss = bss
            .iter()
            .find(|bss| bss.ssid == previous.ssid && bss.bssid == previous.bssid)
            .ok_or_else(changed)?;
        let networks = available(calls, handle, &entry.InterfaceGuid)?;
        let mut fresh = record(calls, handle, &entry, bss, &networks)?;
        fresh.target = previous.target;
        if fresh != previous || context.revision() != revision {
            return Err(changed());
        }
        if disconnect && !fresh.connected {
            return Err(changed());
        }
        if !disconnect
            && (fresh.connected || fresh.capability == NetworkConnectCapability::Unavailable)
        {
            return Err(NetworkError::new(
                NetworkErrorKind::Unsupported,
                "This network's connection security is unavailable or unsupported.",
            ));
        }
        let temporary = !disconnect && fresh.capability != NetworkConnectCapability::SavedProfile;
        let profile = if disconnect {
            SensitiveWide(Vec::new())
        } else if temporary {
            temporary_profile(&fresh, password.as_ref())?
        } else {
            if password.is_some() {
                return Err(invalid(
                    "Saved profiles do not accept replacement credentials here.",
                ));
            }
            SensitiveWide(fresh.profile.clone())
        };
        // A second callback revision check closes credential-entry/query races.
        if context.revision() != revision || !context.has_authority() {
            return Err(changed());
        }
        context.observe_effect(fresh.interface, fresh.ssid.as_bytes());
        let status = if disconnect {
            calls.disconnect(handle, &fresh.interface)
        } else {
            calls.connect(
                handle,
                &fresh.interface,
                ConnectRequest {
                    ssid: fresh.ssid.as_bytes(),
                    bssid: fresh.bssid,
                    profile: &profile.0,
                    temporary,
                },
            )
        };
        // Password, escaped profile and all temporary copies retire on this return.
        drop(profile);
        drop(password);
        self.records.clear();
        if status != 0 {
            return Err(native_error("WLAN connection initiation", status));
        }
        self.pending = Some((fresh, disconnect));
        Ok(())
    }

    pub(super) fn progress<C: NativeCalls>(
        &mut self,
        calls: &C,
        handle: usize,
        context: Option<&Context>,
    ) -> Result<Option<NetworkCommandOutcome>, NetworkError> {
        if let Some(result) = self.profile_result.take() {
            return Ok(Some(NetworkCommandOutcome::ProfileObserved(result)));
        }
        if let Some(result) = self.radio_result.take() {
            return Ok(Some(NetworkCommandOutcome::RadioObserved(result)));
        }
        let Some((target, disconnect)) = self.pending.as_ref() else {
            return Ok(None);
        };
        let context = context
            .filter(|context| context.is_admitted())
            .ok_or_else(watch_unavailable)?;
        if let Some(reason) = context.failure_reason() {
            self.pending = None;
            return Ok(Some(NetworkCommandOutcome::Failed { reason }));
        }
        let interface = observations::interfaces(calls, handle)?
            .into_iter()
            .find(|entry| entry.InterfaceGuid == target.interface)
            .ok_or_else(changed)?;
        if interface.strInterfaceDescription != target.description {
            return Err(changed());
        }
        if *disconnect && interface.isState == wlan_interface_state_disconnected {
            self.pending = None;
            return Ok(Some(NetworkCommandOutcome::Disconnected));
        }
        if interface.isState != wlan_interface_state_connected {
            return Ok(None);
        }
        let buffer = Buffer::acquire(
            calls,
            calls.query(
                handle,
                &target.interface,
                wlan_intf_opcode_current_connection,
            ),
            "WLAN connection readback",
        )?;
        let current = buffer.read::<WLAN_CONNECTION_ATTRIBUTES>(0)?;
        let association = current.wlanAssociationAttributes;
        let security = current.wlanSecurityAttributes;
        let ssid = native_ssid(&association.dot11Ssid)?;
        if !*disconnect
            && current.isState == wlan_interface_state_connected
            && ssid == target.ssid
            && association.dot11Bssid == target.bssid
            && association.dot11BssType == dot11_BSS_type_infrastructure
            && security.dot11AuthAlgorithm == target.auth
            && security.dot11CipherAlgorithm == target.cipher
            && security.bSecurityEnabled.as_bool() == target.secured
        {
            self.pending = None;
            return Ok(Some(NetworkCommandOutcome::Connected));
        }
        Ok(None)
    }
}

fn watch_unavailable() -> NetworkError {
    NetworkError::new(
        NetworkErrorKind::Unsupported,
        "A working WLAN notification watch and native source authority are required for Wi-Fi controls.",
    )
}
fn changed() -> NetworkError {
    NetworkError::new(
        NetworkErrorKind::DeviceChanged,
        "The selected Wi-Fi observation changed. Refresh and choose it again.",
    )
}
fn native_ssid(ssid: &DOT11_SSID) -> Result<Ssid, NetworkError> {
    let length = ssid.uSSIDLength as usize;
    if length > 32 {
        return Err(invalid("WLAN returned an invalid network identity."));
    }
    Ssid::new(&ssid.ucSSID[..length])
}
fn available<C: NativeCalls>(
    calls: &C,
    handle: usize,
    id: &GUID,
) -> Result<Vec<WLAN_AVAILABLE_NETWORK>, NetworkError> {
    let buffer = Buffer::acquire(
        calls,
        calls.available(handle, id, 0),
        "WLAN connection capability observation",
    )?;
    let count = buffer.read::<u32>(offset_of!(WLAN_AVAILABLE_NETWORK_LIST, dwNumberOfItems))?;
    buffer.list(
        offset_of!(WLAN_AVAILABLE_NETWORK_LIST, Network),
        count,
        4096,
    )
}
fn record<C: NativeCalls>(
    calls: &C,
    handle: usize,
    interface: &WLAN_INTERFACE_INFO,
    bss: &tessera_system::network::WifiBss,
    networks: &[WLAN_AVAILABLE_NETWORK],
) -> Result<Record, NetworkError> {
    let matching = networks
        .iter()
        .filter(|network| {
            network.dot11BssType == dot11_BSS_type_infrastructure
                && native_ssid(&network.dot11Ssid).is_ok_and(|ssid| ssid == bss.ssid)
        })
        .collect::<Vec<_>>();
    let first = matching.first().ok_or_else(changed)?;
    if matching.iter().any(|network| {
        network.bSecurityEnabled != first.bSecurityEnabled
            || network.dot11DefaultAuthAlgorithm != first.dot11DefaultAuthAlgorithm
            || network.dot11DefaultCipherAlgorithm != first.dot11DefaultCipherAlgorithm
            || network.strProfileName != first.strProfileName
            || network.dwFlags != first.dwFlags
            || network.bNetworkConnectable != first.bNetworkConnectable
    }) {
        return Err(changed());
    }
    let secured = first.bSecurityEnabled.as_bool();
    if bss.secured != Some(secured) {
        return Err(changed());
    }
    let auth = first.dot11DefaultAuthAlgorithm;
    let cipher = first.dot11DefaultCipherAlgorithm;
    let supported =
        (auth == DOT11_AUTH_ALGO_80211_OPEN && cipher == DOT11_CIPHER_ALGO_NONE && !secured)
            || (auth == DOT11_AUTH_ALGO_RSNA_PSK && cipher == DOT11_CIPHER_ALGO_CCMP && secured);
    let known = first.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE != 0;
    let mut profile = Vec::new();
    let mut profile_xml = Vec::new();
    let mut profile_flags = 0;
    let mut granted_access = 0;
    if known {
        let end = first
            .strProfileName
            .iter()
            .position(|unit| *unit == 0)
            .ok_or_else(|| invalid("WLAN profile identity is not terminated."))?;
        if end == 0 {
            return Err(invalid("WLAN profile identity is empty."));
        }
        profile.extend_from_slice(&first.strProfileName[..=end]);
        let reply = calls.profile(handle, &interface.InterfaceGuid, &profile);
        profile_flags = reply.flags;
        granted_access = reply.granted_access;
        let buffer = Buffer::acquire(calls, reply.allocation, "WLAN managed-profile observation")?;
        if profile_flags & WLAN_PROFILE_USER == 0
            && granted_access & WLAN_EXECUTE_ACCESS != WLAN_EXECUTE_ACCESS
        {
            return Err(NetworkError::new(
                NetworkErrorKind::AccessDenied,
                "Windows denied execution access to the managed Wi-Fi profile.",
            ));
        }
        for index in 0..65536 {
            let unit = buffer.read::<u16>(index * 2)?;
            if unit == 0 {
                break;
            }
            profile_xml.push(unit);
            if index == 65535 {
                return Err(invalid(
                    "WLAN profile exceeds the bounded observation size.",
                ));
            }
        }
    }
    let capability = if !supported || !first.bNetworkConnectable.as_bool() {
        NetworkConnectCapability::Unavailable
    } else if known {
        NetworkConnectCapability::SavedProfile
    } else if secured {
        NetworkConnectCapability::Wpa2Personal
    } else {
        NetworkConnectCapability::Open
    };
    Ok(Record {
        target: NetworkTarget::new(0, 0, 0),
        interface: interface.InterfaceGuid,
        description: interface.strInterfaceDescription,
        ssid: bss.ssid.clone(),
        bssid: bss.bssid,
        auth,
        cipher,
        secured,
        profile,
        profile_xml,
        profile_flags,
        granted_access,
        connected: bss.connected.ok_or_else(changed)?,
        capability,
    })
}

struct SensitiveWide(Vec<u16>);
impl Drop for SensitiveWide {
    fn drop(&mut self) {
        for unit in &mut self.0 {
            // SAFETY: unique live element; prevent removal of credential wipes.
            unsafe { std::ptr::write_volatile(unit, 0) };
        }
    }
}
fn temporary_profile(
    record: &Record,
    password: Option<&tessera_system::network::NetworkPassword>,
) -> Result<SensitiveWide, NetworkError> {
    // Documented WPA2-Personal/SSID/sharedKey schema; raw hex is authoritative.
    // No WlanSetProfile or WlanSaveTemporaryProfile: credentials are not saved.
    let mut xml = SensitiveWide(Vec::with_capacity(1024));
    let mut append = |value: &str| xml.0.extend(value.encode_utf16());
    append(
        "<?xml version=\"1.0\"?><WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\"><name>Tessera temporary connection</name><SSIDConfig><SSID><hex>",
    );
    for byte in record.ssid.as_bytes() {
        append(&format!("{byte:02X}"));
    }
    append(
        "</hex></SSID></SSIDConfig><connectionType>ESS</connectionType><connectionMode>manual</connectionMode><MSM><security><authEncryption><authentication>",
    );
    append(if record.secured { "WPA2PSK" } else { "open" });
    append("</authentication><encryption>");
    append(if record.secured { "AES" } else { "none" });
    append("</encryption><useOneX>false</useOneX></authEncryption>");
    if record.secured {
        let password = password
            .ok_or_else(|| invalid("A WPA2-Personal password is required."))?
            .as_bytes();
        let network_key = password.len() == 64 && password.iter().all(u8::is_ascii_hexdigit);
        if !network_key
            && (!(8..=63).contains(&password.len())
                || !password.iter().all(|byte| (32..=126).contains(byte)))
        {
            return Err(invalid(
                "Use 8–63 printable ASCII password characters or a 64-digit hexadecimal key.",
            ));
        }
        append("<sharedKey><keyType>");
        append(if network_key {
            "networkKey"
        } else {
            "passPhrase"
        });
        append("</keyType><protected>false</protected><keyMaterial>");
        for byte in password {
            match byte {
                b'&' => append("&amp;"),
                b'<' => append("&lt;"),
                b'>' => append("&gt;"),
                b'\"' => append("&quot;"),
                b'\'' => append("&apos;"),
                byte => append(
                    std::str::from_utf8(std::slice::from_ref(byte))
                        .expect("validated ASCII password"),
                ),
            }
        }
        append("</keyMaterial></sharedKey>");
    } else if password.is_some() {
        return Err(invalid("Open networks do not accept credentials."));
    }
    append("</security></MSM></WLANProfile>");
    xml.0.push(0);
    Ok(xml)
}
