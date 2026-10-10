// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::sync::Arc;
use tessera_system::file_search::FileSearchHost;

/// Inert admission only, not proof of a configured/visible Windows Search UI.
/// SDK initialization and dispatch happen only after deliberate input on a worker.
pub fn native_file_search_host() -> Option<Arc<dyn FileSearchHost>> {
    #[cfg(windows)]
    {
        Some(Arc::new(NativeFileSearchHost))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
struct NativeFileSearchHost;

#[cfg(windows)]
impl FileSearchHost for NativeFileSearchHost {
    fn search(
        &self,
        request: tessera_system::file_search::FileSearchRequest,
        completion: tessera_system::file_search::FileSearchCompletion,
    ) -> Result<(), tessera_system::file_search::FileSearchError> {
        use crate::native_uri_dispatch::{self, Target};
        use tessera_system::file_search::FileSearchError;

        native_uri_dispatch::submit(Target::WindowsSearch, request.query, move |result| {
            completion(result.map_err(|kind| FileSearchError { kind }));
        })
        .map_err(|kind| FileSearchError { kind })
    }
}
