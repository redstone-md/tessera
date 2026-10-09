// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Compile/function-pointer checks only: never invoke the native functions.

use super::*;
use windows_sys::Win32::Foundation::WIN32_ERROR;
use windows_sys::Win32::System::Registry::{
    HKEY, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_SAM_FLAGS, RegCloseKey, RegOpenKeyExW,
};

#[test]
fn registry_bindings_match_installed_sdk_without_calling_native_apis() {
    let _: unsafe extern "system" fn(
        HKEY,
        *const u16,
        u32,
        REG_SAM_FLAGS,
        *mut HKEY,
    ) -> WIN32_ERROR = RegOpenKeyExW;
    let _: unsafe extern "system" fn(HKEY) -> WIN32_ERROR = RegCloseKey;
    assert_eq!(READ_ACCESS, KEY_READ);
    assert_eq!(READ_ACCESS & (KEY_WOW64_32KEY | KEY_WOW64_64KEY), 0);
    assert_eq!(OPEN_OPTIONS, 0);
}
