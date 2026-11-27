// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Whether Caps Lock and Num Lock are on, and which window is in front,
//! for the flyouts that show a change of them.

use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL, VK_NUMLOCK};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// The keys that stay on once pressed, as they are now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyLocks {
    pub caps: bool,
    pub num: bool,
}

impl KeyLocks {
    /// Reads the keys. Windows keeps their state for the whole session, so
    /// it is right on any thread, whichever window has the keyboard.
    pub fn read() -> Self {
        // The lowest bit of the answer tells whether the key is toggled on.
        // SAFETY: the calls only read the state of the keyboard.
        let toggled = |key| unsafe { GetKeyState(i32::from(key)) } & 1 == 1;
        Self {
            caps: toggled(VK_CAPITAL.0),
            num: toggled(VK_NUMLOCK.0),
        }
    }
}

/// The window in front, as a number that tells one window from another;
/// 0 while there is none.
pub fn window_in_front() -> isize {
    // SAFETY: the call only reads which window is in front.
    unsafe { GetForegroundWindow() }.0 as isize
}
