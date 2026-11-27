// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Tessera Files as the program that opens folders, and what Explorer's
//! command lines name.
//!
//! [`FolderHandler`] writes, for the user only, the verbs `open`,
//! `explore` and `opennewwindow` of the classes `Directory` and `Drive`
//! under `HKEY_CURRENT_USER\Software\Classes`, each with the command
//! `"<tessera.exe>" files "%1"` and without the `DelegateExecute` that
//! hands the verbs of the machine to Explorer. What the user had there
//! before is copied under `HKEY_CURRENT_USER\Software\Tessera\FolderHandler`
//! first and put back by [`FolderHandler::revert`].
//!
//! The class `Folder` is left alone on purpose: it is the class of every
//! folder of the Shell, virtual ones too (Control Panel, This PC, the
//! Recycle Bin, libraries, `shell:::{CLSID}` items), and those stay with
//! Explorer. `Directory` and `Drive` are the folders and drives of the
//! file system only, and come before `Folder` when the Shell looks for a
//! verb of such an item. Nothing here touches Image File Execution
//! Options or `explorer.exe` itself.
//!
//! [`known_folder_path`] reads the folder a `shell:` name stands for,
//! [`start_explorer`] gives a command line to Explorer as it is, and
//! [`tell_problem`] says in a box of Windows what went wrong where no
//! window of Tessera can.
#![allow(unsafe_code)]

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_SAM_FLAGS, RegCopyTreeW,
    RegDeleteKeyW, RegDeleteTreeW,
};
use windows::Win32::UI::Shell::{
    IKnownFolderManager, KnownFolderManager, SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify,
};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW};
use windows::core::{HSTRING, PCWSTR};

use super::registry::{Key, has_key, registry_text};
use super::winlogon::explorer_program;

/// The classes whose verbs are written: the folders and the drives of the
/// file system.
const CLASSES: [&str; 2] = ["Directory", "Drive"];

/// The verbs written for each class.
const VERBS: [&str; 3] = ["open", "explore", "opennewwindow"];

/// Where the verbs of the machine are copied from, for their labels and
/// where they show (`MUIVerb`, `ProgrammaticAccessOnly`,
/// `OnlyInBrowserWindow`).
const MACHINE_FOLDER: &str = r"Software\Classes\Folder\shell";

/// The values of a verb of Explorer that only Explorer reads; they are
/// not copied.
const EXPLORER_VALUES: [&str; 4] = [
    "DelegateExecute",
    "LaunchExplorerFlags",
    "ExplorerFlags",
    "BrowserFlags",
];

/// Where what the user had is kept until it is put back.
const BACKUP: &str = r"Software\Tessera\FolderHandler";

/// The value of a backup that says whether the key was there before.
const EXISTED: &str = "Existed";

/// The key of a backup under which the tree that was there is copied.
const SAVED: &str = "Saved";

/// The environment variable a Tessera sets when it hands a folder to
/// Explorer because no Tessera answered; a Tessera started with it hands
/// nothing on again.
pub const FILES_FALLBACK: &str = "TESSERA_FILES_FALLBACK";

/// How long a command line value is read, in UTF-16 units.
const VALUE_LENGTH: usize = 2048;

/// What one verb of one class resolves to now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderVerb {
    pub class: &'static str,
    pub verb: &'static str,
    /// The command `HKEY_CLASSES_ROOT` gives, where there is one.
    pub command: Option<String>,
    /// The `DelegateExecute` it gives, which goes before the command.
    pub delegate: Option<String>,
}

impl FolderVerb {
    /// Whether the verb starts this Tessera's file manager, `program`
    /// being the file name of Tessera.
    pub fn is_tessera(&self, program: &str) -> bool {
        self.delegate.is_none()
            && self.command.as_deref().is_some_and(|command| {
                let lower = command.to_ascii_lowercase();
                lower.contains(&program.to_ascii_lowercase()) && lower.contains(" files ")
            })
    }
}

/// Whether Tessera Files opens the folders of the file system, as read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderHandler {
    /// Each verb of each class, as `HKEY_CLASSES_ROOT` gives it.
    pub verbs: Vec<FolderVerb>,
    /// Whether a backup is kept, so that `revert` has something to undo.
    pub installed: bool,
}

impl FolderHandler {
    /// What the registry says now.
    pub fn read() -> Self {
        let mut verbs = Vec::new();
        for class in CLASSES {
            for verb in VERBS {
                let command = format!(r"{class}\shell\{verb}\command");
                verbs.push(FolderVerb {
                    class,
                    verb,
                    command: registry_text(HKEY_CLASSES_ROOT, &command, None, VALUE_LENGTH),
                    delegate: registry_text(
                        HKEY_CLASSES_ROOT,
                        &command,
                        Some("DelegateExecute"),
                        VALUE_LENGTH,
                    ),
                });
            }
        }
        Self {
            verbs,
            installed: has_key(HKEY_CURRENT_USER, BACKUP),
        }
    }

