// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Core Audio behind the portable actor. All owned COM references remain on its
//! MTA worker; callback implementations only own a portable signal sender.
//! Signatures/features are from the installed windows 0.62.2 generated bindings.

use tessera_system::audio::{
    AudioEndpoint, AudioError, AudioErrorKind, AudioFlow, EndpointId, Volume,
};
use windows::Win32::Foundation::{E_ACCESSDENIED, E_INVALIDARG, ERROR_NOT_FOUND, PROPERTYKEY};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDCLNT_E_DEVICE_INVALIDATED, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, EDataFlow, ERole,
    IMMDevice, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
    MMDeviceEnumerator, eCapture, eMultimedia, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::core::{Error, HRESULT, PCWSTR, PWSTR, implement};

use crate::audio::actor::{Driver, Signal, SignalSender};

/// Fields release in declaration order, so the apartment is always last.
pub(crate) struct NativeAudio {
    volumes: [Option<VolumeWatch>; 2],
    device_callback: Option<IMMNotificationClient>,
    enumerator: IMMDeviceEnumerator,
    signals: Option<SignalSender>,
    _apartment: Apartment,
}

impl NativeAudio {
    pub(crate) fn new() -> Result<Self, AudioError> {
        let apartment = Apartment::new()?;
        // SAFETY: this fresh worker has a successfully initialized MTA; the
        // counted interface stays here and is released before the apartment.
        let enumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|error| native_error("Create audio device enumerator", error))?;
        Ok(Self {
            volumes: [None, None],
            device_callback: None,
            enumerator,
            signals: None,
            _apartment: apartment,
        })
    }

    fn device(&self, flow: AudioFlow) -> Result<Option<IMMDevice>, AudioError> {
        // SAFETY: live enumerator, worker apartment, typed flow and role. Never
        // use GetDevice with a caller-supplied identity.
        match unsafe {
            self.enumerator
                .GetDefaultAudioEndpoint(native_flow(flow), eMultimedia)
        } {
            Ok(device) => Ok(Some(device)),
            Err(error) if error.code() == not_found() => Ok(None),
            Err(error) => Err(native_error("Get default multimedia audio endpoint", error)),
        }
    }

    fn register_devices(&mut self) -> Result<(), AudioError> {
        if self.device_callback.is_some() {
            return Ok(());
        }
        let signals = self.signals.as_ref().ok_or_else(watch_stopped)?.clone();
        let callback: IMMNotificationClient = DeviceNotification { signals }.into();
        // SAFETY: the owner retains callback until after unregistration. Unlike
        // volume registration, this registration does NOT AddRef the callback.
        unsafe {
            self.enumerator
                .RegisterEndpointNotificationCallback(&callback)
        }
        .map_err(|error| native_error("Register audio device notifications", error))?;
        self.device_callback = Some(callback);
        Ok(())
    }

    fn rebind_flow(&mut self, flow: AudioFlow) -> Result<(), AudioError> {
        let index = match flow {
            AudioFlow::Output => 0,
            AudioFlow::Input => 1,
        };
        let endpoint = match self.default_endpoint(flow) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                // Unknown default identity must not keep an unrelated old
                // endpoint subscribed as if it were still the default.
                self.volumes[index] = None;
                return Err(error);
            }
        };
        match endpoint {
            Some(endpoint) => {
                if self.volumes[index]
                    .as_ref()
                    .is_some_and(|watch| watch.endpoint.id == endpoint.id)
                {
                    return Ok(());
                }
                self.volumes[index] = None;
                let signals = self.signals.as_ref().ok_or_else(watch_stopped)?.clone();
                self.volumes[index] = Some(VolumeWatch::new(endpoint, signals)?);
            }
            None => self.volumes[index] = None,
        }
        Ok(())
    }
}

impl Driver for NativeAudio {
    type Endpoint = NativeEndpoint;

