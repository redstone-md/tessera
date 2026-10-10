// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Observed application-menu capabilities, distinct from application launch.

use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationAction {
    RunAsAdministrator,
    OpenFileLocation,
}

/// An opaque observation identity, not a path or an executable command.
/// Providers must recognize the exact identity they issued before dispatch.
#[derive(Clone, Debug)]
pub struct ApplicationActionTarget(Arc<ApplicationAction>);

impl ApplicationActionTarget {
    /// Mints a distinct identity for a provider observation. Construction alone
    /// grants no authority: native providers maintain their own issuer registry.
    pub fn new(action: ApplicationAction) -> Self {
        Self(Arc::new(action))
    }

    pub fn action(&self) -> ApplicationAction {
        *self.0
    }
}

impl PartialEq for ApplicationActionTarget {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ApplicationActionTarget {}

/// At most two targets, with no duplicate actions; the key echoes inspect's
/// exact catalog key and is never request authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationMenuSnapshot {
    pub application_key: String,
    pub targets: Vec<ApplicationActionTarget>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationActionOutcome {
    /// The native handler accepted dispatch, not proof of a visible window,
    /// completed launch, elevation, or Explorer selection.
    Accepted,
    /// The native handler explicitly reported user cancellation.
    Declined,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationMenuError {
    Busy,
    Unavailable,
    /// Dispatch was attempted, but its result cannot be confirmed.
    Unconfirmed,
    InvalidTarget,
}

impl std::fmt::Display for ApplicationMenuError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Busy => "An application-menu operation is already in progress",
            Self::Unavailable => "The application-menu capability is unavailable",
            Self::Unconfirmed => "The application action could not be confirmed",
            Self::InvalidTarget => "The application-menu target is no longer valid",
        })
    }
}

impl std::error::Error for ApplicationMenuError {}

pub type ApplicationMenuCompletion =
    Box<dyn FnOnce(Result<ApplicationMenuSnapshot, ApplicationMenuError>) + Send + 'static>;
pub type ApplicationActionCompletion =
    Box<dyn FnOnce(Result<ApplicationActionOutcome, ApplicationMenuError>) + Send + 'static>;

/// Nonblocking, single-flight inspection and identity-bound dispatch. An
/// immediate error accepts no work and does not invoke completion. Accepted
/// work completes independently of popup lifetime; callbacks may reenter.
pub trait ApplicationMenuHost: Send + Sync {
    fn inspect(
        &self,
        application_key: &str,
        completion: ApplicationMenuCompletion,
    ) -> Result<(), ApplicationMenuError>;

    fn request(
        &self,
        target: ApplicationActionTarget,
        completion: ApplicationActionCompletion,
    ) -> Result<(), ApplicationMenuError>;
}
