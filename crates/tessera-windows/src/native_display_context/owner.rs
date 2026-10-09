// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One resource-ownership implementation; adapters replace only platform calls
//! and opaque tokens, never optional state, retirement ordering, or retry policy.
use super::{DisplayContextError, Driver, Monitor};
use std::marker::PhantomData;
use std::rc::Rc;

pub(super) trait Calls {
    type Dpi;
    type Window;
    type Apartment;
    type Settings;

    fn enter_dpi(&mut self) -> Result<Self::Dpi, DisplayContextError>;
    fn restore_dpi(&mut self, dpi: &mut Self::Dpi) -> Result<(), DisplayContextError>;
    /// Forget the context token after checked restoration/best-effort retry;
    /// implementations must not introduce another resource-cleanup protocol.
    fn discard_dpi(&mut self, dpi: Self::Dpi);
    fn monitors(&mut self) -> Result<Vec<Monitor>, DisplayContextError>;
    fn create_window(
        &mut self,
        selected: Monitor,
        x: i32,
        y: i32,
    ) -> Result<Self::Window, DisplayContextError>;
    fn probe_matches(
        &mut self,
        window: &Self::Window,
        selected: Monitor,
    ) -> Result<bool, DisplayContextError>;
    fn dpi(&mut self, window: &Self::Window) -> Result<u32, DisplayContextError>;
    fn destroy_window(&mut self, window: &Self::Window) -> Result<(), DisplayContextError>;
    fn initialize(&mut self) -> Result<Self::Apartment, DisplayContextError>;
    fn uninitialize(&mut self, apartment: Self::Apartment);
    fn activate_settings(&mut self) -> Result<Self::Settings, DisplayContextError>;
    fn text_scale(&mut self, settings: &Self::Settings) -> Result<f64, DisplayContextError>;
    fn release_settings(&mut self, settings: Self::Settings);
}

pub(super) struct Owner<C: Calls> {
    calls: C,
    dpi: Option<C::Dpi>,
    probe: Option<C::Window>,
    apartment: Option<C::Apartment>,
    settings: Option<C::Settings>,
    _thread: PhantomData<Rc<()>>,
}
impl<C: Calls> Owner<C> {
    pub(super) fn new(mut calls: C) -> Result<Self, DisplayContextError> {
        let dpi = calls.enter_dpi()?;
        Ok(Self {
            calls,
            dpi: Some(dpi),
            probe: None,
            apartment: None,
            settings: None,
            _thread: PhantomData,
        })
    }

    fn retire(&mut self) -> Result<(), DisplayContextError> {
        let mut error = None;
        if let Some(window) = self.probe.as_ref() {
            if let Err(failure) = self.calls.destroy_window(window) {
                error = Some(failure);
                // Best-effort retry precedes apartment/DPI retirement. Even a
                // successful retry must not turn the checked failure into success.
                let _ = self.calls.destroy_window(window);
            }
            self.probe = None;
        }
        if let Some(settings) = self.settings.take() {
            self.calls.release_settings(settings);
        }
        if let Some(apartment) = self.apartment.take() {
            self.calls.uninitialize(apartment);
        }
        if let Some(mut dpi) = self.dpi.take() {
            if let Err(failure) = self.calls.restore_dpi(&mut dpi) {
                error.get_or_insert(failure);
                let _ = self.calls.restore_dpi(&mut dpi);
            }
            self.calls.discard_dpi(dpi);
        }
        error.map_or(Ok(()), Err)
    }
}
impl<C: Calls> Driver for Owner<C> {
    fn monitors(&mut self) -> Result<Vec<Monitor>, DisplayContextError> {
        self.calls.monitors()
    }
    fn create_probe(
        &mut self,
        selected: Monitor,
        x: i32,
        y: i32,
    ) -> Result<(), DisplayContextError> {
        self.probe = Some(self.calls.create_window(selected, x, y)?);
        Ok(())
    }
    fn probe_matches(&mut self, selected: Monitor) -> Result<bool, DisplayContextError> {
        self.calls.probe_matches(
            self.probe
                .as_ref()
                .ok_or(DisplayContextError::InvalidData)?,
            selected,
        )
    }
    fn dpi(&mut self) -> Result<u32, DisplayContextError> {
        self.calls.dpi(
            self.probe
                .as_ref()
                .ok_or(DisplayContextError::InvalidData)?,
        )
    }
    fn text_scale(&mut self) -> Result<f64, DisplayContextError> {
        self.apartment = Some(self.calls.initialize()?);
        self.settings = Some(self.calls.activate_settings()?);
        let result = self.calls.text_scale(
            self.settings
                .as_ref()
                .ok_or(DisplayContextError::InvalidData)?,
        );
        self.calls.release_settings(
            self.settings
                .take()
                .ok_or(DisplayContextError::InvalidData)?,
        );
        result
    }
    fn close_probe(&mut self) -> Result<(), DisplayContextError> {
        self.calls.destroy_window(
            self.probe
                .as_ref()
                .ok_or(DisplayContextError::InvalidData)?,
        )?;
        self.probe = None;
        Ok(())
    }
    fn finish(&mut self) -> Result<(), DisplayContextError> {
        self.retire()
    }
}
impl<C: Calls> Drop for Owner<C> {
    fn drop(&mut self) {
        let _ = self.retire();
    }
}
