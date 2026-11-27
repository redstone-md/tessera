// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The edge bars of applications: windows that reserve a strip at an edge
//! of a display with `SHAppBarMessage`, so that maximized windows keep
//! clear of them. Tessera's own surfaces are such bars.
//!
//! `SHAppBarMessage` sends its request to the window of the class
//! `Shell_TrayWnd`, which is the tray window. Beside Explorer that window
//! passes the request on, and Explorer keeps the bars. Without Explorer
//! nobody would, so the tray window answers itself with the server here:
//! it keeps the bars in the order they registered, fits each one against
//! those registered before it, sets the room each display leaves to
//! windows (its work area) and tells the bars when they have to look at
//! their place again.
//!
//! Everything here runs on the tray thread. The server never sends a
//! message to a bar and waits: what bars are told is posted, as one of
//! them is a window of GPUI, whose thread may be the one waiting for the
//! answer. Tessera's own surfaces are not posted to at all, as GPUI does
//! not hand them the messages of bars: a number grows instead that they
//! look at, and they take their place again when it changed. The work areas are set after the answer was given, from a
//! message the tray window posts to itself, as setting them tells every
//! window and waits for them.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::UI::Shell::{
    ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_ACTIVATE, ABM_GETAUTOHIDEBAR,
    ABM_GETAUTOHIDEBAREX, ABM_GETSTATE, ABM_GETTASKBARPOS, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE,
    ABM_SETAUTOHIDEBAR, ABM_SETAUTOHIDEBAREX, ABM_SETPOS, ABM_SETSTATE, ABM_WINDOWPOSCHANGED,
    ABN_FULLSCREENAPP, ABN_POSCHANGED, ABS_ALWAYSONTOP, SHLockShared, SHUnlockShared,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowThreadProcessId, HWND_TOPMOST, IsWindow, MONITORINFOF_PRIMARY, PostMessageW,
    SPI_SETWORKAREA, SPIF_SENDCHANGE, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetWindowPos, SystemParametersInfoW, WM_APP,
};
use windows::core::BOOL;

use super::{NativeSurface, ScreenRect};

/// Where the request of an edge bar names the shared memory that holds its
/// `APPBARDATA`: the handle, then the process the handle is valid in.
pub(super) const SHARED_HANDLE: usize = 48;
pub(super) const SHARED_OWNER: usize = 56;

/// Where the request says what is asked, and the length up to it: a
/// shorter request asks nothing.
const MESSAGE: usize = 40;
const MESSAGE_END: usize = MESSAGE + 4;

/// The length of the `APPBARDATA` the shared memory holds, with its window
/// 32 bits wide; the memory is never written beyond it.
const SHARED_LENGTH: usize = 40;

/// Where that structure holds the edge and the rectangle.
const SHARED_EDGE: usize = 12;
const SHARED_RECT: usize = 16;

/// The message the tray window posts to itself to set the work areas after
/// it answered.
pub(super) const WORK_AREAS: u32 = WM_APP + 0x7e;

/// A bar deeper than this share of its display does not take room from
/// windows: it is one that sent its whole display as its place.
const DEEPEST_SHARE: i64 = 2;

/// A display, as the server arranges bars on it, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Monitor {
    frame: ScreenRect,
    /// The room it leaves to windows now.
    work: ScreenRect,
    primary: bool,
}

impl Monitor {
    /// Every display there is, as Windows lists them.
    fn all() -> Vec<Self> {
        unsafe extern "system" fn add(
            monitor: HMONITOR,
            _: HDC,
            _: *mut RECT,
            list: LPARAM,
        ) -> BOOL {
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: `list` is the pointer `all` passed, to a list that
            // outlives the enumeration, and `info` is correctly sized.
            unsafe {
                let list = &mut *(list.0 as *mut Vec<Monitor>);
                if GetMonitorInfoW(monitor, &raw mut info).as_bool() {
                    list.push(Monitor {
                        frame: info.rcMonitor.into(),
                        work: info.rcWork.into(),
                        primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
                    });
                }
            }
            true.into()
        }
        let mut list: Vec<Self> = Vec::new();
        // SAFETY: the enumeration is synchronous and `list` outlives it.
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&raw mut list as isize));
        }
        list
    }
}

/// What an edge bar asked, as `SHAppBarMessage` sends it: an `APPBARDATA`
/// whose window is 32 bits wide and whose `lParam` is 64, what is asked,
/// and the shared memory the answer goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Request {
    /// The length the `APPBARDATA` states for itself.
    pub size: u32,
    window: u32,
    callback: u32,
    edge: u32,
    rect: ScreenRect,
    lparam: i64,
    message: u32,
    /// The handle of the shared memory, or 0 where no answer is written.
    pub shared: u64,
    /// The process that handle is valid in.
    pub owner: u32,
}

impl Request {
    /// Reads a request; what is missing of a short one reads as zero.
    pub fn parse(data: &[u8]) -> Self {
        let number = |at: usize| {
            let bytes = data.get(at..at + 4).and_then(|bytes| bytes.try_into().ok());
            bytes.map_or(0, u32::from_le_bytes)
        };
        let wide = |at: usize| {
            let bytes = data.get(at..at + 8).and_then(|bytes| bytes.try_into().ok());
            bytes.map_or(0, u64::from_le_bytes)
        };
        Self {
            size: number(0),
            window: number(4),
            callback: number(8),
            edge: number(12),
            rect: ScreenRect {
                left: number(16) as i32,
                top: number(20) as i32,
                right: number(24) as i32,
                bottom: number(28) as i32,
            },
            lparam: wide(32) as i64,
            message: number(MESSAGE),
            shared: wide(SHARED_HANDLE),
            owner: number(SHARED_OWNER),
        }
    }
}

