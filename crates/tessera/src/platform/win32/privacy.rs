// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Which applications use the microphone, the camera or the location now,
//! as the privacy icons of Explorer's notification area tell.
//!
//! Windows keeps a record of every use under
//! `Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore`,
//! of the user and of the machine: below `microphone`, `webcam` and
//! `location`, a key for each packaged application, by its package family,
//! and below `NonPackaged` one for each program, by its path with `#` for
//! each `\`. Each holds `LastUsedTimeStart` and `LastUsedTimeStop`, times
//! as FILETIME; while the use goes on, the stop is 0. A thread of its own
//! waits for the record to change (`RegNotifyChangeKeyValue`) and reads it
//! again then, so nothing is asked over and over.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE, WAIT_FAILED};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET,
    REG_NOTIFY_CHANGE_NAME, RegCloseKey, RegEnumKeyExW, RegNotifyChangeKeyValue, RegOpenKeyExW,
    RegQueryValueExW,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects};
use windows::core::{Owned, PCWSTR, PWSTR};

use super::devices::{text_in, wide};
use super::packaged::app_id_in;
use super::shortcuts::Shortcut;

/// Where Windows keeps the record of uses, below the user's and the
/// machine's root.
const CONSENT_STORE: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore";

/// The key of the programs that are not packaged.
const NON_PACKAGED: &str = "NonPackaged";

/// How long after a change the record is read, so that the changes of one
/// start or stop are read together.
const SETTLE: Duration = Duration::from_millis(150);

/// How long is waited for a change before the record is read anyway, in
/// milliseconds, in case a change was not told.
const WAIT_MS: u32 = 60_000;

/// How long the thread waits before it tries again where the record
/// cannot be watched.
const RETRY: Duration = Duration::from_secs(30);

/// What an application may use that the user is told of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    Microphone,
    Camera,
    Location,
}

impl Capability {
    pub const ALL: [Self; 3] = [Self::Microphone, Self::Camera, Self::Location];

    /// The key of the record Windows keeps of its uses.
    fn key(self) -> &'static str {
        match self {
            Self::Microphone => "microphone",
            Self::Camera => "webcam",
            Self::Location => "location",
        }
    }
}

/// An application that uses a capability now.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityUse {
    pub capability: Capability,
    /// What the application is called.
    pub app: String,
}

/// Follows which applications use what.
pub struct PrivacyWatch;

impl PrivacyWatch {
    /// Starts a thread that reads the record and reads it again whenever
    /// it changes; the receiver yields the uses there are now, first at
    /// once and then after each change of them, the newest only where
    /// several wait. Meant to be called once, by the one part of Tessera
    /// that keeps the uses.
    pub fn start() -> Receiver<Vec<CapabilityUse>> {
        let (told, uses) = async_channel::bounded(1);
        let started = thread::Builder::new()
            .name("privacy".to_owned())
            .spawn(move || watch(&told));
        if let Err(error) = started {
            report!(
                "the use of the microphone, the camera and the location is not followed: {error}"
            );
        }
        uses
    }
}

