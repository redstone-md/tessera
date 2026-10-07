// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

#![allow(unsafe_code)]

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
    GetDpiForWindow, GetThreadDpiAwarenessContext, SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, GetWindowRect,
    IsWindowVisible, MoveWindow, WS_EX_APPWINDOW, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
};
use windows_sys::core::w;

/// A thread-owned test window. The inspector runs in another process so it
/// exercises foreign-window enumeration and caption retrieval.
pub struct Fixture {
    hwnd: HWND,
}

impl Fixture {
    /// A bar-sized pop-up window carrying the exact toolbar caption the
    /// surface check matches on. Existing tests keep using [`Self::new`].
    pub fn toolbar(title: &str) -> Self {
        let caption: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        // SAFETY: STATIC is a built-in class; both strings are NUL-terminated
        // and live through the call. The returned window belongs to this thread.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_APPWINDOW,
                w!("STATIC"),
                caption.as_ptr(),
                WS_POPUP | WS_VISIBLE,
                0,
                0,
                1,
                1,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            )
        };
        assert!(
            !hwnd.is_null(),
            "CreateWindowExW(toolbar): {}",
            std::io::Error::last_os_error()
        );
        Self { hwnd }
    }

    pub fn new(title: &str) -> Self {
        let caption: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        // SAFETY: STATIC is a built-in class; both strings are NUL-terminated
        // and live through the call. The returned window belongs to this thread.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_APPWINDOW,
                w!("STATIC"),
                caption.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                40,
                40,
                400,
                240,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            )
        };
        assert!(
            !hwnd.is_null(),
            "CreateWindowExW: {}",
            std::io::Error::last_os_error()
        );
        Self { hwnd }
    }

    pub fn id(&self) -> u64 {
        self.hwnd as usize as u64
    }

    pub fn bounds(&self) -> (i32, i32, i32, i32) {
        let mut rect = RECT::default();
        // SAFETY: this fixture owns a live HWND and rect is writable output.
        assert_ne!(unsafe { GetWindowRect(self.hwnd, &mut rect) }, 0);
        (rect.left, rect.top, rect.right, rect.bottom)
    }

    pub fn visible(&self) -> bool {
        // SAFETY: the fixture still owns the window; this query is read-only.
        unsafe { IsWindowVisible(self.hwnd) != 0 }
    }

    /// Captured style words; the surface test asserts they stay unchanged
    /// across an inspection run. `GetWindowLongPtrW` returns `isize`.
    pub fn styles(&self) -> (isize, isize) {
        // SAFETY: the fixture owns a live window; style queries are read-only.
        unsafe {
            (
                GetWindowLongPtrW(self.hwnd, GWL_STYLE),
                GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE),
            )
        }
    }

    /// `GetDpiForWindow` for this fixture window (0 on failure).
    pub fn dpi(&self) -> u32 {
        // SAFETY: the fixture owns a live window; this query is read-only.
        unsafe { GetDpiForWindow(self.hwnd) }
    }

    /// Fixture-only resize/move to exact physical geometry.
    pub fn set_rect(&self, x: i32, y: i32, width: i32, height: i32) {
        // SAFETY: the fixture owns a live window; deliberate test geometry.
        assert_ne!(unsafe { MoveWindow(self.hwnd, x, y, width, height, 1) }, 0);
    }

    /// The fixture window's own monitor bounds (physical pixels).
    pub fn monitor_bounds(&self) -> (i32, i32, u32, u32) {
        // SAFETY: live owned handle; MONITORINFO is a correctly sized buffer.
        let monitor = unsafe { MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        assert_ne!(
            unsafe { GetMonitorInfoW(monitor, (&mut info as *mut MONITORINFO).cast()) },
            0
        );
        (
            info.rcMonitor.left,
            info.rcMonitor.top,
            (info.rcMonitor.right - info.rcMonitor.left) as u32,
            (info.rcMonitor.bottom - info.rcMonitor.top) as u32,
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: HWND is thread-bound (not Send) and dropped by its owner.
        unsafe { DestroyWindow(self.hwnd) };
    }
}

/// Thread-bound physical-DPI scope for owned fixtures; never process-wide.
/// Raw opaque tokens also prevent sending this guard to another thread.
pub struct ScopedDpiContext {
    previous: DPI_AWARENESS_CONTEXT,
    current: DPI_AWARENESS_CONTEXT,
}

impl ScopedDpiContext {
    pub fn enter() -> Self {
        Self::set_context(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
    }

    pub fn system_aware() -> Self {
        Self::set_context(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE)
    }

    fn set_context(context: DPI_AWARENESS_CONTEXT) -> Self {
        // SAFETY: only supported predefined contexts reach this constructor;
        // its previous opaque token is restored on this same thread.
        let previous = unsafe { SetThreadDpiAwarenessContext(context) };
        assert!(!previous.is_null());
        // SAFETY: read-only capture on the current fixture thread.
        let current = unsafe { GetThreadDpiAwarenessContext() };
        Self { previous, current }
    }

    pub fn assert_current_unchanged(&self) {
        // SAFETY: read-only comparison of valid opaque context tokens.
        assert_ne!(
            unsafe { AreDpiAwarenessContextsEqual(self.current, GetThreadDpiAwarenessContext()) },
            0,
            "observation must restore the caller's DPI awareness"
        );
    }
}

impl Drop for ScopedDpiContext {
    fn drop(&mut self) {
        // SAFETY: this non-Send guard restores its captured same-thread token.
        unsafe { SetThreadDpiAwarenessContext(self.previous) };
    }
}
