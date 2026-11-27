// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Renaming a file without putting it over another, which the rename of
//! the standard library of Rust does on Windows.

#![allow(unsafe_code)]

use std::io;
use std::path::Path;

use windows::Win32::Storage::FileSystem::{MOVE_FILE_FLAGS, MoveFileExW};
use windows::core::HSTRING;

/// Gives the file or folder at `from` the path `to`, on the same drive,
/// and fails where something is at `to` already, which is left as it is.
/// A rename that changes only capitals is done. Blocks on the disk.
pub fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (
        HSTRING::from(from.as_os_str()),
        HSTRING::from(to.as_os_str()),
    );
    // SAFETY: both paths are NUL-terminated and outlive the call; without
    // MOVEFILE_REPLACE_EXISTING Windows refuses to replace a file.
    unsafe { MoveFileExW(&from, &to, MOVE_FILE_FLAGS(0)) }
        // MoveFileExW fails with the error of the thread, kept in the
        // low word of the HRESULT.
        .map_err(|error| io::Error::from_raw_os_error(error.code().0 & 0xffff))
}
