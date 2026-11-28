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
//! The same hook takes, where the configuration asks for it, a double
//! click and a turn of the wheel on a title bar, a turn of the wheel and
//! the middle button with the key held over any window, and a click on a
//! window that is not in front, which then only brings it in front. It
//! takes the presses of the middle and the side buttons that shortcuts are
//! bound to ([`super::MouseButtons`]), and it notes which program the last
//! click went to, for the guard against windows that take the foreground.
//!
//! The pointer is watched with a low-level hook on a thread of its own
//! that does nothing else and pumps its messages, as Windows asks of such
//! a hook. The hook decides from what it can read at once whether to take
//! a press: whether the key is held and which window is under the
//! pointer, which asks no window anything. Only whether a point is on a
//! title bar is asked of the window itself, and only for a second click
//! or a turn of the wheel near the top of a window while an action for
//! the title bar is set: it is given 50 milliseconds, and a window that
//! hangs is not asked. What is to be done goes to the shell on a channel.
//! The thread runs only while the configuration asks for one of these.
//!
//! Windows does not call the hook for presses meant for a program that
//! runs with more rights than Tessera, so the windows of such a program
//! cannot be moved this way.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError, mpsc};
use std::thread;

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SAM_FLAGS};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetDoubleClickTime, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS,
    KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN,
    VK_SHIFT,
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
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId,
    HTCAPTION, IsHungAppWindow, SM_CXDOUBLECLK, SM_CYDOUBLECLK, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    SendMessageTimeoutW, WM_APP, WM_MBUTTONUP, WM_MOUSEWHEEL, WM_NCHITTEST, WM_XBUTTONDOWN,
    WM_XBUTTONUP, WindowFromPoint,
};
use windows::core::BOOL;

use super::AppWindowId;
use super::Modifiers;
use super::mouse_buttons::{self, MouseButtonName};
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
    /// The title bar of `window` was clicked twice; the window got
    /// nothing of the second click.
    TitleDouble { window: AppWindowId },
    /// The wheel was turned, away from the user where `up`, over the
    /// title bar of `window`, which got nothing of it.
    TitleWheel { window: AppWindowId, up: bool },
    /// The wheel was turned with the key held over `window`, which got
    /// nothing of it.
    ModifierWheel { window: AppWindowId, up: bool },
    /// The middle button was pressed at `at` with the key held over
    /// `window`, which got nothing of it.
    ModifierMiddle { window: AppWindowId, at: (i32, i32) },
    /// `window`, which was not in front, was clicked; it got nothing of
    /// the click and is to come in front.
    Activate { window: AppWindowId },
}

/// What the hook takes besides the grab and the middle button, as the
/// window actions ask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HookAsks {
    /// A double click on a title bar.
    pub title_double: bool,
    /// A turn of the wheel over a title bar.
    pub title_wheel: bool,
    /// A turn of the wheel with the key held, over any window.
    pub modifier_wheel: bool,
    /// The middle button with the key held, over any window.
    pub modifier_middle: bool,
    /// A click on a window that is not in front, which only brings it in
    /// front.
    pub inactive_click: bool,
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
/// What else the hook takes, as [`HookAsks`] says.
static TITLE_DOUBLE: AtomicBool = AtomicBool::new(false);
static TITLE_WHEEL: AtomicBool = AtomicBool::new(false);
static MODIFIER_WHEEL: AtomicBool = AtomicBool::new(false);
static MODIFIER_MIDDLE: AtomicBool = AtomicBool::new(false);
static INACTIVE_CLICK: AtomicBool = AtomicBool::new(false);
/// Whether presses of mouse buttons are bound to shortcuts.
static BUTTONS_BOUND: AtomicBool = AtomicBool::new(false);
/// Whether the program the last click went to is noted.
static WATCH_CLICKS: AtomicBool = AtomicBool::new(false);
/// The program the last click went to, and when, in the milliseconds of
/// `GetTickCount`; 0 for none yet.
static LAST_CLICK_PROCESS: AtomicU32 = AtomicU32::new(0);
static LAST_CLICK_TIME: AtomicU32 = AtomicU32::new(0);
/// The buttons whose press was taken, so that their release is taken too,
/// as the bits of [`Released`].
static TAKEN_UPS: AtomicU8 = AtomicU8::new(0);

