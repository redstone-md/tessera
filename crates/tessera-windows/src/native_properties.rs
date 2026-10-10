// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Bounded Shell property reads shared by window identity and endpoint labels.

use std::marker::PhantomData;
use std::rc::Rc;

use windows::Win32::Foundation::{ERROR_INVALID_DATA, PROPERTYKEY, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Variant::{VT_BSTR, VT_LPWSTR};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::core::{BSTR, Error, HRESULT, Result};

/// Optional request-local initialization; an existing apartment is never changed.
/// Successful S_OK/S_FALSE entries release exactly their own initialization count.
pub(crate) struct PropertyApartment {
    initialized: bool,
    _thread: PhantomData<Rc<()>>,
}

impl PropertyApartment {
    pub(crate) fn acquire() -> Option<Self> {
        // SAFETY: synchronous read owner; no COM references escape this request.
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result.is_ok() || result == RPC_E_CHANGED_MODE {
            Some(Self {
                initialized: result.is_ok(),
                _thread: PhantomData,
            })
        } else {
            None
        }
    }
}

impl Drop for PropertyApartment {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: this thread's successful, balanced initialization only.
            unsafe { CoUninitialize() };
        }
    }
}

/// Read one SDK string property, with RAII on every conversion/validation exit.
/// The caller owns an initialized apartment and retains no native property value.
pub(crate) fn string_property(
    store: &IPropertyStore,
    key: &PROPERTYKEY,
    max_utf16_units: usize,
) -> Result<String> {
    // SAFETY: live store and immutable key for a synchronous SDK call.
    let value = unsafe { store.GetValue(key) }?;
    if !matches!(value.vt(), VT_LPWSTR | VT_BSTR) {
        return Err(invalid_property());
    }
    // Both native allocations are owned by the pinned SDK, including errors.
    let text = BSTR::try_from(&value)?;
    if text.len() > max_utf16_units || text.contains(&0) {
        return Err(invalid_property());
    }
    String::try_from(&text).map_err(|_| invalid_property())
}

fn invalid_property() -> Error {
    Error::from_hresult(HRESULT::from_win32(ERROR_INVALID_DATA.0))
}