    /// Whether every verb opens Tessera Files, `program` being the file
    /// name of Tessera.
    pub fn is_tessera(&self, program: &str) -> bool {
        { self.verbs.iter() }
            .filter(|verb| verb.command.is_some() || verb.delegate.is_some())
            .all(|verb| verb.is_tessera(program))
            && self.verbs.iter().any(|verb| verb.is_tessera(program))
    }

    /// Makes Tessera Files open the folders and drives of the file system
    /// for this user, keeping first what the user had. Gives what the
    /// registry says afterwards.
    pub fn install() -> Result<Self> {
        let program = std::env::current_exe().context("the Tessera program was not found")?;
        let command = files_command(&program.display().to_string());
        for class in CLASSES {
            keep_class(class)?;
            for verb in VERBS {
                // A verb the machine does not have for folders is not made,
                // so that no new entry shows in the menus.
                let machine = format!(r"{MACHINE_FOLDER}\{verb}");
                if !has_key(HKEY_LOCAL_MACHINE, &machine) && verb != "open" {
                    continue;
                }
                keep_verb(class, verb)?;
                write_verb(class, verb, &machine, &command)?;
            }
        }
        associations_changed();
        Ok(Self::read())
    }

    /// Puts back what the user had before [`Self::install`], and removes
    /// the backup. Without a backup nothing is changed.
    pub fn revert() -> Result<Self> {
        for class in CLASSES {
            for verb in VERBS {
                restore_verb(class, verb)?;
            }
            restore_class(class)?;
        }
        delete_tree(HKEY_CURRENT_USER, BACKUP)?;
        associations_changed();
        Ok(Self::read())
    }

    /// What `tessera files status` prints, `program` being the file name
    /// of Tessera.
    pub fn describe(&self, program: &str) -> Vec<String> {
        let mut lines = Vec::new();
        for verb in &self.verbs {
            let what = match (&verb.delegate, &verb.command) {
                _ if verb.is_tessera(program) => "Tessera Files".to_owned(),
                (Some(delegate), _) => format!("Explorer (DelegateExecute {delegate})"),
                (None, Some(command)) => format!("\"{command}\""),
                (None, None) => "nothing".to_owned(),
            };
            lines.push(format!("{}\\shell\\{}: {what}", verb.class, verb.verb));
        }
        lines.push(match self.is_tessera(program) {
            true => "Folders and drives of the file system open in Tessera Files.".to_owned(),
            false => "Folders and drives of the file system open in Explorer or another program."
                .to_owned(),
        });
        lines.push(
            "Virtual folders (Control Panel, This PC, the Recycle Bin, libraries, shell:::{CLSID}) always stay with Explorer."
                .to_owned(),
        );
        if self.installed {
            lines.push(
                "What was there before is kept; `tessera files revert` puts it back.".to_owned(),
            );
        }
        lines
    }
}

/// The command a verb runs: this program with `files` and the item.
fn files_command(program: &str) -> String {
    format!("\"{program}\" files \"%1\"")
}

/// The key of `verb` of `class` for the user.
fn verb_key(class: &str, verb: &str) -> String {
    format!(r"Software\Classes\{class}\shell\{verb}")
}

/// The backup of `verb` of `class`, or of the class itself without verb.
fn backup_key(class: &str, verb: Option<&str>) -> String {
    match verb {
        Some(verb) => format!(r"{BACKUP}\{class}\{verb}"),
        None => format!(r"{BACKUP}\{class}"),
    }
}

/// Notes, once, whether the user had the keys of `class` and of its
/// `shell`, so that keys Tessera made are removed again.
fn keep_class(class: &str) -> Result<()> {
    let backup = backup_key(class, None);
    if has_key(HKEY_CURRENT_USER, &backup) {
        return Ok(());
    }
    let (key, _) = create(HKEY_CURRENT_USER, &backup)?;
    let class_key = format!(r"Software\Classes\{class}");
    let shell_key = format!(r"{class_key}\shell");
    let flag = |there: bool| u32::from(there);
    key.set_number("ClassExisted", flag(has_key(HKEY_CURRENT_USER, &class_key)))
        .map_err(|failure| failure.into_error("the backup was not written"))?;
    key.set_number("ShellExisted", flag(has_key(HKEY_CURRENT_USER, &shell_key)))
        .map_err(|failure| failure.into_error("the backup was not written"))
}

