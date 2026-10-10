// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::PanelController;
use std::sync::Arc;
use tessera_system::file_search::{FileSearchHost, FileSearchRequest};
use tessera_system::search::{SearchOutcome, SearchQuery};
use tessera_system::web_search::{WebSearchHost, WebSearchProvider, WebSearchRequest};

pub(super) enum SearchAction {
    Web {
        request: WebSearchRequest,
        host: Arc<dyn WebSearchHost>,
    },
    Files {
        request: FileSearchRequest,
        host: Arc<dyn FileSearchHost>,
    },
}

impl SearchAction {
    pub(super) fn acquire(controller: &PanelController, raw: &str) -> Option<Self> {
        if let Some(text) = raw.strip_prefix("web:") {
            let query = SearchQuery::new(text)?;
            Some(Self::Web {
                request: WebSearchRequest {
                    provider: WebSearchProvider::DuckDuckGo,
                    query,
                },
                host: controller.core.host().web_search_host()?,
            })
        } else {
            let query = SearchQuery::new(raw.strip_prefix("files:")?)?;
            Some(Self::Files {
                request: FileSearchRequest { query },
                host: controller.core.host().file_search_host()?,
            })
        }
    }

    pub(super) fn host_current(&self, controller: &PanelController) -> bool {
        match self {
            Self::Web { host, .. } => controller
                .core
                .host()
                .web_search_host()
                .is_some_and(|current| Arc::ptr_eq(host, &current)),
            Self::Files { host, .. } => controller
                .core
                .host()
                .file_search_host()
                .is_some_and(|current| Arc::ptr_eq(host, &current)),
        }
    }

    pub(super) fn pending_message(&self) -> &'static str {
        match self {
            Self::Web { .. } => "Requesting browser dispatch…",
            Self::Files { .. } => "Requesting Windows Search dispatch…",
        }
    }

    pub(super) fn receipt(&self) -> fn(SearchOutcome) -> &'static str {
        match self {
            Self::Web { .. } => |outcome| match outcome {
                SearchOutcome::Accepted => {
                    "Browser dispatch accepted; page visibility is not confirmed."
                }
                SearchOutcome::Declined => "Browser dispatch declined.",
            },
            Self::Files { .. } => |outcome| match outcome {
                SearchOutcome::Accepted => {
                    "Windows Search dispatch accepted; results visibility is not confirmed."
                }
                SearchOutcome::Declined => "Windows Search dispatch declined.",
            },
        }
    }

    pub(super) fn dispatch(
        &self,
        completion: impl FnOnce(Result<SearchOutcome, String>) + Send + 'static,
    ) -> Result<(), String> {
        match self {
            Self::Web { request, host } => host
                .search(
                    request.clone(),
                    Box::new(move |result| {
                        completion(result.map_err(|error| error.to_string()));
                    }),
                )
                .map_err(|error| error.to_string()),
            Self::Files { request, host } => host
                .search(
                    request.clone(),
                    Box::new(move |result| {
                        completion(result.map_err(|error| error.to_string()));
                    }),
                )
                .map_err(|error| error.to_string()),
        }
    }
}
