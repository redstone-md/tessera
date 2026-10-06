// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native deployment I/O for the shared installer/native contract.
//!
//! Everything here is explicitly handed the subkey path and value names from
//! [`crate::shell_recovery`]. There is no implicit global state: tests open
//! their own private subtree through an explicitly passed subkey and never
//! touch real Winlogon values.
//!
//! Registry handles are held in RAII wrappers and released deterministically;
//! no FFI callback can observe an unwind across a Win32 boundary. Every value
//! this module writes is read back and required to be byte-identical before
//! the operation reports success; a missing key reads as `Ok(None)`, never as
//! an error.

use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ, REG_SZ, RegCloseKey,
    RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
#[cfg(test)]
use windows_sys::Win32::System::Registry::{REG_OPTION_NON_VOLATILE, RegCreateKeyExW};
use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

use crate::shell_recovery::{
    BackupRejection, MAX_VALUE_CHARS, OriginalShellState, RestoreDecision, ShellRecoveryBackup,
    decode_utf16_bytes,
};

/// Why a deployment operation failed. Fatal operations never return partial
/// state; the caller either has a complete backup record or nothing.
#[derive(Debug)]
#[non_exhaustive]
pub enum DeploymentError {
    /// The current OS is not supported.
    UnsupportedPlatform,
    /// A Win32 operation failed; `operation` names the failed call and
    /// `code` carries the last error.
    Windows { operation: &'static str, code: u32 },
    /// A value was found but rejected by the pure policy layer.
    Rejected(BackupRejection),
    /// The live `Shell` value is not a plain `REG_SZ`.
    ShellValueNotString { kind: u32 },
    /// The live `Shell` value exists but is empty or unrepresentable.
    ShellValueUnreadable,
    /// The live shell changed after the caller's recovery decision.
    ShellOwnershipChanged,
    /// The deployment lock could not be acquired (contended or failed).
    LockBusy,
    /// A path from the environment or a known folder is not usable.
    UnusablePath { context: &'static str },
    /// A decoded value is not valid UTF-16 or is out of bounds.
    InvalidUtf16 { context: &'static str },
    /// A value this module wrote did not read back with the exact type and
    /// payload it was given. The operation never silently claims success.
    RestoreVerificationFailed { value: &'static str },
}

impl std::fmt::Display for DeploymentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "deployment requires Windows"),
            Self::Windows { operation, code } => {
                write!(
                    f,
                    "Windows operation `{operation}` failed (code 0x{code:X})"
                )
            }
            Self::Rejected(reason) => write!(f, "backup rejected: {reason}"),
            Self::ShellValueNotString { kind } => {
                write!(f, "Shell value has unsupported type {kind}")
            }
            Self::ShellValueUnreadable => write!(f, "Shell value is empty or unreadable"),
            Self::ShellOwnershipChanged => write!(
                f,
                "Shell changed; refusing to overwrite another owner's value"
            ),
            Self::LockBusy => write!(f, "deployment lock is held by another operation"),
            Self::UnusablePath { context } => write!(f, "path unusable: {context}"),
            Self::InvalidUtf16 { context } => write!(f, "value at `{context}` is not valid UTF-16"),
            Self::RestoreVerificationFailed { value } => {
                write!(f, "value `{value}` did not read back identical after write")
            }
        }
    }
}

impl std::error::Error for DeploymentError {}

impl From<BackupRejection> for DeploymentError {
    fn from(reason: BackupRejection) -> Self {
        Self::Rejected(reason)
    }
}

/// Maps a registry value name onto its fixed `&'static str` entry. The
/// vocabulary is exactly the constants in [`crate::shell_recovery`], so this
/// is a pure function with no allocation, locking, or lifetime tricks.
fn to_static_field(name: &str) -> &'static str {
    for field in [
        crate::shell_recovery::FIELD_SCHEMA_VERSION,
        crate::shell_recovery::FIELD_ACTIVE,
        crate::shell_recovery::FIELD_SHELL_COMMAND,
        crate::shell_recovery::FIELD_ORIGINAL_PRESENT,
        crate::shell_recovery::FIELD_ORIGINAL_KIND,
        crate::shell_recovery::FIELD_ORIGINAL_VALUE,
        crate::shell_recovery::FIELD_INSTALL_DIRECTORY,
    ] {
        if name == field {
            return field;
        }
    }
    "unknown field"
}

