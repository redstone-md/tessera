// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Prompt, read-only calendar acquisition on a dedicated bounded worker.

use std::sync::Arc;
#[cfg(not(windows))]
use tessera_system::calendar::CalendarErrorKind;
use tessera_system::calendar::{CalendarError, CalendarHost};

/// Starts acquisition without waiting for Windows time or globalization calls.
/// Non-Windows platforms report unsupported rather than inventing local data.
pub fn native_calendar_host() -> Result<Arc<dyn CalendarHost>, CalendarError> {
    #[cfg(windows)]
    {
        worker::start(crate::native_calendar::read_snapshot)
    }
    #[cfg(not(windows))]
    {
        Err(CalendarError::new(
            CalendarErrorKind::Unsupported,
            "Native calendar data is available only on Windows.",
        ))
    }
}

#[cfg(any(windows, test))]
pub(crate) mod worker {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;
    use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
    use tessera_system::calendar::{
        CalendarError, CalendarErrorKind, CalendarHost, CalendarReadCompletion, CalendarSnapshot,
    };

    pub(crate) const QUEUE_CAPACITY: usize = 16;

    struct WorkerHost {
        sender: SyncSender<CalendarReadCompletion>,
    }

    impl CalendarHost for WorkerHost {
        fn read(&self, completion: CalendarReadCompletion) -> Result<(), CalendarError> {
            self.sender
                .try_send(completion)
                .map_err(|error| match error {
                    TrySendError::Full(_) => CalendarError::new(
                        CalendarErrorKind::Busy,
                        "The calendar worker is busy. Try again shortly.",
                    ),
                    TrySendError::Disconnected(_) => CalendarError::new(
                        CalendarErrorKind::Stopped,
                        "The calendar worker has stopped.",
                    ),
                })
        }
    }

    pub(crate) fn start<F>(mut read: F) -> Result<Arc<dyn CalendarHost>, CalendarError>
    where
        F: FnMut() -> Result<CalendarSnapshot, CalendarError> + Send + 'static,
    {
        let (sender, receiver) = sync_channel::<CalendarReadCompletion>(QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("tessera-calendar".into())
            .spawn(move || {
                while let Ok(completion) = receiver.recv() {
                    let result = catch_unwind(AssertUnwindSafe(&mut read)).unwrap_or_else(|_| {
                        Err(CalendarError::new(
                            CalendarErrorKind::Other,
                            "The calendar worker could not complete the request.",
                        ))
                    });
                    // Consumer panics and reentrant submissions cannot strand
                    // later accepted reads. No locks are held across callbacks.
                    let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                }
                // Dropping the last sender drains accepted reads. The UI never
                // retains or joins the worker's handle, including during teardown.
            })
            .map_err(|_| {
                CalendarError::new(
                    CalendarErrorKind::Other,
                    "The calendar worker could not start.",
                )
            })?;
        Ok(Arc::new(WorkerHost { sender }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicBool, Ordering};

        #[test]
        fn disconnected_queue_rejects_without_a_completion() {
            let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
            drop(receiver);
            let host = WorkerHost { sender };
            let called = Arc::new(AtomicBool::new(false));
            let callback_called = Arc::clone(&called);
            let result = host.read(Box::new(move |_| {
                callback_called.store(true, Ordering::SeqCst);
            }));
            assert_eq!(result.unwrap_err().kind, CalendarErrorKind::Stopped);
            assert!(!called.load(Ordering::SeqCst));
        }
    }
}
