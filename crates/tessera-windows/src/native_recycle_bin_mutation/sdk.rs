// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::sync::atomic::{AtomicU64, Ordering};

use windows_sys::Win32::Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::SHEmptyRecycleBinW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, UnregisterClassW, WNDCLASSEXW,
};

use super::{Calls, suppress_owner_close};
use crate::recycle_bin_mutation::worker::native_error;
use tessera_system::dock_utilities::DockUtilityError;

pub(super) struct WindowsCalls;

pub(super) struct WindowClass {
    name: Vec<u16>,
    instance: HINSTANCE,
}

fn class_definition(instance: HINSTANCE, name: *const u16) -> WNDCLASSEXW {
    WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(owner_proc),
        hInstance: instance,
        lpszClassName: name,
        ..Default::default()
    }
}

unsafe extern "system" fn owner_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // No Rust context pointer, callback, allocation or panicking operation.
    if suppress_owner_close(message, wparam) {
        0
    } else {
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }
}

fn last_error(operation: &'static str) -> DockUtilityError {
    let code = unsafe { GetLastError() };
    // Only failed Win32 setup calls use GetLastError, never native Empty.
    let hresult = if code == 0 {
        0x8000_4005
    } else {
        (code & 0xFFFF) | 0x8007_0000
    };
    native_error(hresult, operation)
}

impl Calls for WindowsCalls {
    type Class = WindowClass;
    type Owner = HWND;

    fn initialize(&self, model: i32) -> i32 {
        unsafe { CoInitializeEx(std::ptr::null(), model as u32) }
    }

    fn uninitialize(&self) {
        unsafe { CoUninitialize() }
    }

    fn register_class(&self) -> Result<Self::Class, DockUtilityError> {
        static NEXT_CLASS: AtomicU64 = AtomicU64::new(1);
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        if instance.is_null() {
            return Err(last_error("Recycle Bin mutation owner module"));
        }
        let identity = NEXT_CLASS.fetch_add(1, Ordering::Relaxed);
        let name = format!("TesseraRecycleEmptyOwner{identity}")
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let definition = class_definition(instance, name.as_ptr());
        if unsafe { RegisterClassExW(&definition) } == 0 {
            return Err(last_error("Recycle Bin mutation owner class"));
        }
        Ok(WindowClass { name, instance })
    }

    fn unregister_class(&self, class: &Self::Class) {
        // Best-effort native cleanup can fail (e.g. a window still exists).
        // No Rust pointers are retained by the class/WndProc. Do not replace a
        // returned Empty HRESULT with a claim that cleanup succeeded.
        let _ = unsafe { UnregisterClassW(class.name.as_ptr(), class.instance) };
    }

    fn create_owner(
        &self,
        class: &Self::Class,
        style: u32,
    ) -> Result<Self::Owner, DockUtilityError> {
        let owner = unsafe {
            CreateWindowExW(
                0,
                class.name.as_ptr(),
                windows_sys::w!("Tessera Recycle Bin"),
                style,
                0,
                0,
                1,
                1,
                std::ptr::null_mut(), // Independent top-level popup, no GUI parent.
                std::ptr::null_mut(),
                class.instance,
                std::ptr::null(), // No Rust userdata, including during creation.
            )
        };
        if owner.is_null() {
            Err(last_error("Recycle Bin mutation owner creation"))
        } else {
            Ok(owner)
        }
        // Intentionally no WS_VISIBLE/ShowWindow/foreground manipulation. This
        // does not certify actual Shell confirmation visibility or activation.
    }

    fn destroy_owner(&self, owner: &Self::Owner) {
        // Creator-thread private authority, not an IsWindow/recycled-HWND check.
        // Failure remains a runtime limitation; never retry Empty or leak Rust
        // context in pursuit of native cleanup. This scope runs before completion.
        let _ = unsafe { DestroyWindow(*owner) };
    }

    fn empty(&self, owner: &Self::Owner, root: *const u16, flags: u32) -> i32 {
        // Raw windows-sys preserves nonnegative statuses as well as errors.
        unsafe { SHEmptyRecycleBinW(*owner, root, flags) }
    }
}

#[cfg(test)]
#[path = "sdk_tests.rs"]
mod tests;
