// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Showing the desktop without Explorer: every application window is
//! minimized and remembered, and the next time the windows that are still
//! minimized come back in the order they were in, the one that was in
//! front on top and active.
//!
//! While Explorer runs, its taskbar does this better (it also brings back
//! windows that were minimized by hand in between, as Windows users know
//! it), so the callers ask [`Taskbar::toggle_desktop`](super::Taskbar)
//! first and use this where there is no taskbar, as when Tessera is the
//! shell.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;

use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetShellWindow, GetWindowLongPtrW, HWND_TOP, IsHungAppWindow, IsIconic,
    SHOW_WINDOW_CMD, SW_MINIMIZE, SW_RESTORE, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, ShowWindow, ShowWindowAsync,
    WS_EX_TOPMOST,
};

use super::{AppWindow, AppWindowId};

/// The windows put aside, topmost first; empty while the desktop is not
/// shown. Only windows that took the request to minimize are in it.
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
/// left, and the next press shows it again. A window that never took the
/// request to minimize, such as one of an elevated program, is not among
/// those put aside, so it does not count.
fn step<T: Copy>(aside: &[T], now: impl Fn(T) -> Now) -> Step<T> {
    if aside.iter().any(|&id| now(id) == Now::Shown) {
        return Step::PutAside;
    }
    match still_minimized(aside, now) {
        windows if windows.is_empty() => Step::PutAside,
        windows => Step::BringBack(windows),
    }
}

/// The windows of `aside` that are still minimized, in their order.
fn still_minimized<T: Copy>(aside: &[T], now: impl Fn(T) -> Now) -> Vec<T> {
    { aside.iter().copied() }
        .filter(|&id| now(id) == Now::Minimized)
        .collect()
}

/// What to remember once `minimized` are put aside, topmost first: those,
/// then the ones remembered `before` that are still minimized, each once.
/// The ones put aside before are not forgotten when the desktop is shown
/// again after one of them came back.
fn remembered<T: Copy + PartialEq>(
    minimized: Vec<T>,
    before: &[T],
    now: impl Fn(T) -> Now,
) -> Vec<T> {
    let mut windows = minimized;
    for id in still_minimized(before, now) {
        if !windows.contains(&id) {
            windows.push(id);
        }
    }
    windows
}

/// How `id` is minimized. The window in front would keep the keyboard
/// while minimized without being seen, so it is minimized in the way that
/// hands its place on, where `hands_on`; the others are minimized without
/// taking the foreground.
fn minimizing<T: PartialEq>(id: T, front: Option<&T>, hands_on: bool) -> SHOW_WINDOW_CMD {
    match hands_on && front == Some(&id) {
        true => SW_MINIMIZE,
        false => SW_SHOWMINNOACTIVE,
    }
}

/// Show desktop of Tessera's own.
pub struct ShowDesktop;

impl ShowDesktop {
    /// Minimizes every application window that is shown, or brings back
    /// the ones minimized so last time, if they are all still minimized
    /// or closed. Windows of Tessera itself stay. Windows may refuse to
    /// minimize a window of an elevated process; it then stays, and does
    /// not keep the others from coming back.
    pub fn toggle() {
        let mut aside = Self::aside();
        match step(&aside, state_of) {
            Step::PutAside => put_aside(&mut aside, None),
            Step::BringBack(windows) => {
                aside.clear();
                bring_back(windows);
            }
        }
    }

    /// Minimizes every application window that is shown, as Win+M does,
    /// and remembers them with those put aside before.
    pub fn minimize_all() {
        put_aside(&mut Self::aside(), None);
    }

    /// Brings back the windows put aside that are still minimized, as
    /// Win+Shift+M does, and forgets them.
    pub fn restore_all() {
        let mut aside = Self::aside();
        let windows = still_minimized(&aside, state_of);
        aside.clear();
        bring_back(windows);
    }

    /// Minimizes every application window that is shown but `keep`, as
    /// Win+Home does, and remembers them with those put aside before.
    pub fn minimize_others(keep: AppWindowId) {
        put_aside(&mut Self::aside(), Some(keep));
    }

