// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The buttons of the pointer as the window actions take them, as KWin
//! does: with a key held (Alt or the Windows key), the left button pressed
//! on a window of another program begins moving it and the right button
//! resizing it, and the window gets nothing of those presses; and the
//! middle button pressed on a window is told of, which then still gets it.
//! Also whether Windows itself gives the keyboard to the window under the
//! pointer (focus follows the mouse).
//!
//! The pointer is watched with a low-level hook on a thread of its own
//! that does nothing else and pumps its messages, as Windows asks of such
//! a hook. The hook decides from what it can read at once whether to take
//! a press: whether the key is held and which window is under the
//! pointer, which asks no window anything. It never waits; what is to be
//! done goes to the shell on a channel. The thread runs only while the
//! configuration asks for one of these.
//!
//! Windows does not call the hook for presses meant for a program that
//! runs with more rights than Tessera, so the windows of such a program
//! cannot be moved this way.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError, mpsc};
use std::thread;

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SAM_FLAGS};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_LWIN, VK_MENU, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetClassNameW, GetMessageW, GetWindowRect, HC_ACTION, IsIconic,
    IsWindowVisible, IsZoomed, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, PostThreadMessageW,
    SPI_GETACTIVEWINDOWTRACKING, SPI_GETACTIVEWNDTRKTIMEOUT, SPI_GETACTIVEWNDTRKZORDER,
    SPI_SETACTIVEWINDOWTRACKING, SPI_SETACTIVEWNDTRKTIMEOUT, SPI_SETACTIVEWNDTRKZORDER,
    SYSTEM_PARAMETERS_INFO_ACTION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetWindowsHookExW,
    SystemParametersInfoW, UnhookWindowsHookEx, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
};
use windows::core::BOOL;

use super::AppWindowId;
use super::registry::Key;
use super::snap::window_at;

/// The key held to move and resize windows with the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrabKey {
    Alt,
    Win,
}

impl GrabKey {
    fn code(self) -> u8 {
        match self {
            Self::Alt => 1,
            Self::Win => 2,
        }
    }

    fn of_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Alt),
            2 => Some(Self::Win),
            _ => None,
        }
    }

    /// Whether the key is held down now.
    fn is_held(self) -> bool {
        let held = |key: VIRTUAL_KEY| {
            // SAFETY: the call takes a key and only reads its state.
            unsafe { GetAsyncKeyState(i32::from(key.0)) < 0 }
        };
        match self {
            Self::Alt => held(VK_MENU),
            Self::Win => held(VK_LWIN) || held(VK_RWIN),
        }
    }
}

/// A button of the pointer that moves or resizes a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    /// Moves the window.
    Left,
    /// Resizes the window.
    Right,
}

impl PointerButton {
    fn code(self) -> u8 {
        match self {
            Self::Left => 1,
            Self::Right => 2,
        }
    }
}

/// What the hook saw of the buttons of the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerEvent {
    /// `button` was pressed at `at` with the key held, over `window`,
    /// which got nothing of it.
    Grab {
        button: PointerButton,
        window: AppWindowId,
        at: (i32, i32),
    },
    /// The button of the grab was let go at `at`; the window got nothing
    /// of that either.
    Release { at: (i32, i32) },
    /// The middle button was pressed at `at` over `window`, which got the
    /// press as well.
    Middle { window: AppWindowId, at: (i32, i32) },
}

/// The key that takes presses, as [`GrabKey::code`] gives it; 0 for none.
static GRAB_KEY: AtomicU8 = AtomicU8::new(0);
/// Whether the middle button is told of.
static MIDDLE: AtomicBool = AtomicBool::new(false);
/// The button of the grab under way, as [`PointerButton::code`] gives it;
/// 0 while there is none.
static GRABBED: AtomicU8 = AtomicU8::new(0);
/// The thread that has the hook, while there is one.
static HOOK_THREAD: Mutex<Option<u32>> = Mutex::new(None);

/// The classes of the windows that are no windows to move: the desktop
/// and the taskbars of Explorer, its window switcher and task view, the
/// start menu and search of Windows, and menus that pop up.
const NOT_MOVED: [&str; 9] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "XamlExplorerHostIslandWindow",
    "MultitaskingViewFrame",
    "Windows.UI.Core.CoreWindow",
    "#32768",
    "ForegroundStaging",
];

