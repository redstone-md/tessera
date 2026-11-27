// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What Windows asks of the shell of a session, answered on a thread of
//! its own: the desktop window, the shell hook, the end of the session.
//!
//! [`ShellHost::start`] makes a thread with a message loop that is never
//! held up by GPUI, nor by the work of the [`ShellThread`](super::ShellThread),
//! and makes on it:
//!
//! - In both modes, a hidden top-level tool window (a message-only one is
//!   not given the messages of the shell hook) registered with
//!   `RegisterShellHookWindow`. What the hook tells (a window made,
//!   destroyed, activated, flashing, replaced, moved to another display)
//!   comes out as [`ShellEvent`]s, which any part of Tessera receives
//!   with [`ShellHost::events`]. The same window hears the locking of the
//!   session, a change of the environment variables of the user (which it
//!   takes into the environment of the process, so that programs started
//!   afterwards get them), and the end of the session.
//! - Only as the shell of the session: a desktop window of the class
//!   `Progman`, titled "Program Manager", over the whole virtual screen,
//!   with the children `SHELLDLL_DefView` (on top, drawing nothing) and
//!   `WorkerW` (at the bottom, drawing the wallpaper of Windows with
//!   `PaintDesktop`), which is how Explorer of Windows 11 24H2 lays its
//!   desktop out; the backdrops of Tessera (backdrop.rs) find it and put
//!   themselves between the two. It answers the message 0x052C, which has
//!   Explorer make its `WorkerW`, by making one again where it is
//!   missing. Windows is told it is the shell window (`SetShellWindowEx`,
//!   `SetProgmanWindow`), so that it stays below every other window, and
//!   the hidden window is told to be the task manager of the shell
//!   (`SetTaskmanWindow`), which has Windows send it Ctrl+Esc, the
//!   Windows key without a Start menu, `HSHELL_APPCOMMAND`,
//!   `HSHELL_ENDTASK` and `HSHELL_GETMINRECT`. These exports of user32
//!   are not documented; one that is missing is reported and left out.
//!   Minimized windows are also told to go out of sight instead of lying
//!   at the bottom of the screen, as Explorer has them.
//!
//! Alt+F4 on the desktop closes nothing: it is told as
//! [`ShellEvent::CloseRequested`], for a dialog that asks how to end the
//! session. Messages to the hidden window in the ranges other programs
//! and parts of Windows use (`WM_USER` to 0x7FFF, and registered ones)
//! are given to the handlers of [`ShellHost::intercept`] first, which may
//! answer them on this thread, and come out as [`ShellEvent::Raw`], so a
//! feature that listens to the task manager of the shell, as the
//! progress of `ITaskbarList` may arrive there, needs no change here.
//!
//! The backdrops made by backdrop.rs are children of the desktop window
//! made on other threads, which attaches the input of their threads to
//! this one. The thread therefore only ever waits at the end of the
//! session, and handles messages sent to it while it does; work that
//! should rather run where the desktop window lives can be handed over
//! with [`ShellHost::on_host_thread`].

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{
    CloseHandle, FARPROC, HANDLE, HWND, LPARAM, LRESULT, RECT, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, HDC, PAINTSTRUCT, PaintDesktop, RDW_ALLCHILDREN, RDW_ERASE,
    RDW_INVALIDATE, RedrawWindow,
};
use windows::Win32::Security::{TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY};
use windows::Win32::System::Environment::{
    CreateEnvironmentBlock, DestroyEnvironmentBlock, SetEnvironmentVariableW,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_READWRITE,
    VirtualQuery,
};
use windows::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification,
};
use windows::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, GetCurrentProcess, OpenEventW, OpenProcessToken, SetEvent,
    SetProcessShutdownParameters,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ARW_HIDE, CreateWindowExW, DefWindowProcW, DispatchMessageW, FAPPCOMMAND_MASK, FindWindowExW,
    GetMessageW, GetShellWindow, GetSystemMetrics, HWND_BOTTOM, IDC_ARROW, InSendMessage,
    LoadCursorW, MINIMIZEDMETRICS, MINIMIZEDMETRICS_ARRANGE, MSG,
    MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS, MoveWindow, MsgWaitForMultipleObjectsEx, PM_NOREMOVE,
    PM_QS_SENDMESSAGE, PeekMessageW, PostMessageW, QS_SENDMESSAGE, RegisterClassW,
    RegisterShellHookWindow, RegisterWindowMessageW, SC_CLOSE, SC_TASKLIST, SHELLHOOKINFO,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SPI_GETMINIMIZEDMETRICS, SPI_SETDESKWALLPAPER, SPI_SETMINIMIZEDMETRICS, SPI_SETWORKAREA,
    SW_SHOWNA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetTimer, SetWindowPos, ShowWindow, SystemParametersInfoW,
    TranslateMessage, WINDOW_EX_STYLE, WINDOWPOS, WM_APP, WM_CLOSE, WM_COPYDATA, WM_DISPLAYCHANGE,
    WM_ENDSESSION, WM_ERASEBKGND, WM_HOTKEY, WM_NCDESTROY, WM_PAINT, WM_QUERYENDSESSION,
    WM_SETTINGCHANGE, WM_SYSCOMMAND, WM_TIMER, WM_USER, WM_WINDOWPOSCHANGING, WM_WTSSESSION_CHANGE,
    WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
    WS_POPUP, WS_VISIBLE, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
};
use windows::core::{BOOL, HSTRING, PCSTR, PCWSTR, s, w};

use super::{AppCommand, AppWindowId, Launch, ScreenRect, SystemWallpaper, Taskbar};

/// The codes of the shell hook, as `wParam` of the `SHELLHOOK` message.
const HSHELL_WINDOWCREATED: u32 = 1;
const HSHELL_WINDOWDESTROYED: u32 = 2;
const HSHELL_WINDOWACTIVATED: u32 = 4;
const HSHELL_GETMINRECT: u32 = 5;
const HSHELL_REDRAW: u32 = 6;
const HSHELL_TASKMAN: u32 = 7;
const HSHELL_ENDTASK: u32 = 10;
const HSHELL_APPCOMMAND: u32 = 12;
const HSHELL_WINDOWREPLACED: u32 = 13;
const HSHELL_WINDOWREPLACING: u32 = 14;
const HSHELL_MONITORCHANGED: u32 = 16;
/// The bit the hook adds to a code: to an activation for a window that
/// covers its display, to a redraw for a window that flashes.
const HSHELL_HIGHBIT: u32 = 0x8000;
const HSHELL_RUDEAPPACTIVATED: u32 = HSHELL_WINDOWACTIVATED | HSHELL_HIGHBIT;
const HSHELL_FLASH: u32 = HSHELL_REDRAW | HSHELL_HIGHBIT;

/// How soon after a request for the Start menu another is the same.
const START_TWICE: Duration = Duration::from_millis(300);

/// The message that has the desktop make its `WorkerW`.
const RAISE_DESKTOP: u32 = 0x052C;

/// The message that tells the hidden window that work is waiting.
const WORK_WAITING: u32 = WM_APP + 0x60;

