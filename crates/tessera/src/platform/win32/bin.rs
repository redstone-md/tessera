// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Recycle Bin: how much lies in it, what lies in it and where it came
//! from, bringing items back, erasing them, emptying it, and the window
//! that shows it.
//!
//! The items are read from the folder of the Shell that shows the bins of
//! all drives as one (`FOLDERID_RecycleBinFolder`), with the properties
//! Explorer shows for them: where an item was deleted from, when, and its
//! size. Each deleted file lies in a folder `$Recycle.Bin` of its drive
//! under a name of the form `$R....ext`, next to a file `$I....ext` that
//! records its whole original path; that file is read where it can be, as
//! it also tells the name with its extension. Bringing items back and
//! erasing them are the verbs `undelete` and `delete` of the bin, as its
//! context menu in Explorer does them, with the dialogs of the Shell for
//! a name that is taken and for the confirmation. Reading the bin runs on
//! the [`ShellThread`]; restoring, erasing and emptying, which may take
//! long and wait for the user, each on a thread of its own.
//!
//! ```ignore
//! let items = RecycleBin::list();
//! cx.spawn(async move |cx| {
//!     let items = items.recv().await.unwrap_or(Ok(Vec::new()))?;
//!     RecycleBin::restore(items);
//! }).detach();
//! ```
#![allow(unsafe_code)]

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};
use async_channel::Receiver;
use windows::Win32::Foundation::{E_ABORT, E_UNEXPECTED, ERROR_CANCELLED, FILETIME, PROPERTYKEY};
use windows::Win32::Storage::EnhancedStorage::PKEY_Size;
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_RecycleBinFolder, IEnumShellItems, IShellItem, IShellItem2,
    KF_FLAG_DEFAULT, SHEmptyRecycleBinW, SHGetKnownFolderItem, SHQUERYRBINFO, SHQueryRecycleBinW,
    SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_SETFOREGROUND, MB_YESNO, MessageBoxW,
};
use windows::core::{GUID, HRESULT, HSTRING, Interface as _, PCWSTR};

use super::ShellThread;
use super::shell_item::{IdList, ShellId};
use super::shell_items::{dialog_owner, take_path, take_text};
use super::shell_menu;

/// The properties of a deleted item (`System.Recycle.DeletedFrom` and
/// `System.Recycle.DateDeleted`), in the property set of displaced items.
const DISPLACED: GUID = GUID::from_u128(0x9b174b33_40ff_11d2_a27e_00c04fc30871);
const PKEY_DELETED_FROM: PROPERTYKEY = PROPERTYKEY {
    fmtid: DISPLACED,
    pid: 2,
};
const PKEY_DATE_DELETED: PROPERTYKEY = PROPERTYKEY {
    fmtid: DISPLACED,
    pid: 3,
};

/// The verbs of the bin for its items.
const VERB_RESTORE: &str = "undelete";
const VERB_ERASE: &str = "delete";

/// The span between the start of 1601, where the times of Windows count
/// from, and the start of 1970.
const WINDOWS_TO_UNIX: Duration = Duration::from_secs(11_644_473_600);

/// The Recycle Bin of the user, over all drives.
pub struct RecycleBin;

/// What lies in the bin, as Windows counts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BinUsage {
    pub items: u64,
    /// The bytes the items take.
    pub bytes: u64,
}

/// An item in the bin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecycledItem {
    /// What Explorer calls it in the bin: its name before it was deleted.
    pub name: String,
    /// Where it lay before it was deleted, its name included.
    pub original: PathBuf,
    /// When it was deleted, where Windows says.
    pub deleted: Option<SystemTime>,
    /// Its size in bytes, where Windows says; that of all its files for a
    /// folder.
    pub size: Option<u64>,
    /// Where it lies now, as `C:\$Recycle.Bin\<user>\$R1A2B3C.txt`, where
    /// the bin is a folder of a drive.
    pub stored: Option<PathBuf>,
    /// Its item identifier list, from the desktop through the bin.
    id: ShellId,
}

