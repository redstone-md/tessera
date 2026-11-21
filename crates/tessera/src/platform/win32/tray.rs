// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The notification area: the icons applications add with
//! `Shell_NotifyIcon`.
//!
//! Applications deliver those icons to the first window of the class
//! `Shell_TrayWnd`. The shell creates such a window above Explorer's,
//! records what arrives and passes every message on to Explorer unchanged,
//! so Explorer's own notification area stays complete for when the shell
//! exits.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use anyhow::{Context as _, Result, anyhow};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowExW,
    GetCursorPos, GetMessageW, GetWindowThreadProcessId, HICON, HWND_BROADCAST, HWND_TOPMOST,
    IsWindow, MSG, PostMessageW, RegisterClassW, RegisterWindowMessageW, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SendMessageW, SendNotifyMessageW, SetTimer, SetWindowPos,
    TranslateMessage, WM_CONTEXTMENU, WM_COPYDATA, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_TIMER, WM_USER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::icon::IconPixels;

/// The class applications look for to deliver their icons.
const CLASS: PCWSTR = w!("Shell_TrayWnd");

/// Marks a `WM_COPYDATA` message that carries a notification icon.
const ICON_DATA: usize = 1;

/// Milliseconds between checks that the window is still found first and
/// that the owners of the icons still exist.
const UPKEEP_INTERVAL: u32 = 1000;

/// The first of the message numbers `RegisterWindowMessage` hands out.
const REGISTERED_MESSAGES: u32 = 0xC000;

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETVERSION: u32 = 4;

const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;
const NIF_STATE: u32 = 0x08;
const NIF_GUID: u32 = 0x20;

const NIS_HIDDEN: u32 = 0x01;

/// From this version on, an icon is told the pointer position and its own
/// identifier with every event.
const VERSION_4: u32 = 4;

