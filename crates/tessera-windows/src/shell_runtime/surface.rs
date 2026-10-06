// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Owned native shell surface: documented Win32 styling of our own window
//! plus a reversible appbar reservation. A bar's NOACTIVATE role lasts until
//! its HWND dies, including temporary fullscreen hides.
//!
//! Only a live HWND belonging to the current process is ever accepted; no
//! foreign window is styled, no hook or input path is installed.
//!
//! Appbar reservation rules (per parent review):
//! - Only the Toolbar surface reserves, and only on the top edge.
//! - Dock surfaces never reserve (they do not strip the work area).
//! - Launcher is a tool window but stays activatable (search/keyboard).
//! - Bars are tool windows and no-activate.
//! - Without a validated Explorer appbar host, reservation is skipped.
//! - DWM corner cosmetics are best-effort; no backdrop override is installed.

use windows_sys::Win32::Foundation::{GetLastError, HWND, SetLastError};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmSetWindowAttribute,
};
use windows_sys::Win32::UI::Shell::{
    ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, APPBARDATA, SHAppBarMessage,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, IsWindow, SetWindowLongPtrW, SetWindowPos,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

use crate::shell_runtime::error::ShellRuntimeError;

use super::ShellSurfaceKind;

/// One reserved appbar lease, registered with the documented appbar protocol
/// on the top edge. Follows the documented `ABM_QUERYPOS` (edge-corrected)
/// then `ABM_SETPOS` order, preserving the requested bar height.
struct AppBarReservation {
    window: HWND,
    edge: u32,
    callback_message: u32,
}

impl AppBarReservation {
    const ABE_TOP: u32 = 1;

    /// Registers our own window as a top appbar.
    fn register(window: HWND, callback_message: u32) -> Result<Self, ShellRuntimeError> {
        let mut data = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: window,
            uCallbackMessage: callback_message,
            uEdge: Self::ABE_TOP,
            ..Default::default()
        };
        // SAFETY: documented appbar registration of our own window; the data
        // pointer stays valid for the synchronous call.
        let registered = unsafe { SHAppBarMessage(ABM_NEW, &mut data) };
        if registered == 0 {
            // Caller already established a real host. Refusal is a hard error,
            // not evidence of absence; SHAppBarMessage does not define last-error.
            return Err(ShellRuntimeError::Windows {
                operation: "ABM_NEW refused by existing host",
                code: 31,
            });
        }
        Ok(Self {
            window,
            edge: Self::ABE_TOP,
            callback_message,
        })
    }

    /// Queries the shell-approved top rectangle and commits it with
    /// `ABM_SETPOS`, preserving the original bar height and horizontal span.
    fn reserve(&self) -> Result<(), ShellRuntimeError> {
        validate_owned(self.window)?;
        let bounds = window_rect(self.window)?;
        let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(self.window) };
        if dpi == 0 {
            return Err(windows_error("GetDpiForWindow(toolbar)"));
        }
        let height = bounds.bottom - bounds.top;
        let expected_height = ((32u64 * u64::from(dpi) + 48) / 96) as i32;
        if height != expected_height || bounds.right <= bounds.left {
            return Err(ShellRuntimeError::Windows {
                operation: "toolbar rectangle is not 32 logical pixels",
                code: 87,
            });
        }
        let monitor = unsafe {
            windows_sys::Win32::Graphics::Gdi::MonitorFromWindow(
                self.window,
                windows_sys::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
            )
        };
        let mut info = windows_sys::Win32::Graphics::Gdi::MONITORINFO {
            cbSize: std::mem::size_of::<windows_sys::Win32::Graphics::Gdi::MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { windows_sys::Win32::Graphics::Gdi::GetMonitorInfoW(monitor, &mut info) } == 0 {
            return Err(windows_error("GetMonitorInfoW(toolbar)"));
        }
        let mut data = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: self.window,
            uCallbackMessage: self.callback_message,
            uEdge: self.edge,
            rc: bounds,
            ..Default::default()
        };
        unsafe { SHAppBarMessage(ABM_QUERYPOS, &mut data) };
        data.rc.bottom = data
            .rc
            .top
            .checked_add(height)
            .ok_or(ShellRuntimeError::Windows {
                operation: "toolbar approved rectangle overflow",
                code: 87,
            })?;
        validate_approved_rect(&data.rc, &info.rcMonitor, height)?;
        unsafe { SHAppBarMessage(ABM_SETPOS, &mut data) };
        // SETPOS may adjust the proposal again: use and verify its actual answer.
        validate_approved_rect(&data.rc, &info.rcMonitor, height)?;
        set_window_rect(self.window, &data.rc)?;
        let actual = window_rect(self.window)?;
        if (actual.left, actual.top, actual.right, actual.bottom)
            != (data.rc.left, data.rc.top, data.rc.right, data.rc.bottom)
        {
            return Err(ShellRuntimeError::Windows {
                operation: "toolbar approved rectangle readback mismatch",
                code: 13,
            });
        }
        Ok(())
    }

    fn unregister(&self) -> Result<(), ShellRuntimeError> {
        // A destroyed HWND may be reused by a foreign process; never remove its bar.
        validate_owned(self.window)?;
        let mut data = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: self.window,
            uCallbackMessage: self.callback_message,
            ..Default::default()
        };
        // SAFETY: documented removal of our own appbar registration.
        let removed = unsafe { SHAppBarMessage(ABM_REMOVE, &mut data) };
        if removed == 0 {
            return Err(ShellRuntimeError::Windows {
                operation: "ABM_REMOVE refused",
                code: 31,
            });
        }
        Ok(())
    }
}

