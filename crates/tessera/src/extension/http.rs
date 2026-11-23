// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Sends the requests of extensions and downloads extensions.

use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use ureq::Agent;
use ureq::http::Request;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

/// How long a request may take from start to the end of the body.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The most of a response body that is read.
const BODY_LIMIT: u64 = 1 << 20;

/// How many redirects a download follows.
const DOWNLOAD_REDIRECTS: u32 = 5;

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
/// certificate store. A request over plain HTTP, which the permissions
/// leave to servers of the local network, is sent only if the name of its
/// server stands for addresses of such a network.
pub fn send(request: &HttpRequest) -> Result<HttpResponse, String> {
    let plain = request.url.strip_prefix("http://");
    if let Some(rest) = plain {
        let server = rest.split(['/', '?', '#']).next().unwrap_or_default();
        check_local(server)?;
    }
    let agent = agent(0, plain.is_some());

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

/// What is at the address `url`, if it is no longer than `limit`.
/// Redirects to other HTTPS addresses are followed.
pub fn download(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut response = agent(DOWNLOAD_REDIRECTS, false)
        .get(url)
        .call()
        .map_err(describe)?;
    if !response.status().is_success() {
        return Err(format!("the server answered {}", response.status()));
    }
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(describe)
}

/// Writes what is at the address `url` to `sink` as it arrives, if it is
/// no longer than `limit`, and says how long it was. Redirects to other
/// HTTPS addresses are followed.
pub fn download_into(url: &str, limit: u64, sink: &mut impl std::io::Write) -> Result<u64, String> {
    let mut response = agent(DOWNLOAD_REDIRECTS, false)
        .get(url)
        .call()
        .map_err(describe)?;
    if !response.status().is_success() {
        return Err(format!("the server answered {}", response.status()));
    }
    let mut body = response.body_mut().with_config().limit(limit).reader();
    std::io::copy(&mut body, sink).map_err(describe)
}

/// Fails unless `server`, a host and its port, stands for addresses of a
/// local network and for no other. The name is looked up again when the
/// request is sent, so a name server that answers differently the second
/// time is not guarded against.
fn check_local(server: &str) -> Result<(), String> {
    let addresses: Vec<IpAddr> = server
        .to_socket_addrs()
        .map_err(describe)?
        .map(|address| address.ip())
        .collect();
    if !addresses.is_empty() && addresses.iter().copied().all(is_local) {
        Ok(())
    } else {
        Err(format!("`{server}` is not on the local network"))
    }
}

/// Whether `address` is of this machine or of a private or link-local
/// network.
fn is_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_loopback() || address.is_private() || address.is_link_local()
        }
        IpAddr::V6(address) => {
            let first = address.segments()[0];
            address.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
    }
}

/// A client that follows up to `redirects` redirects and speaks HTTPS
/// only, unless `plain` allows HTTP as well.
fn agent(redirects: u32, plain: bool) -> Agent {
    Agent::config_builder()
        .https_only(!plain)
        .max_redirects(redirects)
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
        .into()
}

fn describe(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_addresses_of_local_networks_are_local() {
        let local = [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.20",
            "169.254.3.4",
            "::1",
            "fd00::1",
            "fe80::1",
        ];
        for address in local {
            assert!(is_local(address.parse().unwrap()), "{address}");
        }
        for address in ["8.8.8.8", "172.32.0.1", "2001:4860:4860::8888"] {
            assert!(!is_local(address.parse().unwrap()), "{address}");
        }
        assert!(check_local("127.0.0.1:8080").is_ok());
        assert!(check_local("8.8.8.8:80").is_err());
    }
}
