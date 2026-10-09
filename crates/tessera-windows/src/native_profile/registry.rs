// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use tessera_system::profile::ProfileError;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, REG_SZ, REG_VALUE_TYPE,
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
};
use windows::core::PCWSTR;

use super::{code_error, wide};
use crate::profile::source::{
    PERSONAL_KEY, PHOTO_KEYS, Value, invalid, strict_utf16, validate_sid_string,
};

const REGISTRY_BYTE_LIMIT: u32 = 65_536;

struct Key(Option<HKEY>);

impl Key {
    fn close(mut self) -> Result<(), ProfileError> {
        let key = self.0.ok_or_else(invalid)?;
        // SAFETY: sole opened key; fixed predefined hive handles are never closed.
        let status = unsafe { RegCloseKey(key) };
        if status.0 != 0 {
            return Err(code_error(status.0 as i32));
        }
        self.0 = None;
        Ok(())
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        if let Some(key) = self.0.take() {
            // SAFETY: best-effort exceptional path, only our opened handle.
            let _ = unsafe { RegCloseKey(key) };
        }
    }
}

pub(super) fn read(source: Value<'_>) -> Result<Option<String>, ProfileError> {
    let (hive, subkey, value) = match source {
        Value::Photo { sid, quality } => {
            validate_sid_string(sid)?;
            if !PHOTO_KEYS.contains(&quality) {
                return Err(invalid());
            }
            (
                HKEY_LOCAL_MACHINE,
                format!(
                    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\AccountPicture\\Users\\{sid}"
                ),
                quality,
            )
        }
        Value::PersonalEmail => (HKEY_CURRENT_USER, PERSONAL_KEY.to_owned(), "UserEmail"),
        Value::PersonalFolder => (HKEY_CURRENT_USER, PERSONAL_KEY.to_owned(), "UserFolder"),
    };
    let subkey = wide(&subkey);
    let value = wide(value);
    let mut handle = HKEY::default();
    // SAFETY: fixed hive/key, only QUERY_VALUE, ordinary registry view. Never
    // enumeration, LastLoggedOnUserSID, other account, expansion or key creation.
    let status = unsafe {
        RegOpenKeyExW(
            hive,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_QUERY_VALUE,
            &mut handle,
        )
    };
    if matches!(status.0, 2 | 3) {
        return Ok(None);
    }
    if status.0 != 0 {
        return Err(code_error(status.0 as i32));
    }
    let key = Key(Some(handle));
    let result = query(handle, PCWSTR(value.as_ptr()));
    let cleanup = key.close();
    result.and_then(|value| cleanup.map(|()| value))
}

fn query(key: HKEY, value: PCWSTR) -> Result<Option<String>, ProfileError> {
    read_with_queries(&mut SdkQueries { key, value })
}

pub(super) trait Queries {
    fn size(&mut self) -> Result<Option<(u32, u32)>, ProfileError>;
    fn copy(&mut self, units: &mut [u16]) -> Result<(u32, u32), ProfileError>;
}

struct SdkQueries {
    key: HKEY,
    value: PCWSTR,
}

impl Queries for SdkQueries {
    fn size(&mut self) -> Result<Option<(u32, u32)>, ProfileError> {
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0;
        // SAFETY: live read-only key and fixed terminated value; null sizing query.
        let status = unsafe {
            RegQueryValueExW(
                self.key,
                self.value,
                None,
                Some(&mut kind),
                None,
                Some(&mut size),
            )
        };
        if matches!(status.0, 2 | 3) {
            return Ok(None);
        }
        if status.0 != 0 {
            return Err(code_error(status.0 as i32));
        }
        Ok(Some((kind.0, size)))
    }

    fn copy(&mut self, units: &mut [u16]) -> Result<(u32, u32), ProfileError> {
        let mut kind = REG_VALUE_TYPE::default();
        let mut copied = (units.len() * 2) as u32;
        // RegQueryValueEx is intentional: RegGetValue repairs missing terminators,
        // hiding malformed ingress. Validate genuine typed bytes, never expand.
        // SAFETY: initialized storage is exactly the bounded byte capacity.
        let status = unsafe {
            RegQueryValueExW(
                self.key,
                self.value,
                None,
                Some(&mut kind),
                Some(units.as_mut_ptr().cast()),
                Some(&mut copied),
            )
        };
        if status.0 != 0 {
            return Err(code_error(status.0 as i32));
        }
        Ok((kind.0, copied))
    }
}

pub(super) fn read_with_queries(calls: &mut impl Queries) -> Result<Option<String>, ProfileError> {
    let Some((kind, required)) = calls.size()? else {
        return Ok(None);
    };
    if kind != REG_SZ.0 || !(2..=REGISTRY_BYTE_LIMIT).contains(&required) || required % 2 != 0 {
        return Err(invalid());
    }
    let mut units = vec![0u16; required as usize / 2];
    let (copied_kind, copied) = calls.copy(&mut units)?;
    if copied != required || copied_kind != REG_SZ.0 {
        return Err(invalid());
    }
    // No growth retries: a changing source is not a coherent snapshot.
    if calls.size()? != Some((kind, required)) {
        return Err(invalid());
    }
    strict_utf16(&units).map(Some)
}
