// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the power plan in use does and when, as the Power and battery page
//! of the Windows settings sets it: after how long the screen goes off and
//! the computer goes to sleep, on mains power and on battery, and what
//! closing the lid and pressing the power and sleep buttons do. Also the
//! power source and whether battery or energy saver is on, as they change,
//! and a request that keeps the screen on and the computer awake while it
//! lasts, as a video player makes.
//!
//! The settings are the values of the plan in use, read and written with
//! `PowerReadACValueIndex` and its kin and put in force again with
//! `PowerSetActiveScheme`, as `powercfg /change` does. Reading and
//! writing them goes through the power service and may wait, so both are
//! to be called away from the thread that draws.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr::null_mut;

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree, WIN32_ERROR,
};
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, GetPwrCapabilities, GetSystemPowerStatus,
    POWERBROADCAST_SETTING, PowerClearRequest, PowerCreateRequest, PowerGetActiveScheme,
    PowerReadACValueIndex, PowerReadDCValueIndex, PowerRequestDisplayRequired,
    PowerRequestSystemRequired, PowerSetActiveScheme, PowerSetRequest,
    PowerSettingRegisterNotification, PowerWriteACValueIndex, PowerWriteDCValueIndex,
    SYSTEM_POWER_CAPABILITIES, SYSTEM_POWER_STATUS,
};
use windows::Win32::System::SystemServices::{
    GUID_ACDC_POWER_SOURCE, GUID_LIDCLOSE_ACTION, GUID_POWER_SAVING_STATUS,
    GUID_POWERBUTTON_ACTION, GUID_SLEEP_SUBGROUP, GUID_SLEEPBUTTON_ACTION, GUID_STANDBY_TIMEOUT,
    GUID_SYSTEM_BUTTON_SUBGROUP, GUID_VIDEO_POWERDOWN_TIMEOUT, GUID_VIDEO_SUBGROUP,
    POWER_REQUEST_CONTEXT_VERSION,
};
use windows::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};
use windows::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_CALLBACK;
use windows::core::{GUID, PWSTR};

/// Energy saver of Windows 11 24H2 and later, which takes the place of
/// battery saver; the windows crate does not name it. Its value is 0 while
/// it is off, 1 or 2 while it saves.
const GUID_ENERGY_SAVER_STATUS: GUID = GUID::from_u128(0x550e8400_e29b_41d4_a716_446655440000);

/// The `SystemStatusFlag` of the power status while battery saver is on.
const SAVER_ON: u8 = 1;

/// Where the power comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Supply {
    /// Mains power: the computer is plugged in.
    Mains,
    Battery,
}

impl Supply {
    pub const ALL: [Self; 2] = [Self::Mains, Self::Battery];
}

/// A setting of the power plan that Tessera offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PowerSetting {
    /// After how many seconds without input the screen goes off; 0 never.
    ScreenOff,
    /// After how many seconds without input the computer sleeps; 0 never.
    Sleep,
    /// What closing the lid does, as a [`ButtonAction`].
    Lid,
    /// What the power button does.
    PowerButton,
    /// What the sleep button does.
    SleepButton,
}

impl PowerSetting {
    pub const ALL: [Self; 5] = [
        Self::ScreenOff,
        Self::Sleep,
        Self::Lid,
        Self::PowerButton,
        Self::SleepButton,
    ];

    /// The group of the plan the setting is in, and the setting.
    fn ids(self) -> (GUID, GUID) {
        match self {
            Self::ScreenOff => (GUID_VIDEO_SUBGROUP, GUID_VIDEO_POWERDOWN_TIMEOUT),
            Self::Sleep => (GUID_SLEEP_SUBGROUP, GUID_STANDBY_TIMEOUT),
            Self::Lid => (GUID_SYSTEM_BUTTON_SUBGROUP, GUID_LIDCLOSE_ACTION),
            Self::PowerButton => (GUID_SYSTEM_BUTTON_SUBGROUP, GUID_POWERBUTTON_ACTION),
            Self::SleepButton => (GUID_SYSTEM_BUTTON_SUBGROUP, GUID_SLEEPBUTTON_ACTION),
        }
    }

