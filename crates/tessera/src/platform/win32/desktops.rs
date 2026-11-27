// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The virtual desktops of Windows: how many there are, which one is
//! shown, what they are called, which window is on which, going to
//! another one, and making and removing one.
//!
//! Windows offers programs no way to ask for most of that. Explorer keeps
//! the list and the names in the registry, which is read here, and goes
//! to the desktop next to the current one, makes one and removes the
//! current one when its keyboard shortcuts are pressed, which is done here
//! with simulated keys: that works only while Explorer runs. Which desktop
//! a window is on is asked of the documented `IVirtualDesktopManager`,
//! which answers for the windows of every program; its third method, which
//! moves a window to another desktop, moves only the windows of the
//! program that calls it, so the windows of other programs cannot be sent
//! to another desktop. The undocumented interfaces of Explorer that do
//! more change with every build of Windows and are not used.
//!
//! The manager lives on a thread of its own in a single-threaded
//! apartment, so that a call that waits for Explorer holds up neither the
//! thread of GPUI nor the shell thread.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{OnceLock, mpsc};
use std::thread;

use async_channel::Receiver;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DWM_CLOAKED_SHELL, DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SAM_FLAGS, RRF_RT_REG_BINARY, RegGetValueW,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_F4, VK_LEFT, VK_LWIN, VK_RIGHT,
};
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW,
    GetWindowThreadProcessId, IsWindowVisible, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, GUID, PCWSTR, w};

use super::AppWindowId;
use super::app_windows::executable_of;
use super::registry::Key;
use super::windows_key::MADE_HERE;

/// The bytes that identify one desktop.
const ID_LENGTH: usize = 16;

/// More desktops than anyone keeps; the list is read into room for these.
const MOST: usize = 128;

/// Where Explorer keeps what it knows of the virtual desktops.
const RECORD: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VirtualDesktops";

/// The key D, which has no name among the virtual keys.
const VK_D: VIRTUAL_KEY = VIRTUAL_KEY(0x44);

/// The virtual desktops at one moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualDesktops {
    /// How many desktops there are, at least one.
    pub count: usize,
    /// Which one is shown, counted from zero.
    pub current: usize,
}

/// A window on one of the virtual desktops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopWindow {
    pub id: AppWindowId,
    /// The desktop it is on, counted from zero.
    pub desktop: usize,
    /// The program of the window, where it can be read.
    pub program: Option<PathBuf>,
}

/// What was found out about the virtual desktops at one moment, away from
/// the thread of GPUI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesktopSurvey {
    pub count: usize,
    pub current: usize,
    /// A number for each desktop that stays while it exists, made of its
    /// identifier.
    pub ids: Vec<u64>,
    /// The name of each desktop given in the settings of Windows; `None`
    /// for one Windows calls by its number.
    pub names: Vec<Option<String>>,
    /// The windows of every desktop, those that the task view of Windows
    /// shows; none where they were not asked for or Windows does not tell
    /// which desktop a window is on.
    pub windows: Vec<DesktopWindow>,
}

impl VirtualDesktops {
    /// The desktops as Explorer has them on record. Explorer records
    /// nothing until a second desktop was made, which reads as one desktop.
    pub fn now() -> Self {
        let ids = binary(w!("VirtualDesktopIDs"));
        let current = binary(w!("CurrentVirtualDesktop"));
        Self::of_record(&ids, &current)
    }

    /// The desktops of a record: `ids` is the identifiers of all desktops
    /// one after another, `current` the identifier of the one shown.
    fn of_record(ids: &[u8], current: &[u8]) -> Self {
        let ids = ids.as_chunks::<ID_LENGTH>().0;
        let found = ids.iter().position(|id| id.as_slice() == current);
        Self {
            count: ids.len().max(1),
            current: found.unwrap_or(0),
        }
    }

    /// Goes from the current desktop to the one at `index`, by pressing
    /// the shortcut for the next or the previous desktop as often as that
    /// is away. Whether Windows took every key is returned.
    pub fn show(self, index: usize) -> bool {
        let index = index.min(self.count - 1);
        let (arrow, steps) = match index.checked_sub(self.current) {
            Some(steps) => (VK_RIGHT, steps),
            None => (VK_LEFT, self.current - index),
        };
        if steps == 0 {
            return true;
        }
        press(&vec![arrow; steps])
    }

