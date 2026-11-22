// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The colors the user chose in the Windows settings.

use std::io;
use windows::UI::ViewManagement::{UIColorType, UISettings};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};
use windows::core::{PCWSTR, w};

/// Where Windows keeps whether it is light or dark, and the two values:
/// one for the taskbar and the Start menu, one for applications.
const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
const SYSTEM_LIGHT: PCWSTR = w!("SystemUsesLightTheme");
const APPS_LIGHT: PCWSTR = w!("AppsUseLightTheme");

/// How long a program gets to take in that the mode changed.
const TOLD_WITHIN_MS: u32 = 200;

/// The accent color as `[red, green, blue]`, or `None` if Windows does not
/// tell it.
pub fn accent_color() -> Option<[u8; 3]> {
    let settings = UISettings::new().ok()?;
    let color = settings.GetColorValue(UIColorType::Accent).ok()?;
    Some([color.R, color.G, color.B])
}

/// Whether the Windows settings ask for a light taskbar and Start menu,
/// rather than a dark one; `None` if Windows does not tell.
pub fn light_mode() -> Option<bool> {
    let mut value = 0_u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the out-parameters outlive the call, and `size` is the size
    // of what `value` points to.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PERSONALIZE,
            SYSTEM_LIGHT,
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut size),
        )
    };
    status.is_ok().then_some(value != 0)
}

/// Makes Windows and its applications light or dark, as the page Colors
/// of the settings does, and tells the running programs.
pub fn set_light_mode(light: bool) -> io::Result<()> {
    let value = u32::from(light);
    for name in [SYSTEM_LIGHT, APPS_LIGHT] {
        // SAFETY: `value` outlives the call and is as large as is said.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PERSONALIZE,
                name,
                REG_DWORD.0,
                Some((&raw const value).cast()),
                size_of::<u32>() as u32,
            )
        };
        if status.is_err() {
            return Err(io::Error::from_raw_os_error(status.0 as i32));
        }
    }
    // SAFETY: the text outlives the call; a program that does not answer
    // is passed over.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("ImmersiveColorSet").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            TOLD_WITHIN_MS,
            None,
        );
    }
    Ok(())
}
