// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The properties the Shell knows of a file, as Explorer shows them in
//! its details pane and its columns: the size of a picture, the length of
//! a song, the authors of a document, the camera a photo was taken with,
//! and the title, rating, tags and comment, which can be written too.
//!
//! The values come from the property store of the file
//! (`SHGetPropertyStoreFromParsingName`), which the property handler of
//! its kind fills, and are written as Windows writes them
//! (`PSFormatForDisplay`), in the language of Windows. Property handlers
//! open the file and some take long, so reading and writing each run on
//! a thread of their own, in a single-threaded apartment.
#![allow(unsafe_code)]

use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use async_channel::Receiver;
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_Author, PKEY_Comment, PKEY_DateCreated, PKEY_FileAttributes, PKEY_FileOwner,
    PKEY_Image_Dimensions, PKEY_Keywords, PKEY_Media_Duration, PKEY_Photo_CameraManufacturer,
    PKEY_Photo_CameraModel, PKEY_Photo_DateTaken, PKEY_Rating, PKEY_Title,
};
use windows::Win32::System::Com::CoTaskMemAlloc;
use windows::Win32::System::Com::StructuredStorage::{
    InitPropVariantFromStringAsVector, PROPVARIANT, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
    PropVariantClear, PropVariantToUInt32, PropVariantToUInt64,
};
use windows::Win32::System::Variant::{
    VT_EMPTY, VT_FILETIME, VT_I1, VT_I2, VT_I4, VT_I8, VT_INT, VT_LPWSTR, VT_UI1, VT_UI2, VT_UI4,
    VT_UI8, VT_UINT,
};
use windows::Win32::UI::Shell::PropertiesSystem::{
    GPS_BESTEFFORT, GPS_DEFAULT, GPS_READWRITE, IPropertyStore, PDFF_DEFAULT,
    PSFormatForDisplayAlloc, SHGetPropertyStoreFromParsingName,
};
use windows::core::{HSTRING, PWSTR};

use super::ShellThread;
use super::shell_item::wide;
use super::shell_items::take_text;

/// How often a file held open by another program is tried, and how long
/// is waited between the tries.
const WRITE_TRIES: u32 = 4;
const WRITE_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);

/// A property of a file that Tessera shows, and for some writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileProperty {
    Title,
    Authors,
    Rating,
    Tags,
    Comment,
    /// How wide and high a picture is.
    Dimensions,
    /// How long a song or a video plays: Explorer's column "Length".
    Length,
    DateTaken,
    CameraMaker,
    CameraModel,
    DateCreated,
    Owner,
    /// The attributes as Explorer writes them, as `RA` for read-only and
    /// archive.
    Attributes,
}

impl FileProperty {
    /// What the details pane shows, in its order.
    pub const DETAILS: [Self; 13] = [
        Self::Title,
        Self::Authors,
        Self::Rating,
        Self::Tags,
        Self::Comment,
        Self::Dimensions,
        Self::Length,
        Self::DateTaken,
        Self::CameraMaker,
        Self::CameraModel,
        Self::DateCreated,
        Self::Owner,
        Self::Attributes,
    ];

    /// What can be a column of the details, in the order the chooser
    /// lists them.
    pub const COLUMNS: [Self; 7] = [
        Self::DateCreated,
        Self::DateTaken,
        Self::Dimensions,
        Self::Length,
        Self::Authors,
        Self::Owner,
        Self::Attributes,
    ];

