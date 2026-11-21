// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The master volume of the default audio output.

use std::ptr;
use std::sync::Once;

use anyhow::{Context as _, Result};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};

/// The device Windows currently plays ordinary sound through. It is a
/// snapshot: when the user picks another device, ask again.
pub struct AudioOutput {
    endpoint: IAudioEndpointVolume,
}

impl AudioOutput {
    /// The current default output. Fails when there is none.
    pub fn default_device() -> Result<Self> {
        static COM: Once = Once::new();
        COM.call_once(|| {
            // SAFETY: initializing COM has no preconditions. A thread that
            // already uses COM, in whichever mode, is left as it is.
            let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        });
        // SAFETY: plain COM calls on interfaces that stay alive for as long
        // as their wrappers do.
        let endpoint = unsafe {
            let devices: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .context("audio devices cannot be listed")?;
            devices
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
                .context("there is no audio output")?
                .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
                .context("the audio output has no volume control")?
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
