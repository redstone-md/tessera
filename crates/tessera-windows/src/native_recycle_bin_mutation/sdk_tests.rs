// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED;
use windows_sys::Win32::UI::Shell::{SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND};
use windows_sys::Win32::UI::WindowsAndMessaging::{SC_CLOSE, WM_CLOSE, WM_SYSCOMMAND, WS_POPUP};

#[test]
fn installed_raw_empty_abi_and_owner_constants_match_production_without_native_calls() {
    let _: unsafe extern "system" fn(HWND, *const u16, u32) -> i32 = SHEmptyRecycleBinW;
    let _: unsafe extern "system" fn(*const std::ffi::c_void, u32) -> i32 = CoInitializeEx;
    let _: unsafe extern "system" fn(HWND) -> i32 = DestroyWindow;
    let _: unsafe extern "system" fn(*const u16, HINSTANCE) -> i32 = UnregisterClassW;
    assert_eq!(super::super::STA_MODEL, COINIT_APARTMENTTHREADED);
    assert_eq!(super::super::EMPTY_FLAGS, SHERB_NOSOUND);
    assert_eq!(
        super::super::EMPTY_FLAGS & (SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI),
        0
    );
    assert_eq!(super::super::OWNER_STYLE, WS_POPUP);
    assert_eq!(super::super::CLOSE_MESSAGE, WM_CLOSE);
    assert_eq!(super::super::SYSTEM_COMMAND_MESSAGE, WM_SYSCOMMAND);
    assert_eq!(super::super::CLOSE_COMMAND, SC_CLOSE as usize);
}

#[test]
fn private_owner_class_has_exact_sdk_size_static_proc_and_no_userdata_slots() {
    let name = [b'T' as u16, 0];
    let definition = class_definition(std::ptr::null_mut(), name.as_ptr());
    assert_eq!(
        definition.cbSize as usize,
        std::mem::size_of::<WNDCLASSEXW>()
    );
    assert!(definition.lpfnWndProc.is_some());
    assert_eq!(definition.cbClsExtra, 0);
    assert_eq!(definition.cbWndExtra, 0);
    assert_eq!(definition.style, 0);
    assert!(definition.hInstance.is_null());
    assert_eq!(definition.lpszClassName, name.as_ptr());
    assert!(definition.lpszMenuName.is_null());
    assert!(definition.hIcon.is_null());
    assert!(definition.hCursor.is_null());
    assert!(definition.hbrBackground.is_null());
    assert!(definition.hIconSm.is_null());
    // Constructing this SDK value does not register a class/create a window.
}
