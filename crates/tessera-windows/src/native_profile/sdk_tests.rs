// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! No process token, registry, file, account directory or Shell operation is
//! performed. SDK symbols are assigned only; fixtures are owned synthetic bytes.

use super::{identity, registry};
use crate::profile::source::invalid;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::mem::size_of;
use tessera_system::profile::{ProfileError, ProfileErrorKind};
use windows::Win32::Foundation::{HANDLE, WIN32_ERROR};
use windows::Win32::Security::Authentication::Identity::EXTENDED_NAME_FORMAT;
use windows::Win32::Security::{
    PSID, SID_AND_ATTRIBUTES, TOKEN_ACCESS_MASK, TOKEN_INFORMATION_CLASS, TOKEN_USER,
};
use windows::Win32::System::Registry::{HKEY, REG_VALUE_TYPE};
use windows::core::{PCWSTR, PWSTR};

type RegQueryValueSignature = unsafe fn(
    HKEY,
    PCWSTR,
    Option<*const u32>,
    Option<*mut REG_VALUE_TYPE>,
    Option<*mut u8>,
    Option<*mut u32>,
) -> WIN32_ERROR;

type ReadFileSignature = unsafe fn(
    HANDLE,
    Option<&mut [u8]>,
    Option<*mut u32>,
    Option<*mut windows::Win32::System::IO::OVERLAPPED>,
) -> windows::core::Result<()>;

#[test]
fn pinned_sdk_function_pointer_shapes_link_without_native_effects() {
    let _: unsafe fn(HANDLE, TOKEN_ACCESS_MASK, *mut HANDLE) -> windows::core::Result<()> =
        windows::Win32::System::Threading::OpenProcessToken;
    let _: unsafe fn(
        HANDLE,
        TOKEN_INFORMATION_CLASS,
        Option<*mut c_void>,
        u32,
        *mut u32,
    ) -> windows::core::Result<()> = windows::Win32::Security::GetTokenInformation;
    let _: unsafe fn(PSID, *mut PWSTR) -> windows::core::Result<()> =
        windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    let _: unsafe fn(EXTENDED_NAME_FORMAT, Option<PWSTR>, *mut u32) -> bool =
        windows::Win32::Security::Authentication::Identity::GetUserNameExW;
    let _: unsafe fn(Option<PWSTR>, *mut u32) -> windows::core::Result<()> =
        windows::Win32::System::WindowsProgramming::GetUserNameW;
    let _: RegQueryValueSignature = windows::Win32::System::Registry::RegQueryValueExW::<PCWSTR>;
    let _: unsafe fn(HKEY) -> WIN32_ERROR = windows::Win32::System::Registry::RegCloseKey;
    let _: ReadFileSignature = windows::Win32::Storage::FileSystem::ReadFile;
    let _: unsafe fn(
        *mut windows::Win32::UI::Shell::SHELLEXECUTEINFOW,
    ) -> windows::core::Result<()> = windows::Win32::UI::Shell::ShellExecuteExW;
    let _: unsafe fn(
        HANDLE,
        windows::Win32::Storage::FileSystem::FILE_INFO_BY_HANDLE_CLASS,
        *mut c_void,
        u32,
    ) -> windows::core::Result<()> =
        windows::Win32::Storage::FileSystem::GetFileInformationByHandleEx;
    let _: fn() -> windows::core::Error = windows::core::Error::from_thread;
    let _: unsafe fn(
        HANDLE,
        &mut [u16],
        windows::Win32::Storage::FileSystem::GETFINALPATHNAMEBYHANDLE_FLAGS,
    ) -> u32 = windows::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
}
#[test]
fn normalized_dos_flags_and_zero_last_error_preserve_truthful_failure() {
    use windows::Win32::Storage::FileSystem::{
        FILE_NAME_NORMALIZED, GETFINALPATHNAMEBYHANDLE_FLAGS, VOLUME_NAME_DOS,
    };
    let flags = GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0);
    assert_eq!(flags.0, 0); // The SDK documents both normalized and DOS as defaults.
    // Pure construction, not from_thread/GetLastError: no native effect occurs.
    let error = super::native_error(windows::core::Error::from_hresult(windows::core::HRESULT(
        0,
    )));
    assert_eq!(error.kind(), ProfileErrorKind::Other);
    assert_eq!(error.native_code(), Some(0));
}