/// The timer that ends the process when GPUI did not quit in time after
/// a key of recovery.
const FORCED_EXIT: usize = 1;

/// How long GPUI gets to quit after a key of recovery.
const QUIT_TIME: u32 = 10_000;

/// The keys of recovery, as numbers of `RegisterHotKey`: Ctrl+Win+Alt+E
/// hands the session to Explorer, Ctrl+Win+Alt+R starts Tessera again.
const KEY_EXPLORER: i32 = 0x7E01;
const KEY_RESTART: i32 = 0x7E02;

/// The level `SetProcessShutdownParameters` is given: the lowest that
/// applications may have, so that the shell is asked to end after them.
const SHUTDOWN_LEVEL: u32 = 0x100;

/// How long the start waits, as the shell, for the host thread to have
/// made the desktop window, which the wallpaper looks for right after.
/// Making three windows takes no waiting, so this is only a bound.
const DESKTOP_TIME: Duration = Duration::from_secs(2);

/// How long the end of the session waits for Tessera to have shown the
/// windows it hid and given back the space it held. Windows tells the
/// user of programs that keep the session longer than about five
/// seconds.
const ENDING_TIME: Duration = Duration::from_secs(4);

/// The events a shell signals once it is ready; programs and Windows
/// wait for one of them. Each that does not exist is left out.
const READY_EVENTS: [PCWSTR; 4] = [
    w!("msgina: ShellReadyEvent"),
    w!(r"Global\msgina: ShellReadyEvent"),
    w!("ShellDesktopSwitchEvent"),
    w!("ShellReadyEvent"),
];

/// Something the shell of the session was told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    /// A top-level window without owner was made.
    Created(AppWindowId),
    /// A top-level window without owner was destroyed.
    Destroyed(AppWindowId),
    /// A window came in front; `fullscreen` where it covers its display.
    Activated {
        window: AppWindowId,
        fullscreen: bool,
    },
    /// The title of a window changed.
    Redraw(AppWindowId),
    /// A window flashes to ask for attention.
    Flash(AppWindowId),
    /// A window moved to another display.
    MonitorChanged(AppWindowId),
    /// A window was replaced by another of the same program, as a hung
    /// window is by its ghost; `new` where it was told.
    Replaced {
        old: AppWindowId,
        new: Option<AppWindowId>,
    },
    /// The user asked for a window to be closed, as Task Manager does.
    EndTask(AppWindowId),
    /// A command of `WM_APPCOMMAND` that the window in front left, as a
    /// key of the volume or of media.
    AppCommand(u16),
    /// A key of the volume or of media that the shell took, for the user
    /// interface to do; told only where Tessera is the shell and
    /// `[shell] media_keys` is on.
    Key(AppCommand),
    /// Ctrl+Esc or the Windows key asked for the Start menu.
    StartRequested,
    /// Alt+F4 on the desktop asked how to end the session.
    CloseRequested,
    /// A key of recovery asked Tessera to quit at once.
    QuitRequested,
    /// The session was locked.
    Locked,
    /// The session was unlocked.
    Unlocked,
    /// The session ends; Tessera is to show the windows it hid and give
    /// back the space it held, and quit. See
    /// [`ShellHost::session_restored`].
    SessionEnding,
    /// The environment variables of the user changed and are in the
    /// environment of the process now.
    EnvironmentChanged,
    /// A display came, went or changed its resolution.
    DisplaysChanged,
    /// A message to the hidden window that is not one of the above, in
    /// the range `WM_USER` to 0x7FFF or a registered one.
    Raw { message: u32, w: usize, l: isize },
}

/// How the host behaves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostOptions {
    /// Whether Tessera is the shell of the session: only then is the
    /// desktop window made and are the tasks of the shell taken.
    pub shell: bool,
    /// Whether Ctrl+Win+Alt+E and Ctrl+Win+Alt+R are held, in shell
    /// mode.
    pub emergency_keys: bool,
    /// Whether the keys of the volume and of media are answered, in shell
    /// mode; beside Explorer, Explorer answers them.
    pub media_keys: bool,
}

/// A piece of work for the host thread.
type Work = Box<dyn FnOnce() + Send>;

/// A handler of [`ShellHost::intercept`].
type Handler = Arc<Mutex<dyn FnMut(u32, usize, isize) -> Option<isize> + Send>>;

/// The hidden window of the shell hook, as a number; zero until it is made.
static HOOK: AtomicIsize = AtomicIsize::new(0);
/// The desktop window, as a number; zero where there is none.
static DESKTOP: AtomicIsize = AtomicIsize::new(0);
/// Its child `SHELLDLL_DefView`, which the icons of the desktop are drawn
/// in, as a number; zero where there is none.
static DEF_VIEW: AtomicIsize = AtomicIsize::new(0);
/// The number of the `SHELLHOOK` message.
static SHELLHOOK: AtomicU32 = AtomicU32::new(0);
static SHELL: AtomicBool = AtomicBool::new(false);
static EMERGENCY_KEYS: AtomicBool = AtomicBool::new(false);
static MEDIA_KEYS: AtomicBool = AtomicBool::new(false);
/// Whether the session ends.
static ENDING: AtomicBool = AtomicBool::new(false);
/// The event set once Tessera has restored what it changed, as a number.
static RESTORED: AtomicIsize = AtomicIsize::new(0);
/// What the host thread restores by itself when the session ends; see
/// [`ShellHost::on_session_end`].
static SESSION_END: OnceLock<fn()> = OnceLock::new();
static SUBSCRIBERS: Mutex<Vec<Sender<ShellEvent>>> = Mutex::new(Vec::new());
static WORK: Mutex<Vec<Work>> = Mutex::new(Vec::new());
static HANDLERS: Mutex<Vec<(u64, Handler)>> = Mutex::new(Vec::new());
/// Where the dock shows each window, for a minimize to fly to.
static MIN_RECTS: Mutex<Option<HashMap<isize, ScreenRect>>> = Mutex::new(None);
/// The variables of the user, as last taken into the environment.
static ENVIRONMENT: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// Whether the window that replaces another was told; see
/// [`HSHELL_WINDOWREPLACING`].
static REPLACING: AtomicIsize = AtomicIsize::new(0);

/// The thread that answers what Windows asks of the shell.
pub struct ShellHost;

