// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Top-level application windows, as a task switcher would list them.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CLOAKED, DWMWA_COLOR_DEFAULT, DWMWA_EXTENDED_FRAME_BOUNDS,
    DwmGetWindowAttribute, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromRect};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumChildWindows, EnumWindows, GW_OWNER, GWL_EXSTYLE, GWL_STYLE,
    GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowPlacement,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    InternalGetWindowText, IsIconic, IsWindow, IsWindowVisible, IsZoomed, PostMessageW, SW_HIDE,
    SW_MINIMIZE, SW_RESTORE, SW_SHOWNA, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOZORDER,
    SetForegroundWindow, SetWindowPos, ShowWindowAsync, WINDOWPLACEMENT, WM_CLOSE, WS_CHILD,
    WS_EX_TOOLWINDOW, WS_THICKFRAME,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_CLOAKED, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_NAMECHANGE,
    EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, OBJID_WINDOW,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};
use windows::core::{BOOL, PWSTR};

use super::{DisplayKey, NativeSurface, ScreenRect};

/// Identifies a top-level window for as long as it exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AppWindowId(isize);

impl AppWindowId {
    pub(super) fn hwnd(self) -> HWND {
        HWND(self.0 as *mut c_void)
    }

    /// The window a number names that [`Self::raw`] gave.
    pub fn from_raw(raw: isize) -> Self {
        Self(raw)
    }

    /// The number Windows knows the window by, which may name another
    /// window once this one is gone.
    pub fn raw(self) -> isize {
        self.0
    }

    /// Whether the window is still there.
    pub fn exists(self) -> bool {
        // SAFETY: the call only reads whether the handle names a window.
        unsafe { IsWindow(Some(self.hwnd())).as_bool() }
    }

    /// The window of `window`, a window of GPUI.
    pub fn of(window: &gpui::Window) -> Option<Self> {
        let surface = NativeSurface::of(window).ok()?;
        Some(Self(surface.hwnd.0 as isize))
    }

    /// Whether the window is shown; a minimized window is.
    pub fn is_shown(self) -> bool {
        // SAFETY: the call only reads window state and tolerates a handle
        // whose window has since been destroyed.
        unsafe { IsWindowVisible(self.hwnd()).as_bool() }
    }

    /// The process the window belongs to; 0 if the window is gone.
    pub fn process(self) -> u32 {
        let mut process = 0u32;
        // SAFETY: `process` outlives the call, which tolerates a handle
        // whose window has since been destroyed.
        unsafe { GetWindowThreadProcessId(self.hwnd(), Some(&raw mut process)) };
        process
    }

    /// Asks the window to stop being shown, without closing it. Windows
    /// does not hide a window of an elevated process.
    pub fn hide(self) {
        // SAFETY: the call tolerates a handle whose window has since been
        // destroyed and fails without side effects.
        unsafe {
            let _ = ShowWindowAsync(self.hwnd(), SW_HIDE);
        }
    }

    /// Asks a window that was hidden to be shown again as it was, without
    /// bringing it forward.
    pub fn unhide(self) {
        // SAFETY: as in `hide`.
        unsafe {
            let _ = ShowWindowAsync(self.hwnd(), SW_SHOWNA);
        }
    }

    /// The window that currently receives keyboard input, if any.
    pub fn foreground() -> Option<Self> {
        // SAFETY: the call takes no arguments and only reads system state.
        let hwnd = unsafe { GetForegroundWindow() };
        (!hwnd.is_invalid()).then_some(Self(hwnd.0 as isize))
    }

    /// Brings the window to the foreground, restoring it if minimized. A
    /// window that is already in the foreground is minimized instead.
    ///
    /// Windows may refuse either request, for example for a window of an
    /// elevated process; there is no reliable signal to report in that case.
    pub fn activate_or_minimize(self) {
        let hwnd = self.hwnd();
        // SAFETY: every call tolerates a handle whose window has since been
        // destroyed and fails without side effects.
        unsafe {
            if Self::foreground() == Some(self) && !IsIconic(hwnd).as_bool() {
                let _ = ShowWindowAsync(hwnd, SW_MINIMIZE);
                return;
            }
        }
        self.activate();
    }

