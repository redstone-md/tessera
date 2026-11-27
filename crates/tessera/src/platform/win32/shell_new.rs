// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The kinds of new files Windows offers under "New" in the context menu
//! of a folder, read from where Explorer reads them, and making one.
//!
//! A kind of file offers itself with a `ShellNew` key in the registry,
//! under `HKEY_CLASSES_ROOT\.ext\<ProgID>\ShellNew` or
//! `HKEY_CLASSES_ROOT\.ext\ShellNew`; the folder has its own under
//! `HKEY_CLASSES_ROOT\Folder\ShellNew`. What the key holds says how the
//! file is made:
//!
//! - `NullFile`: an empty file;
//! - `FileName`: a copy of a template file, looked for in the Templates
//!   folders of the user and of all users and in `%WINDIR%\ShellNew`;
//! - `Data`: a file holding the bytes of the value;
//! - `Directory`: a folder;
//! - `Command`: a program run with the path of the new file for `%1`,
//!   after the file was made where one of the above is given too, as the
//!   shortcut wizard of `.lnk` is.
//!
//! Its name in the menu is `MenuText`, or the friendly name of the kind;
//! the name of the new file is `ItemName`, or "New" and that name, made
//! free in its folder as Explorer makes it: `New Text Document (2).txt`.
//! A key with only a `Handler` (a COM object Explorer asks) is not
//! offered, as Tessera does not run handlers.
//!
//! ```ignore
//! // Away from the thread of GPUI: reading the registry takes a moment.
//! let templates = NewTemplate::all();
//! let made = templates[0].create(&folder, language.say("New"))?;
//! ```
#![allow(unsafe_code)]

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, KEY_READ, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE, RegCloseKey,
    RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonTemplates, FOLDERID_Templates, SHCNE_CREATE, SHCNE_MKDIR, SHCNF_PATHW,
    SHChangeNotify, SHLoadIndirectString,
};
use windows::core::{HSTRING, PWSTR};

use super::execute::expand_environment;
use super::shell_items::{IconPlace, known_folder};

/// How long the kinds that were read are used before the registry is
/// read again: a program that is installed may add one.
const FRESH_FOR: Duration = Duration::from_secs(60);

/// How many names are tried in a folder before giving up.
const NAMES_TRIED: u32 = 10_000;

/// The kinds read last, and when.
static READ: Mutex<Option<(Instant, Arc<Vec<NewTemplate>>)>> = Mutex::new(None);

/// What is put into the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Content {
    Folder,
    /// An empty file.
    Empty,
    /// A copy of the template file of this name or path.
    Copy(String),
    Bytes(Vec<u8>),
    /// Nothing: the command makes it.
    Nothing,
}

/// A kind of item the menu "New" of a folder offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewTemplate {
    /// The extension the new file gets, with its dot (`.txt`); empty for
    /// a folder.
    pub extension: String,
    /// What the menu calls it, in the language of Windows: `Text
    /// Document`.
    pub name: String,
    /// Its icon, where the registry names one.
    pub icon: Option<IconPlace>,
    /// The name of a new item without the extension, where the kind gives
    /// one (`New Shortcut`).
    item_name: Option<String>,
    /// Whether the name of a new item has no extension.
    bare: bool,
    content: Content,
    /// The command line run once the item is made, `%1` standing for it.
    command: Option<String>,
}

#[allow(
    dead_code,
    reason = "for the menu New of the file manager and the desktop"
)]
impl NewTemplate {
    /// Every kind Windows offers: the folder first, then the shortcut,
    /// then the others by name. What was read in the last minute is given
    /// again without reading. It reads the registry, thousands of keys:
    /// never call it on the thread of GPUI.
    pub fn all() -> Vec<Self> {
        let mut read = READ.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((when, templates)) = read.as_ref()
            && when.elapsed() < FRESH_FOR
        {
            return templates.as_ref().clone();
        }
        let templates = Arc::new(scan());
        *read = Some((Instant::now(), templates.clone()));
        templates.as_ref().clone()
    }

    /// Whether this is the new folder.
    pub fn is_folder(&self) -> bool {
        self.content == Content::Folder
    }

