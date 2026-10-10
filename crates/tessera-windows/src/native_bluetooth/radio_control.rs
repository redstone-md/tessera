// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Exact native IDs bind retained COM incarnations; fresh queries never retarget.
use super::radio_source::{SourceRevision, SourceWatcher, invalidate};
use super::winrt::native_error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tessera_system::bluetooth::{
    BluetoothControlledRadio, BluetoothError, BluetoothErrorKind, BluetoothRadioAccess,
    BluetoothRadioCommand, BluetoothRadioKey, BluetoothRadioObservation, BluetoothRadioOutcome,
    BluetoothRadioPower, BluetoothRadioState,
};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
use windows::Foundation::TypedEventHandler;
use windows::core::{HSTRING, IInspectable, IUnknown, Interface};

/// Inert admission mirror; no native interface or ID is accessible from the GUI.
pub(super) struct RadioAuthority {
    captured: BluetoothControlledRadio,
    revision: Arc<AtomicU64>,
    source: Arc<SourceRevision>,
    source_revision: u64,
}

pub(super) type Authorities = Arc<Mutex<Vec<RadioAuthority>>>;

pub(super) fn admit(
    authorities: &Authorities,
    command: &BluetoothRadioCommand,
) -> Result<(), BluetoothError> {
    let authorities = authorities
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if command
        .radio
        .revision
        .checked_add(1)
        .is_some_and(|revision| revision != u64::MAX)
        && authorities.iter().any(|source| {
            source.captured == command.radio
                && source.revision.load(Ordering::Acquire) == command.radio.revision
                && source.source.current(source.source_revision)
                && matches!(
                    source.captured.observation.state,
                    BluetoothRadioState::On | BluetoothRadioState::Off
                )
        })
    {
        Ok(())
    } else {
        Err(stale())
    }
}

struct RadioBinding {
    id: HSTRING,
    radio: Radio,
    canonical: IUnknown,
}

struct RetainedRadio {
    binding: RadioBinding,
    captured: BluetoothControlledRadio,
    revision: Arc<AtomicU64>,
    token: i64,
}

impl Drop for RetainedRadio {
    fn drop(&mut self) {
        invalidate(&self.revision);
        let _ = self.binding.radio.RemoveStateChanged(self.token);
    }
}

#[derive(Default)]
pub(super) struct RadioInventory {
    active: bool,
    radios: Vec<RetainedRadio>,
    pending: Vec<RadioBinding>,
    failure: Option<BluetoothError>,
    authorities: Authorities,
    selector: Option<HSTRING>,
    watch: Option<SourceWatcher>,
    source_revision: Option<u64>,
}

impl RadioInventory {
    pub(super) fn with_authorities(authorities: Authorities) -> Self {
        Self {
            active: false,
            radios: Vec::new(),
            pending: Vec::new(),
            failure: None,
            authorities,
            selector: None,
            watch: None,
            source_revision: None,
        }
    }

    pub(super) fn begin(&mut self) {
        self.retire();
        self.active = true;
    }

    pub(super) fn active(&self) -> bool {
        self.active
    }

    /// The modern read's radio acquisition, not a second GetRadios pipeline.
    pub(super) fn enumerate(&mut self) -> Result<Vec<Radio>, BluetoothError> {
        let selector = Radio::GetDeviceSelector()
            .map_err(|error| native_error(error, "Bluetooth radio selector is unavailable"))?;
        match SourceWatcher::new(&selector) {
            Ok(watch) => match watch.capture() {
                Ok(revision) => {
                    self.source_revision = Some(revision);
                    self.watch = Some(watch);
                }
                Err(error) => self.failure = Some(error),
            },
            Err(error) => self.failure = Some(error),
        }
        // Watch failure disables effects, not independently readable inventory.
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .and_then(|operation| operation.join())
            .map_err(|error| native_error(error, "Bluetooth radio source enumeration failed"))?;
        let count = devices
            .Size()
            .map_err(|error| native_error(error, "Bluetooth radio source count is unavailable"))?;
        let mut radios = Vec::new();
        for index in 0..count {
            let id = devices
                .GetAt(index)
                .and_then(|device| device.Id())
                .map_err(|error| {
                    native_error(error, "Bluetooth radio source identity is unavailable")
                })?;
            if id.is_empty() || self.pending.iter().any(|binding| binding.id == id) {
                return Err(stale());
            }
            let radio = Radio::FromIdAsync(&id)
                .and_then(|operation| operation.join())
                .map_err(|error| {
                    native_error(error, "Bluetooth radio source could not be acquired")
                })?;
            let canonical = radio.cast::<IUnknown>().map_err(|error| {
                native_error(error, "Bluetooth radio incarnation is unavailable")
            })?;
            radios.push(radio.clone());
            self.pending.push(RadioBinding {
                id,
                radio,
                canonical,
            });
        }
        self.selector = Some(selector);
        Ok(radios)
    }

