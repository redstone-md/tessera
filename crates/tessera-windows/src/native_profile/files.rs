// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::io::Read;
use std::mem::size_of;
use tessera_system::profile::ProfileError;
use windows::Win32::Foundation::{GENERIC_READ, HANDLE};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES,
    FILE_READ_DATA, FILE_SHARE_READ, FILE_TYPE_DISK, FileIdInfo, GETFINALPATHNAMEBYHANDLE_FLAGS,
    GetDriveTypeW, GetFileInformationByHandle, GetFileInformationByHandleEx, GetFileType,
    GetFinalPathNameByHandleW, OPEN_EXISTING, ReadFile, VOLUME_NAME_DOS,
};
use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;
use windows::core::PCWSTR;

use super::handle::OwnedHandle as Handle;
use super::{native_error, wide};
use crate::profile::source::{
    DirectoryIdentity, bounded_bytes, invalid, strict_utf16, validate_local_path,
};

/// All ancestor directories are opened without following reparse points and
/// kept alive, denying writes/deletion while the final object is used. Rejecting
/// only the leaf is insufficient: an ancestor junction can otherwise reach UNC.
pub(super) struct LocalPath {
    handles: Vec<Handle>,
    info: BY_HANDLE_FILE_INFORMATION,
    identity: DirectoryIdentity,
}

impl LocalPath {
    pub(super) fn directory(path: &str) -> Result<Self, ProfileError> {
        Self::open(path, false)
    }

    fn open(path: &str, image: bool) -> Result<Self, ProfileError> {
        validate_local_path(path, image)?;
        let root = wide(&path[..3]);
        // SAFETY: explicit plain drive root, never a current-directory/network
        // query. Fail closed for mapped/unknown/removable/nonfixed sources.
        if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != DRIVE_FIXED {
            return Err(invalid());
        }
        let mut handles = Vec::new();
        let mut prefixes = vec![3];
        prefixes
            .extend(path.char_indices().filter_map(|(index, character)| {
                (index > 2 && character == '\\').then_some(index)
            }));
        prefixes.push(path.len());
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        let mut root_volume = None;
        let mut identity = None;
        for length in prefixes {
            let leaf = length == path.len();
            let prefix = wide(&path[..length]);
            // Directory READ_DATA is LIST_DIRECTORY access only; we never list.
            // Attributes-only opens do not enforce sharing restrictions, so this
            // read access is necessary to deny concurrent write/delete handles.
            let access = if leaf && image {
                GENERIC_READ.0
            } else {
                FILE_READ_ATTRIBUTES.0 | FILE_READ_DATA.0
            };
            // SAFETY: already validated local spelling; OPEN_EXISTING only,
            // no inheritance/writes/creation, no reparse-following or recall.
            let raw = unsafe {
                CreateFileW(
                    PCWSTR(prefix.as_ptr()),
                    access,
                    FILE_SHARE_READ,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OPEN_REPARSE_POINT
                        | FILE_FLAG_BACKUP_SEMANTICS
                        | FILE_FLAG_OPEN_NO_RECALL,
                    None,
                )
            }
            .map_err(native_error)?;
            let handle = Handle::new(raw)?;
            info = information(raw)?;
            validate_information(raw, &info, !(leaf && image))?;
            let fresh_identity = file_identity(raw)?;
            if root_volume.is_some_and(|volume| volume != fresh_identity.volume) {
                return Err(invalid());
            }
            root_volume.get_or_insert(fresh_identity.volume);
            identity = Some(fresh_identity);
            handles.push(handle);
        }
        let leaf = handles.last().ok_or_else(invalid)?.raw()?;
        final_local_path(leaf, path, image)?;
        Ok(Self {
            handles,
            info,
            identity: identity.ok_or_else(invalid)?,
        })
    }

    pub(super) fn identity(&self) -> DirectoryIdentity {
        self.identity
    }

