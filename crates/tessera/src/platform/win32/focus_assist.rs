// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What Windows itself says of notifications: whether its do not disturb
//! (Focus assist before Windows 11) holds, and what its settings say of
//! the notifications of each application. Tessera only reads both; it
//! never writes the state or the settings of Windows.
//!
//! Do not disturb is not offered by any documented interface. Windows
//! publishes it as the Windows Notification Facility state
//! `WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED`, which is read with
//! `NtQueryWnfStateData` and followed with
//! `RtlSubscribeWnfStateChangeNotification`, both taken from `ntdll.dll`
//! with `GetProcAddress`. The name of the state is the one published by
//! the projects that list the states of the facility (and read the same
//! way by Seelen UI's `focus_assist` module): 0 for off, 1 for priority
//! only, 2 for alarms only.
//!
//! The settings of the applications are the values the Settings app of
//! Windows writes under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Notifications\Settings\<AUMID>`
//! when the user changes them (System > Notifications > the application):
//! `Enabled` (0: its notifications are off), `ShowInActionCenter` (0: not
//! in the notification center), `ShowBanner` (0: no banners), `Rank` (99
//! for the priority Windows calls Top) and `SoundFile` (empty: no
//! sound). Values that are missing are the defaults: everything on. The
//! names are seen in the registry, not documented by Microsoft.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::OnceLock;

use async_channel::{Receiver, Sender};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SAM_FLAGS};
use windows::core::{GUID, PCSTR, w};

use super::registry::Key;

/// The Windows Notification Facility state of do not disturb, from the
/// lists of the facility's states published by those who studied it.
const WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED: u64 = 0x0D83_063E_A3BF_1C75;

/// Where Windows keeps the settings of the notifications of each
/// application.
const SETTINGS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings";

/// The `Rank` Settings writes for the priority it calls Top.
const TOP_RANK: u32 = 99;

/// What Windows' do not disturb lets through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuietProfile {
    /// It is off: every notification comes.
    Off,
    /// Only the applications on its priority list come.
    PriorityOnly,
    /// Only alarms come.
    AlarmsOnly,
}

impl QuietProfile {
    fn of(value: u32) -> Self {
        match value {
            0 => Self::Off,
            2 => Self::AlarmsOnly,
            _ => Self::PriorityOnly,
        }
    }

    /// Whether it holds notifications back.
    pub fn holds(self) -> bool {
        self != Self::Off
    }
}

type Query = unsafe extern "system" fn(
    *const u64,
    *const GUID,
    *const c_void,
    *mut u32,
    *mut c_void,
    *mut u32,
) -> i32;

type Callback =
    unsafe extern "system" fn(u64, u32, *const GUID, *const c_void, *const c_void, u32) -> i32;

type Subscribe = unsafe extern "system" fn(
    *mut *mut c_void,
    u64,
    u32,
    Callback,
    *const c_void,
    *const GUID,
    u32,
    u32,
) -> i32;

/// The two functions of `ntdll.dll`, found once.
struct Wnf {
    query: Query,
    subscribe: Subscribe,
}

impl Wnf {
    fn get() -> Result<&'static Self, String> {
        static FOUND: OnceLock<Result<Wnf, String>> = OnceLock::new();
        FOUND.get_or_init(Self::find).as_ref().map_err(Clone::clone)
    }

    fn find() -> Result<Self, String> {
        // SAFETY: ntdll.dll is loaded in every process and never unloaded.
        let ntdll = unsafe { GetModuleHandleW(w!("ntdll.dll")) }
            .map_err(|error| format!("ntdll.dll is not there: {error}"))?;
        // SAFETY: the module is loaded and the names are NUL-terminated.
        let (query, subscribe) = unsafe {
            (
                GetProcAddress(ntdll, PCSTR(c"NtQueryWnfStateData".as_ptr().cast())),
                GetProcAddress(
                    ntdll,
                    PCSTR(c"RtlSubscribeWnfStateChangeNotification".as_ptr().cast()),
                ),
            )
        };
        let (Some(query), Some(subscribe)) = (query, subscribe) else {
            return Err("this Windows does not publish the state of do not disturb".to_owned());
        };
        // SAFETY: both functions have the signatures above, as their
        // callers outside Microsoft use them; neither is documented.
        unsafe {
            Ok(Self {
                query: std::mem::transmute::<unsafe extern "system" fn() -> isize, Query>(query),
                subscribe: std::mem::transmute::<unsafe extern "system" fn() -> isize, Subscribe>(
                    subscribe,
                ),
            })
        }
    }
}

/// Where the changes of do not disturb go, once it is followed.
static FOLLOWERS: OnceLock<(Sender<QuietProfile>, Receiver<QuietProfile>)> = OnceLock::new();

