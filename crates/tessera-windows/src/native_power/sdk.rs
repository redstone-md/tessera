// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Shutdown::LockWorkStation;

use super::Calls;

pub(super) struct WindowsCalls;

impl Calls for WindowsCalls {
    fn lock_work_station(&self) -> i32 {
        // SAFETY: argument-free API, called exactly once on the explicit
        // request owner. A raw nonzero BOOL is only asynchronous initiation.
        unsafe { LockWorkStation() }
    }

    fn last_error(&self) -> u32 {
        // SAFETY: reads only this thread's last-error value. The driver calls
        // this immediately after a zero BOOL, never after a nonzero result.
        unsafe { GetLastError() }
    }
}
