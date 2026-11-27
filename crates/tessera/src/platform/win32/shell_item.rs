// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Items of the Shell as the other parts of the platform layer hand them
//! around: identifier lists (PIDLs), which name a file, a folder or a
//! place without a path such as This PC or an item in the Recycle Bin, and
//! the `IShellItem` of a path.
//!
//! An identifier list lives in memory of COM and cannot cross threads; to
//! keep one, or hand it to another thread, it is copied into a
//! [`ShellId`], which is plain bytes.
#![allow(unsafe_code)]

use std::ffi::OsStr;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use anyhow::{Context as _, Result, anyhow};
use windows::Win32::System::Com::{CoTaskMemAlloc, CoTaskMemFree};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    ILClone, ILFindLastID, ILFree, ILGetSize, ILIsEqual, ILRemoveLastID, IShellFolder, IShellItem,
    SHBindToParent, SHCreateItemFromIDList, SHGetDesktopFolder, SHGetIDListFromObject,
    SHParseDisplayName, SIGDN, SIGDN_FILESYSPATH,
};
use windows::core::{Interface, PCWSTR, PWSTR};

/// The identifier list of an item of the Shell, as bytes that can be kept
/// and handed between threads: a file in the Recycle Bin, a place such as
/// This PC, or a file by what the Shell knows it as. It is absolute, from
/// the desktop.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShellId(Vec<u8>);

impl ShellId {
    /// The identifier list in `bytes`, if they hold one whole list: items
    /// that each start with their size, ended by an empty one.
    pub fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        (id_list_length(&bytes)? == bytes.len()).then_some(Self(bytes))
    }

    /// The bytes of the identifier list, with its ending.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// How long the identifier list at the start of `bytes` is, with its
/// ending of two zero bytes; `None` where the bytes end before it does.
pub(super) fn id_list_length(bytes: &[u8]) -> Option<usize> {
    let mut at = 0usize;
    loop {
        let size = bytes.get(at..at + 2)?;
        let size = usize::from(u16::from_le_bytes([size[0], size[1]]));
        if size == 0 {
            return Some(at + 2);
        }
        // An item holds at least its own size.
        if size < 2 {
            return None;
        }
        at = at.checked_add(size)?;
    }
}

/// An identifier list owned here, freed when this is dropped. It stays on
/// the thread that made it.
pub(super) struct IdList(NonNull<ITEMIDLIST>);

impl IdList {
    /// Takes over an identifier list that the Shell allocated; `None` for
    /// a null pointer.
    fn adopt(list: *mut ITEMIDLIST) -> Option<Self> {
        NonNull::new(list).map(Self)
    }

    /// The identifier list of what `path` names: a path of the file
    /// system, or a name the Shell parses, such as `shell:Downloads` or
    /// `::{20D04FE0-3AEA-1069-A2D8-08002B30309D}`.
    pub(super) fn of_path(path: &Path) -> Result<Self> {
        let name = wide(path.as_os_str());
        let mut list = std::ptr::null_mut();
        // SAFETY: the name is NUL-terminated and outlives the call, which
        // writes an identifier list that is owned here from then on.
        unsafe { SHParseDisplayName(PCWSTR(name.as_ptr()), None, &raw mut list, 0, None) }
            .with_context(|| format!("Windows does not know {}", path.display()))?;
        Self::adopt(list).ok_or_else(|| anyhow!("Windows does not know {}", path.display()))
    }

    /// The identifier list of an object of the Shell, such as an
    /// `IShellItem`.
    pub(super) fn of_object(object: &impl Interface) -> Result<Self> {
        let unknown: windows::core::IUnknown = object.cast()?;
        // SAFETY: the object is alive for the call, which returns a new
        // identifier list owned here from then on.
        let list = unsafe { SHGetIDListFromObject(&unknown) }?;
        Self::adopt(list).ok_or_else(|| anyhow!("the item has no identifier list"))
    }

    /// A copy of the identifier list in `id`, on this thread.
    pub(super) fn of_id(id: &ShellId) -> Result<Self> {
        let bytes = id.as_bytes();
        // SAFETY: the memory is allocated with the length of the bytes,
        // which are copied into it whole; a `ShellId` holds only whole
        // lists, so the copy is one, with its ending.
        unsafe {
            let memory = CoTaskMemAlloc(bytes.len()).cast::<u8>();
            if memory.is_null() {
                return Err(anyhow!("no memory for an identifier list"));
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), memory, bytes.len());
            Self::adopt(memory.cast()).ok_or_else(|| anyhow!("no memory for an identifier list"))
        }
    }

    /// The identifier list as bytes, to keep or hand to another thread.
    pub(super) fn to_id(&self) -> ShellId {
        // SAFETY: the list is valid and owned here; its size counts its
        // ending, and it is read only within that size.
        let bytes = unsafe {
            let size = ILGetSize(Some(self.as_ptr())) as usize;
            std::slice::from_raw_parts(self.as_ptr().cast::<u8>(), size).to_vec()
        };
        ShellId(bytes)
    }

    pub(super) fn as_ptr(&self) -> *const ITEMIDLIST {
        self.0.as_ptr()
    }

    /// The last item of the list: what the list names, as its parent
    /// folder knows it. It points into this list.
    pub(super) fn last(&self) -> *const ITEMIDLIST {
        // SAFETY: the list is valid; the answer points into it.
        unsafe { ILFindLastID(self.as_ptr()) }
    }

    /// The identifier list of the folder this one lies in; the desktop's
    /// own list, which is empty, has none.
    pub(super) fn parent(&self) -> Option<Self> {
        // SAFETY: the list is valid; the copy is owned here, and is cut
        // short in place.
        unsafe {
            let copy = Self::adopt(ILClone(self.as_ptr()))?;
            ILRemoveLastID(Some(copy.0.as_ptr()))
                .as_bool()
                .then_some(copy)
        }
    }

    /// Whether this list and `other` name the same item.
    pub(super) fn same_as(&self, other: &Self) -> bool {
        // SAFETY: both lists are valid for the call.
        unsafe { ILIsEqual(self.as_ptr(), other.as_ptr()) }.as_bool()
    }

    /// The `IShellItem` of what the list names.
    pub(super) fn item(&self) -> Result<IShellItem> {
        // SAFETY: the list is valid for the call, which copies it.
        unsafe { SHCreateItemFromIDList(self.as_ptr()) }.context("the item cannot be reached")
    }

    /// The folder the item lies in, and the item as that folder knows it,
    /// which points into this list.
    pub(super) fn bind_to_parent(&self) -> Result<(IShellFolder, *const ITEMIDLIST)> {
        let mut last = std::ptr::null_mut();
        // SAFETY: the list is valid for the call; `last` points into it.
        let folder: IShellFolder = unsafe { SHBindToParent(self.as_ptr(), Some(&raw mut last)) }
            .context("the folder of the item cannot be reached")?;
        Ok((folder, last.cast_const()))
    }
}