    /// The name a new item gets in a folder that holds nothing of that
    /// name, without the extension: what the kind gives, or `new` (the
    /// word "New" in the language of the user) and the name of the kind.
    pub fn item_name(&self, new: &str) -> String {
        match &self.item_name {
            Some(name) => name.clone(),
            None => format!("{new} {}", self.name),
        }
    }

    /// Makes a new item of this kind in `folder` under a name nothing
    /// there has yet (see [`Self::item_name`]) and gives its path. A
    /// command of the kind is started afterwards and not waited for.
    /// Explorer and the other programs that watch the folder are told.
    /// It writes files, perhaps on the network: never call it on the
    /// thread of GPUI.
    pub fn create(&self, folder: &Path, new: &str) -> Result<PathBuf> {
        let stem = self.item_name(new);
        let extension = if self.bare { "" } else { &self.extension };
        let names = free_names(&stem, extension).take(NAMES_TRIED as usize);
        let mut made = None;
        for name in names {
            let path = folder.join(&name);
            match self.make(&path) {
                Ok(()) => {
                    made = Some(path);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(anyhow!(error).context(format!("{name} could not be made")));
                }
            }
        }
        let path = made.with_context(|| format!("{stem} has no free name left"))?;
        if self.content != Content::Nothing {
            tell_explorer(&path, self.is_folder());
        }
        if let Some(command) = &self.command {
            run(command, &path, folder)?;
        }
        Ok(path)
    }

    /// Makes the item at `path`, which fails as `AlreadyExists` where
    /// something lies there already.
    fn make(&self, path: &Path) -> io::Result<()> {
        match &self.content {
            Content::Folder => fs::create_dir(path),
            Content::Empty => File::create_new(path).map(drop),
            Content::Bytes(bytes) => File::create_new(path)?.write_all(bytes),
            Content::Copy(name) => {
                let template = template_file(name).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("the template file {name} is not there"),
                    )
                })?;
                let mut source = File::open(template)?;
                let mut target = File::create_new(path)?;
                io::copy(&mut source, &mut target).map(drop)
            }
            Content::Nothing if path.exists() => Err(io::ErrorKind::AlreadyExists.into()),
            Content::Nothing => Ok(()),
        }
    }
}

/// The names a new item is given, in turn, until one is free: `stem` and
/// the extension, then with ` (2)`, ` (3)` and so on before the
/// extension.
fn free_names<'a>(stem: &'a str, extension: &'a str) -> impl Iterator<Item = String> + 'a {
    let first = std::iter::once(format!("{stem}{extension}"));
    first.chain((2u32..).map(move |number| format!("{stem} ({number}){extension}")))
}

/// Has Explorer and the folder windows that watch the parent of `path`
/// show the new item.
fn tell_explorer(path: &Path, folder: bool) {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let event = if folder { SHCNE_MKDIR } else { SHCNE_CREATE };
    // SAFETY: the path is NUL-terminated and outlives the call, which
    // copies it.
    unsafe { SHChangeNotify(event, SHCNF_PATHW, Some(wide.as_ptr().cast()), None) };
}

/// Runs the command line `command` of a kind for the new item at `path`,
/// in `folder`.
fn run(command: &str, path: &Path, folder: &Path) -> Result<()> {
    // The variables are put in before the path, so that a path with a
    // `%NAME%` in it stays as it is.
    let command = expand_environment(OsStr::new(command));
    let line = command_line(&command.to_string_lossy(), &path.to_string_lossy());
    let (program, arguments) = split_program(&line);
    if program.is_empty() {
        bail!("the command of the new item is empty");
    }
    Command::new(program)
        .raw_arg(arguments)
        .current_dir(folder)
        .spawn()
        .with_context(|| format!("`{line}` did not start"))?;
    Ok(())
}

/// The command line of a kind with the new item at `path` for `%1`, as
/// the registry writes it: unquoted after a comma (`appwiz.cpl,NewLinkHere
/// %1`), where the program takes the rest of the line as the path.
fn command_line(command: &str, path: &str) -> String {
    command.replace("%1", path)
}

