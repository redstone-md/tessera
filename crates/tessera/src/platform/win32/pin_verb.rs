// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! "Pin to Tessera dock" in the menus Explorer and other programs show for
//! files, folders and shortcuts: a verb of the user's registry,
//! `HKCU\Software\Classes\<class>\shell\TesseraPin`, for every file (`*`),
//! for folders (`Directory`) and for shortcuts (`lnkfile`), each running
//! `"<tessera.exe>" pin "%1"`, which hands the item to the running Tessera.
//! Nothing is written until the user asks for it.

#![allow(unsafe_code)]

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SAM_FLAGS, RegDeleteTreeW};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::core::PCWSTR;

use super::files::wide;
use super::registry::Key;

/// The classes the verb is written for: every file, folders and
/// shortcuts.
const CLASSES: [&str; 3] = ["*", "Directory", "lnkfile"];

/// The name of the verb below `shell` of each class.
const VERB: &str = "TesseraPin";

/// The key of the verb of `class`, below that of the user.
fn verb_key(class: &str) -> String {
    format!(r"Software\Classes\{class}\shell\{VERB}")
}

/// The command Windows runs for the verb, with `executable` as Tessera.
pub fn pin_command_line(executable: &str) -> String {
    format!("\"{executable}\" pin \"%1\"")
}

/// The verb that pins an item to the dock of Tessera.
pub struct PinVerb;

impl PinVerb {
    /// Whether the verb is written for every class and runs this Tessera
    /// program.
    pub fn registered() -> bool {
        let Some(executable) = own_executable() else {
            return false;
        };
        let wanted = pin_command_line(&executable);
        CLASSES.iter().all(|class| {
            let command = format!(r"{}\command", verb_key(class));
            let key = Key::read(HKEY_CURRENT_USER, &command, REG_SAM_FLAGS(0));
            key.and_then(|key| key.text(""))
                .is_some_and(|command| command.eq_ignore_ascii_case(&wanted))
        })
    }

    /// Writes the verb, named `label` in the menus, for files, folders and
    /// shortcuts, running this Tessera program.
    pub fn register(label: &str) -> Result<()> {
        let executable = own_executable().context("Tessera does not know where it is")?;
        let failed = |what: &str| format!("the verb to pin to the dock is not written: {what}");
        let make = |path: &str| {
            Key::create(HKEY_CURRENT_USER, path, REG_SAM_FLAGS(0), false)
                .map(|(key, _)| key)
                .map_err(|failure| failure.into_error(&failed("a key was not made")))
        };
        let write = |key: &Key, name: &str, text: &str| {
            key.set_text(name, text)
                .map_err(|failure| failure.into_error(&failed("a value was not written")))
        };
        for class in CLASSES {
            let verb = make(&verb_key(class))?;
            write(&verb, "", label)?;
            write(&verb, "Icon", &format!("\"{executable}\",0"))?;
            // Several items chosen at once are each handed over, one
            // start of the command for each.
            write(&verb, "MultiSelectModel", "Player")?;
            let command = make(&format!(r"{}\command", verb_key(class)))?;
            write(&command, "", &pin_command_line(&executable))?;
        }
        associations_changed();
        Ok(())
    }

    /// Takes the verb out of the user's registry again.
    pub fn unregister() -> Result<()> {
        for class in CLASSES {
            let path = wide(&verb_key(class));
            // SAFETY: the path is NUL-terminated and outlives the call.
            let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr())) };
            if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                let error = windows::core::Error::from(status);
                return Err(error).context("the verb to pin to the dock stays written");
            }
        }
        associations_changed();
        Ok(())
    }
}

/// The path of the running program.
fn own_executable() -> Option<String> {
    let executable = std::env::current_exe().ok()?;
    executable.to_str().map(str::to_owned)
}

/// Tells the shell that the verbs of kinds of files changed.
fn associations_changed() {
    // SAFETY: SHCNE_ASSOCCHANGED takes no items.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verb_hands_the_item_to_the_pin_command() {
        assert_eq!(
            pin_command_line(r"C:\Program Files\Tessera\tessera.exe"),
            r#""C:\Program Files\Tessera\tessera.exe" pin "%1""#
        );
        assert_eq!(verb_key("*"), r"Software\Classes\*\shell\TesseraPin");
        assert_eq!(
            verb_key("lnkfile"),
            r"Software\Classes\lnkfile\shell\TesseraPin"
        );
    }
}
