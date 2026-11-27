// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The keys of the registry the programs that start with the session are
//! kept under: opened, read value by value, written and closed again.

use anyhow::{Result, anyhow};
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
    WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    HKEY, KEY_READ, KEY_SET_VALUE, REG_BINARY, REG_CREATE_KEY_DISPOSITION, REG_CREATED_NEW_KEY,
    REG_DWORD, REG_EXPAND_SZ, REG_OPEN_CREATE_OPTIONS, REG_OPTION_VOLATILE, REG_SAM_FLAGS, REG_SZ,
    REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegEnumKeyExW, RegEnumValueW,
    RegOpenKeyExW, RegQueryInfoKeyW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::{PCWSTR, PWSTR};

/// `text` as a NUL-terminated wide string.
pub(super) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// What a value of the registry holds, as far as the startup reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Value {
    /// `REG_SZ` or `REG_EXPAND_SZ`, as it is written.
    Text(String),
    /// `REG_DWORD`.
    Number(u32),
    /// `REG_BINARY` and every other kind.
    Bytes(Vec<u8>),
}

impl Value {
    fn of(kind: REG_VALUE_TYPE, data: &[u8]) -> Self {
        match kind {
            REG_SZ | REG_EXPAND_SZ => Self::Text(text_of(data)),
            REG_DWORD if data.len() >= 4 => {
                Self::Number(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
            }
            _ => Self::Bytes(data.to_vec()),
        }
    }

    pub(super) fn text(self) -> Option<String> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }
}

