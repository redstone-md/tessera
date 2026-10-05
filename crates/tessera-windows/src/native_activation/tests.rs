// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use tessera_core::WindowId;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows_sys::core::w;

struct Fixture(HWND);

impl Fixture {
    fn new() -> Self {
        // SAFETY: STATIC is a built-in class, both strings are static UTF-16,
        // and this thread owns the returned window through the Fixture.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                w!("STATIC"),
                w!("Tessera activation fixture"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                40,
                40,
                400,
                240,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null());
        Self(hwnd)
    }

    fn target(&self, process_id: u32) -> ActivationTarget {
        ActivationTarget::new(WindowId::new(self.0 as usize as u64), process_id)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: cleanup on the thread that created and owns this HWND.
        unsafe { DestroyWindow(self.0) };
    }
}

#[test]
fn native_validation_rejects_own_mismatched_closed_and_invalid_windows() {
    let fixture = Fixture::new();
    assert!(matches!(
        activate(fixture.target(std::process::id())),
        Err(ActivationError::NotApplication)
    ));
    assert!(matches!(
        activate(fixture.target(u32::MAX)),
        Err(ActivationError::Unavailable)
    ));
    let closed = fixture.target(std::process::id());
    drop(fixture);
    assert!(matches!(
        activate(closed),
        Err(ActivationError::Unavailable)
    ));
    assert!(matches!(
        activate(ActivationTarget::new(WindowId::new(u64::MAX), 1)),
        Err(ActivationError::Unavailable)
    ));
}

#[test]
fn startup_message_sanitizes_nul_and_truncates() {
    assert_eq!(sanitize_startup_message("a\0b"), "a b");
    let long = "x".repeat(STARTUP_ERROR_MAX_CHARS + 500);
    let sanitized = sanitize_startup_message(&long);
    assert_eq!(sanitized.chars().count(), STARTUP_ERROR_MAX_CHARS);
}
