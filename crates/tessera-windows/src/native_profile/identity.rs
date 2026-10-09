// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::mem::size_of;
use tessera_system::profile::ProfileError;
use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameDisplay};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    GetLengthSid, GetTokenInformation, IsValidSid, PSID, RevertToSelf, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::System::WindowsProgramming::GetUserNameW;
use windows::core::PWSTR;

use super::handle::OwnedHandle;
use super::{code_error, native_error};
use crate::profile::source::{invalid, strict_utf16, validate_sid_string};

const TOKEN_LIMIT: usize = 65_536;
const NAME_UNITS: usize = 1_025;

struct SidString(PWSTR);

impl SidString {
    fn close(mut self) -> Result<(), ProfileError> {
        // SAFETY: ConvertSidToStringSidW allocates with LocalAlloc; unique owner.
        let remaining = unsafe { LocalFree(Some(HLOCAL(self.0.0.cast()))) };
        if remaining.0.is_null() {
            self.0 = PWSTR::null();
            Ok(())
        } else {
            self.0 = PWSTR(remaining.0.cast());
            Err(invalid())
        }
    }
}

impl Drop for SidString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: only exceptional-path owned LocalAlloc storage is released.
            let _ = unsafe { LocalFree(Some(HLOCAL(self.0.0.cast()))) };
        }
    }
}

pub(super) fn self_context() -> Result<(), ProfileError> {
    // SAFETY: only our dedicated owner; callbacks cannot leave it impersonating
    // another account for subsequent requests. Failure performs no data read.
    unsafe { RevertToSelf() }.map_err(native_error)
}

pub(super) fn current_sid() -> Result<String, ProfileError> {
    self_context()?;
    let mut raw = HANDLE::default();
    // SAFETY: current-process pseudo handle is borrowed; output receives an owned
    // TOKEN_QUERY handle only. No thread/session/LastLoggedOn token is queried.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) }
        .map_err(native_error)?;
    let token = OwnedHandle::new(raw)?;
    let result = token_sid(raw);
    let cleanup = token.close();
    result.and_then(|value| cleanup.map(|()| value))
}

fn token_sid(token: HANDLE) -> Result<String, ProfileError> {
    let mut required = 0u32;
    // SAFETY: documented null sizing query, no information storage supplied.
    let sizing = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut required) };
    if let Err(error) = sizing
        && error.code().0 as u32 & 0xffff != 122
    {
        return Err(native_error(error));
    }
    let capacity = usize::try_from(required).map_err(|_| invalid())?;
    if !(size_of::<TOKEN_USER>()..=TOKEN_LIMIT).contains(&capacity) {
        return Err(invalid());
    }
    // usize storage is naturally aligned for TOKEN_USER and SID pointers on both
    // Windows architectures. Byte vectors would not provide this guarantee.
    let mut storage = vec![0usize; capacity.div_ceil(size_of::<usize>())];
    let mut returned = required;
    // SAFETY: aligned, initialized allocation contains at least required bytes.
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(storage.as_mut_ptr().cast()),
            required,
            &mut returned,
        )
    }
    .map_err(native_error)?;
    let returned = usize::try_from(returned).map_err(|_| invalid())?;
    if returned > capacity {
        return Err(invalid());
    }
    let sid = bounded_sid(&storage, returned)?;
    // SAFETY: bounded_sid proved header/count/full SID bytes inside this owned
    // TokenUser result before asking APIs which inspect the SID memory.
    if !unsafe { IsValidSid(sid) }.as_bool() || unsafe { GetLengthSid(sid) } as usize > returned {
        return Err(invalid());
    }
    let mut text = PWSTR::null();
    // SAFETY: validated SID remains alive in storage through conversion.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(native_error)?;
    let owned = SidString(text);
    let result = read_sid_string(text).and_then(|value| {
        validate_sid_string(&value)?;
        Ok(value)
    });
    let cleanup = owned.close();
    result.and_then(|value| cleanup.map(|()| value))
}

