// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Deliberate fixed-provider search, never arbitrary URI dispatch or live results.
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSearchQuery(String);

impl WebSearchQuery {
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
pub enum WebSearchProvider {
    DuckDuckGo,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSearchRequest {
    pub provider: WebSearchProvider,
    pub query: WebSearchQuery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebSearchOutcome {
    /// The SDK accepted dispatch, not evidence that a browser/page is visible.
    Accepted,
    Declined,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebSearchErrorKind {
    Busy,
    Unavailable,
    /// Dispatch was attempted, but its final acceptance could not be confirmed.
    Unconfirmed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSearchError {
    pub kind: WebSearchErrorKind,
}

impl fmt::Display for WebSearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind {
            WebSearchErrorKind::Busy => "A web search request is pending",
            WebSearchErrorKind::Unavailable => "Web search is unavailable",
            WebSearchErrorKind::Unconfirmed => "Browser dispatch could not be confirmed",
        })
    }
}

impl std::error::Error for WebSearchError {}

pub type WebSearchCompletion =
    Box<dyn FnOnce(Result<WebSearchOutcome, WebSearchError>) + Send + 'static>;

/// Immediate Ok admits one captured request and exactly one completion (possibly
/// inline); it is NOT native launch acceptance. Err admits no callback. One
/// bounded flight, no retries, requests/suggestions/prefetch or idle observation.
/// Accepted work survives host/UI retirement; drop never joins the UI thread.
pub trait WebSearchHost: Send + Sync + 'static {
    fn search(
        &self,
        request: WebSearchRequest,
        completion: WebSearchCompletion,
    ) -> Result<(), WebSearchError>;
}
