// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Battery-only lazy actor. Construction never reads or subscribes to Windows.

use std::sync::Arc;
use tessera_system::battery::{BatteryError, BatteryHost};

#[cfg(windows)]
#[path = "battery/worker.rs"]
mod worker;

/// Cache this one facade in the desktop host; toolbar and popup share its owner.
pub fn native_battery_host() -> Result<Arc<dyn BatteryHost>, BatteryError> {
    #[cfg(windows)]
    {
        Ok(Arc::new(worker::LazyHost::default()))
    }
    #[cfg(not(windows))]
    {
        Err(BatteryError::Unsupported)
    }
}
