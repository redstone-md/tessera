// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! These tests assign genuine pinned SDK function pointers. They never install
//! hooks, register shortcuts, query input/cursor state, or generate native input.

use windows_sys::Win32::Foundation::{HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, UnregisterHotKey,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetPhysicalCursorPos, HHOOK, HOOKPROC, MSG, MsgWaitForMultipleObjectsEx,
    PeekMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
};

#[test]
fn pinned_hook_hotkey_message_pump_shapes_without_native_effects() {
    let _: unsafe extern "system" fn(i32, HOOKPROC, HINSTANCE, u32) -> HHOOK = SetWindowsHookExW;
    let _: unsafe extern "system" fn(HHOOK) -> i32 = UnhookWindowsHookEx;
    let _: unsafe extern "system" fn(HHOOK, i32, WPARAM, LPARAM) -> LRESULT = CallNextHookEx;
    let _: unsafe extern "system" fn(HWND, i32, u32, u32) -> i32 = RegisterHotKey;
    let _: unsafe extern "system" fn(HWND, i32) -> i32 = UnregisterHotKey;
    let _: unsafe extern "system" fn(*mut MSG, HWND, u32, u32, u32) -> i32 = PeekMessageW;
    let _: unsafe extern "system" fn(u32, *const HANDLE, u32, u32, u32) -> u32 =
        MsgWaitForMultipleObjectsEx;
    // Physical-screen API shape only; this fixture never reads cursor state.
    let _: unsafe extern "system" fn(*mut POINT) -> i32 = GetPhysicalCursorPos;
    let _: unsafe extern "system" fn(i32) -> i16 = GetAsyncKeyState;
}
