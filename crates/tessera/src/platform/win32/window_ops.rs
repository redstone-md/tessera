// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! More of what can be done to the window of another program: maximize,
//! minimize and restore it, keep it above the others, make it see-through,
//! hide its border, and read and set where it goes back to when it is
//! restored. Also: lower it or keep it below the others, take its title
//! bar away, have it fill its display without a frame, ask whether a
//! point is on its title bar, and begin moving or resizing it with the
//! keyboard.
//!
//! These are kept apart from the listing of windows in app_windows.rs,
//! which the dock extends on its own.
//!
//! Windows does none of this to a window of a process that runs with more
//! rights than Tessera, and says nothing about it. A request that sends
//! the window a message is posted, or made from a short thread of its own
//! and not at all to a window whose program hangs, so that such a program
//! never holds the shell up.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, Once, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DWMWA_COLOR_NONE;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_SYSTEM_FOREGROUND, FindWindowExW, GetMessageW, HTCAPTION, HWND_BOTTOM,
    MSG, PostMessageW, SC_MOVE, SC_SIZE, SET_WINDOW_POS_FLAGS, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED,
    SWP_NOZORDER, ShowWindow, TranslateMessage, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    WM_SYSCOMMAND, WS_CAPTION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetLayeredWindowAttributes, GetWindowLongPtrW, GetWindowPlacement,
    GetWindowRect, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, IsHungAppWindow, IsIconic, IsZoomed,
    LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA, SHOW_WINDOW_CMD, SM_CXBORDER, SM_CXFIXEDFRAME,
    SM_CXPADDEDBORDER, SM_CXSIZEFRAME, SM_CYBORDER, SM_CYFIXEDFRAME, SM_CYSIZEFRAME, SW_MAXIMIZE,
    SW_MINIMIZE, SW_RESTORE, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SYSTEM_METRICS_INDEX, SetLayeredWindowAttributes, SetWindowLongPtrW,
    SetWindowPlacement, SetWindowPos, ShowWindowAsync, WINDOWPLACEMENT, WS_DLGFRAME, WS_EX_LAYERED,
    WS_EX_TOPMOST, WS_THICKFRAME,
};

use windows::core::{PCWSTR, w};

use super::{AppWindowId, ScreenRect};

/// The least a window is made opaque, so that it can still be found.
const LEAST_OPACITY: f32 = 0.1;

/// The windows Tessera made layered to make them see-through, so that it
/// takes that away again when they are opaque again, and only from them.
static LAYERED: Mutex<Option<HashSet<isize>>> = Mutex::new(None);

/// The invisible border of windows as it was last measured while they
/// were neither maximized nor minimized, when it is wider or narrower
/// then, by window.
static MARGINS: Mutex<Option<HashMap<isize, [i32; 4]>>> = Mutex::new(None);

/// How many windows the invisible border is remembered for before those
/// that are gone are forgotten.
const MARGINS_KEPT: usize = 256;

/// How much the invisible border of a window reaches beyond what is seen
/// of it, on the left, top, right and bottom: `whole` is the window as
/// Windows places it, `seen` what is drawn of it.
fn margins(seen: ScreenRect, whole: ScreenRect) -> [i32; 4] {
    [
        seen.left - whole.left,
        seen.top - whole.top,
        whole.right - seen.right,
        whole.bottom - seen.bottom,
    ]
}

/// `rect` grown by `margins`, or shrunk by them with `inward`.
fn widened(rect: ScreenRect, [left, top, right, bottom]: [i32; 4], inward: bool) -> ScreenRect {
    let sign = if inward { -1 } else { 1 };
    ScreenRect {
        left: rect.left - sign * left,
        top: rect.top - sign * top,
        right: rect.right + sign * right,
        bottom: rect.bottom + sign * bottom,
    }
}

/// `rect` moved by `(x, y)`.
fn shifted(rect: ScreenRect, (x, y): (i32, i32)) -> ScreenRect {
    ScreenRect {
        left: rect.left + x,
        top: rect.top + y,
        right: rect.right + x,
        bottom: rect.bottom + y,
    }
}