impl Drop for AppBarReservation {
    fn drop(&mut self) {
        // Destruction normally removes the shell's slot. Explicit removal while
        // still alive is best-effort here; Drop cannot return a cleanup error.
        let _ = self.unregister();
    }
}

fn windows_error(operation: &'static str) -> ShellRuntimeError {
    // SAFETY: thread-local error query has no preconditions.
    ShellRuntimeError::Windows {
        operation,
        code: unsafe { GetLastError() },
    }
}

fn window_rect(window: HWND) -> Result<windows_sys::Win32::Foundation::RECT, ShellRuntimeError> {
    let mut rect = windows_sys::Win32::Foundation::RECT::default();
    // SAFETY: valid writable RECT for a live own window.
    if unsafe { GetWindowRect(window, &mut rect) } == 0 {
        return Err(windows_error("GetWindowRect"));
    }
    Ok(rect)
}

fn set_window_rect(
    window: HWND,
    rect: &windows_sys::Win32::Foundation::RECT,
) -> Result<(), ShellRuntimeError> {
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_NOZORDER: u32 = 0x0004;
    // SAFETY: documented move of our own window; SWP flags omit activation.
    if unsafe {
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
    } == 0
    {
        return Err(windows_error("SetWindowPos"));
    }
    Ok(())
}

/// The registered appbar callback message (`RegisterWindowMessageW`).
fn register_callback_message() -> Result<u32, ShellRuntimeError> {
    // SAFETY: constant string registration; the result is stable per session.
    let message = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(windows_sys::core::w!(
            "TesseraAppBarCallback"
        ))
    };
    if message == 0 {
        return Err(windows_error("RegisterWindowMessageW"));
    }
    Ok(message)
}

/// Applies best-effort DWM corner cosmetics to our own
/// window. Never fatal: any attribute failure is skipped, the surface keeps
/// its normal composition.
fn apply_owned_cosmetics(window: HWND) {
    let corner = DWMWCP_DONOTROUND as u32;
    // SAFETY: supported corner preference for our own HWND. Failure preserves
    // Slint's normal composition; no forced dark mode or backdrop replaces it.
    unsafe {
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&corner as *const u32).cast(),
            std::mem::size_of::<u32>() as u32,
        );
    }
}