/// What the thread of the hook is told to do once the hook has returned:
/// press the key no keyboard has, so that a key held for a press that
/// was taken is not taken as pressed by itself.
const BREAK_LONE_KEY: u32 = WM_APP + 1;

/// How near the top of a window a point is asked about, as a title bar
/// there, in logical pixels: as low as the tabs of a browser reach.
const TITLE_BAND: i32 = 80;

/// How long a window is given to say whether a point is on its title bar.
const HIT_TEST_TIMEOUT: u32 = 50;

/// The bits of the buttons whose release is taken.
struct Released;

impl Released {
    const LEFT: u8 = 1;
    const MIDDLE: u8 = 2;
    const BACK: u8 = 4;
    const FORWARD: u8 = 8;
}

/// A press of the left button: when, in the milliseconds of `GetTickCount`,
/// where, and on which window.
type Press = (u32, (i32, i32), isize);

thread_local! {
    /// The last press of the left button on the thread of the hook: when,
    /// where and on which window, to tell a double click.
    static LAST_DOWN: Cell<Option<Press>> = const { Cell::new(None) };
}

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
        refresh()
    }

    /// Takes from now on what `asks` says, besides the grab and the
    /// middle button. Whether Windows took the hook is returned, true when
    /// none is needed.
    pub fn ask(asks: HookAsks) -> bool {
        TITLE_DOUBLE.store(asks.title_double, Ordering::Relaxed);
        TITLE_WHEEL.store(asks.title_wheel, Ordering::Relaxed);
        MODIFIER_WHEEL.store(asks.modifier_wheel, Ordering::Relaxed);
        MODIFIER_MIDDLE.store(asks.modifier_middle, Ordering::Relaxed);
        INACTIVE_CLICK.store(asks.inactive_click, Ordering::Relaxed);
        refresh()
    }

    /// Notes from now on which program each click goes to, or no longer;
    /// see [`Self::last_click`].
    pub fn watch_clicks(on: bool) -> bool {
        WATCH_CLICKS.store(on, Ordering::Relaxed);
        refresh()
    }

    /// The program the last click went to while clicks were watched, and
    /// when, in the milliseconds of `GetTickCount`.
    pub fn last_click() -> Option<(u32, u32)> {
        let process = LAST_CLICK_PROCESS.load(Ordering::Relaxed);
        (process != 0).then(|| (process, LAST_CLICK_TIME.load(Ordering::Relaxed)))
    }
}

/// Has the hook take presses of mouse buttons for shortcuts, or no
/// longer, as [`super::MouseButtons::bind`] asks.
pub(super) fn buttons_bound(on: bool) -> bool {
    BUTTONS_BOUND.store(on, Ordering::Relaxed);
    refresh()
}

/// Sets the hook when anything is asked of it and takes it away when
/// nothing is. Whether Windows took the hook is returned, true when none
/// is needed.
fn refresh() -> bool {
    let asked = [
        &MIDDLE,
        &TITLE_DOUBLE,
        &TITLE_WHEEL,
        &MODIFIER_WHEEL,
        &MODIFIER_MIDDLE,
        &INACTIVE_CLICK,
        &BUTTONS_BOUND,
        &WATCH_CLICKS,
    ];
    let wanted = GRAB_KEY.load(Ordering::Relaxed) != 0
        || asked.iter().any(|flag| flag.load(Ordering::Relaxed));
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
            TAKEN_UPS.store(0, Ordering::Relaxed);
            true
        }
        (None, true) => match start() {
            Ok(id) => {
                *thread = Some(id);
                true
            }
            Err(error) => {
                report!(
                    "the pointer is not watched, so its buttons do nothing of Tessera's: {error}"
                );
                false
            }
        },
    }
}

