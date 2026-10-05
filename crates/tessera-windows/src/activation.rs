// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Foreground activation of ordinary application windows.
//!
//! Activation is an explicit user action (a panel button press) performed on
//! the UI input thread, which keeps the panel process foreground-eligible for
//! `SetForegroundWindow`. It never requests window geometry changes or sends
//! synchronous messages to foreign windows, and never installs hooks or other
//! foreground-acquisition bypasses.
//!
//! Callers must treat [`ActivationTarget`] values as ephemeral. Native HWND/PID
//! and eligibility checks reduce stale-target risk, but activation is not
//! atomic and same-process handle reuse cannot be completely ruled out.

use tessera_core::WindowId;

use crate::snapshot::ObservedWindow;

/// An activation candidate captured from one observation pass.
///
/// Encapsulated value type; there is intentionally no public constructor. The
/// only public way to obtain a target is [`ActivationTarget::from_window`].
/// The raw window identity is meaningful only against the observation pass it
/// came from and must never be persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationTarget {
    window_id: WindowId,
    process_id: u32,
}

impl ActivationTarget {
    pub(crate) const fn new(window_id: WindowId, process_id: u32) -> Self {
        Self {
            window_id,
            process_id,
        }
    }

    /// Qualifies `window` as an ordinary activation candidate, or `None`.
    ///
    /// Excludes this process's own windows (including the panel UI), tool
    /// windows, windows with an owner (dialogs, palettes), DWM-cloaked
    /// windows, known desktop and taskbar host classes, and blank titles.
    /// Unknown cloaking state can appear in the list; native activation must
    /// obtain a successful live cloaking query before acting.
    ///
    /// This is a pure classification of snapshot data with no live Win32
    /// checks; identity is revalidated natively by [`crate::activate`] before
    /// any side effect. Off Windows, observation cannot produce windows, so
    /// the portable classification still runs and simply has no live
    /// counterpart to revalidate against.
    pub fn from_window(window: &ObservedWindow) -> Option<Self> {
        classify(window)
    }

    /// Raw observed window identity; ephemeral, valid for one activation try.
    pub fn window_id(&self) -> WindowId {
        self.window_id
    }

    /// Process owning the observed window.
    pub fn process_id(&self) -> u32 {
        self.process_id
    }
}

/// Why [`crate::activate`] refused to bring a window to the foreground.
#[derive(Debug)]
#[non_exhaustive]
pub enum ActivationError {
    /// Activation is only implemented on Windows.
    UnsupportedPlatform,
    /// The recorded identity no longer matches a live top-level window: it
    /// was destroyed, recycled by another process, or is simply stale.
    Unavailable,
    /// The window is live but no longer an ordinary application candidate
    /// (tool/owned/cloaked, desktop or taskbar class, blank title, own
    /// process, or not a top-level root window).
    NotApplication,
    /// The window qualified but Windows denied the foreground change; `code`
    /// carries the reported last error, `0` when none was set.
    ForegroundDenied { code: u32 },
}

impl std::fmt::Display for ActivationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "window activation requires Windows"),
            Self::Unavailable => write!(f, "activation target is no longer a live window"),
            Self::NotApplication => write!(f, "activation target is not an application window"),
            Self::ForegroundDenied { code } => {
                write!(f, "foreground change was denied (code 0x{code:X})")
            }
        }
    }
}

impl std::error::Error for ActivationError {}

/// Brings the recorded window to the foreground as an explicit user action.
///
/// Must be called directly from the panel's UI input thread (the thread that
/// handles the activation input); a foreground-eligible thread is what makes
/// the change admissible to Windows. A minimized target may be restored —
/// and only then — because this is an explicit user request; no other
/// geometry or placement mutation is ever performed.
///
/// The target is revalidated against the live window (handle validity,
/// owning process, and candidate eligibility) before any side effect, so a
/// stale identity yields [`ActivationError::Unavailable`] or
/// [`ActivationError::NotApplication`] instead of a foreground change.
/// Windows reports foreground denial without a guaranteed error code; the
/// returned [`ActivationError::ForegroundDenied`] reflects the observed
/// result, not an assumed completion.
pub fn activate(target: ActivationTarget) -> Result<(), ActivationError> {
    #[cfg(windows)]
    {
        crate::native_activation::activate(target)
    }
    #[cfg(not(windows))]
    {
        let _ = target;
        Err(ActivationError::UnsupportedPlatform)
    }
}