impl RecycleBin {
    /// How many items lie in the bin and how much they take, or `None` if
    /// Windows does not tell. It looks at every drive, which may wait for
    /// a drive that is slow to answer: never call it on the thread of
    /// GPUI.
    pub fn usage() -> Option<BinUsage> {
        let mut info = SHQUERYRBINFO {
            cbSize: size_of::<SHQUERYRBINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: no path asks for the bins of all drives, and `info`
        // outlives the call and says its own size.
        unsafe { SHQueryRecycleBinW(PCWSTR::null(), &raw mut info) }.ok()?;
        Some(BinUsage {
            items: u64::try_from(info.i64NumItems).ok()?,
            bytes: u64::try_from(info.i64Size).unwrap_or_default(),
        })
    }

    /// Has Explorer open a window on the bin. Tessera's file manager has no
    /// view of the bin yet.
    pub fn open() -> io::Result<()> {
        Command::new("explorer.exe")
            .arg("shell:RecycleBinFolder")
            .spawn()
            .map(drop)
    }

    /// Every item in the bin, the latest deleted first. The receiver
    /// yields once.
    pub fn list() -> Receiver<Result<Vec<RecycledItem>, String>> {
        ShellThread::call(|| list().map_err(|error| format!("{error:#}")))
    }

    /// Brings `items` back to where they were deleted from; the Shell asks
    /// what to do where a name there is taken. It runs on a thread of its
    /// own. The receiver yields once.
    #[allow(dead_code, reason = "for the view of the bin in the file manager")]
    pub fn restore(items: Vec<RecycledItem>) -> Receiver<Result<(), String>> {
        ShellThread::call_alone("restore from the bin", move || {
            invoke(&items, VERB_RESTORE, true).map_err(|error| format!("{error:#}"))
        })
    }

    /// Brings back every item in the bin once the user said yes to a
    /// question of Windows titled `title`, whose text `question` writes
    /// for the number of items and the bytes they take. It runs on a
    /// thread of its own. The receiver yields once: how many items were
    /// brought back, or `None` where the bin was empty or the user said
    /// no.
    pub fn restore_all(
        title: String,
        question: impl FnOnce(usize, u64) -> String + Send + 'static,
    ) -> Receiver<Result<Option<usize>, String>> {
        ShellThread::call_alone("restore the bin", move || {
            let restored = list().and_then(|items| {
                if items.is_empty() {
                    return Ok(None);
                }
                let bytes = items.iter().filter_map(|item| item.size).sum();
                if !ask(&title, &question(items.len(), bytes)) {
                    return Ok(None);
                }
                invoke(&items, VERB_RESTORE, true)?;
                Ok(Some(items.len()))
            });
            restored.map_err(|error| format!("{error:#}"))
        })
    }

    /// Brings back the items that lie at `stored` in the bin (see
    /// [`RecycledItem::stored`]), as the Shell tells where a file it
    /// recycled went, and tells how many of them were still there. This is
    /// how a deletion is undone.
    #[allow(dead_code, reason = "for undoing a deletion in the file manager")]
    pub fn restore_stored(stored: Vec<PathBuf>) -> Receiver<Result<usize, String>> {
        ShellThread::call_alone("restore from the bin", move || {
            let restored = list().and_then(|items| {
                let found: Vec<RecycledItem> = { items.into_iter() }
                    .filter(|item| {
                        item.stored
                            .as_deref()
                            .is_some_and(|at| lies_at(at, &stored))
                    })
                    .collect();
                if !found.is_empty() {
                    invoke(&found, VERB_RESTORE, true)?;
                }
                Ok(found.len())
            });
            restored.map_err(|error| format!("{error:#}"))
        })
    }

    /// Erases `items` for good; with `confirm` the Shell asks the user
    /// first, as Explorer does. It runs on a thread of its own. The
    /// receiver yields once.
    pub fn erase(items: Vec<RecycledItem>, confirm: bool) -> Receiver<Result<(), String>> {
        ShellThread::call_alone("erase from the bin", move || {
            invoke(&items, VERB_ERASE, confirm).map_err(|error| format!("{error:#}"))
        })
    }

    /// Empties the bins of all drives; the Shell asks the user first, then
    /// shows its progress and plays the sound of emptying. It runs on a
    /// thread of its own. The receiver yields once: whether the bin was
    /// emptied, which it was not where it was empty already or the user
    /// said no.
    pub fn empty() -> Receiver<Result<bool, String>> {
        ShellThread::call_alone("empty the bin", || {
            if Self::usage().is_some_and(|usage| usage.items == 0) {
                return Ok(false);
            }
            // SAFETY: no path asks for the bins of all drives; the owner
            // is a window that outlives the dialog or none.
            let emptied = unsafe { SHEmptyRecycleBinW(Some(dialog_owner()), PCWSTR::null(), 0) };
            match emptied {
                Ok(()) => Ok(true),
                // What Windows answers for bins that were emptied meanwhile.
                Err(error) if is_cancel(error.code()) || error.code() == E_UNEXPECTED => Ok(false),
                Err(error) => Err(format!("the Recycle Bin was not emptied: {error}")),
            }
        })
    }
}

// files-previews: what the view of the bin in the file manager needs.
impl RecycledItem {
    /// The item as the Shell knows it, which a [`FileOperation`] brings
    /// back to where it was, so that bringing it back can be undone.
    ///
    /// [`FileOperation`]: super::FileOperation
    pub fn id(&self) -> &ShellId {
        &self.id
    }
}

/// Whether `code` says that the user called off what the Shell asked.
fn is_cancel(code: HRESULT) -> bool {
    code == E_ABORT || code == HRESULT::from_win32(ERROR_CANCELLED.0)
}

/// Asks the user `question` in a box of Windows titled `title`, with the
/// answers yes and no, no by default; whether they said yes.
fn ask(title: &str, question: &str) -> bool {
    let (title, question) = (HSTRING::from(title), HSTRING::from(question));
    let style = MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2 | MB_SETFOREGROUND;
    // SAFETY: the texts are NUL-terminated and outlive the call; the owner
    // is a window that outlives the box or none.
    unsafe { MessageBoxW(Some(dialog_owner()), &question, &title, style) == IDYES }
}

/// Whether `place` is one of `stored`, as Windows compares paths.
fn lies_at(place: &Path, stored: &[PathBuf]) -> bool {
    let place = place.to_string_lossy().replace('/', "\\").to_lowercase();
    { stored.iter() }
        .any(|other| other.to_string_lossy().replace('/', "\\").to_lowercase() == place)
}

/// The bin as a folder of the Shell.
fn bin() -> Result<IShellItem> {
    // SAFETY: the identifier outlives the call; COM is initialized on this
    // thread.
    unsafe { SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None) }
        .context("the Recycle Bin is not reachable")
}

