// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What applications tell the taskbar about their buttons through
//! `ITaskbarList3`: progress, an overlay icon, the tooltip of the
//! picture, buttons under the picture, the part of the window the picture
//! shows, a window marked as full screen, and a button removed.
//!
//! The object `ITaskbarList` of Windows runs in the application. It finds
//! the taskbar as the first window of the class `Shell_TrayWnd` and sends
//! its calls to the window that the property `TaskbandHWND` of that
//! window names, each as a message in the range of `WM_USER` (the numbers
//! as Cairo's ManagedShell and LiteStep answer them): the window as
//! `wParam`, and a number, an icon, or memory shared with
//! `SHAllocShared` for the shell as `lParam`. `AddTab` and `DeleteTab`
//! come to the same window as the `SHELLHOOK` message, with the codes of
//! a window made and destroyed. An application starts to call only once
//! the shell has sent its window the registered message
//! `TaskbarButtonCreated`.
//!
//! [`TaskbarList::start`] makes a hidden window for this on the thread of
//! the [`ShellHost`], which never waits. As the shell of the session it
//! names that window in the property of Tessera's own `Shell_TrayWnd`
//! (made by tray.rs), sends `TaskbarButtonCreated` to every application
//! window that is shown, and keeps what it is told in a [`TaskbarState`]
//! for each window, which [`TaskbarList::generation`] tells a change of.
//! Beside Explorer the calls stay with Explorer: the property of
//! Tessera's `Shell_TrayWnd`, which lies above Explorer's, names
//! Explorer's own window of the buttons, so applications keep reaching
//! it, and nothing new is known here.
//!
//! Not reached: the buttons an application draws from an image list
//! (`ThumbBarSetImageList`), which lives in its own process, and the tabs
//! of windows with several documents (`RegisterTab` and the like), which
//! no application of today uses.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, IsWow64Process, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Shell::{SHLockShared, SHUnlockShared};
use windows::Win32::UI::WindowsAndMessaging::{
    CopyIcon, CreateWindowExW, DefWindowProcW, DestroyIcon, EnumChildWindows, FindWindowExW,
    GetClassNameW, GetPropW, GetWindowThreadProcessId, HICON, PostMessageW, RegisterClassW,
    RegisterWindowMessageW, SendNotifyMessageW, SetPropW, SetTimer, WINDOW_EX_STYLE, WM_COMMAND,
    WM_TIMER, WM_USER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, w};

use super::{AppWindow, AppWindowId, IconPixels, ScreenRect, ShellEvent, ShellHost};

/// The messages of `ITaskbarList`, as `WM_USER` plus these.
const ACTIVATE_TAB: u32 = 50;
const MARK_FULLSCREEN: u32 = 60;
const PROGRESS_VALUE: u32 = 64;
const PROGRESS_STATE: u32 = 65;
const ADD_BUTTONS: u32 = 76;
const UPDATE_BUTTONS: u32 = 77;
const IMAGE_LIST: u32 = 78;
const OVERLAY_ICON: u32 = 79;
const THUMBNAIL_TOOLTIP: u32 = 80;
const THUMBNAIL_CLIP: u32 = 81;
const OVERLAY_DESCRIPTION: u32 = 85;

/// What a progress value of `ITaskbarList3` is at the end: Windows scales
/// the two numbers of `SetProgressValue` to this.
const PROGRESS_FULL: f32 = 0xFFFE as f32;

/// The codes of the shell hook that `AddTab` and `DeleteTab` send.
const HSHELL_WINDOWCREATED: usize = 1;
const HSHELL_WINDOWDESTROYED: usize = 2;

/// The flags of `SetProgressState`.
const TBPF_INDETERMINATE: isize = 0x1;
const TBPF_NORMAL: isize = 0x2;
const TBPF_ERROR: isize = 0x4;
const TBPF_PAUSED: isize = 0x8;

/// The parts of a `THUMBBUTTON` its mask says are given.
const THB_BITMAP: u32 = 0x1;
const THB_ICON: u32 = 0x2;
const THB_TOOLTIP: u32 = 0x4;
const THB_FLAGS: u32 = 0x8;

/// The flags of a `THUMBBUTTON`.
const THBF_DISABLED: u32 = 0x1;
const THBF_DISMISSONCLICK: u32 = 0x2;
const THBF_NOBACKGROUND: u32 = 0x4;
const THBF_HIDDEN: u32 = 0x8;
const THBF_NONINTERACTIVE: u32 = 0x10;

