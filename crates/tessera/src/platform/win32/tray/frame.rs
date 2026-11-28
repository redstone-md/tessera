// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Where programs find the taskbar: the rectangle of `Shell_TrayWnd`, the
//! windows inside it that they look for with `FindWindowEx`, and one
//! `Shell_SecondaryTrayWnd` for each other display with a bar.
//!
//! Explorer's taskbar holds `TrayNotifyWnd`, the notification area, with
//! the clock (`TrayClockWClass`) and the icons (`SysPager` holding a
//! `ToolbarWindow32`), and `ReBarWindow32`, which holds the buttons of
//! the windows (`MSTaskSwWClass` holding `MSTaskListWClass`). Programs
//! that place a window beside the clock or the icons, or that measure the
//! taskbar, read the rectangles of these windows. Tessera's window of the
//! class gets the same children, made on the tray thread, and they stand
//! where Tessera's taskbar is:
//!
//! - where the shell serves the edge bars, `Shell_TrayWnd` stands where
//!   the taskbar stands when shown (the surface appbar.rs says is the
//!   taskbar); the notification area covers the icons the tray widget said
//!   it shows on that bar, with the clock after them, where Explorer has
//!   it, and the buttons take the longer part of the bar left beside it.
//!   Where the tray widget shows no icon on that bar, the notification area
//!   is the end of the bar. Where Tessera draws its clock and its dock is
//!   not told to the tray thread, so the clock's window and the buttons'
//!   windows stand where Explorer would have them;
//! - beside Explorer they stand where Explorer's own stand, as a program
//!   finds Tessera's window first.
//!
//! The windows are never shown, so they take no clicks from the bars, and
//! they answer nothing of what Explorer's answer: a program that counts
//! the buttons of the toolbar of the icons hears of none. Messages in the
//! range of `ITaskbarList` that a program sends to `MSTaskSwWClass` go on
//! to the window that takes them (taskbar_list.rs) while Tessera is the
//! shell.

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, GetWindowRect, HMENU,
    RegisterClassW, SMTO_ABORTIFHUNG, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageTimeoutW,
    SetWindowPos, WINDOW_EX_STYLE, WM_USER, WNDCLASSW, WS_CHILD, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use super::super::ScreenRect;

/// The class of the taskbar of a display other than the primary one.
const SECONDARY_CLASS: PCWSTR = w!("Shell_SecondaryTrayWnd");

/// The classes of the windows inside the taskbar, each with its parent:
/// the index of another entry, or `None` for `Shell_TrayWnd` itself.
const CHILDREN: [(PCWSTR, Option<usize>); 7] = [
    (w!("TrayNotifyWnd"), None),
    (w!("TrayClockWClass"), Some(NOTIFY)),
    (w!("SysPager"), Some(NOTIFY)),
    (w!("ToolbarWindow32"), Some(PAGER)),
    (w!("ReBarWindow32"), None),
    (w!("MSTaskSwWClass"), Some(REBAR)),
    (w!("MSTaskListWClass"), Some(TASK_SWITCH)),
];

/// Where in [`CHILDREN`] the windows are.
const NOTIFY: usize = 0;
const CLOCK: usize = 1;
const PAGER: usize = 2;
const TOOLBAR: usize = 3;
const REBAR: usize = 4;
const TASK_SWITCH: usize = 5;

/// The private messages of a window class, which `ITaskbarList` uses.
const PRIVATE_MESSAGES: std::ops::Range<u32> = WM_USER..0x8000;

/// The messages `RegisterWindowMessage` hands out, such as `SHELLHOOK`.
const REGISTERED_MESSAGES: std::ops::RangeInclusive<u32> = 0xC000..=0xFFFF;

/// How long a message passed on to the window of `ITaskbarList` may
/// take, in milliseconds.
const PASS_ON_WAIT: u32 = 1000;

/// The rectangles of the taskbar and of the windows inside it, on the
/// screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    pub tray: ScreenRect,
    /// The notification area, with the clock and the icons.
    pub notify: ScreenRect,
    pub clock: ScreenRect,
    /// The icons.
    pub icons: ScreenRect,
    /// The buttons of the windows.
    pub buttons: ScreenRect,
}

impl Layout {
    /// The rectangle of the window at `index` of [`CHILDREN`].
    fn of_child(&self, index: usize) -> ScreenRect {
        match index {
            NOTIFY => self.notify,
            CLOCK => self.clock,
            PAGER | TOOLBAR => self.icons,
            _ => self.buttons,
        }
    }
}

/// The part of `bar` from `from` to `to` along it: across it for a bar at
/// the top or the bottom, down it for one at a side.
fn part(bar: ScreenRect, horizontal: bool, from: i32, to: i32) -> ScreenRect {
    match horizontal {
        true => ScreenRect {
            left: from,
            right: to,
            ..bar
        },
        false => ScreenRect {
            top: from,
            bottom: to,
            ..bar
        },
    }
}