/// Shows a modal, app-owned error dialog for a failed Slint startup.
///
/// Used by the entry point when the UI cannot initialize and there is nothing
/// else to surface the failure with. The message is sanitized (NULs become
/// spaces) and truncated before display; the dialog is plain text, blocking
/// until acknowledged. Off Windows this falls back to `stderr`.
///
/// Call this before returning from `main` on the startup failure path; the
/// dialog is process-modal, so it must not be used once a UI event loop is
/// already running.
pub fn show_startup_error(message: &str) {
    #[cfg(windows)]
    {
        crate::native_activation::show_startup_error(message);
    }
    #[cfg(not(windows))]
    {
        eprintln!("Tessera startup failed: {message}");
    }
}

/// Top-level shell hosts that must never receive foreground activation:
/// the desktop (Progman/WorkerW and its icon views) and taskbar surfaces.
const EXCLUDED_CLASSES: [&str; 6] = [
    "Progman",
    "WorkerW",
    "SHELLDLL_DefView",
    "Ghost",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
];

/// Class atoms are case-insensitive; match exclusions the same way.
pub(crate) fn is_excluded_class(class_name: &str) -> bool {
    EXCLUDED_CLASSES
        .iter()
        .any(|excluded| class_name.eq_ignore_ascii_case(excluded))
}

/// Pure candidate classification shared by every construction path.
fn classify(window: &ObservedWindow) -> Option<ActivationTarget> {
    if window.process_id() == std::process::id() {
        return None;
    }
    if window.tool_window() || window.owned() {
        return None;
    }
    if window.cloaked() == Some(true) {
        return None;
    }
    if is_excluded_class(window.class_name()) {
        return None;
    }
    if window.title().trim().is_empty() {
        return None;
    }
    Some(ActivationTarget::new(window.id(), window.process_id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::Rect;

    fn sample_window(overrides: impl FnOnce(&mut ObservedWindow)) -> ObservedWindow {
        let mut window = ObservedWindow {
            id: WindowId::new(0x0000_0001_0000_1234),
            process_id: std::process::id() + 1,
            title: "Notepad - Notes.txt".to_string(),
            class_name: "Notepad".to_string(),
            bounds: Rect::new(10, 10, 640, 480).unwrap(),
            monitor_id: None,
            minimized: false,
            maximized: false,
            cloaked: Some(false),
            tool_window: false,
            owned: false,
            covers_monitor: false,
        };
        overrides(&mut window);
        window
    }

    fn assert_rejected(overrides: impl FnOnce(&mut ObservedWindow)) {
        assert!(classify(&sample_window(overrides)).is_none());
    }

    #[test]
    fn ordinary_window_qualifies_with_recorded_identity() {
        let target = classify(&sample_window(|_| {})).expect("ordinary window qualifies");
        assert_eq!(target.window_id(), WindowId::new(0x0000_0001_0000_1234));
        assert_eq!(target.process_id(), std::process::id() + 1);
    }

    #[test]
    fn own_process_windows_are_never_targets() {
        assert_rejected(|window| window.process_id = std::process::id());
    }

    #[test]
    fn tool_owned_and_cloaked_windows_are_rejected() {
        assert_rejected(|window| window.tool_window = true);
        assert_rejected(|window| window.owned = true);
        assert_rejected(|window| window.cloaked = Some(true));
    }

    #[test]
    fn unreadable_cloak_state_stays_eligible() {
        let target = classify(&sample_window(|window| window.cloaked = None))
            .expect("unknown cloak must not drop a candidate");
        assert_eq!(target.window_id(), WindowId::new(0x0000_0001_0000_1234));
    }

    #[test]
    fn desktop_and_taskbar_classes_are_rejected() {
        assert_rejected(|window| window.class_name = "Progman".to_string());
        assert_rejected(|window| window.class_name = "SHELL_TRAYWND".to_string());
        assert_rejected(|window| window.class_name = "Shell_SecondaryTrayWnd".to_string());
    }

    #[test]
    fn blank_or_whitespace_titles_are_rejected() {
        assert_rejected(|window| window.title = String::new());
        assert_rejected(|window| window.title = " \t\u{00a0}".to_string());
    }

    #[cfg(not(windows))]
    #[test]
    fn activation_requires_windows() {
        let target = ActivationTarget::from_window(&sample_window(|_| {})).unwrap();
        assert!(matches!(
            activate(target),
            Err(ActivationError::UnsupportedPlatform)
        ));
    }
}