/// Writes `edge` and `rect` into the `APPBARDATA` of the shared memory, as
/// far as `block` holds it.
fn write_place(block: &mut [u8], edge: u32, rect: ScreenRect) {
    if let Some(field) = block.get_mut(SHARED_EDGE..SHARED_EDGE + 4) {
        field.copy_from_slice(&edge.to_le_bytes());
    }
    let sides = [rect.left, rect.top, rect.right, rect.bottom];
    for (index, side) in sides.into_iter().enumerate() {
        let at = SHARED_RECT + index * 4;
        if let Some(field) = block.get_mut(at..at + 4) {
            field.copy_from_slice(&side.to_le_bytes());
        }
    }
}

/// A message a bar is to be told, posted to its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Note {
    window: u32,
    callback: u32,
    /// One of the `ABN_` codes.
    code: u32,
    lparam: isize,
    /// Whether the bar is one of Tessera's surfaces, which is told by
    /// [`places`] instead.
    ours: bool,
}

/// The answer to a request and what follows from it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Answer {
    result: isize,
    /// The edge and the rectangle written back to the bar.
    place: Option<(u32, ScreenRect)>,
    notes: Vec<Note>,
    /// Whether the room the displays leave to windows may have changed.
    rearranged: bool,
    /// A bar that hides, to be put above the other bars as it became
    /// active or moved.
    raised: Option<u32>,
}

/// A registered edge bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bar {
    window: u32,
    callback: u32,
    /// Whether the bar is one of Tessera's surfaces.
    ours: bool,
    /// Its edge and rectangle, once it set them.
    place: Option<(u32, ScreenRect)>,
}

/// A bar that hides at an edge of a display, of which each edge has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hider {
    window: u32,
    callback: u32,
    edge: u32,
    /// The frame of its display.
    display: ScreenRect,
    ours: bool,
}

/// The edge bars and what they were told.
#[derive(Debug, Default)]
pub(super) struct AppBarServer {
    /// In the order they registered.
    bars: Vec<Bar>,
    hiders: Vec<Hider>,
    /// The frames of the displays an application fills, as bars were told.
    filled: Vec<ScreenRect>,
}

/// `to - from`, which cannot overflow: the rectangles of bars come from
/// other programs, which may send any numbers.
fn span(from: i32, to: i32) -> i64 {
    i64::from(to) - i64::from(from)
}

/// The display that holds most of `rect`, or the one nearest to it.
fn display_of(rect: ScreenRect, monitors: &[Monitor]) -> Option<usize> {
    let overlap = |frame: ScreenRect| {
        let width = span(rect.left.max(frame.left), rect.right.min(frame.right));
        let height = span(rect.top.max(frame.top), rect.bottom.min(frame.bottom));
        width.max(0) * height.max(0)
    };
    let distance = |frame: ScreenRect| {
        let (x, y) = (
            i64::from(rect.left) + i64::from(rect.right),
            i64::from(rect.top) + i64::from(rect.bottom),
        );
        let (cx, cy) = (
            i64::from(frame.left) + i64::from(frame.right),
            i64::from(frame.top) + i64::from(frame.bottom),
        );
        let (dx, dy) = (x - cx, y - cy);
        dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
    };
    let best = monitors
        .iter()
        .enumerate()
        .max_by_key(|(_, monitor)| overlap(monitor.frame))?;
    if overlap(best.1.frame) > 0 {
        return Some(best.0);
    }
    monitors
        .iter()
        .enumerate()
        .min_by_key(|(_, monitor)| distance(monitor.frame))
        .map(|(index, _)| index)
}

/// Takes from `area` what the bar at `edge` that lies at `bar` covers.
fn clip(area: &mut ScreenRect, edge: u32, bar: ScreenRect) {
    match edge {
        ABE_LEFT => area.left = area.left.max(bar.right),
        ABE_TOP => area.top = area.top.max(bar.bottom),
        ABE_RIGHT => area.right = area.right.min(bar.left),
        ABE_BOTTOM => area.bottom = area.bottom.min(bar.top),
        _ => {}
    }
}

/// Moves `rect`, which is to lie at `edge`, inward off the bar at the same
/// edge that lies at `bar`, keeping its depth.
fn shift(rect: &mut ScreenRect, edge: u32, bar: ScreenRect) {
    let (horizontal, vertical) = match edge {
        ABE_LEFT => (span(rect.left, bar.right).max(0), 0),
        ABE_TOP => (0, span(rect.top, bar.bottom).max(0)),
        ABE_RIGHT => (-span(bar.left, rect.right).max(0), 0),
        ABE_BOTTOM => (0, -span(bar.top, rect.bottom).max(0)),
        _ => (0, 0),
    };
    let moved = |side: i32, by: i64| {
        (i64::from(side) + by).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    };
    rect.left = moved(rect.left, horizontal);
    rect.right = moved(rect.right, horizontal);
    rect.top = moved(rect.top, vertical);
    rect.bottom = moved(rect.bottom, vertical);
}

/// Whether a bar at `edge` that lies at `rect` is no deeper than a share of
/// `frame`, its display, that a bar can take.
fn plausible(edge: u32, rect: ScreenRect, frame: ScreenRect) -> bool {
    match edge {
        ABE_LEFT | ABE_RIGHT => {
            span(rect.left, rect.right) * DEEPEST_SHARE <= span(frame.left, frame.right)
        }
        _ => span(rect.top, rect.bottom) * DEEPEST_SHARE <= span(frame.top, frame.bottom),
    }
}

