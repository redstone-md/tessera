// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The notification area: the icons applications add with
//! `Shell_NotifyIcon`.
//!
//! Applications deliver those icons to the first window of the class
//! `Shell_TrayWnd`. The shell creates such a window above Explorer's and
//! records what arrives. Beside Explorer it passes every message on to
//! Explorer, so Explorer's own notification area stays complete for when
//! the shell exits, and Explorer gives the answers.
//!
//! Where no taskbar of Explorer exists, as when Tessera is the shell of the
//! session, the window answers itself, which it decides anew with every
//! message: it accepts icons and their versions, tells applications where
//! their icons are, and serves the edge bars (see `appbar.rs`). When Explorer's
//! taskbar appears again the window goes back to passing messages on.
//!
//! Edge bars reach Explorer through the same window, the shell's own
//! surfaces among them, so their requests are passed on as well. The
//! private messages of `ITaskbarList` do not come here but to the window
//! the shell registers as the task manager.
//!
//! A message posted to an application that runs with higher privileges
//! than the shell is dropped by Windows, so such an application does not
//! hear of clicks on its icon.

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use anyhow::{Context as _, Result, anyhow};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIN_POPUPCLOSE, NIN_POPUPOPEN, SHAllocShared, SHFreeShared, SHLockShared, SHUnlockShared,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW,
    DispatchMessageW, FindWindowExW, GetClassNameW, GetCursorPos, GetMessageW,
    GetWindowThreadProcessId, HICON, HWND_BROADCAST, HWND_TOPMOST, IsWindow, MSG, MSGFLT_ALLOW,
    PostMessageW, RegisterClassW, RegisterWindowMessageW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SendMessageW, SendNotifyMessageW, SetForegroundWindow, SetTimer, SetWindowPos,
    TranslateMessage, WM_CONTEXTMENU, WM_COPYDATA, WM_DISPLAYCHANGE, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_TIMER, WM_USER,
    WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::ScreenRect;
use super::appbar::{self, Request};
use super::icon::IconPixels;

/// The class of the window Windows gives its Bluetooth icon for.
const BLUETOOTH_CLASS: &str = "BluetoothNotificationAreaIconWindowClass";

/// The class applications look for to deliver their icons.
const CLASS: PCWSTR = w!("Shell_TrayWnd");

/// Marks a `WM_COPYDATA` message that carries a notification icon.
const ICON_DATA: usize = 1;

/// Marks a `WM_COPYDATA` message that asks where an icon is, which is how
/// `Shell_NotifyIconGetRect` works.
const PLACE_DATA: usize = 3;

/// What the request for the place of an icon starts with.
const PLACE_SIGNATURE: u32 = 0x3475_3423;

/// In such a request: asks for the left and top of the icon, or for its
/// width and height. Windows asks for both and makes a rectangle of them.
const PLACE_ORIGIN: u32 = 1;
const PLACE_SIZE: u32 = 2;

/// Marks a `WM_COPYDATA` message that carries the request of an edge bar.
const EDGE_BAR_DATA: usize = 0;

/// Milliseconds between checks that the window is still found first and
/// that the owners of the icons still exist.
const UPKEEP_INTERVAL: u32 = 1000;

/// The first of the message numbers `RegisterWindowMessage` hands out.
const REGISTERED_MESSAGES: u32 = 0xC000;

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETFOCUS: u32 = 3;
const NIM_SETVERSION: u32 = 4;

const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;
const NIF_STATE: u32 = 0x08;
const NIF_GUID: u32 = 0x20;
const NIF_SHOWTIP: u32 = 0x80;

const NIS_HIDDEN: u32 = 0x01;

/// From this version on, an icon is told when it is chosen with a click or
/// with the keyboard, and when its menu is asked for.
const VERSION_3: u32 = 3;

/// From this version on, an icon is told the pointer position and its own
/// identifier with every event, and draws its own description.
const VERSION_4: u32 = 4;

/// Sent after a click on an icon of version 3 or later, in addition to the
/// button events, and after it was chosen with the keyboard.
const NIN_SELECT: u32 = WM_USER;
const NIN_KEYSELECT: u32 = WM_USER + 1;

/// Where the parts of a notification lie in its bytes: a signature and
/// the message number, then a `NOTIFYICONDATAW` whose handles are 32 bits
/// wide. The texts are of UTF-16 units, as many as given.
mod at {
    pub const MESSAGE: usize = 4;
    pub const WINDOW: usize = 12;
    pub const UID: usize = 16;
    pub const FLAGS: usize = 20;
    pub const CALLBACK: usize = 24;
    pub const ICON: usize = 28;
    pub const TIP: (usize, usize) = (32, 128);
    pub const STATE: usize = 288;
    pub const STATE_MASK: usize = 292;
    pub const VERSION: usize = 808;
    pub const GUID: usize = 944;
}

/// What an application asked of the notification area, as it arrives in a
/// `WM_COPYDATA` message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Notice {
    message: u32,
    window: u32,
    uid: u32,
    flags: u32,
    callback: u32,
    icon: u32,
    tip: String,
    state: u32,
    state_mask: u32,
    version: u32,
    guid: [u8; 16],
}

