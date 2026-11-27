// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the file manager asks of Windows: the drives, what a file is
//! (hidden or not), the clipboard of files that Explorer shares, and the
//! short form of the operations of the Shell on files ([`FileWork`]), which
//! bring its own progress and conflict dialogs, put what is deleted into the
//! Recycle Bin and can be undone.
#![allow(unsafe_code)]

use std::ffi::OsString;
use std::fs::Metadata;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, POINT};
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::Shell::{
    DROPFILES, DragQueryFileW, HDROP, SHOP_FILEPATH, SHObjectProperties,
};
use windows::core::{BOOL, PCWSTR, w};

use super::FileOperation;

/// The attributes of a file that is hidden, or that belongs to the system.
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

/// The clipboard format of a list of files.
const CF_HDROP: u32 = 15;

/// What `Preferred DropEffect` says of files on the clipboard.
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;

/// What Windows calls a kind of drive.
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;

/// Whether the file whose metadata is `metadata` is hidden in Explorer
/// unless it is asked to show hidden files.
pub fn is_hidden(metadata: &Metadata) -> bool {
    metadata.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
}

/// What kind of drive a drive is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Optical,
}

/// A drive with a letter.
#[derive(Clone, Debug)]
pub struct Drive {
    /// Its root, as `C:\`.
    pub root: PathBuf,
    /// The name of its volume; empty where it has none.
    pub label: String,
    pub kind: DriveKind,
    /// Its size and what of it is free, in bytes, where it can be read.
    pub total: Option<u64>,
    pub free: Option<u64>,
}

impl Drive {
    /// The drives with a letter, in the order of their letters. Reading
    /// them can wait for a network or a disc, so it is done away from the
    /// user interface.
    pub fn all() -> Vec<Self> {
        // SAFETY: the call takes no arguments.
        let letters = unsafe { GetLogicalDrives() };
        (0..26u8)
            .filter(|bit| letters & (1 << bit) != 0)
            .map(|bit| Self::of(char::from(b'A' + bit)))
            .collect()
    }

    fn of(letter: char) -> Self {
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
        let root_name = PCWSTR(wide.as_ptr());
        // SAFETY: the root is NUL-terminated and outlives the call.
        let kind = match unsafe { GetDriveTypeW(root_name) } {
            DRIVE_REMOVABLE => DriveKind::Removable,
            DRIVE_REMOTE => DriveKind::Network,
            DRIVE_CDROM => DriveKind::Optical,
            _ => DriveKind::Fixed,
        };
        let mut name = [0u16; 261];
        // SAFETY: the root is NUL-terminated, and the buffer carries its
        // length; every other part is left out.
        let named =
            unsafe { GetVolumeInformationW(root_name, Some(&mut name), None, None, None, None) };
        let label = match named {
            Ok(()) => text_in(&name),
            Err(_) => String::new(),
        };
        let space = disk_space(Path::new(&root));
        Self {
            root: PathBuf::from(root),
            label,
            kind,
            total: space.map(|(total, _)| total),
            free: space.map(|(_, free)| free),
        }
    }

    /// What the drive is called: its label and its letter.
    pub fn title(&self) -> String {
        let letter = self.root.to_string_lossy();
        let letter = letter.trim_end_matches('\\');
        match self.label.is_empty() {
            true => letter.to_owned(),
            false => format!("{} ({letter})", self.label),
        }
    }
}

/// The size of the drive `path` lies on and what of it is free to the
/// user, in bytes.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: the path is NUL-terminated and both numbers outlive the call.
    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            Some(&raw mut free),
            Some(&raw mut total),
            None,
        )
    }
    .ok()?;
    Some((total, free))
}

/// The text in a NUL-terminated buffer.
fn text_in(buffer: &[u16]) -> String {
    let length = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
}

/// A list of paths as the Shell takes it: each NUL-terminated, and the
/// list ended by one NUL more.
fn path_list<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Vec<u16> {
    let mut list = Vec::new();
    for path in paths {
        list.extend(path.as_os_str().encode_wide());
        list.push(0);
    }
    list.push(0);
    list
}

/// What the Shell does with files: each operation shows the progress,
/// asks about conflicts and goes into the [`UndoJournal`](super::UndoJournal) as Explorer's
/// own do. It is the short form of a [`FileOperation`] of one kind.
pub enum FileWork {
    /// Into the Recycle Bin.
    Recycle(Vec<PathBuf>),
    /// Gone for good, after the Shell asks.
    Erase(Vec<PathBuf>),
    Copy {
        from: Vec<PathBuf>,
        to: PathBuf,
    },
    Move {
        from: Vec<PathBuf>,
        to: PathBuf,
    },
    /// The file or folder at `path` gets the name `name`.
    Rename {
        path: PathBuf,
        name: String,
    },
}

impl FileWork {
    /// The operation that does the work.
    pub fn operation(self) -> FileOperation {
        let operation = FileOperation::new();
        match self {
            Self::Recycle(paths) => operation.recycle(paths),
            Self::Erase(paths) => operation.erase(paths),
            Self::Copy { from, to } => operation.copy(from, &to),
            Self::Move { from, to } => operation.move_(from, &to),
            Self::Rename { path, name } => operation.rename(&path, name),
        }
    }

    /// Has the Shell do the work on a thread of its own, and `done` called
    /// on that thread after it, whatever came of it. A failure is
    /// reported.
    pub fn start(self, done: impl FnOnce() + Send + 'static) {
        self.operation().then(move |outcome| {
            outcome.report();
            done();
        });
    }
}