    /// Brings the window to the foreground, restoring it if minimized.
    /// Windows may refuse, as for [`Self::activate_or_minimize`].
    ///
    /// Windows lets a program put a window in front only when the last
    /// input went to that program, which a key combination answered
    /// without a window in front is not, nor a command handed over by
    /// another start of the program. When it refuses, a key nothing
    /// listens to is pressed from here, which counts as such input, and
    /// the window is asked for again; when that is refused too, it is
    /// asked for as if by the window that is in front, which may always
    /// give its place away.
    pub fn activate(self) {
        let hwnd = self.hwnd();
        // SAFETY: the call tolerates a handle whose window has since been
        // destroyed and fails without side effects.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindowAsync(hwnd, SW_RESTORE);
            }
        }
        if self.come_in_front() {
            return;
        }
        super::windows_key::press_unassigned_key();
        if self.come_in_front() {
            return;
        }
        // SAFETY: the calls only read which threads these are, and
        // joining the input of a thread that has ended fails without
        // side effects; what is joined is parted again right away.
        unsafe {
            let ours = GetCurrentThreadId();
            let theirs = GetWindowThreadProcessId(GetForegroundWindow(), None);
            if theirs == 0 || theirs == ours {
                return;
            }
            if AttachThreadInput(ours, theirs, true).as_bool() {
                let _ = SetForegroundWindow(hwnd);
                let _ = BringWindowToTop(hwnd);
                let _ = AttachThreadInput(ours, theirs, false);
            }
        }
    }

    /// Asks Windows to put the window in front, and says whether it is
    /// there now: Windows may say yes and only flash its button.
    fn come_in_front(self) -> bool {
        // SAFETY: the call tolerates a handle whose window has since been
        // destroyed and fails without side effects.
        let _ = unsafe { SetForegroundWindow(self.hwnd()) };
        Self::foreground() == Some(self)
    }

    /// Asks the window to close, as its close button does. The application
    /// may ask the user first, or refuse.
    pub fn close(self) {
        // SAFETY: posting to a window that has since been destroyed fails
        // without side effects.
        unsafe {
            let _ = PostMessageW(Some(self.hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    /// What is seen of the window on the screen, without the invisible
    /// border Windows gives windows to be resized by.
    pub fn frame(self) -> Option<ScreenRect> {
        let mut frame = RECT::default();
        // SAFETY: `frame` is as large as the call is told, and a window
        // that has since been destroyed makes the call fail.
        let read = unsafe {
            DwmGetWindowAttribute(
                self.hwnd(),
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&raw mut frame).cast(),
                size_of::<RECT>() as u32,
            )
        };
        read.ok().map(|()| frame.into())
    }

    /// Has Windows draw the thin border of the window in `color`, as
    /// 0x00BBGGRR, or in its own color again when there is none. Whether
    /// Windows took the request is returned.
    pub fn set_border(self, color: Option<u32>) -> bool {
        let color = color.unwrap_or(DWMWA_COLOR_DEFAULT);
        // SAFETY: `color` is as large as the call is told and outlives
        // it, and a window that has since been destroyed makes it fail.
        let set = unsafe {
            DwmSetWindowAttribute(
                self.hwnd(),
                DWMWA_BORDER_COLOR,
                (&raw const color).cast(),
                size_of::<u32>() as u32,
            )
        };
        set.is_ok()
    }

    /// Whether the window can be given a place among tiles: it is neither
    /// minimized nor maximized, and its border resizes it.
    pub fn is_tileable(self) -> bool {
        let hwnd = self.hwnd();
        // SAFETY: the calls only read window state and tolerate a handle
        // whose window has since been destroyed.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            !IsIconic(hwnd).as_bool() && !IsZoomed(hwnd).as_bool() && style & WS_THICKFRAME.0 != 0
        }
    }

    /// Asks the window to move and resize so that what is seen of it
    /// covers `frame`, without bringing it forward. Whether Windows took
    /// the request is returned; the application may still keep another
    /// size, and a window of an elevated process is not moved at all.
    pub fn place(self, frame: ScreenRect) -> bool {
        let hwnd = self.hwnd();
        let Some(seen) = self.frame() else {
            return false;
        };
        let mut whole = RECT::default();
        // SAFETY: `whole` outlives the calls, which tolerate a handle
        // whose window has since been destroyed.
        unsafe {
            if GetWindowRect(hwnd, &raw mut whole).is_err() {
                return false;
            }
            // The invisible border stays as wide as it is.
            let left = frame.left - (seen.left - whole.left);
            let top = frame.top - (seen.top - whole.top);
            let right = frame.right + (whole.right - seen.right);
            let bottom = frame.bottom + (whole.bottom - seen.bottom);
            let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS;
            SetWindowPos(hwnd, None, left, top, right - left, bottom - top, flags).is_ok()
        }
    }

    /// What the window's title bar says; empty if Windows does not tell.
    pub fn title(self) -> String {
        let mut buffer = [0u16; 512];
        // A window of this process is not asked: asking it waits for its
        // thread. What it was last titled is read instead.
        let own = self.process() == std::process::id();
        // SAFETY: the buffer outlives the call, which writes no more than
        // its length.
        let length = unsafe {
            match own {
                true => InternalGetWindowText(self.hwnd(), &mut buffer),
                false => GetWindowTextW(self.hwnd(), &mut buffer),
            }
        };
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
    }

    /// The name of the window class the program made the window of;
    /// empty if Windows does not tell.
    pub fn class(self) -> String {
        let mut buffer = [0u16; 256];
        // SAFETY: the buffer outlives the call, which writes no more than
        // its length.
        let length = unsafe { GetClassNameW(self.hwnd(), &mut buffer) };
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
    }

    /// The window the application draws what is seen in. For the frame
    /// ApplicationFrameHost draws around a packaged application, that is
    /// the core window of the application inside it, or `None` while the
    /// frame holds none, as while its application is suspended; for every
    /// other window it is the window itself.
    pub fn content(self) -> Option<Self> {
        if self.class() != FRAME_CLASS {
            return Some(self);
        }
        let mut core = (self.process(), HWND::default());
        // SAFETY: `core` outlives the synchronous enumeration, which hands
        // it to `find_core` as its state; no window is sent a message.
        unsafe {
            let _ = EnumChildWindows(
                Some(self.hwnd()),
                Some(find_core),
                LPARAM(&raw mut core as isize),
            );
        }
        (!core.1.is_invalid()).then_some(Self(core.1.0 as isize))
    }

    /// The process that runs what the window shows, which is ended to end
    /// its program: that of the packaged application inside a frame of
    /// ApplicationFrameHost, or else the process of the window. `None`
    /// when a frame holds no application, as while it is suspended or
    /// closing, and for a window that is gone. Finding it asks no window
    /// anything.
    pub fn content_process(self) -> Option<u32> {
        self.content()
            .map(Self::process)
            .filter(|&process| process != 0)
    }
}

/// The class of the frames ApplicationFrameHost draws around the windows of
/// packaged applications.
const FRAME_CLASS: &str = "ApplicationFrameWindow";

/// The class of the window a packaged application draws in.
const CORE_CLASS: &str = "Windows.UI.Core.CoreWindow";

/// Keeps in `state`, a frame's process and a handle, the first child of
/// the frame that is the core window of another process.
unsafe extern "system" fn find_core(child: HWND, state: LPARAM) -> BOOL {
    // SAFETY: `state` is the pointer `AppWindowId::content` passed in.
    let core = unsafe { &mut *(state.0 as *mut (u32, HWND)) };
    let child_id = AppWindowId(child.0 as isize);
    if child_id.class() != CORE_CLASS || child_id.process() == core.0 {
        return true.into();
    }
    core.1 = child;
    false.into()
}

/// Whether a button of the pointer is held down, as while a window is
/// dragged.
pub fn pointer_held() -> bool {
    let held = |key: u16| {
        // SAFETY: the call takes a key and only reads its state.
        unsafe { GetAsyncKeyState(key.into()) < 0 }
    };
    held(VK_LBUTTON.0) || held(VK_RBUTTON.0)
}

/// Where the pointer is on the screen, in physical pixels.
pub fn pointer_at() -> Option<(i32, i32)> {
    let mut pointer = windows::Win32::Foundation::POINT::default();
    // SAFETY: `pointer` outlives the call that fills it.
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&raw mut pointer) }.ok()?;
    Some((pointer.x, pointer.y))
}

