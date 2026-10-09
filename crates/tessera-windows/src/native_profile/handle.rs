// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::native_error;
use crate::profile::source::invalid;
use tessera_system::profile::ProfileError;
use windows::Win32::Foundation::{CloseHandle, HANDLE};

/// A token or disk handle retained on the profile owner, never borrowed into UI.
pub(super) struct OwnedHandle(Option<HANDLE>);

impl OwnedHandle {
    pub(super) fn new(raw: HANDLE) -> Result<Self, ProfileError> {
        if raw.is_invalid() {
            return Err(invalid());
        }
        Ok(Self(Some(raw)))
    }

    pub(super) fn raw(&self) -> Result<HANDLE, ProfileError> {
        self.0.ok_or_else(invalid)
    }

    pub(super) fn close(mut self) -> Result<(), ProfileError> {
        let raw = self.raw()?;
        // SAFETY: unique valid owner. On failure retain it for best-effort Drop;
        // only a successful close retires the capability, preventing double-close.
        unsafe { CloseHandle(raw) }.map_err(native_error)?;
        self.0 = None;
        Ok(())
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if let Some(raw) = self.0.take() {
            // SAFETY: remaining uniquely owned handle, same-thread retirement.
            let _ = unsafe { CloseHandle(raw) };
        }
    }
}
