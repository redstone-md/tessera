// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The account Tessera runs under, and the folders Windows keeps for it.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameDisplay};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Profile, FOLDERID_Videos, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::core::{GUID, HSTRING, PCWSTR, PWSTR, w};

/// The longest name asked for, in UTF-16 units.
const NAME_LENGTH: usize = 512;

/// Who is signed in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    /// The name the sign-in screen shows, or else the one of the account.
    pub name: String,
    /// The name of the account itself.
    pub login: String,
    /// The file of the picture of the account, if it has one.
    pub picture: Option<PathBuf>,
}

impl Account {
    /// The account this process runs under.
    pub fn current() -> Self {
        let login = std::env::var("USERNAME").unwrap_or_default();
        let name = display_name()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| login.clone());
        let picture = identifier().and_then(|id| picture(&id));
        Self {
            name,
            login,
            picture,
        }
    }
}

/// The full name of the account, where it has one.
fn display_name() -> Option<String> {
    let mut name = [0_u16; NAME_LENGTH];
    let mut length = NAME_LENGTH as u32;
    // SAFETY: `length` is the size of the buffer `name`, and both outlive
    // the call.
    let told =
        unsafe { GetUserNameExW(NameDisplay, Some(PWSTR(name.as_mut_ptr())), &raw mut length) };
    let name = name.get(..length as usize)?;
    told.then(|| String::from_utf16_lossy(name))
}

/// The security identifier of the account, as text.
fn identifier() -> Option<String> {
    let mut token = HANDLE::default();
    // SAFETY: the handle of the current process needs no closing; `token`
    // outlives the call and is closed below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }.ok()?;
    // Eight-byte units keep the structure Windows writes aligned.
    let mut user = [0_u64; 32];
    let mut written = 0_u32;
    // SAFETY: the buffer is as large as the size given and aligned for
    // `TOKEN_USER`; the token is open and is closed exactly once.
    let read = unsafe {
        let read = GetTokenInformation(
            token,
            TokenUser,
            Some(user.as_mut_ptr().cast::<c_void>()),
            size_of_val(&user) as u32,
            &raw mut written,
        );
        let _ = CloseHandle(token);
        read
    };
    read.ok()?;
    let mut text = PWSTR::null();
    // SAFETY: on success the buffer begins with a `TOKEN_USER` whose
    // identifier lies within the buffer; the text Windows allocates is
    // copied and then freed exactly once.
    unsafe {
        let user = &*user.as_ptr().cast::<TOKEN_USER>();
        ConvertSidToStringSidW(user.User.Sid, &raw mut text).ok()?;
        let identifier = text.to_string().ok();
        LocalFree(Some(HLOCAL(text.0.cast())));
        identifier
    }
}

/// The file Windows keeps of the picture of the account `identifier`.
fn picture(identifier: &str) -> Option<PathBuf> {
    let key = HSTRING::from(format!(
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\AccountPicture\\Users\\{identifier}"
    ));
    // The smallest size that stays sharp where Tessera draws it.
    [w!("Image192"), w!("Image208"), w!("Image96")]
        .into_iter()
        .filter_map(|size| registry_text(&key, size))
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

/// The text value `name` of the key `key` of the machine.
fn registry_text(key: &HSTRING, name: PCWSTR) -> Option<String> {
    let mut text = [0_u16; NAME_LENGTH];
    let mut size = size_of_val(&text) as u32;
    // SAFETY: `size` is the size in bytes of the buffer `text`, both
    // outlive the call, and the names are NUL-terminated.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    status.ok().ok()?;
    let length = text.iter().position(|&unit| unit == 0)?;
    Some(String::from_utf16_lossy(&text[..length]))
}

/// A folder Windows keeps for every account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Videos,
}

impl Place {
    pub const ALL: [Self; 7] = [
        Self::Home,
        Self::Desktop,
        Self::Documents,
        Self::Downloads,
        Self::Pictures,
        Self::Music,
        Self::Videos,
    ];

    /// What a layout calls the folder.
    pub fn code(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Desktop => "desktop",
            Self::Documents => "documents",
            Self::Downloads => "downloads",
            Self::Pictures => "pictures",
            Self::Music => "music",
            Self::Videos => "videos",
        }
    }

    /// Where the folder is, which the user may have changed.
    pub fn path(self) -> Option<PathBuf> {
        let id: &GUID = match self {
            Self::Home => &FOLDERID_Profile,
            Self::Desktop => &FOLDERID_Desktop,
            Self::Documents => &FOLDERID_Documents,
            Self::Downloads => &FOLDERID_Downloads,
            Self::Pictures => &FOLDERID_Pictures,
            Self::Music => &FOLDERID_Music,
            Self::Videos => &FOLDERID_Videos,
        };
        // SAFETY: the identifier outlives the call; the text Windows
        // allocates is copied and then freed exactly once.
        unsafe {
            let text = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
            let path = text.to_string().ok().map(PathBuf::from);
            CoTaskMemFree(Some(text.0.cast()));
            path
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_place_has_its_own_name() {
        let mut codes: Vec<&str> = Place::ALL.iter().map(|place| place.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), Place::ALL.len());
    }

    #[test]
    #[ignore = "reads the account of the machine it runs on"]
    fn the_account_has_a_name_and_a_home() {
        let account = Account::current();
        assert!(!account.name.is_empty());
        assert!(Place::Home.path().is_some_and(|path| path.is_dir()));
    }
}