impl Notice {
    /// Reads the structure Windows sends. Applications built against old
    /// headers send a shorter one; what is missing reads as zero.
    fn parse(data: &[u8]) -> Self {
        let number = |at: usize| {
            let bytes = data.get(at..at + 4).and_then(|bytes| bytes.try_into().ok());
            bytes.map_or(0, u32::from_le_bytes)
        };
        let text = |(at, units): (usize, usize)| {
            let bytes = data.get(at..).unwrap_or_default();
            let units: Vec<u16> = bytes[..bytes.len().min(units * 2)]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .take_while(|&unit| unit != 0)
                .collect();
            String::from_utf16_lossy(&units)
        };
        let guid = data.get(at::GUID..at::GUID + 16);
        Self {
            message: number(at::MESSAGE),
            window: number(at::WINDOW),
            uid: number(at::UID),
            flags: number(at::FLAGS),
            callback: number(at::CALLBACK),
            icon: number(at::ICON),
            tip: text(at::TIP),
            state: number(at::STATE),
            state_mask: number(at::STATE_MASK),
            version: number(at::VERSION),
            guid: guid
                .and_then(|bytes| bytes.try_into().ok())
                .unwrap_or_default(),
        }
    }

    /// What tells this icon from the others: its identifier if it has a
    /// global one, otherwise its window and number.
    fn identity(&self) -> Identity {
        if self.flags & NIF_GUID != 0 {
            Identity::Guid(self.guid)
        } else {
            Identity::Numbered(self.window, self.uid)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Identity {
    Guid([u8; 16]),
    Numbered(u32, u32),
}

/// Reads the request for the place of an icon: which part of it is asked
/// for and of which icon. The structure is a signature, that part, a size
/// and padding, then the window and number of the icon and its global
/// identifier, which is all zeroes if it has none.
fn place_request(data: &[u8]) -> Option<(u32, Identity)> {
    let number = |at: usize| {
        let bytes = data.get(at..at + 4)?.try_into().ok()?;
        Some(u32::from_le_bytes(bytes))
    };
    if number(0)? != PLACE_SIGNATURE {
        return None;
    }
    let guid: [u8; 16] = data.get(24..40)?.try_into().ok()?;
    let identity = if guid == [0; 16] {
        Identity::Numbered(number(16)?, number(20)?)
    } else {
        Identity::Guid(guid)
    };
    Some((number(4)?, identity))
}

/// Where the shell shows an icon, in physical pixels of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Place {
    frame: ScreenRect,
    /// Where a menu of the icon belongs: under it, just off the bar.
    menu: (i32, i32),
}

/// Two screen coordinates as the two halves of a message result.
fn packed(x: i32, y: i32) -> isize {
    ((x as u16 as usize) | ((y as u16 as usize) << 16)) as isize
}

#[derive(Clone, Debug)]
struct Entry {
    id: u64,
    window: u32,
    uid: u32,
    callback: u32,
    version: u32,
    hidden: bool,
    tip: String,
    /// Whether an icon of version 4 asked for the description to be shown
    /// for it; otherwise it draws its own.
    show_tip: bool,
    pixels: Option<Arc<IconPixels>>,
    image_version: u64,
}

impl Entry {
    /// Whether the application draws what describes its icon, and is told
    /// when the pointer comes onto the icon and leaves it.
    fn draws_own_tip(&self) -> bool {
        self.version >= VERSION_4 && !self.show_tip
    }
}

/// The icons applications currently have in the notification area.
#[derive(Default)]
struct Icons {
    entries: HashMap<Identity, Entry>,
    /// Where the shell said it shows the icons, by their `id`.
    places: HashMap<u64, Place>,
    next_id: u64,
    /// Grows with every change.
    generation: u64,
}

impl Icons {
    /// Records `notice` and says whether it was accepted: whether the icon
    /// it names is known after it, or was before it. `load` reads the
    /// pixels of an icon handle.
    fn apply(&mut self, notice: &Notice, mut load: impl FnMut(u32) -> Option<IconPixels>) -> bool {
        let identity = notice.identity();
        match notice.message {
            // An application may modify an icon it added before the shell
            // started, so a modification of an unknown icon adds it.
            NIM_ADD | NIM_MODIFY => {
                let entry = self.entries.entry(identity).or_insert_with(|| {
                    self.next_id += 1;
                    Entry {
                        id: self.next_id,
                        window: notice.window,
                        uid: notice.uid,
                        callback: 0,
                        version: 0,
                        hidden: false,
                        tip: String::new(),
                        show_tip: false,
                        pixels: None,
                        image_version: 0,
                    }
                });
                (entry.window, entry.uid) = (notice.window, notice.uid);
                if notice.flags & NIF_MESSAGE != 0 {
                    entry.callback = notice.callback;
                }
                if notice.flags & NIF_ICON != 0 {
                    entry.pixels = load(notice.icon).map(Arc::new);
                    entry.image_version += 1;
                }
                // Whether the description is shown is said with it.
                if notice.message == NIM_ADD || notice.flags & NIF_TIP != 0 {
                    entry.show_tip = notice.flags & NIF_SHOWTIP != 0;
                }
                if notice.flags & NIF_TIP != 0 {
                    entry.tip.clone_from(&notice.tip);
                }
                if notice.flags & NIF_STATE != 0 && notice.state_mask & NIS_HIDDEN != 0 {
                    entry.hidden = notice.state & NIS_HIDDEN != 0;
                }
            }
            NIM_DELETE => {
                if self.entries.remove(&identity).is_none() {
                    return false;
                }
            }
            NIM_SETVERSION => {
                let Some(entry) = self.entries.get_mut(&identity) else {
                    return false;
                };
                if notice.version > VERSION_4 {
                    return false;
                }
                entry.version = notice.version;
            }
            // The keyboard stays where it is: the strip of icons takes no
            // focus.
            NIM_SETFOCUS => return self.entries.contains_key(&identity),
            _ => return false,
        }
        self.generation += 1;
        true
    }

    /// Forgets the icons whose window `exists` denies.
    fn prune(&mut self, exists: impl Fn(u32) -> bool) {
        let before = self.entries.len();
        self.entries.retain(|_, entry| exists(entry.window));
        if self.entries.len() != before {
            self.generation += 1;
        }
        let entries = &self.entries;
        self.places
            .retain(|id, _| entries.values().any(|entry| entry.id == *id));
    }

    /// The answer to the request for the part `corner` of the place of the
    /// icon `identity`, if the shell shows it and said where.
    fn corner(&self, corner: u32, identity: Identity) -> Option<isize> {
        let entry = self.entries.get(&identity).filter(|entry| !entry.hidden)?;
        let frame = self.places.get(&entry.id)?.frame;
        match corner {
            PLACE_ORIGIN => Some(packed(frame.left, frame.top)),
            PLACE_SIZE => Some(packed(frame.right - frame.left, frame.bottom - frame.top)),
            _ => None,
        }
    }

    /// The icons to show, oldest first.
    fn shown(&self) -> Vec<TrayIcon> {
        let mut icons: Vec<TrayIcon> = self
            .entries
            .values()
            .filter(|entry| !entry.hidden)
            .map(|entry| TrayIcon {
                id: entry.id,
                owner: entry.window,
                tip: entry.tip.clone(),
                draws_own_tip: entry.draws_own_tip(),
                pixels: entry.pixels.clone(),
                image_version: entry.image_version,
            })
            .collect();
        icons.sort_by_key(|icon| icon.id);
        icons
    }
}

/// One icon of the notification area.
#[derive(Clone, Debug)]
pub struct TrayIcon {
    /// Identifies the icon for as long as its application keeps it.
    pub id: u64,
    /// The window the application gave the icon for.
    owner: u32,
    /// What the application says about the icon; may be empty.
    pub tip: String,
    /// Whether the application shows what describes the icon itself when
    /// the pointer is on it, so the shell shows no description.
    pub draws_own_tip: bool,
    pub pixels: Option<Arc<IconPixels>>,
    /// Grows whenever the application replaces the image.
    pub image_version: u64,
}

impl TrayIcon {
    /// Whether this is the icon Windows itself shows for Bluetooth.
    pub fn is_of_bluetooth(&self) -> bool {
        let mut name = [0u16; 64];
        // SAFETY: the buffer outlives the call, which fills nothing for a
        // window that is gone.
        let length = unsafe { GetClassNameW(handle(self.owner), &mut name) }.max(0) as usize;
        String::from_utf16_lossy(&name[..length]) == BLUETOOTH_CLASS
    }
}

/// What the user did to an icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayButton {
    /// The left button was pressed and released on it.
    Left,
    /// The left button was pressed and released on it a second time, soon
    /// after the first.
    Double,
    /// The right button was pressed and released on it.
    Right,
    /// It was chosen with the keyboard.
    #[allow(dead_code, reason = "for the keys that move through the bar")]
    Keyboard,
}

/// The messages an icon of `version` is sent for `button`.
fn events(button: TrayButton, version: u32) -> &'static [u32] {
    match (button, version >= VERSION_3) {
        (TrayButton::Left, false) => &[WM_LBUTTONDOWN, WM_LBUTTONUP],
        (TrayButton::Left, true) => &[WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT],
        (TrayButton::Double, _) => &[WM_LBUTTONDBLCLK, WM_LBUTTONUP],
        (TrayButton::Right, false) => &[WM_RBUTTONDOWN, WM_RBUTTONUP],
        (TrayButton::Right, true) => &[WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU],
        (TrayButton::Keyboard, false) => &[WM_LBUTTONDOWN, WM_LBUTTONUP],
        (TrayButton::Keyboard, true) => &[NIN_KEYSELECT],
    }
}

