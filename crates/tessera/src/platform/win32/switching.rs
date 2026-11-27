// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the window switcher reads of a window beyond what
//! [`AppWindow`](super::AppWindow) lists.

use windows::Win32::UI::WindowsAndMessaging::IsIconic;

use super::AppWindowId;

impl AppWindowId {
    /// Whether the window is minimized at this moment. A window that is
    /// gone is not.
    pub fn is_minimized(self) -> bool {
        // SAFETY: the call only reads window state and tolerates a handle
        // whose window has since been destroyed.
        unsafe { IsIconic(self.hwnd()).as_bool() }
    }
}
