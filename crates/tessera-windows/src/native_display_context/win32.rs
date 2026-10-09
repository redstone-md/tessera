// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Raw operations only. All ownership/disarm/retry/order lives in owner::Owner.
use super::owner::{Calls, SNAPSHOT_LIMIT, TargetKey};
use super::{DisplayContextError, Monitor};
use crate::error::ObservationError;
use crate::helpers::rect_from_edges;
use crate::native::{DpiGuard, collect_monitor_handles, monitor_info};
use std::marker::PhantomData;
use std::ptr::{null, null_mut};
use std::rc::Rc;
use windows::Devices::Display::Core::{DisplayManager, DisplayManagerOptions, DisplayTarget};
use windows::UI::ViewManagement::UISettings;
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, DISPLAYCONFIG_PATH_INFO,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows_sys::Win32::Foundation::{GetLastError, HWND};
use windows_sys::Win32::Graphics::Gdi::{
    HMONITOR, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
    GetWindowDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, MONITORINFOF_PRIMARY, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_POPUP,
};

#[derive(Default)]
pub(super) struct NativeCalls(PhantomData<Rc<()>>);
impl Calls for NativeCalls {
    type Dpi = DpiGuard;
    type Window = HWND;
    type Apartment = ();
    type Settings = UISettings;
    type Manager = DisplayManager;
    type Targets = Vec<Result<DisplayTarget, DisplayContextError>>;
    type Path = DISPLAYCONFIG_PATH_INFO;
    type Mode = DISPLAYCONFIG_MODE_INFO;

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
        Ok(collect_monitor_handles()
            .map_err(observation_error)?
            .into_iter()
            .filter_map(|handle| {
                // Info/bounds failure excludes only this projection. rcWork is irrelevant.
                let info = monitor_info(handle).ok()?;
                monitor_candidate(handle as usize, &info.monitorInfo)
            })
            .collect())
    }
    fn config_sizes(&mut self) -> Result<(u32, u32), DisplayContextError> {
        let (mut paths, mut modes) = (0, 0);
        // SAFETY: writable counts, active-only query, no display mutations.
        let status =
            unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes) };
        returned_status(status.0)?;
        Ok((paths, modes))
    }
    fn config_query(
        &mut self,
        paths: &mut [Self::Path],
        modes: &mut [Self::Mode],
    ) -> Result<(u32, u32), DisplayContextError> {
        let (mut path_count, mut mode_count) = (paths.len() as u32, modes.len() as u32);
        // SAFETY: owner capped/allocated arrays; zero-length slices still have allocated
        // non-null storage. Counts are authoritative only after success. No topology ID.
        let status = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };
        returned_status(status.0)?;
        Ok((path_count, mode_count))
    }
    fn source_index(path: &Self::Path) -> u32 {
        // SAFETY: active-only query without VIRTUAL_MODE_AWARE uses modeInfoIdx.
        unsafe { path.sourceInfo.Anonymous.modeInfoIdx }
    }
    fn source_position(mode: &Self::Mode) -> Option<(i32, i32)> {
        if mode.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
            return None;
        }
        // SAFETY: the discriminant identifies the SOURCE union member.
        let position = unsafe { mode.Anonymous.sourceMode.position };
        Some((position.x, position.y))
    }
    fn path_target(path: &Self::Path) -> TargetKey {
        TargetKey {
            low: path.targetInfo.adapterId.LowPart,
            high: path.targetInfo.adapterId.HighPart,
            id: path.targetInfo.id,
        }
    }
    fn activate_manager(&mut self) -> Result<DisplayManager, DisplayContextError> {
        DisplayManager::Create(DisplayManagerOptions::None).map_err(winrt_error)
    }
    fn current_targets(
        &mut self,
        manager: &DisplayManager,
    ) -> Result<Self::Targets, DisplayContextError> {
        let view = manager.GetCurrentTargets().map_err(winrt_error)?;
        let count = view.Size().map_err(winrt_error)?;
        if count as usize > SNAPSHOT_LIMIT {
            return Err(DisplayContextError::InvalidData);
        }
        // A GetAt failure is saved at its position so it fails candidate lookup,
        // rather than globally discarding a usable later CCD path.
        Ok((0..count)
            .map(|index| view.GetAt(index).map_err(winrt_error))
            .collect())
    }
    fn target_count(&mut self, targets: &Self::Targets) -> Result<u32, DisplayContextError> {
        Ok(targets.len() as u32)
    }
    fn target_adapter(
        &mut self,
        targets: &Self::Targets,
        index: u32,
    ) -> Result<(u32, i32), DisplayContextError> {
        let target = targets[index as usize].as_ref().map_err(|error| *error)?;
        let adapter = target
            .Adapter()
            .map_err(winrt_error)?
            .Id()
            .map_err(winrt_error)?;
        Ok((adapter.LowPart, adapter.HighPart))
    }
    fn target_id(
        &mut self,
        targets: &Self::Targets,
        index: u32,
    ) -> Result<u32, DisplayContextError> {
        let target = targets[index as usize].as_ref().map_err(|error| *error)?;
        target.AdapterRelativeId().map_err(winrt_error)
    }
    fn stable_id(
        &mut self,
        targets: &Self::Targets,
        index: u32,
    ) -> Result<String, DisplayContextError> {
        let target = targets[index as usize].as_ref().map_err(|error| *error)?;
        target
            .StableMonitorId()
            .map(|id| id.to_string())
            .map_err(winrt_error)
    }
    fn release_targets(&mut self, targets: Self::Targets) {
        drop(targets);
    }
    fn release_manager(&mut self, manager: DisplayManager) {
        drop(manager);
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
fn monitor_candidate(identity: usize, info: &MONITORINFO) -> Option<Monitor> {
    let rect = info.rcMonitor;
    Some(Monitor {
        identity,
        bounds: rect_from_edges(rect.left, rect.top, rect.right, rect.bottom).ok()?,
        primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
    })
}
fn returned_status(code: u32) -> Result<(), DisplayContextError> {
    if code == 0 {
        Ok(())
    } else {
        Err(DisplayContextError::Native { code })
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
        use windows::Win32::Devices::Display::{
            DISPLAYCONFIG_TOPOLOGY_ID, QUERY_DISPLAY_CONFIG_FLAGS,
        };
        use windows::Win32::Foundation::WIN32_ERROR;
        let _: unsafe fn(QUERY_DISPLAY_CONFIG_FLAGS, *mut u32, *mut u32) -> WIN32_ERROR =
            GetDisplayConfigBufferSizes;
        let _: unsafe fn(
            QUERY_DISPLAY_CONFIG_FLAGS,
            *mut u32,
            *mut DISPLAYCONFIG_PATH_INFO,
            *mut u32,
            *mut DISPLAYCONFIG_MODE_INFO,
            Option<*mut DISPLAYCONFIG_TOPOLOGY_ID>,
        ) -> WIN32_ERROR = QueryDisplayConfig;
        let _: fn(DisplayManagerOptions) -> windows::core::Result<DisplayManager> =
            DisplayManager::Create;
        let _ = DisplayManager::GetCurrentTargets;
        let _: fn(&DisplayTarget) -> windows::core::Result<u32> = DisplayTarget::AdapterRelativeId;
        let _: fn(&DisplayTarget) -> windows::core::Result<windows::core::HSTRING> =
            DisplayTarget::StableMonitorId;
        let _: fn(
            &windows::Devices::Display::Core::DisplayAdapter,
        ) -> windows::core::Result<windows::Graphics::DisplayAdapterId> =
            windows::Devices::Display::Core::DisplayAdapter::Id;
    }

    #[test]
    fn projection_ignores_work_area_and_decodes_only_source_modes() {
        let mut info = MONITORINFO {
            rcMonitor: windows_sys::Win32::Foundation::RECT {
                left: -100,
                top: 0,
                right: 0,
                bottom: 100,
            },
            ..Default::default()
        };
        // Default rcWork is degenerate; it must not suppress a valid monitor.
        assert!(monitor_candidate(1, &info).is_some());
        info.rcMonitor.right = -100;
        assert!(monitor_candidate(1, &info).is_none());
        let mut mode = DISPLAYCONFIG_MODE_INFO::default();
        assert_eq!(NativeCalls::source_position(&mode), None);
        mode.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE;
        mode.Anonymous.sourceMode = windows::Win32::Devices::Display::DISPLAYCONFIG_SOURCE_MODE {
            position: windows::Win32::Foundation::POINTL { x: -100, y: 4 },
            ..Default::default()
        };
        assert_eq!(NativeCalls::source_position(&mode), Some((-100, 4)));
        let mut path = DISPLAYCONFIG_PATH_INFO::default();
        path.sourceInfo.Anonymous.modeInfoIdx = u32::MAX;
        assert_eq!(NativeCalls::source_index(&path), u32::MAX);
    }
}
