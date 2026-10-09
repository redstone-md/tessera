// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Independent, request-local native display reads. No application HWND is used.
use std::sync::Arc;
use tessera_system::display_context::{DisplayContextError, DisplayContextHost};

/// Construction does not enumerate displays, allocate windows, or initialize WinRT.
pub fn native_display_context_host() -> Result<Arc<dyn DisplayContextHost>, DisplayContextError> {
    #[cfg(windows)]
    {
        Ok(Arc::new(
            crate::native_display_context::NativeDisplayContextHost::default(),
        ))
    }
    #[cfg(not(windows))]
    {
        Err(DisplayContextError::Unsupported)
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
