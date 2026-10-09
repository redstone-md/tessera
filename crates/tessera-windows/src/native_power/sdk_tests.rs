// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Compile-time ABI shape only. Never invoke these session-changing APIs.

#[test]
fn power_sdk_binds_raw_bool_and_dword_function_pointers_without_calls() {
    let _: unsafe extern "system" fn() -> windows_sys::core::BOOL =
        windows_sys::Win32::System::Shutdown::LockWorkStation;
    let _: unsafe extern "system" fn() -> u32 = windows_sys::Win32::Foundation::GetLastError;
    assert_eq!(
        windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED,
        super::ACCESS_DENIED,
    );
}