/// How far the work area of the display that holds most of `rect` lies
/// from the corner of that display. Windows gives and takes the place a
/// window is restored to in coordinates that start at that corner of the
/// work area rather than of the screen.
fn workspace_offset(rect: ScreenRect) -> (i32, i32) {
    let rect = RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `rect` and `info` outlive the calls, and `info` is sized as
    // the call is told; the calls only read the arrangement of displays.
    let read = unsafe {
        let display = MonitorFromRect(&raw const rect, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(display, &raw mut info).as_bool()
    };
    match read {
        true => (
            info.rcWork.left - info.rcMonitor.left,
            info.rcWork.top - info.rcMonitor.top,
        ),
        false => (0, 0),
    }
}

impl AppWindowId {
    /// Whether the window is maximized.
    pub fn is_maximized(self) -> bool {
        // SAFETY: the call only reads window state and tolerates a handle
        // whose window has since been destroyed.
        unsafe { IsZoomed(self.hwnd()).as_bool() }
    }

    /// Whether the window is minimized at this moment. A window that is
    /// gone is not.
    pub fn is_minimized(self) -> bool {
        // SAFETY: as in `is_maximized`.
        unsafe { IsIconic(self.hwnd()).as_bool() }
    }

    /// Whether the window has stopped taking its messages, as a program
    /// that hangs does.
    pub fn is_hung(self) -> bool {
        // SAFETY: as in `is_maximized`.
        unsafe { IsHungAppWindow(self.hwnd()).as_bool() }
    }

    /// Asks the window to be maximized, as its button does.
    pub fn maximize(self) {
        self.show_async(SW_MAXIMIZE);
    }

    /// Asks the window to be minimized.
    pub fn minimize(self) {
        self.show_async(SW_MINIMIZE);
    }

    /// Asks a maximized or minimized window to take the size it had
    /// before.
    pub fn restore(self) {
        self.show_async(SW_RESTORE);
    }

    fn show_async(self, how: SHOW_WINDOW_CMD) {
        // SAFETY: the call is posted, tolerates a handle whose window has
        // since been destroyed and fails without side effects.
        unsafe {
            let _ = ShowWindowAsync(self.hwnd(), how);
        }
    }

    /// Whether the window is kept above the windows that are not.
    pub fn is_topmost(self) -> bool {
        // SAFETY: reading the style of a window, valid or not, has no
        // preconditions.
        let extended = unsafe { GetWindowLongPtrW(self.hwnd(), GWL_EXSTYLE) } as u32;
        extended & WS_EX_TOPMOST.0 != 0
    }

    /// Keeps the window above the windows that are not, or no longer.
    /// Whether Windows took the request is returned.
    pub fn set_topmost(self, above: bool) -> bool {
        let after = if above { HWND_TOPMOST } else { HWND_NOTOPMOST };
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS;
        // SAFETY: the request is posted to the window's thread and fails
        // without side effects for a window that is gone.
        unsafe { SetWindowPos(self.hwnd(), Some(after), 0, 0, 0, 0, flags) }.is_ok()
    }

    /// Puts the window above the other windows that are not kept above,
    /// without bringing it in front.
    pub fn raise(self) -> bool {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS;
        // SAFETY: as in `set_topmost`.
        unsafe { SetWindowPos(self.hwnd(), Some(HWND_TOP), 0, 0, 0, 0, flags) }.is_ok()
    }

    /// How opaque the window is drawn, from 0 to 1.
    pub fn opacity(self) -> f32 {
        let hwnd = self.hwnd();
        // SAFETY: reading the style of a window has no preconditions.
        let extended = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
        if extended & WS_EX_LAYERED.0 == 0 {
            return 1.;
        }
        let mut alpha = 255u8;
        let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
        // SAFETY: both values outlive the call, which only reads them out.
        let read = unsafe {
            GetLayeredWindowAttributes(hwnd, None, Some(&raw mut alpha), Some(&raw mut flags))
        };
        match read.is_ok() && flags.0 & LWA_ALPHA.0 != 0 {
            true => f32::from(alpha) / 255.,
            false => 1.,
        }
    }

    /// Draws the whole window `opacity` opaque, from 0.1 to 1. The window
    /// is made layered for that, which some programs do not draw well in;
    /// it stops being layered once it is opaque again, unless it was
    /// layered before.
    ///
    /// Windows sends the window messages to change its style, so it is
    /// done from a short thread of its own; a window whose program hangs
    /// is left as it is.
    pub fn set_opacity(self, opacity: f32) {
        let raw = self.raw();
        let opacity = opacity.clamp(LEAST_OPACITY, 1.);
        let started = thread::Builder::new()
            .name("tessera window opacity".to_owned())
            .spawn(move || {
                let id = AppWindowId::from_raw(raw);
                if !id.is_hung() {
                    id.set_opacity_here(opacity);
                }
            });
        if let Err(error) = started {
            report!("a window is not made see-through: {error}");
        }
    }

    /// What [`Self::set_opacity`] does, on the thread it is called on.
    fn set_opacity_here(self, opacity: f32) {
        let hwnd = self.hwnd();
        let mut layered = LAYERED.lock().unwrap_or_else(PoisonError::into_inner);
        let ours = layered.get_or_insert_with(HashSet::new);
        // SAFETY: the calls change the extended style and the opacity of a
        // window and fail without side effects for one that is gone.
        unsafe {
            let extended = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let is_layered = extended as u32 & WS_EX_LAYERED.0 != 0;
            if opacity >= 1. {
                if !is_layered {
                    return;
                }
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
                if ours.remove(&self.raw()) {
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, extended & !(WS_EX_LAYERED.0 as isize));
                }
                return;
            }
            if !is_layered {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, extended | WS_EX_LAYERED.0 as isize);
                ours.insert(self.raw());
            }
            let alpha = (opacity * 255.).round() as u8;
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
        }
    }

    /// Has Windows draw no thin border around the window. Whether Windows
    /// took the request is returned; [`Self::set_border`] with `None`
    /// gives the window its border back.
    pub fn hide_border(self) -> bool {
        self.set_border(Some(DWMWA_COLOR_NONE))
    }

    /// The invisible border the window has while it is neither maximized
    /// nor minimized, as [`margins`] gives it. A maximized window reaches
    /// beyond the display on every side, and a minimized one has no
    /// border to measure, so for them it is the border measured before,
    /// or else the one Windows gives a window of its kind.
    fn margins(self) -> Option<[i32; 4]> {
        let mut remembered = MARGINS.lock().unwrap_or_else(PoisonError::into_inner);
        let remembered = remembered.get_or_insert_with(HashMap::new);
        if self.is_maximized() || self.is_minimized() {
            return remembered
                .get(&self.raw())
                .copied()
                .or_else(|| self.usual_margins());
        }
        let seen = self.frame()?;
        let mut whole = RECT::default();
        // SAFETY: `whole` outlives the call, which tolerates a handle whose
        // window has since been destroyed.
        unsafe { GetWindowRect(self.hwnd(), &raw mut whole) }.ok()?;
        let measured = margins(seen, whole.into());
        if remembered.len() >= MARGINS_KEPT {
            remembered.retain(|&raw, _| AppWindowId::from_raw(raw).exists());
        }
        remembered.insert(self.raw(), measured);
        Some(measured)
    }

    /// The invisible border Windows gives a window with the frame this
    /// one has, at its scale, when it is neither maximized nor minimized:
    /// the frame and its padding beyond the line that is drawn of it, on
    /// every side but the top. A program that draws its own frame may
    /// have another.
    fn usual_margins(self) -> Option<[i32; 4]> {
        let hwnd = self.hwnd();
        // SAFETY: the calls only read the scale and the style of a window,
        // and the sizes Windows gives frames at a scale.
        unsafe {
            let dpi = GetDpiForWindow(hwnd);
            if dpi == 0 {
                return None;
            }
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let (x, y) = if style & WS_THICKFRAME.0 != 0 {
                (SM_CXSIZEFRAME, SM_CYSIZEFRAME)
            } else if style & WS_DLGFRAME.0 != 0 {
                (SM_CXFIXEDFRAME, SM_CYFIXEDFRAME)
            } else {
                return Some([0; 4]);
            };
            let metric = |index: SYSTEM_METRICS_INDEX| GetSystemMetricsForDpi(index, dpi);
            let padding = metric(SM_CXPADDEDBORDER);
            let side = (metric(x) + padding - metric(SM_CXBORDER)).max(0);
            let bottom = (metric(y) + padding - metric(SM_CYBORDER)).max(0);
            Some([side, 0, side, bottom])
        }
    }

    /// What is seen of the window when it is restored, on the screen: for
    /// a maximized or minimized window, where it goes back to.
    pub fn restored_frame(self) -> Option<ScreenRect> {
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: `placement` is sized as the call is told and outlives it;
        // the call only reads window state.
        unsafe { GetWindowPlacement(self.hwnd(), &raw mut placement) }.ok()?;
        let normal = ScreenRect::from(placement.rcNormalPosition);
        let whole = shifted(normal, workspace_offset(normal));
        Some(widened(whole, self.margins()?, true))
    }

    /// Has the window go back to `frame`, what is to be seen of it on the
    /// screen, whenever it is restored, and be shown maximized now with
    /// `maximized` or restored there otherwise. A maximized window goes to
    /// the display that holds most of `frame`.
    ///
    /// Windows sends the window messages for this, so it is done from a
    /// short thread of its own; a window whose program hangs is left as it
    /// is.
    pub fn place_restored(self, frame: ScreenRect, maximized: bool) {
        let Some(margins) = self.margins() else {
            return;
        };
        let whole = widened(frame, margins, false);
        let (x, y) = workspace_offset(whole);
        let normal = shifted(whole, (-x, -y));
        let raw = self.raw();
        let placed = thread::Builder::new()
            .name("tessera window placement".to_owned())
            .spawn(move || {
                let id = AppWindowId::from_raw(raw);
                if id.is_hung() {
                    return;
                }
                let mut placement = WINDOWPLACEMENT {
                    length: size_of::<WINDOWPLACEMENT>() as u32,
                    ..Default::default()
                };
                // SAFETY: `placement` is sized as the calls are told and
                // outlives them; a window that is gone makes them fail.
                unsafe {
                    if GetWindowPlacement(id.hwnd(), &raw mut placement).is_err() {
                        return;
                    }
                    placement.rcNormalPosition = RECT {
                        left: normal.left,
                        top: normal.top,
                        right: normal.right,
                        bottom: normal.bottom,
                    };
                    placement.showCmd = match maximized {
                        true => SW_SHOWMAXIMIZED.0 as u32,
                        false => SW_SHOWNORMAL.0 as u32,
                    };
                    let _ = SetWindowPlacement(id.hwnd(), &raw const placement);
                }
            });
        if let Err(error) = placed {
            report!("a window is not placed: {error}");
        }
    }
}