    /// Whether the value is a time in seconds rather than a
    /// [`ButtonAction`].
    pub fn is_timeout(self) -> bool {
        matches!(self, Self::ScreenOff | Self::Sleep)
    }
}

/// What a lid or a button does, by the index the plan keeps for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ButtonAction {
    Nothing,
    Sleep,
    Hibernate,
    ShutDown,
    /// Only the buttons, not the lid, can turn the screen off.
    ScreenOff,
}

impl ButtonAction {
    pub const ALL: [Self; 5] = [
        Self::Nothing,
        Self::Sleep,
        Self::Hibernate,
        Self::ShutDown,
        Self::ScreenOff,
    ];

    /// The action the plan keeps as `index`.
    pub fn of(index: u32) -> Option<Self> {
        Self::ALL.get(index as usize).copied()
    }

    /// The index the plan keeps the action as.
    pub fn index(self) -> u32 {
        Self::ALL
            .iter()
            .position(|action| *action == self)
            .unwrap_or(0) as u32
    }
}

/// The settings Tessera offers of the plan in use, and what the machine
/// has that they apply to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowerSettings {
    /// Whether the machine has a battery, and so settings on battery.
    pub battery: bool,
    pub lid: bool,
    pub power_button: bool,
    pub sleep_button: bool,
    /// Whether the machine can hibernate.
    pub hibernate: bool,
    /// The values read, by setting and supply.
    values: Vec<((PowerSetting, Supply), u32)>,
}

impl PowerSettings {
    /// The settings of the plan in use; `None` where Windows gives no plan.
    pub fn read() -> Option<Self> {
        let scheme = ActiveScheme::get()?;
        let mut capabilities = SYSTEM_POWER_CAPABILITIES::default();
        // SAFETY: the structure outlives the call that fills it.
        let known = unsafe { GetPwrCapabilities(&raw mut capabilities) };
        let values = { PowerSetting::ALL.into_iter() }
            .flat_map(|setting| Supply::ALL.map(|supply| (setting, supply)))
            .filter_map(|key| Some((key, scheme.read(key.0, key.1)?)))
            .collect();
        Some(Self {
            battery: known && capabilities.SystemBatteriesPresent,
            lid: known && capabilities.LidPresent,
            power_button: !known || capabilities.PowerButtonPresent,
            sleep_button: known && capabilities.SleepButtonPresent,
            hibernate: known && capabilities.SystemS4 && capabilities.HiberFilePresent,
            values,
        })
    }

    /// The value of `setting` on `supply`; `None` where the plan has none.
    pub fn value(&self, setting: PowerSetting, supply: Supply) -> Option<u32> {
        { self.values.iter() }
            .find(|(key, _)| *key == (setting, supply))
            .map(|&(_, value)| value)
    }

    /// Takes `value` for `setting` on `supply`, as [`Self::write`] wrote
    /// it, so that what is shown follows at once.
    pub fn set(&mut self, setting: PowerSetting, supply: Supply, value: u32) {
        let key = (setting, supply);
        match self.values.iter_mut().find(|(known, _)| *known == key) {
            Some((_, known)) => *known = value,
            None => self.values.push((key, value)),
        }
    }

    /// Whether the machine has what `setting` is about.
    pub fn applies(&self, setting: PowerSetting) -> bool {
        match setting {
            PowerSetting::ScreenOff | PowerSetting::Sleep => true,
            PowerSetting::Lid => self.lid,
            PowerSetting::PowerButton => self.power_button,
            PowerSetting::SleepButton => self.sleep_button,
        }
    }

