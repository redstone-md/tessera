// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Win32 observation. All FFI and thread-local DPI state live in this module.

use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{null, null_mut};

use tessera_core::WindowId;
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_DATA, GetLastError, HWND, LPARAM, RECT, SetLastError,
};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFOEXW,
    MonitorFromWindow,
};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, IsZoomed,
    MONITORINFOF_PRIMARY, WS_EX_TOOLWINDOW,
};
use windows_sys::core::BOOL;

use crate::error::{ObservationError, ObservationWarning};
use crate::helpers::{covers_monitor, rect_from_edges, utf16_to_string_lossy};
use crate::snapshot::{DesktopSnapshot, MonitorId, ObservedMonitor, ObservedWindow};

pub(crate) fn observe() -> Result<DesktopSnapshot, ObservationError> {
    let _dpi = DpiGuard::enter()?;
    // SAFETY: this query has no pointer parameters or failure case.
    let process_id = unsafe { GetCurrentProcessId() };
    let mut observer = Observer {
        process_id,
        ..Default::default()
    };
    let context = &mut observer as *mut Observer as LPARAM;

    // SAFETY: enumeration is synchronous. The context remains live and is
    // exclusively accessed by our callbacks until each call returns.
    let ok = unsafe { EnumDisplayMonitors(null_mut(), null(), Some(monitor_callback), context) };
    observer.check_enumeration(ok, "EnumDisplayMonitors")?;
    // SAFETY: the same synchronous context lifetime applies here.
    let ok = unsafe { EnumWindows(Some(window_callback), context) };
    observer.check_enumeration(ok, "EnumWindows")?;
    Ok(DesktopSnapshot::new(
        observer.monitors,
        observer.windows,
        observer.warnings,
    ))
}

/// Raw DPI-context tokens are not Send/Sync. The guard never leaves the
/// observing thread and restores its caller's context on every exit path.
struct DpiGuard(DPI_AWARENESS_CONTEXT);

impl DpiGuard {
    fn enter() -> Result<Self, ObservationError> {
        // SAFETY: the predefined context is valid on supported Windows 11.
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.is_null() {
            Err(windows_error("SetThreadDpiAwarenessContext"))
        } else {
            Ok(Self(previous))
        }
    }
}

impl Drop for DpiGuard {
    fn drop(&mut self) {
        // SAFETY: this token was returned by the successful context change on
        // this same thread; it is not a resource handle to close.
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

#[derive(Default)]
struct Observer {
    process_id: u32,
    monitors: Vec<ObservedMonitor>,
    windows: Vec<ObservedWindow>,
    warnings: Vec<ObservationWarning>,
    callback_error: Option<ObservationError>,
}

impl Observer {
    fn check_enumeration(
        &mut self,
        ok: BOOL,
        operation: &'static str,
    ) -> Result<(), ObservationError> {
        if ok != 0 {
            return Ok(());
        }
        let error = windows_error(operation);
        Err(self.callback_error.take().unwrap_or(error))
    }

    fn push_monitor(&mut self, handle: HMONITOR) -> Result<(), ObservationError> {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: derive the pointer from the full EX allocation, not just its
        // prefix field: cbSize permits user32 to write the device name too.
        if unsafe { GetMonitorInfoW(handle, (&mut info as *mut MONITORINFOEXW).cast()) } == 0 {
            return Err(windows_error("GetMonitorInfoW"));
        }
        let monitor = &info.monitorInfo;
        let bounds = rect_from_edges(
            monitor.rcMonitor.left,
            monitor.rcMonitor.top,
            monitor.rcMonitor.right,
            monitor.rcMonitor.bottom,
        )
        .map_err(|_| ObservationError::InvalidGeometry {
            operation: "GetMonitorInfoW(bounds)",
        })?;
        let work_area = rect_from_edges(
            monitor.rcWork.left,
            monitor.rcWork.top,
            monitor.rcWork.right,
            monitor.rcWork.bottom,
        )
        .map_err(|_| ObservationError::InvalidGeometry {
            operation: "GetMonitorInfoW(work-area)",
        })?;
        self.monitors.push(ObservedMonitor::new(
            MonitorId::new(handle as usize as u64),
            utf16_to_string_lossy(&info.szDevice),
            bounds,
            work_area,
            monitor.dwFlags & MONITORINFOF_PRIMARY != 0,
        ));
        Ok(())
    }

    fn push_window(&mut self, hwnd: HWND) -> Result<(), ObservationWarning> {
        // SAFETY: Win32 validates window handles, including ones destroyed
        // during this best-effort scan; no handle is dereferenced by Rust.
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return Ok(());
        }
        let id = WindowId::new(hwnd as usize as u64);
        let warning = |operation, code| ObservationWarning::new(id, operation, code);
        let mut process_id = 0;
        // SAFETY: writable output is valid for the duration of the call.
        if unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) } == 0 {
            return Err(warning("GetWindowThreadProcessId", last_error()));
        }
        if process_id == self.process_id {
            // GetWindowTextW can synchronously call own-window procedures.
            // Excluding our UI avoids hangs/reentrancy and shell self-management.
            return Ok(());
        }

        let mut raw_bounds = RECT::default();
        // SAFETY: Win32 validates hwnd; raw_bounds is a writable RECT.
        if unsafe { GetWindowRect(hwnd, &mut raw_bounds) } == 0 {
            return Err(warning("GetWindowRect", last_error()));
        }
        let bounds = rect_from_edges(
            raw_bounds.left,
            raw_bounds.top,
            raw_bounds.right,
            raw_bounds.bottom,
        )
        .map_err(|_| warning("ValidateWindowRect", ERROR_INVALID_DATA))?;