/// How the taskbar is laid out when it lies at `edge` at `bar` and the
/// notification icons it shows lie at `icons` (those elsewhere are left
/// out): the notification area covers the icons with the clock after
/// them, as long as two depths of the bar, and the buttons take the
/// longer part of the bar beside it. Without icons on the bar the
/// notification area is its end: six depths, at most a third of it.
pub(super) fn lay_out(edge: u32, bar: ScreenRect, icons: &[ScreenRect]) -> Layout {
    let horizontal = matches!(edge, ABE_TOP | ABE_BOTTOM);
    let (start, end, depth) = match horizontal {
        true => (bar.left, bar.right, bar.bottom - bar.top),
        false => (bar.top, bar.bottom, bar.right - bar.left),
    };
    let length = (end - start).max(0);
    let clock_length = (depth * 2).min(length / 4).max(0);
    let on_bar = |icon: &&ScreenRect| {
        let (x, y) = ((icon.left + icon.right) / 2, (icon.top + icon.bottom) / 2);
        bar.left <= x && x < bar.right && bar.top <= y && y < bar.bottom
    };
    let shown: Vec<&ScreenRect> = icons.iter().filter(on_bar).collect();
    let along = |icon: &ScreenRect| match horizontal {
        true => (icon.left, icon.right),
        false => (icon.top, icon.bottom),
    };
    let first = shown.iter().map(|icon| along(icon).0).min();
    let last = shown.iter().map(|icon| along(icon).1).max();
    let (notify, clock, icons) = match (first, last) {
        (Some(first), Some(last)) => {
            let (first, last) = (first.clamp(start, end), last.clamp(start, end));
            let notify_end = (last + clock_length).min(end);
            let clock_start = (notify_end - clock_length).max(first);
            (
                part(bar, horizontal, first, notify_end),
                part(bar, horizontal, clock_start, notify_end),
                part(bar, horizontal, first, last),
            )
        }
        _ => {
            let notify_start = end - (depth * 6).min(length / 3).max(0);
            (
                part(bar, horizontal, notify_start, end),
                part(bar, horizontal, end - clock_length, end),
                part(bar, horizontal, notify_start, end - clock_length),
            )
        }
    };
    let (notify_start, notify_end) = along(&notify);
    let buttons = match notify_start - start >= end - notify_end {
        true => part(bar, horizontal, start, notify_start),
        false => part(bar, horizontal, notify_end, end),
    };
    Layout {
        tray: bar,
        notify,
        clock,
        icons,
        buttons,
    }
}

/// The edge of `display` that `bar` lies at: the top or the bottom for a
/// bar wider than deep, a side otherwise, whichever it is nearer.
pub(super) fn edge_within(bar: ScreenRect, display: ScreenRect) -> u32 {
    let (width, height) = (bar.right - bar.left, bar.bottom - bar.top);
    if width >= height {
        match bar.top - display.top <= display.bottom - bar.bottom {
            true => ABE_TOP,
            false => ABE_BOTTOM,
        }
    } else {
        match bar.left - display.left <= display.right - bar.right {
            true => ABE_LEFT,
            false => ABE_RIGHT,
        }
    }
}

/// The windows made on the tray thread.
#[derive(Default)]
struct Made {
    /// The windows inside `Shell_TrayWnd`, in the order of [`CHILDREN`].
    children: Vec<HWND>,
    /// What they were last placed at.
    placed: Option<Layout>,
    /// The secondary taskbars, each with the frame of its display and its
    /// place.
    secondary: Vec<(ScreenRect, HWND, ScreenRect)>,
}

thread_local! {
    static MADE: RefCell<Made> = RefCell::default();
}

/// The window `index` of [`CHILDREN`] passes messages of `ITaskbarList`
/// on.
fn passes_on(index: usize) -> bool {
    index == TASK_SWITCH
}

/// Makes the windows inside `tray`, on its thread. Those that cannot be
/// made are reported, and the others are made all the same.
pub(super) fn make(tray: HWND) {
    // SAFETY: the class structures and the names outlive the calls, and
    // the window procedures have the signature of one.
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            return;
        };
        for (index, (class, _)) in CHILDREN.iter().enumerate() {
            let procedure = match passes_on(index) {
                true => pass_on,
                false => plain,
            };
            let class = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: module.into(),
                lpszClassName: *class,
                ..Default::default()
            };
            RegisterClassW(&raw const class);
        }
        let secondary = WNDCLASSW {
            lpfnWndProc: Some(plain),
            hInstance: module.into(),
            lpszClassName: SECONDARY_CLASS,
            ..Default::default()
        };
        RegisterClassW(&raw const secondary);

        let mut children: Vec<HWND> = Vec::new();
        for (index, (class, parent)) in CHILDREN.iter().enumerate() {
            let parent = match parent {
                Some(parent) => children.get(*parent).copied(),
                None => Some(tray),
            };
            let made = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                *class,
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                0,
                0,
                parent,
                Some(HMENU(index as *mut c_void)),
                Some(module.into()),
                None,
            );
            match made {
                Ok(window) => children.push(window),
                Err(error) => {
                    report!("programs do not find all the windows of the taskbar: {error}");
                    return;
                }
            }
        }
        MADE.with_borrow_mut(|made| made.children = children);
    }
}

