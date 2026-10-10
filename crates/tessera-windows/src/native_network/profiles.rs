// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Exact native profile names stay on the WLAN owner. Only a digest of transient,
//! encrypted-key XML survives observation; no plaintext key is requested.
use super::buffer::Buffer;
use super::callback::Context;
use super::calls::{NativeCalls, invalid, native_error};
use super::observations;
use std::mem::offset_of;
use tessera_system::network::{
    InterfaceId, NetworkError, NetworkErrorKind, NetworkProfileControl, NetworkProfileInitiation,
    NetworkProfileInventory, NetworkProfilePresence, NetworkProfileResult, NetworkProfileScope,
    NetworkTarget, Observation,
};
use windows::Win32::NetworkManagement::WiFi::{
    WLAN_INTERFACE_INFO, WLAN_PROFILE_GROUP_POLICY, WLAN_PROFILE_INFO, WLAN_PROFILE_INFO_LIST,
    WLAN_PROFILE_USER,
};
use windows::Win32::Storage::FileSystem::{
    DELETE, FILE_EXECUTE, FILE_READ_DATA, FILE_WRITE_DATA, STANDARD_RIGHTS_EXECUTE,
    STANDARD_RIGHTS_READ, STANDARD_RIGHTS_WRITE, WRITE_DAC,
};
use windows::core::GUID;

// Published wlanapi.h WLAN_WRITE_ACCESS, including DELETE and WRITE_DAC.
const WRITE_ACCESS: u32 = STANDARD_RIGHTS_READ.0
    | FILE_READ_DATA.0
    | STANDARD_RIGHTS_EXECUTE.0
    | FILE_EXECUTE.0
    | STANDARD_RIGHTS_WRITE.0
    | FILE_WRITE_DATA.0
    | DELETE.0
    | WRITE_DAC.0;

#[derive(Clone, PartialEq)]
struct Descriptor {
    list_flags: u32,
    flags: u32,
    access: u32,
    digest: [u8; 32],
}

#[derive(Clone)]
pub(super) struct Record {
    pub target: NetworkTarget,
    interface: GUID,
    description: [u16; 256],
    name: Vec<u16>,
    descriptor: Descriptor,
    incarnation: u64,
    revision: u64,
    source_revision: u64,
    interface_revision: u64,
}

fn changed() -> NetworkError {
    NetworkError::new(
        NetworkErrorKind::DeviceChanged,
        "The exact saved-profile source changed. Refresh and choose it again.",
    )
}
fn unwatched() -> NetworkError {
    NetworkError::new(
        NetworkErrorKind::Unsupported,
        "Saved-profile changes cannot be safely watched; Forget is read-only.",
    )
}
fn name(entry: &WLAN_PROFILE_INFO) -> Result<Vec<u16>, NetworkError> {
    let end = entry
        .strProfileName
        .iter()
        .position(|unit| *unit == 0)
        .filter(|end| *end > 0)
        .ok_or_else(|| invalid("WLAN saved-profile identity is empty or unterminated."))?;
    Ok(entry.strProfileName[..=end].to_vec())
}
fn list<C: NativeCalls>(
    calls: &C,
    handle: usize,
    id: &GUID,
) -> Result<Vec<WLAN_PROFILE_INFO>, NetworkError> {
    let buffer = Buffer::acquire(calls, calls.profiles(handle, id), "WLAN saved-profile list")?;
    let count = buffer.read::<u32>(0)?;
    let entries = buffer.list::<WLAN_PROFILE_INFO>(
        offset_of!(WLAN_PROFILE_INFO_LIST, ProfileInfo),
        count,
        4096,
    )?;
    let mut names = Vec::with_capacity(entries.len());
    for entry in &entries {
        let native = name(entry)?;
        if names.contains(&native) {
            return Err(invalid("WLAN saved-profile identities are ambiguous."));
        }
        names.push(native);
    }
    Ok(entries)
}

