// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the window switcher reads of a window beyond what
//! [`AppWindow`](super::AppWindow) lists, and how it comes in front.

use windows::Win32::UI::WindowsAndMessaging::IsIconic;

use super::{AppWindowId, NativeSurface};

impl AppWindowId {
    /// Whether the window is minimized at this moment. A window that is
    /// gone is not.
    pub fn is_minimized(self) -> bool {
        // SAFETY: the call only reads window state and tolerates a handle
        // whose window has since been destroyed.
        unsafe { IsIconic(self.hwnd()).as_bool() }
    }
}

impl NativeSurface {
    /// Brings the window in front with the keyboard, as a window of
    /// another program is brought there: Windows may refuse to do that
    /// when asked once, as after a keyboard hook took the keys that opened
    /// it, and is then asked in the ways [`AppWindowId::activate`] knows.
    pub fn bring_forward(self) {
        AppWindowId::from_raw(self.hwnd.0 as isize).activate();
    }
}
