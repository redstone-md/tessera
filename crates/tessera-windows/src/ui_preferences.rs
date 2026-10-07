// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Read-only system preferences for native presentation.

/// Reads whether Windows permits client-area animations.
///
/// This queries only `SPI_GETCLIENTAREAANIMATION`; it never changes a system
/// setting, observes windows, or changes DPI/focus. Unsupported platforms and
/// failed queries remain errors so callers can conservatively skip motion.
pub fn ui_animations_enabled() -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        read_animation_preference()
    }
    #[cfg(not(windows))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "System UI animation preferences require Windows",
        ))
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn read_animation_preference() -> std::io::Result<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };

    let mut enabled: windows_sys::core::BOOL = 0;
    // SAFETY: this getter writes one BOOL into a live, correctly sized local.
    // uiParam and update flags are zero; no SPI_SET operation is reachable.
    let result = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            std::ptr::from_mut(&mut enabled).cast(),
            0,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(enabled != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platform_is_unknown_not_fabricated_permission() {
        assert_eq!(
            ui_animations_enabled().unwrap_err().kind(),
            std::io::ErrorKind::Unsupported
        );
    }

    #[cfg(windows)]
    #[test]
    fn native_animation_preference_can_be_read_without_mutating_settings() {
        ui_animations_enabled().expect("Windows exposes its client-area animation preference");
    }
}
