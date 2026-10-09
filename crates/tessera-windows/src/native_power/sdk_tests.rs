// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Compile-time ABI shapes and constants ONLY. Never invoke native power APIs.

use windows_sys::Win32::{Foundation, Security, System};
use windows_sys::core::{BOOL, PCWSTR};

#[test]
fn power_sdk_binds_raw_bool_boolean_and_authoritative_dword_pointers_without_calls() {
    let _: unsafe extern "system" fn() -> BOOL = System::Shutdown::LockWorkStation;
    let _: unsafe extern "system" fn(u32, u32) -> BOOL = System::Shutdown::ExitWindowsEx;
    let _: unsafe extern "system" fn(PCWSTR, PCWSTR, u32, u32, u32) -> u32 =
        System::Shutdown::InitiateShutdownW;
    // SetSuspendState's installed BOOLEAN projection is Rust bool, not BOOL.
    let _: unsafe extern "system" fn(bool, bool, bool) -> bool = System::Power::SetSuspendState;
    let _: unsafe extern "system" fn(i32) -> BOOL = Security::ImpersonateSelf;
    let _: unsafe extern "system" fn() -> Foundation::HANDLE = System::Threading::GetCurrentThread;
    let _: unsafe extern "system" fn(
        Foundation::HANDLE,
        u32,
        BOOL,
        *mut Foundation::HANDLE,
    ) -> BOOL = System::Threading::OpenThreadToken;
    let _: unsafe extern "system" fn(PCWSTR, PCWSTR, *mut Foundation::LUID) -> BOOL =
        Security::LookupPrivilegeValueW;
    let _: unsafe extern "system" fn(
        Foundation::HANDLE,
        BOOL,
        *const Security::TOKEN_PRIVILEGES,
        u32,
        *mut Security::TOKEN_PRIVILEGES,
        *mut u32,
    ) -> BOOL = Security::AdjustTokenPrivileges;
    let _: unsafe extern "system" fn() -> BOOL = Security::RevertToSelf;
    let _: unsafe extern "system" fn(Foundation::HANDLE) -> BOOL = Foundation::CloseHandle;
    let _: unsafe extern "system" fn() -> u32 = Foundation::GetLastError;
}

#[test]
fn power_sdk_constants_match_closed_source_mapping_without_force_flags() {
    assert_eq!(Foundation::ERROR_ACCESS_DENIED, super::ACCESS_DENIED);
    assert_eq!(Foundation::ERROR_NOT_ALL_ASSIGNED, 1300);
    assert_eq!(
        Security::SecurityImpersonation,
        super::SECURITY_IMPERSONATION
    );
    assert_eq!(
        Security::TOKEN_ADJUST_PRIVILEGES | Security::TOKEN_QUERY,
        super::SHUTDOWN_TOKEN_ACCESS
    );
    assert_eq!(Security::SE_PRIVILEGE_ENABLED, super::PRIVILEGE_ENABLED);
    assert_eq!(System::Shutdown::EWX_LOGOFF, 0);
    assert_eq!(
        System::Shutdown::SHUTDOWN_POWEROFF,
        super::SHUTDOWN_POWEROFF
    );
    assert_eq!(System::Shutdown::SHUTDOWN_RESTART, super::SHUTDOWN_RESTART);
    assert_eq!(
        System::Shutdown::SHUTDOWN_INSTALL_UPDATES,
        super::SHUTDOWN_INSTALL_UPDATES
    );
    assert_eq!(System::Shutdown::SHTDN_REASON_NONE, 0);
    assert_eq!(
        System::Shutdown::SHTDN_REASON_FLAG_PLANNED
            | System::Shutdown::SHTDN_REASON_MAJOR_OPERATINGSYSTEM
            | System::Shutdown::SHTDN_REASON_MINOR_UPGRADE,
        super::UPDATE_REASON
    );
}