/// The program of a command line and the rest of it: up to the closing
/// quote where it begins with one, else up to the first space.
fn split_program(line: &str) -> (&str, &str) {
    let line = line.trim_start();
    if let Some(quoted) = line.strip_prefix('"') {
        return match quoted.split_once('"') {
            Some((program, rest)) => (program, rest.trim_start()),
            None => (quoted, ""),
        };
    }
    match line.split_once(char::is_whitespace) {
        Some((program, rest)) => (program, rest.trim_start()),
        None => (line, ""),
    }
}

/// The template file `name` names: itself where it is a path that exists,
/// or else the file of that name in the Templates folders of the user and
/// of all users or in `%WINDIR%\ShellNew`.
fn template_file(name: &str) -> Option<PathBuf> {
    let name = PathBuf::from(expand_environment(OsStr::new(name)));
    if name.is_absolute() {
        return name.is_file().then_some(name);
    }
    let windows = std::env::var_os("WINDIR").map(|windows| PathBuf::from(windows).join("ShellNew"));
    let folders = [
        known_folder(&FOLDERID_Templates).ok(),
        known_folder(&FOLDERID_CommonTemplates).ok(),
        windows,
    ];
    { folders.into_iter().flatten() }
        .map(|folder| folder.join(&name))
        .find(|path| path.is_file())
}

/// Reads every kind from the registry, in the order of [`NewTemplate::all`].
fn scan() -> Vec<NewTemplate> {
    let Some(classes) = Key::open(HKEY_CLASSES_ROOT, "") else {
        report!("the kinds of new files could not be read from the registry");
        return Vec::new();
    };
    let mut templates: Vec<NewTemplate> = { classes.subkeys().into_iter() }
        .filter(|name| name.len() > 1 && name.starts_with('.'))
        .filter_map(|extension| of_extension(&classes, &extension))
        .collect();
    let folder = { classes.child(r"Folder\ShellNew") }.and_then(|key| of_key(&key, None, ""));
    templates.push(folder.unwrap_or_else(builtin_folder));
    templates.sort_by_cached_key(|template| {
        (
            rank(template),
            template.name.to_lowercase(),
            template.extension.clone(),
        )
    });
    templates
}

/// Where a kind stands in the menu: the folder, then the shortcut, then
/// the others.
fn rank(template: &NewTemplate) -> u8 {
    match template.extension.as_str() {
        _ if template.is_folder() => 0,
        extension if extension.eq_ignore_ascii_case(".lnk") => 1,
        _ => 2,
    }
}

/// The folder as Windows offers it where its key is missing, in English.
fn builtin_folder() -> NewTemplate {
    NewTemplate {
        extension: String::new(),
        name: "Folder".to_owned(),
        icon: None,
        item_name: Some("New folder".to_owned()),
        bare: true,
        content: Content::Folder,
        command: None,
    }
}

/// The kind of files with `extension` (`.txt`), if it offers itself.
fn of_extension(classes: &Key, extension: &str) -> Option<NewTemplate> {
    let key = classes.child(extension)?;
    let class = key.text("").filter(|class| !class.is_empty());
    let under_class =
        { class.as_deref() }.and_then(|class| key.child(&format!(r"{class}\ShellNew")));
    let shell_new = under_class.or_else(|| key.child("ShellNew"))?;
    let class = class.and_then(|class| classes.child(&class));
    of_key(&shell_new, class.as_ref(), extension)
}