    /// Has Explorer make a new desktop and go there, by pressing its
    /// shortcut Windows+Ctrl+D. Whether Windows took every key is
    /// returned; nothing happens without Explorer.
    pub fn add() -> bool {
        press(&[VK_D])
    }

    /// Has Explorer remove the desktop shown, its windows going to the
    /// desktop beside it, by pressing its shortcut Windows+Ctrl+F4.
    /// Whether Windows took every key is returned; nothing happens without
    /// Explorer.
    pub fn remove_current() -> bool {
        press(&[VK_F4])
    }

    /// Finds out, on the thread of the desktops, how many desktops there
    /// are, what they are called and, with `windows`, which window is on
    /// which. The answer comes once; the receiver closes without one if
    /// the thread is gone.
    pub fn survey(windows: bool) -> Receiver<DesktopSurvey> {
        ask(move |manager| survey(manager.filter(|_| windows)))
    }
}

/// The desktop the window `id` is on, counted from zero: the one shown
/// for a window on every desktop, and `None` for a window Windows puts on
/// no desktop or where it does not tell. This waits for the thread of the
/// desktops, which may wait for Explorer, so it is not to be called on
/// the thread of GPUI; [`VirtualDesktops::survey`] answers without
/// waiting.
#[allow(
    dead_code,
    reason = "for the dock and the rules, which ask of one window"
)]
pub fn desktop_of(id: AppWindowId) -> Option<usize> {
    let asked = ask(move |manager| {
        let order = order();
        desktop_in(manager?, id.hwnd(), &order, current_index(&order))
    });
    asked.recv_blocking().ok().flatten()
}

/// Presses `keys` one after the other while Windows and Ctrl are held,
/// marked as made here so that the watcher of the Windows key leaves them
/// alone. Whether Windows took every key is returned.
fn press(keys: &[VIRTUAL_KEY]) -> bool {
    let mut events = vec![key(VK_LWIN, true), key(VK_CONTROL, true)];
    for &pressed in keys {
        events.extend([key(pressed, true), key(pressed, false)]);
    }
    events.extend([key(VK_CONTROL, false), key(VK_LWIN, false)]);
    // SAFETY: the events outlive the call, which copies them.
    let sent = unsafe { SendInput(&events, size_of::<INPUT>() as i32) };
    sent as usize == events.len()
}

/// A key going down if `down`, and up otherwise, marked as made here so
/// that the watcher of the Windows key leaves it alone.
fn key(key: VIRTUAL_KEY, down: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key == VK_LEFT || key == VK_RIGHT || key == VK_LWIN {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: flags,
                dwExtraInfo: MADE_HERE,
                ..Default::default()
            },
        },
    }
}

/// The bytes Explorer keeps under `name` about virtual desktops; none if
/// there is no such record.
fn binary(name: PCWSTR) -> Vec<u8> {
    let mut bytes = vec![0u8; ID_LENGTH * MOST];
    let mut size = bytes.len() as u32;
    // SAFETY: the out-parameters outlive the call, and `size` is the size
    // of what the buffer holds.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VirtualDesktops"),
            name,
            RRF_RT_REG_BINARY,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    if status.is_err() {
        return Vec::new();
    }
    bytes.truncate(size as usize);
    bytes
}

/// The identifiers of the desktops in the order Explorer shows them.
fn order() -> Vec<[u8; ID_LENGTH]> {
    let ids = binary(w!("VirtualDesktopIDs"));
    ids.as_chunks::<ID_LENGTH>().0.to_vec()
}

/// Where the shown desktop is in `order`.
fn current_index(order: &[[u8; ID_LENGTH]]) -> usize {
    let current = binary(w!("CurrentVirtualDesktop"));
    let found = order.iter().position(|id| id.as_slice() == current);
    found.unwrap_or(0)
}

/// The bytes of `guid` as the registry keeps them.
fn bytes_of(guid: &GUID) -> [u8; ID_LENGTH] {
    let mut bytes = [0u8; ID_LENGTH];
    bytes[..4].copy_from_slice(&guid.data1.to_le_bytes());
    bytes[4..6].copy_from_slice(&guid.data2.to_le_bytes());
    bytes[6..8].copy_from_slice(&guid.data3.to_le_bytes());
    bytes[8..].copy_from_slice(&guid.data4);
    bytes
}

