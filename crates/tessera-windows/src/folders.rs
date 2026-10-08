// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Prompt folder requests; all native targets live exclusively on one worker.

use std::sync::Arc;
#[cfg(not(windows))]
use tessera_system::folders::FolderErrorKind;
use tessera_system::folders::{FolderError, FolderHost};

/// Creates an independent current-user provider, without waiting for shell I/O.
/// COM initialization is performed (and retried after failure) on its STA worker.
pub fn native_folder_host() -> Result<Arc<dyn FolderHost>, FolderError> {
    #[cfg(windows)]
    {
        worker::start(crate::native_folders::NativeFolderDriver::new)
    }
    #[cfg(not(windows))]
    {
        Err(FolderError::new(
            FolderErrorKind::Unsupported,
            "Known folders are available only on Windows.",
        ))
    }
}

#[cfg(any(windows, test))]
pub(crate) mod worker {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;
    use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
    use tessera_system::folders::{
        FolderAvailability, FolderError, FolderErrorKind, FolderHost, FolderId,
        FolderOpenCompletion, FolderReadCompletion, FolderSnapshot, FolderTarget,
    };

    pub(super) const QUEUE_CAPACITY: usize = 16;

    /// Exact FOLDERID values from the pinned windows 0.62.2 Shell bindings.
    pub(crate) const fn known_folder_guid(folder: FolderId) -> u128 {
        match folder {
            FolderId::Recent => 0xae50c081_ebd2_438a_8655_8a092e34987a,
            FolderId::Desktop => 0xb4bfcc3a_db2c_424c_b029_7fe99a87c641,
            FolderId::Downloads => 0x374de290_123f_4565_9164_39c4925e467b,
            FolderId::Documents => 0xfdd39ad0_238f_46af_adb4_6c85480369c7,
            FolderId::Music => 0x4bd8d571_6d19_48d3_be97_422220080e43,
            FolderId::Pictures => 0x33e28130_4e1e_4676_835a_98395c3bc3bb,
            FolderId::Videos => 0x18989b1d_99b5_455b_841c_ab7c74e4ddfc,
        }
    }

    /// Error text contains no native paths, COM descriptions or user identity.
    pub(crate) fn native_error(code: i32, operation: &str) -> FolderError {
        let kind = match code as u32 {
            0x8007_0005 => FolderErrorKind::AccessDenied,
            0x8007_0002 | 0x8007_0003 | 0x8007_0015 | 0x8007_0035 | 0x8007_0043 | 0x8007_0490 => {
                FolderErrorKind::Unavailable
            }
            0x8000_4001 | 0x8007_0057 | 0x8004_0154 => FolderErrorKind::Unsupported,
            0x8007_00aa | 0x8007_06bb | 0x8001_010a => FolderErrorKind::Busy,
            _ => FolderErrorKind::Other,
        };
        FolderError::new(kind, format!("{operation} failed (0x{:08X}).", code as u32))
    }

