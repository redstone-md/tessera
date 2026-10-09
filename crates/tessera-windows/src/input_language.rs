// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Bounded, serialized input-language requests without GUI-thread native work.

use std::sync::Arc;
#[cfg(not(windows))]
use tessera_system::input_language::InputLanguageErrorKind;
use tessera_system::input_language::{InputLanguageError, InputLanguageHost};

pub fn native_input_language_host() -> Result<Arc<dyn InputLanguageHost>, InputLanguageError> {
    #[cfg(windows)]
    {
        worker::start(
            crate::native_input_language::read_snapshot,
            crate::native_input_language::execute,
        )
    }
    #[cfg(not(windows))]
    {
        Err(InputLanguageError::new(
            InputLanguageErrorKind::Unsupported,
            "Native input profiles are available only on Windows.",
        ))
    }
}

#[cfg(any(windows, test))]
pub(crate) mod worker {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;
    use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
    use tessera_system::input_language::{
        InputLanguageAction, InputLanguageActionCompletion, InputLanguageError,
        InputLanguageErrorKind, InputLanguageHost, InputLanguageOutcome,
        InputLanguageReadCompletion, InputLanguageSnapshot,
    };

    pub(crate) const QUEUE_CAPACITY: usize = 16;

    enum Request {
        Read(InputLanguageReadCompletion),
        Execute(InputLanguageAction, InputLanguageActionCompletion),
    }

    struct WorkerHost {
        sender: SyncSender<Request>,
    }

    impl WorkerHost {
        fn submit(&self, request: Request) -> Result<(), InputLanguageError> {
            self.sender.try_send(request).map_err(|error| match error {
                TrySendError::Full(_) => InputLanguageError::new(
                    InputLanguageErrorKind::Busy,
                    "The input-language worker is busy. Try again shortly.",
                ),
                TrySendError::Disconnected(_) => InputLanguageError::new(
                    InputLanguageErrorKind::Stopped,
                    "The input-language worker has stopped.",
                ),
            })
        }
    }

    impl InputLanguageHost for WorkerHost {
        fn read(&self, completion: InputLanguageReadCompletion) -> Result<(), InputLanguageError> {
            self.submit(Request::Read(completion))
        }

        fn execute(
            &self,
            action: InputLanguageAction,
            completion: InputLanguageActionCompletion,
        ) -> Result<(), InputLanguageError> {
            self.submit(Request::Execute(action, completion))
        }
    }

    pub(crate) fn start<R, E>(
        mut read: R,
        mut execute: E,
    ) -> Result<Arc<dyn InputLanguageHost>, InputLanguageError>
    where
        R: FnMut() -> Result<InputLanguageSnapshot, InputLanguageError> + Send + 'static,
        E: FnMut(InputLanguageAction) -> Result<InputLanguageOutcome, InputLanguageError>
            + Send
            + 'static,
    {
        let (sender, receiver) = sync_channel::<Request>(QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("tessera-input-language".into())
            .spawn(move || {
                while let Ok(request) = receiver.recv() {
                    match request {
                        Request::Read(completion) => {
                            let result = catch_unwind(AssertUnwindSafe(&mut read))
                                .unwrap_or_else(|_| Err(panic_error(false)));
                            let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                        }
                        Request::Execute(action, completion) => {
                            // A panic might follow a native effect. Complete once with
                            // an explicit unknown outcome, never retry or roll back.
                            let result = catch_unwind(AssertUnwindSafe(|| execute(action)))
                                .unwrap_or_else(|_| Err(panic_error(true)));
                            let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                        }
                    }
                }
                // Receiver drains accepted work before disconnect. No JoinHandle is
                // retained, so dropping the last GUI-held host cannot wait on COM.
            })
            .map_err(|_| {
                InputLanguageError::new(
                    InputLanguageErrorKind::Other,
                    "The input-language worker could not start.",
                )
            })?;
        Ok(Arc::new(WorkerHost { sender }))
    }

    fn panic_error(action: bool) -> InputLanguageError {
        InputLanguageError::new(
            InputLanguageErrorKind::Other,
            if action {
                "The input-language action could not complete; native effects may have occurred. Refresh to observe the current state."
            } else {
                "The input-language worker could not read the current profiles."
            },
        )
    }

    #[cfg(test)]
    mod tests;
}
