// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What Explorer draws over the icons of files besides the icons
//! themselves: the overlays that programs register
//! (`IShellIconOverlayIdentifier`), as OneDrive marks what is in sync and
//! Git clients mark what changed, and the state of a file of a cloud
//! folder that its attributes tell: only online, kept on this device for
//! good, or free to be made online-only again.
//!
//! Overlays are asked of the folder that holds the files
//! (`IShellIconOverlay::GetOverlayIconIndex`), which asks each registered
//! handler in the order of their priority, and are drawn from the system
//! image list, where each is an icon as large as the one it lies on with
//! the mark in a corner. Handlers live in the programs that register
//! them and can be slow, so a folder is asked on a thread of its own.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::Metadata;
use std::os::windows::fs::MetadataExt as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use async_channel::Receiver;
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_PINNED, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
    FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_UNPINNED,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_SFObject, IShellFolder, IShellIconOverlay, SHGetImageList, SHIL_EXTRALARGE, SHIL_JUMBO,
};
use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;
use windows::core::{HSTRING, Interface as _};

use super::shell_item::item_of_path;
use super::{IconPixels, ShellThread};

/// How many items of one folder are asked for their overlays at most, as
/// each handler is asked for each.
const MOST_ASKED: usize = 4000;

/// The state of a file of a cloud folder (OneDrive and other providers of
/// the cloud files of Windows), as its attributes tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CloudState {
    /// Only in the cloud: opening it downloads it.
    OnlineOnly,
    /// Kept on this device for good ("Always keep on this device").
    AlwaysKept,
    /// On this device, and free to be made online-only again.
    Available,
}

impl CloudState {
    /// The state of the file `metadata` was read of, if it is one of a
    /// cloud folder that says it.
    pub fn of(metadata: &Metadata) -> Option<Self> {
        Self::of_attributes(metadata.file_attributes())
    }

    /// The state the attributes `bits` tell.
    pub fn of_attributes(bits: u32) -> Option<Self> {
        let online = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS.0
            | FILE_ATTRIBUTE_RECALL_ON_OPEN.0
            | FILE_ATTRIBUTE_OFFLINE.0;
        if bits & FILE_ATTRIBUTE_PINNED.0 != 0 {
            Some(Self::AlwaysKept)
        } else if bits & online != 0 {
            Some(Self::OnlineOnly)
        } else if bits & FILE_ATTRIBUTE_UNPINNED.0 != 0 {
            Some(Self::Available)
        } else {
            None
        }
    }

    /// What the column of the state says, in English, as the table of
    /// texts has it.
    pub fn title(self) -> &'static str {
        match self {
            Self::OnlineOnly => "Available when online",
            Self::AlwaysKept => "Always available on this device",
            Self::Available => "Available on this device",
        }
    }
}

/// The overlays of the items of a folder.
#[derive(Clone, Debug, Default)]
pub struct OverlayMarks {
    /// For each item that has one, by its name, which of `pictures` lies
    /// over its icon.
    pub marks: HashMap<OsString, usize>,
    /// The pictures of the overlays, each at the size asked for.
    pub pictures: Vec<IconPixels>,
}

/// The icon overlays of the Shell.
pub struct Overlays;

impl Overlays {
    /// Asks the folder `folder` for the overlays of the items called
    /// `names` in it, with their pictures about `size` pixels wide, on a
    /// thread of its own. The receiver yields once; nothing where the
    /// folder cannot be asked.
    pub fn of_folder(folder: PathBuf, names: Vec<OsString>, size: u32) -> Receiver<OverlayMarks> {
        ShellThread::call_alone("icon overlays", move || {
            marks_of(&folder, &names, size).unwrap_or_else(|error| {
                report!(
                    "the overlays of {} are not shown: {error:#}",
                    folder.display()
                );
                OverlayMarks::default()
            })
        })
    }
}

fn marks_of(folder: &std::path::Path, names: &[OsString], size: u32) -> Result<OverlayMarks> {
    let item = item_of_path(folder)?;
    // SAFETY: COM is initialized on this thread (`call_alone`); every item
    // identifier the folder gives is freed once it was asked about.
    let found = unsafe {
        let shell_folder: IShellFolder = { item.BindToHandler(None, &BHID_SFObject) }
            .context("the folder cannot be asked for overlays")?;
        let overlays: IShellIconOverlay = shell_folder
            .cast()
            .context("the folder draws no overlays")?;
        let mut found: Vec<(OsString, i32)> = Vec::new();
        for name in names.iter().take(MOST_ASKED) {
            let text = HSTRING::from(name.as_os_str());
            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
            let parsed = shell_folder.ParseDisplayName(
                HWND::default(),
                None,
                &text,
                None,
                &raw mut child,
                std::ptr::null_mut(),
            );
            if parsed.is_err() || child.is_null() {
                continue;
            }
            let mut index = 0i32;
            let asked = overlays.GetOverlayIconIndex(child, &raw mut index);
            CoTaskMemFree(Some(child.cast_const().cast()));
            if asked.is_ok() && index > 0 {
                found.push((name.clone(), index));
            }
        }
        found
    };
    let mut marks = OverlayMarks::default();
    if found.is_empty() {
        return Ok(marks);
    }
    let list = image_list(size)?;
    let mut drawn: HashMap<i32, Option<usize>> = HashMap::new();
    for (name, index) in found {
        let picture = *drawn.entry(index).or_insert_with(|| {
            let pixels = picture_of(&list, index)?;
            marks.pictures.push(pixels);
            Some(marks.pictures.len() - 1)
        });
        if let Some(picture) = picture {
            marks.marks.insert(name, picture);
        }
    }
    Ok(marks)
}

/// The system image list whose icons come nearest above `size` pixels.
fn image_list(size: u32) -> Result<IImageList> {
    let kind = match size {
        0..=48 => SHIL_EXTRALARGE,
        _ => SHIL_JUMBO,
    };
    // SAFETY: the call hands out the image list of the process, which
    // lives as long as it.
    unsafe { SHGetImageList(kind as i32) }.context("the system image list cannot be read")
}

/// The icon at `index` of `list`, as pixels.
fn picture_of(list: &IImageList, index: i32) -> Option<IconPixels> {
    // SAFETY: the icon is made for the caller and destroyed once its
    // pixels are copied.
    unsafe {
        let icon = list.GetIcon(index, ILD_TRANSPARENT.0).ok()?;
        let pixels = IconPixels::of_icon(icon);
        let _ = DestroyIcon(icon);
        pixels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_attributes_of_a_cloud_file_tell_its_state() {
        let recall = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS.0;
        assert_eq!(
            CloudState::of_attributes(recall | FILE_ATTRIBUTE_UNPINNED.0),
            Some(CloudState::OnlineOnly)
        );
        assert_eq!(
            CloudState::of_attributes(FILE_ATTRIBUTE_PINNED.0),
            Some(CloudState::AlwaysKept)
        );
        assert_eq!(
            CloudState::of_attributes(FILE_ATTRIBUTE_UNPINNED.0),
            Some(CloudState::Available)
        );
        assert_eq!(
            CloudState::of_attributes(FILE_ATTRIBUTE_OFFLINE.0),
            Some(CloudState::OnlineOnly)
        );
        // Read-only and archive say nothing of the cloud.
        assert_eq!(CloudState::of_attributes(0x1 | 0x20), None);
    }
}
