// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::ffi::c_void;
use std::ptr;

use tessera_system::network::NetworkError;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::Ndis::{NDIS_OBJECT_HEADER, NDIS_OBJECT_TYPE_DEFAULT};
use windows::Win32::NetworkManagement::WiFi::{
    DOT11_BSSID_LIST, DOT11_BSSID_LIST_REVISION_1, DOT11_SSID, WLAN_CONNECTION_PARAMETERS,
    WLAN_PHY_RADIO_STATE, WlanConnect, WlanDeleteProfile, WlanDisconnect, WlanGetProfile,
    WlanGetProfileList, WlanSetInterface, dot11_BSS_type_infrastructure,
    wlan_connection_mode_profile, wlan_connection_mode_temporary_profile,
    wlan_intf_opcode_radio_state,
};
use windows::Win32::NetworkManagement::WiFi::{
    WLAN_INTF_OPCODE, WLAN_NOTIFICATION_CALLBACK, WLAN_NOTIFICATION_SOURCES, WlanCloseHandle,
    WlanEnumInterfaces, WlanFreeMemory, WlanGetAvailableNetworkList, WlanGetNetworkBssList,
    WlanOpenHandle, WlanQueryInterface, WlanRegisterNotification, dot11_BSS_type_any,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::core::{GUID, w};
use windows::core::{PCWSTR, PWSTR};

use super::calls::{ConnectRequest, NativeCalls, ProfileReply, Reply, native_error};

pub(super) struct WindowsCalls;

struct SettingsApartment(std::marker::PhantomData<std::rc::Rc<()>>);

impl SettingsApartment {
    fn enter() -> Result<Self, NetworkError> {
        // SAFETY: temporary STA initialization on the otherwise COM-neutral
        // native worker. S_OK and S_FALSE both require balanced uninitialization.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(|error| {
                native_error("Network Settings COM initialization", error.code().0 as u32)
            })?;
        Ok(Self(std::marker::PhantomData))
    }
}

impl Drop for SettingsApartment {
    fn drop(&mut self) {
        // SAFETY: balances this successful initialization on the same worker.
        unsafe { CoUninitialize() };
    }
}
// SAFETY: only the documented WLAN allocation APIs produce buffers here. Their
// single owner frees them through WlanFreeMemory. SOURCE_NONE is called only on
// the worker and is the documented active-callback barrier.
unsafe impl NativeCalls for WindowsCalls {
    fn open(&self, version: u32) -> (u32, u32, usize) {
        let mut negotiated = 0;
        let mut handle = HANDLE::default();
        // SAFETY: reserved NULL; both out parameters live through the call.
        let status = unsafe { WlanOpenHandle(version, None, &mut negotiated, &mut handle) };
        (status, negotiated, handle.0 as usize)
    }

    fn close(&self, handle: usize) -> u32 {
        // SAFETY: owner takes the unique handle once; never uses CloseHandle.
        unsafe { WlanCloseHandle(HANDLE(handle as *mut c_void), None) }
    }

    fn interfaces(&self, handle: usize) -> Reply {
        let mut data = ptr::null_mut();
        // SAFETY: live WLAN handle; reserved NULL; initialized out pointer.
        let status = unsafe { WlanEnumInterfaces(HANDLE(handle as *mut c_void), None, &mut data) };
        Reply {
            status,
            data: data.cast(),
            size: None,
        }
    }

    fn query(&self, handle: usize, id: &GUID, opcode: WLAN_INTF_OPCODE) -> Reply {
        let mut data = ptr::null_mut();
        let mut size = 0;
        // SAFETY: bounded worker-owned identity/opcode; no value-type output needed.
        let status = unsafe {
            WlanQueryInterface(
                HANDLE(handle as *mut c_void),
                id,
                opcode,
                None,
                &mut size,
                &mut data,
                None,
            )
        };
        Reply {
            status,
            data,
            size: Some(size as usize),
        }
    }

    fn available(&self, handle: usize, id: &GUID, flags: u32) -> Reply {
        let mut data = ptr::null_mut();
        // SAFETY: flags always zero; no hidden/out-of-range profile enumeration.
        let status = unsafe {
            WlanGetAvailableNetworkList(HANDLE(handle as *mut c_void), id, flags, None, &mut data)
        };
        Reply {
            status,
            data: data.cast(),
            size: None,
        }
    }

    fn bss(&self, handle: usize, id: &GUID) -> Reply {
        let mut data = ptr::null_mut();
        // SAFETY: SSID NULL requests the OS cache, all BSS types; security filter
        // is ignored for NULL SSID. This performs no active scan.
        let status = unsafe {
            WlanGetNetworkBssList(
                HANDLE(handle as *mut c_void),
                id,
                None,
                dot11_BSS_type_any,
                false,
                None,
                &mut data,
            )
        };
        Reply {
            status,
            data: data.cast(),
            size: None,
        }
    }
    fn profile(&self, handle: usize, id: &GUID, name: &[u16]) -> ProfileReply {
        let mut xml = PWSTR::null();
        // Never request WLAN_PROFILE_GET_PLAINTEXT_KEY.
        let mut flags = 0;
        let mut granted_access = 0;
        let status = unsafe {
            WlanGetProfile(
                HANDLE(handle as *mut c_void),
                id,
                PCWSTR(name.as_ptr()),
                None,
                &mut xml,
                Some(&mut flags),
                Some(&mut granted_access),
            )
        };
        ProfileReply {
            allocation: Reply {
                status,
                data: xml.0.cast(),
                size: None,
            },
            flags,
            granted_access,
        }
    }

