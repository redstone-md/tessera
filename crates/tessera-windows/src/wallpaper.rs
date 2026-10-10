// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Optional static-image wallpaper for the ordinary desktop process.

use std::sync::Arc;

use tessera_system::wallpaper::WallpaperHost;

/// Process-stable and inert: no SDK objects, native read or worker until choose.
/// Unsupported platforms have no provider. Native capability failures are
/// reported by operations, not hidden by an optimistic factory-time probe.
pub fn native_wallpaper_host() -> Option<Arc<dyn WallpaperHost>> {
    #[cfg(windows)]
    {
        use std::sync::LazyLock;

        static HOST: LazyLock<Arc<crate::native_wallpaper::NativeWallpaperHost>> =
            LazyLock::new(|| Arc::new(crate::native_wallpaper::NativeWallpaperHost::default()));
        Some(HOST.clone())
    }
    #[cfg(not(windows))]
    {
        None
    }
}