impl AppBarServer {
    /// Answers `request`, which the window of one of Tessera's surfaces
    /// sent if `ours`, with `monitors` the displays there are.
    pub fn answer(&mut self, request: &Request, ours: bool, monitors: &[Monitor]) -> Answer {
        let window = request.window;
        let known = self.bars.iter().any(|bar| bar.window == window);
        let mut answer = Answer::default();
        match request.message {
            ABM_NEW if !known && window != 0 => {
                self.bars.push(Bar {
                    window,
                    callback: request.callback,
                    ours,
                    place: None,
                });
                answer.result = 1;
            }
            ABM_REMOVE => {
                let placed = self
                    .bars
                    .iter()
                    .any(|bar| bar.window == window && bar.place.is_some());
                self.bars.retain(|bar| bar.window != window);
                self.hiders.retain(|hider| hider.window != window);
                if placed {
                    answer.notes = self.moved_except(window);
                    answer.rearranged = true;
                }
                answer.result = 1;
            }
            ABM_QUERYPOS if known => {
                let rect = self.fit(window, request.edge, request.rect, monitors);
                answer.place = Some((request.edge, rect));
                answer.result = 1;
            }
            ABM_SETPOS if known => {
                let place = (
                    request.edge,
                    self.fit(window, request.edge, request.rect, monitors),
                );
                if let Some(bar) = self.bars.iter_mut().find(|bar| bar.window == window)
                    && bar.place != Some(place)
                {
                    bar.place = Some(place);
                    answer.notes = self.moved_except(window);
                    answer.rearranged = true;
                }
                answer.place = Some(place);
                answer.result = 1;
            }
            // Tessera's surfaces that hide do so without registering as
            // bars that hide, so the taskbar is never said to hide.
            ABM_GETSTATE => answer.result = ABS_ALWAYSONTOP as isize,
            ABM_GETTASKBARPOS => {
                answer.place = self.taskbar(monitors);
                answer.result = answer.place.is_some().into();
            }
            ABM_GETAUTOHIDEBAR | ABM_GETAUTOHIDEBAREX => {
                let display = self.display_asked(request, monitors);
                let hider = self
                    .hiders
                    .iter()
                    .find(|hider| hider.edge == request.edge && Some(hider.display) == display);
                answer.result = hider.map_or(0, |hider| hider.window as isize);
            }
            ABM_SETAUTOHIDEBAR | ABM_SETAUTOHIDEBAREX => {
                let display = self.display_asked(request, monitors);
                answer.result = display
                    .is_some_and(|display| {
                        let hider = Hider {
                            window,
                            callback: request.callback,
                            edge: request.edge,
                            display,
                            ours,
                        };
                        self.set_hider(hider, request.lparam != 0)
                    })
                    .into();
                if answer.result != 0
                    && self
                        .bars
                        .iter()
                        .any(|bar| bar.window == window && bar.place.is_some())
                {
                    answer.notes = self.moved_except(window);
                    answer.rearranged = true;
                }
            }
            // A bar that hides stays above the other bars when it becomes
            // active or moves.
            ABM_ACTIVATE | ABM_WINDOWPOSCHANGED => {
                answer.raised = self.hides(window).then_some(window);
                answer.result = 1;
            }
            // The taskbar of Tessera hides as its layout says, so a request
            // to have it hide or stay changes nothing.
            ABM_SETSTATE => answer.result = 1,
            _ => {}
        }
        answer
    }

    /// The display `request` is about: the one its rectangle lies on for
    /// the requests that name one, the primary display for the others.
    fn display_asked(&self, request: &Request, monitors: &[Monitor]) -> Option<ScreenRect> {
        let index = match request.message {
            ABM_GETAUTOHIDEBAREX | ABM_SETAUTOHIDEBAREX => display_of(request.rect, monitors),
            _ => monitors.iter().position(|monitor| monitor.primary),
        };
        Some(monitors.get(index?)?.frame)
    }

    /// Registers `hider` as the bar that hides at its edge of its display,
    /// or ends its registration; whether that was done. Each edge has one
    /// such bar.
    fn set_hider(&mut self, hider: Hider, register: bool) -> bool {
        let same = |other: &Hider| other.edge == hider.edge && other.display == hider.display;
        let holder = self.hiders.iter().position(same);
        match (register, holder) {
            (true, None) => {
                self.hiders.push(hider);
                true
            }
            (true, Some(index)) => self.hiders[index].window == hider.window,
            (false, Some(index)) if self.hiders[index].window == hider.window => {
                self.hiders.remove(index);
                true
            }
            (false, _) => false,
        }
    }

    fn hides(&self, window: u32) -> bool {
        self.hiders.iter().any(|hider| hider.window == window)
    }

    /// Where the bar `window` that is to lie at `edge` can, when it asked
    /// for `proposed`: off every bar that registered before it on the same
    /// display. One at the same edge moves it inward, one at another edge
    /// cuts it short.
    fn fit(
        &self,
        window: u32,
        edge: u32,
        proposed: ScreenRect,
        monitors: &[Monitor],
    ) -> ScreenRect {
        let display = display_of(proposed, monitors);
        let before = self
            .bars
            .iter()
            .take_while(|bar| bar.window != window)
            .filter(|bar| !self.hides(bar.window));
        let mut rect = proposed;
        for bar in before {
            let Some((their_edge, their_rect)) = bar.place else {
                continue;
            };
            if display_of(their_rect, monitors) != display {
                continue;
            }
            if their_edge == edge {
                shift(&mut rect, edge, their_rect);
            } else {
                clip(&mut rect, their_edge, their_rect);
            }
        }
        rect
    }

    /// The room each of `monitors` leaves to windows once the bars that do
    /// not hide took theirs.
    pub fn work_areas(&self, monitors: &[Monitor]) -> Vec<ScreenRect> {
        monitors
            .iter()
            .enumerate()
            .map(|(index, monitor)| {
                let mut area = monitor.frame;
                let docked = self.bars.iter().filter(|bar| !self.hides(bar.window));
                for (edge, rect) in docked.filter_map(|bar| bar.place) {
                    if display_of(rect, monitors) == Some(index)
                        && plausible(edge, rect, monitor.frame)
                    {
                        clip(&mut area, edge, rect);
                    }
                }
                area
            })
            .collect()
    }

    /// The taskbar, as applications that ask for it mean it: the first
    /// placed surface of Tessera on the primary display, or on any.
    fn taskbar(&self, monitors: &[Monitor]) -> Option<(u32, ScreenRect)> {
        let primary = monitors.iter().position(|monitor| monitor.primary);
        let ours = || {
            self.bars
                .iter()
                .filter(|bar| bar.ours)
                .filter_map(|bar| bar.place)
        };
        ours()
            .find(|(_, rect)| primary.is_some() && display_of(*rect, monitors) == primary)
            .or_else(|| ours().next())
    }

