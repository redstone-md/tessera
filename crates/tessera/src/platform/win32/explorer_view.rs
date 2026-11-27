// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How Explorer shows the files of a folder, so that the file manager of
//! Tessera shows them alike: the folder options of the user (extensions
//! of known types, hidden files, protected files of the system), the
//! names a `desktop.ini` gives in the language of the user ("Документы"
//! for Documents on Russian Windows), the extensions Windows never shows
//! (`.lnk`, `.url`), and the arrow it draws over a shortcut.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::fs::Metadata;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, KEY_READ, RRF_RT_ANY, RRF_RT_REG_DWORD,
    RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::UI::Shell::{
    IShellItem, SHCreateItemFromParsingName, SHGSI_ICON, SHGSI_SMALLICON, SHGetStockIconInfo,
    SHSTOCKICONINFO, SIGDN_NORMALDISPLAY, SIID_LINK,
};
use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;
use windows::core::{HSTRING, PCWSTR, PWSTR};

use super::IconPixels;

/// The attributes of a file that is read-only, hidden, or of the system.
const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

/// Where Explorer keeps the folder options of the user.
const ADVANCED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";

/// The file that gives a folder its look and, in the language of the
/// user, its name and the names of its files.
const DESKTOP_INI: &str = "desktop.ini";

/// The longest text read from the registry, in UTF-16 units.
const TEXT_LENGTH: usize = 512;

/// The folder options of Explorer that change what a folder shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExplorerSettings {
    /// "Hide extensions for known file types".
    pub hide_extensions: bool,
    /// "Show hidden files, folders and drives".
    pub show_hidden: bool,
    /// The opposite of "Hide protected operating system files".
    pub show_protected: bool,
}

impl Default for ExplorerSettings {
    /// What Windows does where the user changed nothing.
    fn default() -> Self {
        Self {
            hide_extensions: true,
            show_hidden: false,
            show_protected: false,
        }
    }
}

impl ExplorerSettings {
    /// The options as the user set them, or the defaults of Windows for
    /// those never set.
    pub fn read() -> Self {
        let defaults = Self::default();
        let advanced = HSTRING::from(ADVANCED);
        let value = |name: &str| dword(HKEY_CURRENT_USER, &advanced, name);
        Self {
            hide_extensions: value("HideFileExt").map_or(defaults.hide_extensions, |v| v != 0),
            // 1 shows hidden files, 2 does not.
            show_hidden: value("Hidden").map_or(defaults.show_hidden, |v| v == 1),
            show_protected: value("ShowSuperHidden").map_or(defaults.show_protected, |v| v != 0),
        }
    }
}

/// What the attributes of a file say about how it is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attributes {
    pub hidden: bool,
    pub system: bool,
    pub readonly: bool,
}

impl Attributes {
    pub fn of(metadata: &Metadata) -> Self {
        let bits = metadata.file_attributes();
        Self {
            hidden: bits & FILE_ATTRIBUTE_HIDDEN != 0,
            system: bits & FILE_ATTRIBUTE_SYSTEM != 0,
            readonly: bits & FILE_ATTRIBUTE_READONLY != 0,
        }
    }

    /// Whether Explorer counts the file among the protected files of the
    /// system, which it hides even where hidden files are shown.
    pub fn protected(self) -> bool {
        self.hidden && self.system
    }

    /// Whether a folder with these attributes reads its `desktop.ini`,
    /// which Explorer does only for one marked read-only or of the system.
    pub fn customized(self) -> bool {
        self.readonly || self.system
    }
}

/// What decides how the name of a file is shown: whether the extension
/// of its type is, which the registry tells and which is remembered for
/// every extension asked about.
#[derive(Debug)]
pub struct NameRules {
    /// Whether extensions of known types are hidden.
    hide_known: bool,
    /// Whether the rules of Explorer are followed at all; without them
    /// every name is shown as it is.
    follow: bool,
    /// Whether the extension is hidden, by extension in small letters.
    hidden: HashMap<String, bool>,
}

impl NameRules {
    /// The rules of `settings`, or with `follow` false the names as they
    /// are.
    pub fn new(settings: ExplorerSettings, follow: bool) -> Self {
        Self {
            hide_known: settings.hide_extensions,
            follow,
            hidden: HashMap::new(),
        }
    }

    pub fn follows(&self) -> bool {
        self.follow
    }

    /// The name of the file called `name` as Explorer shows it: without
    /// its extension where that is hidden. A folder keeps its name.
    pub fn shown(&mut self, name: &str, folder: bool) -> String {
        if !self.follow || folder {
            return name.to_owned();
        }
        let Some((stem, extension)) = split_extension(name) else {
            return name.to_owned();
        };
        let extension = extension.to_lowercase();
        let hide_known = self.hide_known;
        let hidden = *self
            .hidden
            .entry(extension)
            .or_insert_with_key(|extension| extension_hidden(extension, hide_known));
        match hidden {
            true => stem.to_owned(),
            false => name.to_owned(),
        }
    }
}

