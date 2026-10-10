// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use crate::single_flight::FlightGate;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::LazyLock;
use tessera_system::search::{SearchErrorKind, SearchOutcome, SearchQuery};
use windows::Foundation::Uri;
use windows::System::Launcher;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::HSTRING;

// All fixed URI targets and all native host instances share one bounded flight.
static GATE: LazyLock<FlightGate> = LazyLock::new(FlightGate::default);

pub(crate) enum Target {
    DuckDuckGo,
    WindowsSearch,
}

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        // A successful RoInitialize, including S_FALSE, is balanced once.
        unsafe { RoUninitialize() };
    }
}

fn dispatch(target: Target, query: SearchQuery) -> Result<SearchOutcome, SearchErrorKind> {
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|_| SearchErrorKind::Unavailable)?;
    let _apartment = Apartment;
    // Exact Unicode text is one escaped component, never a URI/path/shell input.
    let escaped = Uri::EscapeComponent(&HSTRING::from(query.as_str()))
        .map_err(|_| SearchErrorKind::Unavailable)?;
    let address = match target {
        Target::DuckDuckGo => format!("https://duckduckgo.com/?q={escaped}"),
        // Documented fixed query only (AQS default); never crumb/location/subquery.
        // https://learn.microsoft.com/en-us/windows/win32/search/getting-started-with-parameter-value-arguments
        // https://learn.microsoft.com/en-us/windows/win32/search/-search-3x-wds-qryidx-searchms
        Target::WindowsSearch => format!("search-ms:query={escaped}&"),
    };
    let uri = Uri::CreateUri(&HSTRING::from(address)).map_err(|_| SearchErrorKind::Unavailable)?;
    let operation = Launcher::LaunchUriAsync(&uri).map_err(|_| SearchErrorKind::Unconfirmed)?;
    let accepted = operation.join().map_err(|_| SearchErrorKind::Unconfirmed)?;
    Ok(if accepted {
        SearchOutcome::Accepted
    } else {
        SearchOutcome::Declined
    })
}

pub(crate) fn submit(
    target: Target,
    query: SearchQuery,
    completion: impl FnOnce(Result<SearchOutcome, SearchErrorKind>) + Send + 'static,
) -> Result<(), SearchErrorKind> {
    let flight = GATE.try_enter().ok_or(SearchErrorKind::Busy)?;
    // One detached worker, no queue or idle apartment. All SDK objects and the
    // apartment are cleaned up before gate release and completion/reentry.
    catch_unwind(AssertUnwindSafe(|| {
        std::thread::Builder::new()
            .name("tessera-search-dispatch".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| dispatch(target, query)))
                    .unwrap_or(Err(SearchErrorKind::Unconfirmed));
                drop(flight);
                let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
            })
            .map(|_| ())
    }))
    .unwrap_or_else(|_| Err(std::io::Error::other("Search worker could not start")))
    .map_err(|_| SearchErrorKind::Unavailable)
}
