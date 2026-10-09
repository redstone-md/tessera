// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One raw Lock request, with no window, COM, privilege or session observation.
//!
//! [LockWorkStation](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-lockworkstation)
//! returns asynchronous initiation only. Nonzero does not prove the session is
//! locked; interactive-desktop and user/session conditions still apply. There
//! is no retry, shortcut synthesis, shell fallback or subsequent state query.

use std::marker::PhantomData;
use std::rc::Rc;

use tessera_system::power::{PowerAction, PowerError, PowerRequestAccepted};

use crate::power::worker::Driver;

const ACCESS_DENIED: u32 = 5;

/// Raw calls are a private recording seam, not native handles exposed to UI.
trait Calls {
    fn lock_work_station(&self) -> i32;
    fn last_error(&self) -> u32;
}

struct PowerDriver<C> {
    calls: C,
    _thread_bound: PhantomData<Rc<()>>,
}

impl<C> PowerDriver<C> {
    fn new(calls: C) -> Self {
        Self {
            calls,
            _thread_bound: PhantomData,
        }
    }
}

impl<C: Calls + 'static> Driver for PowerDriver<C> {
    fn perform(&mut self, action: PowerAction) -> Result<PowerRequestAccepted, PowerError> {
        match action {
            PowerAction::LockSession => {
                let status = self.calls.lock_work_station();
                if status == 0 {
                    // Capture on this owner immediately, before cleanup,
                    // callbacks or any other native API can change last error.
                    let code = self.calls.last_error();
                    Err(if code == ACCESS_DENIED {
                        PowerError::AccessDenied
                    } else {
                        PowerError::Native { code }
                    })
                } else {
                    // Every nonzero BOOL, including negative values, means
                    // initiation only. Never consult stale last error here.
                    Ok(PowerRequestAccepted)
                }
            }
        }
    }
}

/// Construction performs no native call and happens only on the request owner.
#[cfg(windows)]
pub(crate) fn driver() -> impl Driver {
    PowerDriver::new(sdk::WindowsCalls)
}

#[cfg(windows)]
#[path = "native_power/sdk.rs"]
mod sdk;

#[cfg(test)]
#[path = "native_power/tests.rs"]
mod tests;

#[cfg(all(test, windows))]
#[path = "native_power/sdk_tests.rs"]
mod sdk_tests;
