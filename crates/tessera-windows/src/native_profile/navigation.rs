// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::ffi::c_void;
use std::mem::size_of;
use tessera_system::profile::ProfileError;
use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx};
use windows::Win32::UI::Shell::{
    FOLDERID_Profile, IShellItem, KF_FLAG_DEFAULT, SEE_MASK_FLAG_NO_UI, SEE_MASK_IDLIST,
    SEE_MASK_NOASYNC, SHCreateItemFromParsingName, SHELLEXECUTEINFOW, SHGetIDListFromObject,
    SHGetKnownFolderItem, ShellExecuteExW,
};
use windows::core::{PCWSTR, w};

use super::{identity, native_error, wide};
use crate::profile::source::{ACCOUNTS_URI, invalid};

struct Pidl(*mut c_void);

impl Drop for Pidl {
    fn drop(&mut self) {
        // SAFETY: unique absolute PIDL returned by the Shell task allocator;
        // API has no fallible close result, release before MTA retirement.
        unsafe { CoTaskMemFree(Some(self.0)) };
    }
}

pub(super) fn home() -> Result<(), ProfileError> {
    identity::self_context()?;
    // SAFETY: fixed current-user known-folder identity; no create/default path,
    // impersonation token or UI-provided string. Borrowed item remains owner-local.
    let item: IShellItem =
        unsafe { SHGetKnownFolderItem(&FOLDERID_Profile, KF_FLAG_DEFAULT, None) }
            .map_err(native_error)?;
    dispatch(&item)
}

pub(super) fn directory(path: &str) -> Result<(), ProfileError> {
    identity::self_context()?;
    let path = wide(path);
    // SAFETY: called only with a freshly local/nonreparse real directory whose
    // ancestors and identity are held by files::LocalPath through dispatch.
    let item: IShellItem =
        unsafe { SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None::<&IBindCtx>) }
            .map_err(native_error)?;
    dispatch(&item)
}

fn dispatch(item: &IShellItem) -> Result<(), ProfileError> {
    // SAFETY: same-owner live shell item; owned absolute PIDL is not sent to UI.
    let raw = unsafe { SHGetIDListFromObject(item) }.map_err(native_error)?;
    if raw.is_null() {
        return Err(invalid());
    }
    let pidl = Pidl(raw.cast());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_IDLIST | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("open"),
        lpIDList: pidl.0,
        nShow: 1,
        ..Default::default()
    };
    // SAFETY: fixed directory intent, freshly resolved absolute PIDL, no path,
    // executable, parameters or working directory supplied to ShellExecute.
    unsafe { ShellExecuteExW(&mut info) }.map_err(native_error)
}

pub(super) fn accounts() -> Result<(), ProfileError> {
    identity::self_context()?;
    let uri = wide(ACCOUNTS_URI);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpFile: PCWSTR(uri.as_ptr()),
        nShow: 1,
        ..Default::default()
    };
    // SAFETY: single source-derived fixed URI, no UI URI/command/parameters or
    // interpreter transport. Success means native dispatch, not window visibility.
    unsafe { ShellExecuteExW(&mut info) }.map_err(native_error)
}
