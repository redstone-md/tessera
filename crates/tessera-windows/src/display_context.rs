// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Independent native display reads and notifications. No application HWND is used.
use std::sync::Arc;
use tessera_system::display_context::{DisplayContextError, DisplayContextHost};

/// Construction does not enumerate displays, allocate windows, or initialize WinRT.
pub fn native_display_context_host() -> Result<Arc<dyn DisplayContextHost>, DisplayContextError> {
    #[cfg(windows)]
    {
        Ok(Arc::new(CompositeDisplayContextHost {
            read: crate::native_display_context::NativeDisplayContextHost::default(),
            watch: crate::native_display_watch::watch,
        }))
    }
    #[cfg(not(windows))]
    {
        Err(DisplayContextError::Unsupported)
    }
}

#[cfg(any(windows, test))]
type WatchFactory = fn(
    tessera_system::display_context::DisplayContextWatchCallback,
    tessera_system::display_context::DisplayContextWatchReady,
) -> Result<
    Box<dyn tessera_system::display_context::DisplayContextWatchGuard>,
    DisplayContextError,
>;

#[cfg(any(windows, test))]
pub(crate) struct CompositeDisplayContextHost<R> {
    pub(crate) read: R,
    pub(crate) watch: WatchFactory,
}

#[cfg(any(windows, test))]
impl<R: DisplayContextHost> DisplayContextHost for CompositeDisplayContextHost<R> {
    fn read(
        &self,
        completion: tessera_system::display_context::DisplayContextCompletion,
    ) -> Result<(), DisplayContextError> {
        self.read.read(completion)
    }

    fn watch(
        &self,
        on_event: tessera_system::display_context::DisplayContextWatchCallback,
        on_ready: tessera_system::display_context::DisplayContextWatchReady,
    ) -> Result<
        Box<dyn tessera_system::display_context::DisplayContextWatchGuard>,
        DisplayContextError,
    > {
        (self.watch)(on_event, on_ready)
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    #[test]
    fn unsupported_is_not_an_empty_observation() {
        assert!(matches!(
            super::native_display_context_host(),
            Err(tessera_system::display_context::DisplayContextError::Unsupported)
        ));
    }
}