    /// What it is called, in English, as the table of texts has it.
    pub fn title(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::Authors => "Authors",
            Self::Rating => "Rating",
            Self::Tags => "Tags",
            Self::Comment => "Comments",
            Self::Dimensions => "Dimensions",
            Self::Length => "Length",
            Self::DateTaken => "Date taken",
            Self::CameraMaker => "Camera maker",
            Self::CameraModel => "Camera model",
            Self::DateCreated => "Date created",
            Self::Owner => "Owner",
            Self::Attributes => "Attributes",
        }
    }

    /// What the files of Tessera call it, as `date-taken`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Authors => "authors",
            Self::Rating => "rating",
            Self::Tags => "tags",
            Self::Comment => "comment",
            Self::Dimensions => "dimensions",
            Self::Length => "length",
            Self::DateTaken => "date-taken",
            Self::CameraMaker => "camera-maker",
            Self::CameraModel => "camera-model",
            Self::DateCreated => "date-created",
            Self::Owner => "owner",
            Self::Attributes => "attributes",
        }
    }

    /// The property the files of Tessera call `name`.
    pub fn named(name: &str) -> Option<Self> {
        Self::DETAILS
            .into_iter()
            .find(|property| property.name() == name)
    }

    /// Whether the details pane lets the user write it.
    pub fn editable(self) -> bool {
        matches!(
            self,
            Self::Title | Self::Rating | Self::Tags | Self::Comment
        )
    }

    fn key(self) -> PROPERTYKEY {
        match self {
            Self::Title => PKEY_Title,
            Self::Authors => PKEY_Author,
            Self::Rating => PKEY_Rating,
            Self::Tags => PKEY_Keywords,
            Self::Comment => PKEY_Comment,
            Self::Dimensions => PKEY_Image_Dimensions,
            Self::Length => PKEY_Media_Duration,
            Self::DateTaken => PKEY_Photo_DateTaken,
            Self::CameraMaker => PKEY_Photo_CameraManufacturer,
            Self::CameraModel => PKEY_Photo_CameraModel,
            Self::DateCreated => PKEY_DateCreated,
            Self::Owner => PKEY_FileOwner,
            Self::Attributes => PKEY_FileAttributes,
        }
    }
}

/// A value of a property of a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PropertyValue {
    /// As Windows writes it, in the language of Windows.
    pub shown: String,
    /// For the rating, how many stars of five it gives.
    pub stars: Option<u8>,
    /// What it is ordered by where it is a number or a moment, which its
    /// text does not order by: the number, or the moment in the 100
    /// nanoseconds Windows counts.
    pub order: Option<u64>,
}

impl PropertyValue {
    /// The tags of a value of [`FileProperty::Tags`], which Windows writes
    /// separated by semicolons.
    pub fn items(&self) -> Vec<String> {
        split_items(&self.shown)
    }
}

/// What a property is to be written as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropertyEdit {
    /// A text; an empty one removes the property.
    Text(String),
    /// A rating of 0 to 5 stars; 0 removes it.
    Stars(u8),
    /// A list, as the tags are; an empty one removes it.
    Items(Vec<String>),
}

/// The properties of files.
pub struct Properties;

impl Properties {
    /// Reads `wanted` of each of `paths`, on a thread of its own. The
    /// receiver yields once: for each path, a value for each of `wanted`,
    /// `None` where the file has none.
    pub fn read(
        paths: Vec<PathBuf>,
        wanted: Vec<FileProperty>,
    ) -> Receiver<Vec<Vec<Option<PropertyValue>>>> {
        ShellThread::call_alone("file properties", move || {
            { paths.iter() }
                .map(|path| read_values(path, &wanted))
                .collect()
        })
    }

    /// Writes `edits` into the file at `path`, all at once, on a thread of
    /// its own. The receiver yields once.
    pub fn write(
        path: PathBuf,
        edits: Vec<(FileProperty, PropertyEdit)>,
    ) -> Receiver<Result<(), String>> {
        ShellThread::call_alone("file properties", move || {
            // A file another program holds open, as a preview handler
            // letting go of it, is tried again a little later.
            let mut tries = 1;
            loop {
                match write_values(&path, &edits) {
                    Err(error) if tries < WRITE_TRIES && shared_away(&error) => {
                        tries += 1;
                        std::thread::sleep(WRITE_PAUSE);
                    }
                    written => return written.map_err(|error| format!("{error:#}")),
                }
            }
        })
    }
}

