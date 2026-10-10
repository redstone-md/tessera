// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit native current-policy observations and one-monitor slideshow actions.
//! A monitor name, UI index or selected collection never grants this authority.

use super::collection::{NativeStep, OptionsReadback, StateFacts};
use super::{
    WallpaperError, WallpaperImageTarget, WallpaperImageTargetWeak, WallpaperMonitorSelection,
    WallpaperMonitorTarget,
};

/// Distinct identity of one issued actual policy/cohort observation.
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
            .debug_struct("WallpaperSlideshowTarget")
            .finish_non_exhaustive()
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

/// Actual SDK facts can be readable without a safe actionable policy snapshot.
/// Some(target) permits global options editing; advancement also requires that
/// observation's exact monitor member. Construction alone grants no authority.
/// Native entry count is array members: a single folder is not an image count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub target: Option<Target>,
    pub monitors: Vec<WallpaperMonitorSelection>,
    pub state: StateFacts,
    pub options: Option<OptionsReadback>,
    pub native_entry_count: Option<u32>,
}

/// Setter receipt, independent native wallpaper-file change and fresh policy.
/// Changed concerns native file identity/metadata, never desktop pixels/animation.
/// Fresh observations mint new targets. selected_monitor, if present, is an
/// exact newly issued member for the same SDK monitor, not a retained UI index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdvanceOutcome {
    pub receipt: NativeStep,
    pub changed: Option<bool>,
    pub observation: Option<Observation>,
    pub selected_monitor: Option<WallpaperMonitorTarget>,
}

/// AlreadyCurrent means the revalidated native options matched the proposal and
/// no setter was called. Accepted/Rejected are SDK receipts, not persistent state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionsDisposition {
    AlreadyCurrent,
    Accepted,
    Rejected,
}

/// One global options request without replacing/restarting the slideshow source.
/// Independent fresh observation mints new targets; monitor choice resets because
/// this global command does not carry a selected native monitor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionsOutcome {
    pub disposition: OptionsDisposition,
    pub observation: Option<Observation>,
}

pub type ReadCompletion = Box<dyn FnOnce(Result<Observation, WallpaperError>) + Send + 'static>;
pub type AdvanceCompletion =
    Box<dyn FnOnce(Result<AdvanceOutcome, WallpaperError>) + Send + 'static>;
pub type OptionsCompletion =
    Box<dyn FnOnce(Result<OptionsOutcome, WallpaperError>) + Send + 'static>;