/// Every item in the bin, the latest deleted first.
fn list() -> Result<Vec<RecycledItem>> {
    let bin = bin()?;
    let mut items = Vec::new();
    // SAFETY: COM is initialized on this thread; the slot has room for the
    // one item asked for at a time.
    unsafe {
        let listed: IEnumShellItems = bin
            .BindToHandler(None, &BHID_EnumItems)
            .context("the Recycle Bin could not be read")?;
        loop {
            let mut slot = [None];
            let mut fetched = 0u32;
            let next = listed.Next(&mut slot, Some(&raw mut fetched));
            match (next, slot) {
                (Ok(()), [Some(item)]) if fetched == 1 => items.extend(read_item(&item)),
                _ => break,
            }
        }
    }
    items.sort_by_key(|item| std::cmp::Reverse(item.deleted));
    Ok(items)
}

/// An item of the bin as it is shown; `None` for one the Shell does not
/// describe.
fn read_item(item: &IShellItem) -> Option<RecycledItem> {
    let id = IdList::of_object(item).ok()?.to_id();
    // SAFETY: COM is initialized on this thread; the texts the calls give
    // are the caller's to free, which `take_text` and `take_path` do.
    unsafe {
        let name = take_text(item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?)?;
        let stored =
            { item.GetDisplayName(SIGDN_FILESYSPATH).ok() }.and_then(|path| take_path(path));
        let described = item.cast::<IShellItem2>().ok();
        let from = { described.as_ref() }
            .and_then(|item| item.GetString(&PKEY_DELETED_FROM).ok())
            .and_then(|from| take_path(from));
        let deleted = { described.as_ref() }
            .and_then(|item| item.GetFileTime(&PKEY_DATE_DELETED).ok())
            .and_then(|time| system_time(ticks(time)));
        let size = { described.as_ref() }.and_then(|item| item.GetUInt64(&PKEY_Size).ok());
        let recorded = { stored.as_deref() }
            .and_then(info_path)
            .and_then(|info| std::fs::read(info).ok())
            .and_then(|bytes| parse_info(&bytes));
        let original = match &recorded {
            Some(recorded) => recorded.path.clone(),
            None => from.unwrap_or_default().join(&name),
        };
        Some(RecycledItem {
            name: { recorded.as_ref() }
                .and_then(|recorded| {
                    Some(recorded.path.file_name()?.to_string_lossy().into_owned())
                })
                .unwrap_or(name),
            original,
            deleted: deleted.or_else(|| system_time(recorded.as_ref()?.deleted)),
            size: size.or_else(|| Some(recorded.as_ref()?.size)),
            stored,
            id,
        })
    }
}

