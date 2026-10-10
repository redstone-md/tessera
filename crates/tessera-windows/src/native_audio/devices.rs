// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Core Audio inventory inside the existing MTA owner. Native references and
//! registrations never escape this module; portable keys are incarnation leases.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tessera_system::audio::{
    AudioDefaultRoles, AudioDefaultTarget, AudioDevice, AudioDeviceCommand, AudioDeviceFlow,
    AudioDeviceKey, AudioDevicesResult, AudioDevicesSnapshot, AudioError, AudioErrorKind,
    AudioFlow, AudioLevel, AudioRole, AudioRoleResult, AudioSession, AudioSessionKey, EndpointId,
    Volume,
};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    AudioSessionDisconnectReason, AudioSessionState, AudioSessionStateActive,
    AudioSessionStateExpired, DEVICE_STATE_ACTIVE, ERole, IAudioSessionControl,
    IAudioSessionControl2, IAudioSessionEvents, IAudioSessionEvents_Impl, IAudioSessionManager2,
    IAudioSessionNotification, IAudioSessionNotification_Impl, IMMDevice, IMMDeviceEnumerator,
    ISimpleAudioVolume, eCommunications, eConsole, eMultimedia,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PropVariantClear, PropVariantToStringAlloc,
};
use windows::Win32::System::Com::{CLSCTX_ALL, STGM_READ};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::core::{BOOL, GUID, IUnknown, Interface, PCWSTR, Ref, implement, w};

use super::{
    NativeEndpoint, TaskString, VolumeWatch, device_id, native_error, native_flow, not_found,
};
use crate::audio::actor::{Signal, SignalSender};

mod policy;
use policy::DefaultPolicy;

pub(super) struct DeviceInventory {
    records: HashMap<EndpointId, DeviceRecord>,
    policy: Result<DefaultPolicy, AudioError>,
    pub(super) epoch: Arc<AtomicU64>,
    pub(super) retirements: EndpointRetirements,
    observed_epoch: u64,
}

struct DeviceRecord {
    key: AudioDeviceKey,
    flow: AudioFlow,
    device: IMMDevice,
    retired: Arc<AtomicBool>,
    master: Result<NativeEndpoint, AudioError>,
    volume_watch: Result<Option<VolumeWatch>, AudioError>,
    manager: Result<SessionManager, AudioError>,
    sessions: HashMap<String, SessionRecord>,
}

struct SessionManager {
    manager: IAudioSessionManager2,
    callback: IAudioSessionNotification,
}

struct SessionRecord {
    key: AudioSessionKey,
    control: IAudioSessionControl2,
    volume: ISimpleAudioVolume,
    callback: IAudioSessionEvents,
    retired: Arc<AtomicBool>,
}

/// Portable retirement flags shared with the one existing device callback.
/// Unrelated endpoint churn never gives a surviving device a new incarnation.
#[derive(Clone, Default)]
pub(super) struct EndpointRetirements(Arc<Mutex<HashMap<EndpointId, Arc<AtomicBool>>>>);

impl EndpointRetirements {
    pub(super) fn retire_id(&self, id: &PCWSTR) {
        let id = notification_id(id);
        let records = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        for (key, retired) in records.iter() {
            if id.as_ref().is_none_or(|id| id == key) {
                retired.store(true, Ordering::Release);
            }
        }
    }
}

fn notification_id(id: &PCWSTR) -> Option<EndpointId> {
    if id.0.is_null() {
        return None;
    }
    for length in 0..=32_768 {
        // MMDevice callback owns a readable NUL-terminated identity for this call.
        if unsafe { *id.0.add(length) } == 0 {
            let units = unsafe { std::slice::from_raw_parts(id.0, length) };
            return String::from_utf16(units)
                .ok()
                .and_then(|id| EndpointId::new(id).ok());
        }
    }
    None
}

impl DeviceInventory {
    pub(super) fn new() -> Self {
        Self {
            records: HashMap::new(),
            policy: DefaultPolicy::new(),
            epoch: Arc::new(AtomicU64::new(1)),
            retirements: EndpointRetirements::default(),
            observed_epoch: 1,
        }
    }

    pub(super) fn retire(&mut self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        let mut retired = self
            .retirements
            .0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for flag in retired.values() {
            flag.store(true, Ordering::Release);
        }
        retired.clear();
        drop(retired);
        self.records.clear();
    }

