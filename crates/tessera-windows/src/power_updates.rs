// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Read-only pending-update hints on detached, request-local owner threads.

use std::sync::Arc;
use tessera_system::power_updates::PowerUpdatesHost;

#[cfg(not(windows))]
use tessera_system::power_updates::PowerUpdatesError;

/// Pure constructor: no registry call or thread until the first `read`.
///
/// Admission is independent of mutation/display gates. Accepted work owns its
/// resources independently of host/UI drops, with no join or cancellation.
pub fn native_power_updates_host() -> Arc<dyn PowerUpdatesHost> {
    #[cfg(windows)]
    {
        worker::host(|| Ok(crate::native_power_updates::driver()))
    }
    #[cfg(not(windows))]
    {
        Arc::new(Unsupported)
    }
}

#[cfg(not(windows))]
struct Unsupported;

#[cfg(not(windows))]
impl PowerUpdatesHost for Unsupported {
    fn read(
        &self,
        _completion: tessera_system::power_updates::PowerUpdatesCompletion,
    ) -> Result<(), PowerUpdatesError> {
        Err(PowerUpdatesError::Unsupported)
    }
}

#[cfg(any(windows, test))]
pub(crate) mod worker {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;

    use tessera_system::power_updates::{
        PowerUpdateHint, PowerUpdatesCompletion, PowerUpdatesError, PowerUpdatesHost,
    };

    use crate::single_flight::FlightGate;

    /// Created, used and dropped on the read owner; need not be Send.
    pub(crate) trait Driver: 'static {
        fn read(&mut self) -> Result<PowerUpdateHint, PowerUpdatesError>;
    }

    pub(crate) type Job = Box<dyn FnOnce() + Send + 'static>;

    /// Failure/panic must drop, never run or retain, the submitted job.
    pub(crate) trait Spawner: Send + Sync + 'static {
        fn spawn(&self, job: Job) -> std::io::Result<()>;
    }

    struct ReadThread;

    impl Spawner for ReadThread {
        fn spawn(&self, job: Job) -> std::io::Result<()> {
            std::thread::Builder::new()
                .name("tessera-power-updates".into())
                .spawn(job)
                .map(|_| ())
        }
    }

    struct ReadHost<F, S> {
        create: Arc<F>,
        spawner: S,
        gate: FlightGate,
    }

    impl<D, F, S> PowerUpdatesHost for ReadHost<F, S>
    where
        D: Driver,
        F: Fn() -> Result<D, PowerUpdatesError> + Send + Sync + 'static,
        S: Spawner,
    {
        fn read(&self, completion: PowerUpdatesCompletion) -> Result<(), PowerUpdatesError> {
            let flight = self.gate.try_enter().ok_or(PowerUpdatesError::Busy)?;
            let create = Arc::clone(&self.create);
            let job: Job = Box::new(move || {
                let result = execute(create.as_ref());
                // Owner/resource drop is complete before gate release/reentry.
                drop(flight);
                let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
            });
            catch_unwind(AssertUnwindSafe(|| self.spawner.spawn(job)))
                .unwrap_or_else(|_| Err(std::io::Error::other("Update hint worker spawn panicked")))
                .map_err(|_| PowerUpdatesError::Unavailable)
        }
    }

    pub(crate) fn host<D, F>(create: F) -> Arc<dyn PowerUpdatesHost>
    where
        D: Driver,
        F: Fn() -> Result<D, PowerUpdatesError> + Send + Sync + 'static,
    {
        host_with_spawner(create, ReadThread)
    }

    pub(crate) fn host_with_spawner<D, F, S>(create: F, spawner: S) -> Arc<dyn PowerUpdatesHost>
    where
        D: Driver,
        F: Fn() -> Result<D, PowerUpdatesError> + Send + Sync + 'static,
        S: Spawner,
    {
        Arc::new(ReadHost {
            create: Arc::new(create),
            spawner,
            gate: FlightGate::default(),
        })
    }

    fn execute<D: Driver>(
        create: &impl Fn() -> Result<D, PowerUpdatesError>,
    ) -> Result<PowerUpdateHint, PowerUpdatesError> {
        catch_unwind(AssertUnwindSafe(|| {
            let mut driver = create()?;
            driver.read()
            // Driver and any read-local resources retire before returning.
        }))
        .unwrap_or(Err(PowerUpdatesError::Unavailable))
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn off_windows_factory_is_pure_and_read_is_unsupported_without_callback() {
        let host = native_power_updates_host();
        for _ in 0..2 {
            assert_eq!(
                host.read(Box::new(|_| panic!(
                    "an immediate error accepts no callback"
                ))),
                Err(PowerUpdatesError::Unsupported),
            );
        }
    }
}