/// Puts `tray` and the windows inside it at `layout`, unless they are
/// there already. A child is placed within its parent.
pub(super) fn place(tray: HWND, layout: Layout) {
    let (children, before) = MADE.with_borrow(|made| (made.children.clone(), made.placed));
    if before == Some(layout) {
        return;
    }
    move_to(tray, layout.tray, None);
    for (index, child) in children.iter().enumerate() {
        let parent = match CHILDREN[index].1 {
            Some(parent) => layout.of_child(parent),
            None => layout.tray,
        };
        move_to(*child, layout.of_child(index), Some(parent));
    }
    MADE.with_borrow_mut(|made| made.placed = Some(layout));
}

/// Moves `window` to `rect` of the screen, within the client area of a
/// parent at `parent`, or on the screen where it has none.
fn move_to(window: HWND, rect: ScreenRect, parent: Option<ScreenRect>) {
    let (x, y) = parent.map_or((rect.left, rect.top), |parent| {
        (rect.left - parent.left, rect.top - parent.top)
    });
    let (width, height) = (
        (rect.right - rect.left).max(0),
        (rect.bottom - rect.top).max(0),
    );
    // SAFETY: the window belongs to this thread; the call passes numbers.
    unsafe {
        let _ = SetWindowPos(
            window,
            None,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

/// The rectangle of `window` on the screen; the call sends it nothing.
fn rect_of(window: HWND) -> Option<ScreenRect> {
    let mut rect = RECT::default();
    // SAFETY: `rect` outlives the call, which only reads.
    unsafe { GetWindowRect(window, &raw mut rect) }.ok()?;
    Some(rect.into())
}

/// The frame of the display `window` is on.
fn display_frame(window: HWND) -> Option<ScreenRect> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is correctly sized and outlives the calls, which only
    // read.
    unsafe {
        let display = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(display, &raw mut info)
            .as_bool()
            .then(|| info.rcMonitor.into())
    }
}

/// The first window of `class` inside `parent`.
fn child_of(parent: HWND, class: PCWSTR) -> Option<HWND> {
    // SAFETY: the class is a NUL-terminated literal; the call only reads.
    let found = unsafe { FindWindowExW(Some(parent), None, class, PCWSTR::null()) }.ok()?;
    (!found.is_invalid()).then_some(found)
}

/// Where Explorer's taskbar `explorer` and the windows inside it stand:
/// those it has where they are, the others where [`lay_out`] puts them.
pub(super) fn explorer_layout(explorer: HWND) -> Option<Layout> {
    let tray = rect_of(explorer)?;
    let display = display_frame(explorer)?;
    let mut layout = lay_out(edge_within(tray, display), tray, &[]);
    let found = |parent: Option<HWND>, class: PCWSTR| {
        let window = child_of(parent?, class)?;
        Some((window, rect_of(window)?))
    };
    let notify = found(Some(explorer), CHILDREN[NOTIFY].0);
    if let Some((_, rect)) = notify {
        layout.notify = rect;
    }
    let notify = notify.map(|(window, _)| window);
    if let Some((_, rect)) = found(notify, CHILDREN[CLOCK].0) {
        layout.clock = rect;
    }
    if let Some((_, rect)) = found(notify, CHILDREN[PAGER].0) {
        layout.icons = rect;
    }
    if let Some((_, rect)) = found(Some(explorer), CHILDREN[REBAR].0) {
        layout.buttons = rect;
    }
    Some(layout)
}

/// Makes, moves and destroys the secondary taskbars so that there is one
/// at each of `bars`: the frame of a display, the edge, and the bar.
pub(super) fn follow_secondaries(bars: &[(ScreenRect, u32, ScreenRect)]) {
    let mut made = MADE.with_borrow_mut(|made| std::mem::take(&mut made.secondary));
    made.retain(|(display, window, _)| {
        let kept = bars.iter().any(|(other, ..)| other == display);
        if !kept {
            // SAFETY: the window was made on this thread and is not used
            // again.
            unsafe {
                let _ = DestroyWindow(*window);
            }
        }
        kept
    });
    for &(display, _, bar) in bars {
        match made.iter_mut().find(|(other, ..)| *other == display) {
            Some((_, window, placed)) => {
                if *placed != bar {
                    move_to(*window, bar, None);
                    *placed = bar;
                }
            }
            None => {
                if let Some(window) = make_secondary(bar) {
                    made.push((display, window, bar));
                }
            }
        }
    }
    MADE.with_borrow_mut(|kept| kept.secondary = made);
}

/// A secondary taskbar at `bar`, never shown.
fn make_secondary(bar: ScreenRect) -> Option<HWND> {
    // SAFETY: the class was registered by `make`; the window belongs to
    // this thread.
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            SECONDARY_CLASS,
            PCWSTR::null(),
            WS_POPUP,
            bar.left,
            bar.top,
            bar.right - bar.left,
            bar.bottom - bar.top,
            None,
            None,
            Some(module.into()),
            None,
        )
        .ok()
    }
}

