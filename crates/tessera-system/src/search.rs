// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Exact user text for deliberate fixed-target search dispatch, not live results.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchQuery(String);

impl SearchQuery {
    /// Preserve valid text verbatim; whitespace-only, controls and oversized
    /// inputs cannot become an effect. The bound includes UTF-16 surrogate pairs.
    pub fn new(text: &str) -> Option<Self> {
        if text.len() > 8192
            || text.trim().is_empty()
            || text.chars().any(char::is_control)
            || text.encode_utf16().count() > 2048
        {
            return None;
        }
        Some(Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchOutcome {
    /// SDK dispatch acceptance is not evidence that results are visible.
    Accepted,
    Declined,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchErrorKind {
    Busy,
    Unavailable,
    /// Dispatch was attempted, but its final acceptance could not be confirmed.
    Unconfirmed,
}
