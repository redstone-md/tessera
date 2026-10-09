// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A side-effect-free factory; all profile work is explicit and owner-thread-only.

use std::sync::Arc;
use tessera_system::profile::{ProfileError, ProfileHost};

pub fn native_profile_host() -> Result<Arc<dyn ProfileHost>, ProfileError> {
    #[cfg(windows)]
    {
        Ok(actor::host(crate::native_profile::driver))
    }
    #[cfg(not(windows))]
    {
        Err(ProfileError::new(
            tessera_system::profile::ProfileErrorKind::Unsupported,
            "Current-user profiles are unavailable on this platform.",
            None,
        ))
    }
}

#[cfg(any(windows, test))]
mod actor;
#[cfg(any(windows, test))]
pub(crate) mod source;

#[cfg(test)]
mod tests;
