// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Optional current-user startup registration, never sign-in-shell replacement.

use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupRegistration {
    Absent,
    /// The provider observed its exact native registration. This does not prove
    /// that Windows will launch it, or that Task Manager enables the entry.
    Registered,
    /// Another installation, command, type, or malformed value owns the slot.
    /// Providers must not offer mutation authority for this state.
    Foreign,
}

/// An opaque, one-observation identity, never an executable or registry path.
#[derive(Clone, Debug)]
pub struct StartupTarget(Arc<()>);

impl StartupTarget {
    /// Mints an identity only. Providers must recognize their own currently
    /// issued identity; public construction does not grant native authority.
    pub fn new() -> Self {
        Self(Arc::new(()))
    }
}

impl Default for StartupTarget {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for StartupTarget {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for StartupTarget {}

/// `target` is present only for an observed, managed Absent or Registered state.
/// A provider may revoke an old target on any subsequent operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupSnapshot {
    pub registration: StartupRegistration,
    pub target: Option<StartupTarget>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupError {
    Busy,
    Unavailable,
    InvalidTarget,
    /// A mutation was attempted but its actual result cannot be confirmed.
    /// Do not roll back or automatically retry: explicitly read again instead.
    Unconfirmed,
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Busy => "A startup-registration operation is already in progress",
            Self::Unavailable => "Startup registration is unavailable",
            Self::InvalidTarget => "The startup-registration observation is no longer valid",
            Self::Unconfirmed => "The startup-registration change could not be confirmed",
        })
    }
}

impl std::error::Error for StartupError {}

pub type StartupCompletion =
    Box<dyn FnOnce(Result<StartupSnapshot, StartupError>) + Send + 'static>;

/// Nonblocking, single-flight read and observation-bound mutation. An immediate
/// error accepts no work and never invokes completion. Accepted work survives
/// UI teardown; callbacks may run synchronously and reenter a host. Successful
/// set returns an actual fresh readback, not an optimistic requested boolean.
/// Changes take effect immediately, independently of Settings Save or Cancel.
pub trait StartupHost: Send + Sync {
    fn read(&self, completion: StartupCompletion) -> Result<(), StartupError>;

    fn set(
        &self,
        target: StartupTarget,
        registered: bool,
        completion: StartupCompletion,
    ) -> Result<(), StartupError>;
}
