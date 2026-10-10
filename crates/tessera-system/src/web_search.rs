// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Deliberate fixed-provider search, never arbitrary URI dispatch or live results.
use std::fmt;

pub use crate::search::{
    SearchErrorKind as WebSearchErrorKind, SearchOutcome as WebSearchOutcome,
    SearchQuery as WebSearchQuery,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebSearchProvider {
    DuckDuckGo,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSearchRequest {
    pub provider: WebSearchProvider,
    pub query: WebSearchQuery,
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