/// Whether `error` says the file is held open by another program.
fn shared_away(error: &anyhow::Error) -> bool {
    use windows::Win32::Foundation::{
        ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, STG_E_LOCKVIOLATION, STG_E_SHAREVIOLATION,
    };
    let held = [
        ERROR_SHARING_VIOLATION.to_hresult(),
        ERROR_LOCK_VIOLATION.to_hresult(),
        STG_E_SHAREVIOLATION,
        STG_E_LOCKVIOLATION,
    ];
    { error.chain() }
        .filter_map(|cause| cause.downcast_ref::<windows::core::Error>())
        .any(|cause| held.contains(&cause.code()))
}

/// The property store of the file at `path`.
fn store(path: &Path, writing: bool) -> Result<IPropertyStore> {
    let name = HSTRING::from(path.to_string_lossy().replace('/', "\\"));
    let flags = match writing {
        true => GPS_READWRITE,
        false => GPS_DEFAULT | GPS_BESTEFFORT,
    };
    // SAFETY: COM is initialized on this thread (`call_alone`) and the
    // name outlives the call.
    unsafe { SHGetPropertyStoreFromParsingName(&name, None, flags) }
        .with_context(|| format!("the properties of {} cannot be read", path.display()))
}

/// The values of `wanted` of the file at `path`; none where its store
/// cannot be read.
fn read_values(path: &Path, wanted: &[FileProperty]) -> Vec<Option<PropertyValue>> {
    let Ok(store) = store(path, false) else {
        return vec![None; wanted.len()];
    };
    { wanted.iter() }
        .map(|&property| read_value(&store, property))
        .collect()
}

fn read_value(store: &IPropertyStore, property: FileProperty) -> Option<PropertyValue> {
    let key = property.key();
    // SAFETY: the key and the value outlive the calls that read them, and
    // the value is cleared once it was read; the text Windows writes is
    // freed by `take_text`.
    unsafe {
        let mut value = store.GetValue(&raw const key).ok()?;
        let empty = value.Anonymous.Anonymous.vt == VT_EMPTY;
        let read = (!empty).then(|| {
            let shown = { PSFormatForDisplayAlloc(&raw const key, &raw const value, PDFF_DEFAULT) }
                .ok()
                .and_then(|text| take_text(text))
                .unwrap_or_default();
            let stars = { property == FileProperty::Rating }
                .then(|| PropVariantToUInt32(&raw const value).ok())
                .flatten()
                .map(stars_of);
            let kind = value.Anonymous.Anonymous.vt;
            let order = match kind {
                VT_FILETIME => {
                    let time = value.Anonymous.Anonymous.Anonymous.filetime;
                    Some(u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime))
                }
                VT_UI1 | VT_UI2 | VT_UI4 | VT_UI8 | VT_I1 | VT_I2 | VT_I4 | VT_I8 | VT_UINT
                | VT_INT => PropVariantToUInt64(&raw const value).ok(),
                _ => None,
            };
            PropertyValue {
                shown,
                stars,
                order,
            }
        });
        let _ = PropVariantClear(&raw mut value);
        read.filter(|value| !value.shown.trim().is_empty() || value.stars.is_some())
    }
}

/// Writes `edits` into the file at `path` and commits them.
fn write_values(path: &Path, edits: &[(FileProperty, PropertyEdit)]) -> Result<()> {
    let store = store(path, true)?;
    for (property, edit) in edits {
        let key = property.key();
        let mut value = value_of(edit)?;
        // SAFETY: the key and the value outlive the call, and the value is
        // cleared after it, which frees what it holds.
        let set = unsafe {
            let set = store.SetValue(&raw const key, &raw const value);
            let _ = PropVariantClear(&raw mut value);
            set
        };
        set.with_context(|| format!("{} cannot be written", property.title()))?;
    }
    // SAFETY: the store is used on the thread that opened it.
    unsafe { store.Commit() }.context("the file did not take its new properties")
}