        let mut caption = [0u16; 1024];
        // SAFETY: buffers have the advertised capacity. Resetting last error
        // distinguishes a valid empty caption from a failed query.
        unsafe { SetLastError(0) };
        let caption_len =
            unsafe { GetWindowTextW(hwnd, caption.as_mut_ptr(), caption.len() as i32) };
        if caption_len == 0 {
            let code = last_error();
            if code != 0 {
                return Err(warning("GetWindowTextW", code));
            }
        }
        let mut class = [0u16; 256];
        // SAFETY: class is writable and its bounded capacity fits i32.
        if unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) } == 0 {
            return Err(warning("GetClassNameW", last_error()));
        }

        // SAFETY: zero can be a legitimate style, so disambiguate using the
        // documented last-error convention rather than inventing flags.
        unsafe { SetLastError(0) };
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        if style == 0 {
            let code = last_error();
            if code != 0 {
                return Err(warning("GetWindowLongPtrW", code));
            }
        }

        let mut cloak = 0u32;
        // SAFETY: DWMWA_CLOAKED writes one DWORD into a correctly sized buffer.
        let hresult = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as u32,
                (&mut cloak as *mut u32).cast(),
                size_of::<u32>() as u32,
            )
        };
        let cloaked = if hresult >= 0 {
            Some(cloak != 0)
        } else {
            self.warnings
                .push(warning("DwmGetWindowAttribute(cloaked)", hresult as u32));
            None
        };

        // SAFETY: these queries accept transient handles and never retain
        // pointers into Rust memory or mutate windows.
        let (minimized, maximized, owned, monitor_handle) = unsafe {
            (
                IsIconic(hwnd) != 0,
                IsZoomed(hwnd) != 0,
                !GetWindow(hwnd, GW_OWNER).is_null(),
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
            )
        };
        let monitor = self
            .monitors
            .iter()
            .find(|monitor| monitor.id().value() == monitor_handle as usize as u64);
        let covers =
            monitor.is_some_and(|monitor| covers_monitor(bounds, minimized, monitor.bounds()));
        self.windows.push(ObservedWindow {
            id,
            process_id,
            title: utf16_to_string_lossy(&caption),
            class_name: utf16_to_string_lossy(&class),
            bounds,
            monitor_id: monitor.map(ObservedMonitor::id),
            minimized,
            maximized,
            cloaked,
            tool_window: style as u32 & WS_EX_TOOLWINDOW != 0,
            owned,
            covers_monitor: covers,
        });
        Ok(())
    }
}

/// Callback failures are stored before returning FALSE; neither ordinary
/// errors nor Rust panics are allowed to unwind into user32.
fn guard_callback(
    observer: &mut Observer,
    collect: impl FnOnce(&mut Observer) -> Result<(), ObservationError>,
) -> BOOL {
    let error = match catch_unwind(AssertUnwindSafe(|| collect(observer))) {
        Ok(Ok(())) => return 1,
        Ok(Err(error)) => error,
        Err(_) => ObservationError::CallbackPanicked,
    };
    observer.callback_error = Some(error);
    0
}

unsafe extern "system" fn monitor_callback(
    handle: HMONITOR,
    _dc: HDC,
    _bounds: *mut RECT,
    context: LPARAM,
) -> BOOL {
    // SAFETY: observe installs a unique live Observer pointer for this
    // synchronous enumeration. No callback uses it after the call returns.
    let observer = unsafe { &mut *(context as *mut Observer) };
    guard_callback(observer, |observer| observer.push_monitor(handle))
}

unsafe extern "system" fn window_callback(hwnd: HWND, context: LPARAM) -> BOOL {
    // SAFETY: same synchronous, exclusively borrowed context contract.
    let observer = unsafe { &mut *(context as *mut Observer) };
    guard_callback(observer, |observer| {
        if let Err(warning) = observer.push_window(hwnd) {
            observer.warnings.push(warning);
        }
        Ok(())
    })
}

fn last_error() -> u32 {
    // SAFETY: reads only the calling thread's last-error slot.
    unsafe { GetLastError() }
}

fn windows_error(operation: &'static str) -> ObservationError {
    ObservationError::Windows {
        operation,
        code: last_error(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_panic_is_reported_without_unwinding() {
        let mut observer = Observer::default();
        assert_eq!(
            guard_callback(&mut observer, |_| panic!("test callback")),
            0
        );
        assert!(matches!(
            observer.callback_error,
            Some(ObservationError::CallbackPanicked)
        ));
    }

    #[test]
    fn dpi_guard_restores_context_during_unwinding() {
        use windows_sys::Win32::UI::HiDpi::{
            AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext,
        };
        // SAFETY: context tokens are read on this thread and not dereferenced.
        let before = unsafe { GetThreadDpiAwarenessContext() };
        let result = catch_unwind(|| {
            let _guard = DpiGuard::enter().unwrap();
            panic!("test unwinding");
        });
        assert!(result.is_err());
        // SAFETY: both tokens are from this same thread's documented query.
        let equal = unsafe { AreDpiAwarenessContextsEqual(before, GetThreadDpiAwarenessContext()) };
        assert_ne!(equal, 0);
    }
}
