// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Motion notification delivery, independent of desktop observation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use slint::ComponentHandle;

use crate::DesktopHost;

pub(crate) struct MotionSubscription {
    live: Arc<AtomicBool>,
    native: Option<Box<dyn Send>>,
}

impl MotionSubscription {
    pub(crate) fn new<C: ComponentHandle + 'static>(
        host: &dyn DesktopHost,
        target: &C,
        policy_changed: fn(&C, bool),
    ) -> Result<Self, String> {
        // Construct the fence first: a failed subscription must also suppress
        // anything its host already queued before returning the error.
        let mut subscription = Self {
            live: Arc::new(AtomicBool::new(true)),
            native: None,
        };
        let live = Arc::clone(&subscription.live);
        let target = target.as_weak();
        subscription.native = host.subscribe_ui_motion(Arc::new(move |enabled| {
            // Opting back in affects the next presentation, not the current
            // settled content: never resume/replay a skipped transition.
            if enabled || !live.load(Ordering::Acquire) {
                return;
            }
            let delivery = Arc::clone(&live);
            let _ = target.upgrade_in_event_loop(move |target| {
                if delivery.load(Ordering::Acquire) {
                    policy_changed(&target, false);
                }
            });
        }))?;
        Ok(subscription)
    }
}

impl Drop for MotionSubscription {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
        self.native.take();
    }
}
