// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the user chose in the settings of Windows, under Taskbar > Other
//! system tray icons: which icons stand in the taskbar and which only in
//! its overflow.
//!
//! Explorer keeps one key for every icon it has seen under
//! `HKCU\Control Panel\NotifyIconSettings`, named by a number. The key
//! holds the program of the icon (`ExecutablePath`, which may start with
//! the identifier of a known folder instead of its path), the number the
//! program gave the icon (`UID`), and `IsPromoted`, 1 for an icon shown in
//! the taskbar and 0 for one in the overflow; a key without `IsPromoted`
//! is of an icon the user never moved. Icons with a global identifier
//! have it in `IconGuid`, as far as is known; that name is not documented
//! and was not checked against a machine.

use std::path::PathBuf;
use std::sync::Arc;

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SAM_FLAGS};
use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::core::GUID;

use super::super::registry::{Key, Value};

/// Where Explorer keeps what it knows of the icons.
const SETTINGS: &str = "Control Panel\\NotifyIconSettings";

/// One icon as the settings of Windows know it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayPromotion {
    /// The program of the icon, with a known folder at its start resolved.
    pub executable: Option<Arc<PathBuf>>,
    /// The number the program gave the icon, if the key says.
    pub uid: Option<u32>,
    /// The global identifier of the icon, if it has one.
    pub guid: Option<[u8; 16]>,
    /// Whether the user put it in the taskbar; `None` where the user never
    /// chose.
    pub promoted: Option<bool>,
}

/// Every icon Explorer recorded, in the order of the registry. Empty where
/// Explorer never ran for this user.
pub(super) fn read() -> Vec<TrayPromotion> {
    let Some(settings) = Key::read(HKEY_CURRENT_USER, SETTINGS, REG_SAM_FLAGS(0)) else {
        return Vec::new();
    };
    settings
        .subkeys()
        .iter()
        .filter_map(|name| settings.child(name))
        .map(|key| TrayPromotion {
            executable: key
                .text("ExecutablePath")
                .map(|path| resolved(&path, known_folder))
                .filter(|path| !path.as_os_str().is_empty())
                .map(Arc::new),
            uid: key.number("UID"),
            guid: key.value("IconGuid").and_then(guid_of_value),
            promoted: key.number("IsPromoted").map(|promoted| promoted != 0),
        })
        .collect()
}

/// The global identifier a value holds, as text or as its 16 bytes.
fn guid_of_value(value: Value) -> Option<[u8; 16]> {
    match value {
        Value::Text(text) => guid_bytes(&text),
        Value::Bytes(bytes) => bytes.get(..16)?.try_into().ok(),
        Value::Number(_) => None,
    }
}

/// The identifier written as `{01234567-89AB-CDEF-0123-456789ABCDEF}`.
fn parse_guid(text: &str) -> Option<GUID> {
    let inner = text.trim().strip_prefix('{')?.strip_suffix('}')?;
    let groups: Vec<&str> = inner.split('-').collect();
    let lengths = [8, 4, 4, 4, 12];
    let fits = |(group, length): (&&str, usize)| {
        group.len() == length && group.chars().all(|c| c.is_ascii_hexdigit())
    };
    if groups.len() != lengths.len() || !groups.iter().zip(lengths).all(fits) {
        return None;
    }
    u128::from_str_radix(&groups.concat(), 16)
        .ok()
        .map(GUID::from_u128)
}

/// The bytes of such an identifier, laid out as Windows lays out a `GUID`:
/// the first three groups in little-endian order.
fn guid_bytes(text: &str) -> Option<[u8; 16]> {
    parse_guid(text).map(bytes_of)
}

fn bytes_of(guid: GUID) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&guid.data1.to_le_bytes());
    bytes[4..6].copy_from_slice(&guid.data2.to_le_bytes());
    bytes[6..8].copy_from_slice(&guid.data3.to_le_bytes());
    bytes[8..].copy_from_slice(&guid.data4);
    bytes
}

/// `path` with the known folder whose identifier starts it replaced by
/// where `resolve` says that folder is. A path that starts otherwise, or
/// with a folder `resolve` does not know, stays as it is.
fn resolved(path: &str, resolve: impl Fn(&GUID) -> Option<PathBuf>) -> PathBuf {
    let Some(end) = path.starts_with('{').then(|| path.find('}')).flatten() else {
        return PathBuf::from(path);
    };
    match parse_guid(&path[..=end]).as_ref().and_then(resolve) {
        Some(folder) => folder.join(path[end + 1..].trim_start_matches('\\')),
        None => PathBuf::from(path),
    }
}

/// Where the known folder `id` is.
fn known_folder(id: &GUID) -> Option<PathBuf> {
    // SAFETY: the identifier outlives the call; the text Windows allocates
    // is copied and then freed exactly once.
    unsafe {
        let text = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let path = text.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(text.0.cast()));
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRAM_FILES: &str = "{6D809377-6AF0-444B-8957-A3773F02200E}";

    #[test]
    fn an_identifier_is_read_as_windows_lays_it_out() {
        let bytes = guid_bytes(PROGRAM_FILES).expect("an identifier");
        assert_eq!(&bytes[..4], &[0x77, 0x93, 0x80, 0x6d]);
        assert_eq!(&bytes[4..8], &[0xf0, 0x6a, 0x4b, 0x44]);
        assert_eq!(
            &bytes[8..],
            &[0x89, 0x57, 0xa3, 0x77, 0x3f, 0x02, 0x20, 0x0e]
        );
        assert_eq!(
            parse_guid(PROGRAM_FILES),
            Some(GUID::from_u128(0x6d809377_6af0_444b_8957_a3773f02200e))
        );
        assert_eq!(guid_bytes("6D809377-6AF0-444B-8957-A3773F02200E"), None);
        assert_eq!(guid_bytes("{6D809377-6AF0-444B-8957}"), None);
        assert_eq!(guid_bytes("{XD809377-6AF0-444B-8957-A3773F02200E}"), None);
    }

    #[test]
    fn a_known_folder_at_the_start_of_a_path_is_resolved() {
        let resolve = |id: &GUID| {
            (*id == GUID::from_u128(0x6d809377_6af0_444b_8957_a3773f02200e))
                .then(|| PathBuf::from("C:\\Program Files"))
        };
        assert_eq!(
            resolved(&format!("{PROGRAM_FILES}\\App\\app.exe"), resolve),
            PathBuf::from("C:\\Program Files").join("App\\app.exe")
        );
        assert_eq!(
            resolved("C:\\Tools\\tool.exe", resolve),
            PathBuf::from("C:\\Tools\\tool.exe")
        );
        let unknown = "{00000000-0000-0000-0000-000000000001}\\x.exe";
        assert_eq!(resolved(unknown, resolve), PathBuf::from(unknown));
    }

    #[test]
    fn an_identifier_is_taken_from_text_or_bytes() {
        assert_eq!(
            guid_of_value(Value::Bytes((0..16).collect())),
            Some(std::array::from_fn(|index| index as u8))
        );
        assert_eq!(guid_of_value(Value::Bytes(vec![1, 2])), None);
        assert!(guid_of_value(Value::Text(PROGRAM_FILES.to_owned())).is_some());
        assert_eq!(guid_of_value(Value::Number(1)), None);
    }
}
