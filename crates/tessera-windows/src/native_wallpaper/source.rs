// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native file authority stays on the STA; only bounded display metadata escapes.

use std::mem::size_of;

use tessera_system::wallpaper::{WallpaperError, WallpaperPreview};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, GENERIC_READ, HANDLE, HWND};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAGS_AND_ATTRIBUTES, FILE_ID_INFO,
    FILE_INFO_BY_HANDLE_CLASS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_MODE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FILE_TYPE_DISK, FileAttributeTagInfo,
    FileBasicInfo, FileIdInfo, FileStandardInfo, GetFileInformationByHandleEx, GetFileType,
    OPEN_EXISTING,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_ALLOWMULTISELECT, FOS_DONTADDTORECENT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
    FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog,
    IShellItem, SICHINT_CANONICAL, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY,
};
use windows::core::{HRESULT, PCWSTR, PWSTR, w};

const MAX_PATH_UNITS: usize = 32_767;
const MAX_CAPTION_UNITS: usize = 512;
const MAX_CAPTION_CHARS: usize = 256;
const MAX_IMAGE_BYTES: i64 = 128 * 1024 * 1024;

pub(super) struct Image {
    item: IShellItem,
    path: NativeName,
    lease: FileLease,
    identity: FileIdentity,
    pub(super) caption: String,
}

impl Image {
    pub(super) fn choose(owner: HWND) -> Result<Option<Self>, WallpaperError> {
        let Some(dialog) = choose_dialog(owner, Pick::Image)? else {
            return Ok(None);
        };
        let item = unsafe { dialog.GetResult() }.map_err(|_| WallpaperError::Unavailable)?;
        Self::from_item(item).map(Some)
    }

    pub(super) fn from_item(item: IShellItem) -> Result<Self, WallpaperError> {
        let path = NativeName::take(
            unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
                .map_err(|_| WallpaperError::Unavailable)?,
            MAX_PATH_UNITS,
        )?;
        if !path.is_raster_name() {
            return Err(WallpaperError::Unavailable);
        }
        let lease = FileLease::open(&path)?;
        let identity = lease.identity()?;
        let caption = item_caption(&item)?;
        Ok(Self {
            item,
            path,
            lease,
            identity,
            caption,
        })
    }

    pub(super) fn path(&self) -> PCWSTR {
        self.path.as_pcwstr()
    }

    pub(super) fn preview(&self) -> Result<Option<WallpaperPreview>, WallpaperError> {
        // Keep this fresh exact-file lease across native handler acquisition,
        // decoding and bitmap copy, in addition to the selection's own lease.
        let _fresh_file = self.validate()?;
        super::thumbnail::capture(&self.item, || self.validate().map(|_| ()))
    }

    /// Retain the fresh exact-file lease across the SDK write and readback.
    pub(super) fn validate(&self) -> Result<FileLease, WallpaperError> {
        let fresh = NativeName::take(
            unsafe { self.item.GetDisplayName(SIGDN_FILESYSPATH) }
                .map_err(|_| WallpaperError::Unavailable)?,
            MAX_PATH_UNITS,
        )?;
        if fresh != self.path || self.lease.identity()? != self.identity {
            return Err(WallpaperError::Unavailable);
        }
        let file = FileLease::open(&fresh)?;
        if file.identity()? != self.identity {
            return Err(WallpaperError::Unavailable);
        }
        Ok(file)
    }

    pub(super) fn parent(&self) -> Result<IShellItem, WallpaperError> {
        unsafe { self.item.GetParent() }.map_err(|_| WallpaperError::Unavailable)
    }

    pub(super) fn is_in_container(&self, parent: &IShellItem) -> Result<bool, WallpaperError> {
        same_container(&self.item, parent)
    }

    pub(super) fn same_file(&self, other: &Self) -> bool {
        self.identity.volume == other.identity.volume && self.identity.file == other.identity.file
    }

    /// Compare actual file IDs only, not captions, pixels or image usability.
    /// An inaccessible native item is an unavailable observation, not a mismatch.
    pub(super) fn matches_item(&self, item: &IShellItem) -> Result<bool, WallpaperError> {
        Self::find_file(std::slice::from_ref(self), item).map(|index| index.is_some())
    }

    pub(super) fn find_file(
        images: &[Self],
        item: &IShellItem,
    ) -> Result<Option<usize>, WallpaperError> {
        let name = NativeName::take(
            unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
                .map_err(|_| WallpaperError::Unavailable)?,
            MAX_PATH_UNITS,
        )?;
        let file = FileLease::open(&name)?;
        let id = file.file_id()?;
        Ok(images.iter().position(|image| {
            id.VolumeSerialNumber == image.identity.volume
                && id.FileId.Identifier == image.identity.file
        }))
    }