    pub(super) fn default_watch(&mut self, id: &EndpointId) {
        if let Some(record) = self.records.get_mut(id) {
            record.volume_watch = Ok(None);
        }
    }

    fn incarnation() -> Result<u64, AudioError> {
        // A key from a retired/replaced AudioHost must not collide with a new
        // owner's record, even when Windows reuses the exact endpoint ID.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        })
        .map(|previous| previous + 1)
        .map_err(|_| AudioError::new(AudioErrorKind::Stopped, "Audio incarnation space exhausted"))
    }

    pub(super) fn read(
        &mut self,
        enumerator: &IMMDeviceEnumerator,
        signals: &SignalSender,
        watched_endpoints: &[EndpointId],
    ) -> Result<AudioDevicesResult, AudioError> {
        let epoch = self.epoch.load(Ordering::Acquire);
        if epoch != self.observed_epoch {
            self.records
                .retain(|_, record| !record.retired.load(Ordering::Acquire));
            self.observed_epoch = epoch;
        }
        let output = self.read_flow(enumerator, signals, AudioFlow::Output, watched_endpoints);
        let input = self.read_flow(enumerator, signals, AudioFlow::Input, watched_endpoints);
        if self.epoch.load(Ordering::Acquire) != epoch {
            // A racing observation is stale, not retirement of surviving devices.
            return Err(changed());
        }
        Ok(AudioDevicesResult {
            snapshot: AudioDevicesSnapshot {
                output,
                input,
                default_role_control: self.policy.as_ref().map(|_| ()).map_err(Clone::clone),
                device_settings: Ok(()),
            },
            roles: Vec::new(),
        })
    }

    fn read_flow(
        &mut self,
        enumerator: &IMMDeviceEnumerator,
        signals: &SignalSender,
        flow: AudioFlow,
        watched_endpoints: &[EndpointId],
    ) -> AudioDeviceFlow {
        let devices = self.inventory_flow(enumerator, signals, flow, watched_endpoints);
        AudioDeviceFlow {
            devices,
            roles: roles(enumerator, flow),
        }
    }

    fn inventory_flow(
        &mut self,
        enumerator: &IMMDeviceEnumerator,
        signals: &SignalSender,
        flow: AudioFlow,
        watched_endpoints: &[EndpointId],
    ) -> Result<Vec<AudioDevice>, AudioError> {
        let native = active_devices(enumerator, flow)?;
        let mut seen = HashSet::new();
        let mut result = Vec::with_capacity(native.len());
        for device in native {
            let id = device_id(&device)?;
            seen.insert(id.clone());
            if !self.records.contains_key(&id) {
                let key = AudioDeviceKey {
                    id: id.clone(),
                    incarnation: Self::incarnation()?,
                };
                let record = DeviceRecord::new(key, flow, device.clone(), signals);
                self.records.insert(id.clone(), record);
                self.retirements
                    .0
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(id.clone(), self.records[&id].retired.clone());
            }
            // Reconcile sessions with this record's native references still on the owner.
            let mut record = self.records.remove(&id).ok_or_else(changed)?;
            record.bind_volume_watch(watched_endpoints.contains(&id), signals);
            let level = record
                .master
                .as_ref()
                .map_err(Clone::clone)
                .and_then(read_master);
            let sessions = if flow == AudioFlow::Output {
                self.read_sessions(&mut record, signals)
            } else {
                Ok(Vec::new())
            };
            result.push(AudioDevice {
                key: record.key.clone(),
                flow,
                name: friendly_name(&device).unwrap_or_else(|_| "Device name unavailable".into()),
                level,
                watch: record
                    .volume_watch
                    .as_ref()
                    .map(|_| ())
                    .map_err(Clone::clone),
                sessions,
            });
            self.records.insert(id, record);
        }
        self.records
            .retain(|id, record| record.flow != flow || seen.contains(id));
        self.retirements
            .0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .retain(|id, _| self.records.contains_key(id));
        Ok(result)
    }

