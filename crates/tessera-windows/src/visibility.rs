// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Passive pointer capability. Construction never installs a hook or starts a
//! thread; only an accepted `watch` acquires the process-wide native owner.

use std::sync::Arc;
use tessera_system::visibility::{PointerHost, PointerWatchError};

/// Creates an inert host. All factory instances share one native watch gate.
/// Readiness confirms registration and one physical-coordinate startup seed,
/// not indefinite delivery: Windows can silently remove a timed-out LL hook.
pub fn native_pointer_host() -> Result<Arc<dyn PointerHost>, PointerWatchError> {
    #[cfg(windows)]
    {
        Ok(crate::native_visibility::host())
    }
    #[cfg(not(windows))]
    {
        Err(PointerWatchError::Unsupported)
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    #[test]
    fn unsupported_factory_never_fabricates_a_pointer_host() {
        assert!(matches!(
            super::native_pointer_host(),
            Err(tessera_system::visibility::PointerWatchError::Unsupported)
        ));
    }
}