    fn profiles(&self, handle: usize, id: &GUID) -> Reply {
        let mut data = ptr::null_mut();
        let status =
            unsafe { WlanGetProfileList(HANDLE(handle as *mut c_void), id, None, &mut data) };
        Reply {
            status,
            data: data.cast(),
            size: None,
        }
    }

    fn delete_profile(&self, handle: usize, id: &GUID, name: &[u16]) -> u32 {
        unsafe {
            WlanDeleteProfile(
                HANDLE(handle as *mut c_void),
                id,
                PCWSTR(name.as_ptr()),
                None,
            )
        }
    }

    fn profile_digest(&self, bytes: &[u8]) -> Result<[u8; 32], NetworkError> {
        use windows_sys::Win32::Security::Cryptography::CryptHashCertificate2;
        let mut digest = [0; 32];
        let mut length = 32;
        let size = u32::try_from(bytes.len())
            .map_err(|_| super::calls::invalid("WLAN profile descriptor is too large."))?;
        // Documented general byte-buffer hashing; no certificate/key store access.
        let success = unsafe {
            CryptHashCertificate2(
                windows_sys::core::w!("SHA256"),
                0,
                ptr::null(),
                bytes.as_ptr(),
                size,
                digest.as_mut_ptr(),
                &mut length,
            )
        };
        if success == 0 {
            return Err(native_error("WLAN profile descriptor hashing", unsafe {
                windows_sys::Win32::Foundation::GetLastError()
            }));
        }
        if length != 32 {
            return Err(super::calls::invalid(
                "WLAN profile descriptor digest is invalid.",
            ));
        }
        Ok(digest)
    }

    fn connect(&self, handle: usize, id: &GUID, request: ConnectRequest<'_>) -> u32 {
        let ConnectRequest {
            ssid,
            bssid,
            profile,
            temporary,
        } = request;
        let mut native_ssid = DOT11_SSID {
            uSSIDLength: ssid.len() as u32,
            ..Default::default()
        };
        native_ssid.ucSSID[..ssid.len()].copy_from_slice(ssid);
        let mut desired = DOT11_BSSID_LIST {
            Header: NDIS_OBJECT_HEADER {
                Type: NDIS_OBJECT_TYPE_DEFAULT as u8,
                Revision: DOT11_BSSID_LIST_REVISION_1 as u8,
                Size: size_of::<DOT11_BSSID_LIST>() as u16,
            },
            uNumOfEntries: 1,
            uTotalNumOfEntries: 1,
            BSSIDs: bssid,
        };
        let parameters = WLAN_CONNECTION_PARAMETERS {
            wlanConnectionMode: if temporary {
                wlan_connection_mode_temporary_profile
            } else {
                wlan_connection_mode_profile
            },
            strProfile: PCWSTR(profile.as_ptr()),
            pDot11Ssid: &mut native_ssid,
            pDesiredBssidList: &mut desired,
            dot11BssType: dot11_BSS_type_infrastructure,
            dwFlags: 0,
        };
        // All pointers own initialized call-local storage until synchronous return.
        unsafe { WlanConnect(HANDLE(handle as *mut c_void), id, &parameters, None) }
    }

    fn disconnect(&self, handle: usize, id: &GUID) -> u32 {
        unsafe { WlanDisconnect(HANDLE(handle as *mut c_void), id, None) }
    }

    fn set_radio(&self, handle: usize, id: &GUID, state: &WLAN_PHY_RADIO_STATE) -> u32 {
        unsafe {
            WlanSetInterface(
                HANDLE(handle as *mut c_void),
                id,
                wlan_intf_opcode_radio_state,
                std::mem::size_of::<WLAN_PHY_RADIO_STATE>() as u32,
                std::ptr::from_ref(state).cast(),
                None,
            )
        }
    }

    unsafe fn free(&self, data: *mut c_void) {
        // SAFETY: caller transfers a uniquely owned WLAN allocation exactly once.
        unsafe { WlanFreeMemory(data) };
    }

    fn register(
        &self,
        handle: usize,
        source: u32,
        callback: WLAN_NOTIFICATION_CALLBACK,
        context: *const c_void,
    ) -> u32 {
        // SAFETY: pinned callback context survives until a successful NONE
        // barrier. No locks needed by the callback are held by this worker.
        unsafe {
            WlanRegisterNotification(
                HANDLE(handle as *mut c_void),
                WLAN_NOTIFICATION_SOURCES(source),
                true,
                callback,
                Some(context),
                None,
                None,
            )
        }
    }

    fn open_settings(&self) -> Result<(), NetworkError> {
        let _apartment = SettingsApartment::enter()?;
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: w!("open"),
            lpFile: w!("ms-settings:network"),
            nShow: 1,
            ..Default::default()
        };
        // SAFETY: static fixed URI and verb, no parameters, directory, caller
        // strings or process-handle request. Dispatch acknowledgement only.
        unsafe { ShellExecuteExW(&mut info) }.map_err(|error| {
            let code = error.code().0 as u32;
            native_error(
                "Network Settings dispatch",
                if code & 0xffff_0000 == 0x8007_0000 {
                    code & 0xffff
                } else {
                    code
                },
            )
        })
    }

    fn retirement_error(&self, error: &NetworkError) {
        use std::io::Write;
        // Diagnostics must not unwind through owner retirement on an I/O error.
        let _ = writeln!(
            std::io::stderr().lock(),
            "WLAN retirement: {}",
            error.message
        );
    }
}