/// The value Windows keeps for `edit`.
fn value_of(edit: &PropertyEdit) -> Result<PROPVARIANT> {
    let mut value = PROPVARIANT::default();
    match edit {
        PropertyEdit::Text(text) if text.trim().is_empty() => {}
        PropertyEdit::Text(text) => {
            let text = wide(text.trim().as_ref());
            let bytes = text.len() * size_of::<u16>();
            // SAFETY: the memory has room for the text with its NUL, which
            // `wide` ends it with; `PropVariantClear` frees it.
            let held = unsafe {
                let held = CoTaskMemAlloc(bytes).cast::<u16>();
                anyhow::ensure!(!held.is_null(), "there is no memory for the text");
                std::ptr::copy_nonoverlapping(text.as_ptr(), held, text.len());
                held
            };
            value.Anonymous.Anonymous = ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_LPWSTR,
                Anonymous: PROPVARIANT_0_0_0 {
                    pwszVal: PWSTR(held),
                },
                ..Default::default()
            });
        }
        PropertyEdit::Stars(0) => {}
        PropertyEdit::Stars(stars) => {
            value.Anonymous.Anonymous = ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_UI4,
                Anonymous: PROPVARIANT_0_0_0 {
                    ulVal: rating_of(*stars),
                },
                ..Default::default()
            });
        }
        PropertyEdit::Items(items) => {
            let joined = join_items(items);
            if !joined.is_empty() {
                let joined = HSTRING::from(joined);
                // SAFETY: the text outlives the call; the value it makes is
                // the caller's to clear.
                value = unsafe { InitPropVariantFromStringAsVector(&joined) }
                    .context("the tags cannot be written")?;
            }
        }
    }
    Ok(value)
}

/// The stars of five a rating of Windows gives: it keeps 1, 25, 50, 75 and
/// 99 for one to five stars, and reads anything between as the nearest
/// lower.
pub fn stars_of(rating: u32) -> u8 {
    match rating {
        0 => 0,
        1..=12 => 1,
        13..=37 => 2,
        38..=62 => 3,
        63..=87 => 4,
        _ => 5,
    }
}

/// The rating Windows keeps for `stars` of five.
pub fn rating_of(stars: u8) -> u32 {
    match stars {
        0 => 0,
        1 => 1,
        2 => 25,
        3 => 50,
        4 => 75,
        _ => 99,
    }
}

/// The items of a list Windows writes separated by semicolons.
pub fn split_items(text: &str) -> Vec<String> {
    { text.split(';') }
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Items joined as Windows reads a list: separated by semicolons, without
/// the empty ones.
fn join_items(items: &[String]) -> String {
    let items: Vec<&str> = { items.iter() }
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect();
    items.join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_and_ratings_turn_into_each_other() {
        for stars in 0..=5 {
            assert_eq!(stars_of(rating_of(stars)), stars);
        }
        assert_eq!(stars_of(60), 3);
        assert_eq!(stars_of(100), 5);
    }

    #[test]
    fn tags_are_read_and_written_with_semicolons() {
        assert_eq!(
            split_items("holiday; sea ;;  2026"),
            ["holiday", "sea", "2026"]
        );
        let items = vec![" a ".to_owned(), String::new(), "b c".to_owned()];
        assert_eq!(join_items(&items), "a;b c");
        assert!(split_items("").is_empty());
    }

    #[test]
    fn only_the_title_rating_tags_and_comment_are_written() {
        let editable: Vec<FileProperty> = { FileProperty::DETAILS.into_iter() }
            .filter(|property| property.editable())
            .collect();
        assert_eq!(
            editable,
            [
                FileProperty::Title,
                FileProperty::Rating,
                FileProperty::Tags,
                FileProperty::Comment
            ]
        );
    }
}
