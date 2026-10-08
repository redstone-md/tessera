// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use crate::dto::valid_key;

/// How the applications menu fills its selected monitor.
///
/// Fullscreen is a monitor-sized activatable overlay, not native exclusive fullscreen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LauncherDisplayMode {
    #[default]
    Windowed,
    Fullscreen,
}

/// Persisted applications-menu choices, independent of dock pins and appearance.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LauncherPreferences {
    display_mode: LauncherDisplayMode,
    favorites: Vec<String>,
}

impl LauncherPreferences {
    /// Saved applications-menu presentation mode.
    pub fn display_mode(&self) -> LauncherDisplayMode {
        self.display_mode
    }

    /// Exact application identities in saved favorite order.
    pub fn favorites(&self) -> &[String] {
        &self.favorites
    }

    pub(crate) fn with_display_mode(mut self, mode: LauncherDisplayMode) -> Self {
        self.display_mode = mode;
        self
    }

    /// Validate the complete collection before replacing it; retain exact first occurrences.
    pub(crate) fn with_favorites(mut self, favorites: Vec<String>) -> Result<Self, String> {
        let mut ordered = Vec::with_capacity(favorites.len());
        let mut seen = std::collections::HashSet::new();
        for favorite in favorites {
            if !valid_key(&favorite) {
                return Err("Launcher favorite identities must be nonempty, control-free, and at most 1024 UTF-16 units".into());
            }
            if seen.insert(favorite.clone()) {
                ordered.push(favorite);
            }
        }
        self.favorites = ordered;
        Ok(self)
    }
}