    pub(super) fn observe(&mut self, radio: &Radio, observation: &BluetoothRadioObservation) {
        if !self.active || self.failure.is_some() {
            return;
        }
        let result = (|| {
            // This comparison is only between references from this one native
            // acquisition. It assumes no identity stability across enumerations.
            let canonical = radio.cast::<IUnknown>().map_err(|error| {
                native_error(error, "Bluetooth radio incarnation is unavailable")
            })?;
            let index = self
                .pending
                .iter()
                .position(|binding| binding.canonical == canonical)
                .ok_or_else(stale)?;
            let binding = self.pending.remove(index);
            if self
                .pending
                .iter()
                .any(|other| other.canonical == canonical)
                || self
                    .radios
                    .iter()
                    .any(|other| other.binding.canonical == canonical)
            {
                return Err(stale());
            }
            Self::retain(binding, observation)
        })();
        match result {
            Ok(retained) => {
                let Some(watch) = self.watch.as_ref() else {
                    self.failure = Some(stale());
                    return;
                };
                let Some(source_revision) = self.source_revision else {
                    self.failure = Some(stale());
                    return;
                };
                self.authorities
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(RadioAuthority {
                        captured: retained.captured.clone(),
                        revision: retained.revision.clone(),
                        source: watch.source.clone(),
                        source_revision,
                    });
                self.radios.push(retained);
            }
            Err(error) => self.failure = Some(error),
        }
    }

    fn retain(
        binding: RadioBinding,
        observation: &BluetoothRadioObservation,
    ) -> Result<RetainedRadio, BluetoothError> {
        let revision = Arc::new(AtomicU64::new(0));
        let invalidation = revision.clone();
        let handler = TypedEventHandler::<Radio, IInspectable>::new(move |_, _| {
            invalidate(&invalidation);
            Ok(())
        });
        let token = binding
            .radio
            .StateChanged(&handler)
            .map_err(|error| native_error(error, "Bluetooth radio state subscription failed"))?;
        let mut retained = RetainedRadio {
            binding,
            captured: BluetoothControlledRadio {
                key: BluetoothRadioKey::issue(),
                revision: 0,
                observation: observation.clone(),
            },
            revision,
            token,
        };
        let before = retained.revision.load(Ordering::Acquire);
        retained.captured.observation.state = state(&retained.binding.radio)?;
        if before == u64::MAX || retained.revision.load(Ordering::Acquire) != before {
            return Err(stale());
        }
        retained.captured.revision = before;
        Ok(retained)
    }

    pub(super) fn finish(
        &mut self,
        result: &Result<Vec<BluetoothRadioObservation>, BluetoothError>,
    ) -> Result<Vec<BluetoothControlledRadio>, BluetoothError> {
        self.pending.clear();
        let error = result
            .as_ref()
            .err()
            .cloned()
            .or_else(|| self.failure.take());
        let validation = self
            .watch
            .as_ref()
            .ok_or_else(stale)
            .and_then(|watch| watch.validate(self.source_revision.ok_or_else(stale)?));
        if let Some(error) = error.or_else(|| validation.err()) {
            self.retire();
            return Err(error);
        }
        if self.radios.iter().any(|radio| {
            radio.captured.revision == u64::MAX
                || radio.revision.load(Ordering::Acquire) != radio.captured.revision
        }) {
            self.retire();
            return Err(stale());
        }
        Ok(self
            .radios
            .iter()
            .map(|radio| radio.captured.clone())
            .collect())
    }

    pub(super) fn retire(&mut self) {
        self.active = false;
        self.watch.take();
        self.radios.clear();
        self.pending.clear();
        self.authorities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.selector = None;
        self.source_revision = None;
        self.failure = None;
    }