/// The body of the watching thread, for as long as anyone listens.
fn watch(told: &Sender<Vec<CapabilityUse>>) {
    // The names of packaged applications are asked of the Shell, which
    // wants COM; a thread that cannot have it names them by their package.
    // SAFETY: COM is initialized once for this thread, which keeps it.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut names = Names::default();
    let mut last = None;
    while !told.is_closed() {
        let stores = [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
            .map(|root| Key::open(root, CONSENT_STORE, true));
        // Asked for before reading, so that no change in between is missed.
        let watched: Vec<Owned<HANDLE>> = stores.iter().flatten().filter_map(Key::notify).collect();
        let uses = read(&stores, &mut names);
        if last.as_ref() != Some(&uses) {
            last = Some(uses.clone());
            if told.force_send(uses).is_err() {
                return;
            }
        }
        if watched.is_empty() {
            thread::sleep(RETRY);
            continue;
        }
        let events: Vec<HANDLE> = watched.iter().map(|event| **event).collect();
        // SAFETY: the events outlive the wait; each is set by the change
        // it was registered for.
        let waited = unsafe { WaitForMultipleObjects(&events, false, WAIT_MS) };
        if waited == WAIT_FAILED {
            thread::sleep(RETRY);
        } else {
            thread::sleep(SETTLE);
        }
    }
}

/// The uses in `stores`, each once, in order.
fn read(stores: &[Option<Key>], names: &mut Names) -> Vec<CapabilityUse> {
    let mut uses = Vec::new();
    for store in stores.iter().flatten() {
        for capability in Capability::ALL {
            let Some(record) = store.child(capability.key()) else {
                continue;
            };
            for family in record.children() {
                if family == NON_PACKAGED {
                    continue;
                }
                if record.child(&family).is_some_and(|app| app.in_use()) {
                    let app = names.of_package(&family);
                    uses.push(CapabilityUse { capability, app });
                }
            }
            let Some(programs) = record.child(NON_PACKAGED) else {
                continue;
            };
            for program in programs.children() {
                if programs.child(&program).is_some_and(|app| app.in_use()) {
                    let app = names.of_program(&program_path(&program));
                    uses.push(CapabilityUse { capability, app });
                }
            }
        }
    }
    uses.sort();
    uses.dedup();
    uses
}

/// Whether a record with these times stands for a use that goes on.
fn going_on(start: Option<u64>, stop: Option<u64>) -> bool {
    start.is_some_and(|start| start != 0) && stop == Some(0)
}

/// The path of the program a key below `NonPackaged` is named after.
fn program_path(key: &str) -> PathBuf {
    PathBuf::from(key.replace('#', "\\"))
}

/// What a package family is called where the Shell gives no name for it:
/// the last part of its name, without its publisher, as `WindowsCamera`
/// for `Microsoft.WindowsCamera_8wekyb3d8bbwe`.
fn package_short_name(family: &str) -> String {
    let name = family.split('_').next().unwrap_or(family);
    name.rsplit('.').next().unwrap_or(name).to_owned()
}

/// The program `path` names, as the user knows it: its file description,
/// or else its file name without the extension.
fn program_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned());
    description(path)
        .or(stem)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// The names of applications found so far.
#[derive(Default)]
struct Names {
    known: HashMap<String, String>,
}

impl Names {
    fn of_package(&mut self, family: &str) -> String {
        if let Some(name) = self.known.get(family) {
            return name.clone();
        }
        let prefix = format!("{family}!");
        let listed = Shortcut::packaged();
        let named = listed
            .iter()
            .find(|app| app_id_in(&app.path).is_some_and(|id| id.starts_with(&prefix)));
        let name = named.map_or_else(|| package_short_name(family), |app| app.name.clone());
        self.known.insert(family.to_owned(), name.clone());
        name
    }

    fn of_program(&mut self, path: &Path) -> String {
        let key = path.to_string_lossy().into_owned();
        let name = self.known.entry(key).or_insert_with(|| program_name(path));
        name.clone()
    }
}

/// The description in the version resource of the file `path`.
fn description(path: &Path) -> Option<String> {
    let path = wide(&path.to_string_lossy());
    let file = PCWSTR(path.as_ptr());
    // SAFETY: the path is NUL-terminated and outlives the calls; the block
    // is as long as Windows says and outlives every pointer into it, which
    // are only read within it for the lengths Windows gives.
    unsafe {
        let size = GetFileVersionInfoSizeW(file, None);
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        GetFileVersionInfoW(file, None, size, block.as_mut_ptr().cast()).ok()?;
        let query = |part: &str| -> Option<(*const u8, usize)> {
            let part = wide(part);
            let mut found = std::ptr::null_mut();
            let mut length = 0u32;
            let ok = VerQueryValueW(
                block.as_ptr().cast(),
                PCWSTR(part.as_ptr()),
                &mut found,
                &mut length,
            );
            (ok.as_bool() && !found.is_null() && length > 0)
                .then_some((found.cast_const().cast::<u8>(), length as usize))
        };
        let (languages, length) = query(r"\VarFileInfo\Translation")?;
        if length < 4 {
            return None;
        }
        let words = std::slice::from_raw_parts(languages.cast::<u16>(), 2);
        let part = format!(
            r"\StringFileInfo\{:04x}{:04x}\FileDescription",
            words[0], words[1]
        );
        let (text, length) = query(&part)?;
        // The length is of characters here, with the NUL.
        let text = std::slice::from_raw_parts(text.cast::<u16>(), length);
        let text = text_in(text).trim().to_owned();
        (!text.is_empty()).then_some(text)
    }
}

