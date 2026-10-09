// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Fixed sources and provider-owned expectations, independently testable without user data.

use std::io::Read;
use std::sync::Arc;
use tessera_system::profile::{
    ProfileCommand, ProfileError, ProfileErrorKind, ProfilePhoto, ProfilePhotoState,
    ProfileSnapshot, ProfileTarget,
};

use super::actor::Driver;

pub(crate) const PHOTO_KEYS: [&str; 11] = [
    "Image1080",
    "Image448",
    "Image424",
    "Image240",
    "Image208",
    "Image192",
    "Image96",
    "Image64",
    "Image48",
    "Image40",
    "Image32",
];
pub(crate) const PERSONAL_KEY: &str = "Software\\Microsoft\\OneDrive\\Accounts\\Personal";
pub(crate) const ACCOUNTS_URI: &str = "ms-settings:accounts";
pub(crate) const ENCODED_LIMIT: usize = 8 * 1_024 * 1_024;

#[derive(Clone, Copy)]
pub(crate) enum Value<'a> {
    Photo { sid: &'a str, quality: &'static str },
    PersonalEmail,
    PersonalFolder,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct DirectoryIdentity {
    pub(crate) volume: u64,
    pub(crate) file_id: [u8; 16],
}

/// This seam never accepts an arbitrary registry hive/key or launch command.
pub(crate) trait Backend: 'static {
    fn current_sid(&mut self) -> Result<String, ProfileError>;
    fn display_name(&mut self) -> Result<String, ProfileError>;
    fn value(&mut self, source: Value<'_>) -> Result<Option<String>, ProfileError>;
    fn photo(&mut self, local_path: &str) -> Result<ProfilePhoto, ProfileError>;
    fn directory(&mut self, local_path: &str) -> Result<DirectoryIdentity, ProfileError>;
    fn open_directory(
        &mut self,
        local_path: &str,
        identity: DirectoryIdentity,
    ) -> Result<(), ProfileError>;
    fn open_home(&mut self) -> Result<(), ProfileError>;
    fn open_accounts(&mut self) -> Result<(), ProfileError>;
}

struct OneDriveExpectation {
    issuer: Arc<()>,
    sid: String,
    path: String,
    directory: DirectoryIdentity,
}

pub(crate) struct Reader<B> {
    backend: B,
    issuer: Arc<()>,
}

impl<B: Backend> Reader<B> {
    pub(crate) fn new(backend: B) -> Self {
        Self {
            backend,
            issuer: Arc::new(()),
        }
    }

    fn photo(&mut self, sid: &str) -> ProfilePhotoState {
        let mut failure = None;
        for quality in PHOTO_KEYS {
            let result = self
                .backend
                .value(Value::Photo { sid, quality })
                .and_then(|path| {
                    path.map(|path| {
                        validate_local_path(&path, true)?;
                        self.backend.photo(&path)
                    })
                    .transpose()
                });
            match result {
                Ok(Some(photo)) => return ProfilePhotoState::Ready(photo),
                Ok(None) => {}
                Err(error) if error.kind() == ProfileErrorKind::NotFound => {}
                Err(error) => {
                    failure.get_or_insert(error.kind());
                }
            }
        }
        failure.map_or(ProfilePhotoState::Absent, ProfilePhotoState::Unavailable)
    }

    fn onedrive(&mut self, sid: &str) -> Option<ProfileTarget> {
        let path = self.backend.value(Value::PersonalFolder).ok().flatten()?;
        validate_local_path(&path, false).ok()?;
        let directory = self.backend.directory(&path).ok()?;
        Some(ProfileTarget::new(OneDriveExpectation {
            issuer: Arc::clone(&self.issuer),
            sid: sid.to_owned(),
            path,
            directory,
        }))
    }
}

impl<B: Backend> Driver for Reader<B> {
    fn read(&mut self) -> Result<ProfileSnapshot, ProfileError> {
        let sid = self.backend.current_sid()?;
        validate_sid_string(&sid)?;
        let display_name = self.backend.display_name()?;
        let photo = self.photo(&sid);
        // Optional Personal sources are independent of both identity and photo.
        let email = self
            .backend
            .value(Value::PersonalEmail)
            .ok()
            .flatten()
            .filter(|value| {
                !value.trim().is_empty()
                    && value.len() <= 1_024
                    && !value.chars().any(char::is_control)
            });
        let onedrive = self.onedrive(&sid);
        if self.backend.current_sid()? != sid {
            return Err(stale());
        }
        ProfileSnapshot::new(display_name, photo, email, onedrive)
    }

