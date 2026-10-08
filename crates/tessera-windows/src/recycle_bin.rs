// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Independent, fixed-namespace requests; watching never uses the request queue.

use std::sync::Arc;
use tessera_system::dock_utilities::DockUtilityError;
#[cfg(not(windows))]
use tessera_system::dock_utilities::DockUtilityErrorKind;
use tessera_system::recycle_bin::RecycleBinHost;

/// Prompt/lazy factory: no initial query, COM activation, opening or deletion.
/// Accepted requests drain after host drop, with no GUI-thread join. Native calls
/// can stall indefinitely; queue bounds are admission bounds, not timeouts.
pub fn native_recycle_bin_host() -> Result<Arc<dyn RecycleBinHost>, DockUtilityError> {
    #[cfg(windows)]
    {
        worker::start(
            || Ok(crate::native_recycle_bin::NativeRecycleBinDriver::new()),
            crate::native_recycle_bin::watcher::start_native,
        )
    }
    #[cfg(not(windows))]
    {
        Err(DockUtilityError::new(
            DockUtilityErrorKind::Unsupported,
            "Recycle Bin platform: 0x80004001",
        ))
    }
}

#[cfg(any(windows, test))]
pub(crate) mod worker {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;
    use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};

    use tessera_system::dock_utilities::{DockUtilityError, DockUtilityErrorKind};
    use tessera_system::recycle_bin::{
        RecycleBinCompletion, RecycleBinHost, RecycleBinInfo, RecycleBinReadCompletion,
        RecycleBinWatchCallback, RecycleBinWatchCompletion, RecycleBinWatchGuard,
    };

    pub(super) const QUEUE_CAPACITY: usize = 16;
    pub(crate) use crate::dock_utilities::worker::native_error;

    /// No Send bound: each adapter is created, used and dropped on its worker.
    pub(crate) trait Driver: 'static {
        fn read(&mut self) -> Result<RecycleBinInfo, DockUtilityError>;
        fn open(&mut self) -> Result<(), DockUtilityError>;
    }

    enum Request {
        Read(RecycleBinReadCompletion),
        Open(RecycleBinCompletion),
    }

    struct WorkerHost<W> {
        sender: SyncSender<Request>,
        watch: W,
    }

    impl<W> WorkerHost<W> {
        fn enqueue(&self, request: Request) -> Result<(), DockUtilityError> {
            self.sender.try_send(request).map_err(|error| match error {
                TrySendError::Full(_) => native_error(0x8007_00AA, "Recycle Bin queue"),
                TrySendError::Disconnected(_) => DockUtilityError::new(
                    DockUtilityErrorKind::Stopped,
                    "Recycle Bin queue: 0x80010108",
                ),
            })
        }
    }

    impl<W> RecycleBinHost for WorkerHost<W>
    where
        W: Fn(
                RecycleBinWatchCallback,
                RecycleBinWatchCompletion,
            ) -> Result<Box<dyn RecycleBinWatchGuard>, DockUtilityError>
            + Send
            + Sync
            + 'static,
    {
        fn read(&self, completion: RecycleBinReadCompletion) -> Result<(), DockUtilityError> {
            self.enqueue(Request::Read(completion))
        }

        fn open(&self, completion: RecycleBinCompletion) -> Result<(), DockUtilityError> {
            self.enqueue(Request::Open(completion))
        }

        fn watch(
            &self,
            events: RecycleBinWatchCallback,
            ready: RecycleBinWatchCompletion,
        ) -> Result<Box<dyn RecycleBinWatchGuard>, DockUtilityError> {
            (self.watch)(events, ready)
        }
    }

    pub(crate) fn start<D, F, W>(
        mut create: F,
        watch: W,
    ) -> Result<Arc<dyn RecycleBinHost>, DockUtilityError>
    where
        D: Driver,
        F: FnMut() -> Result<D, DockUtilityError> + Send + 'static,
        W: Fn(
                RecycleBinWatchCallback,
                RecycleBinWatchCompletion,
            ) -> Result<Box<dyn RecycleBinWatchGuard>, DockUtilityError>
            + Send
            + Sync
            + 'static,
    {
        let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("tessera-recycle-bin".into())
            .spawn(move || {
                while let Ok(request) = receiver.recv() {
                    match request {
                        Request::Read(completion) => {
                            let result = execute(&mut create, |driver| driver.read());
                            let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                        }
                        Request::Open(completion) => {
                            let result = execute(&mut create, |driver| driver.open());
                            let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                        }
                    }
                }
            })
            .map_err(|_| native_error(0x8000_4005, "Recycle Bin worker start"))?;
        Ok(Arc::new(WorkerHost { sender, watch }))
    }

    fn execute<D: Driver, T>(
        create: &mut impl FnMut() -> Result<D, DockUtilityError>,
        operation: impl FnOnce(&mut D) -> Result<T, DockUtilityError>,
    ) -> Result<T, DockUtilityError> {
        catch_unwind(AssertUnwindSafe(|| {
            let mut driver = create()?;
            operation(&mut driver)
            // Driver/apartment/target drop before completion and idle recv.
        }))
        .unwrap_or_else(|_| Err(native_error(0x8000_4005, "Recycle Bin worker")))
    }

    #[cfg(test)]
    #[path = "tests.rs"]
    mod tests;
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn native_factory_is_unsupported_off_windows() {
        assert_eq!(
            native_recycle_bin_host().err().unwrap().kind,
            DockUtilityErrorKind::Unsupported,
        );
    }
}
