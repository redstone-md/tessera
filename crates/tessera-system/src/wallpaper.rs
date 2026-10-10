// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Optional, immediate static-image wallpaper operations, not saved preferences.

use std::sync::{Arc, Weak};

/// An opaque selection identity, never a filesystem path or monitor identifier.
#[derive(Clone)]
pub struct WallpaperImageTarget {
    // Field order matters: identity is gone before the last-drop notification.
    identity: Arc<()>,
    _retirement: Option<Arc<TargetRetirement>>,
}

impl WallpaperImageTarget {
    /// Mints identity only. Native authority requires the provider's exact,
    /// currently issued selection and its retained native resources.
    pub fn new() -> Self {
        Self {
            identity: Arc::new(()),
            _retirement: None,
        }
    }

    /// Adds a final-ticket-drop notification, not native mutation authority.
    /// Providers use it only to enqueue resource cleanup on their native owner.
    pub fn with_retirement(retired: impl FnOnce() + Send + Sync + 'static) -> Self {
        Self {
            identity: Arc::new(()),
            _retirement: Some(Arc::new(TargetRetirement(Some(Box::new(retired))))),
        }
    }

    /// Native registries must not keep the user's file lease alive themselves.
    pub fn downgrade(&self) -> WallpaperImageTargetWeak {
        WallpaperImageTargetWeak(Arc::downgrade(&self.identity))
    }
}

impl Default for WallpaperImageTarget {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for WallpaperImageTarget {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity)
    }
}

impl Eq for WallpaperImageTarget {}

impl std::fmt::Debug for WallpaperImageTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("WallpaperImageTarget")
            .field(&self.identity)
            .finish()
    }
}

/// Identity/liveness only, with no upgrade or native authority.
#[derive(Clone, Debug)]
pub struct WallpaperImageTargetWeak(Weak<()>);

impl WallpaperImageTargetWeak {
    /// A momentary cleanup hint, never permission to mutate a native resource.
    pub fn is_alive(&self) -> bool {
        self.0.strong_count() != 0
    }

    pub fn matches(&self, target: &WallpaperImageTarget) -> bool {
        self.0.ptr_eq(&Arc::downgrade(&target.identity))
    }
}

struct TargetRetirement(Option<Box<dyn FnOnce() + Send + Sync + 'static>>);

impl Drop for TargetRetirement {
    fn drop(&mut self) {
        if let Some(retired) = self.0.take() {
            // A notification must not turn ordinary UI teardown into a panic.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(retired));
        }
    }
}

/// One opaque captured-monitor identity, never a device path or UI index.
/// Construction alone grants no authority; providers recognize only tickets
/// issued for their exact current image selection.
#[derive(Clone, Debug)]
pub struct WallpaperMonitorTarget(Arc<()>);

impl WallpaperMonitorTarget {
    pub fn new() -> Self {
        Self(Arc::new(()))
    }
}

impl Default for WallpaperMonitorTarget {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for WallpaperMonitorTarget {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for WallpaperMonitorTarget {}

/// Read-only native descriptor metadata, not monitor mutation authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallpaperMonitorSelection {
    pub target: WallpaperMonitorTarget,
    pub caption: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WallpaperApplyScope {
    /// Exactly the original captured cohort, never a native global shortcut.
    AllCaptured,
    Monitor(WallpaperMonitorTarget),
}

/// Read-only native display metadata; `caption` grants no file authority.
/// Selection does not promise a decoded image or a rendered thumbnail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallpaperSelection {
    pub target: WallpaperImageTarget,
    pub caption: String,
    /// Between one and 32 actual native displays captured with this image.
    pub monitors: Vec<WallpaperMonitorSelection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallpaperError {
    Busy,
    Unavailable,
    InvalidTarget,
}

impl std::fmt::Display for WallpaperError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Busy => "A wallpaper operation is already in progress",
            Self::Unavailable => "Static wallpaper is unavailable",
            Self::InvalidTarget => "The wallpaper selection is no longer valid",
        })
    }
}

impl std::error::Error for WallpaperError {}

/// Results for the requested subset of the originally captured native monitors.
/// `requested = accepted + failed + not_submitted`; `confirmed <= accepted`.
/// Accepted counts successful native writes. Confirmed counts fresh native
/// wallpaper paths identifying the selected file, never rendered pixels.
/// `accepted - confirmed` is unconfirmed; neither retry nor rollback is implied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WallpaperApplyOutcome {
    pub requested: u32,
    pub accepted: u32,
    pub confirmed: u32,
    pub failed: u32,
    pub not_submitted: u32,
}

pub type WallpaperChooseCompletion =
    Box<dyn FnOnce(Result<Option<WallpaperSelection>, WallpaperError>) + Send + 'static>;
pub type WallpaperApplyCompletion =
    Box<dyn FnOnce(Result<WallpaperApplyOutcome, WallpaperError>) + Send + 'static>;

/// Nonblocking, single-flight native selection and selection-bound application.
/// Immediate errors accept no work and never invoke completion; accepted work
/// calls it exactly once, independently of UI lifetime. Callbacks may be
/// synchronous and reenter the host. `Ok(None)` means actual user cancellation.
/// A subsequent accepted choose revokes the old selection; apply consumes it.
/// Either scope consumes the image; a monitor ticket cannot replay that image
/// or keep its protected file lease alive independently.
/// Wallpaper changes take effect immediately, outside Settings Save/Cancel.
pub trait WallpaperHost: Send + Sync {
    fn choose(&self, completion: WallpaperChooseCompletion) -> Result<(), WallpaperError>;

    fn apply(
        &self,
        target: WallpaperImageTarget,
        scope: WallpaperApplyScope,
        completion: WallpaperApplyCompletion,
    ) -> Result<(), WallpaperError>;
}