    pub(super) fn close(mut self) -> Result<(), ProfileError> {
        let mut failure = None;
        while let Some(handle) = self.handles.pop() {
            if let Err(error) = handle.close() {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

fn file_identity(raw: HANDLE) -> Result<DirectoryIdentity, ProfileError> {
    let mut info = FILE_ID_INFO::default();
    // SAFETY: live held disk object; typed initialized output has the exact
    // pinned SDK size. Full-width IDs avoid truncating ReFS identities to 64 bits.
    unsafe {
        GetFileInformationByHandleEx(
            raw,
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    }
    .map_err(native_error)?;
    if info.FileId.Identifier == [0; 16] {
        return Err(invalid());
    }
    Ok(DirectoryIdentity {
        volume: info.VolumeSerialNumber,
        file_id: info.FileId.Identifier,
    })
}

fn validate_information(
    raw: HANDLE,
    info: &BY_HANDLE_FILE_INFORMATION,
    directory: bool,
) -> Result<(), ProfileError> {
    let rejected = FILE_ATTRIBUTE_DEVICE.0
        | FILE_ATTRIBUTE_OFFLINE.0
        | FILE_ATTRIBUTE_REPARSE_POINT.0
        | FILE_ATTRIBUTE_RECALL_ON_OPEN.0
        | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS.0;
    // SAFETY: live read-only owned object, no content access by these queries.
    if unsafe { GetFileType(raw) } != FILE_TYPE_DISK
        || info.dwFileAttributes & rejected != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0) != directory
    {
        return Err(invalid());
    }
    Ok(())
}

fn information(raw: HANDLE) -> Result<BY_HANDLE_FILE_INFORMATION, ProfileError> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: a live disk object handle, initialized output of the pinned shape.
    unsafe { GetFileInformationByHandle(raw, &mut info) }.map_err(native_error)?;
    Ok(info)
}

fn final_local_path(raw: HANDLE, expected: &str, image: bool) -> Result<(), ProfileError> {
    let mut buffer = vec![0u16; 32_768];
    // SAFETY: live owned handle, bounded initialized output. All ancestors are
    // already local/nonreparse; no SMB normalized-component queries are reached.
    let length = unsafe {
        GetFinalPathNameByHandleW(
            raw,
            &mut buffer,
            GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0),
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(invalid());
    }
    let final_path = strict_utf16(&buffer[..=length])?;
    let local = final_path.strip_prefix("\\\\?\\").ok_or_else(invalid)?;
    validate_local_path(local, image)?;
    if !local[..2].eq_ignore_ascii_case(&expected[..2]) {
        return Err(invalid());
    }
    Ok(())
}

struct FileReader(HANDLE);

impl Read for FileReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut count = 0u32;
        // SAFETY: synchronous read on our live guarded handle, bounded mutable
        // output, no OVERLAPPED structure or cross-thread native storage.
        unsafe { ReadFile(self.0, Some(buffer), Some(&mut count), None) }.map_err(|error| {
            std::io::Error::from_raw_os_error((error.code().0 as u32 & 0xffff) as i32)
        })?;
        let count = count as usize;
        if count > buffer.len() {
            return Err(std::io::Error::other("Invalid profile read size"));
        }
        Ok(count)
    }
}

pub(super) fn photo_bytes(path: &str) -> Result<Vec<u8>, ProfileError> {
    let guard = LocalPath::open(path, true)?;
    let raw = guard.handles.last().ok_or_else(invalid)?.raw()?;
    let before = guard.info;
    let declared = (u64::from(before.nFileSizeHigh) << 32) | u64::from(before.nFileSizeLow);
    let result = bounded_bytes(&mut FileReader(raw), declared).and_then(|bytes| {
        let after = information(raw)?;
        if file_identity(raw)? != guard.identity() {
            return Err(invalid());
        }
        // Last-access time can change just because we read. Authority, size,
        // creation/write times and attributes must remain exactly unchanged.
        if before.nFileSizeHigh != after.nFileSizeHigh
            || before.nFileSizeLow != after.nFileSizeLow
            || before.dwFileAttributes != after.dwFileAttributes
            || before.ftCreationTime != after.ftCreationTime
            || before.ftLastWriteTime != after.ftLastWriteTime
        {
            return Err(invalid());
        }
        Ok(bytes)
    });
    let cleanup = guard.close();
    result.and_then(|value| cleanup.map(|()| value))
}
