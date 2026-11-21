// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The volume of the default audio output and input, the outputs there
//! are to choose from, and the volume of each application that plays.

use std::ptr;
use std::sync::Once;

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::{PROPERTYKEY, S_OK};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    AudioSessionStateExpired, DEVICE_STATE_ACTIVE, EDataFlow, ERole, IAudioSessionControl2,
    IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
    eCapture, eCommunications, eConsole, eMultimedia, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, STGM_READ,
};
use windows::core::{GUID, HRESULT, HSTRING, IUnknown, IUnknown_Vtbl, Interface, PCWSTR};

use super::app_windows::executable_of;
use super::packaged::take;

/// The name of a device as the sound settings show it.
const FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

/// The class that sets the default devices. Windows does not document it or
/// its interface; the sound settings and volume mixers use them.
const POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// Only the method that names a default device is declared as it is; the
/// ones before it keep their places in the table and are not called.
#[windows::core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    unsafe fn mix_format(&self) -> HRESULT;
    unsafe fn device_format(&self) -> HRESULT;
    unsafe fn reset_device_format(&self) -> HRESULT;
    unsafe fn set_device_format(&self) -> HRESULT;
    unsafe fn processing_period(&self) -> HRESULT;
    unsafe fn set_processing_period(&self) -> HRESULT;
    unsafe fn share_mode(&self) -> HRESULT;
    unsafe fn set_share_mode(&self) -> HRESULT;
    unsafe fn property_value(&self) -> HRESULT;
    unsafe fn set_property_value(&self) -> HRESULT;
    unsafe fn set_default_endpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
}

/// Lets this thread use COM. A thread that already does, in whichever mode,
/// is left as it is.
fn use_com() {
    thread_local! {
        static COM: Once = const { Once::new() };
    }
    COM.with(|com| {
        com.call_once(|| {
            // SAFETY: initializing COM has no preconditions.
            let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        });
    });
}

fn enumerator() -> Result<IMMDeviceEnumerator> {
    use_com();
    // SAFETY: a COM call that creates an object of a registered class.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .context("audio devices cannot be listed")
}

fn default_endpoint(flow: EDataFlow) -> Result<IMMDevice> {
    // SAFETY: a COM call without pointer arguments.
    unsafe { enumerator()?.GetDefaultAudioEndpoint(flow, eMultimedia) }
        .context("there is no such audio device")
}

fn id_of(device: &IMMDevice) -> Option<String> {
    // SAFETY: the string is allocated for the caller, and `take` frees it.
    take(unsafe { device.GetId() }.ok()?)
}

/// An audio output there is to play through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    /// What Windows knows the device by.
    pub id: String,
    pub name: String,
    /// Whether ordinary sound goes through it now.
    pub in_use: bool,
}

impl AudioDevice {
    /// The outputs that are plugged in and enabled, the one in use first.
    pub fn outputs() -> Result<Vec<Self>> {
        let devices = enumerator()?;
        let in_use = default_endpoint(eRender).ok().as_ref().and_then(id_of);
        // SAFETY: plain COM calls; each value read is owned here.
        unsafe {
            let found = devices
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
                .context("the audio outputs cannot be listed")?;
            let mut outputs = Vec::new();
            for index in 0..found.GetCount().unwrap_or(0) {
                let Ok(device) = found.Item(index) else {
                    continue;
                };
                let Some(id) = id_of(&device) else {
                    continue;
                };
                let name = device
                    .OpenPropertyStore(STGM_READ)
                    .and_then(|store| store.GetValue(&FRIENDLY_NAME))
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                outputs.push(Self {
                    in_use: in_use.as_deref() == Some(id.as_str()),
                    name,
                    id,
                });
            }
            outputs.sort_by_key(|output| !output.in_use);
            Ok(outputs)
        }
    }

    /// Has every kind of sound go through this output from now on.
    pub fn choose(&self) -> Result<()> {
        use_com();
        let id = HSTRING::from(self.id.as_str());
        // SAFETY: `id` outlives the calls that read it.
        unsafe {
            let policy: IPolicyConfig = CoCreateInstance(&POLICY_CONFIG, None, CLSCTX_ALL)
                .context("the default audio output cannot be changed here")?;
            for role in [eConsole, eMultimedia, eCommunications] {
                policy
                    .set_default_endpoint(PCWSTR(id.as_ptr()), role)
                    .ok()
                    .context("the audio output was not taken as the default")?;
            }
        }
        Ok(())
    }
}

/// What one program plays, all its streams on the default output together.
#[derive(Clone, Debug, PartialEq)]
pub struct AppSound {
    /// The name of the program file without its ending; empty for the
    /// sounds of Windows itself.
    pub program: String,
    /// From 0 to 1, a share of the volume of the output.
    pub volume: f32,
    pub muted: bool,
}