/// Whether Windows accepted every request of `WindowEvents::watch`.
static WATCHING: AtomicU64 = AtomicU64::new(0);

/// Counts what happened to windows since `WindowEvents::watch`.
static HAPPENED: AtomicU64 = AtomicU64::new(0);

/// Tells when the list of application windows may have changed, so that it
/// is read again only then.
pub struct WindowEvents;

impl WindowEvents {
    /// The ranges of events that can change the list or the foreground
    /// window: from a new foreground window to the end of a move, a resize
    /// or a minimized state; windows destroyed, shown and hidden; renamed;
    /// cloaked and uncloaked.
    const WATCHED: [(u32, u32); 4] = [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND),
        (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE),
        (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
    ];

    /// Starts counting, once. Windows delivers the events through the
    /// message queue of the calling thread, which therefore has to be one
    /// that handles messages, and delivers them for as long as the process
    /// runs. Whether Windows accepted every request is returned.
    pub fn watch() -> bool {
        static STARTED: Once = Once::new();
        STARTED.call_once(|| {
            let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
            let accepted = Self::WATCHED.iter().all(|&(first, last)| {
                // SAFETY: `happened` is a function of this program, which
                // is never unloaded; no module is injected anywhere.
                let hook =
                    unsafe { SetWinEventHook(first, last, None, Some(happened), 0, 0, flags) };
                !hook.is_invalid()
            });
            // The windows of this process that are listed, those of the
            // file manager, are watched apart: every other window of the
            // process is left out in `happened_own`.
            for (first, last) in Self::WATCHED {
                // SAFETY: as above; only the events of this process come.
                let hook = unsafe {
                    SetWinEventHook(
                        first,
                        last,
                        None,
                        Some(happened_own),
                        std::process::id(),
                        0,
                        WINEVENT_OUTOFCONTEXT,
                    )
                };
                if hook.is_invalid() {
                    report!("the windows of the file manager are not watched");
                }
            }
            WATCHING.store(accepted.into(), Ordering::Relaxed);
        });
        Self::watched()
    }

