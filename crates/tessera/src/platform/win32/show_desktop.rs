// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Showing the desktop without Explorer: every application window is
//! minimized and remembered, and the next time the windows that are still
//! minimized come back, the one that was in front last on top.
//!
//! While Explorer runs, its taskbar does this better (it also brings back
//! windows that were minimized by hand in between, as Windows users know
//! it), so the callers ask [`Taskbar::toggle_desktop`](super::Taskbar)
//! first and use this where there is no taskbar, as when Tessera is the
//! shell.

use std::sync::{Mutex, PoisonError};

use windows::Win32::UI::WindowsAndMessaging::{
    IsIconic, SW_RESTORE, SW_SHOWMINNOACTIVE, ShowWindowAsync,
};

use super::{AppWindow, AppWindowId};

/// The windows put aside, topmost first; empty while the desktop is not
/// shown.
static PUT_ASIDE: Mutex<Vec<AppWindowId>> = Mutex::new(Vec::new());

/// What a window put aside is like now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Now {
    Gone,
    Minimized,
    Shown,
}

/// What a press of show desktop does.
#[derive(Debug, PartialEq, Eq)]
enum Step<T> {
    /// Minimizes the windows that are shown and remembers them.
    PutAside,
    /// Brings these back, topmost first, and forgets them.
    BringBack(Vec<T>),
}

/// What to do, given the windows put aside last time and what each one is
/// like now. Once the user brought one back by hand, the desktop counts as
/// left, and the next press shows it again.
fn step<T: Copy>(aside: &[T], now: impl Fn(T) -> Now) -> Step<T> {
    let states: Vec<(T, Now)> = aside.iter().map(|&id| (id, now(id))).collect();
    if states.iter().any(|&(_, state)| state == Now::Shown) {
        return Step::PutAside;
    }
    let minimized: Vec<T> = { states.into_iter() }
        .filter(|&(_, state)| state == Now::Minimized)
        .map(|(id, _)| id)
        .collect();
    match minimized.is_empty() {
        true => Step::PutAside,
        false => Step::BringBack(minimized),
    }
}

/// Show desktop of Tessera's own.
pub struct ShowDesktop;

impl ShowDesktop {
    /// Minimizes every application window that is shown, or brings back
    /// the ones minimized so last time, if they are all still minimized
    /// or closed. Windows of Tessera itself stay. Windows may refuse to
    /// minimize a window of an elevated process.
    pub fn toggle() {
        let mut aside = PUT_ASIDE.lock().unwrap_or_else(PoisonError::into_inner);
        match step(&aside, state_of) {
            Step::PutAside => {
                let shown: Vec<AppWindowId> = { AppWindow::enumerate().into_iter() }
                    .map(|window| window.id)
                    .filter(|&id| state_of(id) == Now::Shown)
                    .collect();
                for &id in &shown {
                    // SAFETY: the call tolerates a handle whose window has
                    // since been destroyed and does not wait for the window.
                    unsafe {
                        let _ = ShowWindowAsync(id.hwnd(), SW_SHOWMINNOACTIVE);
                    }
                }
                *aside = shown;
            }
            Step::BringBack(windows) => {
                aside.clear();
                // The lowest first, so that each one comes over the ones
                // that were below it.
                for &id in windows.iter().rev() {
                    // SAFETY: as above.
                    unsafe {
                        let _ = ShowWindowAsync(id.hwnd(), SW_RESTORE);
                    }
                }
                if let Some(&top) = windows.first() {
                    top.activate();
                }
            }
        }
    }
}

fn state_of(id: AppWindowId) -> Now {
    if !id.exists() || !id.is_shown() {
        return Now::Gone;
    }
    // SAFETY: the call only reads window state.
    match unsafe { IsIconic(id.hwnd()) }.as_bool() {
        true => Now::Minimized,
        false => Now::Shown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_press_puts_the_windows_aside() {
        assert_eq!(step::<u8>(&[], |_| Now::Shown), Step::PutAside);
    }

    #[test]
    fn the_next_press_brings_back_the_ones_still_minimized() {
        let now = |id| match id {
            2 => Now::Gone,
            _ => Now::Minimized,
        };
        assert_eq!(step(&[1, 2, 3], now), Step::BringBack(vec![1, 3]));
    }

    #[test]
    fn a_window_brought_back_by_hand_leaves_the_desktop() {
        let now = |id| match id {
            3 => Now::Shown,
            _ => Now::Minimized,
        };
        assert_eq!(step(&[1, 2, 3], now), Step::PutAside);
        assert_eq!(step(&[1, 2], |_| Now::Gone), Step::PutAside);
    }
}
