// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The colors the user chose in the Windows settings.

use windows::UI::ViewManagement::{UIColorType, UISettings};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::w;

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
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut size),
        )
    };
    status.is_ok().then_some(value != 0)
}