    pub(super) fn set(
        &mut self,
        command: BluetoothRadioCommand,
        permission: RadioAccessStatus,
    ) -> Result<BluetoothRadioOutcome, BluetoothError> {
        let retained = self
            .radios
            .iter()
            .find(|radio| radio.captured.key == command.radio.key)
            .ok_or_else(stale)?;
        if command.radio.revision == u64::MAX
            || retained.captured != command.radio
            || retained.revision.load(Ordering::Acquire) != command.radio.revision
        {
            return Err(stale());
        }
        let watch = self.watch.as_ref().ok_or_else(stale)?;
        let source_revision = self.source_revision.ok_or_else(stale)?;
        watch.validate(source_revision)?;
        let selector = self.selector.as_ref().ok_or_else(stale)?;
        // Fresh exact-ID presence after consent; never acquire a replacement
        // Radio here. Source removal/readdition/update revokes the retained one.
        let devices = DeviceInformation::FindAllAsyncAqsFilter(selector)
            .and_then(|operation| operation.join())
            .map_err(|error| native_error(error, "Bluetooth radio source revalidation failed"))?;
        let count = devices
            .Size()
            .map_err(|error| native_error(error, "Bluetooth radio source revalidation failed"))?;
        let mut found = false;
        for index in 0..count {
            let id = devices
                .GetAt(index)
                .and_then(|device| device.Id())
                .map_err(|error| {
                    native_error(error, "Bluetooth radio source revalidation failed")
                })?;
            if id == retained.binding.id {
                if found {
                    return Err(stale());
                }
                found = true;
            }
        }
        if !found {
            return Err(stale());
        }
        let radio = &retained.binding.radio;
        if radio
            .cast::<IUnknown>()
            .map_err(|error| native_error(error, "Bluetooth radio incarnation is unavailable"))?
            != retained.binding.canonical
            || radio
                .Kind()
                .map_err(|error| native_error(error, "Bluetooth radio kind is unavailable"))?
                != RadioKind::Bluetooth
        {
            return Err(stale());
        }
        let current = state(radio)?;
        if current != command.radio.observation.state
            || !matches!(current, BluetoothRadioState::On | BluetoothRadioState::Off)
            || retained.revision.load(Ordering::Acquire) != command.radio.revision
        {
            return Err(stale());
        }
        watch.validate(source_revision)?;
        let mut access = permission;
        if permission == RadioAccessStatus::Allowed {
            let consumed = command
                .radio
                .revision
                .checked_add(1)
                .filter(|revision| *revision != u64::MAX)
                .ok_or_else(stale)?;
            retained
                .revision
                .compare_exchange(
                    command.radio.revision,
                    consumed,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .map_err(|_| stale())?;
            // A last source-stamp check precedes the effect on the retained
            // incarnation. No fresh wrapper/name/index ever becomes authority.
            watch.validate(source_revision)?;
            let target = match command.power {
                BluetoothRadioPower::On => RadioState::On,
                BluetoothRadioPower::Off => RadioState::Off,
            };
            access = radio
                .SetStateAsync(target)
                .and_then(|operation| operation.join())
                .map_err(|error| native_error(error, "Bluetooth radio power request failed"))?;
        }
        Ok(BluetoothRadioOutcome {
            access: access_status(access),
            observed: state(radio).map(|state| BluetoothRadioObservation {
                name: retained.captured.observation.name.clone(),
                state,
            }),
        })
    }
}

impl Drop for RadioInventory {
    fn drop(&mut self) {
        self.retire();
    }
}

fn state(radio: &Radio) -> Result<BluetoothRadioState, BluetoothError> {
    radio
        .State()
        .map(|value| match value {
            RadioState::On => BluetoothRadioState::On,
            RadioState::Off => BluetoothRadioState::Off,
            RadioState::Disabled => BluetoothRadioState::Disabled,
            _ => BluetoothRadioState::Unknown,
        })
        .map_err(|error| native_error(error, "Bluetooth radio state is unavailable"))
}

fn access_status(access: RadioAccessStatus) -> BluetoothRadioAccess {
    match access {
        RadioAccessStatus::Allowed => BluetoothRadioAccess::Allowed,
        RadioAccessStatus::DeniedByUser => BluetoothRadioAccess::DeniedByUser,
        RadioAccessStatus::DeniedBySystem => BluetoothRadioAccess::DeniedBySystem,
        _ => BluetoothRadioAccess::Unspecified,
    }
}

fn stale() -> BluetoothError {
    BluetoothError::new(
        BluetoothErrorKind::Unavailable,
        "Bluetooth radio source changed, retired, or exhausted. Refresh before another request.",
    )
}