    fn read_sessions(
        &mut self,
        record: &mut DeviceRecord,
        signals: &SignalSender,
    ) -> Result<Vec<AudioSession>, AudioError> {
        let manager = &record.manager.as_ref().map_err(Clone::clone)?.manager;
        let native = sessions(manager)?;
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for control in native {
            let instance = instance_id(&control)?;
            seen.insert(instance.clone());
            let replace = record.sessions.get(&instance).is_none_or(|existing| {
                existing.retired.load(Ordering::Acquire)
                    || !same_session(&existing.control, &control)
            });
            if replace {
                record.sessions.remove(&instance);
                let key = AudioSessionKey {
                    endpoint: record.key.clone(),
                    instance_id: instance.clone(),
                    incarnation: Self::incarnation()?,
                };
                record.sessions.insert(
                    instance.clone(),
                    SessionRecord::new(key, control.clone(), signals)?,
                );
            }
            let session = record.sessions.get(&instance).ok_or_else(changed)?;
            let state = unsafe { control.GetState() }
                .map_err(|error| native_error("Read audio session state", error))?;
            // S_OK alone identifies System Sounds; S_FALSE is not a failure.
            let sound_status = unsafe { control.IsSystemSoundsSession() };
            sound_status
                .ok()
                .map_err(|error| native_error("Identify system-sounds session", error))?;
            let system_sounds = sound_status.0 == 0;
            let name = if system_sounds {
                "System sounds".into()
            } else {
                display_name(&control)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| {
                        unsafe { control.GetProcessId() }
                            .map(|pid| format!("Application ({pid})"))
                            .unwrap_or_else(|_| "Audio session".into())
                    })
            };
            result.push(AudioSession {
                key: session.key.clone(),
                name,
                system_sounds,
                active: state == AudioSessionStateActive,
                level: read_session(&session.volume),
            });
        }
        record
            .sessions
            .retain(|id, session| seen.contains(id) && !session.retired.load(Ordering::Acquire));
        Ok(result)
    }

    fn validate_device(
        &self,
        enumerator: &IMMDeviceEnumerator,
        key: &AudioDeviceKey,
    ) -> Result<&DeviceRecord, AudioError> {
        if self.observed_epoch != self.epoch.load(Ordering::Acquire) {
            return Err(changed());
        }
        let record = self
            .records
            .get(&key.id)
            .filter(|record| &record.key == key && !record.retired.load(Ordering::Acquire))
            .ok_or_else(changed)?;
        // Enumerate fresh active membership rather than opening a caller ID.
        let devices = active_devices(enumerator, record.flow)?;
        if !devices
            .iter()
            .any(|device| device_id(device).as_ref() == Ok(&key.id))
        {
            return Err(changed());
        }
        let active = unsafe { record.device.GetState() }
            .map_err(|error| native_error("Validate audio endpoint state", error))?;
        if active != DEVICE_STATE_ACTIVE
            || record.retired.load(Ordering::Acquire)
            || self.observed_epoch != self.epoch.load(Ordering::Acquire)
        {
            return Err(changed());
        }
        Ok(record)
    }

    fn validate_session(
        &self,
        enumerator: &IMMDeviceEnumerator,
        key: &AudioSessionKey,
    ) -> Result<&SessionRecord, AudioError> {
        let record = self.validate_device(enumerator, &key.endpoint)?;
        if record.flow != AudioFlow::Output {
            return Err(changed());
        }
        let session = record
            .sessions
            .get(&key.instance_id)
            .filter(|session| &session.key == key && !session.retired.load(Ordering::Acquire))
            .ok_or_else(changed)?;
        let fresh = sessions(&record.manager.as_ref().map_err(Clone::clone)?.manager)?;
        if !fresh.iter().any(|control| {
            instance_id(control).as_ref() == Ok(&key.instance_id)
                && same_session(&session.control, control)
        }) || session.retired.load(Ordering::Acquire)
        {
            return Err(changed());
        }
        self.validate_device(enumerator, &key.endpoint)?;
        if unsafe { session.control.GetState() }
            .map_err(|error| native_error("Validate current output session state", error))?
            == AudioSessionStateExpired
            || session.retired.load(Ordering::Acquire)
        {
            return Err(changed());
        }
        Ok(session)
    }

    pub(super) fn execute(
        &mut self,
        enumerator: &IMMDeviceEnumerator,
        signals: &SignalSender,
        command: AudioDeviceCommand,
        watched_endpoints: &[EndpointId],
    ) -> Result<AudioDevicesResult, AudioError> {
        let key = command.endpoint().clone();
        let mut role_results = Vec::new();
        match &command {
            AudioDeviceCommand::SetVolume { volume, .. } => {
                let record = self.validate_device(enumerator, &key)?;
                let endpoint = record.master.as_ref().map_err(Clone::clone)?;
                unsafe {
                    endpoint
                        .volume
                        .SetMasterVolumeLevelScalar(volume.scalar(), std::ptr::null())
                }
                .map_err(|error| native_error("Set selected endpoint volume", error))?;
                read_master(endpoint)?;
            }
            AudioDeviceCommand::SetMuted { muted, .. } => {
                let record = self.validate_device(enumerator, &key)?;
                let endpoint = record.master.as_ref().map_err(Clone::clone)?;
                unsafe { endpoint.volume.SetMute(*muted, std::ptr::null()) }
                    .map_err(|error| native_error("Set selected endpoint mute", error))?;
                read_master(endpoint)?;
            }
            AudioDeviceCommand::SetSessionVolume { session, volume } => {
                let native = self.validate_session(enumerator, session)?;
                unsafe {
                    native
                        .volume
                        .SetMasterVolume(volume.scalar(), std::ptr::null())
                }
                .map_err(|error| native_error("Set audio session volume", error))?;
                read_session(&native.volume)?;
            }
            AudioDeviceCommand::SetSessionMuted { session, muted } => {
                let native = self.validate_session(enumerator, session)?;
                unsafe { native.volume.SetMute(*muted, std::ptr::null()) }
                    .map_err(|error| native_error("Set audio session mute", error))?;
                read_session(&native.volume)?;
            }
            AudioDeviceCommand::SetDefault { target, .. } => {
                let roles: &[AudioRole] = match target {
                    AudioDefaultTarget::MultimediaAndConsole => {
                        &[AudioRole::Multimedia, AudioRole::Console]
                    }
                    AudioDefaultTarget::Communications => &[AudioRole::Communications],
                };
                for role in roles {
                    // Each role validates and reads back separately. A previous
                    // success remains visible when the following role fails.
                    let result = self.set_role(enumerator, &key, *role);
                    role_results.push(AudioRoleResult {
                        role: *role,
                        result,
                    });
                }
            }
            AudioDeviceCommand::OpenDeviceSettings { .. } => {
                self.validate_device(enumerator, &key)?;
                open_settings()?;
            }
        }
        if !matches!(command, AudioDeviceCommand::SetDefault { .. }) {
            self.validate_device(enumerator, &key)?;
        }
        if let AudioDeviceCommand::SetSessionVolume { session, .. }
        | AudioDeviceCommand::SetSessionMuted { session, .. } = &command
        {
            self.validate_session(enumerator, session)?;
        }
        let mut result = match self.read(enumerator, signals, watched_endpoints) {
            Ok(result) => result,
            Err(error) if !role_results.is_empty() => AudioDevicesResult {
                snapshot: AudioDevicesSnapshot {
                    output: AudioDeviceFlow {
                        devices: Err(error.clone()),
                        roles: roles(enumerator, AudioFlow::Output),
                    },
                    input: AudioDeviceFlow {
                        devices: Err(error),
                        roles: roles(enumerator, AudioFlow::Input),
                    },
                    default_role_control: self.policy.as_ref().map(|_| ()).map_err(Clone::clone),
                    device_settings: Ok(()),
                },
                roles: Vec::new(),
            },
            Err(error) => return Err(error),
        };
        if role_results.is_empty() {
            // Return only actual readback for this exact incarnation. A partial
            // inventory or vanished session must not masquerade as confirmation.
            let device = [
                &result.snapshot.output.devices,
                &result.snapshot.input.devices,
            ]
            .into_iter()
            .filter_map(|devices| devices.as_ref().ok())
            .flat_map(|devices| devices.iter())
            .find(|device| device.key == key)
            .ok_or_else(changed)?;
            match &command {
                AudioDeviceCommand::SetVolume { .. } | AudioDeviceCommand::SetMuted { .. } => {
                    device.level.as_ref().map_err(Clone::clone)?;
                }
                AudioDeviceCommand::SetSessionVolume { session, .. }
                | AudioDeviceCommand::SetSessionMuted { session, .. } => {
                    let current = device
                        .sessions
                        .as_ref()
                        .map_err(Clone::clone)?
                        .iter()
                        .find(|current| &current.key == session)
                        .ok_or_else(changed)?;
                    current.level.as_ref().map_err(Clone::clone)?;
                }
                _ => {}
            }
        }
        result.roles = role_results;
        Ok(result)
    }

    fn set_role(
        &self,
        enumerator: &IMMDeviceEnumerator,
        key: &AudioDeviceKey,
        role: AudioRole,
    ) -> Result<(), AudioError> {
        let record = self.validate_device(enumerator, key)?;
        self.policy
            .as_ref()
            .map_err(Clone::clone)?
            .set(&key.id, native_role(role))?;
        self.validate_device(enumerator, key)?;
        if role_id(enumerator, record.flow, native_role(role))?.as_ref() != Some(&key.id) {
            return Err(AudioError::new(
                AudioErrorKind::DeviceChanged,
                "Default audio role readback changed",
            ));
        }
        self.validate_device(enumerator, key)?;
        Ok(())
    }
}