/// Whether a window of the class `class` is one to move.
fn moves_class(class: &str) -> bool {
    !NOT_MOVED.contains(&class)
}

fn channel() -> &'static (Sender<PointerEvent>, Receiver<PointerEvent>) {
    static CHANNEL: OnceLock<(Sender<PointerEvent>, Receiver<PointerEvent>)> = OnceLock::new();
    CHANNEL.get_or_init(async_channel::unbounded)
}

/// Takes presses of the buttons of the pointer as the window actions say.
pub struct MouseHook;

impl MouseHook {
    /// Takes the presses from now on with `grab` held, and tells of the
    /// middle button where `middle`; the hook is set when either is asked
    /// for and taken away when neither is. Whether Windows took the hook
    /// is returned, true when none is needed.
    pub fn configure(grab: Option<GrabKey>, middle: bool) -> bool {
        GRAB_KEY.store(grab.map_or(0, GrabKey::code), Ordering::Relaxed);
        MIDDLE.store(middle, Ordering::Relaxed);
        let wanted = grab.is_some() || middle;
        let mut thread = HOOK_THREAD.lock().unwrap_or_else(PoisonError::into_inner);
        match (*thread, wanted) {
            (Some(_), true) | (None, false) => true,
            (Some(running), false) => {
                // SAFETY: posting to a thread that has ended fails without
                // side effects.
                unsafe {
                    let _ = PostThreadMessageW(running, WM_QUIT, WPARAM(0), LPARAM(0));
                }
                *thread = None;
                GRABBED.store(0, Ordering::Relaxed);
                true
            }
            (None, true) => match start() {
                Ok(id) => {
                    *thread = Some(id);
                    true
                }
                Err(error) => {
                    report!("the buttons of the pointer move no windows: {error}");
                    false
                }
            },
        }
    }

    /// Has the hook no longer take the release of the grab under way, as
    /// when the button was let go where Windows did not show the hook:
    /// over a window of a program that runs with more rights than
    /// Tessera. The next press and release go to the windows as they
    /// come.
    pub fn end_grab(button: PointerButton) {
        let _ = GRABBED.compare_exchange(button.code(), 0, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Where what the hook saw arrives, in the order it happened. Every
    /// event goes to one receiver.
    pub fn events() -> Receiver<PointerEvent> {
        channel().1.clone()
    }

    /// Presses a key no keyboard has and no program listens to, so that
    /// the key held for a grab is not taken as pressed by itself when it
    /// is let go: Alt alone would open the menu of the window in front,
    /// the Windows key alone the start menu.
    pub fn break_lone_key() {
        /// A key code no keyboard has.
        const UNASSIGNED: VIRTUAL_KEY = VIRTUAL_KEY(0xe8);
        let event = |flags: KEYBD_EVENT_FLAGS| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: UNASSIGNED,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        };
        let events = [event(KEYBD_EVENT_FLAGS(0)), event(KEYEVENTF_KEYUP)];
        // SAFETY: the events outlive the call, which copies them.
        unsafe {
            SendInput(&events, size_of::<INPUT>() as i32);
        }
    }
}

/// Starts the thread of the hook and waits until it has the hook; its
/// number is returned.
fn start() -> Result<u32, String> {
    let (ready, started) = mpsc::channel();
    thread::Builder::new()
        .name("tessera pointer hook".to_owned())
        .spawn(move || hook_thread(ready))
        .map_err(|error| error.to_string())?;
    started.recv().map_err(|error| error.to_string())?
}

fn hook_thread(ready: mpsc::Sender<Result<u32, String>>) {
    // SAFETY: `pointer_event` is a function of this program, which is
    // never unloaded; a low-level hook is called in this process and
    // injects no module anywhere.
    let hook = match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(pointer_event), None, 0) } {
        Ok(hook) => hook,
        Err(error) => {
            ready.send(Err(error.to_string())).ok();
            return;
        }
    };
    // SAFETY: the call only reads which thread this is.
    ready.send(Ok(unsafe { GetCurrentThreadId() })).ok();
    let mut message = MSG::default();
    // SAFETY: `message` outlives every call; the thread has no windows, so
    // the loop only waits for the hook to be called and for the quit.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {}
        let _ = UnhookWindowsHookEx(hook);
    }
}