fn synthetic_token(count: u8) -> Vec<usize> {
    let offset = size_of::<TOKEN_USER>();
    let length = offset + 8 + usize::from(count) * 4;
    let mut storage = vec![0usize; length.div_ceil(size_of::<usize>())];
    let pointer = storage.as_mut_ptr().cast::<u8>();
    // SAFETY: owned aligned zero-filled fixture; both headers are within bounds.
    unsafe {
        pointer.add(offset).write(1);
        pointer.add(offset + 1).write(count);
        storage.as_mut_ptr().cast::<TOKEN_USER>().write(TOKEN_USER {
            User: SID_AND_ATTRIBUTES {
                Sid: PSID(pointer.add(offset).cast()),
                Attributes: 0,
            },
        });
    }
    storage
}

#[test]
fn aligned_token_user_rejects_pointer_escape_truncation_revision_and_count_before_sid_api() {
    let offset = size_of::<TOKEN_USER>();
    let mut valid = synthetic_token(1);
    assert_eq!(
        (valid.as_ptr() as usize) % std::mem::align_of::<TOKEN_USER>(),
        0
    );
    assert!(identity::bounded_sid(&valid, offset + 12).is_ok());
    assert!(identity::bounded_sid(&valid, offset + 11).is_err());
    assert!(identity::bounded_sid(&valid, offset - 1).is_err());
    // SAFETY: changing only fixture-owned TOKEN_USER fields, no API invocation.
    unsafe {
        (*valid.as_mut_ptr().cast::<TOKEN_USER>()).User.Sid = PSID(std::ptr::null_mut());
    }
    assert!(identity::bounded_sid(&valid, offset + 12).is_err());
    let mut wrong_revision = synthetic_token(1);
    // SAFETY: in-bounds synthetic SID revision byte.
    unsafe {
        wrong_revision
            .as_mut_ptr()
            .cast::<u8>()
            .add(offset)
            .write(2);
    }
    assert!(identity::bounded_sid(&wrong_revision, offset + 12).is_err());
    let excessive = synthetic_token(16);
    assert!(identity::bounded_sid(&excessive, offset + 72).is_err());
    let mut unaligned = synthetic_token(1);
    let pointer = unaligned.as_mut_ptr().cast::<u8>();
    // SAFETY: only pointer value is malformed; the validator must reject before dereference.
    unsafe {
        (*unaligned.as_mut_ptr().cast::<TOKEN_USER>()).User.Sid =
            PSID(pointer.add(offset + 1).cast());
    }
    assert!(identity::bounded_sid(&unaligned, offset + 12).is_err());
}

struct Names {
    display: Result<Vec<u16>, ProfileError>,
    sam: Vec<u16>,
    calls: Vec<&'static str>,
}

impl identity::NameCalls for Names {
    fn display(&mut self, buffer: &mut [u16]) -> Result<usize, ProfileError> {
        self.calls.push("NameDisplay");
        let units = self.display.as_ref().map_err(Clone::clone)?;
        buffer[..units.len()].copy_from_slice(units);
        Ok(units.len() - 1)
    }
    fn sam(&mut self, buffer: &mut [u16]) -> Result<usize, ProfileError> {
        self.calls.push("GetUserNameW");
        buffer[..self.sam.len()].copy_from_slice(&self.sam);
        Ok(self.sam.len())
    }
}

