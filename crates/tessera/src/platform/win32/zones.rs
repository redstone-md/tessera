// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The time zones Windows knows, by the names of their registry keys
//! ("W. Europe Standard Time"), and the time of day in each of them, with
//! the rules of daylight saving time Windows keeps for every year.
//!
//! The zones are read once, the first time one is asked for, from the
//! registry (`EnumDynamicTimeZoneInformation`). A conversion may still
//! read the rules of daylight saving time of its year from the registry,
//! so it is made off the thread that draws.

#![allow(unsafe_code)]

use std::sync::OnceLock;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use windows::Win32::Foundation::{ERROR_SUCCESS, SYSTEMTIME};
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
use windows::Win32::System::Time::{
    DYNAMIC_TIME_ZONE_INFORMATION, EnumDynamicTimeZoneInformation,
    SystemTimeToTzSpecificLocalTimeEx, TzSpecificLocalTimeToSystemTimeEx,
};

use super::files::text_in;
use super::registry::registry_text;

/// The key under which Windows keeps each time zone.
const ZONES_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Time Zones";

/// The most zones read; Windows has about 140.
const MOST: u32 = 1_000;

/// A time zone of Windows.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemZone {
    /// The name of its registry key, as "Tokyo Standard Time".
    pub id: String,
    /// What the time zone settings of Windows call it, as "(UTC+09:00)
    /// Osaka, Sapporo, Tokyo"; its name of standard time where the
    /// registry has none.
    pub display: String,
    /// What Windows needs to convert a time.
    info: DYNAMIC_TIME_ZONE_INFORMATION,
}

impl SystemZone {
    /// Every time zone of Windows, in the order of Windows, which is by
    /// their offsets. Empty where Windows gives none.
    pub fn all() -> &'static [SystemZone] {
        static ZONES: OnceLock<Vec<SystemZone>> = OnceLock::new();
        ZONES.get_or_init(read_all)
    }

    /// The zone Windows keeps under the key `id`, in any letters.
    pub fn named(id: &str) -> Option<&'static SystemZone> {
        Self::all()
            .iter()
            .find(|zone| zone.id.eq_ignore_ascii_case(id))
    }

    /// The time of day in this zone at the universal time `utc`.
    pub fn local_of(&self, utc: NaiveDateTime) -> Option<NaiveDateTime> {
        let universal = system_time(utc)?;
        let mut local = SYSTEMTIME::default();
        // SAFETY: the three structures outlive the call, which only
        // writes `local`.
        unsafe {
            SystemTimeToTzSpecificLocalTimeEx(Some(&self.info), &universal, &mut local).ok()?;
        }
        naive(local)
    }

    /// The universal time when it is `local` in this zone.
    pub fn utc_of(&self, local: NaiveDateTime) -> Option<NaiveDateTime> {
        let local = system_time(local)?;
        let mut universal = SYSTEMTIME::default();
        // SAFETY: the three structures outlive the call, which only
        // writes `universal`.
        unsafe {
            TzSpecificLocalTimeToSystemTimeEx(Some(&self.info), &local, &mut universal).ok()?;
        }
        naive(universal)
    }
}

/// Reads every zone from Windows, with what its settings call it.
fn read_all() -> Vec<SystemZone> {
    let mut zones = Vec::new();
    for index in 0..MOST {
        let mut info = DYNAMIC_TIME_ZONE_INFORMATION::default();
        // SAFETY: `info` outlives the call, which writes only it.
        let status = unsafe { EnumDynamicTimeZoneInformation(index, &mut info) };
        if status != ERROR_SUCCESS.0 {
            // ERROR_NO_MORE_ITEMS after the last; anything else ends too.
            break;
        }
        let id = text_in(&info.TimeZoneKeyName);
        if id.is_empty() {
            continue;
        }
        let key = format!(r"{ZONES_KEY}\{id}");
        let display = registry_text(HKEY_LOCAL_MACHINE, &key, Some("Display"), 256)
            .filter(|display| !display.is_empty())
            .unwrap_or_else(|| text_in(&info.StandardName));
        zones.push(SystemZone { id, display, info });
    }
    zones
}

/// `time` as Windows writes a moment, to the second.
fn system_time(time: NaiveDateTime) -> Option<SYSTEMTIME> {
    Some(SYSTEMTIME {
        wYear: u16::try_from(time.year()).ok()?,
        wMonth: time.month() as u16,
        wDayOfWeek: time.weekday().num_days_from_sunday() as u16,
        wDay: time.day() as u16,
        wHour: time.hour() as u16,
        wMinute: time.minute() as u16,
        wSecond: time.second() as u16,
        wMilliseconds: 0,
    })
}

/// The moment Windows wrote, to the second.
fn naive(time: SYSTEMTIME) -> Option<NaiveDateTime> {
    NaiveDate::from_ymd_opt(time.wYear.into(), time.wMonth.into(), time.wDay.into())?.and_hms_opt(
        time.wHour.into(),
        time.wMinute.into(),
        time.wSecond.into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_goes_to_windows_and_back() {
        let moment = NaiveDate::from_ymd_opt(2026, 3, 29)
            .unwrap()
            .and_hms_opt(1, 30, 15)
            .unwrap();
        let written = system_time(moment).unwrap();
        // 29 March 2026 is a Sunday.
        assert_eq!(written.wDayOfWeek, 0);
        assert_eq!(naive(written), Some(moment));
    }
}