/// An open key of the registry, closed when dropped.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened here and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

impl Key {
    /// Opens `path` below `root` to read, and with `notify` to be told of
    /// its changes.
    fn open(root: HKEY, path: &str, notify: bool) -> Option<Self> {
        let path = wide(path);
        let access = if notify {
            KEY_READ | KEY_NOTIFY
        } else {
            KEY_READ
        };
        let mut key = HKEY::default();
        // SAFETY: the path is NUL-terminated and `key` outlives the call.
        let opened = unsafe { RegOpenKeyExW(root, PCWSTR(path.as_ptr()), None, access, &mut key) };
        (opened == ERROR_SUCCESS).then_some(Self(key))
    }

    fn child(&self, name: &str) -> Option<Self> {
        Self::open(self.0, name, false)
    }

    /// The names of the keys below this one.
    fn children(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut buffer = [0u16; 512];
        for index in 0.. {
            let mut length = buffer.len() as u32;
            // SAFETY: the buffer and its length in characters outlive the
            // call.
            let listed = unsafe {
                RegEnumKeyExW(
                    self.0,
                    index,
                    Some(PWSTR(buffer.as_mut_ptr())),
                    &mut length,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if listed != ERROR_SUCCESS {
                break;
            }
            names.push(String::from_utf16_lossy(&buffer[..length as usize]));
        }
        names
    }

    /// The number in the QWORD value `name`.
    fn number(&self, name: &str) -> Option<u64> {
        let name = wide(name);
        let mut number = 0u64;
        let mut size = size_of::<u64>() as u32;
        // SAFETY: the name is NUL-terminated; the number and its size
        // outlive the call.
        let read = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(name.as_ptr()),
                None,
                None,
                Some((&raw mut number).cast()),
                Some(&mut size),
            )
        };
        (read == ERROR_SUCCESS && size as usize == size_of::<u64>()).then_some(number)
    }

    /// Whether this record of an application stands for a use that goes
    /// on.
    fn in_use(&self) -> bool {
        going_on(
            self.number("LastUsedTimeStart"),
            self.number("LastUsedTimeStop"),
        )
    }

    /// An event that is set at the next change of this key or of a key
    /// below it.
    fn notify(&self) -> Option<Owned<HANDLE>> {
        // SAFETY: the event is owned and closed once; Windows sets it once
        // for the change it was registered for.
        unsafe {
            let event = Owned::new(CreateEventW(None, false, false, PCWSTR::null()).ok()?);
            let filter = REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_CHANGE_NAME;
            let asked = RegNotifyChangeKeyValue(self.0, true, filter, Some(*event), true);
            (asked == ERROR_SUCCESS).then_some(event)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_use_goes_on_while_it_started_and_did_not_stop() {
        assert!(going_on(Some(133_000_000_000_000_000), Some(0)));
        assert!(!going_on(
            Some(133_000_000_000_000_000),
            Some(133_000_000_000_000_001)
        ));
        assert!(!going_on(Some(0), Some(0)));
        assert!(!going_on(None, Some(0)));
        assert!(!going_on(Some(1), None));
    }

    #[test]
    fn a_program_is_named_after_its_path_with_hashes() {
        assert_eq!(
            program_path("C:#Program Files#Mozilla Firefox#firefox.exe"),
            PathBuf::from(r"C:\Program Files\Mozilla Firefox\firefox.exe")
        );
    }

    #[test]
    fn a_package_without_a_name_is_called_by_its_last_part() {
        assert_eq!(
            package_short_name("Microsoft.WindowsCamera_8wekyb3d8bbwe"),
            "WindowsCamera"
        );
        assert_eq!(package_short_name("Zoom_abc"), "Zoom");
        assert_eq!(package_short_name("plain"), "plain");
    }

    #[test]
    fn the_capabilities_are_read_from_their_keys() {
        let keys: Vec<&str> = Capability::ALL.iter().map(|it| it.key()).collect();
        assert_eq!(keys, ["microphone", "webcam", "location"]);
    }
}
