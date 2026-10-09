// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Adapters replace raw calls and opaque resources, never admission/retry/lifetime policy.
use super::{AdmittedMonitor, DisplayContextError, Driver, Monitor, probe_center};
use std::marker::PhantomData;
use std::rc::Rc;

const INSUFFICIENT_BUFFER: u32 = 122;
pub(super) const SNAPSHOT_LIMIT: usize = 4096;
const SNAPSHOT_ATTEMPTS: usize = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TargetKey {
    pub low: u32,
    pub high: i32,
    pub id: u32,
}

pub(super) trait Calls {
    type Dpi;
    type Window;
    type Apartment;
    type Settings;
    type Manager;
    type Targets;
    type Path: Default + Clone;
    type Mode: Default + Clone;

    fn enter_dpi(&mut self) -> Result<Self::Dpi, DisplayContextError>;
    fn restore_dpi(&mut self, dpi: &mut Self::Dpi) -> Result<(), DisplayContextError>;
    fn discard_dpi(&mut self, dpi: Self::Dpi);
    fn monitors(&mut self) -> Result<Vec<Monitor>, DisplayContextError>;
    fn config_sizes(&mut self) -> Result<(u32, u32), DisplayContextError>;
    /// Returned status, not last-error, is authoritative. Counts/outputs on error are unused.
    fn config_query(
        &mut self,
        paths: &mut [Self::Path],
        modes: &mut [Self::Mode],
    ) -> Result<(u32, u32), DisplayContextError>;
    fn source_index(path: &Self::Path) -> u32;
    fn source_position(mode: &Self::Mode) -> Option<(i32, i32)>;
    fn path_target(path: &Self::Path) -> TargetKey;
    fn activate_manager(&mut self) -> Result<Self::Manager, DisplayContextError>;
    fn current_targets(
        &mut self,
        manager: &Self::Manager,
    ) -> Result<Self::Targets, DisplayContextError>;
    fn target_count(&mut self, targets: &Self::Targets) -> Result<u32, DisplayContextError>;
    fn target_adapter(
        &mut self,
        targets: &Self::Targets,
        index: u32,
    ) -> Result<(u32, i32), DisplayContextError>;
    fn target_id(
        &mut self,
        targets: &Self::Targets,
        index: u32,
    ) -> Result<u32, DisplayContextError>;
    fn stable_id(
        &mut self,
        targets: &Self::Targets,
        index: u32,
    ) -> Result<String, DisplayContextError>;
    fn release_targets(&mut self, targets: Self::Targets);
    fn release_manager(&mut self, manager: Self::Manager);
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

type CurrentConfig<C> = (Vec<<C as Calls>::Path>, Vec<<C as Calls>::Mode>);

pub(super) struct Owner<C: Calls> {
    calls: C,
    dpi: Option<C::Dpi>,
    probe: Option<C::Window>,
    apartment: Option<C::Apartment>,
    settings: Option<C::Settings>,
    manager: Option<C::Manager>,
    targets: Option<C::Targets>,
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
            manager: None,
            targets: None,
            _thread: PhantomData,
        })
    }

    fn ensure_apartment(&mut self) -> Result<(), DisplayContextError> {
        if self.apartment.is_none() {
            self.apartment = Some(self.calls.initialize()?);
        }
        Ok(())
    }

    fn current_config(&mut self) -> Result<CurrentConfig<C>, DisplayContextError> {
        for _ in 0..SNAPSHOT_ATTEMPTS {
            let (path_count, mode_count) = self.calls.config_sizes()?;
            if path_count as usize > SNAPSHOT_LIMIT || mode_count as usize > SNAPSHOT_LIMIT {
                return Err(DisplayContextError::InvalidData);
            }
            // At least one allocated slot keeps native array pointers valid even for zero counts.
            let mut paths = vec![C::Path::default(); (path_count as usize).max(1)];
            let mut modes = vec![C::Mode::default(); (mode_count as usize).max(1)];
            match self.calls.config_query(
                &mut paths[..path_count as usize],
                &mut modes[..mode_count as usize],
            ) {
                Err(DisplayContextError::Native {
                    code: INSUFFICIENT_BUFFER,
                }) => continue,
                Err(error) => return Err(error),
                Ok((used_paths, used_modes)) => {
                    if used_paths > path_count || used_modes > mode_count {
                        return Err(DisplayContextError::InvalidData);
                    }
                    paths.truncate(used_paths as usize);
                    modes.truncate(used_modes as usize);
                    return Ok((paths, modes));
                }
            }
        }
        Err(DisplayContextError::Native {
            code: INSUFFICIENT_BUFFER,
        })
    }

    fn lookup_target(
        &mut self,
        key: TargetKey,
        count: u32,
    ) -> Result<Option<String>, DisplayContextError> {
        let targets = self
            .targets
            .as_ref()
            .ok_or(DisplayContextError::InvalidData)?;
        // A failing property stops this path's lookup, not the global target list or other paths.
        for index in 0..count {
            if self.calls.target_adapter(targets, index)? == (key.low, key.high)
                && self.calls.target_id(targets, index)? == key.id
            {
                return self.calls.stable_id(targets, index).map(Some);
            }
        }
        Ok(None)
    }

    fn candidate_scale(&mut self, monitor: Monitor) -> Result<f64, DisplayContextError> {
        let (x, y) = probe_center(monitor.bounds)?;
        self.create_probe(monitor, x, y)?;
        if !self.probe_matches(monitor)? {
            return Err(DisplayContextError::InvalidData);
        }
        let dpi = self.dpi()?;
        if dpi == 0 {
            return Err(DisplayContextError::InvalidData);
        }
        let text = self.text_scale()?;
        let scale = f64::from(dpi) / 96.0 * text;
        if !text.is_finite() || text <= 0.0 || !scale.is_finite() || scale <= 0.0 {
            return Err(DisplayContextError::InvalidData);
        }
        Ok(scale)
    }

    pub(super) fn create_probe(
        &mut self,
        selected: Monitor,
        x: i32,
        y: i32,
    ) -> Result<(), DisplayContextError> {
        if self.probe.is_some() {
            return Err(DisplayContextError::InvalidData);
        }
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
    pub(super) fn text_scale(&mut self) -> Result<f64, DisplayContextError> {
        self.ensure_apartment()?;
        if self.settings.is_none() {
            self.settings = Some(self.calls.activate_settings()?);
        }
        self.calls.text_scale(
            self.settings
                .as_ref()
                .ok_or(DisplayContextError::InvalidData)?,
        )
    }
    pub(super) fn close_probe(&mut self) -> Result<(), DisplayContextError> {
        self.calls.destroy_window(
            self.probe
                .as_ref()
                .ok_or(DisplayContextError::InvalidData)?,
        )?;
        self.probe = None;
        Ok(())
    }

    fn retire(&mut self) -> Result<(), DisplayContextError> {
        let mut error = None;
        if let Some(window) = self.probe.as_ref() {
            if let Err(failure) = self.calls.destroy_window(window) {
                error = Some(failure);
                let _ = self.calls.destroy_window(window);
            }
            self.probe = None;
        }
        if let Some(settings) = self.settings.take() {
            self.calls.release_settings(settings);
        }
        if let Some(targets) = self.targets.take() {
            self.calls.release_targets(targets);
        }
        if let Some(manager) = self.manager.take() {
            self.calls.release_manager(manager);
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
    fn monitors(&mut self) -> Result<Vec<AdmittedMonitor>, DisplayContextError> {
        let raw = self.calls.monitors()?;
        let (paths, modes) = self.current_config()?;
        self.ensure_apartment()?;
        self.manager = Some(self.calls.activate_manager()?);
        self.targets = Some(
            self.calls.current_targets(
                self.manager
                    .as_ref()
                    .ok_or(DisplayContextError::InvalidData)?,
            )?,
        );
        let count = self.calls.target_count(
            self.targets
                .as_ref()
                .ok_or(DisplayContextError::InvalidData)?,
        )?;
        if count as usize > SNAPSHOT_LIMIT {
            return Err(DisplayContextError::InvalidData);
        }
        let mut admitted = Vec::new();
        for monitor in raw {
            let mut stable_id = None;
            for path in &paths {
                let Some(mode) = modes.get(C::source_index(path) as usize) else {
                    continue;
                };
                if C::source_position(mode) != Some((monitor.bounds.x(), monitor.bounds.y())) {
                    continue;
                }
                if let Ok(Some(id)) = self.lookup_target(C::path_target(path), count) {
                    stable_id = Some(id);
                    break;
                }
            }
            let Some(stable_id) = stable_id else { continue };
            let scale = self.candidate_scale(monitor);
            // Checked destruction is infrastructure, never a projection exclusion.
            // Also closes failed/vanished association and partial scale queries before the next probe.
            if self.probe.is_some() {
                self.close_probe()?;
            }
            if let Ok(scale) = scale {
                admitted.push(AdmittedMonitor {
                    monitor,
                    stable_id,
                    scale,
                });
            }
        }
        Ok(admitted)
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
