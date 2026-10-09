// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Only open/close calls. No values, writes, services, COM or last-error reads.
//!
//! <https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regopenkeyexw>
//! <https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regclosekey>

#![allow(unsafe_code)]

use std::marker::PhantomData;
use std::ptr::null_mut;
use std::rc::Rc;

use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
};

use super::{Calls, READ_ACCESS};

const _: () = assert!(READ_ACCESS == KEY_READ);

pub(super) struct WindowsCalls;

/// No independent Drop cleanup: the same Owner checks and consumes this key.
pub(super) struct Key {
    raw: HKEY,
    _thread_bound: PhantomData<Rc<()>>,
}

impl Calls for WindowsCalls {
    type Key = Key;

    fn open_hklm(&mut self, path: &str, options: u32, access: u32) -> Result<Key, u32> {
        let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let mut raw = null_mut();
        // SAFETY: The fixed nonempty path is NUL terminated and alive for the
        // call; output points to writable HKEY storage. The predefined HKLM is
        // never owned/closed. No WOW64 override changes the native process view.
        let status =
            unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), options, access, &mut raw) };
        if status == 0 {
            Ok(Key {
                raw,
                _thread_bound: PhantomData,
            })
        } else {
            // SDK windows-sys represents LSTATUS as WIN32_ERROR/u32; preserve
            // that full returned code, regardless of unrelated thread last-error.
            Err(status)
        }
    }

    fn close(&mut self, key: Key) -> Result<(), u32> {
        // SAFETY: Owner supplies only a successful open's real key, consumes
        // it once and never retries; it cannot be used after this call.
        let status = unsafe { RegCloseKey(key.raw) };
        if status == 0 { Ok(()) } else { Err(status) }
    }
}