    /// Internal seam also used by the recording driver. Target need not be Send:
    /// creation, comparison, dispatch and destruction occur on the STA only.
    pub(crate) trait Driver: 'static {
        type Target;
        fn resolve(&mut self, folder: FolderId) -> Result<Self::Target, FolderError>;
        fn same_target(
            &mut self,
            expected: &Self::Target,
            fresh: &Self::Target,
        ) -> Result<bool, FolderError>;
        fn dispatch(&mut self, fresh: &Self::Target) -> Result<(), FolderError>;
    }

    struct Expectation {
        provider: Arc<()>,
        folder: FolderId,
        revision: Arc<()>,
    }

    struct CachedTarget<T> {
        revision: Arc<()>,
        target: T,
    }

    pub(super) struct Service<D: Driver> {
        // Declaration order matters: native targets release before COM teardown.
        targets: [Option<CachedTarget<D::Target>>; 7],
        driver: D,
        provider: Arc<()>,
    }

    impl<D: Driver> Service<D> {
        pub(super) fn new(driver: D, provider: Arc<()>) -> Self {
            Self {
                targets: std::array::from_fn(|_| None),
                driver,
                provider,
            }
        }

        pub(super) fn read(&mut self) -> FolderSnapshot {
            let rows = FolderId::ALL.map(|folder| {
                let index = index(folder);
                match self.driver.resolve(folder) {
                    Ok(target) => {
                        // A reread of the same canonical shell identity preserves
                        // valid expectations; redirection invalidates old ones.
                        let revision = self.targets[index]
                            .as_ref()
                            .filter(|old| {
                                self.driver
                                    .same_target(&old.target, &target)
                                    .unwrap_or(false)
                            })
                            .map_or_else(|| Arc::new(()), |old| Arc::clone(&old.revision));
                        self.targets[index] = Some(CachedTarget {
                            revision: Arc::clone(&revision),
                            target,
                        });
                        FolderAvailability::Ready(FolderTarget::new(Expectation {
                            provider: Arc::clone(&self.provider),
                            folder,
                            revision,
                        }))
                    }
                    Err(error) => {
                        self.targets[index] = None;
                        FolderAvailability::Unavailable(error)
                    }
                }
            });
            FolderSnapshot::new(rows)
        }

        pub(super) fn open(
            &mut self,
            folder: FolderId,
            expected: &FolderTarget,
        ) -> Result<(), FolderError> {
            let expectation = expected.get::<Expectation>().ok_or_else(target_changed)?;
            if expectation.folder != folder || !Arc::ptr_eq(&expectation.provider, &self.provider) {
                return Err(target_changed());
            }
            let cached = self.targets[index(folder)]
                .as_ref()
                .ok_or_else(target_changed)?;
            if !Arc::ptr_eq(&expectation.revision, &cached.revision) {
                return Err(target_changed());
            }
            // Resolve and validate on the same worker immediately before dispatch.
            // Nothing from the caller contributes a path, PIDL or verb.
            let fresh = self.driver.resolve(folder)?;
            if !self.driver.same_target(&cached.target, &fresh)? {
                return Err(target_changed());
            }
            self.driver.dispatch(&fresh)
        }
    }

    fn index(folder: FolderId) -> usize {
        match folder {
            FolderId::Recent => 0,
            FolderId::Desktop => 1,
            FolderId::Downloads => 2,
            FolderId::Documents => 3,
            FolderId::Music => 4,
            FolderId::Pictures => 5,
            FolderId::Videos => 6,
        }
    }

    fn target_changed() -> FolderError {
        FolderError::new(
            FolderErrorKind::TargetChanged,
            "The folder target has changed. Reopen or retry the menu.",
        )
    }

    fn worker_panicked() -> FolderError {
        FolderError::new(
            FolderErrorKind::Other,
            "The folder worker could not complete the request.",
        )
    }

    enum Request {
        Read(FolderReadCompletion),
        Open(FolderId, FolderTarget, FolderOpenCompletion),
    }

    struct WorkerHost {
        sender: SyncSender<Request>,
    }

    impl WorkerHost {
        fn submit(&self, request: Request) -> Result<(), FolderError> {
            self.sender.try_send(request).map_err(|error| match error {
                TrySendError::Full(_) => FolderError::new(
                    FolderErrorKind::Busy,
                    "The folder worker is busy. Try again shortly.",
                ),
                TrySendError::Disconnected(_) => {
                    FolderError::new(FolderErrorKind::Stopped, "The folder worker has stopped.")
                }
            })
        }
    }

    impl FolderHost for WorkerHost {
        fn read(&self, completion: FolderReadCompletion) -> Result<(), FolderError> {
            self.submit(Request::Read(completion))
        }

        fn open(
            &self,
            folder: FolderId,
            expected: FolderTarget,
            completion: FolderOpenCompletion,
        ) -> Result<(), FolderError> {
            self.submit(Request::Open(folder, expected, completion))
        }
    }

    pub(crate) fn start<D, F>(mut create: F) -> Result<Arc<dyn FolderHost>, FolderError>
    where
        D: Driver,
        F: FnMut() -> Result<D, FolderError> + Send + 'static,
    {
        let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
        let provider = Arc::new(());
        std::thread::Builder::new()
            .name("tessera-known-folders".into())
            .spawn(move || {
                let mut service = None;
                while let Ok(request) = receiver.recv() {
                    let initialization = if service.is_none() {
                        match catch_unwind(AssertUnwindSafe(&mut create)) {
                            Ok(Ok(driver)) => {
                                service = Some(Service::new(driver, Arc::clone(&provider)));
                                Ok(())
                            }
                            Ok(Err(error)) => Err(error),
                            Err(_) => Err(worker_panicked()),
                        }
                    } else {
                        Ok(())
                    };
                    match request {
                        Request::Read(completion) => {
                            let result = initialization.and_then(|()| {
                                catch_unwind(AssertUnwindSafe(|| service.as_mut().unwrap().read()))
                                    .map_err(|_| worker_panicked())
                            });
                            // A consumer panic must not strand later accepted requests.
                            let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                        }
                        Request::Open(folder, expected, completion) => {
                            let result = initialization.and_then(|()| {
                                catch_unwind(AssertUnwindSafe(|| {
                                    service.as_mut().unwrap().open(folder, &expected)
                                }))
                                .unwrap_or_else(|_| Err(worker_panicked()))
                            });
                            let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                        }
                    }
                }
                // No JoinHandle is retained or joined by UI teardown. Sender drop
                // drains accepted work, then drops targets and the STA here.
            })
            .map_err(|_| {
                FolderError::new(FolderErrorKind::Other, "The folder worker could not start.")
            })?;
        Ok(Arc::new(WorkerHost { sender }))
    }
}

#[cfg(test)]
#[path = "native_folders/tests.rs"]
mod tests;