/// The identifier `bytes` written as the name of its key, as
/// `{1A2B3C4D-...}`.
fn key_name(bytes: &[u8; ID_LENGTH]) -> String {
    let data1 = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let data2 = u16::from_le_bytes([bytes[4], bytes[5]]);
    let data3 = u16::from_le_bytes([bytes[6], bytes[7]]);
    let rest: String = bytes[10..]
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect();
    format!(
        "{{{data1:08X}-{data2:04X}-{data3:04X}-{:02X}{:02X}-{rest}}}",
        bytes[8], bytes[9]
    )
}

/// A number made of the identifier `bytes`, as [`DesktopSurvey::ids`]
/// gives it.
fn folded(bytes: &[u8; ID_LENGTH]) -> u64 {
    let mut halves = bytes.as_chunks::<8>().0.iter().copied();
    let first = halves.next().map_or(0, u64::from_le_bytes);
    let second = halves.next().map_or(0, u64::from_le_bytes);
    first ^ second
}

/// The name given to the desktop `id` in the settings of Windows.
fn desktop_name(id: &[u8; ID_LENGTH]) -> Option<String> {
    let path = format!("{RECORD}\\Desktops\\{}", key_name(id));
    let key = Key::read(HKEY_CURRENT_USER, &path, REG_SAM_FLAGS(0))?;
    let name = key.text("Name")?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// A piece of work for the thread of the desktops, given its manager, or
/// `None` where Windows made none.
type Job = Box<dyn FnOnce(Option<&IVirtualDesktopManager>) + Send>;

/// Has `work` done on the thread of the desktops and gives its answer
/// once; the receiver closes without one if the thread could not start.
fn ask<R: Send + 'static>(
    work: impl FnOnce(Option<&IVirtualDesktopManager>) -> R + Send + 'static,
) -> Receiver<R> {
    static QUEUE: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();
    let (answer, answered) = async_channel::bounded(1);
    let job: Job = Box::new(move |manager| {
        answer.try_send(work(manager)).ok();
    });
    let queue = QUEUE.get_or_init(|| {
        let (queue, jobs) = mpsc::channel();
        let started = thread::Builder::new()
            .name("desktops".to_owned())
            .spawn(move || serve(&jobs));
        match started {
            Ok(_) => Some(queue),
            Err(error) => {
                report!("the thread that asks Windows about its desktops did not start: {error}");
                None
            }
        }
    });
    if let Some(queue) = queue {
        queue.send(job).ok();
    }
    answered
}

/// Runs on the thread of the desktops: makes the manager in a
/// single-threaded apartment and does the work that arrives, in order.
fn serve(jobs: &mpsc::Receiver<Job>) {
    // SAFETY: the thread is new, so its apartment is chosen here, once.
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    // SAFETY: COM is initialized on this thread, and the manager is used
    // on it alone.
    let manager: Option<IVirtualDesktopManager> = match initialized.ok() {
        Ok(()) => unsafe { CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_ALL) }.ok(),
        Err(_) => None,
    };
    if manager.is_none() {
        report!("Windows tells nothing of which window is on which of its desktops");
    }
    while let Ok(job) = jobs.recv() {
        let done = catch_unwind(AssertUnwindSafe(|| job(manager.as_ref())));
        if done.is_err() {
            report!("asking Windows about its desktops failed");
        }
    }
}

/// What [`VirtualDesktops::survey`] finds out, on the thread of the
/// desktops; the windows only with a `manager`.
fn survey(manager: Option<&IVirtualDesktopManager>) -> DesktopSurvey {
    let order = order();
    let current = current_index(&order);
    let windows = match manager {
        Some(manager) => windows_of(manager, &order, current),
        None => Vec::new(),
    };
    DesktopSurvey {
        count: order.len().max(1),
        current,
        ids: order.iter().map(folded).collect(),
        names: order.iter().map(desktop_name).collect(),
        windows,
    }
}