/// What Tessera took from the style of a window of another program, so
/// that it gives it back: the title bar and frame the window had (only
/// those bits of its style, so that what the window became since, such as
/// maximized, hidden or disabled, is never set back), whether its title
/// bar is away, where the whole window was before it was made to fill its
/// display, and the process it belongs to, so that another window that
/// later gets the same handle is not taken for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Restyled {
    style: isize,
    untitled: bool,
    filled_from: Option<ScreenRect>,
    process: u32,
}

impl Restyled {
    /// Whether the window this was taken from is still the one `raw`
    /// names.
    fn still_of(&self, raw: isize) -> bool {
        let id = AppWindowId::from_raw(raw);
        id.exists() && id.process() == self.process
    }
}

/// The class of the windows of the frame host of the applications of the
/// Universal Windows Platform, and of the window within each that the
/// application itself shows.
const FRAME_CLASS: &str = "ApplicationFrameWindow";
const CONTENT_CLASS: PCWSTR = w!("Windows.UI.Core.CoreWindow");

/// The bits of the style of a window Tessera takes: its title bar and its
/// frame.
const TAKEN: isize = (WS_CAPTION.0 | WS_THICKFRAME.0) as isize;

/// The windows Tessera restyled, by window.
static RESTYLED: Mutex<Option<HashMap<isize, Restyled>>> = Mutex::new(None);

