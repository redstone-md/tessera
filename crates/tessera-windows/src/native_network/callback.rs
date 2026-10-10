// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::NetworkManagement::WiFi::{
    L2_NOTIFICATION_DATA, WLAN_CONNECTION_NOTIFICATION_DATA, WLAN_NOTIFICATION_SOURCE_ACM,
};
use windows::core::GUID;

static NEXT_CONTEXT: AtomicU64 = AtomicU64::new(1);

pub(super) struct Context {
    admitted: AtomicBool,
    wake: Arc<dyn Fn() + Send + Sync>,
    revision: AtomicU64,
    incarnation: u64,
    effect: Mutex<Option<EffectNotice>>,
}

struct EffectNotice {
    interface: GUID,
    ssid: Vec<u8>,
    reason: Option<u32>,
}

impl Context {
    pub(super) fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            admitted: AtomicBool::new(true),
            wake,
            revision: AtomicU64::new(1),
            // MAX is a permanent global exhaustion sentinel, never an identity.
            incarnation: NEXT_CONTEXT
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                    (next != 0 && next < u64::MAX).then(|| next + 1)
                })
                .unwrap_or(0),
            effect: Mutex::new(None),
        }
    }

    pub(super) fn close(&self) {
        self.admitted.store(false, Ordering::Release);
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub(super) fn incarnation(&self) -> u64 {
        self.incarnation
    }

    pub(super) fn is_admitted(&self) -> bool {
        self.admitted.load(Ordering::Acquire)
    }

    pub(super) fn has_authority(&self) -> bool {
        let revision = self.revision();
        self.is_admitted()
            && self.incarnation != 0
            && self.incarnation != u64::MAX
            && revision != 0
            && revision != u64::MAX
    }

    pub(super) fn observe_effect(&self, interface: GUID, ssid: &[u8]) {
        *self
            .effect
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(EffectNotice {
            interface,
            ssid: ssid.to_vec(),
            reason: None,
        });
    }

    pub(super) fn failure_reason(&self) -> Option<u32> {
        self.effect
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .and_then(|effect| effect.reason)
    }

    pub(super) fn retire_effect(&self) {
        self.effect
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
    }

    fn connection_failure(&self, data: &L2_NOTIFICATION_DATA) {
        // Only ACM connection_complete carries the terminal documented reason;
        // connection_attempt_fail may still be followed by native attempts.
        if data.NotificationSource.0 & WLAN_NOTIFICATION_SOURCE_ACM.0 == 0
            || data.NotificationCode != 10
            || data.pData.is_null()
            || (data.dwDataSize as usize) < size_of::<WLAN_CONNECTION_NOTIFICATION_DATA>()
            || data.dwDataSize > 65536
            || !self.is_admitted()
        {
            return;
        }
        // Legacy observation callbacks do not inspect payloads at all. An effect
        // enables only a bounded reason record for its exact interface.
        let Ok(mut slot) = self.effect.try_lock() else {
            return;
        };
        let Some(effect) = slot.as_mut() else { return };
        if effect.interface != data.InterfaceGuid {
            return;
        }
        // SAFETY: Windows owns the initialized, size-checked callback record.
        // Only fixed fields are copied; trailing profile XML is never inspected.
        let notice = unsafe {
            data.pData
                .cast::<WLAN_CONNECTION_NOTIFICATION_DATA>()
                .read_unaligned()
        };
        let length = notice.dot11Ssid.uSSIDLength as usize;
        if length > 32 {
            return;
        }
        if effect.ssid == notice.dot11Ssid.ucSSID[..length] && notice.wlanReasonCode != 0 {
            effect.reason = Some(notice.wlanReasonCode);
        }
    }

    fn notify(&self, source: u32, code: u32) {
        // ACM arrival/removal, connection/disconnection, cached scan-list and
        // relevant radio/profile invalidations retire unsubmitted authority.
        if source & WLAN_NOTIFICATION_SOURCE_ACM.0 != 0
            && matches!(
                code,
                1 | 2
                    | 5
                    | 9
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
            // Exhaustion invalidates effect authority forever but must not stop
            // legacy cache-change wakeups or wrap to an earlier revision.
            let _ = self
                .revision
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |revision| {
                    Some(if revision == 0 {
                        u64::MAX
                    } else {
                        revision.saturating_add(1)
                    })
                });
            // One coalesced actor wake; no native reads or UI calls here.
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
        // SAFETY: the registration owns this initialized header for the call.
        let data = unsafe { data.read_unaligned() };
        let context = unsafe { &*context.cast::<Context>() };
        context.connection_failure(&data);
        context.notify(data.NotificationSource.0, data.NotificationCode);
    }));
    if let Err(payload) = result {
        // Even a custom panic payload may panic in Drop. The ABI must not unwind;
        // retain that exceptional payload rather than dropping it here.
        std::mem::forget(payload);
    }
}
