// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Recycle Bin: how much lies in it, and the window that shows it.

use std::io;
use std::process::Command;

use windows::Win32::UI::Shell::{SHQUERYRBINFO, SHQueryRecycleBinW};
use windows::core::PCWSTR;

/// The Recycle Bin of the user, over all drives.
pub struct RecycleBin;

impl RecycleBin {
    /// How many items lie in the bin, or `None` if Windows does not tell.
    pub fn items() -> Option<u64> {
        let mut info = SHQUERYRBINFO {
            cbSize: size_of::<SHQUERYRBINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: no path asks for the bins of all drives, and `info`
        // outlives the call and says its own size.
        unsafe { SHQueryRecycleBinW(PCWSTR::null(), &raw mut info) }.ok()?;
        u64::try_from(info.i64NumItems).ok()
    }

    /// Has Explorer open a window on the bin.
    pub fn open() -> io::Result<()> {
        Command::new("explorer.exe")
            .arg("shell:RecycleBinFolder")
            .spawn()
            .map(drop)
    }
}