/// The window procedure of the windows that only stand where they are.
unsafe extern "system" fn plain(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: the arguments are passed on as they came.
    unsafe { DefWindowProcW(window, message, w, l) }
}

/// The window procedure of `MSTaskSwWClass`: what a program sends it in
/// the ranges of `ITaskbarList` goes on to the window that takes those
/// calls while Tessera is the shell, and its answer comes back.
unsafe extern "system" fn pass_on(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let ranged = PRIVATE_MESSAGES.contains(&message) || REGISTERED_MESSAGES.contains(&message);
    let band = ranged.then(super::super::taskbar_list::own_band).flatten();
    let Some(band) = band else {
        // SAFETY: the arguments are passed on as they came.
        return unsafe { DefWindowProcW(window, message, w, l) };
    };
    let mut answer = 0usize;
    // SAFETY: the band is a window of this process on the thread of the
    // shell host, which never waits for this one; the call gives up after
    // a while, and `answer` outlives it.
    unsafe {
        SendMessageTimeoutW(
            band,
            message,
            w,
            l,
            SMTO_ABORTIFHUNG,
            PASS_ON_WAIT,
            Some(&raw mut answer),
        );
    }
    LRESULT(answer as isize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn the_notification_area_covers_the_icons_and_the_clock_after_them() {
        let bar = rect(0, 1032, 1920, 1080);
        let icons = [
            rect(1500, 1040, 1532, 1072),
            rect(1532, 1040, 1564, 1072),
            // An icon of the overflow, elsewhere, is left out.
            rect(1500, 700, 1532, 732),
        ];
        let layout = lay_out(ABE_BOTTOM, bar, &icons);
        assert_eq!(layout.tray, bar);
        assert_eq!(layout.icons, rect(1500, 1032, 1564, 1080));
        assert_eq!(layout.notify, rect(1500, 1032, 1660, 1080));
        assert_eq!(layout.clock, rect(1564, 1032, 1660, 1080));
        assert_eq!(layout.buttons, rect(0, 1032, 1500, 1080));
    }

    #[test]
    fn icons_at_the_start_leave_the_buttons_the_rest() {
        let bar = rect(0, 0, 40, 1080);
        let icons = [rect(4, 10, 36, 42)];
        let layout = lay_out(ABE_LEFT, bar, &icons);
        assert_eq!(layout.notify, rect(0, 10, 40, 122));
        assert_eq!(layout.buttons, rect(0, 122, 40, 1080));
    }

    #[test]
    fn without_icons_the_notification_area_is_the_end_of_the_bar() {
        let bar = rect(0, 0, 1920, 48);
        let layout = lay_out(ABE_TOP, bar, &[]);
        assert_eq!(layout.notify, rect(1632, 0, 1920, 48));
        assert_eq!(layout.clock, rect(1824, 0, 1920, 48));
        assert_eq!(layout.icons, rect(1632, 0, 1824, 48));
        assert_eq!(layout.buttons, rect(0, 0, 1632, 48));
        // A bar too short for any of it keeps every part inside it.
        let tiny = lay_out(ABE_TOP, rect(0, 0, 0, 48), &[]);
        assert_eq!(tiny.notify, rect(0, 0, 0, 48));
    }

    #[test]
    fn the_edge_of_a_bar_is_the_one_it_lies_nearest() {
        let display = rect(0, 0, 1920, 1080);
        assert_eq!(edge_within(rect(0, 1032, 1920, 1080), display), ABE_BOTTOM);
        assert_eq!(edge_within(rect(0, 0, 1920, 48), display), ABE_TOP);
        assert_eq!(edge_within(rect(1880, 0, 1920, 1080), display), ABE_RIGHT);
        assert_eq!(edge_within(rect(0, 0, 40, 1080), display), ABE_LEFT);
    }
}