impl AppSound {
    /// The streams of the default output that are not over, each with the
    /// name of its program.
    fn streams() -> Result<Vec<(String, ISimpleAudioVolume)>> {
        // SAFETY: plain COM calls on interfaces owned here.
        unsafe {
            let sessions = default_endpoint(eRender)?
                .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
                .context("the audio output does not tell what plays")?
                .GetSessionEnumerator()
                .context("the audio streams cannot be listed")?;
            let mut streams = Vec::new();
            for index in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(session) = sessions.GetSession(index) else {
                    continue;
                };
                let Ok(session) = session.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                if session.GetState() == Ok(AudioSessionStateExpired) {
                    continue;
                }
                let program = if session.IsSystemSoundsSession() == S_OK {
                    Some(String::new())
                } else {
                    let path = session.GetProcessId().ok().and_then(executable_of);
                    path.and_then(|path| Some(path.file_stem()?.to_string_lossy().into_owned()))
                };
                if let (Some(program), Ok(volume)) = (program, session.cast()) {
                    streams.push((program, volume));
                }
            }
            Ok(streams)
        }
    }

    /// The programs that play through the default output, the sounds of
    /// Windows first and the rest by name.
    pub fn all() -> Result<Vec<Self>> {
        let mut sounds: Vec<Self> = Vec::new();
        for (program, stream) in Self::streams()? {
            // SAFETY: COM calls without pointer arguments.
            let (volume, muted) = unsafe {
                let muted = stream.GetMute().is_ok_and(|muted| muted.as_bool());
                (stream.GetMasterVolume().unwrap_or(1.), muted)
            };
            match sounds.iter_mut().find(|sound| sound.program == program) {
                // A program is as loud as its loudest stream, and muted
                // when all of them are.
                Some(sound) => {
                    sound.volume = sound.volume.max(volume);
                    sound.muted &= muted;
                }
                None => sounds.push(Self {
                    program,
                    volume,
                    muted,
                }),
            }
        }
        sounds.sort_by_key(|sound| sound.program.to_lowercase());
        Ok(sounds)
    }

    /// Does `change` to every stream of `program`.
    fn each(
        program: &str,
        change: impl Fn(&ISimpleAudioVolume) -> windows::core::Result<()>,
    ) -> Result<()> {
        let streams = Self::streams()?;
        for (_, stream) in streams.iter().filter(|(name, _)| name == program) {
            change(stream).context("the volume of the program cannot be changed")?;
        }
        Ok(())
    }

    pub fn set_volume(program: &str, volume: f32) -> Result<()> {
        // SAFETY: the event context may be null.
        Self::each(program, |stream| unsafe {
            stream.SetMasterVolume(volume.clamp(0., 1.), ptr::null())
        })
    }

    pub fn set_muted(program: &str, muted: bool) -> Result<()> {
        // SAFETY: the event context may be null.
        Self::each(program, |stream| unsafe {
            stream.SetMute(muted, ptr::null())
        })
    }
}

/// The device Windows currently plays ordinary sound through, or the one
/// it records from. It is a snapshot: when the user picks another device,
/// ask again.
pub struct AudioOutput {
    endpoint: IAudioEndpointVolume,
}

impl AudioOutput {
    /// The current default output. Fails when there is none.
    pub fn default_device() -> Result<Self> {
        Self::of(eRender)
    }

    /// The current default microphone. Fails when there is none.
    pub fn default_input() -> Result<Self> {
        Self::of(eCapture)
    }

    fn of(flow: EDataFlow) -> Result<Self> {
        // SAFETY: a COM call on an interface that stays alive for as long
        // as its wrapper does.
        let endpoint = unsafe {
            default_endpoint(flow)?
                .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
                .context("the audio device has no volume control")?
        };
        Ok(Self { endpoint })
    }

    /// The volume, from 0 to 1, as the system volume slider shows it.
    pub fn volume(&self) -> Result<f32> {
        // SAFETY: a COM call without pointer arguments.
        unsafe { self.endpoint.GetMasterVolumeLevelScalar() }.context("the volume cannot be read")
    }

    pub fn set_volume(&self, volume: f32) -> Result<()> {
        // SAFETY: the event context may be null.
        unsafe {
            self.endpoint
                .SetMasterVolumeLevelScalar(volume.clamp(0., 1.), ptr::null())
        }
        .context("the volume cannot be changed")
    }

    pub fn is_muted(&self) -> Result<bool> {
        // SAFETY: a COM call without pointer arguments.
        let muted = unsafe { self.endpoint.GetMute() }.context("the mute state cannot be read")?;
        Ok(muted.as_bool())
    }

    pub fn set_muted(&self, muted: bool) -> Result<()> {
        // SAFETY: the event context may be null.
        unsafe { self.endpoint.SetMute(muted, ptr::null()) }
            .context("the mute state cannot be changed")
    }
}
