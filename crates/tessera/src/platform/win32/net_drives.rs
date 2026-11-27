// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Network drives and network locations, as This PC in Explorer offers
//! them: the dialogs of Windows that map a share to a letter (kept and
//! connected again at sign-in) and that disconnect one, and the network
//! locations under This PC, which are folder shortcuts in the Network
//! Shortcuts folder (`shell:NetHood`), made as the wizard of Explorer
//! makes them.
//!
//! The dialogs are modal and are shown on the shell thread, owned by its
//! hidden window, so the user interface goes on meanwhile.
//!
//! This PC lists the network locations and the drives remembered for the
//! next sign-in ([`NetworkDrives::locations`], [`NetworkDrives::remembered`]),
//! one of which that is not connected is connected again before it opens
//! ([`NetworkDrives::reconnect`]); and a bare `\\server` that the Shell
//! cannot list has its shares asked of the server itself
//! ([`NetworkDrives::shares`]).
#![allow(unsafe_code)]

use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use async_channel::Receiver;
use windows::Win32::Foundation::{
    ERROR_CANCELLED, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, HANDLE, NO_ERROR, WIN32_ERROR,
};
use windows::Win32::NetworkManagement::NetManagement::{MAX_PREFERRED_LENGTH, NetApiBufferFree};
use windows::Win32::NetworkManagement::WNet::{
    CONNDLG_PERSIST, CONNDLG_USE_MRU, CONNECT_INTERACTIVE, CONNECTDLGSTRUCTW, DISC_UPDATE_PROFILE,
    DISCDLGSTRUCTW, NETRESOURCEW, RESOURCE_REMEMBERED, RESOURCETYPE_DISK, RESOURCEUSAGE_ALL,
    WNetAddConnection3W, WNetCloseEnum, WNetConnectionDialog1W, WNetDisconnectDialog,
    WNetDisconnectDialog1W, WNetEnumResourceW, WNetGetConnectionW, WNetOpenEnumW,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM, SetFileAttributesW,
};
use windows::Win32::Storage::FileSystem::{
    NetShareEnum, SHARE_INFO_1, STYPE_DISKTREE, STYPE_MASK, STYPE_SPECIAL,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IPersistFile,
};
use windows::Win32::UI::Shell::{
    FOLDERID_NetHood, IShellLinkW, KF_FLAG_CREATE, SHGetKnownFolderPath, SHGetNameFromIDList,
    SHParseDisplayName, SIGDN_DESKTOPABSOLUTEPARSING, ShellLink,
};
use windows::core::{HSTRING, Interface, PCWSTR, PWSTR};

use super::ShellThread;

/// The class that makes a folder in Network Shortcuts a shortcut to the
/// target its `target.lnk` names.
const FOLDER_SHORTCUT: &str = "{0AFACED1-E828-11D1-9187-B532F1E9575D}";

/// What a dialog of network drives came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriveDialog {
    /// A drive was mapped (with its letter) or disconnected.
    Done(Option<char>),
    /// The user closed the dialog.
    Cancelled,
    /// Windows refused, with its reason.
    Failed(String),
}

impl DriveDialog {
    fn of(status: WIN32_ERROR, letter: Option<char>) -> Self {
        match status {
            NO_ERROR => Self::Done(letter),
            // The dialogs answer a cancel with -1, or ERROR_CANCELLED.
            WIN32_ERROR(u32::MAX) | ERROR_CANCELLED => Self::Cancelled,
            status => Self::Failed(windows::core::Error::from(status).message()),
        }
    }
}

/// A network location under This PC: a folder shortcut of the Network
/// Shortcuts folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkLocation {
    /// What it is called.
    pub name: String,
    /// Where it leads: a share (`\\server\share`), or the parsing name of
    /// a web or FTP folder.
    pub target: String,
}

/// A drive of the network remembered for the next sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RememberedDrive {
    pub letter: char,
    /// The share it is, `\\server\share`.
    pub remote: String,
    /// Whether it is connected now; one that is not shows with a red cross
    /// and is connected again before it opens.
    pub connected: bool,
}

/// The dialogs of network drives and the network locations.
pub struct NetworkDrives;

