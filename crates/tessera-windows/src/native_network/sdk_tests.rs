// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Compile-only SDK shape checks. Function items are assigned, never invoked.

use std::ffi::c_void;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::WiFi::*;
use windows::Win32::System::Com::{COINIT, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::{SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::core::{GUID, HRESULT};

type WlanQueryInterfaceFn = unsafe fn(
    HANDLE,
    *const GUID,
    WLAN_INTF_OPCODE,
    Option<*const c_void>,
    *mut u32,
    *mut *mut c_void,
    Option<*mut WLAN_OPCODE_VALUE_TYPE>,
) -> u32;
type WlanGetNetworkBssListFn = unsafe fn(
    HANDLE,
    *const GUID,
    Option<*const DOT11_SSID>,
    DOT11_BSS_TYPE,
    bool,
    Option<*const c_void>,
    *mut *mut WLAN_BSS_LIST,
) -> u32;
type WlanRegisterNotificationFn = unsafe fn(
    HANDLE,
    WLAN_NOTIFICATION_SOURCES,
    bool,
    WLAN_NOTIFICATION_CALLBACK,
    Option<*const c_void>,
    Option<*const c_void>,
    Option<*mut u32>,
) -> u32;

#[test]
fn native_network_sdk_shapes_compile_without_native_calls() {
    let _: unsafe fn(u32, Option<*const c_void>, *mut u32, *mut HANDLE) -> u32 = WlanOpenHandle;
    let _: unsafe fn(HANDLE, Option<*const c_void>) -> u32 = WlanCloseHandle;
    let _: unsafe fn(*const c_void) = WlanFreeMemory;
    let _: unsafe fn(HANDLE, Option<*const c_void>, *mut *mut WLAN_INTERFACE_INFO_LIST) -> u32 =
        WlanEnumInterfaces;
    let _: WlanQueryInterfaceFn = WlanQueryInterface;
    let _: unsafe fn(
        HANDLE,
        *const GUID,
        u32,
        Option<*const c_void>,
        *mut *mut WLAN_AVAILABLE_NETWORK_LIST,
    ) -> u32 = WlanGetAvailableNetworkList;
    let _: WlanGetNetworkBssListFn = WlanGetNetworkBssList;
    let _: WlanRegisterNotificationFn = WlanRegisterNotification;
    let _: WLAN_NOTIFICATION_CALLBACK = Some(super::callback::notification);
    let _: unsafe fn(*mut SHELLEXECUTEINFOW) -> windows::core::Result<()> = ShellExecuteExW;
    let _: unsafe fn(Option<*const c_void>, COINIT) -> HRESULT = CoInitializeEx;
    let _: unsafe fn() = CoUninitialize;
}

#[test]
fn native_network_factory_and_worker_owner_shapes_are_pure() {
    fn assert_owner<T: crate::network::worker::NetworkOwner + Send + 'static>() {}
    assert_owner::<super::NativeOwner>();
    let _: fn() -> super::NativeOwner = super::NativeOwner::new;
    // Pure owner construction/drop: no handle, context or apartment was acquired.
    drop(super::NativeOwner::new());
}
