// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit Power requests on detached, request-local owner threads.

use std::sync::Arc;
use tessera_system::power::{PowerError, PowerHost};

/// Side-effect-free factory, independent of display queries and menu lifetime.
///
/// No worker or native call exists until `perform`. Accepted work owns its
/// driver independently of the host/UI; there is no join or cancellation.
pub fn native_power_host() -> Result<Arc<dyn PowerHost>, PowerError> {
    #[cfg(windows)]
    {
        Ok(worker::host(|| Ok(crate::native_power::driver())))
    }
    #[cfg(not(windows))]
    {
        Err(PowerError::Unsupported)
    }
}

#[cfg(any(windows, test))]
pub(crate) mod worker {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;

    use tessera_system::power::{
        PowerAction, PowerCompletion, PowerError, PowerHost, PowerRequestAccepted,
    };

    use crate::single_flight::FlightGate;

    /// Created, used and dropped on the request owner; deliberately not Send.
    pub(crate) trait Driver: 'static {
        fn perform(&mut self, action: PowerAction) -> Result<PowerRequestAccepted, PowerError>;
    }

    type Job = Box<dyn FnOnce() + Send + 'static>;

    /// Failure or panic must drop, never run or retain, the submitted job.
    /// Successful recording spawners may run inline to exercise callback reentry.
    trait Spawner: Send + Sync + 'static {
        fn spawn(&self, job: Job) -> std::io::Result<()>;
    }

    struct RequestThread;

    impl Spawner for RequestThread {
        fn spawn(&self, job: Job) -> std::io::Result<()> {
            std::thread::Builder::new()
                .name("tessera-power".into())
                .spawn(job)
                .map(|_| ())
        }
    }

    struct RequestHost<F, S> {
        create: Arc<F>,
        spawner: S,
        gate: FlightGate,
    }

    impl<D, F, S> PowerHost for RequestHost<F, S>
    where
        D: Driver,
        F: Fn() -> Result<D, PowerError> + Send + Sync + 'static,
        S: Spawner,
    {
        fn perform(
            &self,
            action: PowerAction,
            completion: PowerCompletion,
        ) -> Result<(), PowerError> {
            let flight = self.gate.try_enter().ok_or(PowerError::Busy)?;
            let create = Arc::clone(&self.create);
            let job: Job = Box::new(move || {
                let result = execute(create.as_ref(), action);
                // Native cleanup/driver retirement precedes admission release;
                // the consumer never runs under a gate or with a live driver.
                drop(flight);
                let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
            });
            catch_unwind(AssertUnwindSafe(|| self.spawner.spawn(job)))
                .unwrap_or_else(|_| Err(std::io::Error::other("Power worker spawn panicked")))
                .map_err(|_| PowerError::Unavailable)
            // Failed spawn drops its job/flight and accepts zero callbacks.
        }
    }

    pub(crate) fn host<D, F>(create: F) -> Arc<dyn PowerHost>
    where
        D: Driver,
        F: Fn() -> Result<D, PowerError> + Send + Sync + 'static,
    {
        host_with_spawner(create, RequestThread)
    }

    fn host_with_spawner<D, F, S>(create: F, spawner: S) -> Arc<dyn PowerHost>
    where
        D: Driver,
        F: Fn() -> Result<D, PowerError> + Send + Sync + 'static,
        S: Spawner,
    {
        Arc::new(RequestHost {
            create: Arc::new(create),
            spawner,
            gate: FlightGate::default(),
        })
    }

    fn execute<D: Driver>(
        create: &impl Fn() -> Result<D, PowerError>,
        action: PowerAction,
    ) -> Result<PowerRequestAccepted, PowerError> {
        catch_unwind(AssertUnwindSafe(|| {
            let mut driver = create()?;
            driver.perform(action)
            // Driver and any request-local resources retire before this returns.
        }))
        .unwrap_or(Err(PowerError::Unavailable))
        // Unwinding only: aborts, double panics during Drop, native stalls and
        // process termination cannot be recovered or promised a completion.
    }

    #[cfg(test)]
    #[path = "tests.rs"]
    mod tests;
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn native_power_factory_is_unsupported_off_windows_not_a_fake_lock() {
        assert_eq!(native_power_host().err(), Some(PowerError::Unsupported));
    }
}