impl NetworkDrives {
    /// Shows the dialog "Map Network Drive" of Windows, with "Reconnect at
    /// sign-in" checked, and gives what came of it once it is closed.
    pub fn map() -> Receiver<DriveDialog> {
        ShellThread::call(|| {
            let mut resource = NETRESOURCEW {
                dwType: RESOURCETYPE_DISK,
                ..Default::default()
            };
            let mut dialog = CONNECTDLGSTRUCTW {
                cbStructure: size_of::<CONNECTDLGSTRUCTW>() as u32,
                hwndOwner: ShellThread::owner().unwrap_or_default(),
                lpConnRes: &raw mut resource,
                dwFlags: CONNDLG_PERSIST | CONNDLG_USE_MRU,
                dwDevNum: 0,
            };
            // SAFETY: the structure and the resource it points to outlive
            // the call, which shows a modal dialog on this thread.
            let status = unsafe { WNetConnectionDialog1W(&raw mut dialog) };
            DriveDialog::of(status, drive_letter(dialog.dwDevNum))
        })
    }

    /// Disconnects the network drive `letter`, forgetting it for the next
    /// sign-in, with the questions of Windows where files on it are open;
    /// without a letter, shows the dialog of Windows that lists the
    /// network drives to choose from.
    pub fn disconnect(letter: Option<char>) -> Receiver<DriveDialog> {
        ShellThread::call(move || {
            let owner = ShellThread::owner();
            let status = match letter {
                Some(letter) => {
                    let mut local: Vec<u16> =
                        format!("{letter}:").encode_utf16().chain([0]).collect();
                    let dialog = DISCDLGSTRUCTW {
                        cbStructure: size_of::<DISCDLGSTRUCTW>() as u32,
                        hwndOwner: owner.unwrap_or_default(),
                        lpLocalName: PWSTR(local.as_mut_ptr()),
                        lpRemoteName: PWSTR::null(),
                        dwFlags: DISC_UPDATE_PROFILE,
                    };
                    // SAFETY: the structure and the name outlive the call.
                    unsafe { WNetDisconnectDialog1W(&raw const dialog) }
                }
                // SAFETY: the call shows a modal dialog on this thread.
                None => unsafe { WNetDisconnectDialog(owner, RESOURCETYPE_DISK.0) },
            };
            DriveDialog::of(status, letter)
        })
    }

