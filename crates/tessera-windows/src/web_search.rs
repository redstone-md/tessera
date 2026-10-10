// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::sync::Arc;
use tessera_system::web_search::WebSearchHost;

/// Inert admission only. Native initialization and URI dispatch happen solely
/// on a worker after deliberate input; unsupported platforms expose no host.
pub fn native_web_search_host() -> Option<Arc<dyn WebSearchHost>> {
    #[cfg(windows)]
    {
        Some(Arc::new(crate::native_web_search::NativeWebSearchHost))
    }
    #[cfg(not(windows))]
    {
        None
    }
}