/// Does `verb` of the bin's context menu to `items`; without `dialogs`
/// the Shell asks the user nothing.
fn invoke(items: &[RecycledItem], verb: &str, dialogs: bool) -> Result<()> {
    let lists = { items.iter() }
        .map(|item| IdList::of_id(&item.id))
        .collect::<Result<Vec<_>>>()?;
    if lists.is_empty() {
        return Ok(());
    }
    match shell_menu::invoke_verb_here(lists, verb, dialog_owner(), dialogs) {
        Err(error)
            if { error.downcast_ref::<windows::core::Error>() }
                .is_some_and(|error| is_cancel(error.code())) =>
        {
            Ok(())
        }
        invoked => invoked.with_context(|| format!("the Recycle Bin did not {verb} them")),
    }
}

/// What the `$I` file of a deleted item records.
#[derive(Debug, PartialEq, Eq)]
struct Recorded {
    path: PathBuf,
    size: u64,
    /// When it was deleted, in ticks of 100 nanoseconds from 1601.
    deleted: u64,
}

/// The `$I` file that records the item stored at `stored` (`$R1A2B3C.txt`
/// next to `$I1A2B3C.txt`).
pub(super) fn info_path(stored: &Path) -> Option<PathBuf> {
    let name = stored.file_name()?.to_str()?;
    let rest = name.strip_prefix("$R")?;
    Some(stored.with_file_name(format!("$I{rest}")))
}

/// Reads a `$I` file: a version (1 until Windows 8.1, 2 since Windows 10),
/// the size, the time of deletion, then the original path, in 260
/// characters for the first version and with its length before it for
/// the second.
fn parse_info(bytes: &[u8]) -> Option<Recorded> {
    let number = |at: usize| -> Option<u64> {
        let chunk = bytes.get(at..at + 8)?;
        Some(u64::from_le_bytes(chunk.try_into().ok()?))
    };
    let version = number(0)?;
    let size = number(8)?;
    let deleted = number(16)?;
    let text = match version {
        1 => bytes.get(24..24 + 520)?,
        2 => {
            let length = u32::from_le_bytes(bytes.get(24..28)?.try_into().ok()?) as usize;
            bytes.get(28..28 + length.checked_mul(2)?)?
        }
        _ => return None,
    };
    let units: Vec<u16> = { text.as_chunks::<2>().0.iter() }
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|&unit| unit != 0)
        .collect();
    if units.is_empty() {
        return None;
    }
    Some(Recorded {
        path: PathBuf::from(OsString::from_wide(&units)),
        size,
        deleted,
    })
}