/// The windows kept below the others.
static BELOW: Mutex<Option<HashSet<isize>>> = Mutex::new(None);

/// For how long the styles are being given back when Tessera quits before
/// it stops waiting for programs that do not answer.
const GIVING_BACK: Duration = Duration::from_millis(500);

/// The style a window of `current` style has with the title bar and frame
/// of `original`, without its title bar where `untitled`, and without any
/// frame where `filled`; every other bit stays as it is now.
fn restyled(current: isize, original: isize, untitled: bool, filled: bool) -> isize {
    let mut taken = 0;
    if untitled || filled {
        taken |= WS_CAPTION.0 as isize;
    }
    if filled {
        taken |= WS_THICKFRAME.0 as isize;
    }
    (current & !TAKEN) | (original & TAKEN & !taken)
}

/// The flags that change a window without bringing it in front or waiting
/// for it, with `more`.
fn quietly(more: SET_WINDOW_POS_FLAGS) -> SET_WINDOW_POS_FLAGS {
    SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS | more
}

impl AppWindowId {
    /// Does `work` to the window from a short thread of its own, named
    /// after `what`, unless the program of the window hangs: Windows sends
    /// the window messages for it and waits for the answers.
    fn aside(self, what: &'static str, work: impl FnOnce(AppWindowId) + Send + 'static) {
        let raw = self.raw();
        let started = thread::Builder::new()
            .name(format!("tessera {what}"))
            .spawn(move || {
                let id = AppWindowId::from_raw(raw);
                if !id.is_hung() {
                    work(id);
                }
            });
        if let Err(error) = started {
            report!("the {what} of a window is not changed: {error}");
        }
    }