    /// Makes the network location `address` (a share, `\\server\share`, or
    /// an address of a web or FTP folder) under This PC, called `name`, or
    /// after the last part of the address. Gives the folder made. It may
    /// wait for the network, so it is not done on the thread of the user
    /// interface.
    pub fn add_location(address: &str, name: Option<&str>) -> Result<PathBuf> {
        let address = address.trim();
        if address.is_empty() {
            bail!("a network location needs an address");
        }
        let name = name
            .map(str::to_owned)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| location_name(address));
        let folder = network_shortcuts()?.join(safe_name(&name));
        if folder.exists() {
            bail!("a network location called {name} is there already");
        }
        fs::create_dir_all(&folder)
            .with_context(|| format!("{} was not made", folder.display()))?;
        let made = write_location(&folder, address);
        if made.is_err() {
            let _ = fs::remove_dir_all(&folder);
        }
        made.map(|()| folder)
    }

    /// The network locations under This PC, by name. They are read from
    /// the disk and their shortcuts, away from the user interface: it may
    /// wait.
    pub fn locations() -> Receiver<Vec<NetworkLocation>> {
        ShellThread::call_alone("network locations", || {
            let Ok(folder) = network_shortcuts() else {
                return Vec::new();
            };
            let Ok(read) = fs::read_dir(folder) else {
                return Vec::new();
            };
            let mut locations: Vec<NetworkLocation> = { read.flatten() }
                .filter(|item| item.path().is_dir())
                .filter_map(|item| {
                    let target = link_target(&item.path().join("target.lnk"))?;
                    let name = item.file_name().to_string_lossy().into_owned();
                    Some(NetworkLocation { name, target })
                })
                .collect();
            locations.sort_by_key(|location| location.name.to_lowercase());
            locations
        })
    }

    /// The drives of the network remembered for the next sign-in, with
    /// whether each is connected now, in the order of their letters.
    pub fn remembered() -> Receiver<Vec<RememberedDrive>> {
        ShellThread::call_alone("remembered drives", || {
            let mut drives = remembered_drives();
            drives.sort_by_key(|drive| drive.letter);
            drives
        })
    }

    /// Connects the remembered drive `letter` to `remote` again, asking for
    /// a password where the server wants one, as Explorer does when a
    /// drive with a red cross is opened.
    pub fn reconnect(letter: char, remote: String) -> Receiver<Result<(), String>> {
        ShellThread::call_alone("reconnect drive", move || {
            let mut local: Vec<u16> = format!("{letter}:").encode_utf16().chain([0]).collect();
            let mut remote: Vec<u16> = remote.encode_utf16().chain([0]).collect();
            let resource = NETRESOURCEW {
                dwType: RESOURCETYPE_DISK,
                lpLocalName: PWSTR(local.as_mut_ptr()),
                lpRemoteName: PWSTR(remote.as_mut_ptr()),
                ..Default::default()
            };
            let owner = super::shell_items::dialog_owner();
            // SAFETY: the resource and its names outlive the call, which
            // may show the dialog that asks for a password on this thread.
            let status = unsafe {
                WNetAddConnection3W(
                    Some(owner),
                    &raw const resource,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    CONNECT_INTERACTIVE,
                )
            };
            match DriveDialog::of(status, Some(letter)) {
                DriveDialog::Done(_) => Ok(()),
                DriveDialog::Cancelled => Err("the drive was not connected".to_owned()),
                DriveDialog::Failed(why) => Err(why),
            }
        })
    }

    /// The shares the server `server` (`\\server`) offers, as the server
    /// itself lists them, without the hidden ones (`C$`, `ADMIN$`): where
    /// the Shell cannot list a bare server. It waits for the server, so it
    /// is not done on the thread of the user interface.
    pub fn shares(server: &str) -> Result<Vec<String>> {
        let name = HSTRING::from(server.trim_end_matches('\\'));
        let mut buffer = std::ptr::null_mut::<u8>();
        let (mut read, mut total) = (0u32, 0u32);
        // SAFETY: the name outlives the call; the buffer the call
        // allocates holds `read` entries of level 1 and is freed once.
        unsafe {
            let status = NetShareEnum(
                &name,
                1,
                &raw mut buffer,
                MAX_PREFERRED_LENGTH,
                &raw mut read,
                &raw mut total,
                None,
            );
            if status != NO_ERROR.0 && status != ERROR_MORE_DATA.0 {
                bail!(
                    "{server} lists no shares: {}",
                    windows::core::Error::from(WIN32_ERROR(status)).message()
                );
            }
            let entries = std::slice::from_raw_parts(buffer.cast::<SHARE_INFO_1>(), read as usize);
            let shares = { entries.iter() }
                .filter(|share| shown_share(share.shi1_type.0))
                .filter_map(|share| share.shi1_netname.to_string().ok())
                .map(|share| format!("{}\\{share}", server.trim_end_matches('\\')))
                .collect();
            let _ = NetApiBufferFree(Some(buffer.cast_const().cast()));
            Ok(shares)
        }
    }

    /// Opens the wizard of Windows that adds a network location, where one
    /// is to be chosen rather than named.
    pub fn add_location_wizard() -> Result<()> {
        Command::new("rundll32.exe")
            .arg("shwebsvc.dll,AddNetPlaceRunDll")
            .spawn()
            .context("the wizard of network locations did not start")?;
        Ok(())
    }
}

/// Whether a share of the type `kind` is one a folder shows: a disk that
/// is not hidden.
fn shown_share(kind: u32) -> bool {
    kind & STYPE_MASK.0 == STYPE_DISKTREE.0 && kind & STYPE_SPECIAL.0 == 0
}