/// The ticks a time of Windows counts.
fn ticks(time: FILETIME) -> u64 {
    u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime)
}

/// The moment `ticks` of 100 nanoseconds after the start of 1601 are;
/// `None` for none, and for one before 1970.
fn system_time(ticks: u64) -> Option<SystemTime> {
    let since_1601 = Duration::from_nanos(ticks.checked_mul(100)?);
    let since_1970 = since_1601.checked_sub(WINDOWS_TO_UNIX)?;
    (ticks != 0).then(|| SystemTime::UNIX_EPOCH + since_1970)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    fn header(version: u64, size: u64, deleted: u64) -> Vec<u8> {
        [version, size, deleted]
            .into_iter()
            .flat_map(u64::to_le_bytes)
            .collect()
    }

    #[test]
    fn the_record_of_windows_10_has_the_length_of_its_path() {
        let path = r"C:\Users\a\Documents\report.docx";
        let mut bytes = header(2, 4096, 133_000_000_000_000_000);
        bytes.extend((path.len() as u32 + 1).to_le_bytes());
        bytes.extend(wide(path));
        bytes.extend([0, 0]);
        let recorded = parse_info(&bytes).unwrap();
        assert_eq!(recorded.path, PathBuf::from(path));
        assert_eq!(recorded.size, 4096);
        assert_eq!(recorded.deleted, 133_000_000_000_000_000);
    }

    #[test]
    fn the_older_record_has_a_path_of_260_characters() {
        let path = r"D:\photo.jpg";
        let mut bytes = header(1, 12, 1);
        let mut text = wide(path);
        text.resize(520, 0);
        bytes.extend(text);
        assert_eq!(parse_info(&bytes).unwrap().path, PathBuf::from(path));
        // Cut short, or of a version not known.
        assert_eq!(parse_info(&bytes[..300]), None);
        assert_eq!(parse_info(&header(3, 0, 0)), None);
        assert_eq!(parse_info(&[1, 2, 3]), None);
    }

    #[test]
    fn a_stored_item_is_recorded_next_to_it() {
        let stored = Path::new(r"C:\$Recycle.Bin\S-1-5-21\$R1A2B3C.txt");
        assert_eq!(
            info_path(stored),
            Some(PathBuf::from(r"C:\$Recycle.Bin\S-1-5-21\$I1A2B3C.txt"))
        );
        assert_eq!(info_path(Path::new(r"C:\a\b.txt")), None);
    }

    #[test]
    fn the_times_of_windows_count_from_1601() {
        let start_of_1970 = WINDOWS_TO_UNIX.as_secs() * 10_000_000;
        assert_eq!(system_time(start_of_1970), Some(SystemTime::UNIX_EPOCH));
        assert_eq!(
            system_time(start_of_1970 + 10_000_000),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
        );
        assert_eq!(system_time(0), None);
        assert_eq!(system_time(5), None);
        let time = FILETIME {
            dwLowDateTime: 2,
            dwHighDateTime: 1,
        };
        assert_eq!(ticks(time), (1 << 32) + 2);
    }

    #[test]
    fn a_stored_item_is_found_whatever_the_case_and_slashes() {
        let stored = [PathBuf::from("c:/$recycle.bin/s-1/$rabc.txt")];
        assert!(lies_at(
            Path::new(r"C:\$Recycle.Bin\S-1\$RABC.txt"),
            &stored
        ));
        assert!(!lies_at(
            Path::new(r"C:\$Recycle.Bin\S-1\$RABD.txt"),
            &stored
        ));
    }
}