    /// Puts the window below every other window that is not kept above,
    /// without bringing anything in front. Whether Windows took the
    /// request is returned.
    pub fn lower(self) -> bool {
        let flags = quietly(SWP_NOMOVE | SWP_NOSIZE);
        // SAFETY: the request is posted to the window's thread and fails
        // without side effects for a window that is gone.
        unsafe { SetWindowPos(self.hwnd(), Some(HWND_BOTTOM), 0, 0, 0, 0, flags) }.is_ok()
    }

    /// Keeps the window below the others, or no longer: it is lowered now
    /// and again whenever another window comes in front, which Windows
    /// tells a thread of its own. A window kept above stops being kept so.
    pub fn keep_below(self, below: bool) {
        {
            let mut kept = BELOW.lock().unwrap_or_else(PoisonError::into_inner);
            let kept = kept.get_or_insert_with(HashSet::new);
            if !below {
                kept.remove(&self.raw());
                return;
            }
            kept.insert(self.raw());
        }
        if self.is_topmost() {
            self.set_topmost(false);
        }
        self.lower();
        static WATCHING: Once = Once::new();
        WATCHING.call_once(|| {
            let started = thread::Builder::new()
                .name("tessera keep below".to_owned())
                .spawn(watch_fronts);
            if let Err(error) = started {
                report!("windows kept below come up again: {error}");
            }
        });
    }

    /// Whether Tessera keeps the window below the others.
    pub fn is_kept_below(self) -> bool {
        let kept = BELOW.lock().unwrap_or_else(PoisonError::into_inner);
        kept.as_ref().is_some_and(|kept| kept.contains(&self.raw()))
    }

    /// Whether Tessera took the title bar of the window away.
    pub fn title_bar_taken(self) -> bool {
        self.restyled().is_some_and(|restyled| restyled.untitled)
    }

    /// Whether Tessera has the window fill its display.
    pub fn fills_display(self) -> bool {
        self.restyled()
            .is_some_and(|restyled| restyled.filled_from.is_some())
    }

    fn restyled(self) -> Option<Restyled> {
        let restyled = RESTYLED.lock().unwrap_or_else(PoisonError::into_inner);
        let restyled = restyled.as_ref()?.get(&self.raw()).copied()?;
        restyled.still_of(self.raw()).then_some(restyled)
    }

    /// Takes the title bar of the window away, keeping the border that
    /// resizes it, or gives it back as it was. A program that draws its
    /// own title bar goes on drawing it.
    pub fn show_title_bar(self, shown: bool) {
        self.aside("title bar", move |id| {
            id.restyle_here(|restyled| restyled.untitled = !shown, None);
        });
    }