/// Copies, once, what the user has under `verb` of `class` into the
/// backup.
fn keep_verb(class: &str, verb: &str) -> Result<()> {
    let backup = backup_key(class, Some(verb));
    if has_key(HKEY_CURRENT_USER, &backup) {
        return Ok(());
    }
    let user = verb_key(class, verb);
    let existed = has_key(HKEY_CURRENT_USER, &user);
    let (key, _) = create(HKEY_CURRENT_USER, &backup)?;
    if existed {
        let (saved, _) = create(HKEY_CURRENT_USER, &format!(r"{backup}\{SAVED}"))?;
        copy_tree(HKEY_CURRENT_USER, &user, saved.raw())?;
    }
    key.set_number(EXISTED, u32::from(existed))
        .map_err(|failure| failure.into_error("the backup was not written"))
}

/// Writes `verb` of `class` with `command`, its other values copied from
/// the verb of the machine at `machine` where it has one.
fn write_verb(class: &str, verb: &str, machine: &str, command: &str) -> Result<()> {
    let user = verb_key(class, verb);
    delete_tree(HKEY_CURRENT_USER, &user)?;
    let (key, _) = create(HKEY_CURRENT_USER, &user)?;
    if has_key(HKEY_LOCAL_MACHINE, machine) {
        copy_tree(HKEY_LOCAL_MACHINE, machine, key.raw())?;
        // Only the values of the verb itself are wanted.
        for child in key.subkeys() {
            delete_tree(key.raw(), &child)?;
        }
        for name in EXPLORER_VALUES {
            key.delete(name)
                .map_err(|failure| failure.into_error("a value of a verb was not removed"))?;
        }
    }
    let (run, _) = create(HKEY_CURRENT_USER, &format!(r"{user}\command"))?;
    run.set_text("", command)
        .map_err(|failure| failure.into_error("the command of a verb was not written"))
}

/// Puts back what the backup of `verb` of `class` holds, and removes it.
fn restore_verb(class: &str, verb: &str) -> Result<()> {
    let backup = backup_key(class, Some(verb));
    let Some(kept) = Key::read(HKEY_CURRENT_USER, &backup, REG_SAM_FLAGS(0)) else {
        return Ok(());
    };
    let user = verb_key(class, verb);
    delete_tree(HKEY_CURRENT_USER, &user)?;
    if kept.number(EXISTED) == Some(1) {
        let (key, _) = create(HKEY_CURRENT_USER, &user)?;
        let saved = format!(r"{backup}\{SAVED}");
        if has_key(HKEY_CURRENT_USER, &saved) {
            copy_tree(HKEY_CURRENT_USER, &saved, key.raw())?;
        }
    }
    drop(kept);
    delete_tree(HKEY_CURRENT_USER, &backup)
}

/// Removes the keys of `class` and of its `shell` that Tessera made, once
/// they are empty again.
fn restore_class(class: &str) -> Result<()> {
    let backup = backup_key(class, None);
    let Some(kept) = Key::read(HKEY_CURRENT_USER, &backup, REG_SAM_FLAGS(0)) else {
        return Ok(());
    };
    let class_key = format!(r"Software\Classes\{class}");
    if kept.number("ShellExisted") == Some(0) {
        remove_if_empty(&format!(r"{class_key}\shell"))?;
    }
    if kept.number("ClassExisted") == Some(0) {
        remove_if_empty(&class_key)?;
    }
    Ok(())
}

/// Removes the key `path` of the user where it holds no key and no value.
fn remove_if_empty(path: &str) -> Result<()> {
    let empty = match Key::read(HKEY_CURRENT_USER, path, REG_SAM_FLAGS(0)) {
        Some(key) => key.subkeys().is_empty() && key.values().is_empty(),
        None => return Ok(()),
    };
    if !empty {
        return Ok(());
    }
    let path = HSTRING::from(path);
    // SAFETY: the name is NUL-terminated and outlives the call.
    let status = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, &path) };
    checked(status, "an empty key of the registry was not removed")
}

/// Opens `path` under `root` to write it, making it where it is missing.
fn create(root: HKEY, path: &str) -> Result<(Key, bool)> {
    Key::create(root, path, REG_SAM_FLAGS(0), false)
        .map_err(|failure| failure.into_error(&format!("the key {path} was not made")))
}

/// Copies the tree `path` under `root`, its values and keys, into `into`.
fn copy_tree(root: HKEY, path: &str, into: HKEY) -> Result<()> {
    let path = HSTRING::from(path);
    // SAFETY: the name is NUL-terminated and outlives the call, and both
    // keys are open.
    let status = unsafe { RegCopyTreeW(root, &path, into) };
    checked(status, "a key of the registry was not copied")
}

/// Removes the tree `path` under `root`; one that is missing is as good.
fn delete_tree(root: HKEY, path: &str) -> Result<()> {
    let path = HSTRING::from(path);
    // SAFETY: the name is NUL-terminated and outlives the call.
    let status = unsafe { RegDeleteTreeW(root, &path) };
    checked(status, "a key of the registry was not removed")
}