    /// An SDK-returned path must open the actual selected file, not merely look
    /// like its caption/path. Cached or transcoded copies remain unconfirmed.
    pub(super) fn confirms(&self, wallpaper: PWSTR) -> bool {
        NativeName::take(wallpaper, MAX_PATH_UNITS)
            .and_then(|name| FileLease::open(&name))
            .and_then(|file| file.identity())
            .is_ok_and(|identity| identity == self.identity)
    }
}

/// Read-only native policy identity. Handles are transient and never retained
/// across observations; unlike protected image admission, this permits writers.
pub(super) struct PolicyEntry {
    item: IShellItem,
    identity: PolicyIdentity,
}

impl PolicyEntry {
    pub(super) fn capture(item: IShellItem) -> Result<Self, WallpaperError> {
        let path = NativeName::take(
            unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
                .map_err(|_| WallpaperError::Unavailable)?,
            MAX_PATH_UNITS,
        )?;
        let identity = Self::at_name(&path)?;
        Ok(Self { item, identity })
    }

    pub(super) fn wallpaper(path: PWSTR) -> Result<PolicyIdentity, WallpaperError> {
        let path = NativeName::take(path, MAX_PATH_UNITS)?;
        let identity = Self::at_name(&path)?;
        if identity.directory {
            return Err(WallpaperError::Unavailable);
        }
        Ok(identity)
    }

    fn at_name(path: &NativeName) -> Result<PolicyIdentity, WallpaperError> {
        FileLease::open_handle(
            path,
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
        )?
        .policy_identity()
    }

    pub(super) fn is_directory(&self) -> bool {
        self.identity.directory
    }

    pub(super) fn caption(&self) -> Result<String, WallpaperError> {
        item_caption(&self.item)
    }

    pub(super) fn same_identity(&self, other: &Self) -> bool {
        self.identity == other.identity
    }

    pub(super) fn same_object(&self, other: &Self) -> bool {
        self.identity.file.volume == other.identity.file.volume
            && self.identity.file.file == other.identity.file.file
    }

    pub(super) fn is_current(&self) -> bool {
        Self::capture(self.item.clone()).is_ok_and(|fresh| self.same_identity(&fresh))
    }

    pub(super) fn parent(&self) -> Result<Self, WallpaperError> {
        let parent = unsafe { self.item.GetParent() }.map_err(|_| WallpaperError::Unavailable)?;
        let parent = Self::capture(parent)?;
        if !parent.is_directory() {
            return Err(WallpaperError::Unavailable);
        }
        Ok(parent)
    }

    pub(super) fn is_in_container(&self, parent: &Self) -> Result<bool, WallpaperError> {
        // Canonical Shell equality supplements, never replaces filesystem IDs.
        Ok(same_container(&self.item, &parent.item)? && self.parent()?.same_identity(parent))
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct PolicyIdentity {
    file: FileIdentity,
    directory: bool,
    attributes: u32,
    created: i64,
}

#[derive(Clone, Copy)]
pub(super) enum Pick {
    Image,
    Images,
    Folder,
}

/// Shared real dialog setup; callers retain either GetResult or exact GetResults.
pub(super) fn choose_dialog(
    owner: HWND,
    pick: Pick,
) -> Result<Option<IFileOpenDialog>, WallpaperError> {
    // SAFETY: the caller owns an initialized STA and its own native HWND.
    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
            .map_err(|_| WallpaperError::Unavailable)?;
    (|| unsafe {
        let mut options = FOS_FORCEFILESYSTEM
            | FOS_FILEMUSTEXIST
            | FOS_PATHMUSTEXIST
            | FOS_NOCHANGEDIR
            | FOS_DONTADDTORECENT;
        match pick {
            Pick::Images => options |= FOS_ALLOWMULTISELECT,
            Pick::Folder => options |= FOS_PICKFOLDERS,
            Pick::Image => {}
        }
        dialog.SetOptions(options)?;
        if !matches!(pick, Pick::Folder) {
            dialog.SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: w!("Images (BMP, JPEG, PNG, TIFF)"),
                pszSpec: w!("*.bmp;*.jpg;*.jpeg;*.png;*.tif;*.tiff"),
            }])?;
        }
        let (title, button) = match pick {
            Pick::Image => (w!("Choose a static wallpaper image"), w!("Choose image")),
            Pick::Images => (
                w!("Choose 2–32 slideshow images from the same folder"),
                w!("Choose collection"),
            ),
            Pick::Folder => (w!("Choose a slideshow folder"), w!("Choose folder")),
        };
        dialog.SetTitle(title)?;
        dialog.SetOkButtonLabel(button)?;
        Ok::<_, windows::core::Error>(())
    })()
    .map_err(|_| WallpaperError::Unavailable)?;
    // Only documented cancellation is a successful empty selection.
    if let Err(error) = unsafe { dialog.Show(Some(owner)) } {
        return if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
            Ok(None)
        } else {
            Err(WallpaperError::Unavailable)
        };
    }
    Ok(Some(dialog))
}

