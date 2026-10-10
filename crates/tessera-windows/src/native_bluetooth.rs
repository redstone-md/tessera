// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Legacy scoped read flights; opt-in control retains one native owner separately.
use crate::single_flight::FlightGate;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use tessera_system::bluetooth::{
    BluetoothError, BluetoothErrorKind, BluetoothHost, BluetoothReadCompletion, BluetoothSnapshot,
};

#[cfg(windows)]
mod control;
mod owner;
#[cfg(windows)]
mod radio_control;
#[cfg(windows)]
mod radio_source;
#[cfg(test)]
mod tests;
#[cfg(windows)]
mod winrt;

#[cfg(windows)]
pub(crate) use control::ControlledBluetoothHost;

type ReadResult = Result<BluetoothSnapshot, BluetoothError>;
type Job = Box<dyn FnOnce() + Send + 'static>;

// Driver is deliberately not Send: factory invocation and all native ownership
// stay on the accepted worker, while the host carries only the inert factory.
trait Driver {
    fn read(&mut self) -> BluetoothSnapshot;
}

type DriverFactory = dyn Fn() -> Result<Box<dyn Driver>, BluetoothError> + Send + Sync;
type Spawn = dyn Fn(Job) -> std::io::Result<()> + Send + Sync;

#[derive(Clone)]
pub(crate) struct NativeBluetoothHost {
    gate: FlightGate,
    factory: Arc<DriverFactory>,
    spawn: Arc<Spawn>,
}

#[cfg(windows)]
impl Default for NativeBluetoothHost {
    fn default() -> Self {
        Self {
            gate: FlightGate::default(),
            factory: Arc::new(|| Ok(Box::new(owner::Owner::new(winrt::NativeCalls::default())?))),
            spawn: Arc::new(|job| {
                std::thread::Builder::new()
                    .name("tessera-bluetooth-read".into())
                    .spawn(job)
                    .map(|_| ())
            }),
        }
    }
}

impl BluetoothHost for NativeBluetoothHost {
    fn read(&self, completion: BluetoothReadCompletion) -> Result<(), BluetoothError> {
        let flight = self.gate.try_enter().ok_or_else(|| {
            BluetoothError::new(
                BluetoothErrorKind::Busy,
                "A Bluetooth read is already pending",
            )
        })?;
        let factory = self.factory.clone();
        // Failed spawn drops the unrun job: flight retires and callback is never
        // invoked. Successful spawn detaches; host/UI drop never joins workers.
        catch_unwind(AssertUnwindSafe(|| {
            (self.spawn)(Box::new(move || {
                let result: ReadResult = catch_unwind(AssertUnwindSafe(|| {
                    let mut owner = factory()?;
                    Ok(owner.read())
                    // Scoped SDK resources, then Owner's apartment, retire
                    // inside this catch before releasing admission or callback.
                }))
                .unwrap_or_else(|_| {
                    Err(BluetoothError::new(
                        BluetoothErrorKind::Unavailable,
                        "Bluetooth request owner panicked",
                    ))
                });
                drop(flight);
                let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
            }))
        }))
        .unwrap_or_else(|_| Err(std::io::Error::other("Bluetooth worker spawn panicked")))
        .map_err(|_| {
            BluetoothError::new(
                BluetoothErrorKind::Unavailable,
                "Bluetooth worker could not start",
            )
        })
    }
}
