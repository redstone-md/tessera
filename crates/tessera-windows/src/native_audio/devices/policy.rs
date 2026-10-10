// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Windows has no public Core Audio default-role setter. Keep the conventional
//! PolicyConfigClient ABI isolated here; activation failure is a real unavailable
//! capability, with no shell/registry/elevation fallback. Reads use IMMDeviceEnumerator.

use std::ffi::c_void;
use tessera_system::audio::{AudioError, AudioErrorKind, EndpointId};
use windows::Win32::Media::Audio::ERole;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::core::{GUID, HRESULT, IUnknown_Vtbl, Interface, PCWSTR};

use super::native_error;

windows::core::imp::define_interface!(
    PolicyConfig,
    PolicyConfigVtbl,
    0xf8679f50_850a_41cf_9c72_430f290290c8
);

// IPolicyConfig's ten methods preceding SetDefaultEndpoint are never called.
// Their ABI-sized function slots must remain present and in this exact order.
#[repr(C)]
pub struct PolicyConfigVtbl {
    base: IUnknown_Vtbl,
    get_mix_format: unsafe extern "system" fn(),
    get_device_format: unsafe extern "system" fn(),
    reset_device_format: unsafe extern "system" fn(),
    set_device_format: unsafe extern "system" fn(),
    get_processing_period: unsafe extern "system" fn(),
    set_processing_period: unsafe extern "system" fn(),
    get_share_mode: unsafe extern "system" fn(),
    set_share_mode: unsafe extern "system" fn(),
    get_property_value: unsafe extern "system" fn(),
    set_property_value: unsafe extern "system" fn(),
    set_default_endpoint: unsafe extern "system" fn(*mut c_void, PCWSTR, ERole) -> HRESULT,
    set_endpoint_visibility: unsafe extern "system" fn(),
}

pub(super) struct DefaultPolicy(PolicyConfig);

impl DefaultPolicy {
    pub(super) fn new() -> Result<Self, AudioError> {
        const CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
        // Only this exact known IID/CLSID pairing authorizes the ABI below.
        unsafe { CoCreateInstance::<_, PolicyConfig>(&CLIENT, None, CLSCTX_ALL) }
            .map(Self)
            .map_err(|error| {
                AudioError::new(
                    AudioErrorKind::Unsupported,
                    format!(
                        "Default audio role control unavailable ({:#010X})",
                        error.code().0 as u32
                    ),
                )
            })
    }

    pub(super) fn set(&self, id: &EndpointId, role: ERole) -> Result<(), AudioError> {
        let value: Vec<u16> = id
            .as_str()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        if value[..value.len() - 1].contains(&0) {
            return Err(AudioError::new(
                AudioErrorKind::InvalidValue,
                "Invalid native endpoint identity",
            ));
        }
        // Caller freshly validated active native membership + incarnation, never
        // caller title/process/path data. Buffer survives the synchronous call.
        unsafe {
            (self.0.vtable().set_default_endpoint)(self.0.as_raw(), PCWSTR(value.as_ptr()), role)
                .ok()
        }
        .map_err(|error| native_error("Set default audio role", error))
    }
}