impl ShellHost {
    /// Starts the host thread, once. As the shell, it returns once the
    /// thread has made the desktop window (or failed to), at most after
    /// [`DESKTOP_TIME`]; beside Explorer, at once.
    pub fn start(options: HostOptions) {
        static STARTED: AtomicBool = AtomicBool::new(false);
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        SHELL.store(options.shell, Ordering::Relaxed);
        MEDIA_KEYS.store(options.media_keys, Ordering::Relaxed);
        // The host thread holds the keys once it has its window.
        EMERGENCY_KEYS.store(options.emergency_keys, Ordering::Relaxed);
        // SAFETY: the call changes when the process is asked to end; the
        // event is made without a name or a security descriptor and kept
        // for the life of the process.
        unsafe {
            if let Err(error) = SetProcessShutdownParameters(SHUTDOWN_LEVEL, 0) {
                report!("Tessera may be asked to end before the applications: {error}");
            }
            match CreateEventW(None, true, false, PCWSTR::null()) {
                Ok(event) => RESTORED.store(event.0 as isize, Ordering::SeqCst),
                Err(error) => report!("the end of the session will not wait for Tessera: {error}"),
            }
        }
        let (desktop_made, desktop) = mpsc::sync_channel(1);
        let started = thread::Builder::new()
            .name("shell host".to_owned())
            .spawn(move || host(options.shell, desktop_made));
        match started {
            Err(error) => report!("the thread of the shell does not start: {error}"),
            Ok(_) if options.shell => {
                if desktop.recv_timeout(DESKTOP_TIME).is_err() {
                    report!(
                        "the desktop window was not made in time; the wallpaper may not find it"
                    );
                }
            }
            Ok(_) => {}
        }
    }

    /// Changes what the host does, as the configuration says now. Whether
    /// Tessera is the shell stays as it was started.
    pub fn configure(options: HostOptions) {
        MEDIA_KEYS.store(options.media_keys, Ordering::Relaxed);
        let before = EMERGENCY_KEYS.swap(options.emergency_keys, Ordering::Relaxed);
        if before != options.emergency_keys && Self::is_shell() {
            Self::on_host_thread(hold_emergency_keys);
        }
    }

    /// What the shell is told from now on: every call gives a receiver of
    /// its own that gets each event, until it is dropped. Await it from a
    /// task of GPUI with `recv().await`.
    pub fn events() -> Receiver<ShellEvent> {
        let (sender, receiver) = async_channel::unbounded();
        locked(&SUBSCRIBERS).push(sender);
        receiver
    }

    /// Has `handler` see the messages of the hidden window in the range
    /// `WM_USER` to 0x7FFF, the registered ones and `WM_COPYDATA` first,
    /// with their `wParam` and `lParam` as numbers, until the returned
    /// [`HostInterception`] is dropped. An answer of `Some` is what the
    /// window answers, and the message is not told as
    /// [`ShellEvent::Raw`]. It runs on the host thread and must not wait;
    /// the newest handler is asked first, and one that panics is
    /// reported and removed.
    #[allow(
        dead_code,
        reason = "for the features that answer the task manager of the shell"
    )]
    pub fn intercept(
        handler: impl FnMut(u32, usize, isize) -> Option<isize> + Send + 'static,
    ) -> HostInterception {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let handler: Handler = Arc::new(Mutex::new(handler));
        locked(&HANDLERS).push((number, handler));
        HostInterception(number)
    }

    /// Where the dock shows `window`, for the animation of minimizing it
    /// that Windows asks the shell for (`HSHELL_GETMINRECT`); `None`
    /// forgets it.
    pub fn set_min_rect(window: AppWindowId, rect: Option<ScreenRect>) {
        let mut rects = locked(&MIN_RECTS);
        let rects = rects.get_or_insert_with(HashMap::new);
        match rect {
            Some(rect) => rects.insert(window.raw(), rect),
            None => rects.remove(&window.raw()),
        };
    }

    /// The desktop window Tessera made as the shell, `Progman`; `None`
    /// beside Explorer or before it is made.
    pub fn desktop_window() -> Option<AppWindowId> {
        let window = DESKTOP.load(Ordering::SeqCst);
        (window != 0).then(|| AppWindowId::from_raw(window))
    }

    /// The child `SHELLDLL_DefView` of [`Self::desktop_window`], which
    /// draws nothing and holds the icons of the desktop; `None` beside
    /// Explorer or before it is made.
    pub fn def_view_window() -> Option<AppWindowId> {
        let window = DEF_VIEW.load(Ordering::SeqCst);
        (window != 0 && DESKTOP.load(Ordering::SeqCst) != 0).then(|| AppWindowId::from_raw(window))
    }

    /// Does `work` on the host thread, where the desktop window lives,
    /// after what was handed over before, and returns at once. Work
    /// handed over before the thread is ready waits for it. It must not
    /// wait: the thread answers Windows for the whole session. A panic in
    /// it is reported.
    pub fn on_host_thread(work: impl FnOnce() + Send + 'static) {
        locked(&WORK).push(Box::new(work));
        let hook = HOOK.load(Ordering::SeqCst);
        if hook == 0 {
            return;
        }
        // SAFETY: posting to a window that has been destroyed fails
        // without side effects.
        let _ = unsafe { PostMessageW(Some(handle(hook)), WORK_WAITING, WPARAM(0), LPARAM(0)) };
    }

    /// Tells Windows and the programs that wait for it that the shell is
    /// ready, as Explorer does once its taskbar shows. Only the shell of
    /// the session does.
    pub fn signal_ready() {
        if !Self::is_shell() {
            return;
        }
        Self::on_host_thread(|| {
            for name in READY_EVENTS {
                // SAFETY: an event that is opened is set and closed again;
                // one that does not exist is left out.
                unsafe {
                    if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, name) {
                        let _ = SetEvent(event);
                        let _ = CloseHandle(event);
                    }
                }
            }
        });
    }

    /// Tells the end of the session that Tessera has shown the windows it
    /// hid and given back the space it held, so that it need not wait
    /// longer.
    pub fn session_restored() {
        let event = RESTORED.load(Ordering::SeqCst);
        if event != 0 {
            // SAFETY: the event is kept for the life of the process.
            let _ = unsafe { SetEvent(HANDLE(event as *mut c_void)) };
        }
    }

    /// Has the host thread run `restore` itself when the session ends,
    /// before it waits for GPUI: what must be put back even when the
    /// thread of GPUI does not answer, such as the windows the
    /// workspaces hid. It runs on the host thread, so it must not need
    /// GPUI nor wait. Only the first call counts.
    pub fn on_session_end(restore: fn()) {
        let _ = SESSION_END.set(restore);
    }

    /// Whether the session ends.
    pub fn session_ending() -> bool {
        ENDING.load(Ordering::SeqCst)
    }

    /// Whether the host was started as the shell of the session.
    fn is_shell() -> bool {
        SHELL.load(Ordering::Relaxed)
    }
}

/// While it exists, a handler sees the messages of the hidden window;
/// see [`ShellHost::intercept`].
#[must_use = "the handler is removed when this is dropped"]
pub struct HostInterception(u64);

impl Drop for HostInterception {
    fn drop(&mut self) {
        remove_handler(self.0);
    }
}

fn remove_handler(number: u64) {
    locked(&HANDLERS).retain(|(other, _)| *other != number);
}

/// The value a mutex guards, also after a panic while it was held.
fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The window a number names that was a handle.
fn handle(window: isize) -> HWND {
    HWND(window as *mut c_void)
}

/// Gives `event` to every receiver there still is.
fn publish(event: ShellEvent) {
    locked(&SUBSCRIBERS).retain(|subscriber| subscriber.try_send(event.clone()).is_ok());
}