impl MouseHook {
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
    // the loop only waits for the hook to be called, for what the hook
    // left to do once it returned, and for the quit.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            if message.message == BREAK_LONE_KEY {
                MouseHook::break_lone_key();
            }
        }
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
        if event.flags & LLMHF_INJECTED == 0 && taken(wparam.0 as u32, event) {
            return LRESULT(1);
        }
    }
    // SAFETY: the arguments are passed on as they came.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Sends `event` to the shell.
fn send(event: PointerEvent) {
    // The channel has no bound, and nobody listening is no error.
    let _ = channel().0.try_send(event);
}

/// Whether the press, release or turn of the wheel `message`, described by
/// `event`, is taken from the window under the pointer; what it means is
/// sent on.
fn taken(message: u32, event: &MSLLHOOKSTRUCT) -> bool {
    let POINT { x, y } = event.pt;
    let at = (x, y);
    // The higher word of `mouseData` says which side button, or how far
    // the wheel turned.
    let high = (event.mouseData >> 16) as u16;
    // A release of a press that was taken is taken too.
    if let Some(bit) = released_bit(message, high)
        && TAKEN_UPS.fetch_and(!bit, Ordering::Relaxed) & bit != 0
    {
        return true;
    }
    if matches!(
        message,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    ) {
        note_click(at, event.time);
    }
    match message {
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP => {
            if grabbed(message, at) {
                return true;
            }
            let more = message == WM_LBUTTONDOWN
                && (double_on_title_bar(at, event.time) || inactive_click(at));
            more && take_release(Released::LEFT)
        }
        WM_MBUTTONDOWN => middle_pressed(at),
        WM_XBUTTONDOWN => side_pressed(high),
        WM_MOUSEWHEEL => wheel_turned(at, high as i16 > 0),
        _ => false,
    }
}

/// The bit of [`Released`] of the release `message`; `side` says which
/// side button.
fn released_bit(message: u32, side: u16) -> Option<u8> {
    match message {
        WM_LBUTTONUP => Some(Released::LEFT),
        WM_MBUTTONUP => Some(Released::MIDDLE),
        WM_XBUTTONUP if side == 1 => Some(Released::BACK),
        WM_XBUTTONUP if side == 2 => Some(Released::FORWARD),
        _ => None,
    }
}

/// Has the release of the button of `bit` taken too; true, as the press
/// is taken.
fn take_release(bit: u8) -> bool {
    TAKEN_UPS.fetch_or(bit, Ordering::Relaxed);
    true
}

/// The keys held now.
fn held_now() -> Modifiers {
    let held = |key: VIRTUAL_KEY| {
        // SAFETY: the call takes a key and only reads its state.
        unsafe { GetAsyncKeyState(i32::from(key.0)) < 0 }
    };
    Modifiers {
        alt: held(VK_MENU),
        control: held(VK_CONTROL),
        shift: held(VK_SHIFT),
        windows: held(VK_LWIN) || held(VK_RWIN),
    }
}

/// Has the thread of the hook press the key no keyboard has once the hook
/// returned, if a key held now would open a menu when let go alone: Alt
/// the menu of the window in front, the Windows key the start menu.
fn break_lone_key_after(held: Modifiers) {
    if !held.alt && !held.windows {
        return;
    }
    // SAFETY: the call only reads which thread this is, and posting to it
    // fails without side effects once it is gone.
    unsafe {
        let _ = PostThreadMessageW(GetCurrentThreadId(), BREAK_LONE_KEY, WPARAM(0), LPARAM(0));
    }
}

/// Whether the key that takes presses is held now.
fn grab_key_held() -> bool {
    GrabKey::of_code(GRAB_KEY.load(Ordering::Relaxed)).is_some_and(GrabKey::is_held)
}

