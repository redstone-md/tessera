// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Finding the program a word of a command names, the way the Run dialog
//! of Windows does: among the App Paths the installers register, then in
//! the folders of `PATH` with the extensions of `PATHEXT`. Also the
//! folder of the recent documents, and the expansion of `%VARIABLES%` in
//! what the user types.

#![allow(unsafe_code)]

use std::ffi::OsStr;
use std::path::PathBuf;

use windows::Win32::Storage::FileSystem::SearchPathW;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::UI::Shell::{FOLDERID_Recent, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::core::PCWSTR;

use super::execute::expand_environment;

/// Where installers register the programs that start by their name
/// alone, one key per program file name.
const APP_PATHS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";

/// The extensions a name without one is tried with where `PATHEXT` is
/// not set.
const DEFAULT_EXTENSIONS: &str = ".COM;.EXE;.BAT;.CMD";

/// The longest path asked for, in UTF-16 units.
const PATH_LENGTH: usize = 32_768;

/// The program file `name` starts where it is typed alone, as `notepad`
/// or `winword`: the one its App Paths key names, of the user first and
/// then of the machine, or else the first file of that name in the
/// folders Windows searches, `PATH` among them, with the extensions of
/// `PATHEXT` where the name has none. A name with a folder in it is
/// none of these, and gives `None`. It reads the registry and the disk:
/// not for the thread of GPUI.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() || name.contains(['\\', '/', ':', '"']) {
        return None;
    }
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| DEFAULT_EXTENSIONS.to_owned());
    let tried = extensions_to_try(name, &pathext);
    let registered = { [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter() }.find_map(|root| {
        tried
            .iter()
            .find_map(|extension| registered_path(root, &format!("{name}{extension}")))
    });
    registered.or_else(|| {
        tried
            .iter()
            .find_map(|extension| searched_path(&format!("{name}{extension}")))
    })
}

/// The extensions to try `name` with, the empty one standing for the
/// name as it is: that alone where it has an extension of `pathext`
/// already, or else each of `pathext` in its order, and the name as it
/// is last, as a document may be named without its program.
fn extensions_to_try(name: &str, pathext: &str) -> Vec<String> {
    let extensions: Vec<String> = { pathext.split(';') }
        .map(str::trim)
        .filter(|extension| extension.starts_with('.') && extension.len() > 1)
        .map(str::to_ascii_lowercase)
        .collect();
    let lower = name.to_ascii_lowercase();
    if extensions
        .iter()
        .any(|extension| lower.ends_with(extension))
    {
        return vec![String::new()];
    }
    let mut tried = extensions;
    tried.push(String::new());
    tried
}

/// The program the App Paths key `file` of `root` names, if it is
/// there.
fn registered_path(root: HKEY, file: &str) -> Option<PathBuf> {
    let key: Vec<u16> = format!(r"{APP_PATHS}\{file}")
        .encode_utf16()
        .chain([0])
        .collect();
    let mut text = vec![0u16; PATH_LENGTH];
    let mut size = (text.len() * size_of::<u16>()) as u32;
    // SAFETY: `size` is the size in bytes of `text`, which outlives the
    // call, and the key is NUL-terminated; the default value is read and
    // expanded where it is of the expanding kind.
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR::null(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    status.ok().ok()?;
    let length = text.iter().position(|&unit| unit == 0)?;
    let path = String::from_utf16_lossy(&text[..length]);
    let path = path.trim().trim_matches('"');
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// The first file named `file` in the folders Windows searches for a
/// program: its own, the folder of Tessera, the current one and those of
/// `PATH`.
fn searched_path(file: &str) -> Option<PathBuf> {
    let name: Vec<u16> = file.encode_utf16().chain([0]).collect();
    let mut found = vec![0u16; PATH_LENGTH];
    // SAFETY: the name is NUL-terminated; the buffer is passed with its
    // length and outlives the call.
    let length = unsafe {
        SearchPathW(
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            PCWSTR::null(),
            Some(&mut found),
            None,
        )
    } as usize;
    (length > 0 && length < found.len())
        .then(|| PathBuf::from(String::from_utf16_lossy(&found[..length])))
}

/// `text` with every `%NAME%` replaced by the environment variable of
/// that name, as the Run dialog replaces them; a variable that is not
/// set stays as it is written.
pub fn expand_variables(text: &str) -> String {
    expand_environment(OsStr::new(text))
        .to_string_lossy()
        .into_owned()
}

/// The folder Windows keeps a shortcut in for each document the user
/// opened lately (`shell:recent`).
pub fn recent_folder() -> Option<PathBuf> {
    // SAFETY: the identifier outlives the call; the text Windows
    // allocates is copied and then freed exactly once.
    unsafe {
        let text = SHGetKnownFolderPath(&FOLDERID_Recent, KF_FLAG_DEFAULT, None).ok()?;
        let path = text.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(text.0.cast()));
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_tried_with_the_extensions_of_programs() {
        let pathext = ".COM;.EXE;.BAT;;CMD;.CMD";
        assert_eq!(
            extensions_to_try("notepad", pathext),
            [".com", ".exe", ".bat", ".cmd", ""]
        );
        assert_eq!(extensions_to_try("Setup.EXE", pathext), [""]);
        assert_eq!(
            extensions_to_try("notes.txt", pathext),
            [".com", ".exe", ".bat", ".cmd", ""]
        );
    }

    #[test]
    fn a_name_with_a_folder_is_no_program_to_look_for() {
        assert_eq!(find_program(r"C:\Windows\notepad.exe"), None);
        assert_eq!(find_program("tools/run"), None);
        assert_eq!(find_program("  "), None);
    }
}