/// What the high word of `wParam` of `WM_COMMAND` is for a click on a
/// button under the picture.
const THBN_CLICKED: usize = 0x1800;

/// The most buttons an application may put under its picture.
const MOST_BUTTONS: usize = 7;

/// The length of the tooltip of a `THUMBBUTTON`, in UTF-16 units.
const TIP_LENGTH: usize = 260;

/// The longest text read from shared memory, in UTF-16 units.
const TEXT_MOST: usize = 1024;

/// How often the property of the taskbar is checked and set again, in
/// milliseconds: tray.rs may make its window again, and Explorer may
/// restart.
const NAME_AGAIN: u32 = 3_000;
const NAME_TIMER: usize = 1;

/// The class of the hidden window that receives the calls.
const CLASS: PCWSTR = w!("Tessera.TaskBand");

/// The property of `Shell_TrayWnd` that names the window of the buttons.
const PROPERTY: PCWSTR = w!("TaskbandHWND");

/// How a window's progress is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressState {
    /// Something goes on, without a known end.
    Indeterminate,
    Normal,
    /// The work stopped with a failure.
    Error,
    /// The work waits.
    Paused,
}

/// The progress of a window, as its application set it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub state: ProgressState,
    /// From 0 to 1; meaningless while indeterminate.
    pub value: f32,
}

/// An icon an application gave, with a number that changes whenever it
/// gives another, so that a picture of it is made once.
#[derive(Clone, Debug)]
pub struct GivenIcon {
    pub pixels: IconPixels,
    pub stamp: u64,
}

/// A button an application put under the picture of its window. Its
/// icon is absent where the application gave an image of its image list,
/// which cannot be read from here.
pub type ThumbButton = ThumbButtonOf<GivenIcon>;

/// What an application told the taskbar about one of its windows.
#[derive(Clone, Debug, Default)]
pub struct TaskbarState {
    pub progress: Option<Progress>,
    pub overlay: Option<GivenIcon>,
    /// What the overlay means, for those who cannot see it.
    pub overlay_description: Option<String>,
    /// What the picture of the window is to be titled, instead of the
    /// title of the window.
    pub tooltip: Option<String>,
    pub buttons: Vec<ThumbButton>,
    /// The part of the window, in its own coordinates, that its picture
    /// is to show.
    pub clip: Option<ScreenRect>,
    /// The window says it is full screen.
    pub fullscreen: bool,
    /// `DeleteTab` removed the button of the window.
    pub removed: bool,
}

static STATES: Mutex<Option<HashMap<isize, TaskbarState>>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// The hidden window, as a number; zero until it is made.
static BAND: AtomicIsize = AtomicIsize::new(0);
static SHELL: AtomicBool = AtomicBool::new(false);
static SHELLHOOK: AtomicU32 = AtomicU32::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// The calls of `ITaskbarList3` that reach Tessera.
pub struct TaskbarList;

impl TaskbarList {
    /// Starts listening, once; `shell` says whether Tessera is the shell
    /// of the session, where it takes the calls. It returns at once.
    pub fn start(shell: bool) {
        static STARTED: AtomicBool = AtomicBool::new(false);
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        SHELL.store(shell, Ordering::Relaxed);
        ShellHost::on_host_thread(make_band);
        if !shell {
            return;
        }
        let events = ShellHost::events();
        let started = thread::Builder::new()
            .name("taskbar buttons".to_owned())
            .spawn(move || {
                let created = button_created();
                for (hwnd, _) in AppWindow::handles() {
                    announce(hwnd, created);
                }
                while let Ok(event) = events.recv_blocking() {
                    match event {
                        ShellEvent::Created(id) => announce(id.hwnd(), created),
                        ShellEvent::Destroyed(id) if !id.exists() => forget(id),
                        _ => {}
                    }
                }
            });
        if let Err(error) = started {
            report!("applications are not told that their buttons exist: {error}");
        }
    }

    /// A number that changes whenever an application told something.
    pub fn generation() -> u64 {
        GENERATION.load(Ordering::Relaxed)
    }

    /// What the application of `window` told about it; `None` where it
    /// told nothing.
    pub fn state(window: AppWindowId) -> Option<TaskbarState> {
        let states = locked(&STATES);
        states.as_ref()?.get(&window.raw()).cloned()
    }

