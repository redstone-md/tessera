// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! "Send to": the places Explorer offers to send files to, and sending
//! them there.
//!
//! The places are the items of the Send To folder of the user
//! (`shell:sendto`): shortcuts to programs and folders, and the files that
//! stand for a handler of the Shell, such as `Compressed (zipped)
//! Folder.ZFSendToTarget` or `Desktop (create shortcut).DeskLink`; then
//! the removable drives. Sending is what Explorer does: the files are
//! dropped on the place, as a data object handed to its drop target
//! (`IDropTarget`), so a program gets them on its command line, a folder
//! or a drive copies them, and a handler does its own work. A drop that
//! copies, on a drive, a folder or a shortcut, runs on a thread of its
//! own, as the copy may take long and wait for the user in its dialogs;
//! a drop on a handler runs on the [`ShellThread`], which keeps handling
//! messages for the work and the dialogs it goes on with after the drop.
//!
//! ```ignore
//! let targets = SendToTarget::all();
//! cx.spawn(async move |cx| {
//!     let targets = targets.recv().await.unwrap_or_default();
//!     // ... show them; once one is chosen:
//!     targets[0].send(paths);
//! }).detach();
//! ```
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use async_channel::Receiver;
use windows::Win32::Foundation::POINTL;
use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
use windows::Win32::System::Ole::{DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, IDropTarget};
use windows::Win32::System::SystemServices::{MK_LBUTTON, SFGAO_HIDDEN};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_SendTo, IEnumShellItems, IShellItem, SHCreateItemFromParsingName,
    SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY,
};
use windows::core::{HSTRING, PCWSTR};

use super::ShellThread;
use super::shell_item::IdList;
use super::shell_items::{
    data_object, dialog_owner, known_folder, take_path, take_text, windows_path,
};

/// What `GetDriveTypeW` answers for a removable drive.
const DRIVE_REMOVABLE: u32 = 2;

/// What a place to send files to is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendToKind {
    /// An item of the Send To folder.
    Item,
    /// A removable drive.
    Drive,
}

/// A place to send files to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendToTarget {
    /// What Explorer calls it: the name of the item without its extension,
    /// or the name of the drive with its letter.
    pub name: String,
    /// The item in the Send To folder, or the root of the drive; its icon
    /// is the one the Shell draws for it.
    pub path: PathBuf,
    pub kind: SendToKind,
}

impl SendToTarget {
    /// The places, the items of the Send To folder by name, then the
    /// removable drives by letter. The receiver yields once; a failure to
    /// read the folder is reported, and the drives come anyway.
    pub fn all() -> Receiver<Vec<Self>> {
        ShellThread::call(|| {
            let mut targets = items().unwrap_or_else(|error| {
                report!("the Send To folder could not be read: {error:#}");
                Vec::new()
            });
            sort_by_name(&mut targets);
            targets.extend(drives());
            targets
        })
    }

    /// Sends the files at `paths` there, and tells how it went: the
    /// receiver yields once. A handler may go on with its work, as making
    /// an archive, after it has answered.
    pub fn send(&self, paths: Vec<PathBuf>) -> Receiver<Result<(), String>> {
        let target = self.clone();
        let work = move || {
            target
                .drop_here(&paths)
                .map_err(|error| format!("{error:#}"))
        };
        if self.copies() {
            ShellThread::call_alone("send to", work)
        } else {
            ShellThread::call(work)
        }
    }

    /// Whether a drop here is done once the drop returns, as for a copy:
    /// on a drive, a folder, or a shortcut, which hands the files to the
    /// folder or the program it points to. A handler of the Shell may go
    /// on with its work on the thread of the drop afterwards.
    fn copies(&self) -> bool {
        self.kind == SendToKind::Drive || self.path.is_dir() || { self.path.extension() }
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
    }

    /// Drops `paths` on the place, on this thread, as a press of the left
    /// button lets them go: copied or linked, as the place chooses.
    fn drop_here(&self, paths: &[PathBuf]) -> Result<()> {
        let data = data_object(paths)?;
        let id = IdList::of_path(Path::new(&windows_path(&self.path)))?;
        let allowed = DROPEFFECT_COPY | DROPEFFECT_LINK;
        let keys = MK_LBUTTON;
        let at = POINTL::default();
        let (parent, child) = id.bind_to_parent().context("the place is not reachable")?;
        // SAFETY: the item identifier lists outlive the calls; the child
        // is a part of `id`, which the parent folder does not keep. COM
        // is initialized on this thread.
        unsafe {
            let target: IDropTarget = parent
                .GetUIObjectOf(dialog_owner(), &[child], None)
                .with_context(|| format!("{} takes no files", self.name))?;
            let mut effect = allowed;
            target.DragEnter(&data, keys, at, &raw mut effect)?;
            if effect != DROPEFFECT::default() {
                effect = allowed;
                target.DragOver(keys, at, &raw mut effect)?;
            }
            if effect == DROPEFFECT::default() {
                let _ = target.DragLeave();
                bail!("{} takes none of these files", self.name);
            }
            let mut effect = allowed;
            target
                .Drop(&data, keys, at, &raw mut effect)
                .with_context(|| format!("{} did not take the files", self.name))
        }
    }
}

