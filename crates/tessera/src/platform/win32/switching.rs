// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How the window switcher comes in front. Whether a window is minimized
//! is [`AppWindowId::is_minimized`], beside the other states of a window.

use super::{AppWindowId, NativeSurface};

impl NativeSurface {
    /// Brings the window in front with the keyboard, as a window of
    /// another program is brought there: Windows may refuse to do that
    /// when asked once, as after a keyboard hook took the keys that opened
    /// it, and is then asked in the ways [`AppWindowId::activate`] knows.
    pub fn bring_forward(self) {
        AppWindowId::from_raw(self.hwnd.0 as isize).activate();
    }
}