    /// What tells every bar but `window` that the bars moved.
    fn moved_except(&self, window: u32) -> Vec<Note> {
        self.tell_all(ABN_POSCHANGED, 0, |other| other != window)
    }

    /// What tells each bar and hider whose window `to` accepts `code`.
    fn tell_all(&self, code: u32, lparam: isize, to: impl Fn(u32) -> bool) -> Vec<Note> {
        let bars = self
            .bars
            .iter()
            .map(|bar| (bar.window, bar.callback, bar.ours));
        let hiders = self
            .hiders
            .iter()
            .map(|hider| (hider.window, hider.callback, hider.ours));
        let mut notes: Vec<Note> = Vec::new();
        for (window, callback, ours) in bars.chain(hiders) {
            let new = !notes.iter().any(|note| note.window == window);
            if (callback != 0 || ours) && new && to(window) {
                notes.push(Note {
                    window,
                    callback,
                    code,
                    lparam,
                    ours,
                });
            }
        }
        notes
    }

    /// Forgets the bars whose window `exists` denies.
    pub fn prune(&mut self, exists: impl Fn(u32) -> bool) -> Answer {
        let placed = self
            .bars
            .iter()
            .any(|bar| bar.place.is_some() && !exists(bar.window));
        let before = (self.bars.len(), self.hiders.len());
        self.bars.retain(|bar| exists(bar.window));
        self.hiders.retain(|hider| exists(hider.window));
        let mut answer = Answer::default();
        if placed {
            answer.notes = self.tell_all(ABN_POSCHANGED, 0, |_| true);
        }
        answer.rearranged = placed || before.1 != self.hiders.len();
        answer
    }

    /// The displays were arranged anew: every bar is to look at its place
    /// again, and what was known of applications filling them is stale.
    pub fn rearranged(&mut self) -> Vec<Note> {
        self.filled.clear();
        self.tell_all(ABN_POSCHANGED, 0, |_| true)
    }

    /// Notes that an application fills the display `frame`, or stopped
    /// filling it, and what tells the bars on it if that is news.
    pub fn fullscreen(
        &mut self,
        frame: ScreenRect,
        filled: bool,
        monitors: &[Monitor],
    ) -> Vec<Note> {
        if self.filled.contains(&frame) == filled {
            return Vec::new();
        }
        if filled {
            self.filled.push(frame);
        } else {
            self.filled.retain(|other| *other != frame);
        }
        let on_display: Vec<u32> = self
            .bars
            .iter()
            .filter(|bar| {
                bar.place.is_some_and(|(_, rect)| {
                    let index = display_of(rect, monitors);
                    index
                        .and_then(|index| monitors.get(index))
                        .map(|monitor| monitor.frame)
                        == Some(frame)
                })
            })
            .map(|bar| bar.window)
            .chain(
                self.hiders
                    .iter()
                    .filter(|hider| hider.display == frame)
                    .map(|hider| hider.window),
            )
            .collect();
        self.tell_all(ABN_FULLSCREENAPP, filled.into(), |window| {
            on_display.contains(&window)
        })
    }

    /// Whether a bar of another program than Tessera is registered.
    fn has_others(&self) -> bool {
        self.bars.iter().any(|bar| !bar.ours) || self.hiders.iter().any(|hider| !hider.ours)
    }

    /// The displays with a bar of another program on them, each with the
    /// window of one such bar.
    fn watched(&self, monitors: &[Monitor]) -> Vec<(ScreenRect, u32)> {
        let bars = self.bars.iter().filter(|bar| !bar.ours).filter_map(|bar| {
            let (_, rect) = bar.place?;
            Some((monitors.get(display_of(rect, monitors)?)?.frame, bar.window))
        });
        let hiders = self
            .hiders
            .iter()
            .filter(|hider| !hider.ours)
            .map(|hider| (hider.display, hider.window));
        let mut watched: Vec<(ScreenRect, u32)> = Vec::new();
        for (frame, window) in bars.chain(hiders) {
            if !watched.iter().any(|(other, _)| *other == frame) {
                watched.push((frame, window));
            }
        }
        watched
    }
}

/// The work areas to set, in the order to set them: those of `monitors`
/// that `areas` changes, the primary display last. An area with no room
/// left is not set.
fn changes(monitors: &[Monitor], areas: &[ScreenRect]) -> Vec<ScreenRect> {
    let mut changed: Vec<(bool, ScreenRect)> = monitors
        .iter()
        .zip(areas)
        .filter(|(monitor, area)| {
            monitor.work != **area && area.left < area.right && area.top < area.bottom
        })
        .map(|(monitor, area)| (monitor.primary, *area))
        .collect();
    changed.sort_by_key(|(primary, _)| *primary);
    changed.into_iter().map(|(_, area)| area).collect()
}

/// Grows whenever Tessera's surfaces are to take their place at their edge
/// again: another bar moved, or the bars have another server now.
static PLACES: AtomicU64 = AtomicU64::new(0);

/// The number that grows whenever Tessera's surfaces are to take their
/// place again.
pub(super) fn places() -> u64 {
    PLACES.load(Ordering::Relaxed)
}

/// Has Tessera's surfaces take their place again.
fn look_again() {
    PLACES.fetch_add(1, Ordering::Relaxed);
}

thread_local! {
    static SERVER: RefCell<AppBarServer> = RefCell::default();
    /// Whether the tray window has posted itself the message to set the
    /// work areas and not yet received it.
    static PENDING: Cell<bool> = const { Cell::new(false) };
}

/// The handle a 32-bit value of a request stands for.
fn handle(value: u32) -> HWND {
    HWND(value as usize as *mut c_void)
}

