// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Inert factory for one lazy, stable-thread telemetry reader.

use std::sync::Arc;
use tessera_system::telemetry::TelemetryHost;

/// Cache this facade in the desktop host. Construction starts no thread or SDK read.
pub fn native_telemetry_host() -> Option<Arc<dyn TelemetryHost>> {
    #[cfg(windows)]
    {
        Some(Arc::new(worker::LazyHost::default()))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
mod worker {
    use crate::native_telemetry::NativeReader;
    use crate::single_flight::{Flight, FlightGate};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::OnceLock;
    use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
    use tessera_system::telemetry::{
        TelemetryCompletion, TelemetryEpoch, TelemetryError, TelemetryHost,
    };

    #[derive(Default)]
    pub(super) struct LazyHost {
        worker: OnceLock<Result<Host, TelemetryError>>,
    }
    impl TelemetryHost for LazyHost {
        fn read(
            &self,
            epoch: TelemetryEpoch,
            completion: TelemetryCompletion,
        ) -> Result<(), TelemetryError> {
            self.worker
                .get_or_init(start)
                .as_ref()
                .map_err(|error| *error)?
                .read(epoch, completion)
        }
    }

    struct Read {
        epoch: TelemetryEpoch,
        completion: TelemetryCompletion,
        flight: Flight,
    }
    struct Host {
        sender: SyncSender<Read>,
        gate: FlightGate,
    }
    impl Host {
        fn read(
            &self,
            epoch: TelemetryEpoch,
            completion: TelemetryCompletion,
        ) -> Result<(), TelemetryError> {
            let flight = self.gate.try_enter().ok_or(TelemetryError::Busy)?;
            match self.sender.try_send(Read {
                epoch,
                completion,
                flight,
            }) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(_)) => Err(TelemetryError::Busy),
                Err(TrySendError::Disconnected(_)) => Err(TelemetryError::Stopped),
            }
        }
    }

    fn start() -> Result<Host, TelemetryError> {
        let (sender, receiver) = sync_channel::<Read>(1);
        std::thread::Builder::new()
            .name("tessera-telemetry".into())
            .spawn(move || {
                let mut native = NativeReader::default();
                // No timeout, watch, or idle poll. Sender retirement disconnects
                // only after all accepted messages have independently completed.
                while let Ok(Read {
                    epoch,
                    completion,
                    flight,
                }) = receiver.recv()
                {
                    let result = match catch_unwind(AssertUnwindSafe(|| native.read(epoch))) {
                        Ok(snapshot) => Ok(snapshot),
                        Err(_) => {
                            native = NativeReader::default();
                            Err(TelemetryError::Unavailable)
                        }
                    };
                    drop(flight);
                    let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                }
            })
            .map_err(|_| TelemetryError::Unavailable)?;
        Ok(Host {
            sender,
            gate: FlightGate::default(),
        })
    }
}