struct Payload(Vec<u8>);
impl Drop for Payload {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&mut self.0);
    }
}
fn descriptor<C: NativeCalls>(
    calls: &C,
    handle: usize,
    id: &GUID,
    entry: &WLAN_PROFILE_INFO,
) -> Result<Descriptor, NetworkError> {
    let reply = calls.profile(handle, id, &name(entry)?);
    let flags = reply.flags;
    let access = reply.granted_access;
    let buffer = Buffer::acquire(calls, reply.allocation, "WLAN saved-profile descriptor")?;
    let mut bytes = Payload(Vec::with_capacity(65536 * 2));
    let digest = (|| {
        for index in 0..65536 {
            let unit = buffer.read::<u16>(index * 2)?;
            if unit == 0 {
                if bytes.0.is_empty() {
                    return Err(invalid("WLAN saved-profile descriptor is empty."));
                }
                return calls.profile_digest(&bytes.0);
            }
            bytes.0.extend_from_slice(&unit.to_le_bytes());
        }
        Err(invalid(
            "WLAN saved-profile descriptor exceeds the bounded size.",
        ))
    })();
    // Erase even a partially read payload on malformed/denied hashing paths.
    buffer.erase(bytes.0.len())?;
    Ok(Descriptor {
        list_flags: entry.dwFlags,
        flags,
        access,
        digest: digest?,
    })
}
fn scope(flags: u32) -> NetworkProfileScope {
    if flags & !(WLAN_PROFILE_GROUP_POLICY | WLAN_PROFILE_USER) != 0 {
        NetworkProfileScope::Unsupported
    } else if flags & WLAN_PROFILE_GROUP_POLICY != 0 {
        NetworkProfileScope::GroupPolicy
    } else if flags & WLAN_PROFILE_USER != 0 {
        NetworkProfileScope::CurrentUser
    } else {
        NetworkProfileScope::AllUsers
    }
}
fn rights(descriptor: &Descriptor) -> Result<(), NetworkError> {
    if descriptor.flags != descriptor.list_flags {
        return Err(changed());
    }
    match scope(descriptor.flags) {
        NetworkProfileScope::GroupPolicy => Err(NetworkError::new(
            NetworkErrorKind::AccessDenied,
            "Group-policy saved profiles are read-only.",
        )),
        NetworkProfileScope::Unsupported => Err(NetworkError::new(
            NetworkErrorKind::Unsupported,
            "This saved-profile policy is unsupported; Forget is read-only.",
        )),
        NetworkProfileScope::AllUsers if descriptor.access & WRITE_ACCESS != WRITE_ACCESS => {
            Err(NetworkError::new(
                NetworkErrorKind::AccessDenied,
                "Windows denied deletion rights for the all-user saved profile.",
            ))
        }
        _ => Ok(()),
    }
}

pub(super) fn inventory<C: NativeCalls>(
    calls: &C,
    handle: usize,
    context: Option<&Context>,
    mut issue: impl FnMut(u64) -> Result<NetworkTarget, NetworkError>,
) -> Result<(NetworkProfileInventory, Vec<Record>), NetworkError> {
    let admitted = context.filter(|context| context.has_authority());
    let epoch = admitted.map(|context| {
        (
            context.incarnation(),
            context.revision(),
            context.source_revision(),
            context.interface_revision(),
        )
    });
    let mut profiles = Vec::new();
    let mut records = Vec::new();
    let mut unavailable = Vec::new();
    for interface in observations::interfaces(calls, handle)? {
        let entries = match list(calls, handle, &interface.InterfaceGuid) {
            Ok(entries) => entries,
            Err(error) => {
                unavailable.push(error);
                continue;
            }
        };
        let end = interface
            .strInterfaceDescription
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(interface.strInterfaceDescription.len());
        let interface_name = String::from_utf16_lossy(&interface.strInterfaceDescription[..end]);
        for entry in entries {
            let native_name = name(&entry)?;
            if profiles.len() >= 4096 {
                unavailable.push(invalid(
                    "WLAN saved-profile inventory exceeds the bounded total.",
                ));
                break;
            }
            let observed = descriptor(calls, handle, &interface.InterfaceGuid, &entry);
            let admission = observed
                .as_ref()
                .map_err(Clone::clone)
                .and_then(rights)
                .and_then(|()| {
                    let (_, revision, _, _) = epoch.ok_or_else(unwatched)?;
                    issue(revision)
                });
            let (target, forget) = match admission {
                Ok(target) => (Some(target), Observation::Ready(())),
                Err(error) => (None, Observation::Unavailable(error)),
            };
            profiles.push(NetworkProfileControl {
                target,
                interface: InterfaceId::new(interface.InterfaceGuid.to_u128().to_be_bytes()),
                interface_name: interface_name.clone(),
                name: String::from_utf16_lossy(&native_name[..native_name.len() - 1]),
                scope: scope(entry.dwFlags),
                forget,
            });
            if let (
                Some(target),
                Ok(descriptor),
                Some((incarnation, revision, source_revision, interface_revision)),
            ) = (target, observed, epoch)
            {
                records.push(Record {
                    target,
                    interface: interface.InterfaceGuid,
                    description: interface.strInterfaceDescription,
                    name: native_name,
                    descriptor,
                    incarnation,
                    revision,
                    source_revision,
                    interface_revision,
                });
            }
        }
    }
    if let Some(context) = admitted
        && (epoch
            != Some((
                context.incarnation(),
                context.revision(),
                context.source_revision(),
                context.interface_revision(),
            ))
            || !context.has_authority())
    {
        records.clear();
        for profile in &mut profiles {
            profile.target = None;
            profile.forget = Observation::Unavailable(changed());
        }
        unavailable.push(changed());
    }
    Ok((
        NetworkProfileInventory {
            profiles,
            unavailable,
        },
        records,
    ))
}