impl DeviceRecord {
    fn new(
        key: AudioDeviceKey,
        flow: AudioFlow,
        device: IMMDevice,
        signals: &SignalSender,
    ) -> Self {
        let master = unsafe { device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) }
            .map(|volume| NativeEndpoint {
                id: key.id.clone(),
                volume,
            })
            .map_err(|error| native_error("Activate selected endpoint volume", error));
        // Default routes already have the owner's existing volume watch.
        // Inventory binds only the other active endpoints during projection.
        let volume_watch = Ok(None);
        let manager = if flow == AudioFlow::Output {
            SessionManager::new(&device, signals)
        } else {
            Err(AudioError::new(
                AudioErrorKind::Unsupported,
                "Input endpoints do not have an output mixer",
            ))
        };
        Self {
            key,
            flow,
            device,
            master,
            volume_watch,
            manager,
            retired: Arc::new(AtomicBool::new(false)),
            sessions: HashMap::new(),
        }
    }

    fn bind_volume_watch(&mut self, already_watched: bool, signals: &SignalSender) {
        if self.master.is_err() {
            self.master = unsafe {
                self.device
                    .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            }
            .map(|volume| NativeEndpoint {
                id: self.key.id.clone(),
                volume,
            })
            .map_err(|error| native_error("Activate selected endpoint volume", error));
        }
        if self.flow == AudioFlow::Output && self.manager.is_err() {
            self.manager = SessionManager::new(&self.device, signals);
        }
        if already_watched {
            self.volume_watch = Ok(None);
        } else if !matches!(self.volume_watch, Ok(Some(_))) {
            self.volume_watch = self
                .master
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|endpoint| {
                    VolumeWatch::new(
                        NativeEndpoint {
                            id: endpoint.id.clone(),
                            volume: endpoint.volume.clone(),
                        },
                        signals.clone(),
                    )
                    .map(Some)
                });
        }
    }
}

