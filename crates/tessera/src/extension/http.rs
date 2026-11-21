// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Sends the HTTPS requests of extensions.

use std::time::Duration;

use ureq::Agent;
use ureq::http::Request;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

/// How long a request may take from start to the end of the body.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The most of a response body that is read.
const BODY_LIMIT: u64 = 1 << 20;

#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Sends `request` and waits for the response. Redirects are returned
/// rather than followed, so a permitted host cannot pass a request on to
/// another one. Servers are verified by Windows, against its
/// certificate store.
pub fn send(request: &HttpRequest) -> Result<HttpResponse, String> {
    let agent: Agent = Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(TIMEOUT))
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .user_agent("tessera")
        .build()
        .into();

    let mut builder = Request::builder()
        .method(request.method.as_str())
        .uri(request.url.as_str());
    for (name, value) in &request.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let response = match &request.body {
        Some(body) => builder
            .body(body.as_str())
            .map_err(describe)
            .and_then(|request| agent.run(request).map_err(describe)),
        None => builder
            .body(())
            .map_err(describe)
            .and_then(|request| agent.run(request).map_err(describe)),
    };
    let mut response = response?;
    let body = response
        .body_mut()
        .with_config()
        .limit(BODY_LIMIT)
        .read_to_string()
        .map_err(describe)?;
    Ok(HttpResponse {
        status: response.status().as_u16(),
        body,
    })
}

fn describe(error: impl std::fmt::Display) -> String {
    error.to_string()
}