    fn default_endpoint(&mut self, flow: AudioFlow) -> Result<Option<Self::Endpoint>, AudioError> {
        let Some(device) = self.device(flow)? else {
            return Ok(None);
        };
        let id = device_id(&device)?;
        // SAFETY: live worker-owned device; no activation parameters are needed
        // for endpoint-volume control. The returned reference stays on worker.
        let volume = unsafe { device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) }
            .map_err(|error| native_error("Activate endpoint volume", error))?;
        Ok(Some(NativeEndpoint { id, volume }))
    }

    fn endpoint_id(endpoint: &Self::Endpoint) -> &EndpointId {
        &endpoint.id
    }

    fn read_endpoint(&mut self, endpoint: &Self::Endpoint) -> Result<AudioEndpoint, AudioError> {
        // SAFETY: live endpoint-volume interface, accessed only on its owner.
        let scalar = unsafe { endpoint.volume.GetMasterVolumeLevelScalar() }
            .map_err(|error| native_error("Read endpoint master volume", error))?;
        let volume = Volume::from_scalar(scalar)?;
        // SAFETY: same owned interface. BOOL uses nonzero truth semantics.
        let muted = unsafe { endpoint.volume.GetMute() }
            .map_err(|error| native_error("Read endpoint mute", error))?
            .as_bool();
        Ok(AudioEndpoint {
            id: endpoint.id.clone(),
            volume,
            muted,
        })
    }

    fn current_id(&mut self, flow: AudioFlow) -> Result<Option<EndpointId>, AudioError> {
        self.device(flow)?.as_ref().map(device_id).transpose()
    }

    fn set_volume(&mut self, endpoint: &Self::Endpoint, volume: Volume) -> Result<(), AudioError> {
        // SAFETY: actor just revalidated this flow's exact default identity.
        // Volume is checked finite 0..=1; NULL event context deliberately keeps
        // external and own notifications alike, deduplicated by actual readback.
        unsafe {
            endpoint
                .volume
                .SetMasterVolumeLevelScalar(volume.scalar(), std::ptr::null())
        }
        .map_err(|error| native_error("Set endpoint master volume", error))
    }

    fn set_muted(&mut self, endpoint: &Self::Endpoint, muted: bool) -> Result<(), AudioError> {
        // SAFETY: actor just revalidated the default. Absolute state, no toggle.
        unsafe { endpoint.volume.SetMute(muted, std::ptr::null()) }
            .map_err(|error| native_error("Set endpoint mute", error))
    }

    fn start_watch(&mut self, signals: SignalSender) -> Result<(), AudioError> {
        self.signals = Some(signals);
        self.rebind_watch()
    }

    fn rebind_watch(&mut self) -> Result<(), AudioError> {
        self.register_devices()?;
        // Attempt both independently even when one endpoint cannot be watched.
        // Keep device notifications alive to recover on genuine device churn.
        let output = self.rebind_flow(AudioFlow::Output);
        let input = self.rebind_flow(AudioFlow::Input);
        output.and(input)
    }

    fn stop_watch(&mut self) {
        // All unregister calls run on the owner, never on the callback stack.
        self.volumes = [None, None];
        if let Some(callback) = self.device_callback.take() {
            // SAFETY: exactly the live callback retained after registration.
            // Hold the owned reference through the unregister call.
            let _ = unsafe {
                self.enumerator
                    .UnregisterEndpointNotificationCallback(&callback)
            };
        }
        self.signals = None;
    }
}

impl Drop for NativeAudio {
    fn drop(&mut self) {
        self.stop_watch();
    }
}

pub(crate) struct NativeEndpoint {
    id: EndpointId,
    volume: IAudioEndpointVolume,
}

struct VolumeWatch {
    endpoint: NativeEndpoint,
    callback: IAudioEndpointVolumeCallback,
}

impl VolumeWatch {
    fn new(endpoint: NativeEndpoint, signals: SignalSender) -> Result<Self, AudioError> {
        let callback: IAudioEndpointVolumeCallback = VolumeNotification { signals }.into();
        // SAFETY: endpoint and callback remain owned by this worker until
        // unregister. Registration additionally owns its own callback reference.
        unsafe { endpoint.volume.RegisterControlChangeNotify(&callback) }
            .map_err(|error| native_error("Register endpoint volume notifications", error))?;
        Ok(Self { endpoint, callback })
    }
}

impl Drop for VolumeWatch {
    fn drop(&mut self) {
        // SAFETY: owner thread, registered callback remains alive during call.
        let _ = unsafe {
            self.endpoint
                .volume
                .UnregisterControlChangeNotify(&self.callback)
        };
    }
}

struct Apartment;

impl Apartment {
    fn new() -> Result<Self, AudioError> {
        // SAFETY: called once on a newly spawned native audio worker. S_OK and
        // S_FALSE both require one balanced CoUninitialize; any failure does not.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| native_error("Initialize audio COM apartment", error))?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: paired successful initialization on this same worker; all
        // owned MMDevice and endpoint-volume references have already released.
        unsafe { CoUninitialize() };
    }
}

