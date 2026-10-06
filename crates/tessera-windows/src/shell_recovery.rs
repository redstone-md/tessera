// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shared recovery vocabulary and pure ownership policy; no registry/file I/O.
use std::fmt;

#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod native;

#[cfg(windows)]
pub const SHELL_VALUE_NAME: &str = "Shell";
#[cfg(windows)]
pub const WINLOGON_SUBKEY: &str = "Software\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon";
#[cfg(windows)]
pub const RECOVERY_SUBKEY: &str = "Software\\Tessera\\ShellRecovery";
pub const FIELD_SCHEMA_VERSION: &str = "SchemaVersion";
pub const FIELD_ACTIVE: &str = "Active";
pub const FIELD_SHELL_COMMAND: &str = "ShellCommand";
pub const FIELD_ORIGINAL_PRESENT: &str = "OriginalPresent";
pub const FIELD_ORIGINAL_KIND: &str = "OriginalKind";
pub const FIELD_ORIGINAL_VALUE: &str = "OriginalValue";
pub const FIELD_INSTALL_DIRECTORY: &str = "InstallDirectory";
pub const MAX_VALUE_CHARS: usize = 32_768;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupRejection {
    Malformed {
        field: &'static str,
    },
    #[cfg(windows)]
    UnsupportedSchema {
        version: u32,
    },
    InvalidPath {
        field: &'static str,
    },
    #[cfg(windows)]
    InconsistentKind,
}
impl fmt::Display for BackupRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { field } => {
                write!(f, "backup field `{field}` is missing or malformed")
            }
            #[cfg(windows)]
            Self::UnsupportedSchema { version } => {
                write!(f, "unsupported backup schema version {version}")
            }
            Self::InvalidPath { field } => {
                write!(f, "backup field `{field}` is not a valid installed path")
            }
            #[cfg(windows)]
            Self::InconsistentKind => write!(f, "backup original-kind flags are inconsistent"),
        }
    }
}
impl std::error::Error for BackupRejection {}

/// Presence and raw unexpanded data, including the original registry kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginalShellState {
    Absent,
    RawString(String),
    RawExpandString(String),
}
impl OriginalShellState {
    fn value(&self) -> Option<&str> {
        match self {
            Self::Absent => None,
            Self::RawString(value) | Self::RawExpandString(value) => Some(value),
        }
    }
}

/// Validated immutable record. Production activation is exclusively PowerShell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellRecoveryBackup {
    shell_command: String,
    original: OriginalShellState,
    install_directory: String,
}
impl ShellRecoveryBackup {
    pub const SCHEMA_VERSION: u32 = 1;
    pub const KIND_ABSENT: u32 = 1;
    pub const KIND_STRING: u32 = 1;
    pub const KIND_EXPAND_STRING: u32 = 2;

    pub fn new(
        shell_command: String,
        original: OriginalShellState,
        install_directory: String,
    ) -> Result<Self, BackupRejection> {
        if !is_absolute_windows_path(&install_directory) || install_directory.contains('"') {
            return Err(BackupRejection::InvalidPath {
                field: FIELD_INSTALL_DIRECTORY,
            });
        }
        let executable = shell_command
            .strip_prefix('"')
            .and_then(|path| path.strip_suffix('"'))
            .filter(|path| is_absolute_windows_path(path))
            .ok_or(BackupRejection::InvalidPath {
                field: FIELD_SHELL_COMMAND,
            })?;
        let belongs = executable
            .rsplit_once(['\\', '/'])
            .is_some_and(|(directory, file)| {
                directory == install_directory.trim_end_matches(['\\', '/'])
                    && file.eq_ignore_ascii_case("tessera-shell.exe")
            });
        if !belongs {
            return Err(BackupRejection::InvalidPath {
                field: FIELD_SHELL_COMMAND,
            });
        }
        if let Some(value) = original.value()
            && (value.encode_utf16().count() >= MAX_VALUE_CHARS || value.contains('\0'))
        {
            return Err(BackupRejection::Malformed {
                field: FIELD_ORIGINAL_VALUE,
            });
        }
        Ok(Self {
            shell_command,
            original,
            install_directory,
        })
    }
    pub fn shell_command(&self) -> &str {
        &self.shell_command
    }
    pub fn original(&self) -> &OriginalShellState {
        &self.original
    }
    pub fn install_directory(&self) -> &str {
        &self.install_directory
    }

