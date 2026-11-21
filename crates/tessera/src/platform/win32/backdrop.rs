// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Putting a window where the wallpaper is: behind the icons of the
//! desktop.
//!
//! Windows documents no way to do it. Since Windows 11 24H2 the desktop
//! window of Explorer, `Progman`, holds the icons (`SHELLDLL_DefView`) and
//! below them the wallpaper (`WorkerW`) as its children, once it was sent
//! an undocumented message; a window made a child between the two is
//! drawn over the wallpaper and under the icons. Earlier versions lay the
//! desktop out differently, which is not handled here.

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::MapWindowPoints;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, LWA_ALPHA, SMTO_NORMAL,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SendMessageTimeoutW, SetLayeredWindowAttributes, SetParent,
    SetWindowLongPtrW, SetWindowPos, WS_CHILD, WS_EX_LAYERED, WS_VISIBLE,
};
use windows::core::w;

use super::NativeSurface;

/// The message that has `Progman` lay the desktop out with the wallpaper
/// as a child of its own, and the parameters it is known to take.
const RAISE_DESKTOP: u32 = 0x052C;
const RAISE_DESKTOP_W: usize = 0xD;
const RAISE_DESKTOP_L: isize = 1;

/// How long `Progman` gets to answer, in milliseconds.
const ANSWER_TIME: u32 = 1000;

impl NativeSurface {
    /// Makes the window fill its display behind the icons of the desktop
    /// and over the wallpaper of Windows. It then gets no input. Fails
    /// where the desktop is not laid out as Windows 11 24H2 does.
    pub fn put_behind_desktop_icons(self) -> Result<()> {
        let frame = self.display_frame()?;
        // SAFETY: the calls look windows up by class, send one message
        // with a time limit and change styles, parent and position of
        // `hwnd`, a live window owned by this thread; `corners` outlives
        // the call that maps it.
        unsafe {
            let desktop = FindWindowW(w!("Progman"), None).context("there is no desktop window")?;
            let parts = |desktop: HWND| {
                let icons =
                    FindWindowExW(Some(desktop), None, w!("SHELLDLL_DefView"), None).ok()?;
                FindWindowExW(Some(desktop), None, w!("WorkerW"), None).ok()?;
                Some(icons)
            };
            let icons = match parts(desktop) {
                Some(icons) => icons,
                None => {
                    SendMessageTimeoutW(
                        desktop,
                        RAISE_DESKTOP,
                        WPARAM(RAISE_DESKTOP_W),
                        LPARAM(RAISE_DESKTOP_L),
                        SMTO_NORMAL,
                        ANSWER_TIME,
                        None,
                    );
                    let Some(icons) = parts(desktop) else {
                        bail!("the desktop of this Windows is not laid out in a way Tessera knows");
                    };
                    icons
                }
            };

            SetWindowLongPtrW(self.hwnd, GWL_STYLE, (WS_CHILD | WS_VISIBLE).0 as isize);
            let extended = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, extended | WS_EX_LAYERED.0 as isize);
            SetLayeredWindowAttributes(self.hwnd, COLORREF(0), u8::MAX, LWA_ALPHA)
                .context("the window could not be made a layer of the desktop")?;
            SetParent(self.hwnd, Some(desktop))
                .context("the window could not be given to the desktop")?;

            // A child is placed in the coordinates of its parent.
            let mut corners = [
                POINT {
                    x: frame.left,
                    y: frame.top,
                },
                POINT {
                    x: frame.right,
                    y: frame.bottom,
                },
            ];
            MapWindowPoints(None, Some(desktop), &mut corners);
            // After the icons in the order of the children is below them,
            // and the wallpaper stays the last.
            SetWindowPos(
                self.hwnd,
                Some(icons),
                corners[0].x,
                corners[0].y,
                corners[1].x - corners[0].x,
                corners[1].y - corners[0].y,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("the window could not be put behind the icons")
        }
    }
}
