// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native shortcuts for the source's readonly launcher and editable Settings.
//! This tracer does not implement the source's remaining shortcut matrix.

use std::sync::Arc;
use tessera_system::shortcuts::{ShortcutError, ShortcutHost};

/// Creates a lazy provider. Native registration starts only after configuration.
pub fn native_shortcuts_host() -> Result<Arc<dyn ShortcutHost>, ShortcutError> {
    #[cfg(windows)]
    {
        crate::native_shortcuts::create_host()
    }
    #[cfg(not(windows))]
    {
        Err(tessera_system::shortcuts::ShortcutError::new(
            tessera_system::shortcuts::ShortcutErrorKind::Unsupported,
            None,
            "Native shortcuts require Windows",
        ))
    }
}