/// Sent to version 4 icons after a click, in addition to the button events.
const NIN_SELECT: u32 = WM_USER;

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
    /// Reads the structure Windows sends: a signature and the message
    /// number, then a `NOTIFYICONDATAW` whose handles are 32 bits wide.
    /// Applications built against old headers send a shorter one; what is
    /// missing reads as zero.
    fn parse(data: &[u8]) -> Self {
        let number = |at: usize| {
            let bytes = data.get(at..at + 4).and_then(|bytes| bytes.try_into().ok());
            bytes.map_or(0, u32::from_le_bytes)
        };
        let tip = data.get(32..).unwrap_or_default();
        let tip: Vec<u16> = tip[..tip.len().min(256)]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .take_while(|&unit| unit != 0)
            .collect();
        let guid = data.get(944..960).and_then(|bytes| bytes.try_into().ok());
        Self {
            message: number(4),
            window: number(12),
            uid: number(16),
            flags: number(20),
            callback: number(24),
            icon: number(28),
            tip: String::from_utf16_lossy(&tip),
            state: number(288),
            state_mask: number(292),
            version: number(808),
            guid: guid.unwrap_or_default(),
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

#[derive(Clone, Debug)]
struct Entry {
    id: u64,
    window: u32,
    uid: u32,
    callback: u32,
    version: u32,
    hidden: bool,
    tip: String,
    pixels: Option<Arc<IconPixels>>,
    image_version: u64,
}

/// The icons applications currently have in the notification area.
#[derive(Default)]
struct Icons {
    entries: HashMap<Identity, Entry>,
    next_id: u64,
    /// Grows with every change.
    generation: u64,
}

impl Icons {
    /// Records `notice`. `load` reads the pixels of an icon handle.
    fn apply(&mut self, notice: &Notice, load: impl FnOnce(u32) -> Option<IconPixels>) {
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
                if notice.flags & NIF_TIP != 0 {
                    entry.tip.clone_from(&notice.tip);
                }
                if notice.flags & NIF_STATE != 0 && notice.state_mask & NIS_HIDDEN != 0 {
                    entry.hidden = notice.state & NIS_HIDDEN != 0;
                }
            }
            NIM_DELETE => {
                self.entries.remove(&identity);
            }
            NIM_SETVERSION => {
                if let Some(entry) = self.entries.get_mut(&identity) {
                    entry.version = notice.version;
                }
            }
            _ => return,
        }
        self.generation += 1;
    }

    /// Forgets the icons whose window `exists` denies.
    fn prune(&mut self, exists: impl Fn(u32) -> bool) {
        let before = self.entries.len();
        self.entries.retain(|_, entry| exists(entry.window));
        if self.entries.len() != before {
            self.generation += 1;
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
                tip: entry.tip.clone(),
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
    /// What the application says about the icon; may be empty.
    pub tip: String,
    pub pixels: Option<Arc<IconPixels>>,
    /// Grows whenever the application replaces the image.
    pub image_version: u64,
}

/// A mouse button pressed and released on an icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayButton {
    Left,
    Right,
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

    /// Tells the application of the icon `id` that `button` was clicked on
    /// it, at the pointer's position. An application running with higher
    /// privileges than the shell does not receive that.
    pub fn press(id: u64, button: TrayButton) {
        let entry = icons()
            .entries
            .values()
            .find(|entry| entry.id == id)
            .cloned();
        let Some(entry) = entry else {
            return;
        };
        let window = handle(entry.window);
        let mut pointer = POINT::default();
        let mut process = 0;
        // SAFETY: both out-parameters outlive the calls, which tolerate a
        // window that has been destroyed.
        unsafe {
            let _ = GetCursorPos(&raw mut pointer);
            GetWindowThreadProcessId(window, Some(&raw mut process));
            // The application will want to show a menu or a window.
            let _ = AllowSetForegroundWindow(process);
        }

        let events: &[u32] = match (button, entry.version >= VERSION_4) {
            (TrayButton::Left, false) => &[WM_LBUTTONDOWN, WM_LBUTTONUP],
            (TrayButton::Left, true) => &[WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT],
            (TrayButton::Right, false) => &[WM_RBUTTONDOWN, WM_RBUTTONUP],
            (TrayButton::Right, true) => &[WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU],
        };
        for &event in events {
            let (wparam, lparam) = if entry.version >= VERSION_4 {
                let position = (pointer.x as u16 as usize) | ((pointer.y as u16 as usize) << 16);
                (position, (event & 0xffff) | ((entry.uid & 0xffff) << 16))
            } else {
                (entry.uid as usize, event)
            };
            // SAFETY: posting a message to another process passes plain
            // numbers and fails harmlessly if the window is gone.
            unsafe {
                let _ = PostMessageW(
                    Some(window),
                    entry.callback,
                    WPARAM(wparam),
                    LPARAM(lparam as isize),
                );
            }
        }
    }
}

/// The handle a 32-bit value of a notification stands for.
fn handle(value: u32) -> HWND {
    HWND(value as usize as *mut c_void)
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
    let _ = ready.send(Ok(()));

    // SAFETY: the window belongs to this thread; `message` outlives the
    // loop, and the broadcast passes no pointers.
    unsafe {
        SetTimer(Some(window), 1, UPKEEP_INTERVAL, None);
        let created = RegisterWindowMessageW(w!("TaskbarCreated"));
        if created != 0 {
            let _ = SendNotifyMessageW(HWND_BROADCAST, created, WPARAM(0), LPARAM(0));
        }
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

/// The window procedure: records icons, keeps the window first, and passes
/// what applications sent on to Explorer.
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
            let notice = unsafe {
                let data = &*(lparam.0 as *const COPYDATASTRUCT);
                (data.dwData == ICON_DATA && !data.lpData.is_null()).then(|| {
                    let bytes =
                        std::slice::from_raw_parts(data.lpData.cast::<u8>(), data.cbData as usize);
                    Notice::parse(bytes)
                })
            };
            if let Some(notice) = notice {
                icons().apply(&notice, load_icon);
            }
            match explorer_tray(window) {
                // SAFETY: the system copies the data to Explorer for the
                // duration of the call, as it did for this window.
                Some(explorer) => unsafe {
                    SendMessageW(explorer, message, Some(wparam), Some(lparam))
                },
                None => LRESULT(1),
            }
        }
        WM_TIMER => {
            // SAFETY: the calls only read window state.
            unsafe {
                let first = FindWindowExW(None, None, CLASS, PCWSTR::null());
                if first.ok() != Some(window) {
                    raise(window);
                }
                icons().prune(|owner| IsWindow(Some(handle(owner))).as_bool());
            }
            LRESULT(0)
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
}