/// The body of the host thread; `desktop_made` is told once the desktop
/// window has been made, or could not be.
fn host(shell: bool, desktop_made: mpsc::SyncSender<()>) {
    let hook = match create_hook_window() {
        Ok(hook) => hook,
        Err(error) => {
            report!("Tessera hears nothing of the shell of Windows: {error:#}");
            return;
        }
    };
    // SAFETY: the window belongs to this thread; the name is a literal.
    unsafe {
        SHELLHOOK.store(RegisterWindowMessageW(w!("SHELLHOOK")), Ordering::SeqCst);
        if !RegisterShellHookWindow(hook).as_bool() {
            report!("Tessera is not told of windows by the shell hook of Windows");
        }
        if let Err(error) = WTSRegisterSessionNotification(hook, NOTIFY_FOR_THIS_SESSION) {
            report!("Tessera is not told when the session is locked: {error}");
        }
    }
    *locked(&ENVIRONMENT) = user_environment().unwrap_or_default();
    if shell {
        match create_desktop() {
            Ok((desktop, icons)) => {
                DESKTOP.store(desktop.0 as isize, Ordering::SeqCst);
                DEF_VIEW.store(icons.0 as isize, Ordering::SeqCst);
                // Its wallpaper window draws the wallpaper of Windows with
                // `PaintDesktop`, so Tessera need not draw it again.
                SystemWallpaper::set_painted_by_own_desktop(true);
                become_desktop(desktop, icons);
            }
            Err(error) => report!("the desktop window of the shell was not made: {error:#}"),
        }
        let _ = desktop_made.send(());
        become_task_manager(hook);
        hide_minimized_windows();
    }
    HOOK.store(hook.0 as isize, Ordering::SeqCst);
    if shell {
        hold_emergency_keys();
    }
    work_waiting();
    let mut message = MSG::default();
    // SAFETY: `message` outlives the calls that fill and pass it on; the
    // window procedures are called from inside them.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

/// Registers the class `name` with the procedure `respond`; a class that
/// is there already is as good.
fn register_class(
    name: PCWSTR,
    respond: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
) -> Result<()> {
    // SAFETY: the class structure and the name outlive the call.
    unsafe {
        let module = GetModuleHandleW(None).context("the program has no module")?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(respond),
            hInstance: module.into(),
            lpszClassName: name,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&raw const class);
    }
    Ok(())
}

fn create_hook_window() -> Result<HWND> {
    let class = w!("Tessera.ShellHook");
    register_class(class, respond_hook)?;
    // SAFETY: the class is registered and the names are literals.
    unsafe {
        let module = GetModuleHandleW(None).context("the program has no module")?;
        // A top-level window, never shown: a message-only window is not
        // given the messages of the shell hook.
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class,
            w!("Tessera shell hook"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(module.into()),
            None,
        )
        .context("the window of the shell hook could not be made")
    }
}