    /// Has the window cover `display`, all of a display on the screen,
    /// without title bar or frame and in front of the windows that are not
    /// kept above; with `None` it goes back to its frame and its place
    /// from before. A maximized window is restored first.
    pub fn fill_display(self, display: Option<ScreenRect>) {
        self.aside("full screen", move |id| match display {
            Some(display) => {
                if id.is_maximized() {
                    // SAFETY: the window is not hung, which was asked, and
                    // the call fails for one that is gone.
                    unsafe {
                        let _ = ShowWindow(id.hwnd(), SW_SHOWNOACTIVATE);
                    }
                }
                let mut whole = RECT::default();
                // SAFETY: `whole` outlives the call.
                let read = unsafe { GetWindowRect(id.hwnd(), &raw mut whole) }.is_ok();
                let before = read.then(|| ScreenRect::from(whole));
                let fill = |restyled: &mut Restyled| {
                    restyled.filled_from = restyled.filled_from.or(before);
                };
                id.restyle_here(fill, Some(display));
            }
            None => {
                let before = id.restyled().and_then(|restyled| restyled.filled_from);
                if before.is_some() {
                    id.restyle_here(|restyled| restyled.filled_from = None, before);
                }
            }
        });
    }

    /// Changes what Tessera takes from the style of the window by
    /// `change`, sets the style that follows and has the window drawn with
    /// it, with all of the window at `place` where it is given, on the
    /// calling thread. A window that has all of its style back is
    /// forgotten.
    fn restyle_here(self, change: impl FnOnce(&mut Restyled), place: Option<ScreenRect>) {
        let hwnd = self.hwnd();
        // SAFETY: reading the style of a window has no preconditions.
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
        let (original, after) = {
            let mut all = RESTYLED.lock().unwrap_or_else(PoisonError::into_inner);
            let all = all.get_or_insert_with(HashMap::new);
            all.retain(|&raw, restyled| restyled.still_of(raw));
            let entry = all.entry(self.raw()).or_insert(Restyled {
                style: style & TAKEN,
                untitled: false,
                filled_from: None,
                process: self.process(),
            });
            change(entry);
            let after = *entry;
            if !after.untitled && after.filled_from.is_none() {
                all.remove(&self.raw());
            }
            (after.style, after)
        };
        let filled = after.filled_from.is_some();
        let wanted = restyled(style, original, after.untitled, filled);
        let (after_window, order) = match filled {
            true => (Some(HWND_TOP), SET_WINDOW_POS_FLAGS(0)),
            false => (None, SWP_NOZORDER),
        };
        // SAFETY: the window is not hung, which was asked; the calls
        // change its style and place, and fail for a window that is gone.
        unsafe {
            if wanted != style {
                SetWindowLongPtrW(hwnd, GWL_STYLE, wanted);
            }
            let _ = match place {
                Some(rect) => SetWindowPos(
                    hwnd,
                    after_window,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    quietly(SWP_FRAMECHANGED | order),
                ),
                None => SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    quietly(SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER),
                ),
            };
        }
    }

    /// Gives every window Tessera restyled its style and its place back,
    /// and keeps no window below any longer, waiting a moment at most for
    /// the programs, as when Tessera quits.
    pub fn give_styles_back() {
        if let Some(kept) = BELOW
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            kept.clear();
        }
        let restyled: Vec<(isize, Option<ScreenRect>)> = {
            let mut all = RESTYLED.lock().unwrap_or_else(PoisonError::into_inner);
            let all = all.get_or_insert_with(HashMap::new);
            all.retain(|&raw, restyled| restyled.still_of(raw));
            { all.iter() }
                .map(|(&raw, taken)| (raw, taken.filled_from))
                .collect()
        };
        if restyled.is_empty() {
            return;
        }
        let (done, given) = mpsc::channel();
        let started = thread::Builder::new()
            .name("tessera styles back".to_owned())
            .spawn(move || {
                for (raw, before) in restyled {
                    let id = AppWindowId::from_raw(raw);
                    if !id.is_hung() && id.restyled().is_some() {
                        let back = |restyled: &mut Restyled| {
                            restyled.untitled = false;
                            restyled.filled_from = None;
                        };
                        id.restyle_here(back, before);
                    }
                }
                done.send(()).ok();
            });
        if started.is_ok() {
            let _ = given.recv_timeout(GIVING_BACK);
        }
    }

    /// Whether `point` is on the title bar of the window, away from its
    /// buttons, as the window itself says; `None` when it does not answer
    /// in time. The window waits for its program, so this is asked away
    /// from the thread of the user interface.
    pub fn on_title_bar(self, point: (i32, i32)) -> Option<bool> {
        super::snap::hit_test(self, point).map(|hit| hit == HTCAPTION)
    }

    /// Has Windows move the window with the arrow keys, as Move of its
    /// system menu does, until Enter or Esc.
    pub fn move_with_keys(self) {
        self.system_command(SC_MOVE);
    }

    /// Has Windows resize the window with the arrow keys, as Size of its
    /// system menu does, until Enter or Esc.
    pub fn resize_with_keys(self) {
        self.system_command(SC_SIZE);
    }

    /// The process that shows what is in the window, which is ended to
    /// end its program: the window's own, except for a window of the frame
    /// host of the applications of the Universal Windows Platform, which
    /// shows an application that runs in a process of its own, in a window
    /// within. `None` when that window is not found, as while the
    /// application is suspended or closing, and for a window that is gone.
    /// Finding it asks no window anything.
    pub fn content_process(self) -> Option<u32> {
        let own = self.process();
        if own == 0 {
            return None;
        }
        if self.class() != FRAME_CLASS {
            return Some(own);
        }
        let mut child = None;
        loop {
            // SAFETY: the call only reads the windows within the window,
            // by their class, and fails for a window that is gone.
            let next = unsafe { FindWindowExW(Some(self.hwnd()), child, CONTENT_CLASS, None) };
            let next = match next {
                Ok(next) if !next.is_invalid() => next,
                _ => return None,
            };
            let process = AppWindowId::from_raw(next.0 as isize).process();
            if process != 0 && process != own {
                return Some(process);
            }
            child = Some(next);
        }
    }

    fn system_command(self, command: u32) {
        // SAFETY: posting to a window that has since been destroyed fails
        // without side effects.
        unsafe {
            let _ = PostMessageW(
                Some(self.hwnd()),
                WM_SYSCOMMAND,
                WPARAM(command as usize),
                LPARAM(0),
            );
        }
    }
}