impl Drop for IdList {
    fn drop(&mut self) {
        // SAFETY: the list is owned here and not used after this.
        unsafe { ILFree(Some(self.as_ptr())) }
    }
}

/// Items of the Shell grouped the way the Shell asks for objects of
/// several items at once: by the folder they lie in, or, where they lie in
/// different folders, from the desktop.
pub(super) struct ItemGroup {
    /// The folder that is asked: the common parent, or the desktop.
    pub(super) folder: IShellFolder,
    /// The identifier list of that folder; `None` for the desktop.
    pub(super) parent: Option<IdList>,
    /// The items, absolute.
    items: Vec<IdList>,
    /// Whether the items share `parent`, so that their last ids are what
    /// `folder` knows them by.
    siblings: bool,
}

impl ItemGroup {
    /// Groups `items`, which must not be empty.
    pub(super) fn of(items: Vec<IdList>) -> Result<Self> {
        let first = items.first().context("no items are given")?;
        let parent = first.parent();
        let siblings = match &parent {
            Some(parent) => items.iter().skip(1).all(|item| {
                item.parent()
                    .is_some_and(|other_parent| other_parent.same_as(parent))
            }),
            None => false,
        };
        if siblings {
            let (folder, _) = first.bind_to_parent()?;
            return Ok(Self {
                folder,
                parent,
                items,
                siblings,
            });
        }
        // SAFETY: the call takes no arguments.
        let folder = unsafe { SHGetDesktopFolder() }.context("the desktop cannot be reached")?;
        Ok(Self {
            folder,
            parent: None,
            items,
            siblings,
        })
    }

    /// The items of the paths `paths`.
    pub(super) fn of_paths(paths: &[PathBuf]) -> Result<Self> {
        let items = { paths.iter() }
            .map(|path| IdList::of_path(path))
            .collect::<Result<Vec<_>>>()?;
        Self::of(items)
    }

    /// The items as `folder` knows them: their last ids where they share
    /// it, else the whole lists, which the desktop takes. They point into
    /// the lists of this group.
    pub(super) fn children(&self) -> Vec<*const ITEMIDLIST> {
        { self.items.iter() }
            .map(|item| match self.siblings {
                true => item.last(),
                false => item.as_ptr(),
            })
            .collect()
    }
}

/// `text` as Windows takes it: UTF-16, ended by a NUL.
pub(super) fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain([0]).collect()
}

/// Reads and frees a text that COM allocated.
pub(super) fn take_text(text: PWSTR) -> std::ffi::OsString {
    // SAFETY: the text is NUL-terminated, allocated by COM for the caller,
    // read once and freed once.
    unsafe {
        let read = std::ffi::OsString::from_wide(text.as_wide());
        CoTaskMemFree(Some(text.0.cast_const().cast()));
        read
    }
}

/// The `IShellItem` of the file or folder at `path`.
pub(super) fn item_of_path(path: &Path) -> Result<IShellItem> {
    IdList::of_path(path)?.item()
}

/// The name of `item` of the kind `kind`; `None` where it has none of
/// that kind, as an item without a path in the file system.
pub(super) fn name_of_item(item: &IShellItem, kind: SIGDN) -> Option<std::ffi::OsString> {
    // SAFETY: the item is alive for the call, and the name it returns is
    // read and freed once.
    unsafe { item.GetDisplayName(kind) }.ok().map(take_text)
}

/// The path of `item` in the file system, where it has one.
pub(super) fn path_of_item(item: &IShellItem) -> Option<PathBuf> {
    name_of_item(item, SIGDN_FILESYSPATH).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_list_is_measured_to_its_ending() {
        // Two items of four and three bytes, then the ending.
        let list = [4, 0, 9, 9, 3, 0, 7, 0, 0];
        assert_eq!(id_list_length(&list), Some(9));
        assert_eq!(id_list_length(&[0, 0]), Some(2));
        assert_eq!(id_list_length(&[0, 0, 5]), Some(2));
    }

    #[test]
    fn a_broken_id_list_is_refused() {
        assert_eq!(id_list_length(&[]), None);
        assert_eq!(id_list_length(&[4, 0, 9]), None);
        assert_eq!(id_list_length(&[1, 0, 0, 0]), None);
        assert!(ShellId::from_bytes(vec![4, 0, 1, 2, 0, 0, 3]).is_none());
        assert!(ShellId::from_bytes(vec![4, 0, 1, 2, 0, 0]).is_some());
    }
}