/// The stem and the extension of `name`, where it has both: `.gitignore`
/// has no extension, as for Explorer.
pub fn split_extension(name: &str) -> Option<(&str, &str)> {
    let (stem, extension) = name.rsplit_once('.')?;
    (!stem.is_empty() && !extension.is_empty()).then_some((stem, extension))
}

/// Whether Explorer hides the extension `extension`: always for a type
/// that says `NeverShowExt`, as shortcuts do, never for one that says
/// `AlwaysShowExt` or that Windows does not know, and for the others as
/// `hide_known` says.
fn extension_hidden(extension: &str, hide_known: bool) -> bool {
    let key = HSTRING::from(format!(".{extension}"));
    if !has_key(HKEY_CLASSES_ROOT, &key) {
        return false;
    }
    let class = text(HKEY_CLASSES_ROOT, &key, None).filter(|class| !class.is_empty());
    let keys = std::iter::once(key).chain(class.as_deref().map(HSTRING::from));
    let mut known = class.is_some();
    for key in keys {
        if has_value(HKEY_CLASSES_ROOT, &key, "NeverShowExt") {
            return true;
        }
        if has_value(HKEY_CLASSES_ROOT, &key, "AlwaysShowExt") {
            return false;
        }
        known |= has_key(
            HKEY_CLASSES_ROOT,
            &HSTRING::from(format!("{key}\\OpenWithProgids")),
        );
    }
    known && hide_known
}

/// Which of the things a folder holds have a name of their own in the
/// language of the user, given by a `desktop.ini`, and should be named by
/// the Shell: the folders customized with one, and the files the
/// `desktop.ini` of the folder itself names.
pub fn localized_candidates(
    folder: &Path,
    items: impl IntoIterator<Item = (PathBuf, bool, Attributes)>,
) -> Vec<PathBuf> {
    let listed = std::fs::read(folder.join(DESKTOP_INI))
        .ok()
        .map(|bytes| ini_text(&bytes).to_lowercase())
        .filter(|text| text.contains("[localizedfilenames]"));
    items
        .into_iter()
        .filter(|(path, is_folder, attributes)| match is_folder {
            true => attributes.customized() && path.join(DESKTOP_INI).is_file(),
            false => listed.as_ref().is_some_and(|text| {
                { path.file_name() }
                    .is_some_and(|name| text.contains(&name.to_string_lossy().to_lowercase()))
            }),
        })
        .map(|(path, ..)| path)
        .collect()
}

