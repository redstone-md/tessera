// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Optional startup registration for the ordinary desktop process only.

use std::sync::Arc;

use tessera_system::startup::StartupHost;

/// Returns the process-stable inert provider on Windows, and none elsewhere.
/// Callers expose it only in ordinary Desktop mode, without shell heartbeat.
/// The native owner independently rechecks its image and argument count before
/// every operation. No SDK access or worker starts until an explicit read.
pub fn native_startup_host() -> Option<Arc<dyn StartupHost>> {
    #[cfg(windows)]
    {
        use std::sync::LazyLock;

        static HOST: LazyLock<Arc<crate::native_startup::NativeStartupHost>> =
            LazyLock::new(|| Arc::new(crate::native_startup::NativeStartupHost::default()));
        Some(HOST.clone())
    }
    #[cfg(not(windows))]
    {
        None
    }
}
