// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Fixed native power requests; no window, COM, service, elevation or observer.
//!
//! Lock/logoff are privilege-free on the existing neutral request worker. The
//! four privileged actions create, use and drop a private thread-token owner on
//! a child operation thread. Only a portable result crosses its real join, before
//! existing worker retirement, gate release and consumer delivery. Never replay:
//! even an error may follow an accepted native action and failed cleanup.
//!
//! Failed reversion can leave the child impersonating through runtime/TLS/DLL
//! teardown or a global panic hook. The guarantee is application-consumer
//! neutrality after observed thread termination, not that no code executes in
//! that interval. A failed token close may leak a handle until process exit.
//! Unproven retirement (a panic of join itself) quarantines the background owner
//! without completion or releasing its flight; no timeout or forced termination.

use std::marker::PhantomData;
use std::rc::Rc;

use tessera_system::power::{PowerAction, PowerError, PowerRequestAccepted, PowerUpdatePolicy};

use crate::power::worker::Driver;

#[path = "native_power/operation.rs"]
mod operation;
#[path = "native_power/owner.rs"]
mod owner;
use owner::{Calls, Owner};

const ACCESS_DENIED: u32 = 5;
const SECURITY_IMPERSONATION: i32 = 2;
const SHUTDOWN_TOKEN_ACCESS: u32 = 0x20 | 0x08;
const PRIVILEGE_ENABLED: u32 = 2;
const SHUTDOWN_POWEROFF: u32 = 8;
const SHUTDOWN_RESTART: u32 = 4;
const SHUTDOWN_INSTALL_UPDATES: u32 = 64;
const UPDATE_REASON: u32 = 0x8002_0003;

type Outcome = Result<PowerRequestAccepted, PowerError>;

fn native_error(code: u32) -> PowerError {
    if code == ACCESS_DENIED {
        PowerError::AccessDenied
    } else {
        PowerError::Native { code }
    }
}

fn needs_privilege(action: PowerAction) -> bool {
    matches!(
        action,
        PowerAction::PowerOff { .. }
            | PowerAction::Reboot { .. }
            | PowerAction::Suspend
            | PowerAction::Hibernate
    )
}

struct PowerDriver<F> {
    create: F,
    spawn: operation::Spawn,
    _thread_bound: PhantomData<Rc<()>>,
}

impl<F> PowerDriver<F> {
    fn new(create: F) -> Self {
        Self {
            create,
            spawn: operation::spawn,
            _thread_bound: PhantomData,
        }
    }
}

impl<C, F> Driver for PowerDriver<F>
where
    C: Calls + 'static,
    F: Fn() -> C + Clone + Send + 'static,
{
    fn perform(&mut self, action: PowerAction) -> Outcome {
        if needs_privilege(action) {
            let create = self.create.clone();
            // Capture only neutral factory/config and the closed Copy intent.
            // Calls/resources are constructed INSIDE the child; no consumer,
            // flight guard, host/UI, logger or application Drop closure goes in.
            operation::run(
                Box::new(move || Owner::new(create()).perform(action)),
                self.spawn,
            )
        } else {
            Owner::new((self.create)()).perform(action)
        }
    }
}

/// Pure construction; the existing worker remains the neutral outer owner.
#[cfg(windows)]
pub(crate) fn driver() -> impl Driver {
    PowerDriver::new(sdk::WindowsCalls::new)
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