    fn aside() -> MutexGuard<'static, Vec<AppWindowId>> {
        PUT_ASIDE.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Minimizes the application windows that are shown, but `keep`, and
/// remembers those that took the request with the ones in `aside`. Where
/// no window is kept, the keyboard goes to the desktop: to the window of
/// the shell where there is one, and otherwise to whatever window Windows
/// activates after the one in front.
fn put_aside(aside: &mut Vec<AppWindowId>, keep: Option<AppWindowId>) {
    let front = AppWindowId::foreground();
    let desktop = shell_window();
    let hands_on = keep.is_none() && desktop.is_none();
    let minimized: Vec<AppWindowId> = { AppWindow::enumerate().into_iter() }
        .map(|window| window.id)
        .filter(|&id| Some(id) != keep && state_of(id) == Now::Shown)
        .filter(|&id| {
            let command = minimizing(id, front.as_ref(), hands_on);
            // SAFETY: the call tolerates a handle whose window has since
            // been destroyed and does not wait for the window.
            unsafe { ShowWindowAsync(id.hwnd(), command) }.as_bool()
        })
        .collect();
    *aside = remembered(minimized, aside, state_of);
    if let (None, Some(desktop)) = (keep, desktop) {
        desktop.activate();
    }
}

/// The window of the desktop of the shell, where a shell has one.
fn shell_window() -> Option<AppWindowId> {
    // SAFETY: the call takes no arguments and only reads system state.
    let window = unsafe { GetShellWindow() };
    (!window.is_invalid()).then(|| AppWindowId::from_raw(window.0 as isize))
}

/// Brings `windows` back, topmost first, in that order, and activates the
/// first. It is done on a thread of its own: showing a window of another
/// program and setting its place in the order wait for that program,
/// which the thread of GPUI must not, and only waiting gives an order that
/// does not depend on which program answers first. A window that does not
/// answer is only asked to come back, without a place in the order.
fn bring_back(windows: Vec<AppWindowId>) {
    if windows.is_empty() {
        return;
    }
    let started = thread::Builder::new()
        .name("bring back".to_owned())
        .spawn(move || restore_in_order(&windows));
    if let Err(error) = started {
        report!("the windows put aside were not brought back: {error}");
    }
}

fn restore_in_order(windows: &[AppWindowId]) {
    // SAFETY: the call only reads whether the window answers lately.
    let hung = |id: &AppWindowId| unsafe { IsHungAppWindow(id.hwnd()) }.as_bool();
    let (hung, answering): (Vec<AppWindowId>, Vec<AppWindowId>) =
        windows.iter().copied().partition(hung);
    for id in hung {
        // SAFETY: the call tolerates a handle whose window has since been
        // destroyed and does not wait for the window.
        unsafe {
            let _ = ShowWindowAsync(id.hwnd(), SW_RESTORE);
        }
    }
    // The lowest first, without activating any.
    for id in answering.iter().rev() {
        // SAFETY: as above; this call waits for the window, on this
        // thread of its own.
        unsafe {
            let _ = ShowWindow(id.hwnd(), SW_SHOWNOACTIVATE);
        }
    }
    // Each one below the one that was above it. A window that stays on
    // top of the others is left where it is: put after another window, it
    // would lose that.
    let mut above = HWND_TOP;
    for id in answering.iter().filter(|id| !is_topmost(**id)) {
        // SAFETY: as above.
        unsafe {
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
            let _ = SetWindowPos(id.hwnd(), Some(above), 0, 0, 0, 0, flags);
        }
        above = id.hwnd();
    }
    if let Some(&top) = answering.first() {
        top.activate();
    }
}

/// Whether the window stays on top of the others.
fn is_topmost(id: AppWindowId) -> bool {
    // SAFETY: the call only reads the styles of the window.
    let styles = unsafe { GetWindowLongPtrW(id.hwnd(), GWL_EXSTYLE) };
    styles as u32 & WS_EX_TOPMOST.0 != 0
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

    #[test]
    fn a_window_that_did_not_minimize_keeps_none_from_coming_back() {
        // Window 2 refused to minimize, so only 1 and 3 were put aside.
        let now = |id| match id {
            2 => Now::Shown,
            _ => Now::Minimized,
        };
        assert_eq!(step(&[1, 3], now), Step::BringBack(vec![1, 3]));
    }

    #[test]
    fn the_windows_put_aside_before_are_not_forgotten() {
        // 3 came back by hand, 4 was closed, 5 is new; 3 and 5 are put
        // aside now.
        let now = |id| match id {
            3 | 5 => Now::Shown,
            4 => Now::Gone,
            _ => Now::Minimized,
        };
        let windows = remembered(vec![3, 5], &[1, 3, 2, 4], now);
        assert_eq!(windows, [3, 5, 1, 2]);
        let again = remembered(vec![1], &[1, 2], |_| Now::Minimized);
        assert_eq!(again, [1, 2]);
    }

    #[test]
    fn the_window_in_front_hands_its_place_on() {
        assert_eq!(minimizing(1, Some(&1), true), SW_MINIMIZE);
        assert_eq!(minimizing(2, Some(&1), true), SW_SHOWMINNOACTIVE);
        assert_eq!(minimizing(1, Some(&1), false), SW_SHOWMINNOACTIVE);
        assert_eq!(minimizing(1, None, true), SW_SHOWMINNOACTIVE);
    }
}
