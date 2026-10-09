// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Fixed two-key existence predicate, not an exhaustive Windows Update query.
//! Raw adapters replace calls/opaque keys, never cleanup or error policy.

use std::marker::PhantomData;
use std::rc::Rc;

use tessera_system::power_updates::{PowerUpdateHint, PowerUpdatesError};

use crate::power_updates::worker::Driver;

const PATHS: [&str; 2] = [
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired",
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending",
];
const READ_ACCESS: u32 = 0x0002_0019; // SDK KEY_READ, with neither WOW64 flag.
const OPEN_OPTIONS: u32 = 0;
const FILE_NOT_FOUND: u32 = 2;
const PATH_NOT_FOUND: u32 = 3;
const ACCESS_DENIED: u32 = 5;

/// Only HKLM can be requested by this private, fixed read protocol.
trait Calls {
    type Key;

    /// The status is RegOpenKeyExW's own LSTATUS, never GetLastError.
    fn open_hklm(&mut self, path: &str, options: u32, access: u32) -> Result<Self::Key, u32>;
    /// Consume the key for exactly one checked RegCloseKey; no Drop retry.
    fn close(&mut self, key: Self::Key) -> Result<(), u32>;
}

struct Owner<C: Calls> {
    calls: C,
    key: Option<C::Key>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl<C: Calls> Owner<C> {
    fn new(calls: C) -> Self {
        Self {
            calls,
            key: None,
            _thread_bound: PhantomData,
        }
    }

    fn retire(&mut self) -> Result<(), PowerUpdatesError> {
        // Remove ownership before attempting close, even if an adapter unwinds.
        // A failed native close can leak until process exit; never blindly retry.
        self.key
            .take()
            .map_or(Ok(()), |key| self.calls.close(key).map_err(native_error))
    }
}

impl<C: Calls + 'static> Driver for Owner<C> {
    fn read(&mut self) -> Result<PowerUpdateHint, PowerUpdatesError> {
        let mut first_error = None;
        for path in PATHS {
            match self.calls.open_hklm(path, OPEN_OPTIONS, READ_ACCESS) {
                Ok(key) => {
                    self.key = Some(key);
                    if let Err(error) = self.retire() {
                        // First fault remains diagnostic if cleanup prevents a
                        // trustworthy success; a later fault never erases it.
                        return Err(first_error.unwrap_or(error));
                    }
                    // Known existence wins over an earlier unrelated open error.
                    return Ok(PowerUpdateHint::Pending);
                }
                Err(FILE_NOT_FOUND | PATH_NOT_FOUND) => {}
                Err(code) => {
                    first_error.get_or_insert(native_error(code));
                }
            }
        }
        first_error.map_or(Ok(PowerUpdateHint::NotDetected), Err)
    }
}

impl<C: Calls> Drop for Owner<C> {
    fn drop(&mut self) {
        let _ = self.retire();
    }
}

fn native_error(code: u32) -> PowerUpdatesError {
    if code == ACCESS_DENIED {
        PowerUpdatesError::AccessDenied
    } else {
        PowerUpdatesError::Native { code }
    }
}

/// Pure construction on the already accepted read-owner thread.
#[cfg(windows)]
pub(crate) fn driver() -> impl Driver {
    Owner::new(sdk::WindowsCalls)
}

#[cfg(windows)]
#[path = "native_power_updates/sdk.rs"]
mod sdk;

#[cfg(test)]
#[path = "native_power_updates/tests.rs"]
mod tests;

#[cfg(all(test, windows))]
#[path = "native_power_updates/sdk_tests.rs"]
mod sdk_tests;