struct TaskString(PWSTR);

impl Drop for TaskString {
    fn drop(&mut self) {
        // SAFETY: one allocation returned by successful IMMDevice::GetId;
        // this guard is its unique owner even on UTF-16 validation failure.
        unsafe { CoTaskMemFree(Some(self.0.0.cast())) };
    }
}

fn device_id(device: &IMMDevice) -> Result<EndpointId, AudioError> {
    // SAFETY: native worker-owned device. Success returns one NUL-terminated
    // CoTaskMem allocation; failure returns NULL and transfers no allocation.
    let allocation = TaskString(
        unsafe { device.GetId() }
            .map_err(|error| native_error("Read audio endpoint identity", error))?,
    );
    if allocation.0.0.is_null() {
        return Err(invalid_id());
    }
    // Windows guarantees NUL termination. Bound accepted IDs and avoid lossy
    // decoding: replacement characters could equate distinct native identities.
    const MAX_ID_UNITS: usize = 32_768;
    for length in 0..=MAX_ID_UNITS {
        // SAFETY: GetId guarantees a readable string through its first NUL.
        if unsafe { *allocation.0.0.add(length) } == 0 {
            // SAFETY: the preceding scan established these initialized units.
            let units = unsafe { std::slice::from_raw_parts(allocation.0.0, length) };
            let value = String::from_utf16(units).map_err(|_| invalid_id())?;
            return EndpointId::new(value);
        }
    }
    Err(invalid_id())
}

fn invalid_id() -> AudioError {
    AudioError::new(
        AudioErrorKind::InvalidValue,
        "Invalid or oversized native audio endpoint identity",
    )
}

fn native_flow(flow: AudioFlow) -> EDataFlow {
    match flow {
        AudioFlow::Output => eRender,
        AudioFlow::Input => eCapture,
    }
}

fn not_found() -> HRESULT {
    // MMDevice's E_NOTFOUND is HRESULT_FROM_WIN32(ERROR_NOT_FOUND), not the
    // unrelated HTML Help E_NOTFOUND constant with a similar name.
    HRESULT::from_win32(ERROR_NOT_FOUND.0)
}

fn native_error(operation: &'static str, error: Error) -> AudioError {
    let code = error.code();
    let kind = if code == E_ACCESSDENIED {
        AudioErrorKind::AccessDenied
    } else if code == AUDCLNT_E_DEVICE_INVALIDATED || code == not_found() {
        AudioErrorKind::DeviceChanged
    } else if code == E_INVALIDARG {
        AudioErrorKind::InvalidValue
    } else {
        AudioErrorKind::Other
    };
    // Fixed operation labels + HRESULT are bounded and useful without exposing
    // potentially enormous or driver-controlled system error strings.
    AudioError::new(
        kind,
        format!("{operation} failed ({:#010X})", code.0 as u32),
    )
}

fn watch_stopped() -> AudioError {
    AudioError::new(
        AudioErrorKind::Stopped,
        "Audio notifications are not active",
    )
}

#[implement(IMMNotificationClient)]
struct DeviceNotification {
    signals: SignalSender,
}

impl IMMNotificationClient_Impl for DeviceNotification_Impl {
    fn OnDeviceStateChanged(
        &self,
        _id: &PCWSTR,
        _state: DEVICE_STATE,
    ) -> windows::core::Result<()> {
        self.signals.notify(Signal::Devices);
        Ok(())
    }

    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        self.signals.notify(Signal::Devices);
        Ok(())
    }

    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        self.signals.notify(Signal::Devices);
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _id: &PCWSTR,
    ) -> windows::core::Result<()> {
        if role == eMultimedia && (flow == eRender || flow == eCapture) {
            self.signals.notify(Signal::Devices);
        }
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _id: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows::core::Result<()> {
        self.signals.notify(Signal::Devices);
        Ok(())
    }
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeNotification {
    signals: SignalSender,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeNotification_Impl {
    fn OnNotify(
        &self,
        _notification: *mut AUDIO_VOLUME_NOTIFICATION_DATA,
    ) -> windows::core::Result<()> {
        // No pointer dereference, user callbacks, COM, unregister, or UI work.
        self.signals.notify(Signal::Volume);
        Ok(())
    }
}
