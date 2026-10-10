// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Worker-confined WinRT owner; events only invalidate the shared OS-cache view.

use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use tessera_system::battery::{
    BatteryError, BatteryFact, BatterySettingsAccepted, BatterySnapshot, BatteryState,
    EnergySaverState, PowerSupplyState,
};
use windows::Foundation::{EventHandler, Uri};
use windows::System::Launcher;
use windows::System::Power::{BatteryStatus, EnergySaverStatus, PowerManager, PowerSupplyStatus};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::{HSTRING, IInspectable};

fn error(value: windows::core::Error) -> BatteryError {
    match value.code().0 as u32 {
        0x8007_0005 => BatteryError::AccessDenied,
        0x8000_4001 | 0x8004_0154 => BatteryError::Unsupported,
        code => BatteryError::Native { code },
    }
}
fn mapped<T, U>(
    value: windows::core::Result<T>,
    map: impl FnOnce(T) -> Option<U>,
) -> BatteryFact<U> {
    match value {
        Ok(value) => map(value).map_or(BatteryFact::Unknown, BatteryFact::Known),
        Err(value) => BatteryFact::Unavailable(error(value)),
    }
}

#[derive(Clone, Copy)]
enum Event {
    Battery,
    Supply,
    Percent,
    Remaining,
    Saver,
}
impl Event {
    fn add(self, handler: &EventHandler<IInspectable>) -> windows::core::Result<i64> {
        match self {
            Self::Battery => PowerManager::BatteryStatusChanged(handler),
            Self::Supply => PowerManager::PowerSupplyStatusChanged(handler),
            Self::Percent => PowerManager::RemainingChargePercentChanged(handler),
            Self::Remaining => PowerManager::RemainingDischargeTimeChanged(handler),
            Self::Saver => PowerManager::EnergySaverStatusChanged(handler),
        }
    }
    fn remove(self, token: i64) -> windows::core::Result<()> {
        match self {
            Self::Battery => PowerManager::RemoveBatteryStatusChanged(token),
            Self::Supply => PowerManager::RemovePowerSupplyStatusChanged(token),
            Self::Percent => PowerManager::RemoveRemainingChargePercentChanged(token),
            Self::Remaining => PowerManager::RemoveRemainingDischargeTimeChanged(token),
            Self::Saver => PowerManager::RemoveEnergySaverStatusChanged(token),
        }
    }
}

pub(crate) struct NativeOwner {
    tokens: Vec<(Event, i64)>,
    _thread: PhantomData<Rc<()>>,
}
impl NativeOwner {
    pub(crate) fn new() -> Result<Self, BatteryError> {
        // SAFETY: balanced once on the dedicated battery worker, never moved.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(error)?;
        Ok(Self {
            tokens: Vec::new(),
            _thread: PhantomData,
        })
    }
    pub(crate) fn read(&self) -> BatterySnapshot {
        let battery = mapped(PowerManager::BatteryStatus(), |value| match value {
            BatteryStatus::NotPresent => Some(BatteryState::NotPresent),
            BatteryStatus::Discharging => Some(BatteryState::Discharging),
            BatteryStatus::Idle => Some(BatteryState::Idle),
            BatteryStatus::Charging => Some(BatteryState::Charging),
            _ => None,
        });
        let supply = mapped(PowerManager::PowerSupplyStatus(), |value| match value {
            PowerSupplyStatus::NotPresent => Some(PowerSupplyState::NotPresent),
            PowerSupplyStatus::Inadequate => Some(PowerSupplyState::Inadequate),
            PowerSupplyStatus::Adequate => Some(PowerSupplyState::Adequate),
            _ => None,
        });
        let percent = match PowerManager::RemainingChargePercent() {
            Ok(value) if (0..=100).contains(&value) => BatteryFact::Known(value as u8),
            Ok(_) => BatteryFact::Unavailable(BatteryError::InvalidData),
            Err(value) => BatteryFact::Unavailable(error(value)),
        };
        // SYSTEM_POWER_STATUS documents an unambiguous unknown/AC sentinel (-1),
        // unlike the WinRT TimeSpan getter. Do not manufacture a time estimate.
        let mut status = SYSTEM_POWER_STATUS::default();
        let remaining_seconds = match unsafe { GetSystemPowerStatus(&mut status) } {
            Ok(()) if status.BatteryLifeTime == u32::MAX => BatteryFact::Unknown,
            Ok(()) => BatteryFact::Known(status.BatteryLifeTime),
            Err(value) => BatteryFact::Unavailable(error(value)),
        };
        let energy_saver = mapped(PowerManager::EnergySaverStatus(), |value| match value {
            EnergySaverStatus::Disabled => Some(EnergySaverState::Disabled),
            EnergySaverStatus::Off => Some(EnergySaverState::Off),
            EnergySaverStatus::On => Some(EnergySaverState::On),
            _ => None,
        });
        BatterySnapshot {
            battery,
            supply,
            percent,
            remaining_seconds,
            energy_saver,
        }
    }
    pub(crate) fn register(
        &mut self,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), BatteryError> {
        if !self.tokens.is_empty() {
            return Err(BatteryError::Unavailable);
        }
        let handler = EventHandler::<IInspectable>::new(move |_, _| {
            // No native reads, consumer calls, panics across FFI, or retained UI.
            let _ = catch_unwind(AssertUnwindSafe(|| wake()));
            Ok(())
        });
        for event in [
            Event::Battery,
            Event::Supply,
            Event::Percent,
            Event::Remaining,
            Event::Saver,
        ] {
            match event.add(&handler) {
                Ok(token) => self.tokens.push((event, token)),
                Err(value) => {
                    let registration = error(value);
                    return match self.unregister() {
                        Ok(()) => Err(registration),
                        Err(retirement) => Err(retirement),
                    };
                }
            }
        }
        Ok(())
    }
    pub(crate) fn unregister(&mut self) -> Result<(), BatteryError> {
        let mut failed = Vec::new();
        let mut first = None;
        for (event, token) in self.tokens.drain(..).rev() {
            if let Err(value) = event.remove(token) {
                first.get_or_insert_with(|| error(value));
                failed.push((event, token));
            }
        }
        self.tokens = failed;
        first.map_or(Ok(()), Err)
    }
    pub(crate) fn open_settings(&self) -> Result<BatterySettingsAccepted, BatteryError> {
        let uri = Uri::CreateUri(&HSTRING::from("ms-settings:powersleep")).map_err(error)?;
        // Dedicated worker waits for SDK completion, never the GUI. Success does
        // not prove a Settings window appeared or stayed on the requested page.
        if Launcher::LaunchUriAsync(&uri)
            .map_err(error)?
            .join()
            .map_err(error)?
        {
            Ok(BatterySettingsAccepted)
        } else {
            Err(BatteryError::Unavailable)
        }
    }
}
impl Drop for NativeOwner {
    fn drop(&mut self) {
        let _ = self.unregister();
        // Failed removals remain safely owned by WinRT with only weak wakeups.
        unsafe { RoUninitialize() };
    }
}
