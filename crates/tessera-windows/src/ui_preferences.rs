// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Read-only system preferences for native presentation.

use std::sync::Arc;

/// Effective system permission delivered when its native setting changes.
pub type UiMotionCallback = Arc<dyn Fn(bool) + Send + Sync>;

/// Retained native animation-setting registration. Drop revokes new events;
/// an already in-flight callback may finish, so UI delivery also needs a fence.
pub struct UiMotionWatcher {
    #[cfg(windows)]
    settings: windows::UI::ViewManagement::UISettings,
    #[cfg(windows)]
    token: i64,
    #[cfg(windows)]
    live: Arc<std::sync::atomic::AtomicBool>,
}

/// Subscribes to Windows' animation-setting event, without a message window,
/// desktop scan, system-setting mutation, input hook or polling timer.
///
/// The event is available on Windows 10 version 2004 and later. If it is
/// unavailable, callers may keep querying the preference for each transition.
pub fn watch_ui_motion(callback: UiMotionCallback) -> std::io::Result<UiMotionWatcher> {
    #[cfg(windows)]
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        use windows::Foundation::TypedEventHandler;
        use windows::UI::ViewManagement::{
            UISettings, UISettingsAnimationsEnabledChangedEventArgs,
        };

        let settings = UISettings::new().map_err(std::io::Error::other)?;
        let live = Arc::new(AtomicBool::new(true));
        let delivery = Arc::clone(&live);
        let handler =
            TypedEventHandler::<UISettings, UISettingsAnimationsEnabledChangedEventArgs>::new(
                move |_, _| {
                    if delivery.load(Ordering::Acquire) {
                        callback(ui_animations_enabled().unwrap_or(false));
                    }
                    Ok(())
                },
            );
        let token = settings
            .AnimationsEnabledChanged(&handler)
            .map_err(std::io::Error::other)?;
        Ok(UiMotionWatcher {
            settings,
            token,
            live,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = callback;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "System UI animation notifications require Windows",
        ))
    }
}

#[cfg(windows)]
impl Drop for UiMotionWatcher {
    fn drop(&mut self) {
        self.live.store(false, std::sync::atomic::Ordering::Release);
        let _ = self.settings.RemoveAnimationsEnabledChanged(self.token);
    }
}

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
        assert_eq!(
            watch_ui_motion(Arc::new(|_| panic!("Unsupported hosts must not deliver")))
                .err()
                .unwrap()
                .kind(),
            std::io::ErrorKind::Unsupported
        );
    }

    #[cfg(windows)]
    #[test]
    fn native_animation_preference_can_be_read_without_mutating_settings() {
        ui_animations_enabled().expect("Windows exposes its client-area animation preference");
    }
}
