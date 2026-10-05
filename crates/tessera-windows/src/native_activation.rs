// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native foreground activation and the startup-error dialog. All Win32 FFI
//! for those paths lives here; observation FFI is in `native.rs`.
//!
//! Every side effect is gated on revalidation of the recorded identity, and
//! the only mutations are an explicit user-action restore and
//! `SetForegroundWindow`. No hooks, no `AttachThreadInput`, no
//! `AllowSetForegroundWindow`, no synchronous cross-process messages.

use std::mem::size_of;

use windows_sys::Win32::Foundation::{GetLastError, HWND, SetLastError};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GW_OWNER, GWL_EXSTYLE, GetAncestor, GetClassNameW, GetWindow, GetWindowLongPtrW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, MB_ICONERROR,
    MB_OK, MessageBoxW, SW_RESTORE, SetForegroundWindow, ShowWindowAsync, WS_EX_TOOLWINDOW,
};

use crate::activation::{ActivationError, ActivationTarget};
use crate::helpers::utf16_to_string_lossy;

pub(crate) fn activate(target: ActivationTarget) -> Result<(), ActivationError> {
    let raw =
        usize::try_from(target.window_id().value()).map_err(|_| ActivationError::Unavailable)?;
    let hwnd = raw as HWND;
    let mut process_id = 0;
    // SAFETY: user32 validates transient handles; the PID output is valid.
    if unsafe { IsWindow(hwnd) } == 0
        || unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) } == 0
        || process_id != target.process_id()
    {
        return Err(ActivationError::Unavailable);
    }
    // Reject our own process before GetWindowText, which can message own windows.
    if process_id == std::process::id() {
        return Err(ActivationError::NotApplication);
    }
    // SAFETY: these are documented handle queries, not memory dereferences.
    if unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd
        || unsafe { IsWindowVisible(hwnd) } == 0
        || !unsafe { GetWindow(hwnd, GW_OWNER) }.is_null()
    {
        return Err(ActivationError::NotApplication);
    }

    let mut caption = [0u16; 1024];
    let mut class = [0u16; 256];
    // SAFETY: bounded live UTF-16 buffers. The foreign top-level caption query
    // reads user32's caption, not a synchronous remote WM_GETTEXT request.
    let caption_len = unsafe { GetWindowTextW(hwnd, caption.as_mut_ptr(), caption.len() as i32) };
    let class_len = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    if caption_len <= 0 || class_len <= 0 {
        return Err(ActivationError::NotApplication);
    }
    if utf16_to_string_lossy(&caption).trim().is_empty()
        || crate::activation::is_excluded_class(&utf16_to_string_lossy(&class))
    {
        return Err(ActivationError::NotApplication);
    }
    // SAFETY: distinguish a valid zero extended style from a failed query.
    unsafe { SetLastError(0) };
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    if (style == 0 && last_error() != 0) || style as u32 & WS_EX_TOOLWINDOW != 0 {
        return Err(ActivationError::NotApplication);
    }
    let mut cloak = 0u32;
    // SAFETY: the DWM attribute writes exactly one DWORD into a valid buffer.
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            (&mut cloak as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    };
    if result < 0 {
        return Err(ActivationError::Unavailable);
    }
    if cloak != 0 {
        return Err(ActivationError::NotApplication);
    }
    // Recheck ownership immediately before effects. HWND/PID checks reduce
    // stale-target risk; they cannot make observation and activation atomic.
    // SAFETY: the PID output remains valid; user32 revalidates the handle.
    if unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) } == 0
        || process_id != target.process_id()
    {
        return Err(ActivationError::Unavailable);
    }
    // SAFETY: IsIconic is a query. Restore is an explicitly requested,
    // asynchronous operation, so a hung target does not block on ShowWindow.
    if unsafe { IsIconic(hwnd) } != 0 && unsafe { ShowWindowAsync(hwnd, SW_RESTORE) } == 0 {
        return Err(ActivationError::Unavailable);
    }
    // SAFETY: only this explicit input path requests foreground activation.
    // Respect Windows denial; no input attachment or simulated keystrokes.
    unsafe { SetLastError(0) };
    if unsafe { SetForegroundWindow(hwnd) } == 0 {
        return Err(ActivationError::ForegroundDenied { code: last_error() });
    }
    Ok(())
}

/// Fixed title of the startup-failure dialog.
const STARTUP_ERROR_TITLE: &str = "Tessera alpha — startup failed";

/// `MessageBoxW` grows its text buffer to hold the dialog text; keep the
/// message bounded well below the practical limit.
const STARTUP_ERROR_MAX_CHARS: usize = 1024;

/// Bound the literal plain-text dialog message: NUL bytes become spaces and
/// the payload is truncated to a fixed character budget so `MessageBoxW`
/// never sees unterminated or oversized text.
fn sanitize_startup_message(message: &str) -> String {
    let mut bounded: String = message
        .chars()
        .map(|character| if character == '\0' { ' ' } else { character })
        .take(STARTUP_ERROR_MAX_CHARS)
        .collect();
    let _ = bounded.try_reserve(STARTUP_ERROR_MAX_CHARS.saturating_sub(bounded.len()));
    bounded
}

pub(crate) fn show_startup_error(message: &str) {
    let text = sanitize_startup_message(message);
    let mut text_utf16: Vec<u16> = text.encode_utf16().collect();
    text_utf16.push(0);
    let mut title_utf16: Vec<u16> = STARTUP_ERROR_TITLE.encode_utf16().collect();
    title_utf16.push(0);

    // SAFETY: both strings are NUL-terminated UTF-16 owned for the duration
    // of the call; the dialog is modal and returns when acknowledged.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text_utf16.as_ptr(),
            title_utf16.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn last_error() -> u32 {
    // SAFETY: reads only the calling thread's last-error slot.
    unsafe { GetLastError() }
}

#[cfg(test)]
mod tests;