/// `status` as a result, with a missing key counted as done.
fn checked(status: WIN32_ERROR, what: &str) -> Result<()> {
    match status {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        status => bail!("{what}: {}", windows::core::Error::from(status)),
    }
}

/// Tells the Shell that associations changed, so that it reads the verbs
/// again.
fn associations_changed() {
    // SAFETY: the event takes no items.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

/// The folder of the file system the known folder of the canonical name
/// `name` is (`Downloads`, `Desktop`, `Personal`, `My Music`, ...); `None`
/// for a name Windows does not know and for a virtual folder, as Control
/// Panel or a library. It can wait for a network folder, so it is not
/// asked on the thread of the user interface.
pub fn known_folder_path(name: &str) -> Option<PathBuf> {
    // SAFETY: COM is initialized for the calls on this thread and left as
    // it was where it was initialized already; the path Windows allocates
    // is copied and freed exactly once.
    unsafe {
        let initialized =
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
        let path = (|| {
            let manager: IKnownFolderManager =
                CoCreateInstance(&KnownFolderManager, None, CLSCTX_INPROC_SERVER).ok()?;
            let folder = manager.GetFolderByName(&HSTRING::from(name)).ok()?;
            let text = folder.GetPath(0).ok()?;
            let path = text.to_string().ok().map(PathBuf::from);
            CoTaskMemFree(Some(text.0.cast()));
            path
        })();
        if initialized {
            CoUninitialize();
        }
        path.filter(|path| path.is_dir())
    }
}

/// Starts Explorer with `line` as its command line, as it reads one; with
/// `fallback`, marked with [`FILES_FALLBACK`], so that a Tessera it starts
/// for a folder hands nothing back.
pub fn start_explorer(line: &str, fallback: bool) -> Result<()> {
    let mut command = Command::new(explorer_program());
    if !line.is_empty() {
        command.raw_arg(line);
    }
    if fallback {
        command.env(FILES_FALLBACK, "1");
    }
    command.spawn().context("Explorer did not start")?;
    Ok(())
}

/// Whether this Tessera was started by an Explorer that a Tessera handed a
/// folder to.
pub fn started_by_files_fallback() -> bool {
    std::env::var_os(FILES_FALLBACK).is_some_and(|value| value == "1")
}

/// Says `text` under `title` in a box of Windows, for a process that has
/// no window of its own; it waits until the box is closed.
pub fn tell_problem(title: &str, text: &str) {
    let (title, text) = (HSTRING::from(title), HSTRING::from(text));
    // SAFETY: both texts are NUL-terminated and outlive the call; the box
    // has no owner.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verb(command: Option<&str>, delegate: Option<&str>) -> FolderVerb {
        FolderVerb {
            class: "Directory",
            verb: "open",
            command: command.map(str::to_owned),
            delegate: delegate.map(str::to_owned),
        }
    }

    #[test]
    fn the_command_names_the_program_and_the_item() {
        assert_eq!(
            files_command(r"C:\Tessera\tessera.exe"),
            r#""C:\Tessera\tessera.exe" files "%1""#
        );
        assert_eq!(
            verb_key("Drive", "explore"),
            r"Software\Classes\Drive\shell\explore"
        );
        assert_eq!(
            backup_key("Drive", Some("open")),
            r"Software\Tessera\FolderHandler\Drive\open"
        );
    }

    #[test]
    fn a_verb_is_tessera_s_only_without_a_delegate() {
        let ours = files_command(r"C:\Tessera\tessera.exe");
        assert!(verb(Some(&ours), None).is_tessera("tessera.exe"));
        assert!(
            !verb(Some(&ours), Some("{11dbb47c-a525-400b-9e80-a54615a090c0}"))
                .is_tessera("tessera.exe")
        );
        assert!(!verb(Some(r"%SystemRoot%\Explorer.exe"), None).is_tessera("tessera.exe"));
        assert!(!verb(None, None).is_tessera("tessera.exe"));
    }

    #[test]
    fn the_status_says_who_opens_folders() {
        let ours = files_command(r"C:\T\tessera.exe");
        let handler = FolderHandler {
            verbs: vec![verb(Some(&ours), None), verb(None, None)],
            installed: true,
        };
        assert!(handler.is_tessera("tessera.exe"));
        let lines = handler.describe("tessera.exe");
        assert_eq!(lines[0], r"Directory\shell\open: Tessera Files");
        assert_eq!(lines[1], r"Directory\shell\open: nothing");
        assert!(lines.iter().any(|line| line.contains("revert")));
        let explorer = FolderHandler {
            verbs: vec![verb(Some("explorer.exe"), Some("{x}"))],
            installed: false,
        };
        assert!(!explorer.is_tessera("tessera.exe"));
        assert!(explorer.describe("tessera.exe")[0].contains("DelegateExecute {x}"));
    }
}
