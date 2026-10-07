// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit read-only diagnostics for one already-observed window.
//!
//! [`diagnose_window`] is intentionally NOT part of [`crate::observe`]: the
//! desktop scan must stay cheap, and these extra queries (per-window DPI,
//! styles, placement) only run for a window an inspector explicitly names.
//! Every query is read-only; nothing here mutates the window or the OS.
//!
//! The caller must pass the process id captured by the earlier observation.
//! The HWND is re-validated (live, same pid) before any query, and a window
//! that vanished in between yields [`DiagnosticError::Disappeared`] — unknown
//! values are never guessed or fabricated.

use std::fmt;

use tessera_core::WindowId;

/// Why an explicit diagnostic could not be captured.
#[derive(Debug)]
#[non_exhaustive]
pub enum DiagnosticError {
    /// The window no longer exists; the earlier observation is stale.
    Disappeared,
    /// The window now belongs to a different process than the captured pid.
    ForeignOwner { expected: u32, actual: u32 },
    /// Diagnostics require Windows.
    UnsupportedPlatform,
}

impl fmt::Display for DiagnosticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disappeared => write!(f, "window disappeared before diagnostics"),
            Self::ForeignOwner { expected, actual } => write!(
                f,
                "window pid changed since capture (expected {expected}, actual {actual})"
            ),
            Self::UnsupportedPlatform => write!(f, "diagnostics require Windows"),
        }
    }
}

impl std::error::Error for DiagnosticError {}

/// Read-only native facts about one live window. Each field is `None` only
/// when that individual query failed; no value is ever substituted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowDiagnostics {
    dpi: Option<u32>,
    style: Option<u32>,
    ex_style: Option<u32>,
    show_command: Option<u32>,
}

impl WindowDiagnostics {
    /// `GetDpiForWindow`; `None` when the query failed.
    pub fn dpi(&self) -> Option<u32> {
        self.dpi
    }

    /// `GWL_STYLE`; `None` when the query failed.
    pub fn style(&self) -> Option<u32> {
        self.style
    }

    /// `GWL_EXSTYLE`; `None` when the query failed.
    pub fn ex_style(&self) -> Option<u32> {
        self.ex_style
    }

    /// `GetWindowPlacement` `showCmd`; `None` when the query failed.
    pub fn show_command(&self) -> Option<u32> {
        self.show_command
    }
}

/// Re-validates one observed window (live, same owning process) and reads its
/// native DPI, styles, and placement show command without mutating anything.
pub fn diagnose_window(
    id: WindowId,
    expected_pid: u32,
) -> Result<WindowDiagnostics, DiagnosticError> {
    #[cfg(windows)]
    {
        native_diagnostics::diagnose(id, expected_pid)
    }
    #[cfg(not(windows))]
    {
        let _ = (id, expected_pid);
        Err(DiagnosticError::UnsupportedPlatform)
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod native_diagnostics {
    use std::mem::size_of;

    use windows_sys::Win32::Foundation::{GetLastError, HWND};
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, GetWindowPlacement, GetWindowThreadProcessId,
        IsWindow, WINDOWPLACEMENT,
    };

    use super::{DiagnosticError, WindowDiagnostics};
    use tessera_core::WindowId;

    pub(super) fn diagnose(
        id: WindowId,
        expected_pid: u32,
    ) -> Result<WindowDiagnostics, DiagnosticError> {
        validate_live(id, expected_pid)?;

        let hwnd = id.value() as HWND;
        let mut diagnostics = WindowDiagnostics::default();
        // SAFETY: zero is a legitimate style value, so disambiguate failure
        // with the documented last-error convention (as in native.rs).
        unsafe { windows_sys::Win32::Foundation::SetLastError(0) };
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
        if style != 0 || unsafe { GetLastError() } == 0 {
            diagnostics.style = Some(style as u32);
        }

        unsafe { windows_sys::Win32::Foundation::SetLastError(0) };
        let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        if ex_style != 0 || unsafe { GetLastError() } == 0 {
            diagnostics.ex_style = Some(ex_style as u32);
        }

        // SAFETY: one DWORD output; zero means failure, not DPI 0.
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi != 0 {
            diagnostics.dpi = Some(dpi);
        }

        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: placement is a correctly sized writable buffer.
        if unsafe { GetWindowPlacement(hwnd, &mut placement) } != 0 {
            diagnostics.show_command = Some(placement.showCmd);
        }

        // Recheck after the queries: a window destroyed (or an HWND reused by
        // another process) mid-read must never yield a "fresh" report.
        validate_live(id, expected_pid)?;
        Ok(diagnostics)
    }

    /// Validates the window is live and still owned by `expected_pid`.
    fn validate_live(id: WindowId, expected_pid: u32) -> Result<(), DiagnosticError> {
        let hwnd = id.value() as HWND;
        // SAFETY: Win32 validates window handles; a destroyed handle is a
        // documented FALSE return, which maps to a stale snapshot, not a guess.
        if unsafe { IsWindow(hwnd) } == 0 {
            return Err(DiagnosticError::Disappeared);
        }
        let mut actual_pid = 0;
        // SAFETY: writable output is valid for the duration of the call.
        if unsafe { GetWindowThreadProcessId(hwnd, &mut actual_pid) } == 0 {
            return Err(DiagnosticError::Disappeared);
        }
        if actual_pid != expected_pid {
            return Err(DiagnosticError::ForeignOwner {
                expected: expected_pid,
                actual: actual_pid,
            });
        }
        Ok(())
    }
}
