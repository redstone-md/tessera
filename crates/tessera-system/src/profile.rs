// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit, memory-only current-user profile reads and closed navigation intents.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileErrorKind {
    Unsupported,
    AccessDenied,
    NotFound,
    InvalidData,
    Busy,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileError {
    kind: ProfileErrorKind,
    message: &'static str,
    native_code: Option<i32>,
}

impl ProfileError {
    /// Messages are fixed diagnostic text, never native strings or user data.
    pub fn new(kind: ProfileErrorKind, message: &'static str, native_code: Option<i32>) -> Self {
        Self {
            kind,
            message: if message.len() <= 256 && !message.chars().any(char::is_control) {
                message
            } else {
                "The profile request could not be completed."
            },
            native_code,
        }
    }

    pub fn kind(&self) -> ProfileErrorKind {
        self.kind
    }

    pub fn message(&self) -> &'static str {
        self.message
    }

    pub fn native_code(&self) -> Option<i32> {
        self.native_code
    }
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for ProfileError {}

#[derive(Clone, Eq, PartialEq)]
pub struct ProfilePhoto {
    width: u32,
    height: u32,
    rgba: Arc<[u8]>,
}

impl ProfilePhoto {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, ProfileError> {
        let expected = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .and_then(|bytes| usize::try_from(bytes).ok());
        if width == 0
            || height == 0
            || width > 512
            || height > 512
            || expected != Some(rgba.len())
            || rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[0] > pixel[3] || pixel[1] > pixel[3] || pixel[2] > pixel[3])
        {
            return Err(invalid_data());
        }
        Ok(Self {
            width,
            height,
            rgba: rgba.into(),
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

impl fmt::Debug for ProfilePhoto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfilePhoto")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfilePhotoState {
    Absent,
    Ready(ProfilePhoto),
    Unavailable(ProfileErrorKind),
}

/// Provider-issued expectation, not a path, command or persistent identity.
/// Providers must validate their private payload and issuer before using it.
#[derive(Clone)]
pub struct ProfileTarget(Arc<dyn Any + Send + Sync>);

impl ProfileTarget {
    pub fn new<T: Any + Send + Sync>(expectation: T) -> Self {
        Self(Arc::new(expectation))
    }

    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl fmt::Debug for ProfileTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProfileTarget(<redacted>)")
    }
}

#[derive(Clone)]
pub struct ProfileSnapshot {
    display_name: String,
    photo: ProfilePhotoState,
    personal_email: Option<String>,
    onedrive: Option<ProfileTarget>,
}

impl ProfileSnapshot {
    pub fn new(
        display_name: String,
        photo: ProfilePhotoState,
        personal_email: Option<String>,
        onedrive: Option<ProfileTarget>,
    ) -> Result<Self, ProfileError> {
        if !valid_text(&display_name)
            || personal_email
                .as_deref()
                .is_some_and(|email| !valid_text(email))
        {
            return Err(invalid_data());
        }
        Ok(Self {
            display_name,
            photo,
            personal_email,
            onedrive,
        })
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn photo(&self) -> &ProfilePhotoState {
        &self.photo
    }

    /// Only the optional OneDrive Personal UserEmail value, not an OS account claim.
    pub fn personal_email(&self) -> Option<&str> {
        self.personal_email.as_deref()
    }

    pub fn onedrive(&self) -> Option<&ProfileTarget> {
        self.onedrive.as_ref()
    }
}

impl fmt::Debug for ProfileSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileSnapshot")
            .field("display_name", &"<redacted>")
            .field("photo", &self.photo)
            .field("has_personal_email", &self.personal_email.is_some())
            .field("has_onedrive", &self.onedrive.is_some())
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum ProfileCommand {
    OpenHome,
    OpenAccountsSettings,
    OpenOneDrive { expected: ProfileTarget },
}

pub type ProfileReadCompletion = Box<dyn FnOnce(Result<ProfileSnapshot, ProfileError>) + Send>;
pub type ProfileOpenCompletion = Box<dyn FnOnce(Result<(), ProfileError>) + Send>;

pub trait ProfileHost: Send + Sync {
    /// Ok accepts exactly one completion; Err accepts none. May complete inline.
    fn read(&self, completion: ProfileReadCompletion) -> Result<(), ProfileError>;

    /// Closed user-clicked intents only; no UI-supplied paths or URI transport.
    fn execute(
        &self,
        command: ProfileCommand,
        completion: ProfileOpenCompletion,
    ) -> Result<(), ProfileError>;
}

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 1_024 && !value.chars().any(char::is_control)
}

fn invalid_data() -> ProfileError {
    ProfileError::new(
        ProfileErrorKind::InvalidData,
        "The profile data is invalid.",
        None,
    )
}

#[cfg(test)]
#[path = "profile/tests.rs"]
mod tests;