/// Where the shortcut `file` leads: its path, or else the parsing name of
/// the item it holds.
fn link_target(file: &Path) -> Option<String> {
    // SAFETY: the link object is made and used on this thread, which has
    // COM; the path buffer is given with its length, and the identifier
    // list the shortcut gives is freed once.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let path = HSTRING::from(file.as_os_str());
        link.cast::<IPersistFile>()
            .ok()?
            .Load(&path, Default::default())
            .ok()?;
        let mut target = [0u16; 1024];
        if link.GetPath(&mut target, std::ptr::null_mut(), 0).is_ok() {
            let length = target.iter().position(|&unit| unit == 0).unwrap_or(0);
            if length > 0 {
                return Some(String::from_utf16_lossy(&target[..length]));
            }
        }
        let list = link.GetIDList().ok()?;
        let name = SHGetNameFromIDList(list, SIGDN_DESKTOPABSOLUTEPARSING);
        CoTaskMemFree(Some(list.cast_const().cast()));
        let name = name.ok()?;
        let text = name.to_string().ok();
        CoTaskMemFree(Some(name.0.cast_const().cast()));
        text
    }
}

/// The drives of the network remembered for the next sign-in.
fn remembered_drives() -> Vec<RememberedDrive> {
    let mut drives = Vec::new();
    let mut handle = HANDLE::default();
    // SAFETY: the handle is written by the call and closed once below.
    let opened = unsafe {
        WNetOpenEnumW(
            RESOURCE_REMEMBERED,
            RESOURCETYPE_DISK,
            RESOURCEUSAGE_ALL,
            None,
            &raw mut handle,
        )
    };
    if opened != NO_ERROR {
        return drives;
    }
    // 16 KB, as the documentation of WNetEnumResource advises, in u64 for
    // the alignment of the structures written into it.
    let mut buffer = vec![0u64; 2048];
    loop {
        let mut count = u32::MAX;
        let mut size = (buffer.len() * size_of::<u64>()) as u32;
        // SAFETY: the buffer is given with its size in bytes; the call
        // writes `count` resources at its start, whose texts lie in it.
        let status = unsafe {
            WNetEnumResourceW(
                handle,
                &raw mut count,
                buffer.as_mut_ptr().cast(),
                &raw mut size,
            )
        };
        if status != NO_ERROR {
            if status != ERROR_NO_MORE_ITEMS {
                report!("the remembered network drives are not read: {}", status.0);
            }
            break;
        }
        // SAFETY: as above; the texts are read before the buffer is used
        // again.
        let resources = unsafe {
            std::slice::from_raw_parts(buffer.as_ptr().cast::<NETRESOURCEW>(), count as usize)
        };
        for resource in resources {
            // SAFETY: the texts are NUL-terminated within the buffer, or null.
            let (local, remote) = unsafe {
                (
                    resource.lpLocalName.to_string().ok(),
                    resource.lpRemoteName.to_string().ok(),
                )
            };
            let letter = { local.as_deref() }
                .and_then(|local| local.chars().next())
                .filter(char::is_ascii_alphabetic);
            if let (Some(letter), Some(remote)) = (letter, remote) {
                drives.push(RememberedDrive {
                    letter: letter.to_ascii_uppercase(),
                    remote,
                    connected: is_connected(letter),
                });
            }
        }
    }
    // SAFETY: the handle was opened above and is closed once.
    let _ = unsafe { WNetCloseEnum(handle) };
    drives
}

/// Whether the drive `letter` is connected to its share now.
fn is_connected(letter: char) -> bool {
    let local = HSTRING::from(format!("{letter}:"));
    let mut remote = [0u16; 512];
    let mut length = remote.len() as u32;
    // SAFETY: the buffer is given with its length in characters.
    let status =
        unsafe { WNetGetConnectionW(&local, Some(PWSTR(remote.as_mut_ptr())), &raw mut length) };
    status == NO_ERROR
}

/// The letter of the drive number a dialog gives, 1 for `A`.
fn drive_letter(number: u32) -> Option<char> {
    (1..=26)
        .contains(&number)
        .then(|| char::from(b'A' + (number - 1) as u8))
}

/// What a network location is called after its address: the last part
/// of the path, or the server where there is none.
fn location_name(address: &str) -> String {
    let rest = { address.split("://").nth(1) }.unwrap_or(address);
    let parts: Vec<&str> = rest
        .split(['\\', '/'])
        .filter(|part| !part.is_empty())
        .collect();
    match parts.as_slice() {
        [] => address.to_owned(),
        [server] => (*server).to_owned(),
        [server, .., last] => format!("{last} ({server})"),
    }
}