    /// Writes `value` for `setting` on `supply` into the plan in use and
    /// puts the plan in force again, so that Windows follows it at once.
    pub fn write(setting: PowerSetting, supply: Supply, value: u32) -> Result<(), String> {
        let scheme = ActiveScheme::get().ok_or("Windows gives no power plan in use")?;
        let (group, id) = setting.ids();
        // SAFETY: the plan, the group and the setting outlive the calls.
        let written = unsafe {
            match supply {
                Supply::Mains => {
                    PowerWriteACValueIndex(None, scheme.0, Some(&group), Some(&id), value)
                }
                Supply::Battery => WIN32_ERROR(PowerWriteDCValueIndex(
                    None,
                    scheme.0,
                    Some(&group),
                    Some(&id),
                    value,
                )),
            }
        };
        if written != ERROR_SUCCESS {
            return Err(format!(
                "the power plan did not take {setting:?} on {supply:?}: {}",
                windows::core::Error::from(written.to_hresult()).message()
            ));
        }
        // SAFETY: the plan outlives the call.
        let applied = unsafe { PowerSetActiveScheme(None, Some(scheme.0)) };
        match applied {
            ERROR_SUCCESS => Ok(()),
            error => Err(format!(
                "the power plan was changed but not put in force again: {}",
                windows::core::Error::from(error.to_hresult()).message()
            )),
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

    /// The value `setting` has on `supply`.
    fn read(&self, setting: PowerSetting, supply: Supply) -> Option<u32> {
        let (group, id) = setting.ids();
        let mut value = 0u32;
        // SAFETY: the plan, the group, the setting and the value outlive
        // the call that fills the value.
        let read = unsafe {
            match supply {
                Supply::Mains => {
                    PowerReadACValueIndex(None, Some(self.0), Some(&group), Some(&id), &mut value).0
                }
                Supply::Battery => {
                    PowerReadDCValueIndex(None, Some(self.0), Some(&group), Some(&id), &mut value)
                }
            }
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

/// The power source and the savers, as they are now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerState {
    pub on_battery: bool,
    /// Battery saver, or energy saver where Windows has it, is on.
    pub saver: bool,
}

/// A change of the power source or of a saver, as Windows tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerChange {
    OnBattery(bool),
    BatterySaver(bool),
    EnergySaver(bool),
}

impl PowerState {
    /// The state now, as the power status gives it.
    pub fn read() -> Self {
        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: `status` outlives the call that fills it.
        if unsafe { GetSystemPowerStatus(&mut status) }.is_err() {
            return Self::default();
        }
        Self {
            // 0 is mains, 1 battery, 255 unknown, which counts as mains.
            on_battery: status.ACLineStatus == 0,
            saver: status.SystemStatusFlag & SAVER_ON != 0,
        }
    }

    /// Starts looking out for changes of the power source, of battery
    /// saver and of energy saver; the receiver yields each. Windows tells
    /// each once right after the start, with the state then. `None` where
    /// Windows takes no request, which is reported. Meant to be called
    /// once.
    pub fn watch() -> Option<Receiver<PowerChange>> {
        let (told, changes) = async_channel::unbounded();
        // Windows keeps calling with this for as long as the process
        // lives, so it is never freed.
        let parameters: &'static mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS =
            Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
                Callback: Some(power_changed),
                Context: Box::into_raw(Box::new(told)).cast(),
            }));
        let settings = [
            GUID_ACDC_POWER_SOURCE,
            GUID_POWER_SAVING_STATUS,
            GUID_ENERGY_SAVER_STATUS,
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
            // A Windows before energy saver refuses its setting.
            if asked == ERROR_SUCCESS {
                registered += 1;
            }
        }
        if registered == 0 {
            report!("Windows does not tell of changes of the power source");
            return None;
        }
        Some(changes)
    }
}

/// The change a setting of `kind` with `value` stands for.
fn change_of(kind: GUID, value: u32) -> Option<PowerChange> {
    match kind {
        // 0 is mains; 1 battery, 2 a short-term source such as a UPS.
        GUID_ACDC_POWER_SOURCE => Some(PowerChange::OnBattery(value != 0)),
        GUID_POWER_SAVING_STATUS => Some(PowerChange::BatterySaver(value != 0)),
        GUID_ENERGY_SAVER_STATUS => Some(PowerChange::EnergySaver(value != 0)),
        _ => None,
    }
}