impl SessionManager {
    fn new(device: &IMMDevice, signals: &SignalSender) -> Result<Self, AudioError> {
        let manager = unsafe { device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) }
            .map_err(|error| native_error("Activate output session manager", error))?;
        let callback: IAudioSessionNotification = SessionCreated {
            signals: signals.clone(),
        }
        .into();
        unsafe { manager.RegisterSessionNotification(&callback) }
            .map_err(|error| native_error("Register output session creation", error))?;
        // GetCount arms Core Audio session creation delivery on this owner.
        let owner = Self { manager, callback };
        let enumerator = unsafe { owner.manager.GetSessionEnumerator() }
            .map_err(|error| native_error("Enumerate initial output sessions", error))?;
        unsafe { enumerator.GetCount() }
            .map_err(|error| native_error("Arm output session notifications", error))?;
        Ok(owner)
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        let _ = unsafe { self.manager.UnregisterSessionNotification(&self.callback) };
    }
}

impl SessionRecord {
    fn new(
        key: AudioSessionKey,
        control: IAudioSessionControl2,
        signals: &SignalSender,
    ) -> Result<Self, AudioError> {
        let volume = control
            .cast::<ISimpleAudioVolume>()
            .map_err(|error| native_error("Activate output session volume", error))?;
        let retired = Arc::new(AtomicBool::new(false));
        let callback: IAudioSessionEvents = SessionEvents {
            signals: signals.clone(),
            retired: retired.clone(),
        }
        .into();
        unsafe { control.RegisterAudioSessionNotification(&callback) }
            .map_err(|error| native_error("Register output session changes", error))?;
        Ok(Self {
            key,
            control,
            volume,
            callback,
            retired,
        })
    }
}

impl Drop for SessionRecord {
    fn drop(&mut self) {
        self.retired.store(true, Ordering::Release);
        let _ = unsafe {
            self.control
                .UnregisterAudioSessionNotification(&self.callback)
        };
    }
}

