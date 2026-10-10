// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use crate::native_uri_dispatch::{self, Target};
use tessera_system::web_search::{
    WebSearchCompletion, WebSearchError, WebSearchHost, WebSearchProvider, WebSearchRequest,
};

#[derive(Default)]
pub(crate) struct NativeWebSearchHost;

impl WebSearchHost for NativeWebSearchHost {
    fn search(
        &self,
        request: WebSearchRequest,
        completion: WebSearchCompletion,
    ) -> Result<(), WebSearchError> {
        let target = match request.provider {
            WebSearchProvider::DuckDuckGo => Target::DuckDuckGo,
        };
        native_uri_dispatch::submit(target, request.query, move |result| {
            completion(result.map_err(|kind| WebSearchError { kind }));
        })
        .map_err(|kind| WebSearchError { kind })
    }
}