unsafe extern "system" fn pointer_event(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for this code Windows passes a pointer to the event of
        // the pointer, which is valid for the duration of the call.
        let event = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        // What a program made up, Tessera's own presses among them, is
        // passed on as it came.
        if event.flags & LLMHF_INJECTED == 0 && taken(wparam.0 as u32, event.pt) {
            return LRESULT(1);
        }
    }
    // SAFETY: the arguments are passed on as they came.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Whether the press or release `message` at `point` is taken from the
/// window under the pointer; what it means is sent on.
fn taken(message: u32, POINT { x, y }: POINT) -> bool {
    let at = (x, y);
    let send = |event| {
        // The channel has no bound, and nobody listening is no error.
        let _ = channel().0.try_send(event);
    };
    let button = match message {
        WM_LBUTTONDOWN | WM_LBUTTONUP => PointerButton::Left,
        WM_RBUTTONDOWN | WM_RBUTTONUP => PointerButton::Right,
        WM_MBUTTONDOWN => {
            if MIDDLE.load(Ordering::Relaxed)
                && let Some(window) = movable_at(at)
            {
                send(PointerEvent::Middle { window, at });
            }
            return false;
        }
        _ => return false,
    };
    let down = matches!(message, WM_LBUTTONDOWN | WM_RBUTTONDOWN);
    let grabbed = GRABBED.load(Ordering::Relaxed);
    if !down {
        if grabbed != button.code() {
            return false;
        }
        GRABBED.store(0, Ordering::Relaxed);
        send(PointerEvent::Release { at });
        return true;
    }
    // The button of the grab pressed again was let go where the hook did
    // not see it: that grab is over.
    if grabbed == button.code() {
        GRABBED.store(0, Ordering::Relaxed);
    } else if grabbed != 0 {
        return false;
    }
    let Some(key) = GrabKey::of_code(GRAB_KEY.load(Ordering::Relaxed)) else {
        return false;
    };
    if !key.is_held() {
        return false;
    }
    let Some(window) = movable_at(at).filter(|&window| grabbable(window)) else {
        return false;
    };
    GRABBED.store(button.code(), Ordering::Relaxed);
    send(PointerEvent::Grab { button, window, at });
    true
}

/// The window of another program under `point` that can be moved: one
/// that is shown and is no part of the shell of Windows. Finding it asks no
/// window anything, as the hook may not wait.
fn movable_at(point: (i32, i32)) -> Option<AppWindowId> {
    let window = window_at(point)?;
    // SAFETY: the calls only read the state and the class of a window, and
    // the buffer outlives the call that writes no more than its length.
    unsafe {
        if !IsWindowVisible(window.hwnd()).as_bool() {
            return None;
        }
        let mut name = [0u16; 64];
        let length = GetClassNameW(window.hwnd(), &mut name).max(0) as usize;
        let class = String::from_utf16_lossy(&name[..length]);
        moves_class(&class).then_some(window)
    }
}

/// Whether a press on `window` with the key held is taken to move or
/// resize it: not when it is minimized, nor when it covers its display
/// without being maximized, as a game or a video shown full screen does,
/// which then gets the press as it came. Telling asks no window anything.
fn grabbable(window: AppWindowId) -> bool {
    let hwnd = window.hwnd();
    let mut whole = RECT::default();
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: the calls only read the state and the place of a window and
    // of its display, into structures that are sized as the calls are
    // told and outlive them; a window that is gone makes them fail.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            return false;
        }
        if IsZoomed(hwnd).as_bool() {
            return true;
        }
        let display = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if GetWindowRect(hwnd, &raw mut whole).is_err()
            || !GetMonitorInfoW(display, &raw mut monitor).as_bool()
        {
            return true;
        }
    }
    let display = monitor.rcMonitor;
    !(whole.left <= display.left
        && whole.top <= display.top
        && whole.right >= display.right
        && whole.bottom >= display.bottom)
}

