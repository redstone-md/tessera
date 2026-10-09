// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::NetworkManagement::WiFi::{L2_NOTIFICATION_DATA, WLAN_NOTIFICATION_SOURCE_ACM};

pub(super) struct Context {
    admitted: AtomicBool,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Context {
    pub(super) fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            admitted: AtomicBool::new(true),
            wake,
        }
    }

    pub(super) fn close(&self) {
        self.admitted.store(false, Ordering::Release);
    }

    fn notify(&self, source: u32, code: u32) {
        // ACM arrival/removal, connection/disconnection, cached scan-list and
        // relevant radio/profile invalidations. Nothing consumes pData.
        if source & WLAN_NOTIFICATION_SOURCE_ACM.0 != 0
            && matches!(
                code,
                1 | 2
                    | 5
                    | 6
                    | 7
                    | 8
                    | 10
                    | 11
                    | 12
                    | 13
                    | 14
                    | 15
                    | 16
                    | 18
                    | 19
                    | 20
                    | 21
                    | 22
                    | 23
                    | 25
                    | 26
                    | 27
            )
            && self.admitted.load(Ordering::Acquire)
        {
            // The actor's wake performs the single atomic dirty/coalesced bounded
            // mailbox admission. No reads, event delivery, locks or UI calls here.
            (self.wake)();
        }
    }
}

/// # Safety
/// Windows (or the recording seam) supplies a live Context and an initialized
/// notification header throughout this call. Retirement frees Context only after
/// successful SOURCE_NONE on the worker has waited for active callbacks.
pub(super) unsafe extern "system" fn notification(
    data: *mut L2_NOTIFICATION_DATA,
    context: *mut c_void,
) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if data.is_null() || context.is_null() {
            return;
        }
        // SAFETY: header/context lifetime is guaranteed by registration. Copy
        // only header scalars; never dereference, retain or even copy pData.
        let source = unsafe { std::ptr::addr_of!((*data).NotificationSource).read_unaligned() }.0;
        let code = unsafe { std::ptr::addr_of!((*data).NotificationCode).read_unaligned() };
        let context = unsafe { &*context.cast::<Context>() };
        context.notify(source, code);
    }));
    if let Err(payload) = result {
        // Even a custom panic payload may panic in Drop. The ABI must not unwind;
        // retain that exceptional payload rather than dropping it here.
        std::mem::forget(payload);
    }
}
