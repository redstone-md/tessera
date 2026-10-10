// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Opt-in native lifetime. Legacy `read` still retires its resources per flight.
use super::{
    NativeBluetoothHost,
    owner::Owner,
    radio_control::{Authorities, admit},
    winrt::{NativeCalls, native_error},
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, mpsc};
use tessera_system::bluetooth::{
    BluetoothControlReadCompletion, BluetoothError, BluetoothErrorKind, BluetoothHost,
    BluetoothRadioCommand, BluetoothRadioCompletion, BluetoothRadioControlHost,
    BluetoothReadCompletion,
};
use windows::Devices::Radios::{Radio, RadioAccessStatus};
use windows::Win32::System::Com::{
    APTTYPE, APTTYPE_MAINSTA, APTTYPE_STA, APTTYPEQUALIFIER, CoGetApartmentType,
};

type NativeOwner = Owner<NativeCalls>;
type Work = Box<dyn FnOnce(&mut Option<NativeOwner>) + Send + 'static>;

#[derive(Default)]
pub(crate) struct ControlledBluetoothHost {
    legacy: NativeBluetoothHost,
    worker: Mutex<Option<mpsc::Sender<Work>>>,
    authorities: Authorities,
    access: Arc<Mutex<Option<RadioAccessStatus>>>,
}

impl ControlledBluetoothHost {
    fn submit(&self, work: Work) -> Result<(), BluetoothError> {
        let mut worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if worker.is_none() {
            let (sender, receiver) = mpsc::channel::<Work>();
            std::thread::Builder::new()
                .name("tessera-bluetooth-owner".into())
                .spawn(move || {
                    let mut owner = None;
                    while let Ok(work) = receiver.recv() {
                        work(&mut owner);
                    }
                    // No UI join. Event tokens/radios retire before apartment.
                    drop(owner);
                })
                .map_err(|_| unavailable("Bluetooth control owner could not start"))?;
            *worker = Some(sender);
        }
        // Shared admission allows at most one accepted work item, not a backlog.
        if worker
            .as_ref()
            .is_some_and(|sender| sender.send(work).is_ok())
        {
            Ok(())
        } else {
            *worker = None;
            Err(unavailable("Bluetooth control owner has retired"))
        }
    }
}

impl BluetoothHost for ControlledBluetoothHost {
    fn read(&self, completion: BluetoothReadCompletion) -> Result<(), BluetoothError> {
        self.legacy.read(completion)
    }

    fn radio_controls(&self) -> Option<&dyn BluetoothRadioControlHost> {
        Some(self)
    }
}

impl BluetoothRadioControlHost for ControlledBluetoothHost {
    fn read_controls(
        &self,
        completion: BluetoothControlReadCompletion,
    ) -> Result<(), BluetoothError> {
        let flight = self.legacy.gate.try_enter().ok_or_else(busy)?;
        let authorities = self.authorities.clone();
        self.submit(Box::new(move |owner| {
            let result = execute(owner, authorities, |owner| Ok(owner.read_controls()));
            drop(flight);
            let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
        }))
    }

    fn set_radio(
        &self,
        command: BluetoothRadioCommand,
        completion: BluetoothRadioCompletion,
    ) -> Result<(), BluetoothError> {
        let flight = self.legacy.gate.try_enter().ok_or_else(busy)?;
        admit(&self.authorities, &command)?;
        // The GUI owns the consent-capable apartment for its event-loop lifetime.
        // Do not initialize/tear down a temporary UI apartment or request access
        // on the background read owner. This method never waits on the GUI.
        let mut apartment = APTTYPE::default();
        let mut qualifier = APTTYPEQUALIFIER::default();
        unsafe { CoGetApartmentType(&mut apartment, &mut qualifier) }.map_err(|error| {
            native_error(
                error,
                "Bluetooth consent requires an initialized GUI apartment",
            )
        })?;
        if apartment != APTTYPE_STA && apartment != APTTYPE_MAINSTA {
            return Err(unavailable(
                "Bluetooth consent requires the current GUI apartment",
            ));
        }
        let cached = *self
            .access
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let permission = if cached.is_none() {
            Some(Radio::RequestAccessAsync().map_err(|error| {
                native_error(error, "Bluetooth radio permission request could not start")
            })?)
        } else {
            None
        };
        let authorities = self.authorities.clone();
        let access = self.access.clone();
        self.submit(Box::new(move |owner| {
            let result = execute(owner, authorities, |owner| {
                let permission = match permission {
                    Some(operation) => operation.join().map_err(|error| {
                        native_error(error, "Bluetooth radio permission request failed")
                    })?,
                    None => cached
                        .ok_or_else(|| unavailable("Bluetooth radio permission is unavailable"))?,
                };
                *access
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    (permission == RadioAccessStatus::Allowed).then_some(permission);
                owner.calls_mut().controls.set(command, permission)
            });
            if result
                .as_ref()
                .is_err_and(|error| error.kind == BluetoothErrorKind::AccessDenied)
                || result.as_ref().is_ok_and(|outcome| {
                    outcome.access != tessera_system::bluetooth::BluetoothRadioAccess::Allowed
                })
            {
                *access
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }
            drop(flight);
            let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
        }))
    }
}

fn execute<T>(
    owner: &mut Option<NativeOwner>,
    authorities: Authorities,
    operation: impl FnOnce(&mut NativeOwner) -> Result<T, BluetoothError>,
) -> Result<T, BluetoothError> {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if owner.is_none() {
            *owner = Some(Owner::new(NativeCalls::with_authorities(authorities))?);
        }
        operation(owner.as_mut().expect("initialized Bluetooth owner"))
    }));
    match result {
        Ok(result) => result,
        Err(_) => {
            // A panicked native owner is never reused with surviving authority.
            let _ = catch_unwind(AssertUnwindSafe(|| drop(owner.take())));
            Err(unavailable("Bluetooth control owner panicked"))
        }
    }
}

fn busy() -> BluetoothError {
    BluetoothError::new(
        BluetoothErrorKind::Busy,
        "A Bluetooth observation or command is already pending",
    )
}

fn unavailable(message: &'static str) -> BluetoothError {
    BluetoothError::new(BluetoothErrorKind::Unavailable, message)
}
