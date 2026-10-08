// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Windows 0.62.2 shell calls, confined to the folder worker's strict STA.

use std::ffi::c_void;
use std::marker::PhantomData;
use std::mem::size_of;
use std::rc::Rc;

use tessera_system::folders::{FolderError, FolderErrorKind, FolderId};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    IShellItem, KF_FLAG_DEFAULT, SEE_MASK_FLAG_NO_UI, SEE_MASK_IDLIST, SEE_MASK_NOASYNC,
    SHELLEXECUTEINFOW, SHGetIDListFromObject, SHGetKnownFolderItem, SICHINT_CANONICAL,
    SIGDN_DESKTOPABSOLUTEPARSING, ShellExecuteExW,
};
use windows::core::{GUID, w};

use crate::folders::worker::{Driver, known_folder_guid, native_error};

/// Unlike the application enumeration's read-only inherited apartment, this
/// worker requires its own STA. Every successful initialization is balanced.
struct Apartment {
    _thread_bound: PhantomData<Rc<()>>,
}

impl Apartment {
    fn enter() -> Result<Self, FolderError> {
        // SAFETY: called only by the new dedicated worker, no reserved pointer.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        result
            .ok()
            .map_err(|error| native_error(error.code().0, "Folder COM initialization"))?;
        Ok(Self {
            _thread_bound: PhantomData,
        })
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances successful initialization on this exact worker.
        unsafe { CoUninitialize() };
    }
}

/// Shell-task allocator ownership for PIDLs and GetDisplayName strings.
struct ShellAllocation(*mut c_void);

impl Drop for ShellAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: both allocation sources use the COM task allocator. This
            // sole owner frees once, after comparison/dispatch has finished.
            unsafe { CoTaskMemFree(Some(self.0)) };
        }
    }
}

pub(crate) struct NativeTarget {
    item: IShellItem,
    pidl: ShellAllocation,
    canonical: Vec<u16>,
}

pub(crate) struct NativeFolderDriver {
    _apartment: Apartment,
}

impl NativeFolderDriver {
    pub(crate) fn new() -> Result<Self, FolderError> {
        Ok(Self {
            _apartment: Apartment::enter()?,
        })
    }
}

impl Driver for NativeFolderDriver {
    type Target = NativeTarget;

    fn resolve(&mut self, folder: FolderId) -> Result<Self::Target, FolderError> {
        let guid = GUID::from_u128(known_folder_guid(folder));
        // SAFETY: closed, pinned known-folder GUID; current user, DEFAULT only.
        // No creation, unverified default paths, impersonation or UI input.
        let item: IShellItem = unsafe { SHGetKnownFolderItem(&guid, KF_FLAG_DEFAULT, None) }
            .map_err(|error| native_error(error.code().0, "Folder resolution"))?;
        let canonical = canonical_identity(&item)?;
        // SAFETY: item belongs to this live STA; API returns an owned absolute
        // PIDL backed by the shell task allocator, never sent to another thread.
        let pidl = unsafe { SHGetIDListFromObject(&item) }
            .map_err(|error| native_error(error.code().0, "Folder identity resolution"))?;
        let pidl = ShellAllocation(pidl.cast());
        if pidl.0.is_null() {
            return Err(FolderError::new(
                FolderErrorKind::Unavailable,
                "The folder did not provide a shell identity.",
            ));
        }
        Ok(NativeTarget {
            item,
            pidl,
            canonical,
        })
    }

    fn same_target(
        &mut self,
        expected: &Self::Target,
        fresh: &Self::Target,
    ) -> Result<bool, FolderError> {
        // Keep the canonical parsing identity as well as the shell comparison:
        // a known-folder wrapper must not silently redirect an old expectation.
        if expected.canonical != fresh.canonical {
            return Ok(false);
        }
        // SAFETY: both shell items were created and retained in this STA.
        unsafe {
            expected
                .item
                .Compare(&fresh.item, SICHINT_CANONICAL.0 as u32)
        }
        .map(|ordering| ordering == 0)
        .map_err(|error| native_error(error.code().0, "Folder identity comparison"))
    }

    fn dispatch(&mut self, fresh: &Self::Target) -> Result<(), FolderError> {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_IDLIST | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: w!("open"),
            lpIDList: fresh.pidl.0,
            nShow: 1, // SW_SHOWNORMAL, matching the existing shell dispatch.
            ..Default::default()
        };
        // SAFETY: freshly revalidated absolute PIDL, explicit folder open verb,
        // no parameters/path/working directory. NOASYNC retains ownership until
        // shell acceptance returns. This does not prove Explorer became visible.
        unsafe { ShellExecuteExW(&mut info) }
            .map_err(|error| native_error(error.code().0, "Folder open"))
    }
}

fn canonical_identity(item: &IShellItem) -> Result<Vec<u16>, FolderError> {
    // SAFETY: same-worker live shell item; returned NUL-terminated shell string
    // is owned by the caller and documented as CoTaskMemFree-allocated.
    let name = unsafe { item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING) }
        .map_err(|error| native_error(error.code().0, "Folder canonical identity"))?;
    let owned_name = ShellAllocation(name.0.cast());
    if !owned_name.0.is_null() {
        // Windows paths/qualified names are bounded to 32K UTF-16 units here.
        // We stop at the API-guaranteed terminator; no native string reaches UI.
        for length in 0..32_768 {
            // SAFETY: the shell returned a valid NUL-terminated UTF-16 string.
            if unsafe { *name.0.add(length) } == 0 {
                if length > 0 {
                    // SAFETY: exactly the initialized units preceding that NUL.
                    return Ok(unsafe { std::slice::from_raw_parts(name.0, length) }.to_vec());
                }
                break;
            }
        }
    }
    Err(FolderError::new(
        FolderErrorKind::Unavailable,
        "The folder did not provide a bounded canonical identity.",
    ))
}