/// The parameters of the message `event` to an icon of `version` whose
/// number is `uid`, with `point` the place it happened at. From version 4
/// on, the point comes first and the event and the number share the second;
/// before, the number comes first and the event alone second.
fn parameters(version: u32, uid: u32, event: u32, point: (i32, i32)) -> (usize, isize) {
    if version >= VERSION_4 {
        let position = (point.0 as u16 as usize) | ((point.1 as u16 as usize) << 16);
        let event = (event & 0xffff) | ((uid & 0xffff) << 16);
        (position, event as isize)
    } else {
        (uid as usize, event as isize)
    }
}

/// The application of an icon, as it is told what happened to the icon.
struct Target {
    window: HWND,
    callback: u32,
    uid: u32,
    /// The version the icon is treated as.
    version: u32,
    process: u32,
    /// Where it happened: the point under the icon where a menu of it
    /// belongs, or the pointer's if the shell never said where the icon is.
    point: POINT,
    draws_own_tip: bool,
}

impl Target {
    /// The application of the icon `id`, unless the icon is gone or never
    /// said which message it wants: an icon added before the shell started
    /// and only modified since.
    fn of(id: u64) -> Option<Self> {
        let (entry, place) = {
            let icons = icons();
            let entry = icons.entries.values().find(|entry| entry.id == id).cloned();
            (entry, icons.places.get(&id).copied())
        };
        let entry = entry.filter(|entry| entry.callback != 0)?;
        let window = handle(entry.window);
        let mut point = POINT::default();
        let mut process = 0;
        // SAFETY: both out-parameters outlive the calls, which tolerate a
        // window that has been destroyed.
        unsafe {
            match place {
                Some(place) => (point.x, point.y) = place.menu,
                None => {
                    let _ = GetCursorPos(&raw mut point);
                }
            }
            GetWindowThreadProcessId(window, Some(&raw mut process));
        }
        // Explorer's own icons were added before the shell started, so
        // their version was never told; they use the current one.
        let version = if entry.version == 0 && explorer_process() == Some(process) {
            VERSION_4
        } else {
            entry.version
        };
        Some(Self {
            window,
            callback: entry.callback,
            uid: entry.uid,
            version,
            process,
            point,
            draws_own_tip: entry.draws_own_tip(),
        })
    }

