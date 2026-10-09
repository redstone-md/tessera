// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Raw operations only. All ownership/disarm/retry/order lives in owner::Owner.
use super::owner::Calls;
use super::{DisplayContextError, Monitor};
use crate::error::ObservationError;
use crate::native::{DpiGuard, collect_monitors};
use std::marker::PhantomData;
use std::ptr::{null, null_mut};
use std::rc::Rc;
use windows::UI::ViewManagement::UISettings;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows_sys::Win32::Foundation::{GetLastError, HWND};
use windows_sys::Win32::Graphics::Gdi::{HMONITOR, MONITOR_DEFAULTTONULL, MonitorFromWindow};
use windows_sys::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
    GetWindowDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

#[derive(Default)]
pub(super) struct NativeCalls(PhantomData<Rc<()>>);
impl Calls for NativeCalls {
    type Dpi = DpiGuard;
    type Window = HWND;
    type Apartment = ();
    type Settings = UISettings;

    fn enter_dpi(&mut self) -> Result<DpiGuard, DisplayContextError> {
        DpiGuard::enter().map_err(observation_error)
    }
    fn restore_dpi(&mut self, dpi: &mut DpiGuard) -> Result<(), DisplayContextError> {
        dpi.restore().map_err(observation_error)
    }
    fn discard_dpi(&mut self, mut dpi: DpiGuard) {
        // Owner has performed checked restoration and its best-effort retry.
        // Disable the helper's legacy Drop fallback so the shared owner is the
        // sole resource retirement policy. Existing observation never disarms.
        dpi.disarm();
    }
    fn monitors(&mut self) -> Result<Vec<Monitor>, DisplayContextError> {
        Ok(collect_monitors()
            .map_err(observation_error)?
            .into_iter()
            .map(|monitor| Monitor {
                identity: monitor.id().value() as usize,
                bounds: monitor.bounds(),
                primary: monitor.primary(),
            })
            .collect())
    }
    fn create_window(
        &mut self,
        _selected: Monitor,
        x: i32,
        y: i32,
    ) -> Result<HWND, DisplayContextError> {
        const STATIC: [u16; 7] = [83, 84, 65, 84, 73, 67, 0];
        // SAFETY: built-in class; hidden top-level 1x1 popup wholly within selected
        // bounds, no parent/owner/menu/module/Rust userdata/custom procedure.
        // The owner worker is scoped PMv2; no app or overlay window is touched.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                STATIC.as_ptr(),
                null(),
                WS_POPUP,
                x,
                y,
                1,
                1,
                null_mut(),
                null_mut(),
                null_mut(),
                null(),
            )
        };
        if hwnd.is_null() {
            return Err(last_error());
        }
        Ok(hwnd)
    }
    fn probe_matches(
        &mut self,
        window: &HWND,
        selected: Monitor,
    ) -> Result<bool, DisplayContextError> {
        // SAFETY: owner retains this live HWND; DEFAULTTONULL deliberately avoids
        // substituting the nearest monitor when the association is missing.
        Ok(unsafe {
            AreDpiAwarenessContextsEqual(
                GetWindowDpiAwarenessContext(*window),
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            ) != 0
                && MonitorFromWindow(*window, MONITOR_DEFAULTTONULL)
                    == selected.identity as HMONITOR
        })
    }
    fn dpi(&mut self, window: &HWND) -> Result<u32, DisplayContextError> {
        // SAFETY: owner has validated PMv2 awareness and monitor association.
        Ok(unsafe { GetDpiForWindow(*window) })
    }
    fn destroy_window(&mut self, window: &HWND) -> Result<(), DisplayContextError> {
        // SAFETY: shared owner destroys on the same thread as creation.
        if unsafe { DestroyWindow(*window) } == 0 {
            return Err(last_error());
        }
        Ok(())
    }
    fn initialize(&mut self) -> Result<(), DisplayContextError> {
        // SAFETY: dedicated owner worker; shared owner balances S_OK/S_FALSE and
        // owns no uninit obligation after changed-mode/initialization failure.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(winrt_error)
    }
    fn uninitialize(&mut self, (): ()) {
        // SAFETY: shared owner retires one successful init on this same thread.
        unsafe { RoUninitialize() };
    }
    fn activate_settings(&mut self) -> Result<UISettings, DisplayContextError> {
        UISettings::new().map_err(winrt_error)
    }
    fn text_scale(&mut self, settings: &UISettings) -> Result<f64, DisplayContextError> {
        settings.TextScaleFactor().map_err(winrt_error)
    }
    fn release_settings(&mut self, settings: UISettings) {
        drop(settings);
    }
}
fn last_error() -> DisplayContextError {
    // SAFETY: immediate owner-thread capture following a failed native call.
    DisplayContextError::Native {
        code: unsafe { GetLastError() },
    }
}
fn winrt_error(error: windows::core::Error) -> DisplayContextError {
    DisplayContextError::Native {
        code: error.code().0 as u32,
    }
}
fn observation_error(error: ObservationError) -> DisplayContextError {
    match error {
        ObservationError::UnsupportedPlatform => DisplayContextError::Unsupported,
        ObservationError::Windows { code, .. } => DisplayContextError::Native { code },
        ObservationError::InvalidGeometry { .. } => DisplayContextError::InvalidData,
        ObservationError::CallbackPanicked => DisplayContextError::Unavailable,
    }
}

#[cfg(test)]
mod sdk_shapes {
    use super::*;
    // Compile-only bindings; no test calls any native function.
    #[test]
    fn query_sdk_shapes() {
        let _: unsafe extern "system" fn(HWND) -> u32 = GetDpiForWindow;
        let _: unsafe extern "system" fn(HWND) -> windows_sys::core::BOOL = DestroyWindow;
        let _: unsafe extern "system" fn(HWND, u32) -> HMONITOR = MonitorFromWindow;
    }
}
