// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The shortcuts of the Start menu.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr;

use anyhow::Result;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{Interface, PCWSTR};

use super::execute::{Launch, expand_environment};

/// Where the Start menu keeps shortcuts, below a profile directory.
const PROGRAMS: &str = r"Microsoft\Windows\Start Menu\Programs";

/// The longest path a shortcut reports.
const MAX_PATH: usize = 260;

/// Something that starts an application: a shortcut file, a program file,
/// or the path that names a packaged application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    /// The name the Start menu shows.
    pub name: String,
    pub path: PathBuf,
}

/// What a shortcut points to.
#[derive(Clone, Debug, Default)]
pub struct ShortcutLink {
    /// The file it opens, when it names one.
    pub target: Option<PathBuf>,
    /// The file and the index within it of the icon it asks for, if any.
    pub icon: Option<(PathBuf, i32)>,
}

impl Shortcut {
    /// Everything the Start menu starts, by name: the shortcuts in its
    /// folders of the user and of the machine, and the packaged (Store)
    /// applications, which have no shortcut file. Of several with one
    /// name, the user's shortcut is kept, or else the machine's.
    pub fn start_menu() -> Vec<Self> {
        let mut shortcuts = Vec::new();
        for profile in ["APPDATA", "PROGRAMDATA"] {
            if let Some(profile) = env::var_os(profile) {
                Self::collect(&Path::new(&profile).join(PROGRAMS), &mut shortcuts);
            }
        }
        shortcuts.extend(Self::packaged());
        let mut seen = HashSet::new();
        shortcuts.retain(|shortcut| seen.insert(shortcut.name.to_lowercase()));
        shortcuts.sort_by_cached_key(|shortcut| shortcut.name.to_lowercase());
        shortcuts
    }

    /// Adds the shortcuts in `directory` and below it. Directories that
    /// cannot be read are skipped.
    fn collect(directory: &Path, shortcuts: &mut Vec<Self>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                Self::collect(&path, shortcuts);
                continue;
            }
            let is_shortcut = path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"));
            if let Some(name) = path.file_stem().and_then(|name| name.to_str())
                && is_shortcut
            {
                shortcuts.push(Self {
                    name: name.to_owned(),
                    path,
                });
            }
        }
    }

    /// Opens the shortcut the way a double click would, with [`Launch`]
    /// on the shell thread. A failure is reported.
    pub fn launch(&self) {
        Launch::new(&self.path)
            .verb("open")
            .named(format!("`{}`", self.name))
            .start();
    }

    /// Reads what the shortcut points to; empty when it cannot be read.
    pub fn link(&self) -> ShortcutLink {
        self.read_link().unwrap_or_default()
    }

    fn read_link(&self) -> Result<ShortcutLink> {
        let path = wide(&self.path);
        let mut target = [0u16; MAX_PATH];
        let mut icon = [0u16; MAX_PATH];
        let mut icon_index = 0;
        // SAFETY: GPUI initialized COM on this thread; the file name is
        // NUL-terminated, and each buffer is passed with its length.
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.cast::<IPersistFile>()?
                .Load(PCWSTR(path.as_ptr()), STGM_READ)?;
            // Either may be missing; the buffer then stays empty.
            let _ = link.GetPath(&mut target, ptr::null_mut(), 0);
            let _ = link.GetIconLocation(&mut icon, &raw mut icon_index);
        }
        Ok(ShortcutLink {
            target: path_in(&target),
            icon: path_in(&icon).map(|file| (expand(&file), icon_index)),
        })
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// The path in a NUL-terminated buffer, unless it is empty.
fn path_in(buffer: &[u16]) -> Option<PathBuf> {
    let length = buffer.iter().position(|&unit| unit == 0)?;
    (length > 0).then(|| std::ffi::OsString::from_wide(&buffer[..length]).into())
}

/// Replaces `%NAME%` references to environment variables in `path`.
fn expand(path: &Path) -> PathBuf {
    expand_environment(path.as_os_str()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_are_found_in_nested_folders() {
        let root = env::temp_dir().join(format!("tessera-shortcuts-{}", std::process::id()));
        fs::create_dir_all(root.join("Tools")).unwrap();
        fs::write(root.join("Editor.lnk"), []).unwrap();
        fs::write(root.join("Tools").join("Viewer.LNK"), []).unwrap();
        fs::write(root.join("desktop.ini"), []).unwrap();

        let mut shortcuts = Vec::new();
        Shortcut::collect(&root, &mut shortcuts);
        fs::remove_dir_all(&root).unwrap();

        let mut names: Vec<_> = shortcuts.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["Editor", "Viewer"]);
    }

    #[test]
    fn an_empty_buffer_holds_no_path() {
        assert_eq!(path_in(&[0, 0]), None);
        assert_eq!(path_in(&[b'a' as u16, 0]), Some(PathBuf::from("a")));
    }
}
