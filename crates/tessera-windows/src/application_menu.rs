// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Installed-application actions use only an injected, exact catalog lookup.

use std::sync::Arc;

use tessera_system::application_menu::ApplicationMenuHost;

use crate::Application;

pub(crate) type ApplicationLookup = dyn Fn(&str) -> Option<Application> + Send + Sync;

/// Creates an inert host. Its one native owner starts only on explicit inspect;
/// unsupported platforms have no provider. The lookup must return only exact,
/// currently trusted catalog records, never resolve arbitrary caller paths.
pub fn native_application_menu_host(
    lookup: Arc<ApplicationLookup>,
) -> Option<Arc<dyn ApplicationMenuHost>> {
    #[cfg(windows)]
    {
        Some(Arc::new(
            crate::native_application_menu::NativeApplicationMenuHost::new(lookup),
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = lookup;
        None
    }
}