    /// Tells the application of `window` that its button `button` under
    /// the picture was clicked, as the taskbar does.
    pub fn click(window: AppWindowId, button: u32) {
        let command = THBN_CLICKED << 16 | (button as usize & 0xFFFF);
        // SAFETY: posting to a window that has since been destroyed fails
        // without side effects.
        unsafe {
            let _ = PostMessageW(Some(window.hwnd()), WM_COMMAND, WPARAM(command), LPARAM(0));
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Changes what is known of `window` with `change`, and tells of it.
fn change(window: isize, change: impl FnOnce(&mut TaskbarState)) {
    let mut states = locked(&STATES);
    let state = states.get_or_insert_with(HashMap::new).entry(window);
    change(state.or_default());
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Forgets a window that is gone.
fn forget(window: AppWindowId) {
    let mut states = locked(&STATES);
    let removed = states
        .as_mut()
        .and_then(|states| states.remove(&window.raw()));
    if removed.is_some() {
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

/// The number of the registered message `TaskbarButtonCreated`.
fn button_created() -> u32 {
    // SAFETY: the name is a NUL-terminated literal.
    unsafe { RegisterWindowMessageW(w!("TaskbarButtonCreated")) }
}

/// Tells the application of `hwnd` that its button exists, without
/// waiting for it.
fn announce(hwnd: HWND, created: u32) {
    if created == 0 {
        return;
    }
    // SAFETY: a notification to a window that has since been destroyed
    // fails without side effects; nothing waits for the answer.
    unsafe {
        let _ = SendNotifyMessageW(hwnd, created, WPARAM(0), LPARAM(0));
    }
}

/// Makes the hidden window on the host thread and names it.
fn make_band() {
    // SAFETY: the names are NUL-terminated literals; the class and the
    // window live as long as the process.
    unsafe {
        SHELLHOOK.store(RegisterWindowMessageW(w!("SHELLHOOK")), Ordering::Relaxed);
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::Relaxed,
        );
        let class = WNDCLASSW {
            lpfnWndProc: Some(respond),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassW(&raw const class);
        // A hidden window at the top, not one only for messages: those do
        // not hear `TaskbarCreated`.
        let band = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
            CLASS,
            w!("TaskbandHWND"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        );
        let band = match band {
            Ok(band) => band,
            Err(error) => {
                report!("the calls of applications to the taskbar are not heard: {error}");
                return;
            }
        };
        BAND.store(band.0 as isize, Ordering::SeqCst);
        let _ = SetTimer(Some(band), NAME_TIMER, NAME_AGAIN, None);
    }
    name_band();
}

/// Names the window that receives the calls in the property of
/// Tessera's own `Shell_TrayWnd`: the hidden window as the shell, and
/// Explorer's window of the buttons beside it.
fn name_band() {
    let Some(tray) = own_tray() else {
        return;
    };
    let named = if SHELL.load(Ordering::Relaxed) {
        HWND(BAND.load(Ordering::SeqCst) as *mut c_void)
    } else {
        match explorer_band() {
            Some(band) => band,
            None => return,
        }
    };
    if named.is_invalid() {
        return;
    }
    // SAFETY: the property is set on a window of this process; its value
    // is a number, which nothing frees.
    unsafe {
        if GetPropW(tray, PROPERTY).0 != named.0 {
            let _ = SetPropW(tray, PROPERTY, Some(HANDLE(named.0)));
        }
    }
}

/// The `Shell_TrayWnd` of Tessera, where tray.rs made one.
fn own_tray() -> Option<HWND> {
    trays()
        .into_iter()
        .find(|&(_, own)| own)
        .map(|(tray, _)| tray)
}

/// Every window of the class `Shell_TrayWnd`, topmost first, each with
/// whether it is Tessera's.
fn trays() -> Vec<(HWND, bool)> {
    let mut found = Vec::new();
    let mut previous = None;
    // SAFETY: the class is a NUL-terminated literal, and the search resumes
    // after the window it found before; the calls only read window state.
    unsafe {
        let own = GetCurrentProcessId();
        while let Ok(tray) = FindWindowExW(None, previous, w!("Shell_TrayWnd"), PCWSTR::null()) {
            if tray.is_invalid() {
                break;
            }
            let mut process = 0;
            GetWindowThreadProcessId(tray, Some(&raw mut process));
            found.push((tray, process == own));
            previous = Some(tray);
        }
    }
    found
}

/// The window of the buttons of Explorer's taskbar: what its property
/// names, or else its child of the class `MSTaskSwWClass`.
fn explorer_band() -> Option<HWND> {
    let (tray, _) = trays().into_iter().find(|&(_, own)| !own)?;
    // SAFETY: reading a property has no preconditions.
    let named = unsafe { GetPropW(tray, PROPERTY) };
    if !named.is_invalid() && !named.0.is_null() {
        return Some(HWND(named.0));
    }
    let mut band = HWND::default();
    // SAFETY: `band` outlives the synchronous enumeration, which hands it
    // to `find_band` as its state.
    unsafe {
        let _ = EnumChildWindows(Some(tray), Some(find_band), LPARAM(&raw mut band as isize));
    }
    (!band.is_invalid()).then_some(band)
}

unsafe extern "system" fn find_band(child: HWND, state: LPARAM) -> BOOL {
    let mut name = [0u16; 32];
    // SAFETY: the buffer outlives the call, which writes no more than its
    // length.
    let length = unsafe { GetClassNameW(child, &mut name) }.max(0) as usize;
    if String::from_utf16_lossy(&name[..length]) != "MSTaskSwWClass" {
        return true.into();
    }
    // SAFETY: `state` is the pointer `explorer_band` passed in.
    unsafe { *(state.0 as *mut HWND) = child };
    false.into()
}

/// The window procedure of the hidden window, on the host thread.
unsafe extern "system" fn respond(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let respond = || {
        if message == WM_TIMER && w.0 == NAME_TIMER {
            name_band();
            return Some(LRESULT(0));
        }
        let created = TASKBAR_CREATED.load(Ordering::Relaxed);
        if message == created && created != 0 {
            name_band();
            return None;
        }
        let shellhook = SHELLHOOK.load(Ordering::Relaxed);
        if message == shellhook && shellhook != 0 {
            tab(w.0, l.0);
            return Some(LRESULT(0));
        }
        let call = Call::of(message, w.0, l.0)?;
        if SHELL.load(Ordering::Relaxed) {
            call.take();
        }
        Some(LRESULT(0))
    };
    match catch_unwind(AssertUnwindSafe(respond)) {
        Ok(Some(answer)) => return answer,
        Ok(None) => {}
        Err(_) => report!("a call of an application to the taskbar was not handled"),
    }
    // SAFETY: the arguments are passed on as they came.
    unsafe { DefWindowProcW(window, message, w, l) }
}

/// Takes `AddTab` and `DeleteTab`, which come as codes of the shell hook.
fn tab(code: usize, window: isize) {
    let removed = match code {
        HSHELL_WINDOWCREATED => false,
        HSHELL_WINDOWDESTROYED => true,
        _ => return,
    };
    change(window, |state| state.removed = removed);
}

/// A call of `ITaskbarList3`, as its message tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    Fullscreen {
        window: isize,
        on: bool,
    },
    ProgressValue {
        window: isize,
        value: isize,
    },
    ProgressState {
        window: isize,
        flags: isize,
    },
    AddButtons {
        window: isize,
        shared: isize,
    },
    UpdateButtons {
        window: isize,
        shared: isize,
    },
    Overlay {
        window: isize,
        icon: isize,
    },
    OverlayDescription {
        window: isize,
        shared: isize,
    },
    Tooltip {
        window: isize,
        shared: isize,
    },
    Clip {
        window: isize,
        shared: isize,
    },
    /// Heard, and nothing to do: activation is told by the shell hook,
    /// and an image list cannot be read from here.
    Ignored,
}

impl Call {
    /// The call `message` with `w` and `l` stands for; `None` for a
    /// message that is none.
    fn of(message: u32, w: usize, l: isize) -> Option<Self> {
        let window = w as isize;
        let call = match message.checked_sub(WM_USER)? {
            ACTIVATE_TAB | IMAGE_LIST => Self::Ignored,
            // This one has the window in `lParam`.
            MARK_FULLSCREEN => Self::Fullscreen {
                window: l,
                on: w != 0,
            },
            PROGRESS_VALUE => Self::ProgressValue { window, value: l },
            PROGRESS_STATE => Self::ProgressState { window, flags: l },
            ADD_BUTTONS => Self::AddButtons { window, shared: l },
            UPDATE_BUTTONS => Self::UpdateButtons { window, shared: l },
            OVERLAY_ICON => Self::Overlay { window, icon: l },
            OVERLAY_DESCRIPTION => Self::OverlayDescription { window, shared: l },
            THUMBNAIL_TOOLTIP => Self::Tooltip { window, shared: l },
            THUMBNAIL_CLIP => Self::Clip { window, shared: l },
            _ => return None,
        };
        Some(call)
    }

    /// Takes the call into what is known, while the application waits
    /// for the answer: what it handed over is read now.
    fn take(self) {
        match self {
            Self::Fullscreen { window, on } => change(window, |state| state.fullscreen = on),
            Self::ProgressValue { window, value } => change(window, |state| {
                state.progress = Some(valued(state.progress, value));
            }),
            Self::ProgressState { window, flags } => change(window, |state| {
                state.progress = progress_state(flags).map(|kind| Progress {
                    state: kind,
                    value: state.progress.map_or(0., |progress| progress.value),
                });
            }),
            Self::Overlay { window, icon } => {
                let overlay = given_icon(icon);
                change(window, |state| {
                    if overlay.is_none() {
                        state.overlay_description = None;
                    }
                    state.overlay = overlay;
                });
            }
            Self::OverlayDescription { window, shared } => {
                let text = shared_text(shared, window);
                change(window, |state| state.overlay_description = text);
            }
            Self::Tooltip { window, shared } => {
                let text = shared_text(shared, window);
                change(window, |state| state.tooltip = text);
            }
            Self::Clip { window, shared } => {
                let clip = with_shared(shared, window, rect_of);
                change(window, |state| state.clip = clip.flatten());
            }
            Self::AddButtons { window, shared } | Self::UpdateButtons { window, shared } => {
                let wide = !is_wow64(window);
                let given = with_shared(shared, window, |bytes| buttons_of(bytes, wide));
                let Some(given) = given else {
                    return;
                };
                let add = matches!(self, Self::AddButtons { .. });
                change(window, |state| {
                    if add {
                        state.buttons.clear();
                    }
                    merge(&mut state.buttons, given, given_icon);
                });
            }
            Self::Ignored => {}
        }
    }
}

/// The progress after `SetProgressValue` sent `value` to `before`: a
/// value shows a progress that showed none or did not know its end as
/// normal, as Windows does, and keeps a pause or a failure.
fn valued(before: Option<Progress>, value: isize) -> Progress {
    let state = match before.map(|before| before.state) {
        None | Some(ProgressState::Indeterminate) => ProgressState::Normal,
        Some(kept) => kept,
    };
    Progress {
        state,
        value: progress_value(value),
    }
}

/// The share of the work done, from the number `SetProgressValue` sends.
fn progress_value(value: isize) -> f32 {
    (value as f32 / PROGRESS_FULL).clamp(0., 1.)
}

/// How progress is shown with the flags of `SetProgressState`; `None`
/// for none.
fn progress_state(flags: isize) -> Option<ProgressState> {
    // Windows takes the strongest flag where several are given.
    if flags & TBPF_ERROR != 0 {
        Some(ProgressState::Error)
    } else if flags & TBPF_PAUSED != 0 {
        Some(ProgressState::Paused)
    } else if flags & TBPF_NORMAL != 0 {
        Some(ProgressState::Normal)
    } else if flags & TBPF_INDETERMINATE != 0 {
        Some(ProgressState::Indeterminate)
    } else {
        None
    }
}

/// A copy of the icon `icon` of an application, read while the
/// application waits, which may destroy it afterwards; `None` for none.
fn given_icon(icon: isize) -> Option<GivenIcon> {
    if icon == 0 {
        return None;
    }
    // SAFETY: an icon is a handle of the session, which any process may
    // copy; a handle that names no icon makes the copy fail. The copy is
    // destroyed once its pixels are read.
    let pixels = unsafe {
        let copy = CopyIcon(HICON(icon as *mut c_void)).ok()?;
        let pixels = IconPixels::of_icon(copy);
        let _ = DestroyIcon(copy);
        pixels
    }?;
    Some(GivenIcon {
        pixels,
        stamp: GENERATION.fetch_add(1, Ordering::Relaxed) + 1,
    })
}

/// Whether the process of `window` runs as 32 bits on 64-bit Windows,
/// so that what it shares is laid out for 32 bits.
fn is_wow64(window: isize) -> bool {
    let mut process = 0;
    // SAFETY: the calls only read the state of the window and of its
    // process; the handle is closed before returning.
    unsafe {
        GetWindowThreadProcessId(HWND(window as *mut c_void), Some(&raw mut process));
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process) else {
            return false;
        };
        let mut wow64 = BOOL(0);
        let read = IsWow64Process(handle, &raw mut wow64).is_ok();
        let _ = CloseHandle(handle);
        read && wow64.as_bool()
    }
}

/// Reads the memory `shared` that the application of `window` shared
/// with the shell, with `read`, which is given the bytes that can be
/// read there. `None` where it cannot be read.
///
/// `SHAllocShared` makes the handle for the process it is told; Windows
/// tells it the shell's, and a handle of the application's own is tried
/// as well.
fn with_shared<T>(shared: isize, window: isize, read: impl FnOnce(&[u8]) -> T) -> Option<T> {
    if shared == 0 {
        return None;
    }
    let mut owner = 0;
    // SAFETY: the calls only read which process this is and which owns
    // the window.
    let processes = unsafe {
        GetWindowThreadProcessId(HWND(window as *mut c_void), Some(&raw mut owner));
        [GetCurrentProcessId(), owner]
    };
    for process in processes.into_iter().filter(|&process| process != 0) {
        // SAFETY: a handle that names no shared memory of `process` makes
        // the call give null; what is mapped is unmapped once it is read.
        unsafe {
            let start = SHLockShared(HANDLE(shared as *mut c_void), process);
            if start.is_null() {
                continue;
            }
            let length = readable(start);
            // SAFETY: the `length` bytes from `start` lie in the mapped
            // view, as `readable` found.
            let bytes = std::slice::from_raw_parts(start.cast::<u8>(), length);
            let answer = read(bytes);
            let _ = SHUnlockShared(start);
            return Some(answer);
        }
    }
    None
}

/// How many bytes from `start` lie in the region of memory it is in.
fn readable(start: *const c_void) -> usize {
    let mut region = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: the call only describes the memory at `start` into
    // `region`, which outlives it.
    let described = unsafe {
        VirtualQuery(
            Some(start),
            &raw mut region,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if described == 0 {
        return 0;
    }
    let offset = start as usize - region.BaseAddress as usize;
    region.RegionSize.saturating_sub(offset)
}

/// The text in the memory the application shared; `None` where it is
/// empty or cannot be read.
fn shared_text(shared: isize, window: isize) -> Option<String> {
    with_shared(shared, window, text_of).flatten()
}

/// The NUL-terminated UTF-16 text at the start of `bytes`; `None` where
/// it is empty.
fn text_of(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = { bytes.chunks_exact(2).take(TEXT_MOST) }
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|&unit| unit != 0)
        .collect();
    let text = String::from_utf16_lossy(&units);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let four = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([four[0], four[1], four[2], four[3]]))
}

/// The rectangle at the start of `bytes`, as a `RECT` is laid out; `None`
/// where it is empty.
fn rect_of(bytes: &[u8]) -> Option<ScreenRect> {
    let side = |index: usize| u32_at(bytes, index * 4).map(|side| side as i32);
    let rect = ScreenRect {
        left: side(0)?,
        top: side(1)?,
        right: side(2)?,
        bottom: side(3)?,
    };
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

/// A `THUMBBUTTON` as it was read, with the handle of its icon.
#[derive(Clone, Debug, PartialEq, Eq)]
struct GivenButton {
    mask: u32,
    id: u32,
    icon: isize,
    tip: String,
    flags: u32,
}

/// The buttons in `bytes`: their count as 32 bits, then each
/// `THUMBBUTTON` as it is laid out in a process of 64 bits where `wide`,
/// and of 32 bits otherwise. At most [`MOST_BUTTONS`]; what lies beyond
/// the bytes is left out.
fn buttons_of(bytes: &[u8], wide: bool) -> Vec<GivenButton> {
    // The offsets of the icon, the tooltip and the flags, and the size.
    let (icon_at, tip_at, flags_at, size) = match wide {
        true => (16, 24, 544, 552),
        false => (12, 16, 536, 540),
    };
    let Some(count) = u32_at(bytes, 0) else {
        return Vec::new();
    };
    let count = (count as usize).min(MOST_BUTTONS);
    let button = |index: usize| -> Option<GivenButton> {
        let start = 4 + index * size;
        let one = bytes.get(start..start + size)?;
        let icon = match wide {
            true => {
                let eight = one.get(icon_at..icon_at + 8)?;
                i64::from_le_bytes(eight.try_into().ok()?) as isize
            }
            false => u32_at(one, icon_at)? as isize,
        };
        Some(GivenButton {
            mask: u32_at(one, 0)?,
            id: u32_at(one, 4)?,
            icon,
            tip: text_of(one.get(tip_at..tip_at + TIP_LENGTH * 2)?).unwrap_or_default(),
            flags: u32_at(one, flags_at)?,
        })
    };
    (0..count).map_while(button).collect()
}

/// Takes `given` into `buttons`: a button not there yet is added, and of
/// one that is, what its mask names changes. `icon_of` reads an icon.
fn merge<I>(
    buttons: &mut Vec<ThumbButtonOf<I>>,
    given: Vec<GivenButton>,
    icon_of: impl Fn(isize) -> Option<I>,
) {
    for given in given {
        let index = match buttons.iter().position(|button| button.id == given.id) {
            Some(index) => index,
            None => {
                buttons.push(ThumbButtonOf::new(given.id));
                buttons.len() - 1
            }
        };
        let button = &mut buttons[index];
        if given.mask & THB_ICON != 0 {
            button.icon = icon_of(given.icon);
        } else if given.mask & THB_BITMAP != 0 {
            button.icon = None;
        }
        if given.mask & THB_TOOLTIP != 0 {
            button.tip = given.tip;
        }
        if given.mask & THB_FLAGS != 0 {
            button.enabled = given.flags & THBF_DISABLED == 0;
            button.dismiss = given.flags & THBF_DISMISSONCLICK != 0;
            button.plain = given.flags & THBF_NOBACKGROUND != 0;
            button.hidden = given.flags & THBF_HIDDEN != 0;
            button.inert = given.flags & THBF_NONINTERACTIVE != 0;
        }
    }
}

/// A button under the picture, with an icon of the kind `I`, so that
/// merging can be tested without Windows.
#[derive(Clone, Debug, PartialEq)]
pub struct ThumbButtonOf<I> {
    /// What the application is told on a click.
    pub id: u32,
    pub icon: Option<I>,
    pub tip: String,
    pub enabled: bool,
    /// The preview closes after a click.
    pub dismiss: bool,
    /// Drawn without the shape of a button.
    pub plain: bool,
    pub hidden: bool,
    /// Shown, but a click does nothing, as for a picture of a state.
    pub inert: bool,
}

impl<I> ThumbButtonOf<I> {
    fn new(id: u32) -> Self {
        Self {
            id,
            icon: None,
            tip: String::new(),
            enabled: true,
            dismiss: false,
            plain: false,
            hidden: false,
            inert: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_messages_are_told_apart_by_their_number() {
        assert_eq!(
            Call::of(WM_USER + 64, 0x10, 0x7FFF),
            Some(Call::ProgressValue {
                window: 0x10,
                value: 0x7FFF
            })
        );
        assert_eq!(
            Call::of(WM_USER + 60, 1, 0x20),
            Some(Call::Fullscreen {
                window: 0x20,
                on: true
            })
        );
        assert_eq!(
            Call::of(WM_USER + 79, 0x10, 0x99),
            Some(Call::Overlay {
                window: 0x10,
                icon: 0x99
            })
        );
        assert_eq!(Call::of(WM_USER + 50, 0, 0x10), Some(Call::Ignored));
        assert_eq!(Call::of(WM_USER + 99, 0, 0), None);
        assert_eq!(Call::of(0x10, 0, 0), None);
    }

    #[test]
    fn progress_is_a_share_and_its_strongest_flag() {
        assert_eq!(progress_value(0), 0.);
        assert_eq!(progress_value(0xFFFE), 1.);
        assert!((progress_value(0x7FFF) - 0.5).abs() < 0.001);
        assert_eq!(progress_value(0x1_0000), 1.);
        assert_eq!(progress_state(0), None);
        assert_eq!(progress_state(1), Some(ProgressState::Indeterminate));
        assert_eq!(progress_state(2), Some(ProgressState::Normal));
        assert_eq!(progress_state(4 | 2), Some(ProgressState::Error));
        assert_eq!(progress_state(8), Some(ProgressState::Paused));
    }

    fn wide_text(text: &str, units: usize) -> Vec<u8> {
        let mut bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        bytes.resize(units * 2, 0);
        bytes
    }

    fn button_bytes(wide: bool, mask: u32, id: u32, icon: u64, tip: &str, flags: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(mask.to_le_bytes());
        bytes.extend(id.to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        if wide {
            bytes.extend(0u32.to_le_bytes());
            bytes.extend(icon.to_le_bytes());
        } else {
            bytes.extend((icon as u32).to_le_bytes());
        }
        bytes.extend(wide_text(tip, TIP_LENGTH));
        bytes.extend(flags.to_le_bytes());
        if wide {
            bytes.extend(0u32.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn a_value_ends_a_progress_without_an_end_and_keeps_a_pause() {
        let paused = Progress {
            state: ProgressState::Paused,
            value: 0.2,
        };
        let unknown = Progress {
            state: ProgressState::Indeterminate,
            value: 0.,
        };
        assert_eq!(valued(None, 0xFFFE).state, ProgressState::Normal);
        assert_eq!(valued(Some(unknown), 0).state, ProgressState::Normal);
        let kept = valued(Some(paused), 0x7FFF);
        assert_eq!(kept.state, ProgressState::Paused);
        assert!((kept.value - 0.5).abs() < 0.001);
    }

    #[test]
    fn buttons_are_read_as_either_process_lays_them_out() {
        for wide in [true, false] {
            let mut bytes = 2u32.to_le_bytes().to_vec();
            bytes.extend(button_bytes(wide, 0xF, 7, 0x1234, "Play", 0));
            bytes.extend(button_bytes(wide, 0x4, 8, 0, "Next", THBF_DISABLED));
            let buttons = buttons_of(&bytes, wide);
            assert_eq!(buttons.len(), 2, "wide: {wide}");
            assert_eq!(buttons[0].id, 7);
            assert_eq!(buttons[0].icon, 0x1234);
            assert_eq!(buttons[0].tip, "Play");
            assert_eq!(buttons[1].tip, "Next");
            assert_eq!(buttons[1].flags, THBF_DISABLED);
        }
    }

    #[test]
    fn buttons_beyond_the_bytes_are_left_out() {
        let mut bytes = 5u32.to_le_bytes().to_vec();
        bytes.extend(button_bytes(true, 0xF, 1, 0, "One", 0));
        assert_eq!(buttons_of(&bytes, true).len(), 1);
        assert!(buttons_of(&[1, 0], true).is_empty());
    }

    #[test]
    fn an_update_changes_only_what_its_mask_names() {
        let mut buttons: Vec<ThumbButtonOf<isize>> = Vec::new();
        let given = |mask, id, icon, tip: &str, flags| GivenButton {
            mask,
            id,
            icon,
            tip: tip.to_owned(),
            flags,
        };
        let icon_of = |icon: isize| (icon != 0).then_some(icon);
        merge(
            &mut buttons,
            vec![given(0xF, 1, 5, "Play", 0), given(0xF, 2, 6, "Next", 0)],
            icon_of,
        );
        merge(
            &mut buttons,
            vec![given(
                THB_TOOLTIP | THB_FLAGS,
                1,
                0,
                "Pause",
                THBF_DISMISSONCLICK,
            )],
            icon_of,
        );
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0].icon, Some(5));
        assert_eq!(buttons[0].tip, "Pause");
        assert!(buttons[0].dismiss && buttons[0].enabled);
        merge(
            &mut buttons,
            vec![given(THB_FLAGS, 2, 0, "", THBF_HIDDEN)],
            icon_of,
        );
        assert!(buttons[1].hidden);
        assert_eq!(buttons[1].tip, "Next");
    }

    #[test]
    fn shared_texts_end_at_their_first_nul() {
        assert_eq!(text_of(&wide_text("Ready", 10)).as_deref(), Some("Ready"));
        assert_eq!(text_of(&wide_text("", 4)), None);
        assert_eq!(text_of(&[]), None);
    }

    #[test]
    fn an_empty_clip_is_none() {
        let rect = |sides: [i32; 4]| -> Vec<u8> {
            sides.iter().flat_map(|side| side.to_le_bytes()).collect()
        };
        assert_eq!(
            rect_of(&rect([1, 2, 30, 40])),
            Some(ScreenRect {
                left: 1,
                top: 2,
                right: 30,
                bottom: 40
            })
        );
        assert_eq!(rect_of(&rect([5, 5, 5, 9])), None);
        assert_eq!(rect_of(&[0; 8]), None);
    }
}
