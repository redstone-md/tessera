// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit dock utility requests, without desktop state or window authority.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockUtilityErrorKind {
    Unsupported,
    AccessDenied,
    Unavailable,
    Busy,
    Stopped,
    Other,
}

#[derive(Clone, Debug)]
pub struct DockUtilityError {
    pub kind: DockUtilityErrorKind,
    pub message: String,
}

impl DockUtilityError {
    pub fn new(kind: DockUtilityErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for DockUtilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DockUtilityError {}

pub type DockUtilityCompletion = Box<dyn FnOnce(Result<(), DockUtilityError>) + Send + 'static>;

/// Prompt, nonblocking acceptance of an explicit Shell-owned desktop toggle.
///
/// `Ok` guarantees exactly one completion, inline or on a worker; immediate
/// `Err` guarantees none. Success acknowledges the native call, not the current
/// visibility of windows. No desired state or automatic retry is implied.
pub trait DockUtilitiesHost: Send + Sync + 'static {
    fn toggle_desktop(&self, completion: DockUtilityCompletion) -> Result<(), DockUtilityError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dock_utility_error_preserves_kind_and_displays_message() {
        let error = DockUtilityError::new(DockUtilityErrorKind::Busy, "pending");
        assert_eq!(error.clone().kind, DockUtilityErrorKind::Busy);
        assert_eq!(error.to_string(), "pending");
        let _: &dyn std::error::Error = &error;
    }
}