/// The kind a `ShellNew` key describes, with `class` the key of its
/// ProgID; `None` where it gives nothing Tessera can make or no name.
fn of_key(shell_new: &Key, class: Option<&Key>, extension: &str) -> Option<NewTemplate> {
    let content = if shell_new.has("Directory") {
        Content::Folder
    } else if let Some(name) = shell_new.text("FileName").filter(|name| !name.is_empty()) {
        Content::Copy(name)
    } else if let Some(data) = shell_new.value("Data") {
        match data {
            Value::Bytes(bytes) => Content::Bytes(bytes),
            Value::Text(text) => Content::Bytes(text.into_bytes()),
        }
    } else if shell_new.has("NullFile") {
        Content::Empty
    } else {
        Content::Nothing
    };
    let command = shell_new
        .text("Command")
        .filter(|command| !command.trim().is_empty());
    if content == Content::Nothing && command.is_none() {
        return None;
    }
    let indirect = |text: String| indirect_text(&text);
    let name = { shell_new.text("MenuText").and_then(indirect) }
        .or_else(|| class?.text("FriendlyTypeName").and_then(indirect))
        .or_else(|| class?.text(""))
        .filter(|name| !name.trim().is_empty())?;
    let icon = { shell_new.text("IconPath") }
        .or_else(|| class?.child("DefaultIcon")?.text(""))
        .and_then(|location| IconPlace::from_location(&location));
    let item_name =
        { shell_new.text("ItemName").and_then(indirect) }.filter(|name| !name.trim().is_empty());
    let bare = shell_new
        .child("Config")
        .is_some_and(|config| config.has("NoExtension"));
    Some(NewTemplate {
        extension: extension.to_owned(),
        name,
        icon,
        item_name,
        bare: bare || content == Content::Folder,
        content,
        command,
    })
}

/// A text of the registry that may name a resource (`@shell32.dll,-30397`,
/// `@{Package?ms-resource://...}`), read; a plain text as it is.
pub(super) fn indirect_text(text: &str) -> Option<String> {
    if !text.starts_with('@') {
        return Some(text.to_owned());
    }
    let source = HSTRING::from(text);
    let mut buffer = [0u16; 1024];
    // SAFETY: the source is NUL-terminated and the buffer carries its
    // length; the call writes a NUL-terminated text into it.
    unsafe { SHLoadIndirectString(&source, &mut buffer, None) }.ok()?;
    let length = buffer.iter().position(|&unit| unit == 0)?;
    Some(String::from_utf16_lossy(&buffer[..length]))
}

/// A value of the registry, as far as kinds of files use them.
enum Value {
    Text(String),
    Bytes(Vec<u8>),
}

/// An open key of the registry, closed when dropped.
struct Key(HKEY);

impl Key {
    /// Opens `path` under `parent` to read; an empty path opens `parent`
    /// once more.
    fn open(parent: HKEY, path: &str) -> Option<Self> {
        let path = HSTRING::from(path);
        let mut key = HKEY::default();
        // SAFETY: the path is NUL-terminated and outlives the call, and
        // the key it opens is closed by `Drop`.
        let opened = unsafe { RegOpenKeyExW(parent, &path, None, KEY_READ, &raw mut key) };
        (opened == ERROR_SUCCESS).then_some(Self(key))
    }

    fn child(&self, path: &str) -> Option<Self> {
        Self::open(self.0, path)
    }

    /// The names of the keys under this one.
    fn subkeys(&self) -> Vec<String> {
        let mut names = Vec::new();
        // A name of a key has at most 255 characters.
        let mut buffer = [0u16; 256];
        for index in 0.. {
            let mut length = buffer.len() as u32;
            // SAFETY: the buffer carries its length, which the call sets
            // to the length of the name it writes.
            let read = unsafe {
                RegEnumKeyExW(
                    self.0,
                    index,
                    Some(PWSTR(buffer.as_mut_ptr())),
                    &raw mut length,
                    None,
                    None,
                    None,
                    None,
                )
            };
            // The end of the keys, or a failure such as a key deleted
            // meanwhile, which would fail again for every index after it.
            if read != ERROR_SUCCESS {
                break;
            }
            let length = (length as usize).min(buffer.len());
            names.push(String::from_utf16_lossy(&buffer[..length]));
        }
        names
    }

    /// Whether the value `name` is there, whatever it holds.
    fn has(&self, name: &str) -> bool {
        let name = HSTRING::from(name);
        // SAFETY: without a buffer the call only tells whether the value
        // is there.
        let read = unsafe { RegQueryValueExW(self.0, &name, None, None, None, None) };
        read == ERROR_SUCCESS
    }

    /// The text of the value `name`, the default value for an empty name,
    /// with the variables of the environment replaced where it is of the
    /// kind that has them.
    fn text(&self, name: &str) -> Option<String> {
        match self.value(name)? {
            Value::Text(text) => Some(text),
            Value::Bytes(_) => None,
        }
    }

