// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One detached owner thread per explicitly accepted mutation; no idle worker.

use std::sync::Arc;
use tessera_system::dock_utilities::DockUtilityError;
#[cfg(not(windows))]
use tessera_system::dock_utilities::DockUtilityErrorKind;
use tessera_system::recycle_bin_mutation::RecycleBinMutationHost;

/// Side-effect-free factory: no thread, COM, window, query or native mutation.
/// Accepted work owns its native scope independently of host/UI lifetime. There
/// is no timeout, native cancellation or drain-across-process-exit guarantee.
pub fn native_recycle_bin_mutation_host()
-> Result<Arc<dyn RecycleBinMutationHost>, DockUtilityError> {
    #[cfg(windows)]
    {
        Ok(worker::host(|| {
            Ok(crate::native_recycle_bin_mutation::NativeRecycleBinMutationDriver::new())
        }))
    }
    #[cfg(not(windows))]
    {
        Err(DockUtilityError::new(
            DockUtilityErrorKind::Unsupported,
            "Recycle Bin mutation platform: 0x80004001",
        ))
    }
}

#[cfg(any(windows, test))]
pub(crate) mod worker {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tessera_system::dock_utilities::DockUtilityError;
    use tessera_system::recycle_bin_mutation::{
        RecycleBinEmptyCompletion, RecycleBinEmptyOutcome, RecycleBinMutationHost,
    };

    pub(crate) use crate::dock_utilities::worker::native_error;

    /// Created/used/dropped on the request thread. Deliberately no Send bound.
    pub(crate) trait Driver: 'static {
        fn empty(&mut self) -> Result<RecycleBinEmptyOutcome, DockUtilityError>;
    }

    type Job = Box<dyn FnOnce() + Send + 'static>;

    // An Err must not run the job. Successful production spawning detaches; the
    // recording adapter can execute inline to exercise real callback reentry.
    trait Spawner: Send + Sync + 'static {
        fn spawn(&self, job: Job) -> std::io::Result<()>;
    }

    struct RequestThread;

    impl Spawner for RequestThread {
        fn spawn(&self, job: Job) -> std::io::Result<()> {
            std::thread::Builder::new()
                .name("tessera-recycle-empty".into())
                .spawn(job)
                .map(|_| ())
        }
    }

    struct Flight(Arc<AtomicBool>);

    impl Drop for Flight {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }

    struct MutationHost<F, S> {
        create: Arc<F>,
        spawner: S,
        busy: Arc<AtomicBool>,
    }

    impl<D, F, S> RecycleBinMutationHost for MutationHost<F, S>
    where
        D: Driver,
        F: Fn() -> Result<D, DockUtilityError> + Send + Sync + 'static,
        S: Spawner,
    {
        fn empty(&self, completion: RecycleBinEmptyCompletion) -> Result<(), DockUtilityError> {
            self.busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| native_error(0x8007_00AA, "Recycle Bin mutation busy"))?;
            let flight = Flight(Arc::clone(&self.busy));
            let create = Arc::clone(&self.create);
            let job: Job = Box::new(move || {
                let result = execute(create.as_ref());
                // Driver drop/native cleanup attempts have finished; native
                // release can fail. Admit reentry only after that scope, never
                // during native setup/call/cleanup.
                drop(flight);
                let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
            });
            catch_unwind(AssertUnwindSafe(|| self.spawner.spawn(job)))
                .unwrap_or_else(|_| Err(std::io::Error::other("mutation spawn panicked")))
                .map_err(|_| native_error(0x8000_4005, "Recycle Bin mutation thread start"))
            // Failed spawn drops the job/flight without invoking its completion.
        }
    }

    pub(crate) fn host<D, F>(create: F) -> Arc<dyn RecycleBinMutationHost>
    where
        D: Driver,
        F: Fn() -> Result<D, DockUtilityError> + Send + Sync + 'static,
    {
        host_with_spawner(create, RequestThread)
    }

    fn host_with_spawner<D, F, S>(create: F, spawner: S) -> Arc<dyn RecycleBinMutationHost>
    where
        D: Driver,
        F: Fn() -> Result<D, DockUtilityError> + Send + Sync + 'static,
        S: Spawner,
    {
        Arc::new(MutationHost {
            create: Arc::new(create),
            spawner,
            busy: Arc::new(AtomicBool::new(false)),
        })
    }

    fn execute<D: Driver>(
        create: &impl Fn() -> Result<D, DockUtilityError>,
    ) -> Result<RecycleBinEmptyOutcome, DockUtilityError> {
        catch_unwind(AssertUnwindSafe(|| {
            let mut driver = create()?;
            driver.empty()
            // Request-local resources, then driver, drop before returning.
        }))
        .unwrap_or_else(|_| Err(native_error(0x8000_4005, "Recycle Bin mutation worker")))
    }

    #[cfg(test)]
    #[path = "tests.rs"]
    mod tests;
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn native_recycle_bin_mutation_is_unsupported_off_windows() {
        assert_eq!(
            native_recycle_bin_mutation_host().err().unwrap().kind,
            DockUtilityErrorKind::Unsupported,
        );
    }
}