#[test]
fn display_name_uses_genuine_local_sam_fallback_without_account_email_inference() {
    let sam: Vec<u16> = "local-user".encode_utf16().chain(Some(0)).collect();
    for display in [
        Err(ProfileError::new(
            ProfileErrorKind::NotFound,
            "The display format is unavailable.",
            Some(1332),
        )),
        Ok(vec![0xd800, 0]),
    ] {
        let mut calls = Names {
            display,
            sam: sam.clone(),
            calls: vec![],
        };
        assert_eq!(identity::name_with_calls(&mut calls).unwrap(), "local-user");
        assert_eq!(calls.calls, ["NameDisplay", "GetUserNameW"]);
    }
    let display: Vec<u16> = "Genuine 名字".encode_utf16().chain(Some(0)).collect();
    let mut calls = Names {
        display: Ok(display),
        sam: vec![0xd800, 0],
        calls: vec![],
    };
    assert_eq!(
        identity::name_with_calls(&mut calls).unwrap(),
        "Genuine 名字"
    );
    assert_eq!(calls.calls, ["NameDisplay"]);
    calls.display = Err(invalid());
    assert!(identity::name_with_calls(&mut calls).is_err());
}

struct Queries {
    sizes: VecDeque<Option<(u32, u32)>>,
    payload: Vec<u16>,
    copy_result: Result<(u32, u32), ProfileError>,
    copies: usize,
}

impl registry::Queries for Queries {
    fn size(&mut self) -> Result<Option<(u32, u32)>, ProfileError> {
        Ok(self
            .sizes
            .pop_front()
            .expect("bounded recording sizing call"))
    }
    fn copy(&mut self, units: &mut [u16]) -> Result<(u32, u32), ProfileError> {
        self.copies += 1;
        let length = units.len().min(self.payload.len());
        units[..length].copy_from_slice(&self.payload[..length]);
        self.copy_result.clone()
    }
}

#[test]
fn registry_recordings_reject_type_utf16_growth_disappearance_and_over_budget() {
    let payload = vec![b'a' as u16, 0];
    let mut valid = Queries {
        sizes: VecDeque::from([Some((1, 4)), Some((1, 4))]),
        payload: payload.clone(),
        copy_result: Ok((1, 4)),
        copies: 0,
    };
    assert_eq!(
        registry::read_with_queries(&mut valid).unwrap(),
        Some("a".into())
    );
    for (final_size, copied, units) in [
        (Some((1, 6)), (1, 4), payload.clone()),
        (None, (1, 4), payload.clone()),
        (Some((2, 4)), (1, 4), payload.clone()),
        (Some((1, 4)), (2, 4), payload.clone()),
        (Some((1, 4)), (1, 2), payload.clone()),
        (Some((1, 4)), (1, 4), vec![0xd800, 0]),
        (Some((1, 4)), (1, 4), vec![b'a' as u16, b'b' as u16]),
    ] {
        let mut calls = Queries {
            sizes: VecDeque::from([Some((1, 4)), final_size]),
            payload: units,
            copy_result: Ok(copied),
            copies: 0,
        };
        assert!(registry::read_with_queries(&mut calls).is_err());
    }
    for initial in [None, Some((2, 4)), Some((1, 3)), Some((1, 65_538))] {
        let mut calls = Queries {
            sizes: VecDeque::from([initial]),
            payload: payload.clone(),
            copy_result: Ok((1, 4)),
            copies: 0,
        };
        assert_eq!(
            registry::read_with_queries(&mut calls).is_ok(),
            initial.is_none()
        );
        assert_eq!(calls.copies, 0);
    }
    let mut denied = Queries {
        sizes: VecDeque::from([Some((1, 4))]),
        payload,
        copy_result: Err(ProfileError::new(
            ProfileErrorKind::AccessDenied,
            "The source is unavailable.",
            Some(5),
        )),
        copies: 0,
    };
    assert_eq!(
        registry::read_with_queries(&mut denied).unwrap_err().kind(),
        ProfileErrorKind::AccessDenied
    );
}
