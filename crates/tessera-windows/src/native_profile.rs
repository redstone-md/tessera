// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pinned Windows 0.62.2 calls; created only after explicit actor acceptance.

use std::marker::PhantomData;
use std::rc::Rc;

use tessera_system::profile::{ProfileError, ProfileErrorKind, ProfilePhoto};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::profile::source::{Backend, DirectoryIdentity, Reader, Value};

mod files;
mod handle;
mod identity;
mod navigation;
mod registry;

#[cfg(test)]
mod sdk_tests;

pub(crate) fn driver() -> Result<Reader<NativeBackend>, ProfileError> {
    Ok(Reader::new(NativeBackend {
        _apartment: Apartment::enter()?,
    }))
}

struct Apartment(PhantomData<Rc<()>>);

impl Apartment {
    fn enter() -> Result<Self, ProfileError> {
        // SAFETY: new dedicated owner thread, no reserved argument; never UI COM.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(native_error)?;
        Ok(Self(PhantomData))
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: exactly balances a successful same-thread initialization.
        unsafe { CoUninitialize() };
    }
}

pub(crate) struct NativeBackend {
    _apartment: Apartment,
}

impl Backend for NativeBackend {
    fn current_sid(&mut self) -> Result<String, ProfileError> {
        identity::current_sid()
    }

    fn display_name(&mut self) -> Result<String, ProfileError> {
        identity::display_name()
    }

    fn value(&mut self, source: Value<'_>) -> Result<Option<String>, ProfileError> {
        registry::read(source)
    }

    fn photo(&mut self, local_path: &str) -> Result<ProfilePhoto, ProfileError> {
        let bytes = files::photo_bytes(local_path)?;
        let decoded = crate::image_decode::decode_bytes(
            &bytes,
            crate::image_decode::ImageDecodeLimits::profile_photo(),
        )
        .map_err(|error| {
            ProfileError::new(
                ProfileErrorKind::InvalidData,
                "The profile photo could not be decoded.",
                error.hresult(),
            )
        })?;
        ProfilePhoto::new(decoded.width(), decoded.height(), decoded.into_rgba())
    }

    fn directory(&mut self, local_path: &str) -> Result<DirectoryIdentity, ProfileError> {
        let guard = files::LocalPath::directory(local_path)?;
        let identity = guard.identity();
        guard.close()?;
        Ok(identity)
    }

    fn open_directory(
        &mut self,
        local_path: &str,
        identity: DirectoryIdentity,
    ) -> Result<(), ProfileError> {
        let guard = files::LocalPath::directory(local_path)?;
        if guard.identity() != identity {
            return Err(crate::profile::source::invalid());
        }
        let result = navigation::directory(local_path);
        let cleanup = guard.close();
        result.and(cleanup)
    }

    fn open_home(&mut self) -> Result<(), ProfileError> {
        navigation::home()
    }

    fn open_accounts(&mut self) -> Result<(), ProfileError> {
        navigation::accounts()
    }
}

fn native_error(error: windows::core::Error) -> ProfileError {
    code_error(error.code().0)
}

fn code_error(code: i32) -> ProfileError {
    let low = (code as u32) & 0xffff;
    let kind = match low {
        5 => ProfileErrorKind::AccessDenied,
        2 | 3 => ProfileErrorKind::NotFound,
        32 | 33 | 170 => ProfileErrorKind::Busy,
        13 | 87 | 122 | 234 => ProfileErrorKind::InvalidData,
        _ => ProfileErrorKind::Other,
    };
    ProfileError::new(
        kind,
        "The native profile request could not be completed.",
        Some(code),
    )
}

pub(super) fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
