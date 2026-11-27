// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the verbs of the Shell that act on files share: files as the
//! Shell names them (item identifier lists), a data object that carries
//! them to a handler, the window that owns the dialogs of a handler, and
//! the texts and folders the Shell hands out.
//!
//! Everything here is to be used on a thread with COM initialized
//! apartment-threaded, as the [`ShellThread`] has it.
#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoTaskMemFree, IDataObject};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    ILFree, ILGetSize, KF_FLAG_DEFAULT, SHCreateDataObject, SHGetKnownFolderPath,
    SHParseDisplayName,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId, WS_EX_TOOLWINDOW,
};
use windows::core::{GUID, HSTRING, PWSTR};

use super::execute::expand_environment;
use super::{IconPixels, ShellThread};

/// An absolute item identifier list, the name the Shell gives a file, a
/// folder or a place without a path; freed when dropped.
pub(super) struct ItemId(*mut ITEMIDLIST);

impl ItemId {
    /// The item at `path`, which must exist. A path written with forward
    /// slashes is taken as well.
    pub(super) fn of_path(path: &Path) -> Result<Self> {
        let name = HSTRING::from(windows_path(path));
        let mut id = std::ptr::null_mut();
        // SAFETY: the name is NUL-terminated and outlives the call, which
        // gives a list allocated for the caller, freed by `Drop`.
        unsafe { SHParseDisplayName(&name, None, &raw mut id, 0, None) }
            .with_context(|| format!("the Shell does not know {}", path.display()))?;
        if id.is_null() {
            bail!("the Shell does not know {}", path.display());
        }
        Ok(Self(id))
    }

    /// Takes over a list the Shell allocated for the caller.
    ///
    /// # Safety
    ///
    /// `id` must be a list allocated with the allocator of COM that nothing
    /// else frees.
    pub(super) unsafe fn adopt(id: *mut ITEMIDLIST) -> Option<Self> {
        (!id.is_null()).then_some(Self(id))
    }

    pub(super) fn as_ptr(&self) -> *const ITEMIDLIST {
        self.0
    }

    /// The bytes of the list, its terminating zero included, to keep and
    /// name the item by later; see [`id_bytes`].
    #[allow(dead_code, reason = "for the items that are kept by their bytes")]
    pub(super) fn bytes(&self) -> Vec<u8> {
        // SAFETY: the list is valid while `self` lives.
        unsafe { id_bytes(self.0) }
    }
}

impl Drop for ItemId {
    fn drop(&mut self) {
        // SAFETY: the list was allocated for this value and is freed once.
        unsafe { ILFree(Some(self.0)) };
    }
}

/// The bytes of the item identifier list `id`, its terminating zero
/// included. A list made of them again is read where it lies; an item
/// identifier may lie at any address, as the Shell packs them.
///
/// # Safety
///
/// `id` must point to a valid item identifier list.
pub(super) unsafe fn id_bytes(id: *const ITEMIDLIST) -> Vec<u8> {
    // SAFETY: as the caller promises; the size counts the terminator.
    unsafe {
        let size = ILGetSize(Some(id)) as usize;
        std::slice::from_raw_parts(id.cast::<u8>(), size).to_vec()
    }
}

/// Whether `bytes` are a whole item identifier list: identifiers that
/// each say their size and fit, then a terminating zero of two bytes.
/// Bytes that are not are never handed to the Shell.
pub(super) fn is_item_id_list(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    loop {
        let Some(size) = rest
            .first_chunk::<2>()
            .map(|size| u16::from_le_bytes(*size))
        else {
            return false;
        };
        match size as usize {
            0 => return rest.len() == 2,
            // An identifier carries its size and at least one byte.
            size if size > 2 && size <= rest.len() => rest = &rest[size..],
            _ => return false,
        }
    }
}

/// A data object of the Shell that carries the files at `paths`, as a
/// selection in Explorer does: the formats of the Shell (`Shell IDList
/// Array`) and a list of files (`CF_HDROP`) a handler may ask for. The
/// files may lie in different folders.
pub(super) fn data_object(paths: &[PathBuf]) -> Result<IDataObject> {
    if paths.is_empty() {
        bail!("no file was given");
    }
    let ids = { paths.iter() }
        .map(|path| ItemId::of_path(path))
        .collect::<Result<Vec<_>>>()?;
    let pointers: Vec<*const ITEMIDLIST> = ids.iter().map(ItemId::as_ptr).collect();
    // SAFETY: without a folder the lists are taken as absolute, which they
    // are; the data object copies them, and they outlive the call.
    unsafe { SHCreateDataObject(None, Some(&pointers), None) }
        .context("the files could not be handed over")
}

/// The window the dialogs of a handler of the Shell belong to: the window
/// of Tessera in front, where one is, so that the dialog comes in front
/// too, or else the hidden window of the shell thread. A menu or a popup
/// of Tessera in front is passed over: it closes once something in it
/// was chosen, and a dialog it owned would close with it.
pub(super) fn dialog_owner() -> HWND {
    let mut process = 0u32;
    // SAFETY: the calls only read which window is in front, whose it is
    // and its styles; `process` outlives the call that fills it.
    let front = unsafe {
        let window = GetForegroundWindow();
        GetWindowThreadProcessId(window, Some(&raw mut process));
        let passing = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0;
        let own = !window.is_invalid() && process == GetCurrentProcessId();
        (own && !passing).then_some(window)
    };
    front.or_else(ShellThread::owner).unwrap_or_default()
}