/// The places in alphabetical order, as Explorer lists them.
fn sort_by_name(targets: &mut [SendToTarget]) {
    targets.sort_by_cached_key(|target| target.name.to_lowercase());
}

/// The items of the Send To folder, but for hidden ones such as
/// `desktop.ini`.
fn items() -> Result<Vec<SendToTarget>> {
    let folder = known_folder(&FOLDERID_SendTo)?;
    let name = HSTRING::from(folder.as_os_str());
    let mut targets = Vec::new();
    // SAFETY: the name outlives the call; COM is initialized on this
    // thread; the slot has room for the one item asked for at a time.
    unsafe {
        let folder: IShellItem = SHCreateItemFromParsingName(&name, None)?;
        let listed: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
        loop {
            let mut slot = [None];
            let mut fetched = 0u32;
            let next = listed.Next(&mut slot, Some(&raw mut fetched));
            let item = match (next, slot) {
                (Ok(()), [Some(item)]) if fetched == 1 => item,
                _ => break,
            };
            let hidden = item
                .GetAttributes(SFGAO_HIDDEN)
                .is_ok_and(|attributes| attributes.contains(SFGAO_HIDDEN));
            if hidden {
                continue;
            }
            let name = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok();
            let path = item.GetDisplayName(SIGDN_FILESYSPATH).ok();
            if let (Some(name), Some(path)) = (
                name.and_then(|name| take_text(name)),
                path.and_then(|path| take_path(path)),
            ) {
                targets.push(SendToTarget {
                    name,
                    path,
                    kind: SendToKind::Item,
                });
            }
        }
    }
    Ok(targets)
}

/// The removable drives with a letter, named as Explorer names them.
fn drives() -> Vec<SendToTarget> {
    // SAFETY: the call only reads which letters name drives.
    let letters = unsafe { GetLogicalDrives() };
    removable_letters(letters, |root| {
        let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
        // SAFETY: the root is NUL-terminated; the call only reads what
        // kind of drive it names, and waits for no disc.
        unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) == DRIVE_REMOVABLE }
    })
    .into_iter()
    .map(|root| SendToTarget {
        name: drive_name(&root).unwrap_or_else(|| root.trim_end_matches('\\').to_owned()),
        path: PathBuf::from(root),
        kind: SendToKind::Drive,
    })
    .collect()
}

/// The roots (`E:\`) of the drives whose bits are set in `letters`, as
/// `GetLogicalDrives` gives them, that `removable` says are removable.
fn removable_letters(letters: u32, removable: impl Fn(&str) -> bool) -> Vec<String> {
    { (0..26u8).filter(|bit| letters & (1 << bit) != 0) }
        .map(|bit| format!("{}:\\", char::from(b'A' + bit)))
        .filter(|root| removable(root))
        .collect()
}

/// What Explorer calls the drive at `root`, as `USB Drive (E:)`.
fn drive_name(root: &str) -> Option<String> {
    let root = HSTRING::from(root);
    // SAFETY: the root outlives the call; COM is initialized on this
    // thread; the name is the caller's to free, which `take_text` does.
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&root, None).ok()?;
        take_text(item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drives_are_the_letters_whose_bits_are_set() {
        let letters = 0b1_0000_0000_0000_0000_0001_0101;
        let all = removable_letters(letters, |_| true);
        assert_eq!(all, [r"A:\", r"C:\", r"E:\", r"Y:\"]);
        let some = removable_letters(letters, |root| root != r"C:\");
        assert_eq!(some, [r"A:\", r"E:\", r"Y:\"]);
    }

    #[test]
    fn the_items_come_by_name_whatever_their_case() {
        let item = |name: &str| SendToTarget {
            name: name.to_owned(),
            path: PathBuf::from(name),
            kind: SendToKind::Item,
        };
        let mut targets = vec![
            item("mail recipient"),
            item("Documents"),
            item("Compressed"),
        ];
        sort_by_name(&mut targets);
        let names: Vec<&str> = targets.iter().map(|target| target.name.as_str()).collect();
        assert_eq!(names, ["Compressed", "Documents", "mail recipient"]);
    }
}