fn active_devices(
    enumerator: &IMMDeviceEnumerator,
    flow: AudioFlow,
) -> Result<Vec<IMMDevice>, AudioError> {
    let collection =
        unsafe { enumerator.EnumAudioEndpoints(native_flow(flow), DEVICE_STATE_ACTIVE) }
            .map_err(|error| native_error("Enumerate active audio endpoints", error))?;
    let count = unsafe { collection.GetCount() }
        .map_err(|error| native_error("Count active audio endpoints", error))?;
    if count > 1_024 {
        return Err(oversized());
    }
    (0..count)
        .map(|index| {
            unsafe { collection.Item(index) }
                .map_err(|error| native_error("Read active audio endpoint", error))
        })
        .collect()
}

fn sessions(manager: &IAudioSessionManager2) -> Result<Vec<IAudioSessionControl2>, AudioError> {
    let enumerator = unsafe { manager.GetSessionEnumerator() }
        .map_err(|error| native_error("Enumerate output audio sessions", error))?;
    let count = unsafe { enumerator.GetCount() }
        .map_err(|error| native_error("Count output audio sessions", error))?;
    if !(0..=4_096).contains(&count) {
        return Err(oversized());
    }
    let mut result = Vec::new();
    for index in 0..count {
        let control = unsafe { enumerator.GetSession(index) }
            .and_then(|control| control.cast::<IAudioSessionControl2>())
            .map_err(|error| native_error("Read output audio session", error))?;
        let state = unsafe { control.GetState() }
            .map_err(|error| native_error("Validate output audio session", error))?;
        if state != AudioSessionStateExpired {
            result.push(control);
        }
    }
    Ok(result)
}

fn same_session(left: &IAudioSessionControl2, right: &IAudioSessionControl2) -> bool {
    match (left.cast::<IUnknown>(), right.cast::<IUnknown>()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn read_master(endpoint: &NativeEndpoint) -> Result<AudioLevel, AudioError> {
    let scalar = unsafe { endpoint.volume.GetMasterVolumeLevelScalar() }
        .map_err(|error| native_error("Read selected endpoint volume", error))?;
    let muted = unsafe { endpoint.volume.GetMute() }
        .map_err(|error| native_error("Read selected endpoint mute", error))?
        .as_bool();
    Ok(AudioLevel {
        volume: Volume::from_scalar(scalar)?,
        muted,
    })
}

fn read_session(volume: &ISimpleAudioVolume) -> Result<AudioLevel, AudioError> {
    let scalar = unsafe { volume.GetMasterVolume() }
        .map_err(|error| native_error("Read output session volume", error))?;
    let muted = unsafe { volume.GetMute() }
        .map_err(|error| native_error("Read output session mute", error))?
        .as_bool();
    Ok(AudioLevel {
        volume: Volume::from_scalar(scalar)?,
        muted,
    })
}

fn task_text(allocation: TaskString) -> Result<String, AudioError> {
    if allocation.0.0.is_null() {
        return Err(oversized());
    }
    for length in 0..=32_768 {
        // Core Audio/propsys guarantee a readable NUL-terminated task string.
        if unsafe { *allocation.0.0.add(length) } == 0 {
            let units = unsafe { std::slice::from_raw_parts(allocation.0.0, length) };
            return String::from_utf16(units).map_err(|_| oversized());
        }
    }
    Err(oversized())
}

fn instance_id(control: &IAudioSessionControl2) -> Result<String, AudioError> {
    let allocation = unsafe { control.GetSessionInstanceIdentifier() }
        .map_err(|error| native_error("Read output session incarnation", error))?;
    let value = task_text(TaskString(allocation))?;
    if value.is_empty() {
        return Err(oversized());
    }
    Ok(value)
}

fn display_name(control: &IAudioSessionControl2) -> Option<String> {
    unsafe { control.GetDisplayName() }
        .ok()
        .and_then(|value| task_text(TaskString(value)).ok())
}

struct PropertyValue(PROPVARIANT);

impl Drop for PropertyValue {
    fn drop(&mut self) {
        // Exactly one clear for every successful GetValue, including conversion failure.
        let _ = unsafe { PropVariantClear(&mut self.0) };
    }
}

fn friendly_name(device: &IMMDevice) -> Result<String, AudioError> {
    const FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 14,
    };
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }
        .map_err(|error| native_error("Open audio endpoint properties", error))?;
    let value = PropertyValue(
        unsafe { store.GetValue(&FRIENDLY_NAME) }
            .map_err(|error| native_error("Read audio endpoint name", error))?,
    );
    let converted = unsafe { PropVariantToStringAlloc(&value.0) };
    let allocation =
        converted.map_err(|error| native_error("Decode audio endpoint name", error))?;
    task_text(TaskString(allocation))
}

