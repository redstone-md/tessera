// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The global Windows wallpaper display policy, independent of selected images.
//! Native writes are immediate; Settings Save/Cancel cannot roll them back.

use std::sync::Arc;

use super::WallpaperError;

/// Closed native position values; unknown SDK values offer no write authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Center,
    Tile,
    Stretch,
    Fit,
    Fill,
    Span,
}

impl Position {
    pub const ALL: [Self; 6] = [
        Self::Center,
        Self::Tile,
        Self::Stretch,
        Self::Fit,
        Self::Fill,
        Self::Span,
    ];
}

/// Opaque identity of one actual native global-position observation.
/// Construction grants no authority: the issuing provider must recognize it.
/// This holds no image, file lease, monitor selection, or per-monitor scope.
#[derive(Clone)]
pub struct Target(Arc<()>);

impl Target {
    pub fn new() -> Self {
        Self(Arc::new(()))
    }
}

impl Default for Target {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for Target {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Target {}

impl std::fmt::Debug for Target {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WallpaperPositionTarget")
            .finish_non_exhaustive()
    }
}

/// Read-only actual SDK state paired with its issuer identity, not UI authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub target: Target,
    pub position: Position,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteDisposition {
    /// Fresh native state already matched; no native setter was called.
    AlreadyCurrent,
    /// The native setter succeeded, not proof of rendered wallpaper pixels.
    Accepted,
    /// The native setter returned an error; actual state may still have changed.
    Rejected,
}

/// Fresh readback is optional and independent of the setter's receipt.
/// A returned observation is newly issued, never a replay of the consumed target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteOutcome {
    pub disposition: WriteDisposition,
    pub observation: Option<Observation>,
}

pub type ReadCompletion = Box<dyn FnOnce(Result<Observation, WallpaperError>) + Send + 'static>;
pub type WriteCompletion = Box<dyn FnOnce(Result<WriteOutcome, WallpaperError>) + Send + 'static>;
