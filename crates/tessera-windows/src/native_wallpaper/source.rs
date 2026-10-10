// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native file authority stays on the STA; only bounded display text escapes.

use std::mem::size_of;

use tessera_system::wallpaper::WallpaperError;
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, GENERIC_READ, HANDLE, HWND};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_BASIC_INFO, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
    FILE_INFO_BY_HANDLE_CLASS, FILE_SHARE_READ, FILE_STANDARD_INFO, FILE_TYPE_DISK,
    FileAttributeTagInfo, FileBasicInfo, FileIdInfo, FileStandardInfo,
    GetFileInformationByHandleEx, GetFileType, OPEN_EXISTING,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_DONTADDTORECENT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR,
    FOS_PATHMUSTEXIST, FileOpenDialog, IFileOpenDialog, IShellItem, SIGDN_FILESYSPATH,
    SIGDN_NORMALDISPLAY,
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
        // SAFETY: the caller owns an initialized STA and its own native HWND.
        let dialog: IFileOpenDialog =
            unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
                .map_err(|_| WallpaperError::Unavailable)?;
        (|| unsafe {
            dialog.SetOptions(
                FOS_FORCEFILESYSTEM
                    | FOS_FILEMUSTEXIST
                    | FOS_PATHMUSTEXIST
                    | FOS_NOCHANGEDIR
                    | FOS_DONTADDTORECENT,
            )?;
            dialog.SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: w!("Static images (BMP, JPEG, PNG, TIFF)"),
                pszSpec: w!("*.bmp;*.jpg;*.jpeg;*.png;*.tif;*.tiff"),
            }])?;
            dialog.SetTitle(w!("Choose a static wallpaper image"))?;
            dialog.SetOkButtonLabel(w!("Choose image"))?;
            Ok::<_, windows::core::Error>(())
        })()
        .map_err(|_| WallpaperError::Unavailable)?;
        // Show is the actual SDK dialog, not suppressed or owned by another
        // application's window. Only its documented cancellation maps to None.
        if let Err(error) = unsafe { dialog.Show(Some(owner)) } {
            return if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                Ok(None)
            } else {
                Err(WallpaperError::Unavailable)
            };
        }
        let item = unsafe { dialog.GetResult() }.map_err(|_| WallpaperError::Unavailable)?;
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
        Ok(Some(Self {
            item,
            path,
            lease,
            identity,
            caption,
        }))
    }

    pub(super) fn path(&self) -> PCWSTR {
        self.path.as_pcwstr()
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

    /// An SDK-returned path must open the actual selected file, not merely look
    /// like its caption/path. Cached or transcoded copies remain unconfirmed.
    pub(super) fn confirms(&self, wallpaper: PWSTR) -> bool {
        NativeName::take(wallpaper, MAX_PATH_UNITS)
            .and_then(|name| FileLease::open(&name))
            .and_then(|file| file.identity())
            .is_ok_and(|identity| identity == self.identity)
    }
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
        if !path.has_no_stream() {
            return Err(WallpaperError::Unavailable);
        }
        // SAFETY: owned, terminated SDK path. Read sharing only denies writers,
        // deletion and replacement for the entire admitted selection lifetime.
        // Opening the final reparse point itself lets validation reject it.
        unsafe {
            CreateFileW(
                path.as_pcwstr(),
                GENERIC_READ.0,
                FILE_SHARE_READ,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
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
        let id: FILE_ID_INFO = self.information(FileIdInfo)?;
        if id.FileId.Identifier == [0; 16] {
            return Err(WallpaperError::Unavailable);
        }
        let basic: FILE_BASIC_INFO = self.information(FileBasicInfo)?;
        Ok(FileIdentity {
            volume: id.VolumeSerialNumber,
            file: id.FileId.Identifier,
            bytes: standard.EndOfFile,
            modified: basic.LastWriteTime,
            changed: basic.ChangeTime,
        })
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