/// Requests foreground for our own activatable UI window, respecting Windows
/// focus policy. `false` is a normal denial; no input is synthesized.
pub fn request_owned_foreground(handle: isize) -> Result<bool, ShellRuntimeError> {
    let window = handle as HWND;
    validate_owned(window)?;
    if read_styles(window)? & WS_EX_NOACTIVATE as isize != 0 {
        return Err(ShellRuntimeError::Windows {
            operation: "foreground request targets a non-activating bar",
            code: 87,
        });
    }
    // SAFETY: a validated live own-process HWND; Windows decides whether the
    // caller may activate it. A denial does not define a last-error code.
    Ok(unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(window) } != 0)
}

/// RAII lease for one owned shell surface: styling plus an optional appbar
/// reservation, kept until the window closes.
pub struct OwnedShellSurface {
    kind: ShellSurfaceKind,
    window: isize,
    original_styles: isize,
    reservation: Option<AppBarReservation>,
}

impl OwnedShellSurface {
    /// Attaches native presentation to an owned live window. Foreign and
    /// destroyed windows are rejected; the lease is returned only after the
    /// styling is applied and the requested reservation is settled.
    pub fn attach(handle: isize, kind: ShellSurfaceKind) -> Result<Self, ShellRuntimeError> {
        let window = handle as HWND;
        validate_owned(window)?;
        let original_styles = read_styles(window)?;
        // Construct rollback before any styling/registration can fail.
        let mut lease = Self {
            kind,
            window: handle,
            original_styles,
            reservation: None,
        };

        // Tool-window/no-activation split per surface kind. Launcher stays
        // activatable for search/keyboard input.
        configure_styles(window, kind)?;
        // Best-effort cosmetics; never fatal.
        apply_owned_cosmetics(window);

        if kind == ShellSurfaceKind::Toolbar
            && super::native_presentation::find_taskbar()?.is_some()
        {
            let callback = register_callback_message()?;
            lease.reservation = Some(AppBarReservation::register(window, callback)?);
            lease.reserve()?;
        }
        Ok(lease)
    }

    /// Refreshes an existing toolbar reservation after owned geometry changes.
    /// A surface without an Explorer host, Dock or Launcher has no reservation.
    pub fn reserve(&self) -> Result<(), ShellRuntimeError> {
        let Some(reservation) = self.reservation.as_ref() else {
            return Ok(());
        };
        reservation.reserve()
    }

    pub fn kind(&self) -> ShellSurfaceKind {
        self.kind
    }
}

/// Applies documented extended styles to our own window and reasserts the
/// topmost ordering without activation. `SetWindowLongPtrW` returning 0 can
/// be a legitimate zero style, so last-error distinguishes failure.
fn configure_styles(window: HWND, kind: ShellSurfaceKind) -> Result<(), ShellRuntimeError> {
    let previous = read_styles(window)?;
    let styles = surface_styles(previous, kind);
    // SAFETY: documented style write of our own window; zero return is
    // disambiguated through last-error.
    write_styles(window, styles)?;
    refresh_frame(
        window,
        windows_sys::Win32::UI::WindowsAndMessaging::HWND_TOPMOST,
    )?;
    let mask = (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST) as isize;
    if read_styles(window)? & mask != styles & mask {
        return Err(ShellRuntimeError::Windows {
            operation: "owned surface style readback mismatch",
            code: 13,
        });
    }
    Ok(())
}

fn surface_styles(previous: isize, kind: ShellSurfaceKind) -> isize {
    let styles = previous as u32 | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
    (if kind == ShellSurfaceKind::Launcher {
        styles & !WS_EX_NOACTIVATE
    } else {
        styles | WS_EX_NOACTIVATE
    }) as isize
}

fn refresh_frame(window: HWND, order: HWND) -> Result<(), ShellRuntimeError> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };
    if unsafe {
        SetWindowPos(
            window,
            order,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
    } == 0
    {
        return Err(windows_error("SetWindowPos(owned surface styles)"));
    }
    Ok(())
}

fn validate_owned(window: HWND) -> Result<(), ShellRuntimeError> {
    let mut process = 0;
    if unsafe { IsWindow(window) } == 0
        || unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
                window,
                &mut process,
            )
        } == 0
    {
        return Err(ShellRuntimeError::Windows {
            operation: "owned surface is not live",
            code: 1400,
        });
    }
    if process != std::process::id() {
        return Err(ShellRuntimeError::Windows {
            operation: "owned surface belongs to foreign process",
            code: 5,
        });
    }
    Ok(())
}

