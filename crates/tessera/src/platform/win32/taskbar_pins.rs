// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the dock takes from Windows for its pins: the shortcuts pinned to
//! the taskbar of Windows, in the order the taskbar shows them where that
//! can be read, and the program that opens the links of a scheme, whose
//! icon a pinned web address shows.

#![allow(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SAM_FLAGS};
use windows::Win32::UI::Shell::{
    ASSOCF_IS_PROTOCOL, ASSOCF_NOTRUNCATE, ASSOCSTR_EXECUTABLE, AssocQueryStringW,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

use super::registry::Key;

/// The folder below the roaming profile where the taskbar of Windows
/// keeps a shortcut for each of its pins.
const PINNED_FOLDER: &str = r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar";

/// The key and value where Explorer keeps the order of its pins, in a
/// layout of its own that is not documented: a list of the identifier
/// lists of the shortcuts.
const TASKBAND: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Taskband";
const FAVORITES: &str = "Favorites";

/// The shortcuts pinned to the taskbar of Windows, in its order where the
/// value it keeps the order in names them, and the others after them by
/// name. Reads the disk and the registry: not for the thread of GPUI.
pub fn windows_taskbar_pins() -> Vec<PathBuf> {
    let Some(folder) = std::env::var_os("APPDATA").map(|data| Path::new(&data).join(PINNED_FOLDER))
    else {
        return Vec::new();
    };
    let Ok(listed) = fs::read_dir(&folder) else {
        return Vec::new();
    };
    let mut links: Vec<PathBuf> = { listed.filter_map(Result::ok) }
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        })
        .collect();
    links.sort_by_cached_key(|path| path.to_string_lossy().to_lowercase());
    let order = Key::read(HKEY_CURRENT_USER, TASKBAND, REG_SAM_FLAGS(0))
        .and_then(|key| key.bytes(FAVORITES))
        .unwrap_or_default();
    let names: Vec<String> = { links.iter() }
        .map(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    by_appearance(&order, &names)
        .into_iter()
        .map(|index| links[index].clone())
        .collect()
}

/// The indices of `names` in the order they first appear in `bytes`, as
/// UTF-16 at either alignment and whatever their case; those that do not
/// appear come after the others in their own order.
pub(super) fn by_appearance(bytes: &[u8], names: &[String]) -> Vec<usize> {
    let lower = |unit: u16| match unit {
        0x41..=0x5a => unit + 0x20,
        _ => unit,
    };
    let units = |shift: usize| -> Vec<u16> {
        { bytes.get(shift..).unwrap_or_default().chunks_exact(2) }
            .map(|pair| lower(u16::from_le_bytes([pair[0], pair[1]])))
            .collect()
    };
    let aligned = [units(0), units(1)];
    let found = |name: &str| -> Option<usize> {
        let wanted: Vec<u16> = name.encode_utf16().map(lower).collect();
        if wanted.is_empty() {
            return None;
        }
        let place = |(shift, units): (usize, &Vec<u16>)| {
            let at = units
                .windows(wanted.len())
                .position(|window| window == wanted)?;
            Some(at * 2 + shift)
        };
        aligned.iter().enumerate().filter_map(place).min()
    };
    let mut indices: Vec<(Option<usize>, usize)> = { names.iter().enumerate() }
        .map(|(index, name)| (found(name), index))
        .collect();
    indices.sort_by_key(|&(at, index)| (at.is_none(), at, index));
    indices.into_iter().map(|(_, index)| index).collect()
}

/// The program that opens the links of `scheme` (`https`, `mailto`), as
/// Windows chooses it for the user. Reads the registry.
pub fn scheme_handler(scheme: &str) -> Option<PathBuf> {
    let scheme = HSTRING::from(scheme);
    let mut text = vec![0u16; 1024];
    let mut length = text.len() as u32;
    // SAFETY: the buffer holds `length` characters and outlives the call;
    // the scheme is NUL-terminated and no extra text is given.
    unsafe {
        AssocQueryStringW(
            ASSOCF_IS_PROTOCOL | ASSOCF_NOTRUNCATE,
            ASSOCSTR_EXECUTABLE,
            &scheme,
            PCWSTR::null(),
            Some(PWSTR(text.as_mut_ptr())),
            &raw mut length,
        )
        .ok()
        .ok()?;
    }
    let end = text
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(text.len());
    let path = PathBuf::from(String::from_utf16_lossy(&text[..end]));
    path.is_absolute().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn the_pins_follow_the_order_the_taskbar_keeps() {
        let mut bytes = vec![7u8; 5];
        bytes.extend(utf16("Mail.lnk"));
        // An odd place, as items of an identifier list may lie.
        bytes.push(0);
        bytes.extend(utf16("FILE EXPLORER.LNK"));
        let names = [
            "Browser.lnk".to_owned(),
            "File Explorer.lnk".to_owned(),
            "Mail.lnk".to_owned(),
            "Archive.lnk".to_owned(),
        ];
        assert_eq!(by_appearance(&bytes, &names), [2, 1, 0, 3]);
        assert_eq!(by_appearance(&[], &names), [0, 1, 2, 3]);
    }
}
