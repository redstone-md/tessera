// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What snapping windows needs to know of Windows: the displays with their
//! work areas and scales, whether Windows arranges dragged windows itself,
//! which window is under the pointer, when the pointer rests on the
//! maximize button of one, how small a window can be made, and placing
//! the windows that share edges together.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError, mpsc};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, MonitorFromWindow,
};
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos, GA_ROOT, GetAncestor, GetClassNameW,
    GetForegroundWindow, GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId, HTBOTTOMRIGHT,
    HTLEFT, HTMAXBUTTON, IsWindowVisible, MINMAXINFO, SM_CXMINTRACK, SM_CYMINTRACK,
    SMTO_ABORTIFHUNG, SMTO_BLOCK, SPI_GETWINARRANGING, SPI_SETWINARRANGING, SWP_NOACTIVATE,
    SWP_NOOWNERZORDER, SWP_NOZORDER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageTimeoutW,
    SystemParametersInfoW, WM_GETMINMAXINFO, WM_NCHITTEST, WindowFromPoint,
};
use windows::core::BOOL;

use super::registry::registry_text;
use super::{
    AppWindowId, DisplayKey, NOT_FILLING, NativeSurface, ScreenRect, pointer_at, pointer_held,
};

/// The logical pixels a display has per inch at a scale of 1.
const BASE_DPI: f32 = 96.;

/// A display, as windows are snapped on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    pub key: DisplayKey,
    /// The whole display on the virtual screen, in physical pixels.
    pub frame: ScreenRect,
    /// What the bars at its edges leave to windows.
    pub work: ScreenRect,
    /// Physical pixels per logical one.
    pub scale: f32,
}

impl Display {
    /// Every display, from the left to the right and, where two begin at
    /// the same place, from the top down.
    pub fn all() -> Vec<Self> {
        unsafe extern "system" fn add(
            display: HMONITOR,
            _: HDC,
            _: *mut RECT,
            found: LPARAM,
        ) -> BOOL {
            // SAFETY: `found` is the pointer `all` passed, to a list that
            // outlives the enumeration.
            let found = unsafe { &mut *(found.0 as *mut Vec<HMONITOR>) };
            found.push(display);
            true.into()
        }
        let mut found: Vec<HMONITOR> = Vec::new();
        // SAFETY: the enumeration is synchronous and `found` outlives it.
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&raw mut found as isize));
        }
        let mut displays: Vec<Self> = found.into_iter().filter_map(Self::of).collect();
        displays.sort_by_key(|display| (display.frame.left, display.frame.top));
        displays
    }

    /// The display at `point`, or the nearest one to it.
    pub fn at((x, y): (i32, i32)) -> Option<Self> {
        // SAFETY: the call only reads the arrangement of the displays.
        Self::of(unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) })
    }

    /// The display that holds most of the window `id`, or the nearest one.
    pub fn of_window(id: AppWindowId) -> Option<Self> {
        // SAFETY: the call only reads window state and tolerates a handle
        // whose window has since been destroyed.
        Self::of(unsafe { MonitorFromWindow(id.hwnd(), MONITOR_DEFAULTTONEAREST) })
    }

    fn of(display: HMONITOR) -> Option<Self> {
        if display.is_invalid() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let (mut dpi, mut unused) = (0u32, 0u32);
        // SAFETY: every value outlives the calls, `info` is sized as the
        // call is told, and a display that is gone makes them fail.
        let (read, scaled) = unsafe {
            (
                GetMonitorInfoW(display, &raw mut info).as_bool(),
                GetDpiForMonitor(display, MDT_EFFECTIVE_DPI, &raw mut dpi, &raw mut unused).is_ok(),
            )
        };
        if !read {
            return None;
        }
        let scale = match scaled && dpi > 0 {
            true => dpi as f32 / BASE_DPI,
            false => 1.,
        };
        Some(Self {
            key: DisplayKey(display.0 as isize),
            frame: info.rcMonitor.into(),
            work: info.rcWork.into(),
            scale,
        })
    }
}

/// Whether Windows itself arranges windows that are dragged to an edge of
/// a display, and answers the Windows key with the arrows: its setting
/// "Snap windows".
pub struct WindowArranging;