/// Notes the program the press at `at`, at `time`, went to, while clicks
/// are watched. Telling asks no window anything.
fn note_click(at: (i32, i32), time: u32) {
    if !WATCH_CLICKS.load(Ordering::Relaxed) {
        return;
    }
    let mut process = 0u32;
    // SAFETY: the calls only read which window is at a point and whose it
    // is.
    unsafe {
        let window = WindowFromPoint(POINT { x: at.0, y: at.1 });
        if !window.is_invalid() {
            GetWindowThreadProcessId(window, Some(&raw mut process));
        }
    }
    LAST_CLICK_PROCESS.store(process, Ordering::Relaxed);
    LAST_CLICK_TIME.store(time, Ordering::Relaxed);
}

/// Whether the middle button pressed at `at` is taken: when a shortcut is
/// bound to it with the keys held now, or when the key is held and an
/// action of the middle button is set. Otherwise the shell hears of it
/// where it asked to, and the window gets it.
fn middle_pressed(at: (i32, i32)) -> bool {
    let held = held_now();
    if BUTTONS_BOUND.load(Ordering::Relaxed)
        && mouse_buttons::pressed(MouseButtonName::Middle, held)
    {
        break_lone_key_after(held);
        return take_release(Released::MIDDLE);
    }
    if MODIFIER_MIDDLE.load(Ordering::Relaxed)
        && grab_key_held()
        && let Some(window) = movable_at(at)
    {
        send(PointerEvent::ModifierMiddle { window, at });
        break_lone_key_after(held);
        return take_release(Released::MIDDLE);
    }
    if MIDDLE.load(Ordering::Relaxed)
        && let Some(window) = movable_at(at)
    {
        send(PointerEvent::Middle { window, at });
    }
    false
}

/// Whether the side button `side` (1 for back, 2 for forward) is taken:
/// only when a shortcut is bound to it with the keys held now.
fn side_pressed(side: u16) -> bool {
    let (button, bit) = match side {
        1 => (MouseButtonName::Back, Released::BACK),
        2 => (MouseButtonName::Forward, Released::FORWARD),
        _ => return false,
    };
    let held = held_now();
    if !BUTTONS_BOUND.load(Ordering::Relaxed) || !mouse_buttons::pressed(button, held) {
        return false;
    }
    break_lone_key_after(held);
    take_release(bit)
}

/// Whether a turn of the wheel at `at`, away from the user where `up`, is
/// taken: with the key held where an action of the wheel with it is set,
/// or over a title bar where an action of the wheel there is set.
fn wheel_turned(at: (i32, i32), up: bool) -> bool {
    if MODIFIER_WHEEL.load(Ordering::Relaxed)
        && grab_key_held()
        && let Some(window) = movable_at(at)
    {
        send(PointerEvent::ModifierWheel { window, up });
        break_lone_key_after(held_now());
        return true;
    }
    if TITLE_WHEEL.load(Ordering::Relaxed)
        && let Some(window) = movable_at(at)
        && on_title_bar(window, at)
    {
        send(PointerEvent::TitleWheel { window, up });
        return true;
    }
    false
}

/// Whether the press of the left button at `at`, at `time`, is the second
/// of a double click on a title bar, while an action for that is set.
fn double_on_title_bar(at: (i32, i32), time: u32) -> bool {
    if !TITLE_DOUBLE.load(Ordering::Relaxed) {
        return false;
    }
    let Some(window) = movable_at(at) else {
        LAST_DOWN.set(None);
        return false;
    };
    let before = LAST_DOWN.replace(Some((time, at, window.raw())));
    // SAFETY: the calls only read settings of the user.
    let (most_time, width, height) = unsafe {
        (
            GetDoubleClickTime(),
            GetSystemMetrics(SM_CXDOUBLECLK),
            GetSystemMetrics(SM_CYDOUBLECLK),
        )
    };
    let double = before.is_some_and(|(then, there, on)| {
        on == window.raw()
            && time.wrapping_sub(then) <= most_time
            && is_double_click((there, at), (width, height))
    });
    if !double {
        return false;
    }
    LAST_DOWN.set(None);
    if !on_title_bar(window, at) {
        return false;
    }
    send(PointerEvent::TitleDouble { window });
    true
}

