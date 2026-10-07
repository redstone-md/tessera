// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native window-action tests against an owned fixture window only.

use super::*;
use crate::activation::ActivationTarget;
use tessera_core::WindowId;
use windows_sys::Win32::Foundation::HWND;
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
                w!("Tessera window-action fixture"),
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
fn actions_reject_stale_mismatched_and_non_application_windows_without_effects() {
    let fixture = Fixture::new();
    // Own-process window: not an application candidate; nothing is posted.
    assert!(matches!(
        window_action(fixture.target(std::process::id()), WindowAction::Close),
        Err(WindowActionError::NotApplication)
    ));
    // Wrong PID: identity mismatch.
    assert!(matches!(
        window_action(fixture.target(u32::MAX), WindowAction::Minimize),
        Err(WindowActionError::Unavailable)
    ));
    // Destroyed after capture: stale identity.
    let stale = fixture.target(std::process::id() + 1);
    drop(fixture);
    assert!(matches!(
        window_action(stale, WindowAction::Close),
        Err(WindowActionError::Unavailable)
    ));
}