    /// Whether Windows reports what happens to windows, so that
    /// [`WindowEvents::generation`] tells of every change.
    pub fn watched() -> bool {
        WATCHING.load(Ordering::Relaxed) != 0
    }

    /// A number that changes whenever something happened to a window.
    pub fn generation() -> u64 {
        HAPPENED.load(Ordering::Relaxed)
    }
}

unsafe extern "system" fn happened(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    // Events of the parts of a window, such as its scroll bars, and of the
    // pointer do not change the list.
    if object != OBJID_WINDOW.0 || child != 0 {
        return;
    }
    // Neither do those of the windows inside a window, which programs show,
    // hide and rename all the time. A window that is gone by now has no
    // style to read, and counts.
    // SAFETY: reading the style of a window, valid or not, has no
    // preconditions.
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    if style & WS_CHILD.0 == 0 {
        HAPPENED.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe extern "system" fn happened_own(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if object != OBJID_WINDOW.0 || child != 0 {
        return;
    }
    // The events come only once the thread that made them is back in its
    // message loop, when a window that was closed is gone already: the
    // mark is looked for by its handle alone, and forgotten with it.
    let mut marks = OWN_MARKS.lock().unwrap_or_else(PoisonError::into_inner);
    let raw = hwnd.0 as isize;
    if !marks.iter().any(|mark| mark.hwnd == raw) {
        return;
    }
    if event == EVENT_OBJECT_DESTROY {
        marks.retain(|mark| mark.hwnd != raw);
    }
    drop(marks);
    HAPPENED.fetch_add(1, Ordering::Relaxed);
}

/// A window of Tessera that is listed among the application windows; see
/// [`AppWindow::mark_own`].
#[derive(Clone, Debug)]
struct OwnMark {
    hwnd: isize,
    app_id: String,
    /// The file whose icon the window shows.
    icon: PathBuf,
}

/// The windows of Tessera that are listed.
static OWN_MARKS: Mutex<Vec<OwnMark>> = Mutex::new(Vec::new());

/// What `hwnd` was marked as, if it was and is still there. A mark is
/// forgotten when the events of its window tell it is gone, in
/// `happened_own`, which needs it until then, or when another is made.
fn own_mark(hwnd: HWND) -> Option<OwnMark> {
    // SAFETY: the call only reads whether a handle names a window.
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return None;
    }
    let marks = OWN_MARKS.lock().unwrap_or_else(PoisonError::into_inner);
    marks
        .iter()
        .find(|mark| mark.hwnd == hwnd.0 as isize)
        .cloned()
}

/// A window the user would expect to find in a task switcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppWindow {
    pub id: AppWindowId,
    /// The display the window is on, or would return to when restored.
    pub display: DisplayKey,
    /// The image of the owning process, when the system allows reading it.
    pub executable: Option<PathBuf>,
    /// The identifier of the packaged (Store) application the window
    /// belongs to, if it belongs to one.
    pub app_id: Option<String>,
}