/// What Windows calls with each change of the state.
unsafe extern "system" fn changed(
    _state: u64,
    _stamp: u32,
    _type: *const GUID,
    _context: *const c_void,
    buffer: *const c_void,
    size: u32,
) -> i32 {
    if !buffer.is_null()
        && size as usize >= size_of::<u32>()
        && let Some((sender, _)) = FOLLOWERS.get()
    {
        // SAFETY: Windows gives a buffer of `size` bytes, at least four.
        let value = unsafe { buffer.cast::<u32>().read_unaligned() };
        sender.try_send(QuietProfile::of(value)).ok();
    }
    0
}

/// Windows' do not disturb.
pub struct WindowsQuiet;

impl WindowsQuiet {
    /// What it lets through now.
    pub fn now() -> Result<QuietProfile, String> {
        Self::read().map(|(profile, _)| profile)
    }

    /// The state and the stamp of its last change.
    fn read() -> Result<(QuietProfile, u32), String> {
        let wnf = Wnf::get()?;
        let state = WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED;
        let (mut stamp, mut value, mut size) = (0u32, 0u32, size_of::<u32>() as u32);
        // SAFETY: every pointer is to a local that outlives the call, and
        // the buffer is as large as the size says.
        let status = unsafe {
            (wnf.query)(
                &raw const state,
                std::ptr::null(),
                std::ptr::null(),
                &raw mut stamp,
                (&raw mut value).cast(),
                &raw mut size,
            )
        };
        match status {
            0 => Ok((QuietProfile::of(value), stamp)),
            status => Err(format!(
                "the state of do not disturb cannot be read (status {status:#x})"
            )),
        }
    }

    /// Follows it: the receiver yields each change from now on, for as
    /// long as Tessera runs. Every caller gets the same receiver.
    pub fn follow() -> Result<Receiver<QuietProfile>, String> {
        if let Some((_, receiver)) = FOLLOWERS.get() {
            return Ok(receiver.clone());
        }
        let wnf = Wnf::get()?;
        let (_, stamp) = Self::read()?;
        let (_, receiver) = FOLLOWERS.get_or_init(async_channel::unbounded);
        let mut subscription = std::ptr::null_mut();
        // SAFETY: the callback has the signature Windows calls it with and
        // lives for the whole run, as the subscription does, which is
        // never ended.
        let status = unsafe {
            (wnf.subscribe)(
                &raw mut subscription,
                WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED,
                stamp,
                changed,
                std::ptr::null(),
                std::ptr::null(),
                0,
                0,
            )
        };
        match status {
            0 => Ok(receiver.clone()),
            status => Err(format!(
                "the changes of do not disturb cannot be followed (status {status:#x})"
            )),
        }
    }
}

/// What the settings of Windows say of the notifications of one
/// application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppNotificationSettings {
    /// Whether its notifications are on at all.
    pub enabled: bool,
    /// Whether they are kept in the notification center.
    pub in_center: bool,
    /// Whether they come as banners.
    pub banners: bool,
    /// Whether they have the priority Windows calls Top.
    pub top_priority: bool,
    /// Whether they play a sound.
    pub sound: bool,
}

impl Default for AppNotificationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            in_center: true,
            banners: true,
            top_priority: false,
            sound: true,
        }
    }
}

impl AppNotificationSettings {
    /// What Windows says of the application `app_id`; everything on where
    /// it says nothing. It reads the registry, which is quick, but is for
    /// a thread away from the user interface all the same.
    pub fn of(app_id: &str) -> Self {
        if app_id.is_empty() || app_id.contains(['\\', '/']) {
            return Self::default();
        }
        let path = format!(r"{SETTINGS_KEY}\{app_id}");
        let Some(key) = Key::read(HKEY_CURRENT_USER, &path, REG_SAM_FLAGS(0)) else {
            return Self::default();
        };
        let on = |name: &str| key.number(name).is_none_or(|value| value != 0);
        Self {
            enabled: on("Enabled"),
            in_center: on("ShowInActionCenter"),
            banners: on("ShowBanner"),
            top_priority: key.number("Rank") == Some(TOP_RANK),
            sound: key.text("SoundFile").is_none_or(|file| !file.is_empty()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profiles_of_do_not_disturb_are_read_from_their_numbers() {
        assert_eq!(QuietProfile::of(0), QuietProfile::Off);
        assert_eq!(QuietProfile::of(1), QuietProfile::PriorityOnly);
        assert_eq!(QuietProfile::of(2), QuietProfile::AlarmsOnly);
        assert!(!QuietProfile::Off.holds() && QuietProfile::AlarmsOnly.holds());
    }
}