fn item_caption(item: &IShellItem) -> Result<String, WallpaperError> {
    let display = NativeName::take(
        unsafe { item.GetDisplayName(SIGDN_NORMALDISPLAY) }
            .map_err(|_| WallpaperError::Unavailable)?,
        MAX_CAPTION_UNITS,
    )?;
    let caption: String = String::from_utf16(display.units())
        .map_err(|_| WallpaperError::Unavailable)?
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_CAPTION_CHARS)
        .collect();
    if caption.trim().is_empty() {
        return Err(WallpaperError::Unavailable);
    }
    Ok(caption)
}

pub(super) fn same_container(
    item: &IShellItem,
    parent: &IShellItem,
) -> Result<bool, WallpaperError> {
    let actual = unsafe { item.GetParent() }.map_err(|_| WallpaperError::Unavailable)?;
    unsafe { actual.Compare(parent, SICHINT_CANONICAL.0 as u32) }
        .map(|order| order == 0)
        .map_err(|_| WallpaperError::Unavailable)
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct NativeName(Vec<u16>);

impl NativeName {
    /// Takes ownership of a documented NUL-terminated COM task allocation.
    pub(super) fn take(value: PWSTR, limit: usize) -> Result<Self, WallpaperError> {
        let allocation = TaskString(value);
        if allocation.0.0.is_null() {
            return Err(WallpaperError::Unavailable);
        }
        let mut units = Vec::new();
        for index in 0..=limit {
            // SAFETY: SDK strings are NUL-terminated; never scan without a bound.
            let unit = unsafe { *allocation.0.0.add(index) };
            if unit == 0 {
                if units.is_empty() {
                    return Err(WallpaperError::Unavailable);
                }
                units.push(0);
                return Ok(Self(units));
            }
            units.push(unit);
        }
        Err(WallpaperError::Unavailable)
    }

    pub(super) fn as_pcwstr(&self) -> PCWSTR {
        PCWSTR(self.0.as_ptr())
    }

    pub(super) fn units(&self) -> &[u16] {
        &self.0[..self.0.len() - 1]
    }

    fn has_no_stream(&self) -> bool {
        let units = self.units();
        let colon = u16::from(b':');
        let drive = |unit: u16| {
            (u16::from(b'A')..=u16::from(b'Z')).contains(&unit)
                || (u16::from(b'a')..=u16::from(b'z')).contains(&unit)
        };
        let drive_colon = if units.len() > 1 && drive(units[0]) && units[1] == colon {
            Some(1)
        } else if units.len() > 5
            && units.starts_with(&[92, 92, 63, 92])
            && drive(units[4])
            && units[5] == colon
        {
            Some(5)
        } else {
            None
        };
        // File IDs identify a file, not its alternate named data streams.
        !units
            .iter()
            .enumerate()
            .any(|(index, unit)| *unit == colon && Some(index) != drive_colon)
    }

    fn is_raster_name(&self) -> bool {
        let name = self.units();
        let Some(dot) = name.iter().rposition(|unit| *unit == u16::from(b'.')) else {
            return false;
        };
        let extension = &name[dot + 1..];
        // The Open dialog's filter is guidance, not validation of typed names.
        ["bmp", "jpg", "jpeg", "png", "tif", "tiff"]
            .iter()
            .any(|allowed| {
                extension.len() == allowed.len()
                    && extension.iter().zip(allowed.bytes()).all(|(unit, byte)| {
                        let folded = if (u16::from(b'A')..=u16::from(b'Z')).contains(unit) {
                            *unit + u16::from(b'a' - b'A')
                        } else {
                            *unit
                        };
                        folded == u16::from(byte)
                    })
            })
    }
}

struct TaskString(PWSTR);

impl Drop for TaskString {
    fn drop(&mut self) {
        // SAFETY: uniquely owned SDK task allocation, freed on its owner STA.
        unsafe { CoTaskMemFree(Some(self.0.0.cast())) };
    }
}

pub(super) struct FileLease(HANDLE);

impl FileLease {
    fn open(path: &NativeName) -> Result<Self, WallpaperError> {
        // Read sharing only denies writers, deletion and replacement for the
        // entire protected selection lifetime. Preserve image admission exactly.
        Self::open_handle(
            path,
            GENERIC_READ.0,
            FILE_SHARE_READ,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    }

    fn open_handle(
        path: &NativeName,
        access: u32,
        share: FILE_SHARE_MODE,
        flags: FILE_FLAGS_AND_ATTRIBUTES,
    ) -> Result<Self, WallpaperError> {
        if !path.has_no_stream() {
            return Err(WallpaperError::Unavailable);
        }
        // SAFETY: terminated SDK path; final reparse point is opened for
        // rejection, not followed. This owns the only raw-handle open/close path.
        unsafe {
            CreateFileW(
                path.as_pcwstr(),
                access,
                share,
                None,
                OPEN_EXISTING,
                flags,
                None,
            )
        }
        .map(Self)
        .map_err(|_| WallpaperError::Unavailable)
    }

    fn identity(&self) -> Result<FileIdentity, WallpaperError> {
        if unsafe { GetFileType(self.0) } != FILE_TYPE_DISK {
            return Err(WallpaperError::Unavailable);
        }
        let attributes: FILE_ATTRIBUTE_TAG_INFO = self.information(FileAttributeTagInfo)?;
        let standard: FILE_STANDARD_INFO = self.information(FileStandardInfo)?;
        if attributes.FileAttributes & (FILE_ATTRIBUTE_DIRECTORY.0 | FILE_ATTRIBUTE_REPARSE_POINT.0)
            != 0
            || standard.Directory
            || standard.DeletePending
            || standard.EndOfFile <= 0
            || standard.EndOfFile > MAX_IMAGE_BYTES
            || standard.NumberOfLinks == 0
        {
            return Err(WallpaperError::Unavailable);
        }
        let id = self.file_id()?;
        if id.FileId.Identifier == [0; 16] {
            return Err(WallpaperError::Unavailable);
        }
        let basic: FILE_BASIC_INFO = self.information(FileBasicInfo)?;
        Ok(FileIdentity::from_native(&id, &standard, &basic))
    }

    fn policy_identity(&self) -> Result<PolicyIdentity, WallpaperError> {
        if unsafe { GetFileType(self.0) } != FILE_TYPE_DISK {
            return Err(WallpaperError::Unavailable);
        }
        let attributes: FILE_ATTRIBUTE_TAG_INFO = self.information(FileAttributeTagInfo)?;
        let standard: FILE_STANDARD_INFO = self.information(FileStandardInfo)?;
        let directory = attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
        if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
            || directory != standard.Directory
            || standard.DeletePending
            || standard.NumberOfLinks == 0
            || standard.EndOfFile < 0
        {
            return Err(WallpaperError::Unavailable);
        }
        let id = self.file_id()?;
        if id.VolumeSerialNumber == 0 || id.FileId.Identifier == [0; 16] {
            return Err(WallpaperError::Unavailable);
        }
        let basic: FILE_BASIC_INFO = self.information(FileBasicInfo)?;
        Ok(PolicyIdentity {
            file: FileIdentity::from_native(&id, &standard, &basic),
            directory,
            attributes: attributes.FileAttributes,
            created: basic.CreationTime,
        })
    }

    fn file_id(&self) -> Result<FILE_ID_INFO, WallpaperError> {
        self.information(FileIdInfo)
    }

    fn information<T: Default>(
        &self,
        class: FILE_INFO_BY_HANDLE_CLASS,
    ) -> Result<T, WallpaperError> {
        let mut value = T::default();
        // SAFETY: private callers pair the documented class and exact SDK struct.
        unsafe {
            GetFileInformationByHandleEx(
                self.0,
                class,
                (&mut value as *mut T).cast(),
                size_of::<T>() as u32,
            )
        }
        .map_err(|_| WallpaperError::Unavailable)?;
        Ok(value)
    }
}

impl Drop for FileLease {
    fn drop(&mut self) {
        // SAFETY: file handles never escape the owner STA; close exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

#[derive(PartialEq, Eq)]
struct FileIdentity {
    volume: u64,
    file: [u8; 16],
    bytes: i64,
    modified: i64,
    changed: i64,
}

impl FileIdentity {
    fn from_native(
        id: &FILE_ID_INFO,
        standard: &FILE_STANDARD_INFO,
        basic: &FILE_BASIC_INFO,
    ) -> Self {
        Self {
            volume: id.VolumeSerialNumber,
            file: id.FileId.Identifier,
            bytes: standard.EndOfFile,
            modified: basic.LastWriteTime,
            changed: basic.ChangeTime,
        }
    }
}