/// The desktop of the window `hwnd` in `order`, where `current` is shown.
fn desktop_in(
    manager: &IVirtualDesktopManager,
    hwnd: HWND,
    order: &[[u8; ID_LENGTH]],
    current: usize,
) -> Option<usize> {
    // SAFETY: the manager belongs to this thread, and a window that is
    // gone makes the calls fail.
    unsafe {
        if manager
            .IsWindowOnCurrentVirtualDesktop(hwnd)
            .ok()?
            .as_bool()
        {
            return Some(current);
        }
        let desktop = bytes_of(&manager.GetWindowDesktopId(hwnd).ok()?);
        order.iter().position(|id| *id == desktop)
    }
}

/// The windows the task view of Windows shows, on every desktop, with
/// the desktop of each.
fn windows_of(
    manager: &IVirtualDesktopManager,
    order: &[[u8; ID_LENGTH]],
    current: usize,
) -> Vec<DesktopWindow> {
    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY: `collect` receives the pointer to `handles`, which outlives
    // the synchronous enumeration.
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&raw mut handles as isize));
    }
    let mut windows = Vec::new();
    for hwnd in handles {
        let Some((process, cloaked)) = candidate(hwnd) else {
            continue;
        };
        let Some(desktop) = desktop_in(manager, hwnd, order, current) else {
            continue;
        };
        // A window Windows hides on the desktop shown is no window of it.
        if cloaked && desktop == current {
            continue;
        }
        windows.push(DesktopWindow {
            id: AppWindowId::from_raw(hwnd.0 as isize),
            desktop,
            program: executable_of(process),
        });
    }
    windows
}

/// The process of `hwnd`, and whether Windows hides it as a window of
/// another desktop, if it is a window a task switcher shows on some
/// desktop: shown, with a title, owned by no window, no tool window, of
/// another process, and hidden by nothing but the desktops.
fn candidate(hwnd: HWND) -> Option<(u32, bool)> {
    // SAFETY: the calls only read window state and tolerate a handle
    // whose window has since been destroyed.
    unsafe {
        let mut process = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&raw mut process));
        if process == GetCurrentProcessId() {
            return None;
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
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        );
        match cloaked {
            0 => Some((process, false)),
            DWM_CLOAKED_SHELL => Some((process, true)),
            _ => None,
        }
    }
}

unsafe extern "system" fn collect(hwnd: HWND, state: LPARAM) -> BOOL {
    // SAFETY: `state` is the pointer `windows_of` passed in.
    let handles = unsafe { &mut *(state.0 as *mut Vec<HWND>) };
    handles.push(hwnd);
    true.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(desktops: &[u8]) -> Vec<u8> {
        desktops
            .iter()
            .flat_map(|&desktop| [desktop; ID_LENGTH])
            .collect()
    }

    #[test]
    fn the_shown_desktop_is_found_in_the_list() {
        let desktops = VirtualDesktops::of_record(&ids(&[7, 8, 9]), &ids(&[8]));
        assert_eq!(
            desktops,
            VirtualDesktops {
                count: 3,
                current: 1
            }
        );
        let last = VirtualDesktops::of_record(&ids(&[7, 8, 9]), &ids(&[9]));
        assert_eq!((last.count, last.current), (3, 2));
    }

    #[test]
    fn no_record_is_one_desktop() {
        let desktops = VirtualDesktops::of_record(&[], &[]);
        assert_eq!((desktops.count, desktops.current), (1, 0));
    }

    #[test]
    fn an_unknown_shown_desktop_counts_as_the_first() {
        let desktops = VirtualDesktops::of_record(&ids(&[7, 8]), &ids(&[5]));
        assert_eq!((desktops.count, desktops.current), (2, 0));
    }

    #[test]
    fn an_identifier_is_written_as_the_registry_names_its_key() {
        let guid = GUID::from_u128(0x1a2b3c4d_5e6f_7081_92a3_b4c5d6e7f809);
        let bytes = bytes_of(&guid);
        assert_eq!(bytes[0], 0x4d, "the first part is little-endian");
        assert_eq!(key_name(&bytes), "{1A2B3C4D-5E6F-7081-92A3-B4C5D6E7F809}");
    }

    #[test]
    fn the_number_of_a_desktop_is_made_of_all_of_its_identifier() {
        let one = [1u8; ID_LENGTH];
        let mut other = one;
        other[15] = 2;
        assert_ne!(folded(&one), folded(&other));
        assert_eq!(folded(&one), folded(&one));
    }
}
