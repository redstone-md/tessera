// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The presentation engine is shared by the native guard and in-memory fixtures.
//! Its original backup is immutable; only the current validated window changes.

use super::placement::{CapturedAppbarState, CapturedPlacement};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TaskbarHide {
    pub placement: CapturedPlacement,
    pub appbar: CapturedAppbarState,
    pub visible: bool,
}

impl TaskbarHide {
    pub(crate) fn restore_command(self) -> u32 {
        if self.visible {
            self.placement.show_command
        } else {
            CapturedPlacement::SW_HIDE
        }
    }
}

pub(crate) trait TaskbarBackend {
    type Window: Copy;
    type Error;

    fn find(&mut self) -> Result<Option<Self::Window>, Self::Error>;
    fn capture(&mut self, window: Self::Window) -> Result<Option<TaskbarHide>, Self::Error>;
    fn hide(&mut self, window: Self::Window, original: TaskbarHide) -> Result<(), Self::Error>;
    fn restore(
        &mut self,
        window: Option<Self::Window>,
        original: TaskbarHide,
    ) -> Result<(), Self::Error>;
}

pub(crate) struct SessionPresentation<B: TaskbarBackend> {
    backend: B,
    original: Option<TaskbarHide>,
    current: Option<B::Window>,
    active: bool,
    dirty: bool,
}

impl<B: TaskbarBackend> SessionPresentation<B> {
    pub(crate) fn new(backend: B) -> Self {
        Self {
            backend,
            original: None,
            current: None,
            active: false,
            dirty: false,
        }
    }

    /// Capture before child startup, including a taskbar whose visibility is false.
    pub(crate) fn prepare(&mut self) -> Result<(), B::Error> {
        if let Some(window) = self.backend.find()? {
            let _ = self.bind(window)?;
        }
        Ok(())
    }

    fn bind(&mut self, window: B::Window) -> Result<Option<TaskbarHide>, B::Error> {
        let original = match self.original {
            Some(original) => original,
            None => {
                let Some(original) = self.backend.capture(window)? else {
                    return Ok(None);
                };
                self.original = Some(original);
                original
            }
        };
        self.current = Some(window);
        Ok(Some(original))
    }

    /// The caller has consumed readiness. Absence is valid; a later event captures once.
    pub(crate) fn hide_after_ready(&mut self) -> Result<(), B::Error> {
        self.active = true;
        self.refresh()
    }

    pub(crate) fn refresh(&mut self) -> Result<(), B::Error> {
        if self.active
            && let Some(window) = self.backend.find()?
        {
            self.rehide(window)?;
        }
        Ok(())
    }

    pub(crate) fn rehide(&mut self, window: B::Window) -> Result<(), B::Error> {
        if !self.active {
            return Ok(());
        }
        let Some(original) = self.bind(window)? else {
            return Ok(());
        };
        // Mark before the first mutation: even a partially failed hide must restore.
        self.dirty = true;
        self.backend.hide(window, original)
    }

    pub(crate) fn restore(&mut self) -> Result<(), B::Error> {
        self.active = false;
        if !self.dirty {
            return Ok(());
        }
        // HWNDs can be destroyed/reused. Always resolve the current validated taskbar,
        // including a replacement created just before the watcher was stopped.
        self.current = self.backend.find()?;
        if let Some(original) = self.original {
            self.backend.restore(self.current, original)?;
        }
        self.dirty = false;
        Ok(())
    }
}

/// Both cleanup operations run, even when stopping or restoration fails.
/// Restoration failures take precedence because they describe an unrecovered desktop.
pub(crate) fn finish_session<E>(
    outcome: Result<(), E>,
    stop: impl FnOnce() -> Result<(), E>,
    restore: impl FnOnce() -> Result<(), E>,
) -> Result<(), E> {
    let stopped = stop();
    let restored = restore();
    restored.and(stopped).and(outcome)
}