/// The virtual screen: all displays.
fn virtual_screen() -> ScreenRect {
    // SAFETY: the calls only read metrics of the system.
    unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        ScreenRect {
            left,
            top,
            right: left + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: top + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// Makes the desktop window with its two children, shown at the bottom
/// of the order of windows; gives it and its child for the icons.
fn create_desktop() -> Result<(HWND, HWND)> {
    register_class(w!("Progman"), respond_desktop)?;
    register_class(w!("WorkerW"), respond_wallpaper)?;
    register_class(w!("SHELLDLL_DefView"), respond_icons)?;
    let frame = virtual_screen();
    let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
    // SAFETY: the classes are registered, the names are literals and the
    // windows belong to this thread.
    unsafe {
        let module = GetModuleHandleW(None).context("the program has no module")?;
        let desktop = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("Progman"),
            w!("Program Manager"),
            WS_POPUP | WS_CLIPCHILDREN,
            frame.left,
            frame.top,
            width,
            height,
            None,
            None,
            Some(module.into()),
            None,
        )
        .context("the desktop window could not be made")?;
        // A child is put at the top of the order of its siblings, so the
        // wallpaper is made first and stays below.
        create_child(desktop, w!("WorkerW"), WINDOW_EX_STYLE(0))?;
        let icons = create_child(desktop, w!("SHELLDLL_DefView"), WS_EX_TRANSPARENT)?;
        let _ = ShowWindow(desktop, SW_SHOWNA);
        let _ = SetWindowPos(
            desktop,
            Some(HWND_BOTTOM),
            frame.left,
            frame.top,
            width,
            height,
            SWP_NOACTIVATE,
        );
        Ok((desktop, icons))
    }
}

/// Makes a child of the class `class` that fills the desktop window.
fn create_child(desktop: HWND, class: PCWSTR, style: WINDOW_EX_STYLE) -> Result<HWND> {
    let frame = virtual_screen();
    // SAFETY: the class is registered and the parent belongs to this
    // thread.
    unsafe {
        let module = GetModuleHandleW(None).context("the program has no module")?;
        CreateWindowExW(
            style | WS_EX_NOACTIVATE,
            class,
            w!(""),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            frame.right - frame.left,
            frame.bottom - frame.top,
            Some(desktop),
            None,
            Some(module.into()),
            None,
        )
        .context("a part of the desktop window could not be made")
    }
}

/// A function as `GetProcAddress` gives it, before it is given its type.
type Export = unsafe extern "system" fn() -> isize;

/// A function of user32 that Windows exports without documenting it.
fn user32(name: PCSTR) -> FARPROC {
    // SAFETY: user32 is loaded in every program with windows; the names
    // are NUL-terminated literals.
    unsafe {
        let module = GetModuleHandleW(w!("user32.dll")).ok()?;
        GetProcAddress(module, name)
    }
}

/// The types of the exports of user32 that mark the windows of the
/// shell; they are what ReactOS and Explorer call.
type OneWindow = unsafe extern "system" fn(HWND) -> BOOL;
type TwoWindows = unsafe extern "system" fn(HWND, HWND) -> BOOL;

/// Tells Windows that `desktop`, with `icons` in it, is the shell window,
/// which keeps it below every other window. What Windows lacks or
/// refuses is reported.
fn become_desktop(desktop: HWND, icons: HWND) {
    // SAFETY: the exports have these signatures in every user32 that has
    // them, and the windows belong to this thread.
    unsafe {
        let shell = match (user32(s!("SetShellWindowEx")), user32(s!("SetShellWindow"))) {
            (Some(both), _) => mem::transmute::<Export, TwoWindows>(both)(desktop, icons).as_bool(),
            (None, Some(one)) => mem::transmute::<Export, OneWindow>(one)(desktop).as_bool(),
            (None, None) => false,
        };
        if !shell || GetShellWindow() != desktop {
            report!("Windows did not take Tessera's desktop as the shell window");
        }
        match user32(s!("SetProgmanWindow")) {
            Some(set) => {
                let _ = mem::transmute::<Export, OneWindow>(set)(desktop);
            }
            None => report!("this Windows has no SetProgmanWindow"),
        }
    }
}

/// Tells Windows that `hook` is the task manager of the shell, which
/// Windows hands Ctrl+Esc, the Windows key and some codes of the shell
/// hook. What Windows lacks is reported.
fn become_task_manager(hook: HWND) {
    // SAFETY: the export has this signature in every user32 that has it,
    // and the window belongs to this thread.
    unsafe {
        match user32(s!("SetTaskmanWindow")) {
            Some(set) => {
                let _ = mem::transmute::<Export, OneWindow>(set)(hook);
            }
            None => {
                report!("this Windows has no SetTaskmanWindow, so Ctrl+Esc does not reach Tessera")
            }
        }
    }
}

/// Has minimized windows go out of sight instead of lying at the bottom
/// of the screen as small title bars, as Explorer has them, for the rest
/// of the session.
fn hide_minimized_windows() {
    let mut metrics = MINIMIZEDMETRICS {
        cbSize: size_of::<MINIMIZEDMETRICS>() as u32,
        ..Default::default()
    };
    let pointer = (&raw mut metrics).cast::<c_void>();
    // SAFETY: `metrics` is a sized structure that outlives the calls; the
    // setting is not written to the profile.
    unsafe {
        let read = SystemParametersInfoW(
            SPI_GETMINIMIZEDMETRICS,
            metrics.cbSize,
            Some(pointer),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        if read.is_err() {
            return;
        }
        metrics.iArrange = MINIMIZEDMETRICS_ARRANGE(metrics.iArrange.0 | ARW_HIDE);
        if let Err(error) = SystemParametersInfoW(
            SPI_SETMINIMIZEDMETRICS,
            metrics.cbSize,
            Some(pointer),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        ) {
            report!("minimized windows lie at the bottom of the screen: {error}");
        }
    }
}

/// Holds the keys of recovery while the configuration asks for them, and
/// lets them go otherwise; on the host thread.
fn hold_emergency_keys() {
    let hook = HOOK.load(Ordering::SeqCst);
    if hook == 0 {
        return;
    }
    let hook = handle(hook);
    let modifiers = MOD_CONTROL | MOD_WIN | MOD_ALT | MOD_NOREPEAT;
    // SAFETY: the window belongs to this thread; a key that is not held
    // is let go without effect.
    unsafe {
        for (key, letter) in [(KEY_EXPLORER, b'E'), (KEY_RESTART, b'R')] {
            if !EMERGENCY_KEYS.load(Ordering::Relaxed) {
                let _ = UnregisterHotKey(Some(hook), key);
                continue;
            }
            if let Err(error) = RegisterHotKey(Some(hook), key, modifiers, u32::from(letter)) {
                let letter = char::from(letter);
                report!("Ctrl+Win+Alt+{letter} is held by another program: {error}");
            }
        }
    }
}

/// Does the work that is waiting, in the order it was handed over.
fn work_waiting() {
    let waiting = mem::take(&mut *locked(&WORK));
    for work in waiting {
        if catch_unwind(AssertUnwindSafe(work)).is_err() {
            report!("work done on the thread of the shell failed");
        }
    }
}

/// Keeps a window that is moved in the order of windows at the bottom.
///
/// # Safety
/// `position` must be the `WINDOWPOS` of `WM_WINDOWPOSCHANGING`.
unsafe fn keep_at_bottom(position: LPARAM) {
    let position = position.0 as *mut WINDOWPOS;
    if position.is_null() {
        return;
    }
    // SAFETY: the caller passes the structure Windows gave.
    unsafe {
        if (*position).flags & SWP_NOZORDER == SWP_NOZORDER {
            return;
        }
        (*position).hwndInsertAfter = HWND_BOTTOM;
    }
}

/// Paints `window` with the wallpaper of Windows.
fn paint_with_wallpaper(window: HWND) {
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: the paint structure outlives the calls, and the context is
    // released by `EndPaint`.
    unsafe {
        let context = BeginPaint(window, &raw mut paint);
        let _ = PaintDesktop(context);
        let _ = EndPaint(window, &raw const paint);
    }
}

/// Fits the desktop window and its children to the virtual screen.
fn fit_desktop(desktop: HWND) {
    let frame = virtual_screen();
    let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
    // SAFETY: the windows belong to this thread.
    unsafe {
        let _ = SetWindowPos(
            desktop,
            Some(HWND_BOTTOM),
            frame.left,
            frame.top,
            width,
            height,
            SWP_NOACTIVATE,
        );
        for class in [w!("WorkerW"), w!("SHELLDLL_DefView")] {
            if let Ok(child) = FindWindowExW(Some(desktop), None, class, None) {
                let _ = MoveWindow(child, 0, 0, width, height, true);
            }
        }
        let _ = RedrawWindow(
            Some(desktop),
            None,
            None,
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
        );
    }
}

/// What the end of a session asks the windows of this thread, answered:
/// `None` for other messages.
fn session_message(message: u32, w: WPARAM) -> Option<LRESULT> {
    match message {
        WM_QUERYENDSESSION => Some(LRESULT(1)),
        WM_ENDSESSION => {
            if w.0 != 0 && !ENDING.swap(true, Ordering::SeqCst) {
                publish(ShellEvent::SessionEnding);
                // What needs no GPUI is put back here and now, so that it
                // is done even if GPUI does not answer; GPUI is then given
                // the rest of the time for what only it can do.
                if let Some(restore) = SESSION_END.get()
                    && catch_unwind(*restore).is_err()
                {
                    report!("what Tessera hid was not all shown at the end of the session");
                }
                wait_for_restoring();
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// Waits until Tessera has restored what it changed, at most
/// [`ENDING_TIME`], handling the messages other threads send meanwhile:
/// a backdrop that goes, made on the thread of GPUI, sends one to the
/// desktop window.
fn wait_for_restoring() {
    let event = RESTORED.load(Ordering::SeqCst);
    if event == 0 {
        return;
    }
    let events = [HANDLE(event as *mut c_void)];
    let until = Instant::now() + ENDING_TIME;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        // SAFETY: the event is kept for the life of the process, and
        // `message` outlives the call that dispatches what was sent.
        unsafe {
            let woken = MsgWaitForMultipleObjectsEx(
                Some(&events),
                left.as_millis() as u32,
                QS_SENDMESSAGE,
                MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(0),
            );
            if woken == WAIT_OBJECT_0 {
                break;
            }
            if woken.0 == WAIT_OBJECT_0.0 + 1 {
                let mut message = MSG::default();
                let _ = PeekMessageW(
                    &raw mut message,
                    None,
                    0,
                    0,
                    PM_NOREMOVE | PM_QS_SENDMESSAGE,
                );
                continue;
            }
            break;
        }
    }
}

/// Runs `respond` for a window procedure, reporting a panic instead of
/// letting it unwind into Windows, and gives the default handling for
/// `None`.
///
/// # Safety
/// The arguments must be those Windows passed to the window procedure.
unsafe fn answer(
    window: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
    respond: impl FnOnce() -> Option<LRESULT>,
) -> LRESULT {
    match catch_unwind(AssertUnwindSafe(respond)) {
        Ok(Some(answer)) => return answer,
        Ok(None) => {}
        Err(_) => report!("a message to the shell was not handled"),
    }
    // SAFETY: the arguments are passed on as they came.
    unsafe { DefWindowProcW(window, message, w, l) }
}

/// The window procedure of the desktop window.
unsafe extern "system" fn respond_desktop(
    window: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    let respond = || match message {
        WM_PAINT => {
            paint_with_wallpaper(window);
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => {
            // SAFETY: `wParam` is the context of the device to erase.
            let _ = unsafe { PaintDesktop(HDC(w.0 as *mut c_void)) };
            Some(LRESULT(1))
        }
        WM_WINDOWPOSCHANGING => {
            // SAFETY: `lParam` of this message is its `WINDOWPOS`.
            unsafe { keep_at_bottom(l) };
            None
        }
        // Alt+F4 on the desktop: the desktop stays, the user is asked how
        // to end the session.
        WM_CLOSE => {
            publish(ShellEvent::CloseRequested);
            Some(LRESULT(0))
        }
        WM_SYSCOMMAND if w.0 as u32 & 0xFFF0 == SC_CLOSE => {
            publish(ShellEvent::CloseRequested);
            Some(LRESULT(0))
        }
        RAISE_DESKTOP => {
            // SAFETY: the window belongs to this thread.
            let missing =
                unsafe { FindWindowExW(Some(window), None, w!("WorkerW"), None) }.is_err();
            if missing {
                match create_child(window, w!("WorkerW"), WINDOW_EX_STYLE(0)) {
                    // SAFETY: the child belongs to this thread.
                    Ok(wallpaper) => unsafe {
                        let flags = SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE;
                        let _ = SetWindowPos(wallpaper, Some(HWND_BOTTOM), 0, 0, 0, 0, flags);
                    },
                    Err(error) => report!("{error:#}"),
                }
            }
            Some(LRESULT(0))
        }
        WM_DISPLAYCHANGE => {
            fit_desktop(window);
            None
        }
        WM_SETTINGCHANGE
            if w.0 as u32 == SPI_SETWORKAREA.0 || w.0 as u32 == SPI_SETDESKWALLPAPER.0 =>
        {
            fit_desktop(window);
            None
        }
        WM_NCDESTROY => {
            DESKTOP.store(0, Ordering::SeqCst);
            None
        }
        _ => session_message(message, w),
    };
    // SAFETY: the arguments are those Windows passed.
    unsafe { answer(window, message, w, l, respond) }
}

/// The window procedure of `WorkerW`, which draws the wallpaper.
unsafe extern "system" fn respond_wallpaper(
    window: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    let respond = || match message {
        WM_PAINT => {
            paint_with_wallpaper(window);
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => {
            // SAFETY: `wParam` is the context of the device to erase.
            let _ = unsafe { PaintDesktop(HDC(w.0 as *mut c_void)) };
            Some(LRESULT(1))
        }
        _ => None,
    };
    // SAFETY: the arguments are those Windows passed.
    unsafe { answer(window, message, w, l, respond) }
}

/// The window procedure of `SHELLDLL_DefView`, which draws nothing, so
/// that what lies below it is seen.
unsafe extern "system" fn respond_icons(
    window: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    let respond = || match message {
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            // SAFETY: the paint structure outlives both calls.
            unsafe {
                BeginPaint(window, &raw mut paint);
                let _ = EndPaint(window, &raw const paint);
            }
            Some(LRESULT(0))
        }
        _ => None,
    };
    // SAFETY: the arguments are those Windows passed.
    unsafe { answer(window, message, w, l, respond) }
}

/// The window procedure of the hidden window of the shell hook.
unsafe extern "system" fn respond_hook(
    window: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    let respond = || {
        let shellhook = SHELLHOOK.load(Ordering::Relaxed);
        if message == shellhook && shellhook != 0 {
            return Some(shell_hook(w.0 as u32, l));
        }
        match message {
            WORK_WAITING => {
                work_waiting();
                Some(LRESULT(0))
            }
            WM_SYSCOMMAND if w.0 as u32 & 0xFFF0 == SC_TASKLIST => {
                request_start();
                Some(LRESULT(0))
            }
            WM_HOTKEY => {
                recover(w.0 as i32);
                Some(LRESULT(0))
            }
            WM_TIMER if w.0 == FORCED_EXIT => {
                report!("Tessera did not quit in time and ends now");
                std::process::exit(1);
            }
            WM_WTSSESSION_CHANGE => {
                match w.0 as u32 {
                    WTS_SESSION_LOCK => publish(ShellEvent::Locked),
                    WTS_SESSION_UNLOCK => publish(ShellEvent::Unlocked),
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_SETTINGCHANGE => {
                // SAFETY: `lParam` of this message is null or a text.
                if unsafe { names_environment(l) } {
                    refresh_environment();
                    publish(ShellEvent::EnvironmentChanged);
                }
                None
            }
            WM_DISPLAYCHANGE => {
                publish(ShellEvent::DisplaysChanged);
                None
            }
            WM_QUERYENDSESSION | WM_ENDSESSION => session_message(message, w),
            _ if is_open_range(message) || message == WM_COPYDATA => {
                let answer = ask_handlers(message, w.0, l.0);
                if answer.is_none() && is_open_range(message) {
                    publish(ShellEvent::Raw {
                        message,
                        w: w.0,
                        l: l.0,
                    });
                }
                answer.map(LRESULT)
            }
            _ => None,
        }
    };
    // SAFETY: the arguments are those Windows passed.
    unsafe { answer(window, message, w, l, respond) }
}

/// Tells of a request for the Start menu. Windows may tell one press both
/// as `SC_TASKLIST` and as `HSHELL_TASKMAN`; the second within
/// [`START_TWICE`] is the same press.
fn request_start() {
    static FIRST: OnceLock<Instant> = OnceLock::new();
    static LAST: AtomicU64 = AtomicU64::new(0);
    let first = *FIRST.get_or_init(Instant::now);
    // Counted from one, so that the first press is never too soon.
    let now = first.elapsed().as_millis() as u64 + 1;
    let last = LAST.swap(now, Ordering::Relaxed);
    if last == 0 || now - last >= START_TWICE.as_millis() as u64 {
        publish(ShellEvent::StartRequested);
    }
}

/// Whether `message` is in the ranges other programs use: `WM_USER` to
/// 0x7FFF, or a registered one.
fn is_open_range(message: u32) -> bool {
    (WM_USER..0x8000).contains(&message) || (0xC000..=0xFFFF).contains(&message)
}

/// Asks the handlers of [`ShellHost::intercept`], the newest first.
fn ask_handlers(message: u32, w: usize, l: isize) -> Option<isize> {
    // A copy, so that a handler can add or remove handlers.
    let handlers: Vec<(u64, Handler)> = locked(&HANDLERS).iter().rev().cloned().collect();
    for (number, handler) in handlers {
        let Ok(mut handler) = handler.try_lock() else {
            continue;
        };
        match catch_unwind(AssertUnwindSafe(|| (*handler)(message, w, l))) {
            Ok(Some(answer)) => return Some(answer),
            Ok(None) => {}
            Err(_) => {
                drop(handler);
                report!("a handler of the shell failed and was removed");
                remove_handler(number);
            }
        }
    }
    None
}

/// Answers a message of the shell hook.
fn shell_hook(code: u32, l: LPARAM) -> LRESULT {
    let shell = ShellHost::is_shell();
    match code {
        HSHELL_GETMINRECT => {
            // SAFETY: `lParam` of this code is the `SHELLHOOKINFO` to fill.
            let found = unsafe { answer_min_rect(l) };
            LRESULT(found.into())
        }
        HSHELL_APPCOMMAND => {
            let command = app_command(l.0);
            publish(ShellEvent::AppCommand(command));
            // As the shell, a key of the volume or of media is taken here
            // and done by the user interface, whose flyout shows it.
            let taken = { shell && MEDIA_KEYS.load(Ordering::Relaxed) }
                .then(|| AppCommand::of(command))
                .flatten();
            if let Some(key) = taken {
                publish(ShellEvent::Key(key));
            }
            LRESULT(taken.is_some().into())
        }
        HSHELL_ENDTASK => {
            publish(ShellEvent::EndTask(AppWindowId::from_raw(l.0)));
            if shell {
                end_task(l.0);
            }
            LRESULT(shell.into())
        }
        HSHELL_TASKMAN if shell => {
            request_start();
            LRESULT(1)
        }
        code => {
            if let Some(event) = event_of(code, l.0, &REPLACING) {
                publish(event);
            }
            LRESULT(0)
        }
    }
}

/// The event a code of the shell hook about `window` tells; `replacing`
/// keeps the window that replaces another until the one it replaces is
/// told.
fn event_of(code: u32, window: isize, replacing: &AtomicIsize) -> Option<ShellEvent> {
    let id = AppWindowId::from_raw(window);
    Some(match code {
        HSHELL_WINDOWCREATED => ShellEvent::Created(id),
        HSHELL_WINDOWDESTROYED => ShellEvent::Destroyed(id),
        HSHELL_WINDOWACTIVATED | HSHELL_RUDEAPPACTIVATED => ShellEvent::Activated {
            window: id,
            fullscreen: code == HSHELL_RUDEAPPACTIVATED,
        },
        HSHELL_REDRAW => ShellEvent::Redraw(id),
        HSHELL_FLASH => ShellEvent::Flash(id),
        HSHELL_MONITORCHANGED => ShellEvent::MonitorChanged(id),
        HSHELL_WINDOWREPLACING => {
            replacing.store(window, Ordering::Relaxed);
            return None;
        }
        HSHELL_WINDOWREPLACED => {
            let new = replacing.swap(0, Ordering::Relaxed);
            ShellEvent::Replaced {
                old: id,
                new: (new != 0).then(|| AppWindowId::from_raw(new)),
            }
        }
        _ => return None,
    })
}

/// The command of a `WM_APPCOMMAND` from its `lParam`.
fn app_command(l: isize) -> u16 {
    let high = (l as usize >> 16) as u32 & 0xFFFF;
    (high & !FAPPCOMMAND_MASK) as u16
}

/// Fills the `SHELLHOOKINFO` of `HSHELL_GETMINRECT` with where the dock
/// shows its window, and says whether it knew.
///
/// The number of the `SHELLHOOK` message is known to every program, so
/// the message is checked before its `lParam` is used: Windows sends it
/// and waits for the answer, so a posted one is not from Windows, and
/// the structure must lie in memory of this process that can be written.
///
/// # Safety
/// `info` must be the `lParam` of a message of the shell hook with this
/// code.
unsafe fn answer_min_rect(info: LPARAM) -> bool {
    let info = info.0 as *mut SHELLHOOKINFO;
    // SAFETY: the call only reads the state of the message being handled.
    let sent = unsafe { InSendMessage() }.as_bool();
    if !sent || !is_writable(info.cast(), size_of::<SHELLHOOKINFO>()) {
        return false;
    }
    // SAFETY: the structure lies in committed memory of this process that
    // can be read and written, as checked above.
    let window = unsafe { (*info).hwnd.0 as isize };
    let rects = locked(&MIN_RECTS);
    let Some(rect) = rects.as_ref().and_then(|rects| rects.get(&window)) else {
        return false;
    };
    // SAFETY: as above; the memory can be written.
    unsafe {
        (*info).rc = RECT {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        };
    }
    true
}

/// Whether the `size` bytes at `start` lie in one committed region of
/// this process that can be read and written. A null `start` cannot.
fn is_writable(start: *const c_void, size: usize) -> bool {
    if start.is_null() {
        return false;
    }
    let mut region = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: the call only describes the memory at `start` into `region`,
    // which outlives it; it reads nothing at `start`.
    let written = unsafe {
        VirtualQuery(
            Some(start),
            &raw mut region,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if written == 0 || region.State != MEM_COMMIT {
        return false;
    }
    let protection = region.Protect;
    let writable = protection & PAGE_GUARD != PAGE_GUARD
        && [PAGE_READWRITE, PAGE_EXECUTE_READWRITE]
            .into_iter()
            .any(|kind| protection.0 & 0xFF == kind.0);
    let end = region.BaseAddress as usize + region.RegionSize;
    writable
        && (start as usize)
            .checked_add(size)
            .is_some_and(|last| last <= end)
}

/// Asks `window` to close, and has Windows end it if it does not answer
/// (`EndTask` with `fForce`, as Explorer does for `HSHELL_ENDTASK`), on a
/// thread of its own, as `EndTask` may wait.
fn end_task(window: isize) {
    type EndTask = unsafe extern "system" fn(HWND, BOOL, BOOL) -> BOOL;
    let Some(end) = user32(s!("EndTask")) else {
        return;
    };
    // SAFETY: `EndTask` has this signature in every user32.
    let end = unsafe { mem::transmute::<Export, EndTask>(end) };
    let started = thread::Builder::new()
        .name("end task".to_owned())
        .spawn(move || {
            // SAFETY: the call tolerates a window that has been destroyed.
            let _ = unsafe { end(handle(window), false.into(), true.into()) };
        });
    if let Err(error) = started {
        report!("the window was not asked to close: {error}");
    }
}

/// What a key of recovery does, on the host thread.
fn recover(key: i32) {
    match key {
        KEY_EXPLORER => {
            Launch::new("taskmgr.exe").start();
            match super::ExplorerShell::hand_over() {
                Ok(true) => request_quit(),
                Ok(false) => {}
                Err(error) => report!("the session was not handed to Explorer: {error:#}"),
            }
        }
        KEY_RESTART => {
            let started = std::env::current_exe()
                .context("the Tessera program was not found")
                .and_then(|program| Taskbar::start_successor(&program));
            match started {
                Ok(()) => request_quit(),
                Err(error) => report!("Tessera does not start again: {error:#}"),
            }
        }
        _ => {}
    }
}

/// Asks GPUI to quit, and ends the process if it has not after
/// [`QUIT_TIME`]: a key of recovery must work when GPUI does not.
fn request_quit() {
    publish(ShellEvent::QuitRequested);
    let hook = HOOK.load(Ordering::SeqCst);
    // SAFETY: the timer belongs to the window of this thread.
    unsafe { SetTimer(Some(handle(hook)), FORCED_EXIT, QUIT_TIME, None) };
}

/// Whether the `lParam` of `WM_SETTINGCHANGE` names the environment.
///
/// # Safety
/// `l` must be that `lParam`: null or a NUL-terminated text.
unsafe fn names_environment(l: LPARAM) -> bool {
    if l.0 == 0 {
        return false;
    }
    // SAFETY: as the caller says.
    let text = unsafe { PCWSTR(l.0 as *const u16).to_string() };
    text.is_ok_and(|text| text.eq_ignore_ascii_case("Environment"))
}

/// The variables of the user as Windows would give them to a program
/// started now: those of the system, of the user and of the session.
fn user_environment() -> Option<Vec<(String, String)>> {
    let mut token = HANDLE::default();
    let mut block: *mut c_void = std::ptr::null_mut();
    // SAFETY: the token is closed and the block destroyed below; the block
    // is read up to the empty text that ends it.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE,
            &raw mut token,
        )
        .ok()?;
        let made = CreateEnvironmentBlock(&raw mut block, Some(token), false);
        let _ = CloseHandle(token);
        made.ok()?;
        let start = block as *const u16;
        let mut length = 0;
        while *start.add(length) != 0 || *start.add(length + 1) != 0 {
            length += 1;
        }
        let units = std::slice::from_raw_parts(start, length + 2);
        let variables = parse_block(units);
        let _ = DestroyEnvironmentBlock(block);
        Some(variables)
    }
}

/// The variables of an environment block: `NAME=value` texts ending in
/// NUL, the last followed by another NUL. Names that start with `=`, as
/// the folders of drives, are left out.
fn parse_block(units: &[u16]) -> Vec<(String, String)> {
    units
        .split(|&unit| unit == 0)
        .take_while(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let entry = String::from_utf16_lossy(entry);
            let (name, value) = entry.split_once('=')?;
            (!name.is_empty()).then(|| (name.to_owned(), value.to_owned()))
        })
        .collect()
}

/// The names of the variables `before` had that `after` lacks; names are
/// compared as Windows does, without case.
fn removed(before: &[(String, String)], after: &[(String, String)]) -> Vec<String> {
    let kept = |name: &str| {
        after
            .iter()
            .any(|(other, _)| other.eq_ignore_ascii_case(name))
    };
    before
        .iter()
        .map(|(name, _)| name)
        .filter(|name| !kept(name))
        .cloned()
        .collect()
}

/// Takes the variables of the user into the environment of the process,
/// so that what Tessera starts afterwards gets them, as programs Explorer
/// starts do. A variable the user removed is removed; variables the
/// process has of its own, that the user never had, stay.
fn refresh_environment() {
    let Some(after) = user_environment() else {
        report!("the new environment variables of the user were not read");
        return;
    };
    let mut before = locked(&ENVIRONMENT);
    for name in removed(&before, &after) {
        // SAFETY: the name is NUL-terminated and outlives the call.
        let _ = unsafe { SetEnvironmentVariableW(&HSTRING::from(name.as_str()), PCWSTR::null()) };
    }
    for (name, value) in &after {
        let name = HSTRING::from(name.as_str());
        let value = HSTRING::from(value.as_str());
        // SAFETY: both texts are NUL-terminated and outlive the call.
        let _ = unsafe { SetEnvironmentVariableW(&name, &value) };
    }
    *before = after;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codes_of_the_hook_become_events() {
        let replacing = AtomicIsize::new(0);
        let window = AppWindowId::from_raw(0x1234);
        assert_eq!(
            event_of(HSHELL_WINDOWCREATED, 0x1234, &replacing),
            Some(ShellEvent::Created(window))
        );
        assert_eq!(
            event_of(HSHELL_RUDEAPPACTIVATED, 0x1234, &replacing),
            Some(ShellEvent::Activated {
                window,
                fullscreen: true
            })
        );
        assert_eq!(
            event_of(HSHELL_WINDOWACTIVATED, 0x1234, &replacing),
            Some(ShellEvent::Activated {
                window,
                fullscreen: false
            })
        );
        assert_eq!(
            event_of(HSHELL_FLASH, 0x1234, &replacing),
            Some(ShellEvent::Flash(window))
        );
        assert_eq!(event_of(0x77, 0x1234, &replacing), None);
    }

    #[test]
    fn a_replacement_is_told_with_both_windows() {
        let replacing = AtomicIsize::new(0);
        assert_eq!(event_of(HSHELL_WINDOWREPLACING, 0x20, &replacing), None);
        assert_eq!(
            event_of(HSHELL_WINDOWREPLACED, 0x10, &replacing),
            Some(ShellEvent::Replaced {
                old: AppWindowId::from_raw(0x10),
                new: Some(AppWindowId::from_raw(0x20)),
            })
        );
        assert_eq!(
            event_of(HSHELL_WINDOWREPLACED, 0x10, &replacing),
            Some(ShellEvent::Replaced {
                old: AppWindowId::from_raw(0x10),
                new: None,
            })
        );
    }

    #[test]
    fn the_command_is_read_without_the_device_bits() {
        // The volume up key of a keyboard: the command in the high word,
        // the device in its top bits.
        // APPCOMMAND_VOLUME_UP is 10.
        let l = ((0x8000 | 10u32) << 16) as isize;
        assert_eq!(app_command(l), 10);
        assert_eq!(AppCommand::of(app_command(l)), Some(AppCommand::VolumeUp));
        // Browser back is left to the window.
        assert_eq!(AppCommand::of(1), None);
    }

    #[test]
    fn an_environment_block_is_read_up_to_its_end() {
        let text = "=C:=C:\\\0PATH=C:\\Windows\0TEMP=C:\\Temp\0\0LEFT=over\0";
        let units: Vec<u16> = text.encode_utf16().collect();
        assert_eq!(
            parse_block(&units),
            [
                ("PATH".to_owned(), r"C:\Windows".to_owned()),
                ("TEMP".to_owned(), r"C:\Temp".to_owned())
            ]
        );
    }

    #[test]
    fn only_variables_the_user_had_are_removed() {
        let pair = |name: &str| (name.to_owned(), "1".to_owned());
        let before = [pair("Path"), pair("OLD"), pair("TEMP")];
        let after = [pair("PATH"), pair("TEMP"), pair("NEW")];
        assert_eq!(removed(&before, &after), ["OLD"]);
        assert!(removed(&[], &after).is_empty());
    }

    #[test]
    fn the_ranges_of_other_programs_are_open() {
        assert!(is_open_range(WM_USER));
        assert!(is_open_range(0x7FFF));
        assert!(!is_open_range(0x8000));
        assert!(is_open_range(0xC123));
        assert!(!is_open_range(WM_PAINT));
    }
}