/// Posts each note to its bar. A bar whose window is gone, or that runs
/// with higher privileges than the shell, does not receive it. Tessera's
/// surfaces see for themselves when an application fills their display,
/// and only hear that they are to look at their place again.
fn post(notes: &[Note]) {
    for note in notes {
        if note.ours {
            if note.code == ABN_POSCHANGED {
                look_again();
            }
            continue;
        }
        // SAFETY: posting passes plain numbers and fails harmlessly for a
        // window that is gone.
        unsafe {
            let _ = PostMessageW(
                Some(handle(note.window)),
                note.callback,
                WPARAM(note.code as usize),
                LPARAM(note.lparam),
            );
        }
    }
}

/// Has the tray window `window` set the work areas once it is done with
/// the message it is on.
fn rearrange_later(window: HWND) {
    if PENDING.replace(true) {
        return;
    }
    // SAFETY: the window belongs to this thread; posting passes no pointers.
    if unsafe { PostMessageW(Some(window), WORK_AREAS, WPARAM(0), LPARAM(0)) }.is_err() {
        work_areas_due(true);
    }
}

/// What follows from an answer: its notes are posted, a bar that hides is
/// put above the others, and the work areas are set later if they may have
/// changed.
fn follow(window: HWND, answer: &Answer) {
    post(&answer.notes);
    if let Some(bar) = answer.raised {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS;
        // SAFETY: the call passes no pointers, fails harmlessly for a
        // window that is gone, and only posts the change to the thread of
        // the bar, which may be the one waiting for this answer.
        unsafe {
            let _ = SetWindowPos(handle(bar), Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
        }
    }
    if answer.rearranged {
        rearrange_later(window);
    }
}

/// Answers the request of an edge bar in `data`, which arrived at the tray
/// window `window` while it serves the bars itself.
pub(super) fn serve(window: HWND, data: &[u8]) -> LRESULT {
    if data.len() < MESSAGE_END {
        return LRESULT(0);
    }
    let request = Request::parse(data);
    let mut process = 0;
    // SAFETY: `process` outlives the call, which tolerates a window that is
    // gone.
    unsafe { GetWindowThreadProcessId(handle(request.window), Some(&raw mut process)) };
    let ours = process != 0 && process == std::process::id();
    let monitors = Monitor::all();
    let answer = SERVER.with_borrow_mut(|server| server.answer(&request, ours, &monitors));
    if let Some((edge, rect)) = answer.place
        && request.shared != 0
    {
        let size = (request.size as usize).min(SHARED_LENGTH);
        // SAFETY: the shared memory functions accept any handle and answer
        // null for one that is not theirs. A block they hand out holds the
        // structure its sender put there, of the length that structure
        // states, and stays valid until it is unlocked.
        unsafe {
            let shared = SHLockShared(
                HANDLE(request.shared as usize as *mut c_void),
                request.owner,
            );
            if !shared.is_null() {
                write_place(
                    std::slice::from_raw_parts_mut(shared.cast::<u8>(), size),
                    edge,
                    rect,
                );
                let _ = SHUnlockShared(shared);
            }
        }
    }
    follow(window, &answer);
    LRESULT(answer.result)
}

/// The message the tray window posted itself arrived: the work areas the
/// bars leave are set if the window still serves the bars, as `serving`
/// says. Either way a later change posts the message again.
pub(super) fn work_areas_due(serving: bool) {
    PENDING.set(false);
    if serving {
        set_work_areas();
    }
}

/// Sets the work areas the bars leave.
fn set_work_areas() {
    let monitors = Monitor::all();
    let areas = SERVER.with_borrow(|server| server.work_areas(&monitors));
    let changes = changes(&monitors, &areas);
    let count = changes.len();
    for (index, area) in changes.into_iter().enumerate() {
        // Windows are told once, after the last display.
        let flags = if index + 1 == count {
            SPIF_SENDCHANGE
        } else {
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)
        };
        let mut rect = RECT {
            left: area.left,
            top: area.top,
            right: area.right,
            bottom: area.bottom,
        };
        // SAFETY: `rect` outlives the call, which reads it. Telling the
        // windows sends them a message and waits, so no borrow of the
        // server is held here: a bar may ask again meanwhile.
        let set = unsafe {
            SystemParametersInfoW(SPI_SETWORKAREA, 0, Some((&raw mut rect).cast()), flags)
        };
        if let Err(error) = set {
            report!("the room a display leaves to windows was not set: {error}");
        }
    }
}

/// Forgets the bars whose windows are gone and tells the bars on a display
/// when an application starts or stops filling it, as the tray window
/// does every second while it serves the bars.
pub(super) fn upkeep(window: HWND) {
    // SAFETY: the call only reads whether a window exists.
    let exists = |bar: u32| unsafe { IsWindow(Some(handle(bar))) }.as_bool();
    let answer = SERVER.with_borrow_mut(|server| server.prune(exists));
    follow(window, &answer);

    // Tessera's surfaces see for themselves when an application fills
    // their display; bars of other programs are told. The windows are read
    // without the list the surfaces share: the GPUI thread may wait for it
    // while this thread would wait for one of its windows.
    if SERVER.with_borrow(AppBarServer::has_others) {
        let monitors = Monitor::all();
        let watched = SERVER.with_borrow(|server| server.watched(&monitors));
        for (frame, bar) in watched {
            let filled = NativeSurface { hwnd: handle(bar) }.display_is_filled_apart();
            let notes =
                SERVER.with_borrow_mut(|server| server.fullscreen(frame, filled, &monitors));
            post(&notes);
        }
    }
}

/// The displays were arranged anew: the bars are told to look at their
/// place again, and the work areas are set afterwards.
pub(super) fn displays_changed(window: HWND) {
    let notes = SERVER.with_borrow_mut(AppBarServer::rearranged);
    post(&notes);
    rearrange_later(window);
}

/// Begins serving the bars, as Explorer's taskbar went: the work areas
/// are set again without the room it took, and Tessera's surfaces, which
/// registered with Explorer, register here.
pub(super) fn begin(window: HWND) {
    look_again();
    rearrange_later(window);
}