    fn execute(&mut self, command: ProfileCommand) -> Result<(), ProfileError> {
        match command {
            ProfileCommand::OpenHome => self.backend.open_home(),
            ProfileCommand::OpenAccountsSettings => self.backend.open_accounts(),
            ProfileCommand::OpenOneDrive { expected } => {
                let expected = expected
                    .downcast_ref::<OneDriveExpectation>()
                    .ok_or_else(stale)?;
                if !Arc::ptr_eq(&self.issuer, &expected.issuer)
                    || self.backend.current_sid()? != expected.sid
                {
                    return Err(stale());
                }
                let path = self
                    .backend
                    .value(Value::PersonalFolder)?
                    .ok_or_else(stale)?;
                validate_local_path(&path, false)?;
                if path != expected.path || self.backend.directory(&path)? != expected.directory {
                    return Err(stale());
                }
                if self.backend.current_sid()? != expected.sid {
                    return Err(stale());
                }
                // Native backend acquires/holds local path component handles and
                // rechecks this file identity again immediately before dispatch.
                self.backend.open_directory(&path, expected.directory)
            }
        }
    }
}

pub(crate) fn validate_sid_string(sid: &str) -> Result<(), ProfileError> {
    if sid.len() > 184
        || !sid.starts_with("S-1-")
        || sid[4..]
            .split('-')
            .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(invalid());
    }
    Ok(())
}

/// Only plain drive-absolute Win32 paths. No expansion, streams, device names,
/// normalization aliases, executable extensions, UNC, or namespace prefixes.
pub(crate) fn validate_local_path(path: &str, image: bool) -> Result<(), ProfileError> {
    let bytes = path.as_bytes();
    if path.encode_utf16().count() > 32_000
        || bytes.len() < 4
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || bytes[2] != b'\\'
        || path[3..]
            .chars()
            .any(|c| c.is_control() || matches!(c, ':' | '/' | '"' | '<' | '>' | '|' | '?' | '*'))
    {
        return Err(invalid());
    }
    if path[3..].split('\\').count() > 128 {
        return Err(invalid());
    }
    for part in path[3..].split('\\') {
        let stem = part
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end_matches(' ')
            .to_ascii_uppercase();
        if part.is_empty()
            || matches!(part, "." | "..")
            || part.ends_with(['.', ' '])
            || matches!(
                stem.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
            )
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
            || stem == "COM¹"
            || stem == "COM²"
            || stem == "COM³"
            || stem == "LPT¹"
            || stem == "LPT²"
            || stem == "LPT³"
        {
            return Err(invalid());
        }
    }
    let extension = path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "exe" | "com" | "bat" | "cmd" | "msi" | "scr" | "dll" | "lnk" | "url" | "ps1"
    ) {
        return Err(invalid());
    }
    if image
        && !matches!(
            extension.as_str(),
            "jpg"
                | "jpeg"
                | "png"
                | "bmp"
                | "gif"
                | "tif"
                | "tiff"
                | "webp"
                | "heic"
                | "heif"
                | "ico"
        )
    {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) fn strict_utf16(units: &[u16]) -> Result<String, ProfileError> {
    let Some((&0, text)) = units.split_last() else {
        return Err(invalid());
    };
    if text.is_empty() || text.contains(&0) || units.len() > 32_768 {
        return Err(invalid());
    }
    let value = String::from_utf16(text).map_err(|_| invalid())?;
    if value.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(value)
}

/// Copy at most the advertised bounded payload plus one sentinel byte. A file
/// that shrinks or grows cannot be mistaken for a valid immutable image input.
pub(crate) fn bounded_bytes(
    reader: &mut impl Read,
    declared: u64,
) -> Result<Vec<u8>, ProfileError> {
    let expected = usize::try_from(declared).map_err(|_| invalid())?;
    if expected == 0 || expected > ENCODED_LIMIT {
        return Err(invalid());
    }
    let mut bytes = vec![0u8; expected + 1];
    let mut filled = 0;
    while filled < bytes.len() {
        let count = match reader.read(&mut bytes[filled..]) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(ProfileError::new(
                    if error.kind() == std::io::ErrorKind::PermissionDenied {
                        ProfileErrorKind::AccessDenied
                    } else {
                        ProfileErrorKind::Other
                    },
                    "The profile photo could not be read.",
                    error.raw_os_error(),
                ));
            }
        };
        if count == 0 {
            break;
        }
        if count > bytes.len() - filled {
            return Err(invalid());
        }
        filled += count;
    }
    if filled != expected || bytes.starts_with(b"MZ") {
        return Err(invalid());
    }
    bytes.truncate(expected);
    Ok(bytes)
}

pub(crate) fn invalid() -> ProfileError {
    ProfileError::new(
        ProfileErrorKind::InvalidData,
        "The profile source data is invalid.",
        None,
    )
}

fn stale() -> ProfileError {
    ProfileError::new(
        ProfileErrorKind::InvalidData,
        "The profile target is no longer current.",
        None,
    )
}