/// A live HKEY that is closed deterministically on drop.
pub(crate) struct OwnedKey(pub(crate) HKEY);

impl OwnedKey {
    /// Opens `subkey` under `parent` with exactly `rights`. A missing key is
    /// `Ok(None)`, so callers can treat absent state as "nothing recorded"
    /// instead of an error.
    fn open(parent: HKEY, subkey: &str, rights: u32) -> Result<Option<Self>, DeploymentError> {
        let mut key: HKEY = std::ptr::null_mut();
        let wide = to_wide(subkey);
        // SAFETY: `wide` is NUL-terminated UTF-16, `key` is a valid out-param.
        let status = unsafe { RegOpenKeyExW(parent, wide.as_ptr(), 0, rights, &mut key) };
        match status {
            ERROR_SUCCESS => Ok(Some(Self(key))),
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
            code => Err(DeploymentError::Windows {
                operation: "RegOpenKeyExW",
                code,
            }),
        }
    }

    /// Creates (or opens) `subkey` under `parent` with read/write access.
    /// Production recovery code only ever opens existing keys; key creation
    /// exists for the isolated test fixtures.
    #[cfg(test)]
    fn create(parent: HKEY, subkey: &str) -> Result<Self, DeploymentError> {
        let mut key: HKEY = std::ptr::null_mut();
        let wide = to_wide(subkey);
        // SAFETY: `wide` is NUL-terminated UTF-16; out-params are valid.
        let status = unsafe {
            RegCreateKeyExW(
                parent,
                wide.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(DeploymentError::Windows {
                operation: "RegCreateKeyExW",
                code: status,
            });
        }
        Ok(Self(key))
    }
}

impl Drop for OwnedKey {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` was produced by RegOpenKeyExW/RegCreateKeyExW
            // and is closed exactly once, here.
            unsafe { RegCloseKey(self.0) };
        }
    }
}

/// Converts a Rust string to NUL-terminated UTF-16 for a single call.
pub(crate) fn to_wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Raw value read with a bounded buffer: type code and exact payload bytes.
fn query_raw(key: HKEY, value_name: &str) -> Result<Option<(u32, Vec<u8>)>, DeploymentError> {
    let mut wide = to_wide(value_name);
    let mut kind: u32 = 0;
    let mut size: u32 = (MAX_VALUE_CHARS * 2) as u32;
    let mut buffer: Vec<u8> = vec![0; size as usize];
    // SAFETY: buffer and size agree; RegQueryValueExW writes at most `size`.
    let status = unsafe {
        RegQueryValueExW(
            key,
            wide.as_mut_ptr(),
            std::ptr::null(),
            &mut kind,
            buffer.as_mut_ptr(),
            &mut size,
        )
    };
    match status {
        ERROR_SUCCESS => {
            buffer.truncate(size as usize);
            Ok(Some((kind, buffer)))
        }
        // The documented missing-value code. `ERROR_MORE_DATA` means the value
        // exceeded the bounded buffer: a size violation, reported through the
        // out-of-bounds arm of `InvalidUtf16` by the callers.
        ERROR_FILE_NOT_FOUND => Ok(None),
        windows_sys::Win32::Foundation::ERROR_MORE_DATA => Err(DeploymentError::InvalidUtf16 {
            context: to_static_field(value_name),
        }),
        code => Err(DeploymentError::Windows {
            operation: "RegQueryValueExW",
            code,
        }),
    }
}

/// Writes a raw value with an explicit type.
pub(crate) fn set_raw(
    key: HKEY,
    value_name: &str,
    kind: u32,
    payload: &[u8],
) -> Result<(), DeploymentError> {
    let wide = to_wide(value_name);
    // SAFETY: payload length matches cbData; empty payload passes null.
    let status = unsafe {
        RegSetValueExW(
            key,
            wide.as_ptr(),
            0,
            kind,
            payload.as_ptr(),
            payload.len() as u32,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(DeploymentError::Windows {
            operation: "RegSetValueExW",
            code: status,
        });
    }
    Ok(())
}

fn delete_value(key: HKEY, value_name: &str) -> Result<(), DeploymentError> {
    let wide = to_wide(value_name);
    // SAFETY: `wide` is NUL-terminated UTF-16.
    let status = unsafe { RegDeleteValueW(key, wide.as_ptr()) };
    match status {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        code => Err(DeploymentError::Windows {
            operation: "RegDeleteValueW",
            code,
        }),
    }
}

/// Bounded, type-checked registry reads used by the recovery paths.
pub(crate) fn read_dword(key: &OwnedKey, name: &str) -> Result<Option<u32>, DeploymentError> {
    let Some((kind, payload)) = query_raw(key.0, name)? else {
        return Ok(None);
    };
    if kind != REG_DWORD || payload.len() != 4 {
        return Err(DeploymentError::Rejected(BackupRejection::Malformed {
            field: to_static_field(name),
        }));
    }
    Ok(Some(u32::from_le_bytes(
        payload[..4].try_into().expect("length checked"),
    )))
}

/// Reads and validates a bounded UTF-16 string value (raw, no expansion).
pub(crate) fn read_string(key: &OwnedKey, name: &str) -> Result<Option<String>, DeploymentError> {
    let Some((kind, payload)) = query_raw(key.0, name)? else {
        return Ok(None);
    };
    if kind != REG_SZ {
        return Err(DeploymentError::Rejected(BackupRejection::Malformed {
            field: to_static_field(name),
        }));
    }
    decode_utf16_bytes(&payload)
        .map(Some)
        .ok_or(DeploymentError::InvalidUtf16 {
            context: to_static_field(name),
        })
}

/// The live `Shell` value under Winlogon, read as raw, unexpanded data.
/// A missing Winlogon key or missing value is `Ok(None)`.
pub fn read_live_shell() -> Result<Option<OriginalShellState>, DeploymentError> {
    read_live_shell_at(HKEY_CURRENT_USER, crate::shell_recovery::WINLOGON_SUBKEY)
}

pub(crate) fn read_live_shell_at(
    parent: HKEY,
    winlogon_subkey: &str,
) -> Result<Option<OriginalShellState>, DeploymentError> {
    let Some(key) = OwnedKey::open(parent, winlogon_subkey, KEY_READ)? else {
        return Ok(None);
    };
    let Some((kind, payload)) = query_raw(key.0, crate::shell_recovery::SHELL_VALUE_NAME)? else {
        return Ok(None);
    };
    let state = match kind {
        REG_SZ => OriginalShellState::RawString(
            decode_utf16_bytes(&payload).ok_or(DeploymentError::ShellValueUnreadable)?,
        ),
        REG_EXPAND_SZ => OriginalShellState::RawExpandString(
            decode_utf16_bytes(&payload).ok_or(DeploymentError::ShellValueUnreadable)?,
        ),
        other => return Err(DeploymentError::ShellValueNotString { kind: other }),
    };
    Ok(Some(state))
}

/// Reads and validates the entire backup record from a single open key.
pub(crate) fn read_backup_from(
    key: &OwnedKey,
) -> Result<Option<ShellRecoveryBackup>, DeploymentError> {
    let schema = read_dword(key, crate::shell_recovery::FIELD_SCHEMA_VERSION)?;
    let Some(schema) = schema else {
        return Ok(None);
    };
    // An inactive record is reported as "no usable backup", making a repeat
    // restore after the GUI already restored an idempotent no-op. A missing
    // Active value is malformed instead, because a complete record always
    // carries it.
    let active = read_dword(key, crate::shell_recovery::FIELD_ACTIVE)?.ok_or(
        BackupRejection::Malformed {
            field: crate::shell_recovery::FIELD_ACTIVE,
        },
    )?;
    if active == 0 {
        return Ok(None);
    }
    if active != 1 {
        return Err(BackupRejection::Malformed {
            field: crate::shell_recovery::FIELD_ACTIVE,
        }
        .into());
    }
    let shell_command = read_string(key, crate::shell_recovery::FIELD_SHELL_COMMAND)?.ok_or(
        BackupRejection::Malformed {
            field: crate::shell_recovery::FIELD_SHELL_COMMAND,
        },
    )?;
    let original_present = read_dword(key, crate::shell_recovery::FIELD_ORIGINAL_PRESENT)?.ok_or(
        BackupRejection::Malformed {
            field: crate::shell_recovery::FIELD_ORIGINAL_PRESENT,
        },
    )?;
    let original_kind = read_dword(key, crate::shell_recovery::FIELD_ORIGINAL_KIND)?.ok_or(
        BackupRejection::Malformed {
            field: crate::shell_recovery::FIELD_ORIGINAL_KIND,
        },
    )?;
    let install_directory = read_string(key, crate::shell_recovery::FIELD_INSTALL_DIRECTORY)?
        .ok_or(BackupRejection::Malformed {
            field: crate::shell_recovery::FIELD_INSTALL_DIRECTORY,
        })?;
    let original_value = read_string(key, crate::shell_recovery::FIELD_ORIGINAL_VALUE)?.ok_or(
        BackupRejection::Malformed {
            field: crate::shell_recovery::FIELD_ORIGINAL_VALUE,
        },
    )?;

    if schema != ShellRecoveryBackup::SCHEMA_VERSION {
        return Err(BackupRejection::UnsupportedSchema { version: schema }.into());
    }
    let original = match (original_present, original_kind) {
        (0, ShellRecoveryBackup::KIND_ABSENT) if original_value.is_empty() => {
            OriginalShellState::Absent
        }
        (1, ShellRecoveryBackup::KIND_STRING) => OriginalShellState::RawString(original_value),
        (1, ShellRecoveryBackup::KIND_EXPAND_STRING) => {
            OriginalShellState::RawExpandString(original_value)
        }
        _ => return Err(BackupRejection::InconsistentKind.into()),
    };
    Ok(Some(ShellRecoveryBackup::new(
        shell_command,
        original,
        install_directory,
    )?))
}

/// Reads and validates the active backup from the real recovery subkey. A
/// missing recovery key is `Ok(None)`.
pub fn read_backup() -> Result<Option<ShellRecoveryBackup>, DeploymentError> {
    read_backup_at(HKEY_CURRENT_USER, crate::shell_recovery::RECOVERY_SUBKEY)
}

pub(crate) fn read_backup_at(
    parent: HKEY,
    recovery_subkey: &str,
) -> Result<Option<ShellRecoveryBackup>, DeploymentError> {
    let Some(key) = OwnedKey::open(parent, recovery_subkey, KEY_READ)? else {
        return Ok(None);
    };
    read_backup_from(&key)
}

/// Executes a decided restoration against the live registry.
pub fn apply_restore(
    backup: &ShellRecoveryBackup,
    decision: &RestoreDecision,
) -> Result<(), DeploymentError> {
    apply_restore_at(
        HKEY_CURRENT_USER,
        crate::shell_recovery::WINLOGON_SUBKEY,
        crate::shell_recovery::RECOVERY_SUBKEY,
        backup,
        decision,
    )
}

/// Executes a decided restoration with explicit subkeys, so tests exercise
/// the exact production path against a private subtree.
pub(crate) fn apply_restore_at(
    parent: HKEY,
    winlogon_subkey: &str,
    recovery_subkey: &str,
    backup: &ShellRecoveryBackup,
    decision: &RestoreDecision,
) -> Result<(), DeploymentError> {
    if matches!(
        decision,
        RestoreDecision::RefuseClobber | RestoreDecision::RefuseMalformed(_)
    ) {
        return Ok(());
    }
    // Re-read immediately before mutation. Our file lock serializes Tessera's
    // actors, not third-party registry writers; never trust a stale decision.
    let live = read_live_shell_at(parent, winlogon_subkey)?;
    let decision = crate::shell_recovery::decide_restore(Some(backup), live.as_ref());
    match decision {
        RestoreDecision::AlreadyRestored => clear_active_at(parent, recovery_subkey),
        RestoreDecision::RestoreOriginal => {
            // Write access only on this path; reads never request it.
            let Some(key) = OwnedKey::open(parent, winlogon_subkey, KEY_READ | KEY_WRITE)? else {
                return Err(DeploymentError::Windows {
                    operation: "RegOpenKeyExW",
                    code: ERROR_FILE_NOT_FOUND,
                });
            };
            match backup.original() {
                OriginalShellState::Absent => {
                    delete_value(key.0, crate::shell_recovery::SHELL_VALUE_NAME)?;
                    if query_raw(key.0, crate::shell_recovery::SHELL_VALUE_NAME)?.is_some() {
                        return Err(DeploymentError::RestoreVerificationFailed {
                            value: crate::shell_recovery::SHELL_VALUE_NAME,
                        });
                    }
                }
                OriginalShellState::RawString(value) => {
                    let payload = crate::shell_recovery::utf16_bytes(value);
                    set_raw(
                        key.0,
                        crate::shell_recovery::SHELL_VALUE_NAME,
                        REG_SZ,
                        &payload,
                    )?;
                    verify_value(
                        &key,
                        crate::shell_recovery::SHELL_VALUE_NAME,
                        REG_SZ,
                        &payload,
                    )?;
                }
                OriginalShellState::RawExpandString(value) => {
                    let payload = crate::shell_recovery::utf16_bytes(value);
                    set_raw(
                        key.0,
                        crate::shell_recovery::SHELL_VALUE_NAME,
                        REG_EXPAND_SZ,
                        &payload,
                    )?;
                    verify_value(
                        &key,
                        crate::shell_recovery::SHELL_VALUE_NAME,
                        REG_EXPAND_SZ,
                        &payload,
                    )?;
                }
            }
            clear_active_at(parent, recovery_subkey)
        }
        // Refusals never touch the registry; the caller reports the error.
        RestoreDecision::RefuseClobber | RestoreDecision::RefuseMalformed(_) => {
            Err(DeploymentError::ShellOwnershipChanged)
        }
    }
}

/// Reads back a written value and requires the exact type and payload.
fn verify_value(
    key: &OwnedKey,
    value_name: &'static str,
    kind: u32,
    payload: &[u8],
) -> Result<(), DeploymentError> {
    match query_raw(key.0, value_name)? {
        Some((got_kind, got_payload)) if got_kind == kind && got_payload == payload => Ok(()),
        _ => Err(DeploymentError::RestoreVerificationFailed { value: value_name }),
    }
}

/// Clears `Active` after a verified restore; the remaining backup fields are
/// kept for diagnosis but no longer authorize a second restore. Only a flag
/// that is exactly `1` is cleared: a missing, zero, or foreign value is left
/// untouched, and the write is verified by readback.
fn clear_active_at(parent: HKEY, recovery_subkey: &str) -> Result<(), DeploymentError> {
    let Some(key) = OwnedKey::open(parent, recovery_subkey, KEY_READ | KEY_WRITE)? else {
        return Ok(());
    };
    match read_dword(&key, crate::shell_recovery::FIELD_ACTIVE)? {
        // Nothing recorded, already cleared, or not ours to touch.
        None | Some(0) => Ok(()),
        Some(other) if other != 1 => Ok(()),
        Some(_) => {
            set_raw(
                key.0,
                crate::shell_recovery::FIELD_ACTIVE,
                REG_DWORD,
                &0u32.to_le_bytes(),
            )?;
            match query_raw(key.0, crate::shell_recovery::FIELD_ACTIVE)? {
                Some((kind, payload)) if kind == REG_DWORD && payload == 0u32.to_le_bytes() => {
                    Ok(())
                }
                _ => Err(DeploymentError::RestoreVerificationFailed {
                    value: crate::shell_recovery::FIELD_ACTIVE,
                }),
            }
        }
    }
}

/// Resolves the per-user LocalAppData base for Tessera state.
fn local_app_data() -> Result<PathBuf, DeploymentError> {
    let mut result: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: out-param is valid; the returned buffer is freed below.
    let status = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_LocalAppData,
            KF_FLAG_DEFAULT as u32,
            std::ptr::null_mut(),
            &mut result,
        )
    };
    if status != 0 {
        return Err(DeploymentError::UnusablePath {
            context: "LocalAppData",
        });
    }
    let owned = unsafe { OwnedWide::take(result) };
    // SAFETY: `owned` owns the allocation for the duration of this call.
    let units = unsafe { owned.as_wide_until_nul(MAX_VALUE_CHARS) }.map_err(|_| {
        DeploymentError::UnusablePath {
            context: "LocalAppData",
        }
    })?;
    Ok(PathBuf::from(OsString::from_wide(units)))
}

/// RAII for a CoTaskMem-allocated wide string.
struct OwnedWide(windows_sys::core::PWSTR);

impl OwnedWide {
    /// Takes ownership of a string returned by a Win32 allocator.
    ///
    /// # Safety
    /// `ptr` must come from a single allocation owned by this call site.
    unsafe fn take(ptr: windows_sys::core::PWSTR) -> Self {
        Self(ptr)
    }

    /// The units up to the NUL terminator, capped at `max_units`; `Err` when
    /// the terminator is not found within the cap or the pointer is null.
    ///
    /// # Safety
    /// The allocation must stay alive for the returned borrow and be
    /// NUL-terminated within `max_units` units, per the Win32 contract for
    /// `SHGetKnownFolderPath` output.
    unsafe fn as_wide_until_nul(&self, max_units: usize) -> Result<&[u16], ()> {
        if self.0.is_null() {
            return Err(());
        }
        // SAFETY: the pointer is non-null and NUL-terminated within
        // `max_units` units; the scan never reads past that terminator.
        unsafe {
            let mut end = 0usize;
            while end < max_units && *self.0.add(end) != 0 {
                end += 1;
            }
            if end == max_units && *self.0.add(end) != 0 {
                return Err(());
            }
            Ok(std::slice::from_raw_parts(self.0, end))
        }
    }
}

impl Drop for OwnedWide {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from SHGetKnownFolderPath's allocator.
            unsafe {
                windows_sys::Win32::System::Com::CoTaskMemFree(self.0 as *const _);
            }
        }
    }
}

/// Holds the per-user deployment lock with share mode 0 for the guard's
/// lifetime. Lock contention surfaces as [`DeploymentError::LockBusy`].
pub struct DeploymentLock {
    _file: std::fs::File,
}

impl DeploymentLock {
    /// Creates the Tessera directory if needed and takes the lock.
    pub fn acquire() -> Result<Self, DeploymentError> {
        let base = local_app_data()?.join("Tessera");
        Self::acquire_in(&base)
    }

    /// Takes the lock on `directory\deployment.lock`, so tests can isolate
    /// the fixture from real per-user state.
    pub(crate) fn acquire_in(directory: &std::path::Path) -> Result<Self, DeploymentError> {
        std::fs::create_dir_all(directory).map_err(|_| DeploymentError::UnusablePath {
            context: "Tessera directory",
        })?;
        let path = directory.join("deployment.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .share_mode(0)
            .open(&path)
            .map_err(|error| {
                // A sharing violation (the lock file held with share mode 0)
                // means another deployment owns the lock. `raw_os_error` is
                // checked explicitly: Windows maps ERROR_SHARING_VIOLATION
                // (32) and ERROR_LOCK_VIOLATION (33) to uncategorized
                // io::Error kinds, which must never read as a path problem.
                match error.raw_os_error() {
                    Some(32 | 33) => DeploymentError::LockBusy,
                    _ => match error.kind() {
                        io::ErrorKind::AlreadyExists | io::ErrorKind::PermissionDenied => {
                            DeploymentError::LockBusy
                        }
                        _ => DeploymentError::UnusablePath {
                            context: "deployment.lock",
                        },
                    },
                }
            })?;
        Ok(Self { _file: file })
    }
}

/// Absolute path of the running supervisor, fully qualified.
pub fn supervisor_path() -> Result<PathBuf, DeploymentError> {
    let mut buffer = vec![0u16; MAX_VALUE_CHARS];
    // SAFETY: buffer and size agree; GetModuleFileNameW writes at most
    // buffer.len() units and NUL-terminates within the buffer.
    let written = unsafe {
        windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(
            std::ptr::null_mut(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    };
    if written == 0 || written as usize >= buffer.len() {
        return Err(DeploymentError::UnusablePath {
            context: "supervisor path",
        });
    }
    buffer.truncate(written as usize);
    let path = PathBuf::from(OsString::from_wide(&buffer));
    if !path.is_absolute() {
        return Err(DeploymentError::UnusablePath {
            context: "supervisor path is relative",
        });
    }
    Ok(path)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
