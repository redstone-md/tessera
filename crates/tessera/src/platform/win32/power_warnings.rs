// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What Windows needs to warn of a battery running low, which the battery
//! icon of Explorer does and nothing else does without Explorer: the
//! levels the power plan in use sets for a machine on battery, and a word
//! whenever the charge, the power source or the plan changes.
//!
//! The levels are the settings of the battery group of the plan, as
//! `powercfg /query` lists them: the low level and whether to warn there,
//! the reserve level, the critical level and what Windows does there. The
//! changes come through `PowerSettingRegisterNotification` with a
//! callback, which needs no window.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr::null_mut;

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree};
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerGetActiveScheme, PowerReadDCValueIndex,
    PowerSettingRegisterNotification,
};
use windows::Win32::System::SystemServices::{
    GUID_ACDC_POWER_SOURCE, GUID_BATTERY_DISCHARGE_ACTION_0, GUID_BATTERY_DISCHARGE_FLAGS_1,
    GUID_BATTERY_DISCHARGE_LEVEL_0, GUID_BATTERY_DISCHARGE_LEVEL_1,
    GUID_BATTERY_PERCENTAGE_REMAINING, GUID_BATTERY_SUBGROUP, GUID_POWERSCHEME_PERSONALITY,
};
use windows::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_CALLBACK;
use windows::core::GUID;

/// The reserve battery level of the battery group, which the windows
/// crate does not name.
const RESERVE_LEVEL: GUID = GUID::from_u128(0xf3c5027d_cd16_4930_aa6b_90db844a8f00);

/// What Windows gives the levels when a plan does not say: the defaults of
/// the plans that come with it.
const DEFAULT_LOW: u8 = 10;
const DEFAULT_RESERVE: u8 = 7;
const DEFAULT_CRITICAL: u8 = 5;

/// What Windows does when the battery reaches the critical level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CriticalAction {
    Nothing,
    Sleep,
    Hibernate,
    ShutDown,
}

impl CriticalAction {
    /// The action the setting gives as `index`.
    fn of(index: u32) -> Self {
        match index {
            1 => Self::Sleep,
            2 => Self::Hibernate,
            3 => Self::ShutDown,
            _ => Self::Nothing,
        }
    }
}

/// The battery levels of the power plan in use, in percent, for a machine
/// on battery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatteryLevels {
    /// The low level, `None` where the plan says not to warn there.
    pub low: Option<u8>,
    /// The reserve level, where Windows warns that it is time to plug in.
    pub reserve: u8,
    /// The critical level, where Windows does `critical_action`.
    pub critical: u8,
    pub critical_action: CriticalAction,
}

impl Default for BatteryLevels {
    fn default() -> Self {
        Self {
            low: Some(DEFAULT_LOW),
            reserve: DEFAULT_RESERVE,
            critical: DEFAULT_CRITICAL,
            critical_action: CriticalAction::Hibernate,
        }
    }
}

impl BatteryLevels {
    /// The levels of the plan in use; the defaults of Windows for what it
    /// does not say.
    pub fn read() -> Self {
        let defaults = Self::default();
        let Some(scheme) = ActiveScheme::get() else {
            return defaults;
        };
        let level = |setting: &GUID, default: u8| {
            scheme
                .read(setting)
                .map_or(default, |value| value.min(100) as u8)
        };
        let warns_low = scheme.read(&GUID_BATTERY_DISCHARGE_FLAGS_1) != Some(0);
        Self {
            low: warns_low.then(|| level(&GUID_BATTERY_DISCHARGE_LEVEL_1, DEFAULT_LOW)),
            reserve: level(&RESERVE_LEVEL, DEFAULT_RESERVE),
            critical: level(&GUID_BATTERY_DISCHARGE_LEVEL_0, DEFAULT_CRITICAL),
            critical_action: scheme
                .read(&GUID_BATTERY_DISCHARGE_ACTION_0)
                .map_or(defaults.critical_action, CriticalAction::of),
        }
    }
}

/// The power plan in use, as Windows gives it to the caller to free.
struct ActiveScheme(*mut GUID);

impl ActiveScheme {
    fn get() -> Option<Self> {
        let mut scheme = null_mut();
        // SAFETY: the pointer outlives the call, which allocates what it
        // points to for the caller.
        let got = unsafe { PowerGetActiveScheme(None, &mut scheme) };
        (got == ERROR_SUCCESS && !scheme.is_null()).then_some(Self(scheme))
    }

    /// The value the battery setting `setting` has on battery.
    fn read(&self, setting: &GUID) -> Option<u32> {
        let mut value = 0u32;
        // SAFETY: the plan, the group and the setting outlive the call,
        // and so does the value it fills.
        let read = unsafe {
            PowerReadDCValueIndex(
                None,
                Some(self.0),
                Some(&GUID_BATTERY_SUBGROUP),
                Some(setting),
                &mut value,
            )
        };
        (read == ERROR_SUCCESS.0).then_some(value)
    }
}

impl Drop for ActiveScheme {
    fn drop(&mut self) {
        // SAFETY: Windows allocated the plan with LocalAlloc for the
        // caller, which frees it once.
        unsafe {
            LocalFree(Some(HLOCAL(self.0.cast())));
        }
    }
}

/// Tells of changes that may call for a warning.
pub struct BatteryChanges;

impl BatteryChanges {
    /// Starts looking out for changes of the charge, of the power source
    /// and of the plan in use; the receiver yields at each, those close
    /// together told once. `None` where Windows takes no request, which is
    /// reported. Meant to be called once.
    pub fn watch() -> Option<Receiver<()>> {
        let (changed, changes) = async_channel::bounded(1);
        // Windows keeps calling with this for as long as the process
        // lives, so it is never freed.
        let parameters: &'static mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS =
            Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
                Callback: Some(told),
                Context: Box::into_raw(Box::new(changed)).cast(),
            }));
        let settings = [
            GUID_BATTERY_PERCENTAGE_REMAINING,
            GUID_ACDC_POWER_SOURCE,
            GUID_POWERSCHEME_PERSONALITY,
        ];
        let mut registered = 0;
        for setting in &settings {
            let mut registration = null_mut();
            // SAFETY: the parameters live as long as the process, which
            // keeps the registration; the setting outlives the call.
            let asked = unsafe {
                PowerSettingRegisterNotification(
                    setting,
                    DEVICE_NOTIFY_CALLBACK,
                    HANDLE((&raw mut *parameters).cast()),
                    &mut registration,
                )
            };
            if asked == ERROR_SUCCESS {
                registered += 1;
            }
        }
        if registered == 0 {
            report!("Windows does not tell of changes of the battery; it is read every minute");
            return None;
        }
        Some(changes)
    }
}

/// Called by Windows on a thread of its own at each change.
unsafe extern "system" fn told(context: *const c_void, _kind: u32, _setting: *const c_void) -> u32 {
    // SAFETY: the context is the sender put there in `watch`, which is
    // never freed.
    let changed = unsafe { &*context.cast::<Sender<()>>() };
    // A change waiting to be read covers this one too.
    changed.try_send(()).ok();
    ERROR_SUCCESS.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_critical_action_is_read_from_its_index() {
        assert_eq!(CriticalAction::of(0), CriticalAction::Nothing);
        assert_eq!(CriticalAction::of(1), CriticalAction::Sleep);
        assert_eq!(CriticalAction::of(2), CriticalAction::Hibernate);
        assert_eq!(CriticalAction::of(3), CriticalAction::ShutDown);
        assert_eq!(CriticalAction::of(9), CriticalAction::Nothing);
    }
}