/// The text of a `desktop.ini`, which Windows writes in UTF-16 with a
/// mark of its order, or else in the code page of the system, of which
/// the letters of the names of sections are the same as ASCII.
fn ini_text(bytes: &[u8]) -> String {
    match bytes {
        [0xff, 0xfe, rest @ ..] => {
            let units: Vec<u16> = { rest.chunks_exact(2) }
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// The names the Shell gives the files and folders at `paths`, as
/// Explorer shows them, one for each path; `None` where it gives none.
/// Blocks on the disk; it is asked away from the user interface.
pub fn shell_names(paths: &[PathBuf]) -> Vec<Option<String>> {
    if paths.is_empty() {
        return Vec::new();
    }
    // SAFETY: COM is initialized for the calls on this thread, and left
    // as it was where it was initialized already; every name the Shell
    // allocates is copied and freed exactly once.
    unsafe {
        let initialized =
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
        let names = { paths.iter() }
            .map(|path| {
                let path = HSTRING::from(path.as_os_str());
                let item: IShellItem = SHCreateItemFromParsingName(&path, None).ok()?;
                let name = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
                let text = name.to_string().ok();
                CoTaskMemFree(Some(name.0.cast()));
                text.filter(|text| !text.is_empty())
            })
            .collect();
        if initialized {
            CoUninitialize();
        }
        names
    }
}

impl IconPixels {
    /// The arrow Windows draws over the icon of a shortcut, small, as the
    /// user may have replaced it.
    pub fn link_arrow() -> Option<Self> {
        let mut info = SHSTOCKICONINFO {
            cbSize: size_of::<SHSTOCKICONINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: the structure carries its size and outlives the call;
        // the icon it gives is destroyed once its pixels are copied.
        unsafe {
            SHGetStockIconInfo(SIID_LINK, SHGSI_ICON | SHGSI_SMALLICON, &raw mut info).ok()?;
            let pixels = Self::of_icon(info.hIcon);
            let _ = DestroyIcon(info.hIcon);
            pixels
        }
    }
}

/// The number `name` of the key `key` under `root`.
pub(super) fn dword(root: HKEY, key: &HSTRING, name: &str) -> Option<u32> {
    let name = HSTRING::from(name);
    let mut value = 0_u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the out-parameters outlive the call, and `size` is the size
    // of what `value` points to.
    let status = unsafe {
        RegGetValueW(
            root,
            key,
            &name,
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

/// The text `name` of the key `key` under `root`; with no name, the
/// default value of the key.
pub(super) fn text(root: HKEY, key: &HSTRING, name: Option<&str>) -> Option<String> {
    let name = name.map(HSTRING::from);
    let name = name
        .as_ref()
        .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr()));
    let mut buffer = [0_u16; TEXT_LENGTH];
    let mut size = size_of_val(&buffer) as u32;
    // SAFETY: `size` is the size in bytes of `buffer`, and both outlive
    // the call; the names are NUL-terminated or null.
    let status = unsafe {
        RegGetValueW(
            root,
            key,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let length = buffer.iter().position(|&unit| unit == 0)?;
    Some(String::from_utf16_lossy(&buffer[..length]))
}

/// Whether the key `key` under `root` has a value `name`, of any type.
fn has_value(root: HKEY, key: &HSTRING, name: &str) -> bool {
    let name = HSTRING::from(name);
    // SAFETY: no data is asked for; the names are NUL-terminated.
    let status = unsafe { RegGetValueW(root, key, &name, RRF_RT_ANY, None, None, None) };
    status == ERROR_SUCCESS
}

/// Whether there is a key `key` under `root`.
fn has_key(root: HKEY, key: &HSTRING) -> bool {
    let mut opened = HKEY::default();
    // SAFETY: the name is NUL-terminated, and a key opened is closed.
    unsafe {
        let status = RegOpenKeyExW(root, key, None, KEY_READ, &raw mut opened);
        if status != ERROR_SUCCESS {
            return false;
        }
        let _ = RegCloseKey(opened);
    }
    true
}

/// The names of the keys right under the key `key` of `root`.
pub(super) fn subkeys(root: HKEY, key: &HSTRING) -> Vec<String> {
    let mut opened = HKEY::default();
    let mut names = Vec::new();
    // SAFETY: the name is NUL-terminated; each name is read into a buffer
    // whose length is given; the key opened is closed.
    unsafe {
        if RegOpenKeyExW(root, key, None, KEY_READ, &raw mut opened) != ERROR_SUCCESS {
            return names;
        }
        for index in 0.. {
            let mut name = [0_u16; 256];
            let mut length = name.len() as u32;
            let status = RegEnumKeyExW(
                opened,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &raw mut length,
                None,
                None,
                None,
                None,
            );
            if status != ERROR_SUCCESS {
                break;
            }
            names.push(String::from_utf16_lossy(&name[..length as usize]));
        }
        let _ = RegCloseKey(opened);
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_has_an_extension_after_its_last_dot() {
        assert_eq!(
            split_extension("report.final.pdf"),
            Some(("report.final", "pdf"))
        );
        assert_eq!(split_extension(".gitignore"), None);
        assert_eq!(split_extension("trailing."), None);
        assert_eq!(split_extension("plain"), None);
    }

    #[test]
    fn without_the_rules_of_explorer_a_name_is_as_it_is() {
        let mut rules = NameRules::new(ExplorerSettings::default(), false);
        assert_eq!(rules.shown("Shortcut.lnk", false), "Shortcut.lnk");
        let mut rules = NameRules::new(ExplorerSettings::default(), true);
        assert_eq!(rules.shown("Folder.with.dots", true), "Folder.with.dots");
    }

    #[test]
    fn a_desktop_ini_is_read_in_either_encoding() {
        let wide: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain(
                "[.ShellClassInfo]"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes),
            )
            .collect();
        assert_eq!(ini_text(&wide), "[.ShellClassInfo]");
        assert_eq!(ini_text(b"[LocalizedFileNames]"), "[LocalizedFileNames]");
    }

    #[test]
    fn protected_files_are_hidden_files_of_the_system() {
        let hidden = Attributes {
            hidden: true,
            ..Attributes::default()
        };
        assert!(!hidden.protected());
        assert!(
            Attributes {
                system: true,
                ..hidden
            }
            .protected()
        );
        assert!(
            Attributes {
                readonly: true,
                ..Attributes::default()
            }
            .customized()
        );
    }

    #[test]
    #[ignore = "reads the registry of the machine it runs on"]
    fn shortcuts_never_show_their_extension() {
        let mut rules = NameRules::new(ExplorerSettings::read(), true);
        assert_eq!(rules.shown("Notes.lnk", false), "Notes");
        assert_eq!(rules.shown("unknown.zzzzqq", false), "unknown.zzzzqq");
    }
}
