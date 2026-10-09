// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::ffi::c_void;
use std::ptr;

use tessera_system::network::NetworkError;
use windows::Win32::Foundation::HANDLE;
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

use super::calls::{NativeCalls, Reply, native_error};

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