/// Forgets every bar without touching the work areas, as Explorer came
/// back and keeps the bars from now on. Tessera's surfaces register with
/// Explorer.
pub(super) fn forget() {
    SERVER.with_borrow_mut(|server| *server = AppBarServer::default());
    PENDING.set(false);
    look_again();
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

    /// A display of 1280 by 1024 right of the primary one of 1920 by 1080,
    /// listed first.
    fn monitors() -> Vec<Monitor> {
        let primary = rect(0, 0, 1920, 1080);
        let second = rect(1920, 0, 3200, 1024);
        vec![
            Monitor {
                frame: second,
                work: second,
                primary: false,
            },
            Monitor {
                frame: primary,
                work: primary,
                primary: true,
            },
        ]
    }

    fn request(message: u32, window: u32, edge: u32, at: ScreenRect, lparam: i64) -> Request {
        Request {
            size: SHARED_LENGTH as u32,
            window,
            callback: 0x8000 + window,
            edge,
            rect: at,
            lparam,
            message,
            shared: 0,
            owner: 0,
        }
    }

    /// Makes `rect` a strip `depth` deep at `edge`, keeping its inner side
    /// where the edge is.
    fn strip(mut rect: ScreenRect, edge: u32, depth: i32) -> ScreenRect {
        match edge {
            ABE_LEFT => rect.right = rect.left + depth,
            ABE_TOP => rect.bottom = rect.top + depth,
            ABE_RIGHT => rect.left = rect.right - depth,
            _ => rect.top = rect.bottom - depth,
        }
        rect
    }

    /// Registers the bar `window` and has it take `depth` pixels at `edge`
    /// of the display `frame`, as the sample of the documentation does:
    /// it asks with a strip of its depth, then sets the strip it got.
    fn dock(
        server: &mut AppBarServer,
        window: u32,
        edge: u32,
        frame: ScreenRect,
        depth: i32,
    ) -> ScreenRect {
        let monitors = monitors();
        server.answer(&request(ABM_NEW, window, 0, frame, 0), false, &monitors);
        let asked = request(ABM_QUERYPOS, window, edge, strip(frame, edge, depth), 0);
        let (_, got) = server
            .answer(&asked, false, &monitors)
            .place
            .expect("a place");
        let set = request(ABM_SETPOS, window, edge, strip(got, edge, depth), 0);
        server
            .answer(&set, false, &monitors)
            .place
            .expect("a place")
            .1
    }

    #[test]
    fn a_request_is_read_from_its_bytes() {
        let mut data = vec![0u8; 64];
        let mut put =
            |at: usize, value: u32| data[at..at + 4].copy_from_slice(&value.to_le_bytes());
        put(0, 40);
        put(4, 0x1234);
        put(8, 0x8001);
        put(12, ABE_BOTTOM);
        put(16, (-1280i32).cast_unsigned());
        put(20, 1000);
        put(24, 0);
        put(28, 1024);
        put(32, 1);
        put(MESSAGE, ABM_SETPOS);
        put(SHARED_HANDLE, 0xdead);
        put(SHARED_OWNER, 4242);
        let read = Request::parse(&data);
        assert_eq!(
            (
                read.size,
                read.window,
                read.callback,
                read.edge,
                read.message
            ),
            (40, 0x1234, 0x8001, ABE_BOTTOM, ABM_SETPOS)
        );
        assert_eq!(read.rect, rect(-1280, 1000, 0, 1024));
        assert_eq!((read.lparam, read.shared, read.owner), (1, 0xdead, 4242));

        let short = Request::parse(&data[..20]);
        assert_eq!((short.window, short.message, short.shared), (0x1234, 0, 0));
    }

    #[test]
    fn the_place_is_written_only_into_what_the_block_holds() {
        let mut block = vec![0xffu8; SHARED_LENGTH];
        write_place(&mut block, ABE_TOP, rect(1, 2, 3, -4));
        assert_eq!(&block[..SHARED_EDGE], &[0xff; SHARED_EDGE]);
        assert_eq!(&block[SHARED_EDGE..SHARED_EDGE + 4], &ABE_TOP.to_le_bytes());
        assert_eq!(
            &block[SHARED_RECT + 12..SHARED_RECT + 16],
            &(-4i32).to_le_bytes()
        );
        assert_eq!(&block[32..], &[0xff; 8]);

        let mut short = vec![0u8; 24];
        write_place(&mut short, ABE_LEFT, rect(7, 8, 9, 10));
        assert_eq!(&short[SHARED_RECT..SHARED_RECT + 4], &7i32.to_le_bytes());
        assert_eq!(&short[SHARED_RECT + 4..], &8i32.to_le_bytes());
    }

    #[test]
    fn bars_are_fitted_in_the_order_they_registered() {
        let primary = rect(0, 0, 1920, 1080);
        let mut server = AppBarServer::default();
        assert_eq!(
            dock(&mut server, 1, ABE_TOP, primary, 48),
            rect(0, 0, 1920, 48)
        );
        // A second bar at the same edge comes below the first.
        assert_eq!(
            dock(&mut server, 2, ABE_TOP, primary, 30),
            rect(0, 48, 1920, 78)
        );
        // One at the side starts below both.
        assert_eq!(
            dock(&mut server, 3, ABE_LEFT, primary, 60),
            rect(0, 78, 60, 1080)
        );
        // One at the bottom is cut short by the one at the side.
        assert_eq!(
            dock(&mut server, 4, ABE_BOTTOM, primary, 40),
            rect(60, 1040, 1920, 1080)
        );

        // The first bar stays where it is: none came before it.
        let monitors = monitors();
        let asked = request(ABM_QUERYPOS, 1, ABE_TOP, primary, 0);
        let asked = server.answer(&asked, false, &monitors);
        assert_eq!(asked.place, Some((ABE_TOP, primary)));
        // An unknown window is not fitted.
        let asked = request(ABM_QUERYPOS, 9, ABE_TOP, primary, 0);
        let asked = server.answer(&asked, false, &monitors);
        assert_eq!((asked.result, asked.place), (0, None));
    }

    #[test]
    fn a_bar_on_another_display_does_not_move_one_here() {
        let mut server = AppBarServer::default();
        dock(&mut server, 1, ABE_TOP, rect(1920, 0, 3200, 1024), 40);
        assert_eq!(
            dock(&mut server, 2, ABE_TOP, rect(0, 0, 1920, 1080), 40),
            rect(0, 0, 1920, 40)
        );
    }

    #[test]
    fn the_work_areas_leave_out_the_bars() {
        let primary = rect(0, 0, 1920, 1080);
        let second = rect(1920, 0, 3200, 1024);
        let mut server = AppBarServer::default();
        dock(&mut server, 1, ABE_BOTTOM, primary, 48);
        dock(&mut server, 2, ABE_RIGHT, second, 100);
        dock(&mut server, 3, ABE_TOP, primary, 30);
        let monitors = monitors();
        assert_eq!(
            server.work_areas(&monitors),
            vec![rect(1920, 0, 3100, 1024), rect(0, 30, 1920, 1032)]
        );

        // A bar that hides takes no room.
        let hides = request(ABM_SETAUTOHIDEBAR, 3, ABE_TOP, primary, 1);
        let answer = server.answer(&hides, false, &monitors);
        assert_eq!(answer.result, 1);
        assert!(answer.rearranged);
        assert_eq!(server.work_areas(&monitors)[1], rect(0, 0, 1920, 1032));

        // Nor does one that set its whole display as its place.
        let whole = request(ABM_SETPOS, 1, ABE_BOTTOM, primary, 0);
        server.answer(&whole, false, &monitors);
        assert_eq!(server.work_areas(&monitors)[1], primary);
    }

    #[test]
    fn the_other_bars_are_told_when_one_moves() {
        let primary = rect(0, 0, 1920, 1080);
        let monitors = monitors();
        let mut server = AppBarServer::default();
        dock(&mut server, 1, ABE_TOP, primary, 48);
        server.answer(&request(ABM_NEW, 2, 0, primary, 0), false, &monitors);

        let thinner = request(ABM_SETPOS, 1, ABE_TOP, rect(0, 0, 1920, 30), 0);
        let set = server.answer(&thinner, false, &monitors);
        assert!(set.rearranged);
        assert_eq!(
            set.notes,
            vec![Note {
                window: 2,
                callback: 0x8002,
                code: ABN_POSCHANGED,
                lparam: 0,
                ours: false,
            }]
        );
        // The same place again is no news.
        let again = server.answer(&thinner, false, &monitors);
        assert!(!again.rearranged && again.notes.is_empty());

        let removed = server.answer(&request(ABM_REMOVE, 1, 0, primary, 0), false, &monitors);
        assert!(removed.rearranged);
        assert_eq!(removed.notes.len(), 1);
        assert_eq!(server.work_areas(&monitors)[1], primary);
        // A bar that never took a place leaves nothing to rearrange.
        let removed = server.answer(&request(ABM_REMOVE, 2, 0, primary, 0), false, &monitors);
        assert!(!removed.rearranged && removed.notes.is_empty());
    }

    #[test]
    fn a_surface_of_tessera_hears_of_moves_without_a_message() {
        let primary = rect(0, 0, 1920, 1080);
        let monitors = monitors();
        let mut server = AppBarServer::default();
        server.answer(&request(ABM_NEW, 1, 0, primary, 0), true, &monitors);
        let other = dock(&mut server, 2, ABE_TOP, primary, 30);
        let moved = request(ABM_SETPOS, 2, ABE_TOP, rect(0, 0, 1920, 60), 0);
        let notes = server.answer(&moved, false, &monitors).notes;
        assert_eq!(other, rect(0, 0, 1920, 30));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].ours && notes[0].code == ABN_POSCHANGED);

        let before = places();
        post(&notes);
        assert_eq!(places(), before + 1);
    }

    #[test]
    fn a_window_registers_once() {
        let monitors = monitors();
        let mut server = AppBarServer::default();
        let new = request(ABM_NEW, 1, 0, rect(0, 0, 0, 0), 0);
        assert_eq!(server.answer(&new, false, &monitors).result, 1);
        assert_eq!(server.answer(&new, false, &monitors).result, 0);
        let nobody = request(ABM_NEW, 0, 0, rect(0, 0, 0, 0), 0);
        assert_eq!(server.answer(&nobody, false, &monitors).result, 0);
    }

    #[test]
    fn each_edge_of_a_display_has_one_bar_that_hides() {
        let primary = rect(0, 0, 1920, 1080);
        let second = rect(1920, 0, 3200, 1024);
        let monitors = monitors();
        let mut server = AppBarServer::default();
        let mut ask = |message, window, at, lparam| {
            let asked = request(message, window, ABE_BOTTOM, at, lparam);
            server.answer(&asked, false, &monitors).result
        };
        assert_eq!(ask(ABM_SETAUTOHIDEBAR, 1, primary, 1), 1);
        assert_eq!(ask(ABM_SETAUTOHIDEBAR, 2, primary, 1), 0);
        // Another display has its own.
        assert_eq!(ask(ABM_SETAUTOHIDEBAREX, 2, second, 1), 1);
        assert_eq!(ask(ABM_GETAUTOHIDEBAR, 0, primary, 0), 1);
        assert_eq!(ask(ABM_GETAUTOHIDEBAREX, 0, second, 0), 2);
        // Only the one that holds an edge gives it up.
        assert_eq!(ask(ABM_SETAUTOHIDEBAR, 2, primary, 0), 0);
        assert_eq!(ask(ABM_SETAUTOHIDEBAR, 1, primary, 0), 1);
        assert_eq!(ask(ABM_GETAUTOHIDEBAR, 0, primary, 0), 0);
    }

    #[test]
    fn the_taskbar_is_a_surface_of_tessera_that_never_hides() {
        let primary = rect(0, 0, 1920, 1080);
        let monitors = monitors();
        let mut server = AppBarServer::default();
        let position = request(ABM_GETTASKBARPOS, 5, 0, primary, 0);
        let state = request(ABM_GETSTATE, 5, 0, primary, 0);
        let asked = server.answer(&position, false, &monitors);
        assert_eq!((asked.result, asked.place), (0, None));
        let asked = server.answer(&state, false, &monitors);
        assert_eq!(asked.result, ABS_ALWAYSONTOP as isize);

        dock(&mut server, 1, ABE_TOP, primary, 40);
        server.answer(&request(ABM_NEW, 2, 0, primary, 0), true, &monitors);
        let bottom = rect(0, 1032, 1920, 1080);
        let set = request(ABM_SETPOS, 2, ABE_BOTTOM, bottom, 0);
        server.answer(&set, true, &monitors);
        let asked = server.answer(&position, false, &monitors);
        assert_eq!((asked.result, asked.place), (1, Some((ABE_BOTTOM, bottom))));

        // A bar that hides is put above the others when it becomes
        // active; one that does not is left where it is.
        let activate = request(ABM_ACTIVATE, 1, 0, primary, 1);
        assert_eq!(server.answer(&activate, false, &monitors).raised, None);
        let hides = request(ABM_SETAUTOHIDEBAR, 1, ABE_TOP, primary, 1);
        assert_eq!(server.answer(&hides, false, &monitors).result, 1);
        assert_eq!(server.answer(&activate, false, &monitors).raised, Some(1));
        let asked = server.answer(&state, false, &monitors);
        assert_eq!(asked.result, ABS_ALWAYSONTOP as isize);
    }

    #[test]
    fn bars_on_a_filled_display_are_told_once() {
        let primary = rect(0, 0, 1920, 1080);
        let second = rect(1920, 0, 3200, 1024);
        let monitors = monitors();
        let mut server = AppBarServer::default();
        dock(&mut server, 1, ABE_TOP, primary, 40);
        dock(&mut server, 2, ABE_TOP, second, 40);
        assert!(server.has_others());
        assert_eq!(server.watched(&monitors), vec![(primary, 1), (second, 2)]);

        let notes = server.fullscreen(primary, true, &monitors);
        assert_eq!(
            notes,
            vec![Note {
                window: 1,
                callback: 0x8001,
                code: ABN_FULLSCREENAPP,
                lparam: 1,
                ours: false,
            }]
        );
        assert!(server.fullscreen(primary, true, &monitors).is_empty());
        assert_eq!(server.fullscreen(primary, false, &monitors)[0].lparam, 0);
        // After the displays were arranged anew it is news again.
        server.fullscreen(primary, true, &monitors);
        assert_eq!(server.rearranged().len(), 2);
        assert_eq!(server.fullscreen(primary, true, &monitors).len(), 1);
    }

    #[test]
    fn bars_of_closed_windows_are_forgotten() {
        let primary = rect(0, 0, 1920, 1080);
        let monitors = monitors();
        let mut server = AppBarServer::default();
        dock(&mut server, 1, ABE_TOP, primary, 40);
        dock(&mut server, 2, ABE_TOP, primary, 40);
        let answer = server.prune(|window| window == 2);
        assert!(answer.rearranged);
        assert_eq!(answer.notes.len(), 1);
        // The bar left is told to move up, and keeps its room until it has.
        assert_eq!(server.work_areas(&monitors)[1], rect(0, 80, 1920, 1080));
        let answer = server.prune(|_| true);
        assert!(!answer.rearranged && answer.notes.is_empty());
    }

    #[test]
    fn the_primary_display_gets_its_work_area_last() {
        let monitors = monitors();
        let areas = [rect(1920, 40, 3200, 1024), rect(0, 40, 1920, 1080)];
        assert_eq!(changes(&monitors, &areas), vec![areas[0], areas[1]]);
        let areas = [rect(1920, 0, 3200, 1024), rect(0, 40, 1920, 1080)];
        assert_eq!(changes(&monitors, &areas), vec![areas[1]]);
        let areas = [rect(1920, 0, 3200, 1024), rect(0, 1080, 1920, 1080)];
        assert!(changes(&monitors, &areas).is_empty());

        let monitors = [monitors[1], monitors[0]];
        let areas = [rect(0, 40, 1920, 1080), rect(1920, 40, 3200, 1024)];
        assert_eq!(changes(&monitors, &areas), vec![areas[1], areas[0]]);
    }

    #[test]
    fn any_numbers_a_bar_sends_are_fitted() {
        let primary = rect(0, 0, 1920, 1080);
        let monitors = monitors();
        let mut server = AppBarServer::default();
        dock(&mut server, 1, ABE_LEFT, primary, 60);
        let far = rect(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
        for edge in [ABE_LEFT, ABE_TOP, ABE_RIGHT, ABE_BOTTOM] {
            server.answer(&request(ABM_NEW, 2, 0, far, 0), false, &monitors);
            let set = server.answer(&request(ABM_SETPOS, 2, edge, far, 0), false, &monitors);
            assert_eq!(set.result, 1);
            server.work_areas(&monitors);
            server.answer(&request(ABM_REMOVE, 2, 0, far, 0), false, &monitors);
        }
        let mut moved = rect(i32::MAX - 1, 0, i32::MAX, 10);
        shift(&mut moved, ABE_LEFT, rect(i32::MIN, 0, i32::MAX, 10));
        assert_eq!(moved.right, i32::MAX);
        assert!(!plausible(ABE_TOP, far, primary));
        assert_eq!(
            display_of(rect(i32::MIN, 0, i32::MIN, 0), &monitors),
            Some(1)
        );
    }

    #[test]
    fn a_place_off_every_display_belongs_to_the_nearest() {
        let monitors = monitors();
        assert_eq!(display_of(rect(3300, 0, 3400, 50), &monitors), Some(0));
        assert_eq!(display_of(rect(-200, 0, -100, 50), &monitors), Some(1));
        assert_eq!(display_of(rect(1900, 0, 2000, 50), &monitors), Some(0));
        assert_eq!(display_of(rect(0, 0, 1, 1), &[]), None);
    }
}
