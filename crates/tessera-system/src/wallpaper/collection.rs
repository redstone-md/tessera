// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit file collections for the global Windows slideshow policy.
//! Windows owns persistence and scheduling; these are not preference drafts.

use super::{WallpaperError, WallpaperImageTarget, WallpaperImageTargetWeak, WallpaperPreview};

/// A distinct opaque collection capability backed by the existing retirement key.
/// Construction alone grants no authority; only its native issuer may accept it.
#[derive(Clone, PartialEq, Eq)]
pub struct Target(WallpaperImageTarget);

impl Target {
    pub fn new() -> Self {
        Self(WallpaperImageTarget::new())
    }

    pub fn with_retirement(retire: impl FnOnce() + Send + Sync + 'static) -> Self {
        Self(WallpaperImageTarget::with_retirement(retire))
    }

    pub fn downgrade(&self) -> TargetWeak {
        TargetWeak(self.0.downgrade())
    }
}

impl Default for Target {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Target {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WallpaperCollectionTarget")
            .finish_non_exhaustive()
    }
}

/// Identity/liveness only; it cannot retain a file or upgrade to authority.
#[derive(Clone, Debug)]
pub struct TargetWeak(WallpaperImageTargetWeak);

impl TargetWeak {
    pub fn is_alive(&self) -> bool {
        self.0.is_alive()
    }

    pub fn matches(&self, target: &Target) -> bool {
        self.0.matches(&target.0)
    }
}

/// Read-only selected-file presentation, never a path or member mutation ticket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub caption: String,
    pub preview: Option<WallpaperPreview>,
}

/// A native source, never a UI path or locally enumerated folder inventory.
/// Images contain 2..32 same-container files; a folder has no image count/preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Images(Vec<Item>),
    Folder { caption: String },
}

/// One explicitly selected source. Applying consumes its whole authority.
/// Windows enumerates folder contents; no monitor selector is implied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub target: Target,
    pub source: Source,
}

/// Supported proposed transition intervals, not a fabricated native observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interval {
    OneMinute,
    TenMinutes,
    ThirtyMinutes,
    OneHour,
    SixHours,
    OneDay,
}

impl Interval {
    pub const ALL: [Self; 6] = [
        Self::OneMinute,
        Self::TenMinutes,
        Self::ThirtyMinutes,
        Self::OneHour,
        Self::SixHours,
        Self::OneDay,
    ];

    pub const fn milliseconds(self) -> u32 {
        match self {
            Self::OneMinute => 60_000,
            Self::TenMinutes => 600_000,
            Self::ThirtyMinutes => 1_800_000,
            Self::OneHour => 3_600_000,
            Self::SixHours => 21_600_000,
            Self::OneDay => 86_400_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub interval: Interval,
    pub shuffle: bool,
}

/// Fresh native timing may differ from all supported proposed intervals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionsReadback {
    pub interval_ms: u32,
    pub shuffle: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeStep {
    NotSubmitted,
    Accepted,
    Rejected,
}

/// Actual SDK flags, not proof that rendered images are advancing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateFacts {
    pub enabled: bool,
    pub slideshow: bool,
    pub disabled_by_remote_session: bool,
}

/// Global SetSlideshow and SetSlideshowOptions are separate, non-atomic effects.
/// A rejected first step leaves options unsubmitted. After any native setter,
/// partial receipts are preserved rather than collapsed to an error or rollback.
/// Confirmation compares the native file-identity multiset or exact native folder
/// identity, never order, folder contents, image usability, current desktop pixels,
/// or durable future file identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub collection: NativeStep,
    pub options: NativeStep,
    pub collection_matches: Option<bool>,
    pub options_readback: Option<OptionsReadback>,
    pub state: Option<StateFacts>,
}

pub type ChooseCompletion =
    Box<dyn FnOnce(Result<Option<Selection>, WallpaperError>) + Send + 'static>;
pub type ApplyCompletion = Box<dyn FnOnce(Result<ApplyOutcome, WallpaperError>) + Send + 'static>;