    fn value(&self, name: &str) -> Option<Value> {
        let name = HSTRING::from(name);
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        // SAFETY: without a buffer the call tells the kind and the size of
        // the value.
        let asked = unsafe {
            RegQueryValueExW(
                self.0,
                &name,
                None,
                Some(&raw mut kind),
                None,
                Some(&raw mut size),
            )
        };
        if asked != ERROR_SUCCESS {
            return None;
        }
        let mut bytes = vec![0u8; size as usize];
        // SAFETY: the buffer is as large as the call said, and it is told
        // so; a value that grew meanwhile makes the call fail.
        let read = unsafe {
            RegQueryValueExW(
                self.0,
                &name,
                None,
                Some(&raw mut kind),
                Some(bytes.as_mut_ptr()),
                Some(&raw mut size),
            )
        };
        if read != ERROR_SUCCESS {
            return None;
        }
        bytes.truncate(size as usize);
        Some(match kind {
            REG_SZ => Value::Text(registry_text(&bytes)),
            REG_EXPAND_SZ => {
                let text = registry_text(&bytes);
                let expanded = expand_environment(OsStr::new(&text));
                Value::Text(expanded.to_string_lossy().into_owned())
            }
            // Binary data, and numbers, which no kind uses.
            _ => Value::Bytes(bytes),
        })
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened for this value and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// The text a value of the registry holds as UTF-16 bytes, without the
/// NUL it may end in.
fn registry_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = { bytes.as_chunks::<2>().0.iter() }
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(extension: &str, name: &str, content: Content) -> NewTemplate {
        NewTemplate {
            extension: extension.to_owned(),
            name: name.to_owned(),
            icon: None,
            item_name: None,
            bare: false,
            content,
            command: None,
        }
    }

    #[test]
    fn a_new_item_is_numbered_before_its_extension() {
        let names: Vec<String> = free_names("New Text Document", ".txt").take(3).collect();
        assert_eq!(
            names,
            [
                "New Text Document.txt",
                "New Text Document (2).txt",
                "New Text Document (3).txt"
            ]
        );
        let folders: Vec<String> = free_names("New folder", "").take(2).collect();
        assert_eq!(folders, ["New folder", "New folder (2)"]);
    }

    #[test]
    fn a_new_item_is_named_by_its_kind_unless_the_kind_names_it() {
        let mut text = template(".txt", "Text Document", Content::Empty);
        assert_eq!(text.item_name("New"), "New Text Document");
        text.item_name = Some("Note".to_owned());
        assert_eq!(text.item_name("Neu"), "Note");
    }

    #[test]
    fn the_folder_and_the_shortcut_come_first() {
        let folder = builtin_folder();
        let link = template(".LNK", "Shortcut", Content::Empty);
        let text = template(".txt", "Text Document", Content::Empty);
        assert_eq!(rank(&folder), 0);
        assert_eq!(rank(&link), 1);
        assert_eq!(rank(&text), 2);
        assert!(folder.is_folder() && folder.bare);
    }

    #[test]
    fn a_command_line_gets_the_new_item_and_names_its_program() {
        let line = command_line(
            r"%SystemRoot%\system32\rundll32.exe appwiz.cpl,NewLinkHere %1",
            r"C:\Users\a\Desktop\New Shortcut.lnk",
        );
        assert_eq!(
            line,
            r"%SystemRoot%\system32\rundll32.exe appwiz.cpl,NewLinkHere C:\Users\a\Desktop\New Shortcut.lnk"
        );
        assert_eq!(
            split_program(r#" "C:\Program Files\x.exe" -n "%1""#),
            (r"C:\Program Files\x.exe", r#"-n "%1""#)
        );
        assert_eq!(split_program("notepad.exe  a b"), ("notepad.exe", "a b"));
        assert_eq!(split_program("notepad.exe"), ("notepad.exe", ""));
        assert_eq!(split_program(r#""C:\x.exe"#), (r"C:\x.exe", ""));
    }

    #[test]
    fn a_text_of_the_registry_ends_at_its_nul() {
        let bytes: Vec<u8> = "Hi\0\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(registry_text(&bytes), "Hi");
        assert_eq!(registry_text(&[]), "");
        assert_eq!(registry_text(&[0x41, 0, 0x42]), "A");
    }
}