fn roles(enumerator: &IMMDeviceEnumerator, flow: AudioFlow) -> AudioDefaultRoles {
    AudioDefaultRoles {
        console: role_id(enumerator, flow, eConsole),
        multimedia: role_id(enumerator, flow, eMultimedia),
        communications: role_id(enumerator, flow, eCommunications),
    }
}

fn role_id(
    enumerator: &IMMDeviceEnumerator,
    flow: AudioFlow,
    role: ERole,
) -> Result<Option<EndpointId>, AudioError> {
    match unsafe { enumerator.GetDefaultAudioEndpoint(native_flow(flow), role) } {
        Ok(device) => device_id(&device).map(Some),
        Err(error) if error.code() == not_found() => Ok(None),
        Err(error) => Err(native_error("Read default audio role", error)),
    }
}

fn native_role(role: AudioRole) -> ERole {
    match role {
        AudioRole::Console => eConsole,
        AudioRole::Multimedia => eMultimedia,
        AudioRole::Communications => eCommunications,
    }
}

fn open_settings() -> Result<(), AudioError> {
    // Same fixed-target ShellExecute pattern as the existing native Network host.
    // No caller string, parameters, working directory, elevation or process handle.
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("open"),
        lpFile: w!("ms-settings:sound"),
        nShow: 1,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }
        .map_err(|error| native_error("Sound Settings dispatch", error))
}

fn changed() -> AudioError {
    AudioError::new(
        AudioErrorKind::DeviceChanged,
        "Audio device or session incarnation changed; refresh",
    )
}

fn oversized() -> AudioError {
    AudioError::new(
        AudioErrorKind::InvalidValue,
        "Invalid or oversized native audio inventory",
    )
}

#[implement(IAudioSessionNotification)]
struct SessionCreated {
    signals: SignalSender,
}

impl IAudioSessionNotification_Impl for SessionCreated_Impl {
    fn OnSessionCreated(&self, _session: Ref<IAudioSessionControl>) -> windows::core::Result<()> {
        self.signals.notify(Signal::Volume);
        Ok(())
    }
}

#[implement(IAudioSessionEvents)]
struct SessionEvents {
    signals: SignalSender,
    retired: Arc<AtomicBool>,
}

impl IAudioSessionEvents_Impl for SessionEvents_Impl {
    fn OnDisplayNameChanged(
        &self,
        _name: &PCWSTR,
        _context: *const GUID,
    ) -> windows::core::Result<()> {
        self.signals.notify(Signal::Volume);
        Ok(())
    }
    fn OnIconPathChanged(
        &self,
        _path: &PCWSTR,
        _context: *const GUID,
    ) -> windows::core::Result<()> {
        self.signals.notify(Signal::Volume);
        Ok(())
    }
    fn OnSimpleVolumeChanged(
        &self,
        _volume: f32,
        _muted: BOOL,
        _context: *const GUID,
    ) -> windows::core::Result<()> {
        self.signals.notify(Signal::Volume);
        Ok(())
    }
    fn OnChannelVolumeChanged(
        &self,
        _count: u32,
        _volumes: *const f32,
        _channel: u32,
        _context: *const GUID,
    ) -> windows::core::Result<()> {
        self.signals.notify(Signal::Volume);
        Ok(())
    }
    fn OnGroupingParamChanged(
        &self,
        _group: *const GUID,
        _context: *const GUID,
    ) -> windows::core::Result<()> {
        self.signals.notify(Signal::Volume);
        Ok(())
    }
    fn OnStateChanged(&self, state: AudioSessionState) -> windows::core::Result<()> {
        if state == AudioSessionStateExpired {
            self.retired.store(true, Ordering::Release);
        }
        self.signals.notify(Signal::Volume);
        Ok(())
    }
    fn OnSessionDisconnected(
        &self,
        _reason: AudioSessionDisconnectReason,
    ) -> windows::core::Result<()> {
        self.retired.store(true, Ordering::Release);
        self.signals.notify(Signal::Volume);
        Ok(())
    }
}