/// Lowers the windows kept below whenever another window comes in front,
/// for as long as the process runs.
fn watch_fronts() {
    // SAFETY: `came_in_front` is a function of this program, which is
    // never unloaded; no module is injected anywhere.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(came_in_front),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    if hook.is_invalid() {
        report!("Windows does not report windows coming in front, so windows kept below come up");
        return;
    }
    let mut message = MSG::default();
    // SAFETY: `message` outlives every call; the loop only hands the
    // messages of this thread on to where they belong.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

unsafe extern "system" fn came_in_front(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let mut kept = BELOW.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(kept) = kept.as_mut() else {
        return;
    };
    kept.retain(|&raw| AppWindowId::from_raw(raw).exists());
    for &raw in kept.iter() {
        AppWindowId::from_raw(raw).lower();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_bar_and_a_frame_are_taken_and_given_back() {
        let other = 0x0001_0000_isize;
        let style = TAKEN | other;
        let original = style & TAKEN;
        let untitled = restyled(style, original, true, false);
        assert_eq!(untitled & WS_CAPTION.0 as isize, 0);
        assert_ne!(untitled & WS_THICKFRAME.0 as isize, 0);
        assert_ne!(untitled & other, 0);
        let filled = restyled(style, original, false, true);
        assert_eq!(filled & TAKEN, 0);
        assert_ne!(filled & other, 0);
        assert_eq!(restyled(style, original, false, false), style);
    }

    #[test]
    fn giving_a_style_back_keeps_what_the_window_became_since() {
        let maximized = 0x0100_0000_isize;
        let hidden_before = TAKEN;
        let now = maximized | WS_THICKFRAME.0 as isize;
        let back = restyled(now, hidden_before & TAKEN, false, false);
        assert_eq!(back, maximized | TAKEN);
    }

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn the_invisible_border_is_taken_off_and_put_back() {
        let whole = rect(93, 100, 1007, 707);
        let seen = rect(100, 100, 1000, 700);
        let margins = margins(seen, whole);
        assert_eq!(margins, [7, 0, 7, 7]);
        assert_eq!(widened(seen, margins, false), whole);
        assert_eq!(widened(whole, margins, true), seen);
    }

    #[test]
    fn a_rect_moves_by_an_offset() {
        let moved = shifted(rect(0, 0, 10, 20), (5, -3));
        assert_eq!(moved, rect(5, -3, 15, 17));
        assert_eq!(shifted(moved, (-5, 3)), rect(0, 0, 10, 20));
    }
}
