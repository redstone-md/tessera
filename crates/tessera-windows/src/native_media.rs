// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! GSMTC owner-thread adapter. Only portable owned facts leave the actor thread.
//! The production owner is also exercised through recorded calls without COM.

mod artwork;
mod owner;
#[cfg(windows)]
mod sdk;
#[cfg(all(test, windows))]
mod sdk_tests;
#[cfg(test)]
mod tests;

#[cfg(windows)]
pub(crate) struct NativeMediaDriver(owner::Owner<sdk::WindowsCalls>);

#[cfg(windows)]
impl NativeMediaDriver {
    pub(crate) fn new() -> Result<Self, tessera_system::media::MediaError> {
        Ok(Self(owner::Owner::new(sdk::WindowsCalls::new()?)))
    }
}

#[cfg(windows)]
impl crate::media::actor::Driver for NativeMediaDriver {
    fn read(
        &mut self,
    ) -> Result<tessera_system::media::MediaSnapshot, tessera_system::media::MediaError> {
        self.0.read()
    }

    fn execute(
        &mut self,
        command: tessera_system::media::MediaRequest,
    ) -> Result<(), tessera_system::media::MediaError> {
        self.0.execute(command)
    }

    fn start_watch(
        &mut self,
        dirty: std::sync::Arc<dyn Fn(tessera_system::media::MediaEvent) + Send + Sync>,
    ) -> Result<(), tessera_system::media::MediaError> {
        self.0.start_watch(dirty)
    }

    fn refresh_watch(&mut self) -> Result<(), tessera_system::media::MediaError> {
        self.0.refresh_watch()
    }

    fn take_watch_failure(&mut self) -> Option<tessera_system::media::MediaError> {
        self.0.take_watch_failure()
    }

    fn stop_watch(&mut self) {
        self.0.stop_watch();
    }
}
