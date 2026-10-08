// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Prompt utility acceptance; request-scoped native resources stay on a worker.

use std::sync::Arc;
#[cfg(not(windows))]
use tessera_system::dock_utilities::DockUtilityErrorKind;
use tessera_system::dock_utilities::{DockUtilitiesHost, DockUtilityError};

/// Starts an independent worker without initializing COM or toggling anything.
/// Accepted requests drain after the last host is dropped; UI teardown never
/// joins the worker. Each action acquires and releases its own Shell apartment.
pub fn native_dock_utilities_host() -> Result<Arc<dyn DockUtilitiesHost>, DockUtilityError> {
    #[cfg(windows)]
    {
        worker::start(|| Ok(crate::native_dock_utilities::NativeDockUtilitiesDriver::new()))
    }
    #[cfg(not(windows))]
    {
        Err(DockUtilityError::new(
            DockUtilityErrorKind::Unsupported,
            "Dock utility platform: 0x80004001",
        ))
    }
}

#[cfg(any(windows, test))]
pub(crate) mod worker {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;
    use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};

    use tessera_system::dock_utilities::{
        DockUtilitiesHost, DockUtilityCompletion, DockUtilityError, DockUtilityErrorKind,
    };

    pub(super) const QUEUE_CAPACITY: usize = 16;

    /// Never format a COM description, path, identity, or provider-supplied text.
    pub(crate) fn native_error(code: u32, operation: &'static str) -> DockUtilityError {
        let kind = match code {
            0x8000_4001 | 0x8000_4002 | 0x8004_0154 => DockUtilityErrorKind::Unsupported,
            0x8007_0005 => DockUtilityErrorKind::AccessDenied,
            0x8007_00AA | 0x8001_010A | 0x8001_0001 => DockUtilityErrorKind::Busy,
            0x8007_06BA | 0x8001_0108 | 0x8001_0007 => DockUtilityErrorKind::Unavailable,
            _ => DockUtilityErrorKind::Other,
        };
        DockUtilityError::new(kind, format!("{operation}: 0x{code:08X}"))
    }

    /// Created, invoked and dropped on the worker for each accepted request.
    /// No Send bound: native apartment and Shell values must remain thread-local.
    pub(crate) trait Driver: 'static {
        fn toggle_desktop(&mut self) -> Result<(), DockUtilityError>;
    }

    struct WorkerHost {
        sender: SyncSender<DockUtilityCompletion>,
    }

    impl DockUtilitiesHost for WorkerHost {
        fn toggle_desktop(
            &self,
            completion: DockUtilityCompletion,
        ) -> Result<(), DockUtilityError> {
            self.sender
                .try_send(completion)
                .map_err(|error| match error {
                    TrySendError::Full(_) => native_error(0x8007_00AA, "Dock utility queue"),
                    TrySendError::Disconnected(_) => DockUtilityError::new(
                        DockUtilityErrorKind::Stopped,
                        "Dock utility queue: 0x80010108",
                    ),
                })
        }
    }

    pub(crate) fn start<D, F>(mut create: F) -> Result<Arc<dyn DockUtilitiesHost>, DockUtilityError>
    where
        D: Driver,
        F: FnMut() -> Result<D, DockUtilityError> + Send + 'static,
    {
        let (sender, receiver) = sync_channel::<DockUtilityCompletion>(QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("tessera-dock-utilities".into())
            .spawn(move || {
                while let Ok(completion) = receiver.recv() {
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        let mut driver = create()?;
                        driver.toggle_desktop()
                        // Driver and all native resources drop before the callback
                        // and before the next blocking recv (no live idle STA).
                    }))
                    .unwrap_or_else(|_| Err(native_error(0x8000_4005, "Dock utility worker")));
                    // Consumer reentry only queues; a consumer panic cannot strand
                    // subsequent accepted requests. No locks/native borrows remain.
                    let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                }
                // Detached: receiver drains the channel after all senders drop.
            })
            .map_err(|_| native_error(0x8000_4005, "Dock utility worker start"))?;
        Ok(Arc::new(WorkerHost { sender }))
    }

    #[cfg(test)]
    #[path = "tests.rs"]
    mod tests;
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn native_dock_utilities_are_unsupported_off_windows() {
        let error = native_dock_utilities_host().err().unwrap();
        assert_eq!(error.kind, DockUtilityErrorKind::Unsupported);
        assert_eq!(error.message, "Dock utility platform: 0x80004001");
    }
}