fn interface<C: NativeCalls>(
    calls: &C,
    handle: usize,
    record: &Record,
) -> Result<WLAN_INTERFACE_INFO, NetworkError> {
    let mut matching = observations::interfaces(calls, handle)?
        .into_iter()
        .filter(|entry| entry.InterfaceGuid == record.interface);
    let entry = matching.next().ok_or_else(changed)?;
    if matching.next().is_some() || entry.strInterfaceDescription != record.description {
        return Err(changed());
    }
    Ok(entry)
}
fn observed<C: NativeCalls>(
    calls: &C,
    handle: usize,
    record: &Record,
) -> Result<Option<Descriptor>, NetworkError> {
    interface(calls, handle, record)?;
    let entry = list(calls, handle, &record.interface)?
        .into_iter()
        .find(|entry| name(entry).is_ok_and(|name| name == record.name));
    entry
        .map(|entry| descriptor(calls, handle, &record.interface, &entry))
        .transpose()
}
fn readback<C: NativeCalls>(
    calls: &C,
    handle: usize,
    context: Option<&Context>,
    record: &Record,
) -> Result<(NetworkProfilePresence, Option<NetworkError>), NetworkError> {
    let context = context.ok_or_else(unwatched)?;
    let valid = || {
        context.is_admitted()
            && context.incarnation() == record.incarnation
            && context.interface_revision() == record.interface_revision
            && record.interface_revision != u64::MAX
    };
    if !valid() {
        return Err(changed());
    }
    interface(calls, handle, record)?;
    let entry = list(calls, handle, &record.interface)?
        .into_iter()
        .find(|entry| name(entry).is_ok_and(|name| name == record.name));
    let result = match entry {
        None => (NetworkProfilePresence::Absent, None),
        Some(entry) => match descriptor(calls, handle, &record.interface, &entry) {
            Ok(fresh) if fresh == record.descriptor => (NetworkProfilePresence::Present, None),
            Ok(_) => (NetworkProfilePresence::Replaced, None),
            Err(error) => (NetworkProfilePresence::Present, Some(error)),
        },
    };
    if !valid() {
        return Err(changed());
    }
    Ok(result)
}

pub(super) fn execute<C: NativeCalls>(
    calls: &C,
    handle: usize,
    context: Option<&Context>,
    record: Record,
) -> NetworkProfileResult {
    let validate = || -> Result<(), NetworkError> {
        let context = context
            .filter(|context| context.has_authority())
            .ok_or_else(unwatched)?;
        let valid = || {
            context.incarnation() == record.incarnation
                && context.revision() == record.revision
                && context.source_revision() == record.source_revision
                && context.interface_revision() == record.interface_revision
                && context.has_authority()
        };
        if !valid() {
            return Err(changed());
        }
        let fresh = observed(calls, handle, &record)?.ok_or_else(changed)?;
        if fresh != record.descriptor {
            return Err(changed());
        }
        rights(&fresh)?;
        // Last check is immediately adjacent to WlanDeleteProfile, after full
        // interface/name/descriptor/policy/rights readback. Never choose a fallback.
        if !valid() {
            return Err(changed());
        }
        Ok(())
    };
    let initiation = match validate() {
        Err(error) => NetworkProfileInitiation::NotSubmitted(error),
        Ok(()) => {
            let status = calls.delete_profile(handle, &record.interface, &record.name);
            if status == 0 {
                NetworkProfileInitiation::Accepted
            } else {
                NetworkProfileInitiation::NativeError {
                    code: status,
                    error: native_error("WLAN saved-profile deletion initiation", status),
                }
            }
        }
    };
    // A successful SDK return is not confirmed removal. Own profile-change
    // notification retires stale authority but does not erase this captured flight.
    let (readback, descriptor_error) = match readback(calls, handle, context, &record) {
        Ok((presence, error)) => (Observation::Ready(presence), error),
        Err(error) => (Observation::Unavailable(error), None),
    };
    NetworkProfileResult {
        target: record.target,
        interface: InterfaceId::new(record.interface.to_u128().to_be_bytes()),
        initiation,
        readback,
        descriptor_error,
    }
}