/// For how long a list of the windows is given again where Windows does
/// not report what happens to them.
const LIST_KEPT: Duration = Duration::from_millis(100);

/// For how long it is given again where Windows does, and nothing was
/// reported: what asks several times a second, as every bar does to step
/// aside for a game, then reads the windows once a second.
const LIST_KEPT_WATCHED: Duration = Duration::from_secs(1);

impl AppWindow {
    /// What starts another window of the window's application and names
    /// it when it is pinned: the packaged application, or else the program
    /// file.
    pub fn program(&self) -> Option<PathBuf> {
        if let Some(mark) = own_mark(self.id.hwnd()) {
            return Some(mark.icon);
        }
        let app = self.app_id.as_deref().map(super::packaged::app_path);
        app.or_else(|| self.executable.clone())
    }

    /// Lists `window`, a window of Tessera, among the application
    /// windows from now on, as a window of the application `app_id`
    /// whose icon is that of the file `icon`, or of Tessera's own program
    /// without one; see [`Self::enumerate`]. Only for the windows of the
    /// file manager, with `app_id` `Tessera.Files`: never for a bar, a
    /// popup or a card. The mark goes with the window.
    pub fn mark_own(window: &gpui::Window, app_id: &str, icon: Option<&Path>) {
        let Ok(surface) = NativeSurface::of(window) else {
            return;
        };
        let icon = icon
            .map(Path::to_owned)
            .or_else(|| std::env::current_exe().ok());
        let mark = OwnMark {
            hwnd: surface.hwnd.0 as isize,
            app_id: app_id.to_owned(),
            icon: icon.unwrap_or_default(),
        };
        let mut marks = OWN_MARKS.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: the call only reads whether a handle names a window.
        let alive = |other: &OwnMark| unsafe { IsWindow(Some(HWND(other.hwnd as *mut c_void))) };
        marks.retain(|other| other.hwnd != mark.hwnd && alive(other).as_bool());
        marks.push(mark);
        drop(marks);
        HAPPENED.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether the window is one of Tessera's that is listed, as those of
    /// the file manager are.
    pub fn is_own(&self) -> bool {
        own_mark(self.id.hwnd()).is_some()
    }

    /// Lists application windows in z-order, topmost first.
    ///
    /// The windows of Tessera are left out, but for those marked with
    /// [`Self::mark_own`]: the windows of the file manager, each listed
    /// with its `app_id` (`Tessera.Files`), the executable of Tessera and
    /// the icon it was marked with (see [`Self::program`]). Bars, popups,
    /// cards and every other window of Tessera are never listed, so the
    /// dock, the window switcher, tiling and the rules, which all read
    /// this list, may take every window in it for one of an application.
    ///
    /// Every bar asks at the same moment, when something happened to a
    /// window, and the answer costs a look into each process. The list
    /// read last is therefore given again for a tenth of a second, or
    /// for a second where Windows reports what happens to windows, unless
    /// something has happened to a window since.
    pub fn enumerate() -> Vec<Self> {
        static LAST: Mutex<Option<(u64, Instant, Vec<AppWindow>)>> = Mutex::new(None);
        let generation = WindowEvents::generation();
        let kept = match WindowEvents::watched() {
            true => LIST_KEPT_WATCHED,
            false => LIST_KEPT,
        };
        let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((read_at, read, windows)) = last.as_ref()
            && *read_at == generation
            && read.elapsed() < kept
        {
            return windows.clone();
        }
        let windows = Self::read();
        *last = Some((generation, Instant::now(), windows.clone()));
        windows
    }

    fn read() -> Vec<Self> {
        Self::handles()
            .into_iter()
            .map(|(hwnd, process)| Self::describe(hwnd, process))
            .collect()
    }

    /// The windows [`Self::enumerate`] lists, in z-order, topmost first,
    /// each with its process, without looking into the processes and
    /// without the list kept for the bars. No window of this process is
    /// asked anything, so a thread may call it without waiting for another
    /// thread of the shell.
    pub(super) fn handles() -> Vec<(HWND, u32)> {
        let mut handles: Vec<HWND> = Vec::new();
        // SAFETY: `collect` receives the pointer to `handles`, which outlives
        // the synchronous enumeration.
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&raw mut handles as isize));
        }
        handles.into_iter().filter_map(Self::listed).collect()
    }

    /// The process of `hwnd` if the window is one a task switcher shows.
    /// The windows of this process are left out before anything else is
    /// read: asking one of them for its text sends it a message and waits
    /// for its thread.
    fn listed(hwnd: HWND) -> Option<(HWND, u32)> {
        // SAFETY: the calls only read window state and tolerate a handle
        // whose window has since been destroyed.
        unsafe {
            let mut process = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&raw mut process));
            if process == GetCurrentProcessId() {
                // A marked window of Tessera is not asked for its title.
                let shown = IsWindowVisible(hwnd).as_bool() && !is_cloaked(hwnd);
                return (shown && own_mark(hwnd).is_some()).then_some((hwnd, process));
            }
            if !IsWindowVisible(hwnd).as_bool() || GetWindowTextLengthW(hwnd) <= 0 {
                return None;
            }
            if GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
                return None;
            }
            let extended = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if extended & WS_EX_TOOLWINDOW.0 != 0 {
                return None;
            }
            if is_cloaked(hwnd) {
                return None;
            }
            Some((hwnd, process))
        }
    }

    fn describe(hwnd: HWND, process: u32) -> Self {
        let (executable, app_id) = match own_mark(hwnd) {
            Some(mark) => (std::env::current_exe().ok(), Some(mark.app_id)),
            None => program_of(hwnd, process),
        };
        Self {
            id: AppWindowId(hwnd.0 as isize),
            display: display_of(hwnd),
            executable,
            app_id,
        }
    }
}