    /// Lets the application put a window in front, as it may want to
    /// after what it is told.
    fn allow_front(&self) {
        // SAFETY: the call passes a process number and fails harmlessly
        // for one that is gone.
        unsafe {
            let _ = AllowSetForegroundWindow(self.process);
        }
    }

    /// Prepares for a click: the application will want to show a menu or
    /// a window, which it may put in front only if the shell, which lets
    /// it, is in front itself. The shell may come to the front, having just
    /// been clicked. A menu that is not in front stays open when something
    /// else is clicked.
    fn bring_forward(&self) {
        let host = HOST.load(Ordering::Relaxed);
        // SAFETY: the window is the shell's own or none; the call fails
        // harmlessly for one that is gone.
        unsafe {
            let _ = SetForegroundWindow(handle(host));
        }
        self.allow_front();
    }

    /// Posts `events` to the application, in that order.
    fn tell(&self, events: &[u32]) {
        let point = (self.point.x, self.point.y);
        for &event in events {
            let (wparam, lparam) = parameters(self.version, self.uid, event, point);
            // SAFETY: posting a message to another process passes plain
            // numbers and fails harmlessly if the window is gone.
            unsafe {
                let _ = PostMessageW(
                    Some(self.window),
                    self.callback,
                    WPARAM(wparam),
                    LPARAM(lparam),
                );
            }
        }
    }
}

static ICONS: OnceLock<Mutex<Icons>> = OnceLock::new();

fn icons() -> std::sync::MutexGuard<'static, Icons> {
    let icons = ICONS.get_or_init(Mutex::default);
    icons
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The notification area of the running shell.
pub struct Tray;

impl Tray {
    /// Starts receiving icons, once; later calls report the first outcome.
    ///
    /// Applications are then told that a taskbar was created, which is what
    /// makes them add their icons again. Icons of applications that ignore
    /// that stay unknown until they change.
    pub fn start() -> Result<(), String> {
        static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
        STARTED
            .get_or_init(|| {
                let (ready, outcome) = mpsc::channel();
                std::thread::Builder::new()
                    .name("tray".to_owned())
                    .spawn(move || run(&ready))
                    .map_err(|error| format!("the thread could not be started: {error}"))?;
                let outcome = outcome
                    .recv()
                    .map_err(|_| "the thread stopped".to_owned())?;
                outcome.map_err(|error| format!("{error:#}"))
            })
            .clone()
    }

    /// Starts the tray before the shell opens its surfaces where no taskbar
    /// of Explorer exists: the surfaces reserve their screen edges through
    /// its window then, and edge bars started before the shell find it.
    /// Beside Explorer the tray starts with the first `tray` widget.
    pub fn start_before_surfaces() {
        if super::explorer_running() {
            return;
        }
        if let Err(error) = Self::start() {
            report!("edge bars and notification icons have nothing to answer them: {error}");
        }
    }

    /// The icons to show, oldest first, and a number that grows whenever
    /// they change.
    pub fn icons() -> (u64, Vec<TrayIcon>) {
        let icons = icons();
        (icons.generation, icons.shown())
    }

    /// The number that grows whenever the icons change.
    pub fn generation() -> u64 {
        icons().generation
    }

    /// Says where the shell shows the icon `id`: its `frame` on the screen
    /// and the point `menu` under it where a menu of it belongs. An
    /// application that asks Windows where its icon is gets that frame.
    pub fn locate(id: u64, frame: ScreenRect, menu: (i32, i32)) {
        icons().places.insert(id, Place { frame, menu });
    }

    /// Tells the application of the icon `id` that the pointer came onto
    /// it, or left it. One that draws its own description is told to show
    /// it there, or to take it away.
    pub fn hover(id: u64, entered: bool) {
        let Some(target) = Target::of(id) else {
            return;
        };
        match (entered, target.draws_own_tip) {
            (true, false) => target.tell(&[WM_MOUSEMOVE]),
            (true, true) => {
                target.allow_front();
                target.tell(&[WM_MOUSEMOVE, NIN_POPUPOPEN]);
            }
            (false, true) => target.tell(&[NIN_POPUPCLOSE]),
            (false, false) => {}
        }
    }