/// Called by Windows on a thread of its own at each change.
unsafe extern "system" fn power_changed(
    context: *const c_void,
    _kind: u32,
    setting: *const c_void,
) -> u32 {
    // SAFETY: the context is the sender put there in `watch`, which is
    // never freed; for a power setting Windows gives the setting with as
    // many bytes of value as it says, for the call.
    unsafe {
        let told = &*context.cast::<Sender<PowerChange>>();
        let Some(setting) = setting.cast::<POWERBROADCAST_SETTING>().as_ref() else {
            return ERROR_SUCCESS.0;
        };
        if setting.DataLength as usize >= size_of::<u32>() {
            let value = std::ptr::read_unaligned(setting.Data.as_ptr().cast::<u32>());
            if let Some(change) = change_of(setting.PowerSetting, value) {
                told.try_send(change).ok();
            }
        }
    }
    ERROR_SUCCESS.0
}

/// While it lives, Windows keeps the screen on and the computer awake, and
/// lists the request with its reason in `powercfg /requests`.
#[derive(Debug)]
pub struct SleepBlock {
    request: HANDLE,
}

impl SleepBlock {
    /// Asks Windows to keep the screen on and the computer awake, for
    /// `reason`.
    pub fn begin(reason: &str) -> Result<Self, String> {
        let mut text: Vec<u16> = reason.encode_utf16().chain([0]).collect();
        let context = REASON_CONTEXT {
            Version: POWER_REQUEST_CONTEXT_VERSION,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 {
                SimpleReasonString: PWSTR(text.as_mut_ptr()),
            },
        };
        // SAFETY: the context and the text it points to outlive the call,
        // which copies the text.
        let request = unsafe { PowerCreateRequest(&raw const context) }
            .map_err(|error| format!("Windows made no power request: {}", error.message()))?;
        let block = Self { request };
        for kind in [PowerRequestDisplayRequired, PowerRequestSystemRequired] {
            // SAFETY: the request is the one just made, which lives until
            // the block is dropped.
            unsafe { PowerSetRequest(block.request, kind) }.map_err(|error| {
                format!(
                    "Windows did not take the power request: {}",
                    error.message()
                )
            })?;
        }
        Ok(block)
    }
}

impl Drop for SleepBlock {
    fn drop(&mut self) {
        // SAFETY: the request is the one `begin` made; it is cleared and
        // closed once. Clearing a kind that was not set fails harmlessly.
        unsafe {
            let _ = PowerClearRequest(self.request, PowerRequestDisplayRequired);
            let _ = PowerClearRequest(self.request, PowerRequestSystemRequired);
            let _ = CloseHandle(self.request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_action_is_kept_as_its_index() {
        for action in ButtonAction::ALL {
            assert_eq!(ButtonAction::of(action.index()), Some(action));
        }
        assert_eq!(ButtonAction::of(1), Some(ButtonAction::Sleep));
        assert_eq!(ButtonAction::of(9), None);
    }

    #[test]
    fn the_settings_windows_tells_of_are_changes() {
        assert_eq!(
            change_of(GUID_ACDC_POWER_SOURCE, 1),
            Some(PowerChange::OnBattery(true))
        );
        assert_eq!(
            change_of(GUID_ACDC_POWER_SOURCE, 0),
            Some(PowerChange::OnBattery(false))
        );
        assert_eq!(
            change_of(GUID_ENERGY_SAVER_STATUS, 2),
            Some(PowerChange::EnergySaver(true))
        );
        assert_eq!(change_of(GUID_VIDEO_SUBGROUP, 1), None);
    }

    #[test]
    fn a_value_written_is_the_value_shown() {
        let mut settings = PowerSettings {
            battery: true,
            lid: true,
            power_button: true,
            sleep_button: false,
            hibernate: false,
            values: Vec::new(),
        };
        assert_eq!(settings.value(PowerSetting::Sleep, Supply::Battery), None);
        settings.set(PowerSetting::Sleep, Supply::Battery, 600);
        settings.set(PowerSetting::Sleep, Supply::Battery, 900);
        assert_eq!(
            settings.value(PowerSetting::Sleep, Supply::Battery),
            Some(900)
        );
        assert_eq!(settings.value(PowerSetting::Sleep, Supply::Mains), None);
        assert!(!settings.applies(PowerSetting::SleepButton));
    }
}
