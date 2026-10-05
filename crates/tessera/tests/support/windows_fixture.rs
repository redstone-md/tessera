#![allow(unsafe_code)]

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetWindowRect, IsWindowVisible, WS_EX_APPWINDOW,
    WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows_sys::core::w;

/// A thread-owned test window. The inspector runs in another process so it
/// exercises foreign-window enumeration and caption retrieval.
pub struct Fixture {
    hwnd: HWND,
}

impl Fixture {
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: HWND is thread-bound (not Send) and dropped by its owner.
        unsafe { DestroyWindow(self.hwnd) };
    }
}