/// `name` with the characters a file name cannot hold put as `_`.
fn safe_name(name: &str) -> String {
    let safe: String = { name.chars() }
        .map(|letter| match letter {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            letter if letter.is_control() => '_',
            letter => letter,
        })
        .collect();
    safe.trim_end_matches(['.', ' ']).to_owned()
}

/// The Network Shortcuts folder of the user.
fn network_shortcuts() -> Result<PathBuf> {
    // SAFETY: the identifier is a constant; the text Windows allocates is
    // copied and freed exactly once.
    unsafe {
        let text = SHGetKnownFolderPath(&FOLDERID_NetHood, KF_FLAG_CREATE, None)
            .context("the Network Shortcuts folder was not found")?;
        let path = text.to_string().map(PathBuf::from);
        CoTaskMemFree(Some(text.0.cast()));
        Ok(path?)
    }
}

/// Makes `folder` the folder shortcut of a network location: a
/// `desktop.ini` that names the class of folder shortcuts, and a
/// `target.lnk` to `address`.
fn write_location(folder: &Path, address: &str) -> Result<()> {
    let ini = folder.join("desktop.ini");
    fs::write(
        &ini,
        format!("[.ShellClassInfo]\r\nCLSID2={FOLDER_SHORTCUT}\r\nFlags=2\r\n"),
    )
    .context("desktop.ini was not written")?;
    set_attributes(&ini, FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0)?;
    write_link(&folder.join("target.lnk"), address)?;
    // A folder is only read as a folder shortcut where it is read-only.
    set_attributes(folder, FILE_ATTRIBUTE_READONLY.0)
}

fn set_attributes(path: &Path, attributes: u32) -> Result<()> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: the path is NUL-terminated and outlives the call.
    unsafe {
        SetFileAttributesW(
            PCWSTR(wide.as_ptr()),
            windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(attributes),
        )
    }
    .with_context(|| format!("{} was not marked", path.display()))
}

/// Saves a shortcut at `file` to `address`: a share by its path, another
/// address by the item of the Shell it names.
fn write_link(file: &Path, address: &str) -> Result<()> {
    // SAFETY: COM is initialized for the calls on this thread and left as
    // it was where it was initialized already; the item list the Shell
    // allocates is freed once, after the shortcut copied it.
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let saved = (|| -> Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let target = HSTRING::from(address);
            if address.starts_with("\\\\") {
                link.SetPath(&target)?;
            } else {
                let mut list = std::ptr::null_mut();
                SHParseDisplayName(&target, None, &raw mut list, 0, None)
                    .with_context(|| format!("Windows does not know {address}"))?;
                let set = link.SetIDList(list);
                CoTaskMemFree(Some(list.cast()));
                set?;
            }
            let path = HSTRING::from(file.as_os_str());
            link.cast::<IPersistFile>()?.Save(&path, true)?;
            Ok(())
        })();
        if initialized {
            CoUninitialize();
        }
        saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dialog_names_its_drive_by_number() {
        assert_eq!(drive_letter(1), Some('A'));
        assert_eq!(drive_letter(26), Some('Z'));
        assert_eq!(drive_letter(0), None);
        assert_eq!(drive_letter(27), None);
    }

    #[test]
    fn a_location_is_named_after_its_address() {
        assert_eq!(location_name(r"\\nas\photos"), "photos (nas)");
        assert_eq!(location_name(r"\\nas"), "nas");
        assert_eq!(
            location_name("https://cloud.example.com/dav/files/"),
            "files (cloud.example.com)"
        );
        assert_eq!(safe_name("a:b?c. "), "a_b_c");
    }

    #[test]
    fn hidden_shares_and_printers_are_left_out() {
        assert!(shown_share(STYPE_DISKTREE.0));
        assert!(!shown_share(STYPE_DISKTREE.0 | STYPE_SPECIAL.0));
        // A printer queue.
        assert!(!shown_share(1));
    }

    #[test]
    fn a_cancel_is_no_failure() {
        assert_eq!(
            DriveDialog::of(WIN32_ERROR(u32::MAX), None),
            DriveDialog::Cancelled
        );
        assert_eq!(
            DriveDialog::of(NO_ERROR, Some('Z')),
            DriveDialog::Done(Some('Z'))
        );
    }
}