fn read_styles(window: HWND) -> Result<isize, ShellRuntimeError> {
    unsafe { SetLastError(0) };
    let styles = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) };
    if styles == 0 && unsafe { GetLastError() } != 0 {
        return Err(windows_error("GetWindowLongPtrW(GWL_EXSTYLE)"));
    }
    Ok(styles)
}

fn write_styles(window: HWND, styles: isize) -> Result<(), ShellRuntimeError> {
    unsafe { SetLastError(0) };
    let previous = unsafe { SetWindowLongPtrW(window, GWL_EXSTYLE, styles) };
    if previous == 0 && unsafe { GetLastError() } != 0 {
        return Err(windows_error("SetWindowLongPtrW(GWL_EXSTYLE)"));
    }
    Ok(())
}

fn validate_approved_rect(
    rect: &windows_sys::Win32::Foundation::RECT,
    monitor: &windows_sys::Win32::Foundation::RECT,
    height: i32,
) -> Result<(), ShellRuntimeError> {
    if height <= 0
        || rect.bottom.checked_sub(rect.top) != Some(height)
        || rect.right <= rect.left
        || rect.left < monitor.left
        || rect.right > monitor.right
        || rect.top < monitor.top
        || rect.bottom > monitor.bottom
    {
        return Err(ShellRuntimeError::Windows {
            operation: "invalid approved toolbar rectangle",
            code: 87,
        });
    }
    Ok(())
}

impl Drop for OwnedShellSurface {
    fn drop(&mut self) {
        // Unregister before releasing styles; hidden Windows HWNDs survive.
        drop(self.reservation.take());
        let window = self.window as HWND;
        if validate_owned(window).is_ok() {
            let restored =
                super::placement::detached_surface_styles(self.original_styles as u32, self.kind);
            let _ = write_styles(window, restored as isize);
            let order = if self.original_styles as u32 & WS_EX_TOPMOST != 0 {
                windows_sys::Win32::UI::WindowsAndMessaging::HWND_TOPMOST
            } else {
                windows_sys::Win32::UI::WindowsAndMessaging::HWND_NOTOPMOST
            };
            let _ = refresh_frame(window, order);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::RECT;

    #[test]
    fn bars_are_tool_noactivate_and_launcher_is_tool_activatable() {
        for kind in [
            ShellSurfaceKind::Dock,
            ShellSurfaceKind::Toolbar,
            ShellSurfaceKind::Launcher,
        ] {
            let styles = surface_styles(0, kind) as u32;
            assert_ne!(styles & WS_EX_TOOLWINDOW, 0);
            assert_ne!(styles & WS_EX_TOPMOST, 0);
            assert_eq!(
                styles & WS_EX_NOACTIVATE != 0,
                kind != ShellSurfaceKind::Launcher
            );
        }
        assert_eq!(
            surface_styles(WS_EX_NOACTIVATE as isize, ShellSurfaceKind::Launcher) as u32
                & WS_EX_NOACTIVATE,
            0
        );
        // Keep the portable policy's documented bit in sync with Win32.
        assert_eq!(
            super::super::placement::detached_surface_styles(0, ShellSurfaceKind::Dock),
            WS_EX_NOACTIVATE
        );
    }

    #[test]
    fn normal_full_width_top_toolbar_and_negative_origin_are_accepted() {
        let monitor = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let rect = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 32,
        };
        assert!(validate_approved_rect(&rect, &monitor, 32).is_ok());
        let dpi_scaled = RECT { bottom: 48, ..rect };
        assert!(validate_approved_rect(&dpi_scaled, &monitor, 48).is_ok());
        assert!(validate_approved_rect(&dpi_scaled, &monitor, 32).is_err());
        assert!(validate_approved_rect(&RECT { right: 10, ..rect }, &monitor, 32).is_err());
        assert!(validate_approved_rect(&RECT { bottom: 0, ..rect }, &monitor, 32).is_err());
    }
}
