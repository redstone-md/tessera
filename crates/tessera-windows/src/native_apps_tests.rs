// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::ActivationTarget;
use tessera_core::WindowId;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, WS_OVERLAPPEDWINDOW,
};
use windows_sys::core::w;

struct Fixture(HWND);
impl Fixture {
    fn new() -> Self {
        // SAFETY: hidden built-in class, owned and destroyed on this test thread.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                w!("STATIC"),
                w!("Tessera icon fixture"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                64,
                64,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null());
        Self(hwnd)
    }
    fn target(&self) -> ActivationTarget {
        ActivationTarget::new(WindowId::new(self.0 as usize as u64), std::process::id())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: only our own hidden HWND on its creating thread.
        unsafe { DestroyWindow(self.0) };
    }
}

#[test]
fn window_icon_refuses_foreign_pid_and_stale_handles() {
    let fixture = Fixture::new();
    assert!(
        window_icon(ActivationTarget::new(
            WindowId::new(fixture.0 as usize as u64),
            u32::MAX
        ))
        .is_none()
    );
    let dead = fixture.target();
    drop(fixture);
    assert!(window_icon(dead).is_none());
    assert!(window_icon(ActivationTarget::new(WindowId::new(u64::MAX), 1)).is_none());
}

#[test]
fn parsing_names_are_rejected_not_truncated_or_lossily_rewritten() {
    let name: Vec<u16> = "hello".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: each test array is valid and NUL-terminated, with explicit bounds.
    unsafe {
        assert_eq!(
            bounded_string(name.as_ptr(), 5, false).as_deref(),
            Some("hello")
        );
        assert!(bounded_string(name.as_ptr(), 4, false).is_none());
        assert!(bounded_string([0xd800, 0].as_ptr(), 2, false).is_none());
    }
}

#[test]
fn dib_copy_is_rgba_and_keeps_requested_top_down_rows() {
    use windows_sys::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS};
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: 1,
            biHeight: -2,
            biPlanes: 1,
            biBitCount: 32,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: valid descriptor/out-parameter; the owned bitmap guards its allocation.
    let raw = unsafe {
        sys_CreateDIBSection(
            std::ptr::null_mut(),
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        )
    };
    assert!(!raw.is_null());
    let bitmap = OwnedBitmap(raw);
    assert!(!bits.is_null());
    let bgra = [0u8, 0, 255, 255, 255, 0, 0, 255];
    // SAFETY: the 1x2 32-bit DIB has exactly eight writable bytes and is not selected into a DC.
    unsafe { std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits.cast::<u8>(), bgra.len()) };
    let icon = bitmap_rgba(bitmap.0).expect("32-bit DIB copies with alpha");
    assert_eq!(icon.rgba(), &[255, 0, 0, 255, 0, 0, 255, 255]);
}