    /// Tells the application of the icon `id` that `button` was used on
    /// it. The position it is told is the point under the icon where a
    /// menu belongs, or the pointer's if the shell never said where the
    /// icon is; an application may ignore it and ask for the pointer's. An
    /// application running with higher privileges than the shell does not
    /// receive the click.
    pub fn press(id: u64, button: TrayButton) {
        let Some(target) = Target::of(id) else {
            return;
        };
        target.bring_forward();
        target.tell(events(button, target.version));
    }
}

/// The window of the shell that applications deliver their icons to; 0
/// before it exists.
static HOST: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// Whether the window answered itself at its last upkeep, as no
    /// taskbar of Explorer existed.
    static SERVING: Cell<bool> = const { Cell::new(false) };
}

/// The handle a 32-bit value of a notification stands for.
fn handle(value: u32) -> HWND {
    HWND(value as usize as *mut c_void)
}

/// The message that tells applications a taskbar was created; 0 if
/// Windows did not give one.
fn taskbar_created() -> u32 {
    static CREATED: OnceLock<u32> = OnceLock::new();
    // SAFETY: the name is a NUL-terminated literal.
    *CREATED.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

/// Tells every application that a taskbar was created, which has them add
/// their icons and register their edge bars again.
fn announce() {
    let created = taskbar_created();
    if created != 0 {
        // SAFETY: the broadcast passes no pointers and does not wait.
        unsafe {
            let _ = SendNotifyMessageW(HWND_BROADCAST, created, WPARAM(0), LPARAM(0));
        }
    }
}

/// Creates the window and serves it until the process exits.
fn run(ready: &mpsc::Sender<Result<()>>) {
    let window = match create_window() {
        Ok(window) => window,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    HOST.store(window.0 as usize as u32, Ordering::Relaxed);
    SERVING.set(explorer_tray(window).is_none());
    let _ = ready.send(Ok(()));

    // SAFETY: the window belongs to this thread, and `message` outlives
    // the loop.
    unsafe {
        SetTimer(Some(window), 1, UPKEEP_INTERVAL, None);
        announce();
        let mut message = MSG::default();
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

fn create_window() -> Result<HWND> {
    // SAFETY: the class structure and the strings outlive the calls, and
    // `receive` has the signature of a window procedure.
    unsafe {
        let module = GetModuleHandleW(None).context("the module handle could not be read")?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(receive),
            hInstance: module.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassW(&raw const class) == 0 {
            return Err(anyhow!("the window class could not be registered"));
        }
        let window = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            CLASS,
            PCWSTR::null(),
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
        .context("the window could not be created")?;
        // A shell started with higher privileges than the applications
        // still hears from them. Failing that, it hears from its equals.
        for message in [WM_COPYDATA, taskbar_created()] {
            if message != 0 {
                let _ = ChangeWindowMessageFilterEx(window, message, MSGFLT_ALLOW, None);
            }
        }
        raise(window);
        Ok(window)
    }
}

/// Puts `window` above every other one, where a search by class finds it
/// first.
fn raise(window: HWND) {
    // SAFETY: the window belongs to this thread.
    unsafe {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        let _ = SetWindowPos(window, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
    }
}

/// Explorer's taskbar: the first window of the class that is not `ours`.
fn explorer_tray(ours: HWND) -> Option<HWND> {
    let mut previous = None;
    // SAFETY: the class name is a NUL-terminated literal, and the search
    // resumes after the previously returned top-level window.
    while let Ok(window) = unsafe { FindWindowExW(None, previous, CLASS, PCWSTR::null()) } {
        if window.is_invalid() {
            break;
        }
        if window != ours {
            return Some(window);
        }
        previous = Some(window);
    }
    None
}

/// The process of Explorer's taskbar.
fn explorer_process() -> Option<u32> {
    let mut previous = None;
    // SAFETY: as in `explorer_tray`; the out-parameter outlives the call.
    while let Ok(window) = unsafe { FindWindowExW(None, previous, CLASS, PCWSTR::null()) } {
        if window.is_invalid() {
            break;
        }
        let mut process = 0;
        unsafe { GetWindowThreadProcessId(window, Some(&raw mut process)) };
        if process != 0 && process != std::process::id() {
            return Some(process);
        }
        previous = Some(window);
    }
    None
}

/// Sends Explorer the `WM_COPYDATA` message `data` came in, as `sender`
/// sent it.
///
/// The request of an edge bar can name shared memory, which its sender made
/// readable for the process of the window it found: this one. Explorer
/// cannot read it and answers such a request without doing anything, so the
/// memory is shared again with Explorer and Explorer's answer copied back.
///
/// # Safety
///
/// `data` must describe a buffer that is readable for the length it states.
unsafe fn pass_on(explorer: HWND, sender: WPARAM, data: &COPYDATASTRUCT) -> LRESULT {
    let send = |data: &COPYDATASTRUCT| {
        let data = LPARAM(std::ptr::from_ref(data) as isize);
        // SAFETY: the system copies the data to Explorer for the duration
        // of the call.
        unsafe { SendMessageW(explorer, WM_COPYDATA, Some(sender), Some(data)) }
    };
    if data.dwData != EDGE_BAR_DATA || data.lpData.is_null() {
        return send(data);
    }
    // SAFETY: the caller guarantees the buffer.
    let bytes =
        unsafe { std::slice::from_raw_parts(data.lpData.cast::<u8>(), data.cbData as usize) };
    let request = Request::parse(bytes);
    if request.shared == 0 || bytes.len() < appbar::SHARED_OWNER + 4 {
        return send(data);
    }

    // The request starts with the structure the memory holds, which states
    // its own length. That is less than `APPBARDATA` takes in a program,
    // as handles are 32 bits wide in a request, and not a byte more may be
    // written: the memory of a request from this very process is not
    // mapped but lies among its other allocations.
    let size = (request.size as usize).min(appbar::SHARED_HANDLE);
    // SAFETY: the shared memory functions accept any handle and answer
    // null for one that is not theirs. A block they hand out holds the
    // structure its sender put there, of the length that structure
    // states, and both blocks stay valid until they are unlocked below.
    unsafe {
        let ours = SHLockShared(
            HANDLE(request.shared as usize as *mut c_void),
            request.owner,
        );
        if ours.is_null() {
            return send(data);
        }
        let mut process = 0;
        GetWindowThreadProcessId(explorer, Some(&mut process));
        let shared = SHAllocShared(Some(ours.cast_const()), size as u32, process);
        if shared.is_invalid() {
            let _ = SHUnlockShared(ours);
            return send(data);
        }

        let mut forwarded = bytes.to_vec();
        forwarded[appbar::SHARED_HANDLE..appbar::SHARED_HANDLE + 8]
            .copy_from_slice(&(shared.0 as usize as u64).to_le_bytes());
        forwarded[appbar::SHARED_OWNER..appbar::SHARED_OWNER + 4]
            .copy_from_slice(&process.to_le_bytes());
        let result = send(&COPYDATASTRUCT {
            dwData: data.dwData,
            cbData: data.cbData,
            lpData: forwarded.as_mut_ptr().cast(),
        });

        let theirs = SHLockShared(shared, process);
        if !theirs.is_null() {
            std::ptr::copy_nonoverlapping(theirs.cast::<u8>(), ours.cast::<u8>(), size);
            let _ = SHUnlockShared(theirs);
        }
        let _ = SHFreeShared(shared, process);
        let _ = SHUnlockShared(ours);
        result
    }
}

/// Records the notification in `data` and says whether it was accepted.
fn record(data: &[u8]) -> bool {
    icons().apply(&Notice::parse(data), load_icon)
}

/// Keeps the window first, forgets the icons and bars of closed windows,
/// and follows Explorer's taskbar coming and going.
fn upkeep(window: HWND) {
    // SAFETY: the calls only read window state.
    unsafe {
        let first = FindWindowExW(None, None, CLASS, PCWSTR::null());
        if first.ok() != Some(window) {
            raise(window);
        }
    }
    let serving = explorer_tray(window).is_none();
    // SAFETY: the call only reads whether a window exists.
    icons().prune(|owner| unsafe { IsWindow(Some(handle(owner))) }.as_bool());

    if SERVING.replace(serving) != serving {
        if serving {
            // Explorer went: the applications that knew only Explorer
            // bring their icons and bars here.
            announce();
        } else {
            // Explorer came back and keeps the bars from now on.
            appbar::forget();
        }
    }
    if serving {
        appbar::upkeep(window);
    }
}

/// The window procedure: records icons, keeps the window first, and passes
/// what applications sent on to Explorer, or answers itself without it.
unsafe extern "system" fn receive(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_COPYDATA => {
            // SAFETY: for this message the system guarantees that `lparam`
            // points to a COPYDATASTRUCT whose buffer is readable for the
            // length it states, until the procedure returns.
            let (kind, bytes) = unsafe {
                let data = &*(lparam.0 as *const COPYDATASTRUCT);
                let bytes = (!data.lpData.is_null()).then(|| {
                    std::slice::from_raw_parts(data.lpData.cast::<u8>(), data.cbData as usize)
                });
                (data.dwData, bytes)
            };
            let explorer = explorer_tray(window);
            // What the window answers when it serves the icons itself.
            let mut own = 0;
            match (kind, bytes) {
                (ICON_DATA, Some(bytes)) => own = record(bytes).into(),
                // Explorer would answer with the place in its own, hidden
                // taskbar, where the application would then open its menu.
                (PLACE_DATA, Some(bytes)) => {
                    let corner = place_request(bytes)
                        .and_then(|(corner, identity)| icons().corner(corner, identity));
                    if let Some(corner) = corner {
                        return LRESULT(corner);
                    }
                }
                (EDGE_BAR_DATA, Some(bytes)) if explorer.is_none() => {
                    return appbar::serve(window, bytes);
                }
                _ => {}
            }
            match explorer {
                // SAFETY: `lparam` points to the COPYDATASTRUCT of this
                // message, which stays readable until the procedure returns.
                Some(explorer) => unsafe {
                    pass_on(explorer, wparam, &*(lparam.0 as *const COPYDATASTRUCT))
                },
                None => LRESULT(own),
            }
        }
        WM_TIMER => {
            upkeep(window);
            LRESULT(0)
        }
        appbar::WORK_AREAS => {
            if SERVING.get() {
                appbar::set_work_areas();
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            if SERVING.get() {
                appbar::displays_changed(window);
            }
            // SAFETY: the default procedure accepts any message of a window.
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
        // Applications also talk to the taskbar with private messages.
        // Registered messages are broadcast, so Explorer has its own copy.
        WM_USER..REGISTERED_MESSAGES => match explorer_tray(window) {
            // SAFETY: private messages are passed on as they arrived.
            Some(explorer) => unsafe {
                SendMessageW(explorer, message, Some(wparam), Some(lparam))
            },
            None => LRESULT(0),
        },
        // SAFETY: the default procedure accepts any message of a window.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// The pixels of the icon a notification names. Icon handles are valid in
/// every process of the session.
fn load_icon(icon: u32) -> Option<IconPixels> {
    (icon != 0).then(|| IconPixels::of_icon(HICON(icon as usize as *mut c_void)))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A notification as Windows sends it.
    fn bytes(message: u32, window: u32, uid: u32, flags: u32, tip: &str) -> Vec<u8> {
        let mut data = vec![0u8; 964];
        let mut put =
            |at: usize, value: u32| data[at..at + 4].copy_from_slice(&value.to_le_bytes());
        put(0, 0x3475_3423);
        put(4, message);
        put(8, 956);
        put(12, window);
        put(16, uid);
        put(20, flags);
        put(24, 0x8001);
        put(28, 77);
        for (index, unit) in tip.encode_utf16().enumerate() {
            data[32 + index * 2..34 + index * 2].copy_from_slice(&unit.to_le_bytes());
        }
        data
    }

    fn notice(message: u32, window: u32, uid: u32, flags: u32, tip: &str) -> Notice {
        Notice::parse(&bytes(message, window, uid, flags, tip))
    }

    fn pixels(_: u32) -> Option<IconPixels> {
        Some(IconPixels {
            width: 1,
            height: 1,
            bgra: vec![0; 4],
        })
    }

    #[test]
    fn a_notification_is_read_from_its_bytes() {
        let read = notice(NIM_ADD, 10, 3, NIF_MESSAGE | NIF_TIP, "Volume: 40%");
        assert_eq!(
            (
                read.message,
                read.window,
                read.uid,
                read.callback,
                read.icon
            ),
            (NIM_ADD, 10, 3, 0x8001, 77)
        );
        assert_eq!(read.tip, "Volume: 40%");
        assert_eq!(read.identity(), Identity::Numbered(10, 3));

        let mut data = bytes(NIM_ADD, 10, 3, NIF_GUID, "");
        data[944..960].copy_from_slice(&[7; 16]);
        data[808..812].copy_from_slice(&4u32.to_le_bytes());
        let read = Notice::parse(&data);
        assert_eq!(read.identity(), Identity::Guid([7; 16]));
        assert_eq!(read.version, 4);
    }

    #[test]
    fn the_place_of_an_icon_is_told_once_the_shell_said_it() {
        let mut request = Vec::new();
        for number in [PLACE_SIGNATURE, PLACE_SIZE, 0x20, 0, 7, 3] {
            request.extend_from_slice(&number.to_le_bytes());
        }
        request.extend_from_slice(&[0; 16]);
        let (corner, identity) = place_request(&request).expect("a request");
        assert_eq!((corner, identity), (PLACE_SIZE, Identity::Numbered(7, 3)));
        assert_eq!(place_request(&request[..30]), None);
        assert_eq!(place_request(&[0; 40]), None);

        let mut icons = Icons::default();
        icons.apply(&notice(NIM_ADD, 7, 3, 0, ""), pixels);
        assert_eq!(icons.corner(PLACE_SIZE, identity), None);
        let id = icons.shown()[0].id;
        let frame = ScreenRect {
            left: -40,
            top: 12,
            right: -16,
            bottom: 36,
        };
        icons.places.insert(
            id,
            Place {
                frame,
                menu: (-28, 48),
            },
        );
        assert_eq!(icons.corner(PLACE_ORIGIN, identity), Some(packed(-40, 12)));
        assert_eq!(icons.corner(PLACE_SIZE, identity), Some(packed(24, 24)));
        assert_eq!(packed(-40, 12), 0x000c_ffd8);
        icons.prune(|_| false);
        assert!(icons.places.is_empty());
    }

    #[test]
    fn a_short_notification_reads_as_zeroes() {
        let read = Notice::parse(&bytes(NIM_MODIFY, 10, 3, NIF_TIP, "Old")[..96]);
        assert_eq!(
            (read.message, read.window, read.version),
            (NIM_MODIFY, 10, 0)
        );
        assert_eq!(read.tip, "Old");
        assert_eq!(Notice::parse(&[]), Notice::default());
    }

    #[test]
    fn icons_are_added_changed_and_removed() {
        let mut icons = Icons::default();
        let flags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        icons.apply(&notice(NIM_ADD, 10, 1, flags, "First"), pixels);
        icons.apply(&notice(NIM_MODIFY, 20, 1, flags, "Second"), pixels);
        let shown = icons.shown();
        assert_eq!(shown.len(), 2);
        assert_eq!(
            (shown[0].tip.as_str(), shown[1].tip.as_str()),
            ("First", "Second")
        );
        assert!(shown[0].pixels.is_some());

        // Only what the flags name changes.
        icons.apply(&notice(NIM_MODIFY, 10, 1, NIF_TIP, "Renamed"), |_| None);
        let first = &icons.shown()[0];
        assert_eq!((first.tip.as_str(), first.image_version), ("Renamed", 1));
        assert!(first.pixels.is_some());

        let generation = icons.generation;
        icons.apply(&notice(NIM_DELETE, 10, 1, 0, ""), pixels);
        assert_eq!(icons.shown().len(), 1);
        assert!(icons.generation > generation);
    }

    #[test]
    fn hidden_icons_are_not_shown() {
        let mut icons = Icons::default();
        icons.apply(&notice(NIM_ADD, 10, 1, NIF_TIP, "Shy"), pixels);
        let mut data = bytes(NIM_MODIFY, 10, 1, NIF_STATE, "");
        data[288..292].copy_from_slice(&NIS_HIDDEN.to_le_bytes());
        data[292..296].copy_from_slice(&NIS_HIDDEN.to_le_bytes());
        icons.apply(&Notice::parse(&data), pixels);
        assert!(icons.shown().is_empty());

        data[288..292].copy_from_slice(&0u32.to_le_bytes());
        icons.apply(&Notice::parse(&data), pixels);
        assert_eq!(icons.shown().len(), 1);
    }

    #[test]
    fn icons_of_closed_windows_are_forgotten() {
        let mut icons = Icons::default();
        icons.apply(&notice(NIM_ADD, 10, 1, 0, ""), pixels);
        icons.apply(&notice(NIM_ADD, 20, 1, 0, ""), pixels);
        let generation = icons.generation;
        icons.prune(|window| window == 20);
        assert_eq!(icons.shown().len(), 1);
        assert!(icons.generation > generation);
        icons.prune(|window| window == 20);
        assert_eq!(icons.generation, generation + 1);
    }

    #[test]
    fn the_version_is_remembered_for_a_known_icon() {
        let mut icons = Icons::default();
        icons.apply(&notice(NIM_ADD, 10, 1, 0, ""), pixels);
        let mut data = bytes(NIM_SETVERSION, 10, 1, 0, "");
        data[808..812].copy_from_slice(&4u32.to_le_bytes());
        icons.apply(&Notice::parse(&data), pixels);
        assert_eq!(icons.entries[&Identity::Numbered(10, 1)].version, 4);
    }

    fn put(data: &mut [u8], at: usize, value: u32) {
        data[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn a_notification_is_accepted_for_a_known_icon() {
        let mut icons = Icons::default();
        assert!(!icons.apply(&notice(NIM_DELETE, 10, 1, 0, ""), pixels));
        assert!(!icons.apply(&notice(NIM_SETFOCUS, 10, 1, 0, ""), pixels));
        assert!(icons.apply(&notice(NIM_ADD, 10, 1, 0, ""), pixels));
        assert!(icons.apply(&notice(NIM_SETFOCUS, 10, 1, 0, ""), pixels));
        let mut data = bytes(NIM_SETVERSION, 10, 1, 0, "");
        put(&mut data, at::VERSION, 5);
        assert!(!icons.apply(&Notice::parse(&data), pixels));
        assert!(!icons.apply(&notice(9, 10, 1, 0, ""), pixels));
        assert!(icons.apply(&notice(NIM_DELETE, 10, 1, 0, ""), pixels));
    }

    #[test]
    fn an_icon_of_version_4_draws_its_own_description_unless_it_asks() {
        let mut icons = Icons::default();
        icons.apply(&notice(NIM_ADD, 10, 1, NIF_TIP, "Volume"), pixels);
        assert!(!icons.shown()[0].draws_own_tip);
        let mut data = bytes(NIM_SETVERSION, 10, 1, 0, "");
        put(&mut data, at::VERSION, 4);
        icons.apply(&Notice::parse(&data), pixels);
        assert!(icons.shown()[0].draws_own_tip);

        icons.apply(
            &notice(NIM_MODIFY, 10, 1, NIF_TIP | NIF_SHOWTIP, "Volume"),
            pixels,
        );
        assert!(!icons.shown()[0].draws_own_tip);
        // A change that does not touch the description keeps the choice.
        icons.apply(&notice(NIM_MODIFY, 10, 1, NIF_ICON, ""), pixels);
        assert!(!icons.shown()[0].draws_own_tip);
    }

    #[test]
    fn each_version_is_told_of_clicks_in_its_way() {
        assert_eq!(events(TrayButton::Left, 0), &[WM_LBUTTONDOWN, WM_LBUTTONUP]);
        assert_eq!(
            events(TrayButton::Left, 3),
            &[WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT]
        );
        assert_eq!(
            events(TrayButton::Right, 4),
            &[WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU]
        );
        assert_eq!(
            events(TrayButton::Double, 4),
            &[WM_LBUTTONDBLCLK, WM_LBUTTONUP]
        );
        assert_eq!(events(TrayButton::Keyboard, 4), &[NIN_KEYSELECT]);
        assert_eq!(
            events(TrayButton::Keyboard, 0),
            &[WM_LBUTTONDOWN, WM_LBUTTONUP]
        );

        assert_eq!(
            parameters(3, 7, WM_LBUTTONUP, (5, 6)),
            (7, WM_LBUTTONUP as isize)
        );
        let (wparam, lparam) = parameters(4, 7, WM_CONTEXTMENU, (-2, 1040));
        assert_eq!(wparam, 0x0410_fffe);
        assert_eq!(lparam, (0x0007_0000 | WM_CONTEXTMENU) as isize);
    }
}
