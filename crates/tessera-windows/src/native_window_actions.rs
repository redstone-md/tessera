// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native window-action effects for user-requested minimize and close.
//!
//! Close posts `WM_CLOSE` asynchronously: the owning application decides what
//! to do (prompt, save, refuse); the process is never killed and no forced
//! destruction is attempted. Minimize posts `ShowWindowAsync(SW_MINIMIZE)`,
//! which never blocks on a hung target. Foreground activation reuses the
//! exact activation path. No synchronous cross-process messages, no
//! `AttachThreadInput`, no simulated input.

use tessera_core::WindowId;
use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, IsIconic, PostMessageW, SW_MINIMIZE, ShowWindowAsync, WM_CLOSE,
};

use crate::activation::ActivationTarget;
use crate::native_activation::{live_hwnd, validate_target};
use crate::window_actions::dispatch::{Dispatcher, run};
use crate::window_actions::{WindowAction, WindowActionError};

pub(crate) fn window_action(
    target: ActivationTarget,
    action: WindowAction,
) -> Result<(), WindowActionError> {
    run(&mut NativeDispatcher, target, action)
}

/// The user32 effect adapter behind the dispatch seam.
struct NativeDispatcher;

impl Dispatcher for NativeDispatcher {
    fn foreground(&self) -> Option<WindowId> {
        // SAFETY: a query of the calling desktop's foreground window.
        let hwnd = unsafe { GetForegroundWindow() };
        (!hwnd.is_null()).then(|| WindowId::new(hwnd as usize as u64))
    }

    fn is_minimized(&self, target: &ActivationTarget) -> Result<bool, WindowActionError> {
        // The toggle decision must reflect genuine current state, not a stale
        // snapshot flag; this query runs just before the effect.
        let hwnd = live_hwnd(*target)?;
        // SAFETY: IsIconic is a pure query on a validated handle.
        Ok(unsafe { IsIconic(hwnd) } != 0)
    }

    fn activate(&mut self, target: ActivationTarget) -> Result<(), WindowActionError> {
        crate::native_activation::activate(target).map_err(Into::into)
    }

    fn minimize(&mut self, target: &ActivationTarget) -> Result<(), WindowActionError> {
        // The effect boundary: identity revalidated immediately before the
        // asynchronous request.
        let hwnd = validate_target(*target)?;
        // Windows may leave last-error unchanged on failure.
        unsafe { SetLastError(0) };
        // SAFETY: ShowWindowAsync is asynchronous and never blocks on a hung
        // target; a failed post is reported, not ignored.
        if unsafe { ShowWindowAsync(hwnd, SW_MINIMIZE) } == 0 {
            return Err(WindowActionError::NativeRequestDenied { code: last_error() });
        }
        Ok(())
    }

    fn close(&mut self, target: &ActivationTarget) -> Result<(), WindowActionError> {
        // The effect boundary: identity revalidated immediately before the
        // asynchronous request.
        let hwnd = validate_target(*target)?;
        unsafe { SetLastError(0) };
        // SAFETY: posting WM_CLOSE is asynchronous; the owning application
        // remains free to prompt, save, or refuse. Never a process kill.
        if unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) } == 0 {
            return Err(WindowActionError::NativeRequestDenied { code: last_error() });
        }
        Ok(())
    }
}

fn last_error() -> u32 {
    // SAFETY: reads only the calling thread's last-error slot.
    unsafe { GetLastError() }
}

#[cfg(test)]
mod tests;