/// Opens the dialog of the properties of the file or folder at `path`.
pub fn show_properties(path: &Path) -> Result<()> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: the path is NUL-terminated and outlives the call, which
    // shows the dialog on a thread of the Shell's own.
    let shown =
        unsafe { SHObjectProperties(None, SHOP_FILEPATH, PCWSTR(wide.as_ptr()), PCWSTR::null()) };
    if !shown.as_bool() {
        bail!("Windows shows no properties of {}", path.display());
    }
    Ok(())
}

/// Opens a terminal in `folder`: Windows Terminal where it is installed,
/// or else the command prompt.
pub fn open_terminal(folder: &Path) -> Result<()> {
    let terminal = Command::new("wt.exe").arg("-d").arg(folder).spawn();
    if terminal.is_ok() {
        return Ok(());
    }
    Command::new("cmd.exe")
        .args(["/c", "start", "cmd.exe", "/k", "cd", "/d"])
        .arg(folder)
        .spawn()?;
    Ok(())
}

/// Has Explorer open a window with `path` selected in its folder.
pub fn reveal_in_explorer(path: &Path) -> Result<()> {
    let mut argument = OsString::from("/select,");
    argument.push(path.as_os_str());
    Command::new("explorer.exe").arg(argument).spawn()?;
    Ok(())
}

/// The clipboard of files, which Explorer and other programs share: what
/// one cuts or copies there, another pastes.
pub struct FileClipboard;

impl FileClipboard {
    /// Puts `paths` on the clipboard, to be moved when `cut`.
    pub fn put(paths: &[PathBuf], cut: bool) -> Result<()> {
        let list = path_list(paths.iter().map(PathBuf::as_path));
        let header = size_of::<DROPFILES>();
        let bytes = header + list.len() * size_of::<u16>();
        // SAFETY: the memory is allocated with the size written to it,
        // locked while it is written and handed to the clipboard, which
        // owns it from then on; the clipboard is closed on every path.
        unsafe {
            let files = GlobalAlloc(GHND, bytes)?;
            let start = GlobalLock(files).cast::<u8>();
            if start.is_null() {
                bail!("no memory for the clipboard");
            }
            let drop = DROPFILES {
                pFiles: header as u32,
                pt: POINT::default(),
                fNC: BOOL(0),
                fWide: BOOL(1),
            };
            start.cast::<DROPFILES>().write_unaligned(drop);
            std::ptr::copy_nonoverlapping(
                list.as_ptr().cast::<u8>(),
                start.add(header),
                list.len() * size_of::<u16>(),
            );
            let _ = GlobalUnlock(files);
            let effect = GlobalAlloc(GHND, size_of::<u32>())?;
            let value = GlobalLock(effect).cast::<u32>();
            if !value.is_null() {
                value.write_unaligned(if cut {
                    DROPEFFECT_MOVE
                } else {
                    DROPEFFECT_COPY
                });
                let _ = GlobalUnlock(effect);
            }
            OpenClipboard(None)?;
            let result = (|| -> Result<()> {
                EmptyClipboard()?;
                SetClipboardData(CF_HDROP, Some(HANDLE(files.0)))?;
                SetClipboardData(drop_effect(), Some(HANDLE(effect.0)))?;
                Ok(())
            })();
            let _ = CloseClipboard();
            result
        }
    }

    /// The files on the clipboard, and whether they were cut; `None`
    /// where it holds none.
    pub fn take() -> Option<(Vec<PathBuf>, bool)> {
        // SAFETY: the clipboard is opened and closed around the reading;
        // the memory it owns is only read, while it is locked.
        unsafe {
            IsClipboardFormatAvailable(CF_HDROP).ok()?;
            OpenClipboard(None).ok()?;
            let files = GetClipboardData(CF_HDROP).ok().map(|handle| {
                let drop = HDROP(handle.0);
                let count = DragQueryFileW(drop, u32::MAX, None);
                (0..count)
                    .filter_map(|index| {
                        let length = DragQueryFileW(drop, index, None) as usize;
                        let mut name = vec![0u16; length + 1];
                        let written = DragQueryFileW(drop, index, Some(&mut name)) as usize;
                        (written > 0).then(|| PathBuf::from(OsString::from_wide(&name[..written])))
                    })
                    .collect::<Vec<_>>()
            });
            let cut = GetClipboardData(drop_effect()).ok().is_some_and(|handle| {
                let memory = HGLOBAL(handle.0);
                let value = GlobalLock(memory).cast::<u32>();
                let effect = if value.is_null() {
                    0
                } else {
                    value.read_unaligned()
                };
                let _ = GlobalUnlock(memory);
                effect & DROPEFFECT_MOVE != 0
            });
            let _ = CloseClipboard();
            files
                .filter(|files| !files.is_empty())
                .map(|files| (files, cut))
        }
    }
}

/// The format that tells whether files on the clipboard were cut.
fn drop_effect() -> u32 {
    // SAFETY: the name is a NUL-terminated constant.
    unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_list_ends_with_two_nuls() {
        let list = path_list([Path::new("a"), Path::new("bc")]);
        assert_eq!(list, [97, 0, 98, 99, 0, 0]);
        assert_eq!(path_list(std::iter::empty()), [0]);
    }

    #[test]
    fn a_drive_is_titled_by_its_label_and_letter() {
        let drive = Drive {
            root: PathBuf::from("D:\\"),
            label: "Data".to_owned(),
            kind: DriveKind::Fixed,
            total: None,
            free: None,
        };
        assert_eq!(drive.title(), "Data (D:)");
        let bare = Drive {
            label: String::new(),
            ..drive
        };
        assert_eq!(bare.title(), "D:");
    }
}