    /// Test fixtures mirror all seven PowerShell fields, including an empty REG_SZ.
    #[cfg(test)]
    pub fn encode(&self) -> Vec<(&'static str, RegistryType, Vec<u8>)> {
        let kind = match self.original {
            OriginalShellState::Absent => Self::KIND_ABSENT,
            OriginalShellState::RawString(_) => Self::KIND_STRING,
            OriginalShellState::RawExpandString(_) => Self::KIND_EXPAND_STRING,
        };
        vec![
            (
                FIELD_SCHEMA_VERSION,
                RegistryType::Dword,
                Self::SCHEMA_VERSION.to_le_bytes().to_vec(),
            ),
            (
                FIELD_ACTIVE,
                RegistryType::Dword,
                1u32.to_le_bytes().to_vec(),
            ),
            (
                FIELD_SHELL_COMMAND,
                RegistryType::String,
                utf16_bytes(self.shell_command()),
            ),
            (
                FIELD_ORIGINAL_PRESENT,
                RegistryType::Dword,
                u32::from(self.original.value().is_some())
                    .to_le_bytes()
                    .to_vec(),
            ),
            (
                FIELD_ORIGINAL_KIND,
                RegistryType::Dword,
                kind.to_le_bytes().to_vec(),
            ),
            (
                FIELD_ORIGINAL_VALUE,
                RegistryType::String,
                utf16_bytes(self.original.value().unwrap_or("")),
            ),
            (
                FIELD_INSTALL_DIRECTORY,
                RegistryType::String,
                utf16_bytes(self.install_directory()),
            ),
        ]
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryType {
    String,
    Dword,
}
#[cfg(test)]
impl RegistryType {
    pub fn as_raw(self) -> u32 {
        match self {
            Self::String => 1,
            Self::Dword => 4,
        }
    }
}

pub fn utf16_bytes(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}
pub fn decode_utf16_bytes(raw: &[u8]) -> Option<String> {
    if raw.len() < 2 || !raw.len().is_multiple_of(2) || raw.len() > MAX_VALUE_CHARS * 2 {
        return None;
    }
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16(&units[..end]).ok()
}

/// Windows-rooted literal path, optionally quoted as one executable; never expanded.
pub fn is_absolute_windows_path(value: &str) -> bool {
    let path = value
        .strip_prefix('"')
        .and_then(|path| path.strip_suffix('"'))
        .unwrap_or(value);
    if path.is_empty()
        || path.encode_utf16().count() >= MAX_VALUE_CHARS
        || path.contains('"')
        || path.chars().any(char::is_control)
    {
        return false;
    }
    let bytes = path.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/'))
        || path
            .strip_prefix("\\\\")
            .is_some_and(|rest| rest.split('\\').filter(|part| !part.is_empty()).count() >= 2)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreDecision {
    RestoreOriginal,
    AlreadyRestored,
    RefuseClobber,
    RefuseMalformed(BackupRejection),
}

/// Ownership is derived from the record, not a caller-supplied boolean.
pub fn decide_restore(
    backup: Option<&ShellRecoveryBackup>,
    live: Option<&OriginalShellState>,
) -> RestoreDecision {
    let Some(backup) = backup else {
        return RestoreDecision::RefuseMalformed(BackupRejection::Malformed {
            field: FIELD_SCHEMA_VERSION,
        });
    };
    if live.unwrap_or(&OriginalShellState::Absent) == backup.original() {
        return RestoreDecision::AlreadyRestored;
    }
    if matches!(live, Some(OriginalShellState::RawString(value)) if value == backup.shell_command())
    {
        RestoreDecision::RestoreOriginal
    } else {
        RestoreDecision::RefuseClobber
    }
}

#[cfg(test)]
#[path = "shell_recovery/policy_tests.rs"]
mod policy_tests;