/// Whether Windows hides `hwnd` without its knowing, as it does the
/// windows of other virtual desktops.
fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: `cloaked` is as large as the call is told and outlives it;
    // a window that is gone makes the call fail, which reads as shown.
    let _ = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        )
    };
    cloaked != 0
}

unsafe extern "system" fn collect(hwnd: HWND, state: LPARAM) -> BOOL {
    // SAFETY: `state` is the pointer `AppWindow::enumerate` passed in.
    let handles = unsafe { &mut *(state.0 as *mut Vec<HWND>) };
    handles.push(hwnd);
    true.into()
}

fn display_of(hwnd: HWND) -> DisplayKey {
    // SAFETY: `placement` is correctly sized, and the calls only read window
    // state and tolerate a handle whose window has since been destroyed.
    unsafe {
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // A minimized window is parked off-screen; use where it restores to.
        if IsIconic(hwnd).as_bool() && GetWindowPlacement(hwnd, &raw mut placement).is_ok() {
            let display = MonitorFromRect(
                &raw const placement.rcNormalPosition,
                MONITOR_DEFAULTTONEAREST,
            );
            return DisplayKey(display.0 as isize);
        }
    }
    DisplayKey::of_window(hwnd)
}

/// The image of `process`, when the system allows reading it, and the
/// identifier of the packaged application its window `hwnd` belongs to.
/// The image of `process`, when the system allows reading it.
pub(super) fn executable_of(process: u32) -> Option<PathBuf> {
    program_of(HWND::default(), process).0
}

fn program_of(hwnd: HWND, process: u32) -> (Option<PathBuf>, Option<String>) {
    // SAFETY: the buffer and its length describe the same allocation, and the
    // process handle is closed before returning.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process) else {
            return (None, None);
        };
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        );
        let executable = result
            .ok()
            .map(|()| PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize])));
        let app_id = super::packaged::app_id_of(hwnd, handle, executable.as_deref());
        let _ = CloseHandle(handle);
        (executable, app_id)
    }
}