/// The text in the bytes of a string value, up to its first NUL.
fn text_of(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Why a key could not be used.
#[derive(Debug)]
pub(super) enum Failure {
    /// The user may not open it as asked, as a medium process may not
    /// write a key of the machine.
    Denied,
    Other(anyhow::Error),
}

impl Failure {
    fn of(status: WIN32_ERROR, what: &str) -> Self {
        match status {
            ERROR_ACCESS_DENIED => Self::Denied,
            _ => Self::Other(anyhow!("{what}: {}", windows::core::Error::from(status))),
        }
    }

    pub(super) fn into_error(self, what: &str) -> anyhow::Error {
        match self {
            Self::Denied => anyhow!("{what}: access is denied"),
            Self::Other(error) => error,
        }
    }
}

/// An open key, closed when dropped.
pub(super) struct Key(HKEY);

// SAFETY: a key handle may be used and closed from any thread.
unsafe impl Send for Key {}

impl Key {
    /// Opens `path` under `root` with `access`; `None` if it does not
    /// exist.
    pub(super) fn open(
        root: HKEY,
        path: &str,
        access: REG_SAM_FLAGS,
    ) -> Result<Option<Self>, Failure> {
        let path = wide(path);
        let mut key = HKEY::default();
        // SAFETY: the path is NUL-terminated and `key` outlives the call.
        let status =
            unsafe { RegOpenKeyExW(root, PCWSTR(path.as_ptr()), None, access, &raw mut key) };
        match status {
            ERROR_SUCCESS => Ok(Some(Self(key))),
            ERROR_FILE_NOT_FOUND => Ok(None),
            status => Err(Failure::of(status, "a key of the registry did not open")),
        }
    }

    /// Opens `path` under `root` for reading; `None` where it does not
    /// exist or may not be read.
    pub(super) fn read(root: HKEY, path: &str, view: REG_SAM_FLAGS) -> Option<Self> {
        Self::open(root, path, KEY_READ | view).ok().flatten()
    }

    /// Opens `path` under `root` to read and write it, making it and the
    /// keys above it where they are missing; a volatile key is gone when
    /// the user signs out. Also tells whether the key was made now.
    pub(super) fn create(
        root: HKEY,
        path: &str,
        view: REG_SAM_FLAGS,
        volatile: bool,
    ) -> Result<(Self, bool), Failure> {
        let path = wide(path);
        let options = if volatile {
            REG_OPTION_VOLATILE
        } else {
            REG_OPEN_CREATE_OPTIONS(0)
        };
        let mut key = HKEY::default();
        let mut disposition = REG_CREATE_KEY_DISPOSITION::default();
        // SAFETY: the path is NUL-terminated, `key` and `disposition`
        // outlive the call, and no class or security is given.
        let status = unsafe {
            RegCreateKeyExW(
                root,
                PCWSTR(path.as_ptr()),
                None,
                PCWSTR::null(),
                options,
                KEY_READ | KEY_SET_VALUE | view,
                None,
                &raw mut key,
                Some(&raw mut disposition),
            )
        };
        match status {
            ERROR_SUCCESS => Ok((Self(key), disposition == REG_CREATED_NEW_KEY)),
            status => Err(Failure::of(status, "a key of the registry was not made")),
        }
    }

    /// The names of the keys under this one, in the order of the registry.
    pub(super) fn subkeys(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut buffer = vec![0u16; 256];
        for index in 0.. {
            let mut length = buffer.len() as u32;
            // SAFETY: `length` is the length of `buffer` in characters,
            // both outlive the call; nothing else is asked.
            let status = unsafe {
                RegEnumKeyExW(
                    self.0,
                    index,
                    Some(PWSTR(buffer.as_mut_ptr())),
                    &raw mut length,
                    None,
                    None,
                    None,
                    None,
                )
            };
            match status {
                ERROR_SUCCESS => {
                    names.push(String::from_utf16_lossy(&buffer[..length as usize]));
                }
                // The name of a key is at most 255 characters.
                ERROR_MORE_DATA => continue,
                _ => break,
            }
        }
        names
    }

    /// Every value of the key, by name, in the order of the registry.
    pub(super) fn values(&self) -> Vec<(String, Value)> {
        let (mut longest_name, mut longest_data) = (0u32, 0u32);
        // SAFETY: the two lengths outlive the call; nothing else is asked.
        let status = unsafe {
            RegQueryInfoKeyW(
                self.0,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(&raw mut longest_name),
                Some(&raw mut longest_data),
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut name = vec![0u16; longest_name as usize + 1];
        let mut data = vec![0u8; longest_data as usize + 2];
        let mut values = Vec::new();
        let mut index = 0;
        loop {
            let mut name_length = name.len() as u32;
            let mut data_length = data.len() as u32;
            let mut kind = 0u32;
            // SAFETY: the lengths are those of the buffers, in characters
            // and in bytes; all of them outlive the call.
            let status = unsafe {
                RegEnumValueW(
                    self.0,
                    index,
                    Some(PWSTR(name.as_mut_ptr())),
                    &raw mut name_length,
                    None,
                    Some(&raw mut kind),
                    Some(data.as_mut_ptr()),
                    Some(&raw mut data_length),
                )
            };
            match status {
                ERROR_SUCCESS => {
                    let text = String::from_utf16_lossy(&name[..name_length as usize]);
                    let value = Value::of(REG_VALUE_TYPE(kind), &data[..data_length as usize]);
                    values.push((text, value));
                    index += 1;
                }
                // A value was made or grew since the key was asked: the
                // buffers grow and the same value is asked again.
                ERROR_MORE_DATA if data.len() < 1 << 20 => {
                    name.resize(name.len().max(16_384), 0);
                    data.resize(data.len() * 2 + 64, 0);
                }
                ERROR_NO_MORE_ITEMS => break,
                _ => break,
            }
        }
        values
    }

    /// The value `name`, where there is one.
    pub(super) fn value(&self, name: &str) -> Option<Value> {
        let name = wide(name);
        let mut length = 0u32;
        let mut kind = REG_VALUE_TYPE::default();
        // SAFETY: the name is NUL-terminated; without a buffer the call
        // only tells the kind and the size of the data.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&raw mut kind),
                None,
                Some(&raw mut length),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let mut data = vec![0u8; length as usize + 2];
        let mut length = data.len() as u32;
        // SAFETY: as above, with a buffer whose size is `length`.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&raw mut kind),
                Some(data.as_mut_ptr()),
                Some(&raw mut length),
            )
        };
        (status == ERROR_SUCCESS).then(|| Value::of(kind, &data[..length as usize]))
    }

    /// The text value `name`.
    pub(super) fn text(&self, name: &str) -> Option<String> {
        self.value(name)?.text()
    }

    /// The number value `name`.
    pub(super) fn number(&self, name: &str) -> Option<u32> {
        match self.value(name)? {
            Value::Number(number) => Some(number),
            _ => None,
        }
    }

    /// The bytes of the value `name`, of whatever kind it is.
    pub(super) fn bytes(&self, name: &str) -> Option<Vec<u8>> {
        match self.value(name)? {
            Value::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    fn set(&self, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<(), Failure> {
        let name = wide(name);
        // SAFETY: the name is NUL-terminated and the data outlives the
        // call, which copies it.
        let status =
            unsafe { RegSetValueExW(self.0, PCWSTR(name.as_ptr()), None, kind, Some(data)) };
        match status {
            ERROR_SUCCESS => Ok(()),
            status => Err(Failure::of(
                status,
                "a value of the registry was not written",
            )),
        }
    }

    pub(super) fn set_text(&self, name: &str, text: &str) -> Result<(), Failure> {
        let data: Vec<u8> = wide(text)
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        self.set(name, REG_SZ, &data)
    }

    pub(super) fn set_number(&self, name: &str, number: u32) -> Result<(), Failure> {
        self.set(name, REG_DWORD, &number.to_le_bytes())
    }

    pub(super) fn set_bytes(&self, name: &str, bytes: &[u8]) -> Result<(), Failure> {
        self.set(name, REG_BINARY, bytes)
    }

    /// Removes the value `name`; one that is not there is no failure.
    pub(super) fn delete(&self, name: &str) -> Result<(), Failure> {
        let name = wide(name);
        // SAFETY: the name is NUL-terminated.
        let status = unsafe { RegDeleteValueW(self.0, PCWSTR(name.as_ptr())) };
        match status {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            status => Err(Failure::of(
                status,
                "a value of the registry was not removed",
            )),
        }
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened by this value and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_read_by_its_kind() {
        let text: Vec<u8> = wide("C:\\a.exe")
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        assert_eq!(
            Value::of(REG_SZ, &text),
            Value::Text("C:\\a.exe".to_owned())
        );
        // A string written without its NUL is read whole.
        assert_eq!(
            Value::of(REG_EXPAND_SZ, &text[..text.len() - 2]),
            Value::Text("C:\\a.exe".to_owned())
        );
        assert_eq!(Value::of(REG_DWORD, &[2, 0, 0, 0]), Value::Number(2));
        assert_eq!(Value::of(REG_BINARY, &[3, 0]), Value::Bytes(vec![3, 0]));
    }
}
