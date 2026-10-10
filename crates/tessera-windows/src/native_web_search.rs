// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use crate::single_flight::FlightGate;
use std::panic::{AssertUnwindSafe, catch_unwind};
use tessera_system::web_search::{
    WebSearchCompletion, WebSearchError, WebSearchErrorKind, WebSearchHost, WebSearchOutcome,
    WebSearchProvider, WebSearchRequest,
};
use windows::Foundation::Uri;
use windows::System::Launcher;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::HSTRING;

#[derive(Default)]
pub(crate) struct NativeWebSearchHost {
    gate: FlightGate,
}

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        // A successful RoInitialize, including S_FALSE, is balanced once.
        unsafe { RoUninitialize() };
    }
}

fn dispatch(request: WebSearchRequest) -> Result<WebSearchOutcome, WebSearchError> {
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
        .map_err(|_| error(WebSearchErrorKind::Unavailable))?;
    let _apartment = Apartment;
    // The maintained Windows URI implementation escapes the exact Unicode
    // component. No input is interpreted as a scheme, URL, shell or path.
    let escaped = Uri::EscapeComponent(&HSTRING::from(request.query.as_str()))
        .map_err(|_| error(WebSearchErrorKind::Unavailable))?;
    let uri = match request.provider {
        WebSearchProvider::DuckDuckGo => Uri::CreateUri(&HSTRING::from(format!(
            "https://duckduckgo.com/?q={escaped}"
        ))),
    }
    .map_err(|_| error(WebSearchErrorKind::Unavailable))?;
    let operation =
        Launcher::LaunchUriAsync(&uri).map_err(|_| error(WebSearchErrorKind::Unconfirmed))?;
    let accepted = operation
        .join()
        .map_err(|_| error(WebSearchErrorKind::Unconfirmed))?;
    Ok(if accepted {
        WebSearchOutcome::Accepted
    } else {
        WebSearchOutcome::Declined
    })
}

fn error(kind: WebSearchErrorKind) -> WebSearchError {
    WebSearchError { kind }
}

impl WebSearchHost for NativeWebSearchHost {
    fn search(
        &self,
        request: WebSearchRequest,
        completion: WebSearchCompletion,
    ) -> Result<(), WebSearchError> {
        let flight = self
            .gate
            .try_enter()
            .ok_or_else(|| error(WebSearchErrorKind::Busy))?;
        // One detached scoped worker, no queue or idle apartment. Captured work
        // survives host/source retirement; cleanup precedes callback/reentry.
        catch_unwind(AssertUnwindSafe(|| {
            std::thread::Builder::new()
                .name("tessera-web-search".into())
                .spawn(move || {
                    let result = catch_unwind(AssertUnwindSafe(|| dispatch(request)))
                        .unwrap_or_else(|_| Err(error(WebSearchErrorKind::Unconfirmed)));
                    drop(flight);
                    let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                })
                .map(|_| ())
        }))
        .unwrap_or_else(|_| Err(std::io::Error::other("Web search worker could not start")))
        .map_err(|_| error(WebSearchErrorKind::Unavailable))
    }
}
