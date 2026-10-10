// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Optional immediate native images, collections and global policy, not saved preferences.

use std::sync::{Arc, Weak};

pub mod collection;
pub mod position;
pub mod slideshow;

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

/// Bounded, read-only native thumbnail pixels, not file or effect authority.
/// This does not establish what Windows rendered as the desktop wallpaper.
#[derive(Clone, PartialEq, Eq)]
pub struct WallpaperPreview {
    width: u32,
    height: u32,
    rgba: Arc<[u8]>,
}

impl WallpaperPreview {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, WallpaperError> {
        if !(1..=128).contains(&width) || !(1..=128).contains(&height) {
            return Err(WallpaperError::Unavailable);
        }
        let bytes = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .and_then(|bytes| usize::try_from(bytes).ok())
            .ok_or(WallpaperError::Unavailable)?;
        if rgba.len() != bytes {
            return Err(WallpaperError::Unavailable);
        }
        let mut visible = false;
        for pixel in rgba.chunks_exact(4) {
            let alpha = pixel[3];
            if pixel[..3].iter().any(|channel| *channel > alpha) {
                return Err(WallpaperError::Unavailable);
            }
            visible |= alpha != 0;
        }
        if !visible {
            return Err(WallpaperError::Unavailable);
        }
        Ok(Self {
            width,
            height,
            rgba: rgba.into(),
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Canonical premultiplied RGBA, exactly `width * height * 4` bytes.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

impl std::fmt::Debug for WallpaperPreview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WallpaperPreview")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rgba_len", &self.rgba.len())
            .finish()
    }
}

/// Read-only native display metadata; `caption` grants no file authority.
/// The optional actual native thumbnail is not rendered-wallpaper proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallpaperSelection {
    pub target: WallpaperImageTarget,
    pub caption: String,
    /// Between one and 32 actual native displays captured with this image.
    pub monitors: Vec<WallpaperMonitorSelection>,
    pub preview: Option<WallpaperPreview>,
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

    /// Explicit native read of the global Windows position policy. No idle read.
    /// This does not consume or replace a selected image.
    fn read_position(&self, _completion: position::ReadCompletion) -> Result<(), WallpaperError> {
        Err(WallpaperError::Unavailable)
    }

    /// Consume an exact issued position observation, revalidate its original
    /// native state, and request a global policy change with fresh SDK readback.
    /// Changes outside this provider can race the non-atomic SDK read/set pair.
    /// This never uses a monitor selector or consumes an image selection.
    fn set_position(
        &self,
        _target: position::Target,
        _position: position::Position,
        _completion: position::WriteCompletion,
    ) -> Result<(), WallpaperError> {
        Err(WallpaperError::Unavailable)
    }

    /// Native multi-select of 2..32 protected same-container image files.
    /// A newly accepted static/collection choose revokes the previous file
    /// selection of either kind, but not the global position observation.
    fn choose_collection(
        &self,
        _completion: collection::ChooseCompletion,
    ) -> Result<(), WallpaperError> {
        Err(WallpaperError::Unavailable)
    }

    /// Consume the native group's exact target and request a GLOBAL Windows
    /// slideshow, then its proposed interval/shuffle, with independent readback.
    /// This does not use the static-image monitor selector. Windows owns future
    /// persistence/transitions; SDK steps can partially succeed, without rollback.
    fn apply_collection(
        &self,
        _target: collection::Target,
        _options: collection::Options,
        _completion: collection::ApplyCompletion,
    ) -> Result<(), WallpaperError> {
        Err(WallpaperError::Unavailable)
    }

    /// Explicit native filesystem-folder picker for one Windows slideshow source.
    /// No local child enumeration, synthetic image inventory or UI path authority.
    fn choose_folder(
        &self,
        _completion: collection::ChooseCompletion,
    ) -> Result<(), WallpaperError> {
        Err(WallpaperError::Unavailable)
    }

    /// Explicit SDK read of current slideshow policy/status and actual monitors.
    /// This observes OS state, not the user's unsubmitted file-selection draft.
    /// Readable but disabled/unsupported policy grants no advancement target.
    fn read_slideshow(&self, _completion: slideshow::ReadCompletion) -> Result<(), WallpaperError> {
        Err(WallpaperError::Unavailable)
    }

    /// Consume one exact issued current-policy observation and advance only its
    /// exact native monitor member. Revalidate original policy/cohort before SDK.
    /// No null, next-scheduled, primary, caption or UI-index monitor fallback.
    /// SDK state can race the checked read/advance pair; readback is not pixels.
    fn advance_slideshow(
        &self,
        _target: slideshow::Target,
        _monitor: WallpaperMonitorTarget,
        _direction: slideshow::Direction,
        _completion: slideshow::AdvanceCompletion,
    ) -> Result<(), WallpaperError> {
        Err(WallpaperError::Unavailable)
    }

    /// Consume the exact current-policy target, revalidate original source,
    /// status/options and complete native cohort, then change only global timing
    /// and shuffle. This never invokes SetSlideshow or replaces file drafts.
    fn set_slideshow_options(
        &self,
        _target: slideshow::Target,
        _options: collection::Options,
        _completion: slideshow::OptionsCompletion,
    ) -> Result<(), WallpaperError> {
        Err(WallpaperError::Unavailable)
    }
}
