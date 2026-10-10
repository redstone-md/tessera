// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::ffi::c_void;
use std::sync::Arc;

use tessera_system::network::{
    InterfaceId, NetworkCommand, NetworkCommandOutcome, NetworkControlView, NetworkError,
    NetworkErrorKind, NetworkSnapshot, NetworkView, Observation, WifiInterface,
};
use windows::Win32::NetworkManagement::WiFi::{
    WLAN_NOTIFICATION_SOURCE_ACM, WLAN_NOTIFICATION_SOURCE_NONE,
};

use super::callback::{Context, notification};
use super::calls::{NativeCalls, invalid, native_error};
use super::observations;

pub(super) struct Owner<C: NativeCalls> {
    calls: C,
    handle: Option<usize>,
    context: Option<Box<Context>>,
    controls: super::controls::Controls,
}

impl<C: NativeCalls> Owner<C> {
    pub(super) fn new(calls: C) -> Self {
        Self {
            calls,
            handle: None,
            context: None,
            controls: super::controls::Controls::new(),
        }
    }

    fn handle(&mut self) -> Result<usize, NetworkError> {
        if let Some(handle) = self.handle {
            return Ok(handle);
        }
        let (status, negotiated, handle) = self.calls.open(2);
        if status != 0 || negotiated < 2 || handle == 0 {
            // Even anomalous non-null error out handles have one close attempt.
            if handle != 0 {
                let close = self.calls.close(handle);
                if close != 0 {
                    self.calls
                        .retirement_error(&native_error("WLAN client close", close));
                }
            }
            return Err(if status != 0 {
                native_error("WLAN client v2 open", status)
            } else if negotiated < 2 {
                NetworkError::new(
                    NetworkErrorKind::Unsupported,
                    "WLAN client version 2 is unavailable",
                )
            } else {
                invalid("WLAN returned a null successful client handle")
            });
        }
        self.handle = Some(handle);
        Ok(handle)
    }

    pub(super) fn read(&mut self) -> Result<NetworkSnapshot, NetworkError> {
        let handle = self.handle()?;
        let mut interfaces = observations::interfaces(&self.calls, handle)?
            .into_iter()
            .map(|entry| {
                let id = entry.InterfaceGuid;
                let length = entry
                    .strInterfaceDescription
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(entry.strInterfaceDescription.len());
                WifiInterface {
                    id: InterfaceId::new(id.to_u128().to_be_bytes()),
                    name: String::from_utf16_lossy(&entry.strInterfaceDescription[..length]),
                    radio: observation(observations::radio(&self.calls, handle, &id)),
                    discovery: observation(observations::discovery(
                        &self.calls,
                        handle,
                        &id,
                        entry.isState,
                    )),
                }
            })
            .collect::<Vec<_>>();
        interfaces.sort_by_key(|interface| interface.id);
        Ok(NetworkSnapshot { interfaces })
    }

    pub(super) fn read_view(&mut self) -> Result<NetworkView, NetworkError> {
        let snapshot = self.read()?;
        let handle = self.handle()?;
        let controls = observation(self.controls.inventory(
            &self.calls,
            handle,
            self.context.as_deref(),
            &snapshot,
        ));
        Ok(NetworkView {
            snapshot,
            controls: Some(controls),
        })
    }

    pub(super) fn read_control_view(&mut self) -> Result<NetworkControlView, NetworkError> {
        let view = self.read_view()?;
        let handle = self.handle()?;
        let radios = observation(self.controls.radio_inventory(
            &self.calls,
            handle,
            self.context.as_deref(),
        ));
        let mut result = NetworkControlView::from(view);
        result.radios = Some(radios);
        result.profiles = Some(observation(self.controls.profile_inventory(
            &self.calls,
            handle,
            self.context.as_deref(),
        )));
        Ok(result)
    }

    pub(super) fn command(&mut self, command: NetworkCommand) -> Result<(), NetworkError> {
        let handle = self.handle()?;
        self.controls
            .begin(&self.calls, handle, self.context.as_deref(), command)
    }

    pub(super) fn command_accepted(&self) -> bool {
        self.controls.accepted()
    }

    pub(super) fn command_readback(
        &mut self,
    ) -> Result<Option<NetworkCommandOutcome>, NetworkError> {
        let handle = self.handle()?;
        self.controls
            .progress(&self.calls, handle, self.context.as_deref())
    }

    pub(super) fn command_retired(&mut self) {
        self.controls.retire();
        if let Some(context) = self.context.as_deref() {
            context.retire_effect();
        }
    }

    pub(super) fn open_settings(&mut self) -> Result<(), NetworkError> {
        // Settings remains independent of WLAN service/client/permission failure.
        self.calls.open_settings()
    }

    pub(super) fn register(
        &mut self,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), NetworkError> {
        if self.context.is_some() {
            return Err(NetworkError::new(
                NetworkErrorKind::Busy,
                "WLAN callback context has not retired",
            ));
        }
        let handle = self.handle()?;
        self.context = Some(Box::new(Context::new(wake)));
        let context = self
            .context
            .as_deref()
            .expect("newly installed WLAN context") as *const Context;
        let status = self.calls.register(
            handle,
            WLAN_NOTIFICATION_SOURCE_ACM.0,
            Some(notification),
            context.cast::<c_void>(),
        );
        if status == 0 {
            return Ok(());
        }
        // Treat even failed registration conservatively: the seam may have
        // published the pointer before reporting failure. NONE proves retirement.
        if let Err(error) = self.unregister() {
            self.calls.retirement_error(&error);
        }
        Err(native_error("WLAN ACM watch registration", status))
    }

    pub(super) fn unregister(&mut self) -> Result<(), NetworkError> {
        let Some(context) = self.context.as_deref() else {
            return Ok(());
        };
        context.close();
        let Some(handle) = self.handle else {
            return Err(invalid("WLAN callback context has no client handle"));
        };
        // This call is a worker-only barrier. No callback-needed locks are held.
        let status = self.calls.register(
            handle,
            WLAN_NOTIFICATION_SOURCE_NONE.0,
            None,
            std::ptr::null(),
        );
        if status != 0 {
            // Keep the context pinned and closed; do not replace/reuse it.
            return Err(native_error("WLAN watch retirement", status));
        }
        self.context.take();
        Ok(())
    }
}

fn observation<T>(result: Result<T, NetworkError>) -> Observation<T> {
    match result {
        Ok(value) => Observation::Ready(value),
        Err(error) => Observation::Unavailable(error),
    }
}

impl<C: NativeCalls> Drop for Owner<C> {
    fn drop(&mut self) {
        if let Err(error) = self.unregister() {
            self.calls.retirement_error(&error);
        }
        if let Some(handle) = self.handle.take() {
            let status = self.calls.close(handle);
            if status != 0 {
                self.calls
                    .retirement_error(&native_error("WLAN client close", status));
            }
        }
        // Close undoes registration but its documentation does not establish the
        // NONE active-callback barrier. If that barrier failed, retain reachable
        // memory even on successful close. Never retry a close on an ambiguous
        // failure. The redacted error above records this exceptional leak.
        if let Some(context) = self.context.take() {
            context.close();
            std::mem::forget(context);
        }
    }
}
