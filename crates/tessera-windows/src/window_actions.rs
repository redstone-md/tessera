// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit user actions on already-observed application windows.
//!
//! [`window_action`] applies one action to an [`ActivationTarget`] captured
//! from an observation pass. Close means an asynchronous `WM_CLOSE` request —
//! the owning application may prompt, save, or refuse; the process is never
//! killed. Minimize is an asynchronous `ShowWindowAsync(SW_MINIMIZE)` request.
//! Activate-or-minimize toggles: a genuinely foreground, non-minimized window
//! is minimized; otherwise the window is activated through the same foreground
//! policy as [`crate::activate`].
//!
//! Effects use the same live validation as activation immediately before the
//! request. HWND/PID checks reduce stale-target risk but cannot eliminate
//! same-process handle reuse or make validation and the effect atomic.
//! Success means the request was accepted, not that the application finished
//! saving, closing, or minimizing.

use crate::activation::{ActivationError, ActivationTarget};

/// The user-requested window action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    /// Restore/activate without toggling an already focused window.
    Activate,
    /// Minimize when genuinely foreground and restored; otherwise activate.
    ActivateOrMinimize,
    /// Asynchronously request minimization.
    Minimize,
    /// Asynchronously request `WM_CLOSE`; the target may prompt or refuse.
    Close,
}

/// Why [`crate::window_action`] refused to apply a window action.
#[derive(Debug)]
#[non_exhaustive]
pub enum WindowActionError {
    /// Window actions are only implemented on Windows.
    UnsupportedPlatform,
    /// The recorded identity no longer matches a live top-level window: it
    /// was destroyed, recycled by another process, or is simply stale.
    Unavailable,
    /// The window is live but no longer an ordinary application candidate.
    NotApplication,
    /// The window qualified but the platform denied the requested native
    /// operation; `code` carries the reported last error, `0` when none was
    /// set. The request was not accepted.
    NativeRequestDenied { code: u32 },
}

impl std::fmt::Display for WindowActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "window actions require Windows"),
            Self::Unavailable => write!(f, "window action target is no longer a live window"),
            Self::NotApplication => write!(f, "window action target is not an application window"),
            Self::NativeRequestDenied { code } => {
                write!(f, "native window operation was denied (code 0x{code:X})")
            }
        }
    }
}

impl std::error::Error for WindowActionError {}

impl From<ActivationError> for WindowActionError {
    fn from(error: ActivationError) -> Self {
        match error {
            ActivationError::UnsupportedPlatform => Self::UnsupportedPlatform,
            ActivationError::Unavailable => Self::Unavailable,
            ActivationError::NotApplication => Self::NotApplication,
            ActivationError::ForegroundDenied { code } => Self::NativeRequestDenied { code },
        }
    }
}

/// Applies `action` to the recorded window as an explicit user action.
///
/// Must be called from the panel's UI input thread for
/// [`WindowAction::ActivateOrMinimize`], exactly like [`crate::activate`];
/// foreground admissibility comes from that thread. The target is
/// revalidated against the live window before any effect; a stale identity
/// yields [`WindowActionError::Unavailable`] or
/// [`WindowActionError::NotApplication`] with no side effect.
pub fn window_action(
    target: ActivationTarget,
    action: WindowAction,
) -> Result<(), WindowActionError> {
    #[cfg(windows)]
    {
        crate::native_window_actions::window_action(target, action)
    }
    #[cfg(not(windows))]
    {
        let _ = (target, action);
        // Off Windows no effect can ever be requested; report it, never noop.
        Err(WindowActionError::UnsupportedPlatform)
    }
}

/// Foreground-toggle policy shared by native and recording adapters.
/// Each adapter must reject ineligible identities before an effect; native
/// eligibility checks remain in the existing activation boundary.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) mod dispatch {
    use tessera_core::WindowId;

    use super::{ActivationTarget, WindowAction, WindowActionError};

    /// Live queries and guarded asynchronous effects for an observed target.
    pub(crate) trait Dispatcher {
        fn foreground(&self) -> Option<WindowId>;
        fn is_minimized(&self, target: &ActivationTarget) -> Result<bool, WindowActionError>;
        fn activate(&mut self, target: ActivationTarget) -> Result<(), WindowActionError>;
        fn minimize(&mut self, target: &ActivationTarget) -> Result<(), WindowActionError>;
        fn close(&mut self, target: &ActivationTarget) -> Result<(), WindowActionError>;
    }

    /// Decide from genuine foreground state; never trust a snapshot's focus.
    pub(crate) fn run(
        dispatcher: &mut dyn Dispatcher,
        target: ActivationTarget,
        action: WindowAction,
    ) -> Result<(), WindowActionError> {
        match action {
            WindowAction::Activate => dispatcher.activate(target),
            WindowAction::ActivateOrMinimize => {
                let focused = dispatcher.foreground() == Some(target.window_id())
                    && !dispatcher.is_minimized(&target)?;
                if focused {
                    dispatcher.minimize(&target)
                } else {
                    dispatcher.activate(target)
                }
            }
            WindowAction::Minimize => dispatcher.minimize(&target),
            WindowAction::Close => dispatcher.close(&target),
        }
    }
}

#[cfg(test)]
mod tests;
