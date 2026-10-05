// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

/// How a window participates in the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowMode {
    /// Participates in tiling.
    Tiled,
    /// Managed but not tiled; excluded from layout proposals.
    Floating,
    /// Occupies a full output; excluded from layout proposals.
    Fullscreen,
}

/// Opaque identifier for an application window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(u64);

impl WindowId {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

/// An application window known to the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Window {
    id: WindowId,
    mode: WindowMode,
}

impl Window {
    pub fn new(id: WindowId, mode: WindowMode) -> Self {
        Self { id, mode }
    }

    pub fn id(&self) -> WindowId {
        self.id
    }

    pub fn mode(&self) -> WindowMode {
        self.mode
    }
}
