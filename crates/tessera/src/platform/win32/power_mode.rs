// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The power mode of Windows 10 and 11, which the settings call Best power
//! efficiency, Balanced and Best performance: an overlay over the power
//! plan, which Windows offers while the Balanced plan is in use. The
//! functions that read and set it, PowerGetEffectiveOverlayScheme and
//! PowerSetActiveOverlayScheme of powrprof.dll, are not in the documented
//! headers of every SDK, so they are looked up by name; a Windows without
//! them has no power mode to show. Also whether battery saver is on.

use std::sync::OnceLock;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::core::{GUID, s, w};

/// The overlays Windows knows by these identifiers.
const BEST_EFFICIENCY: GUID = GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a);
const BETTER_BATTERY: GUID = GUID::from_u128(0x3af9b8d9_7c97_431d_ad78_34a8bfea439f);
const BALANCED: GUID = GUID::zeroed();
const BEST_PERFORMANCE: GUID = GUID::from_u128(0xded574b5_45a0_4f42_8737_46345c09c238);

/// The `SystemStatusFlag` of the power status while battery saver is on.
const SAVER_ON: u8 = 1;

/// A power mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerMode {
    /// Best power efficiency: the processors are held back.
    Efficiency,
    Balanced,
    /// Best performance: the processors are let run.
    Performance,
}

impl PowerMode {
    /// The modes, from the one that saves the most to the one that saves
    /// the least.
    pub const ALL: [Self; 3] = [Self::Efficiency, Self::Balanced, Self::Performance];

    /// The mode an overlay stands for. Windows 10 had a fourth one, Better
    /// battery, which is counted with Best power efficiency.
    fn of(overlay: GUID) -> Option<Self> {
        match overlay {
            BEST_EFFICIENCY | BETTER_BATTERY => Some(Self::Efficiency),
            BALANCED => Some(Self::Balanced),
            BEST_PERFORMANCE => Some(Self::Performance),
            _ => None,
        }
    }

    fn overlay(self) -> GUID {
        match self {
            Self::Efficiency => BEST_EFFICIENCY,
            Self::Balanced => BALANCED,
            Self::Performance => BEST_PERFORMANCE,
        }
    }

    /// The mode after this one, the first after the last.
    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// Sets the mode for the power source in use now.
    pub fn activate(self) -> Result<(), String> {
        let Some(functions) = functions() else {
            return Err("this Windows has no power mode".to_owned());
        };
        // SAFETY: the function was looked up in powrprof.dll under its
        // name, has this signature and takes the identifier by value.
        let result = unsafe { (functions.set)(self.overlay()) };
        match result {
            0 => Ok(()),
            error => Err(format!(
                "the power mode was not set: {}",
                std::io::Error::from_raw_os_error(error as i32)
            )),
        }
    }
}

/// The power mode in use and whether battery saver is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerModeState {
    /// `None` while Windows uses an overlay that is none of the three, as
    /// a power plan other than Balanced may.
    pub mode: Option<PowerMode>,
    pub saver: bool,
}

impl PowerModeState {
    /// What Windows says now; `None` on a Windows without power modes or
    /// where it does not say.
    pub fn read() -> Option<Self> {
        let functions = functions()?;
        let mut overlay = GUID::zeroed();
        // SAFETY: the function was looked up in powrprof.dll under its
        // name, has this signature and fills the identifier, which
        // outlives the call.
        let result = unsafe { (functions.get)(&raw mut overlay) };
        if result != ERROR_SUCCESS.0 {
            return None;
        }
        Some(Self {
            mode: PowerMode::of(overlay),
            saver: saver_on(),
        })
    }
}

/// Whether battery saver is on.
fn saver_on() -> bool {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: `status` outlives the call that fills it.
    let read = unsafe { GetSystemPowerStatus(&mut status) };
    read.is_ok() && status.SystemStatusFlag == SAVER_ON
}

type GetOverlay = unsafe extern "system" fn(*mut GUID) -> u32;
type SetOverlay = unsafe extern "system" fn(GUID) -> u32;

/// The two functions, where powrprof.dll has them.
struct Overlays {
    get: GetOverlay,
    set: SetOverlay,
}

fn functions() -> Option<&'static Overlays> {
    static FUNCTIONS: OnceLock<Option<Overlays>> = OnceLock::new();
    FUNCTIONS
        .get_or_init(|| {
            // SAFETY: the library is loaded from the system directory and
            // never unloaded, so the functions stay where they are; each
            // is turned into the signature it is documented with by those
            // who use it, PowerToys among them.
            unsafe {
                let library =
                    LoadLibraryExW(w!("powrprof.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
                let get = GetProcAddress(library, s!("PowerGetEffectiveOverlayScheme"))?;
                let set = GetProcAddress(library, s!("PowerSetActiveOverlayScheme"))?;
                Some(Overlays {
                    get: std::mem::transmute::<unsafe extern "system" fn() -> isize, GetOverlay>(
                        get,
                    ),
                    set: std::mem::transmute::<unsafe extern "system" fn() -> isize, SetOverlay>(
                        set,
                    ),
                })
            }
        })
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_modes_follow_one_another_in_a_circle() {
        assert_eq!(PowerMode::Efficiency.next(), PowerMode::Balanced);
        assert_eq!(PowerMode::Balanced.next(), PowerMode::Performance);
        assert_eq!(PowerMode::Performance.next(), PowerMode::Efficiency);
    }

    #[test]
    fn every_mode_is_known_by_its_overlay() {
        for mode in PowerMode::ALL {
            assert_eq!(PowerMode::of(mode.overlay()), Some(mode));
        }
        assert_eq!(PowerMode::of(BETTER_BATTERY), Some(PowerMode::Efficiency));
        assert_eq!(PowerMode::of(GUID::from_u128(1)), None);
    }

    /// Reads the power mode of the machine it runs on; changes nothing.
    #[test]
    #[ignore = "asks the power mode of this machine"]
    fn the_machine_says_its_power_mode() {
        println!("{:?}", PowerModeState::read());
    }
}