impl WindowArranging {
    /// Whether Windows arranges windows now.
    pub fn is_on() -> bool {
        let mut on = BOOL(1);
        // SAFETY: `on` is the value the call fills and outlives it.
        let read = unsafe {
            SystemParametersInfoW(
                SPI_GETWINARRANGING,
                0,
                Some((&raw mut on).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        read.is_err() || on.as_bool()
    }

    /// Whether the user has Windows arrange windows, as the settings keep
    /// it, whatever it does now: a Tessera that ended without giving the
    /// arranging back leaves it off until the next sign in.
    pub fn chosen() -> bool {
        let key = r"Control Panel\Desktop";
        let chosen = registry_text(HKEY_CURRENT_USER, key, Some("WindowArrangementActive"), 8);
        // Without the value Windows arranges windows.
        chosen.is_none_or(|text| !text.starts_with('0'))
    }

    /// Has Windows arrange windows, or not, until the user signs out; the
    /// setting the user chose stays as it is written. Whether Windows took
    /// it is returned.
    pub fn set(on: bool) -> bool {
        // The value is given itself in place of a pointer, as Windows asks
        // for this setting.
        let value = std::ptr::without_provenance_mut::<c_void>(usize::from(on));
        // SAFETY: Windows reads the value, not what it would point to, and
        // nothing is written to the settings of the user.
        let set = unsafe {
            SystemParametersInfoW(
                SPI_SETWINARRANGING,
                u32::from(on),
                Some(value),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        set.is_ok()
    }
}

impl NativeSurface {
    /// Asks for keyboard input to go to the window, as
    /// [`Self::activate`] does. Windows refuses that when the last input
    /// went to another program, as it has after a window of that program
    /// was dragged; then a key nothing listens to is pressed from here,
    /// which counts as input to Tessera, and the window is asked for
    /// again. Whether the window is in front afterwards is returned.
    pub fn take_keyboard(self) -> bool {
        // SAFETY: the call takes no arguments and only reads which window
        // is in front.
        let in_front = || unsafe { GetForegroundWindow() } == self.hwnd;
        self.activate();
        if in_front() {
            return true;
        }
        super::windows_key::press_unassigned_key();
        self.activate();
        in_front()
    }
}

/// Whether a Shift key is held down.
pub fn shift_held() -> bool {
    // SAFETY: the call takes a key and only reads its state.
    unsafe { GetAsyncKeyState(i32::from(VK_SHIFT.0)) < 0 }
}

/// The top-level window of another program under `point`, if there is one.
pub fn window_at((x, y): (i32, i32)) -> Option<AppWindowId> {
    // SAFETY: the calls only read which window is at a point, its top
    // window and whose it is.
    unsafe {
        let window = WindowFromPoint(POINT { x, y });
        if window.is_invalid() {
            return None;
        }
        let root = GetAncestor(window, GA_ROOT);
        let root = if root.is_invalid() { window } else { root };
        let mut process = 0;
        GetWindowThreadProcessId(root, Some(&raw mut process));
        (process != std::process::id()).then(|| AppWindowId::from_raw(root.0 as isize))
    }
}

/// How long the window is given to say what is at a point of it.
const HIT_TEST_TIMEOUT: u32 = 60;

/// What the window `id` says is at `point` of it, as one of the `HT`
/// values of `WM_NCHITTEST`; `None` when it does not answer in time, as a
/// window that hangs does not.
pub(super) fn hit_test(id: AppWindowId, (x, y): (i32, i32)) -> Option<u32> {
    let packed = ((y & 0xffff) << 16) | (x & 0xffff);
    let mut answer = 0usize;
    // SAFETY: `answer` outlives the call, which gives up on a window that
    // hangs or does not answer in time and fails for one that is gone.
    let answered = unsafe {
        SendMessageTimeoutW(
            id.hwnd(),
            WM_NCHITTEST,
            WPARAM(0),
            LPARAM(packed as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            HIT_TEST_TIMEOUT,
            Some(&raw mut answer),
        )
    };
    (answered.0 != 0).then_some(answer as u32)
}

/// Whether `point` is on the maximize button of the window `id`, as the
/// window itself says. A window that does not answer at once is not asked
/// again until the pointer moves on.
pub fn on_maximize_button(id: AppWindowId, point: (i32, i32)) -> bool {
    hit_test(id, point) == Some(HTMAXBUTTON)
}

/// Whether `point` is on the border of the window `id` that resizes it,
/// on a side or at a corner, as the window itself says; `None` when it
/// does not answer in time. The window waits for its program, so this is
/// asked away from the thread of the user interface.
pub fn on_resizing_border(id: AppWindowId, point: (i32, i32)) -> Option<bool> {
    hit_test(id, point).map(|hit| (HTLEFT..=HTBOTTOMRIGHT).contains(&hit))
}

/// Whether the window in front fills all of `display`, as a game or a
/// video shown full screen does. The desktop, the screens of Windows that
/// show all windows and Tessera's own windows do not count.
pub fn front_fills(display: ScreenRect) -> bool {
    // SAFETY: the calls only read the state of the window in front, and a
    // window that went away makes them fail, which counts as not filling.
    unsafe {
        let front = GetForegroundWindow();
        if front.is_invalid() || !IsWindowVisible(front).as_bool() {
            return false;
        }
        let mut process = 0;
        GetWindowThreadProcessId(front, Some(&raw mut process));
        if process == std::process::id() {
            return false;
        }
        let mut name = [0u16; 64];
        let length = GetClassNameW(front, &mut name).max(0) as usize;
        if NOT_FILLING.contains(&String::from_utf16_lossy(&name[..length]).as_str()) {
            return false;
        }
        let mut frame = RECT::default();
        GetWindowRect(front, &raw mut frame).is_ok() && ScreenRect::from(frame).covers(display)
    }
}

/// How long the pointer rests on a maximize button before it is reported,
/// in milliseconds; 0 while nothing is to be reported.
static HOVER_DELAY: AtomicU64 = AtomicU64::new(0);

/// How often the pointer is looked at while it is to be.
const HOVER_LOOK: Duration = Duration::from_millis(80);

/// How far below the top of a window and how far from its right side the
/// pointer may be for the window to be asked whether it is on its
/// maximize button, in physical pixels.
const CAPTION_BAND: (i32, i32) = (96, 320);

/// Reports the pointer resting on the maximize button of a window.
pub struct MaximizeHover;

/// A window whose maximize button the pointer rests on, and where the
/// pointer is.
type Hover = (AppWindowId, (i32, i32));

fn hovers() -> &'static (Sender<Hover>, Receiver<Hover>) {
    static CHANNEL: OnceLock<(Sender<Hover>, Receiver<Hover>)> = OnceLock::new();
    CHANNEL.get_or_init(|| async_channel::bounded(4))
}

impl MaximizeHover {
    /// Reports from now on every window whose maximize button the pointer
    /// has rested on for `delay`, with where the pointer is, once for each
    /// time it comes there; nothing with `None`. The pointer is looked at
    /// from a thread of its own, a few times a second, and the window is
    /// asked only while the pointer is near its top right corner.
    pub fn watch(delay: Option<Duration>) -> Receiver<Hover> {
        static WATCHER: OnceLock<Option<Thread>> = OnceLock::new();
        let millis = delay.map_or(0, |delay| delay.as_millis().max(1) as u64);
        HOVER_DELAY.store(millis, Ordering::Relaxed);
        let watcher = WATCHER.get_or_init(|| {
            let started = thread::Builder::new()
                .name("tessera maximize hover".to_owned())
                .spawn(watch_hover);
            match started {
                Ok(handle) => Some(handle.thread().clone()),
                Err(error) => {
                    report!("the snap layouts do not open over a maximize button: {error}");
                    None
                }
            }
        });
        if let Some(watcher) = watcher {
            watcher.unpark();
        }
        hovers().1.clone()
    }
}

/// Where the pointer rests and since when.
struct Resting {
    window: AppWindowId,
    since: Instant,
    reported: bool,
}

fn watch_hover() {
    let mut resting: Option<Resting> = None;
    loop {
        let delay = HOVER_DELAY.load(Ordering::Relaxed);
        if delay == 0 {
            resting = None;
            thread::park();
            continue;
        }
        thread::sleep(HOVER_LOOK);
        let on = pointer_at().filter(|_| !pointer_held()).and_then(|point| {
            let window = window_at(point)?;
            let frame = window.frame()?;
            let near =
                point.1 - frame.top < CAPTION_BAND.0 && frame.right - point.0 < CAPTION_BAND.1;
            (near && on_maximize_button(window, point)).then_some((window, point))
        });
        let Some((window, point)) = on else {
            resting = None;
            continue;
        };
        let now = Instant::now();
        let rest = match resting.as_mut() {
            Some(rest) if rest.window == window => rest,
            _ => resting.insert(Resting {
                window,
                since: now,
                reported: false,
            }),
        };
        if !rest.reported && now.duration_since(rest.since) >= Duration::from_millis(delay) {
            rest.reported = true;
            let _ = hovers().0.try_send((window, point));
        }
    }
}

/// How long a window is given to say how small it can be made.
const LEAST_TIMEOUT: u32 = 100;

/// The least width and height of what is seen of the window `id`, as the
/// window answers `WM_GETMINMAXINFO`, without its invisible border; the
/// least Windows gives any window where it does not answer in time. The
/// window waits for its program, so this is asked away from the thread of
/// the user interface.
pub fn least_size(id: AppWindowId) -> (i32, i32) {
    let hwnd = id.hwnd();
    // SAFETY: the calls only read sizes; `info` and `whole` outlive them,
    // the message gives up on a window that hangs or does not answer in
    // time, and Windows copies `info` to and from the window's process.
    unsafe {
        let usual = (
            GetSystemMetrics(SM_CXMINTRACK),
            GetSystemMetrics(SM_CYMINTRACK),
        );
        let mut info = MINMAXINFO {
            ptMinTrackSize: POINT {
                x: usual.0,
                y: usual.1,
            },
            ..Default::default()
        };
        let answered = SendMessageTimeoutW(
            hwnd,
            WM_GETMINMAXINFO,
            WPARAM(0),
            LPARAM(&raw mut info as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            LEAST_TIMEOUT,
            None,
        );
        let (width, height) = match answered.0 != 0 {
            true => (info.ptMinTrackSize.x, info.ptMinTrackSize.y),
            false => usual,
        };
        let mut whole = RECT::default();
        let (Some(seen), Ok(())) = (id.frame(), GetWindowRect(hwnd, &raw mut whole)) else {
            return (width.max(1), height.max(1));
        };
        let border_x = (whole.right - whole.left) - (seen.right - seen.left);
        let border_y = (whole.bottom - whole.top) - (seen.bottom - seen.top);
        ((width - border_x).max(1), (height - border_y).max(1))
    }
}

/// Windows to place together, each with what is to be seen of it.
pub type Placements = Vec<(AppWindowId, ScreenRect)>;

/// Places windows of other programs together in one step, as the edges a
/// snapped window shares follow it. Windows sends the windows messages
/// for this and waits for them, so it is done on a thread of its own; of
/// the placements asked for meanwhile only the latest is made.
pub struct Arranger;

impl Arranger {
    /// Has the windows of `placements` placed, each so that what is seen
    /// of it covers its rect, without bringing any forward. A window that
    /// hangs or is gone is left out.
    pub fn place(placements: Placements) {
        static SENDER: OnceLock<Option<Mutex<mpsc::Sender<Placements>>>> = OnceLock::new();
        let sender = SENDER.get_or_init(|| {
            let (send, receive) = mpsc::channel::<Placements>();
            let started = thread::Builder::new()
                .name("tessera shared edges".to_owned())
                .spawn(move || {
                    while let Ok(mut latest) = receive.recv() {
                        while let Ok(newer) = receive.try_recv() {
                            latest = newer;
                        }
                        arrange(&latest);
                    }
                });
            match started {
                Ok(_) => Some(Mutex::new(send)),
                Err(error) => {
                    report!("the windows that share edges are not moved together: {error}");
                    None
                }
            }
        });
        if let Some(sender) = sender {
            let sender = sender.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = sender.send(placements);
        }
    }
}

/// Places the windows of `placements` together, with what Windows calls
/// one deferred placement.
fn arrange(placements: &[(AppWindowId, ScreenRect)]) {
    let mut wholes = Vec::with_capacity(placements.len());
    for &(id, frame) in placements {
        if id.is_hung() {
            continue;
        }
        let Some(seen) = id.frame() else {
            continue;
        };
        let mut whole = RECT::default();
        // SAFETY: `whole` outlives the call, which tolerates a handle
        // whose window has since been destroyed.
        if unsafe { GetWindowRect(id.hwnd(), &raw mut whole) }.is_err() {
            continue;
        }
        // The invisible border stays as wide as it is.
        let placed = ScreenRect {
            left: frame.left - (seen.left - whole.left),
            top: frame.top - (seen.top - whole.top),
            right: frame.right + (whole.right - seen.right),
            bottom: frame.bottom + (whole.bottom - seen.bottom),
        };
        wholes.push((id, placed));
    }
    if wholes.is_empty() {
        return;
    }
    let flags = SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE;
    // SAFETY: the handle of the deferred placement is used only here and
    // given back by EndDeferWindowPos; a window that is gone makes
    // DeferWindowPos fail, which gives the handle back as well.
    unsafe {
        let Ok(mut deferred) = BeginDeferWindowPos(wholes.len() as i32) else {
            return;
        };
        for (id, rect) in wholes {
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            match DeferWindowPos(
                deferred,
                id.hwnd(),
                None,
                rect.left,
                rect.top,
                width,
                height,
                flags,
            ) {
                Ok(next) => deferred = next,
                Err(_) => return,
            }
        }
        let _ = EndDeferWindowPos(deferred);
    }
}
