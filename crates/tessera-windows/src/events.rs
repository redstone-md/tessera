// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Passive desktop-event observation.
//!
//! [`watch_desktop`] installs scoped out-of-context WinEvent hooks and
//! forwards change notifications; the UI coalesces them before observation.
//! This is notification-based observation only: no global keyboard or mouse
//! hooks, no window-text polling, no in-process injection, and no mutation
//! of any window. The guard returned by [`watch_desktop`] owns the hook and
//! its pump thread; dropping it stops the thread cleanly.
//!
//! Off Windows [`watch_desktop`] always returns
//! [`ApplicationError::UnsupportedPlatform`] and never starts a thread.

use crate::apps::ApplicationError;

/// Callback invoked when a desktop event may have changed the observed set
/// of windows. Called from the watcher's own thread; it must remain cheap —
/// expensive work belongs to the caller's refresh path.
pub type DesktopEventCallback = std::sync::Arc<dyn Fn() + Send + Sync>;

/// Watcher guard owned by the caller. Dropping it stops the hook and joins
/// its pump thread. Off Windows this is a zero-sized token that never
/// observes anything.
pub struct DesktopWatcher {
    #[cfg(windows)]
    _inner: crate::native_events::NativeWatcher,
}

impl DesktopWatcher {
    /// Stops the watcher early; also implied by `Drop`.
    ///
    /// Consumes the guard: the hook is unhooked, `WM_QUIT` is posted to the
    /// pump thread, and the thread is joined before this returns. Off
    /// Windows this is a no-op.
    pub fn stop(self) {}
}

/// Starts watching desktop window events until the returned guard drops.
///
/// `callback` is invoked from the owned message-pump thread for
/// foreground/create/destroy/show/hide/minimize/name/location window events.
/// The watcher's own process's object events are excluded, except
/// that foreground changes are always delivered so the shell can show or
/// hide its surfaces after its own flyout takes focus.
///
/// Fails when the hook or pump cannot be created; no partial watcher is
/// returned.
pub fn watch_desktop(callback: DesktopEventCallback) -> Result<DesktopWatcher, ApplicationError> {
    #[cfg(windows)]
    {
        crate::native_events::start(callback).map(|inner| DesktopWatcher { _inner: inner })
    }
    #[cfg(not(windows))]
    {
        let _ = callback;
        Err(ApplicationError::UnsupportedPlatform)
    }
}