pub(super) fn bounded_sid(storage: &[usize], returned: usize) -> Result<PSID, ProfileError> {
    let available = storage
        .len()
        .checked_mul(size_of::<usize>())
        .ok_or_else(invalid)?;
    if returned < size_of::<TOKEN_USER>() || returned > available {
        return Err(invalid());
    }
    // SAFETY: storage is TOKEN_USER aligned and fully initialized, length checked.
    let user = unsafe { storage.as_ptr().cast::<TOKEN_USER>().read() };
    let base = storage.as_ptr() as usize;
    let address = user.User.Sid.0 as usize;
    let offset = address.checked_sub(base).ok_or_else(invalid)?;
    if offset < size_of::<TOKEN_USER>()
        || offset % 4 != 0
        || offset.checked_add(8).is_none_or(|end| end > returned)
    {
        return Err(invalid());
    }
    // SAFETY: initialized byte view of integer storage; no uninitialized padding.
    let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), returned) };
    let count = bytes[offset + 1] as usize;
    if bytes[offset] != 1
        || count > 15
        || offset
            .checked_add(8 + 4 * count)
            .is_none_or(|end| end > returned)
    {
        return Err(invalid());
    }
    Ok(user.User.Sid)
}

fn read_sid_string(text: PWSTR) -> Result<String, ProfileError> {
    if text.is_null() {
        return Err(invalid());
    }
    for length in 0..=184 {
        // SAFETY: SDK-guaranteed terminated SID string; maximum bounded scan.
        if unsafe { *text.0.add(length) } == 0 {
            // SAFETY: exactly the initialized string and its guaranteed NUL.
            return strict_utf16(unsafe { std::slice::from_raw_parts(text.0, length + 1) });
        }
    }
    Err(invalid())
}

pub(super) trait NameCalls {
    fn display(&mut self, buffer: &mut [u16]) -> Result<usize, ProfileError>;
    fn sam(&mut self, buffer: &mut [u16]) -> Result<usize, ProfileError>;
}

struct SdkNameCalls;

impl NameCalls for SdkNameCalls {
    fn display(&mut self, buffer: &mut [u16]) -> Result<usize, ProfileError> {
        let mut count = buffer.len() as u32;
        // SAFETY: bounded initialized output; successful count excludes its NUL.
        if unsafe { GetUserNameExW(NameDisplay, Some(PWSTR(buffer.as_mut_ptr())), &mut count) } {
            Ok(count as usize)
        } else {
            // Capture GetLastError immediately, before any other native call.
            // Even a zero last-error remains an Err, mapped to generic Other.
            let error = windows::core::Error::from_thread();
            Err(native_error(error))
        }
    }

    fn sam(&mut self, buffer: &mut [u16]) -> Result<usize, ProfileError> {
        let mut count = buffer.len() as u32;
        // SAFETY: same bounded output; GetUserNameW count includes its NUL.
        unsafe { GetUserNameW(Some(PWSTR(buffer.as_mut_ptr())), &mut count) }
            .map_err(native_error)?;
        Ok(count as usize)
    }
}

pub(super) fn display_name() -> Result<String, ProfileError> {
    self_context()?;
    name_with_calls(&mut SdkNameCalls)
}

pub(super) fn name_with_calls(calls: &mut impl NameCalls) -> Result<String, ProfileError> {
    let mut buffer = vec![0u16; NAME_UNITS];
    if let Ok(count) = calls.display(&mut buffer)
        && count < buffer.len()
        && let Ok(name) = strict_utf16(&buffer[..=count])
        && name.len() <= 1_024
    {
        return Ok(name);
    }
    buffer.fill(0);
    let count = calls.sam(&mut buffer)?;
    if count == 0 || count > buffer.len() {
        return Err(code_error(13));
    }
    let name = strict_utf16(&buffer[..count])?;
    if name.len() > 1_024 {
        return Err(invalid());
    }
    Ok(name)
}