/// Whether Windows gives the keyboard to the window under the pointer
/// once the pointer rests on it for `delay` milliseconds, and whether it
/// also brings that window above the others (`raises`): its setting
/// "Activate a window by hovering over it with the mouse".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveTracking {
    pub on: bool,
    pub delay: u32,
    pub raises: bool,
}

/// Where the user's own choice of [`ActiveTracking`] is kept.
const DESKTOP_KEY: &str = r"Control Panel\Desktop";

/// The bits of the first byte of `UserPreferencesMask` that say whether
/// Windows tracks the active window, and whether it raises it.
const TRACKING_BIT: u8 = 0x01;
const RAISING_BIT: u8 = 0x40;

/// Sets a setting for the session only: nothing is written to the
/// settings of the user, and no window is sent a message about it, which
/// would wait for every program.
const NOT_WRITTEN: SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS = SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0);

impl ActiveTracking {
    /// What Windows does now.
    pub fn now() -> Option<Self> {
        let mut on = BOOL(0);
        let mut delay = 0u32;
        let mut raises = BOOL(0);
        let read = |action: SYSTEM_PARAMETERS_INFO_ACTION, value: *mut c_void| {
            // SAFETY: `value` points to a value of the kind the action
            // fills, which outlives the call.
            unsafe {
                SystemParametersInfoW(
                    action,
                    0,
                    Some(value),
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                )
            }
            .is_ok()
        };
        let all = read(SPI_GETACTIVEWINDOWTRACKING, (&raw mut on).cast())
            && read(SPI_GETACTIVEWNDTRKTIMEOUT, (&raw mut delay).cast())
            && read(SPI_GETACTIVEWNDTRKZORDER, (&raw mut raises).cast());
        all.then_some(Self {
            on: on.as_bool(),
            delay,
            raises: raises.as_bool(),
        })
    }

    /// What the user chose, as the settings keep it, whatever Windows does
    /// now: a Tessera that ended without giving it back leaves what it set
    /// until the next sign in. Where the settings say nothing, what
    /// Windows does now.
    pub fn chosen() -> Option<Self> {
        let now = Self::now();
        let written = Key::read(HKEY_CURRENT_USER, DESKTOP_KEY, REG_SAM_FLAGS(0)).and_then(|key| {
            let first = *key.bytes("UserPreferencesMask")?.first()?;
            Some(Self {
                on: first & TRACKING_BIT != 0,
                delay: key
                    .number("ActiveWndTrkTimeout")
                    .or(now.map(|now| now.delay))
                    .unwrap_or(0),
                raises: first & RAISING_BIT != 0,
            })
        });
        written.or(now)
    }

    /// Has Windows do this until the user signs out; the settings of the
    /// user stay as they are written. Whether Windows took all of it is
    /// returned.
    pub fn set(self) -> bool {
        let set = |action: SYSTEM_PARAMETERS_INFO_ACTION, value: usize| {
            // The value is given itself in place of a pointer, as Windows
            // asks for these settings.
            let value = std::ptr::without_provenance_mut::<c_void>(value);
            // SAFETY: Windows reads the value, not what it would point to,
            // and nothing is written to the settings of the user.
            unsafe { SystemParametersInfoW(action, 0, Some(value), NOT_WRITTEN) }.is_ok()
        };
        let tracking = set(SPI_SETACTIVEWINDOWTRACKING, usize::from(self.on));
        let delay = set(SPI_SETACTIVEWNDTRKTIMEOUT, self.delay as usize);
        let raising = set(SPI_SETACTIVEWNDTRKZORDER, usize::from(self.raises));
        tracking && delay && raising
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_are_known_by_their_codes() {
        for key in [GrabKey::Alt, GrabKey::Win] {
            assert_eq!(GrabKey::of_code(key.code()), Some(key));
        }
        assert_eq!(GrabKey::of_code(0), None);
        assert_ne!(PointerButton::Left.code(), PointerButton::Right.code());
    }

    #[test]
    fn the_shell_of_windows_is_not_moved() {
        for class in ["XamlExplorerHostIslandWindow", "Shell_TrayWnd", "#32768"] {
            assert!(!moves_class(class));
        }
        assert!(moves_class("Notepad"));
        assert!(moves_class("ApplicationFrameWindow"));
    }
}