/// The text the Shell allocated for the caller, freed here; `None` for no
/// text.
///
/// # Safety
///
/// `text` must be null or a NUL-terminated text allocated with the
/// allocator of COM that nothing else frees.
pub(super) unsafe fn take_text(text: PWSTR) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: as the caller promises; the text is freed once.
    unsafe {
        let taken = text.to_string().ok();
        CoTaskMemFree(Some(text.0.cast::<c_void>().cast_const()));
        taken
    }
}

/// The same for a text that names a file.
///
/// # Safety
///
/// As for [`take_text`].
pub(super) unsafe fn take_path(text: PWSTR) -> Option<PathBuf> {
    if text.is_null() {
        return None;
    }
    // SAFETY: as the caller promises; the text is freed once.
    unsafe {
        let path = PathBuf::from(OsString::from_wide(text.as_wide()));
        CoTaskMemFree(Some(text.0.cast::<c_void>().cast_const()));
        Some(path)
    }
}

/// Where the known folder `id` of the user is, such as `FOLDERID_SendTo`.
pub(super) fn known_folder(id: &GUID) -> Result<PathBuf> {
    // SAFETY: the identifier outlives the call, and the text it gives is
    // the caller's to free, which `take_path` does.
    unsafe {
        let text = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)
            .map_err(|error| anyhow!("the folder is not known: {error}"))?;
        take_path(text).context("the folder has no path")
    }
}

/// Where an icon is drawn from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconPlace {
    /// An icon inside a program, a library or an icon file; `index`
    /// counts its icons from zero, and a negative one is the identifier
    /// of a resource.
    Resource { file: PathBuf, index: i32 },
    /// A picture file, as the logo of a packaged application.
    Picture(PathBuf),
}

impl IconPlace {
    /// The place a location of the registry names, as `DefaultIcon` and
    /// `IconPath` write it (`%SystemRoot%\system32\imageres.dll,-102`,
    /// `"C:\app.exe",0`, `C:\a.ico`), with the variables of the
    /// environment replaced, or a resource string of a package
    /// (`@{Package?ms-resource://...}`) read. `None` for `%1`, which
    /// means the icon of each file, and for nothing.
    pub(super) fn from_location(location: &str) -> Option<Self> {
        let location = location.trim();
        if location.starts_with('@') {
            let picture = super::shell_new::indirect_text(location)?;
            return Some(Self::Picture(PathBuf::from(picture)));
        }
        let (file, index) = split_location(location)?;
        let file = expand_environment(OsStr::new(file));
        Some(Self::Resource {
            file: PathBuf::from(file),
            index,
        })
    }

    /// The icon, `size` pixels wide and high, or `None` where it cannot be
    /// read. It reads files: never call it on the thread of GPUI.
    #[allow(
        dead_code,
        reason = "for the menus Open with and New of the file manager"
    )]
    pub fn pixels(&self, size: u32) -> Option<IconPixels> {
        match self {
            Self::Resource { file, index } => IconPixels::of_file(file, *index, size),
            Self::Picture(path) => {
                let picture = image::open(path).ok()?;
                let picture = picture.thumbnail(size, size).into_rgba8();
                let (width, height) = picture.dimensions();
                let mut bgra = picture.into_raw();
                bgra.as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .for_each(|pixel| pixel.swap(0, 2));
                Some(IconPixels {
                    width,
                    height,
                    bgra,
                })
            }
        }
    }
}

/// The file and the index of an icon location (`file,index`, or `file`
/// alone for the first icon), with quotes taken away; `None` for `%1` and
/// for nothing.
fn split_location(location: &str) -> Option<(&str, i32)> {
    let (file, index) = match location.rsplit_once(',') {
        Some((file, index)) => match index.trim().parse() {
            Ok(index) => (file, index),
            Err(_) => (location, 0),
        },
        None => (location, 0),
    };
    let file = file.trim().trim_matches('"').trim();
    (!file.is_empty() && file != "%1").then_some((file, index))
}

/// `path` with every forward slash a backslash, as the Shell takes it.
pub(super) fn windows_path(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_is_whole_when_its_identifiers_fit_and_it_ends_in_zero() {
        assert!(is_item_id_list(&[0, 0]));
        assert!(is_item_id_list(&[4, 0, 7, 9, 0, 0]));
        assert!(is_item_id_list(&[3, 0, 1, 5, 0, 2, 3, 4, 0, 0]));
        // No terminator, one too long, one too short, bytes after the end.
        assert!(!is_item_id_list(&[]));
        assert!(!is_item_id_list(&[4, 0, 7, 9]));
        assert!(!is_item_id_list(&[9, 0, 7, 9, 0, 0]));
        assert!(!is_item_id_list(&[2, 0, 0, 0]));
        assert!(!is_item_id_list(&[0, 0, 1]));
    }

    #[test]
    fn an_icon_location_names_a_file_and_an_index() {
        let split = split_location;
        assert_eq!(
            split(r"%SystemRoot%\system32\imageres.dll,-102"),
            Some((r"%SystemRoot%\system32\imageres.dll", -102))
        );
        assert_eq!(
            split(r#""C:\Program Files\a,b\app.exe",3"#),
            Some((r"C:\Program Files\a,b\app.exe", 3))
        );
        assert_eq!(split(r"C:\a,b\icon.ico"), Some((r"C:\a,b\icon.ico", 0)));
        assert_eq!(split(r"C:\icon.ico"), Some((r"C:\icon.ico", 0)));
        assert_eq!(split("%1"), None);
        assert_eq!(split(" "), None);
    }

    #[test]
    fn the_shell_gets_backslashes() {
        assert_eq!(
            windows_path(Path::new("C:/Users/a b/x.txt")),
            r"C:\Users\a b\x.txt"
        );
    }
}