/// Whether two presses at `points` are near enough for a double click,
/// within a rectangle of `size` around the first, as Windows tells them.
fn is_double_click(points: ((i32, i32), (i32, i32)), size: (i32, i32)) -> bool {
    let ((x, y), (other_x, other_y)) = points;
    (x - other_x).abs() <= size.0 / 2 && (y - other_y).abs() <= size.1 / 2
}

/// Whether the press of the left button at `at` lands on a window that
/// is not in front, while such a press is only to bring it in front.
fn inactive_click(at: (i32, i32)) -> bool {
    if !INACTIVE_CLICK.load(Ordering::Relaxed) {
        return false;
    }
    let Some(window) = movable_at(at) else {
        return false;
    };
    // SAFETY: the calls only read which window is in front and its top
    // window.
    let front = unsafe {
        let front = GetForegroundWindow();
        let root = GetAncestor(front, GA_ROOT);
        if root.is_invalid() { front } else { root }
    };
    if front.0 as isize == window.raw() {
        return false;
    }
    send(PointerEvent::Activate { window });
    true
}

/// Whether `at` is on the title bar of `window`, away from its buttons, as
/// the window says: asked only near its top, of a window that does not
/// hang, and given [`HIT_TEST_TIMEOUT`] to answer.
fn on_title_bar(window: AppWindowId, at: (i32, i32)) -> bool {
    let hwnd = window.hwnd();
    let mut whole = RECT::default();
    // SAFETY: the calls read the state, the place and the density of a
    // window into a structure that outlives them; a window that is gone
    // makes them fail.
    let near_top = unsafe {
        if IsHungAppWindow(hwnd).as_bool() || GetWindowRect(hwnd, &raw mut whole).is_err() {
            return false;
        }
        let dpi = match GetDpiForWindow(hwnd) {
            0 => 96,
            dpi => dpi as i32,
        };
        at.1 - whole.top <= TITLE_BAND * dpi / 96
    };
    if !near_top {
        return false;
    }
    let packed = ((at.1 & 0xffff) << 16) | (at.0 & 0xffff);
    let mut answer = 0usize;
    // SAFETY: `answer` outlives the call, which gives up on a window that
    // hangs or does not answer in time, and processes nothing else of this
    // thread meanwhile.
    let answered = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_NCHITTEST,
            WPARAM(0),
            LPARAM(packed as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            HIT_TEST_TIMEOUT,
            Some(&raw mut answer),
        )
    };
    answered.0 != 0 && answer as u32 == HTCAPTION
}

/// Whether the grab takes the press or release `message` at `at`; what it
/// means is sent on.
fn grabbed(message: u32, at: (i32, i32)) -> bool {
    let button = match message {
        WM_LBUTTONDOWN | WM_LBUTTONUP => PointerButton::Left,
        WM_RBUTTONDOWN | WM_RBUTTONUP => PointerButton::Right,
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
    fn two_presses_near_each_other_are_a_double_click() {
        assert!(is_double_click(((100, 100), (102, 98)), (4, 4)));
        assert!(!is_double_click(((100, 100), (104, 100)), (4, 4)));
        assert_eq!(released_bit(WM_XBUTTONUP, 1), Some(Released::BACK));
        assert_eq!(released_bit(WM_XBUTTONUP, 2), Some(Released::FORWARD));
        assert_eq!(released_bit(WM_XBUTTONUP, 3), None);
        assert_eq!(released_bit(WM_RBUTTONUP, 0), None);
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
