// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Observed enabled input profiles and fixed desktop-session activation.
//!
//! Profile identities are native-issued opaque values. Display text, locale
//! names and active flags are observations, never switching instructions.

use std::fmt;

/// Native-issued identity; callers must not parse its representation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProfileId(String);

impl ProfileId {
    /// Accept a bounded, nonempty opaque identity, not a label or native handle.
    pub fn new(value: impl Into<String>) -> Result<Self, InputLanguageError> {
        let value = value.into();
        if value.is_empty() || value.len() > 512 || value.contains('\0') {
            return Err(InputLanguageError::new(
                InputLanguageErrorKind::InvalidValue,
                "The input profile identity is invalid.",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputProfile {
    pub id: ProfileId,
    pub display_name: String,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputLanguage {
    pub id: String,
    /// Observed Windows locale name; empty means unavailable.
    pub code: String,
    pub name: String,
    pub native_name: String,
    pub profiles: Vec<InputProfile>,
}

/// Empty success and unavailable data are different results. Multiple active
/// flags are retained exactly as observed, not normalized by the presentation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputLanguageSnapshot {
    pub languages: Vec<InputLanguage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputLanguageAction {
    /// The adapter revalidates enabled exact identity before any effect. Scope
    /// is fixed to FORPROCESS | FORSESSION (the current Windows desktop).
    Activate { profile: ProfileId },
    /// Only the fixed `ms-settings:keyboard` target; no caller-provided URI.
    OpenKeyboardSettings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputLanguageOutcome {
    /// Fresh post-action observation, never an optimistic active flag.
    Snapshot(InputLanguageSnapshot),
    /// Windows accepted dispatch; this is not proof that Settings displayed.
    SettingsDispatched,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputLanguageErrorKind {
    Unsupported,
    AccessDenied,
    Busy,
    Stopped,
    ProfileChanged,
    Unavailable,
    InvalidValue,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputLanguageError {
    pub kind: InputLanguageErrorKind,
    pub message: String,
}

impl InputLanguageError {
    pub fn new(kind: InputLanguageErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for InputLanguageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for InputLanguageError {}

pub type InputLanguageReadCompletion =
    Box<dyn FnOnce(Result<InputLanguageSnapshot, InputLanguageError>) + Send + 'static>;
pub type InputLanguageActionCompletion =
    Box<dyn FnOnce(Result<InputLanguageOutcome, InputLanguageError>) + Send + 'static>;

/// Prompt acceptance: immediate `Err` calls no completion; accepted `Ok` calls
/// exactly one, possibly inline. Completions must be posted/generation-gated by
/// UI callers. Read has no switching or enabling effects. Dropping a caller
/// neither cancels an accepted action nor synchronously joins native work.
///
/// Activation can partially change language before failure. Errors must report
/// that truthfully; callers must not automatically retry or roll back. Successful
/// activation followed by failed confirmation is an error, not inferred state.
pub trait InputLanguageHost: Send + Sync + 'static {
    fn read(&self, completion: InputLanguageReadCompletion) -> Result<(), InputLanguageError>;
    fn execute(
        &self,
        action: InputLanguageAction,
        completion: InputLanguageActionCompletion,
    ) -> Result<(), InputLanguageError>;
}

#[cfg(test)]
mod tests;
