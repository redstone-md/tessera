// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit Windows Search handoff only; no in-launcher file inventory.
use std::fmt;

pub use crate::search::{
    SearchErrorKind as FileSearchErrorKind, SearchOutcome as FileSearchOutcome,
    SearchQuery as FileSearchQuery,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileSearchRequest {
    pub query: FileSearchQuery,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileSearchError {
    pub kind: FileSearchErrorKind,
}

impl fmt::Display for FileSearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind {
            FileSearchErrorKind::Busy => "A search dispatch is pending",
            FileSearchErrorKind::Unavailable => "Windows Search dispatch is unavailable",
            FileSearchErrorKind::Unconfirmed => "Windows Search dispatch could not be confirmed",
        })
    }
}

impl std::error::Error for FileSearchError {}

pub type FileSearchCompletion =
    Box<dyn FnOnce(Result<FileSearchOutcome, FileSearchError>) + Send + 'static>;

/// Immediate Ok admits one captured request and exactly one completion (possibly
/// inline), not native acceptance or visible results. Err admits no callback.
/// No live queries, enumeration, suggestions, prefetch, retries or idle observation.
/// Accepted work survives source/host retirement; drop never joins the UI thread.
pub trait FileSearchHost: Send + Sync + 'static {
    fn search(
        &self,
        request: FileSearchRequest,
        completion: FileSearchCompletion,
    ) -> Result<(), FileSearchError>;
}
