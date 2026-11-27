// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! `tessera://` links: registered for the user under
//! `HKCU\Software\Classes\tessera`, so that a click on such a link in a
//! browser hands it to the running Tessera as `tessera do install-resource
//! "<link>"`. Nothing is registered until the user asks for it.

#![allow(unsafe_code)]

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SAM_FLAGS, RegDeleteTreeW};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::core::PCWSTR;

use super::files::wide;
use super::registry::Key;

/// The key of the scheme, below that of the user.
const SCHEME_KEY: &str = r"Software\Classes\tessera";

/// The action a link is handed to.
const ACTION: &str = "install-resource";

/// The command Windows runs for a link, with `executable` as Tessera.
pub fn command_line(executable: &str) -> String {
    format!("\"{executable}\" do {ACTION} \"%1\"")
}

/// The `tessera://` scheme of the user's registry.
pub struct UriScheme;

impl UriScheme {
    /// Whether `tessera://` links are handed to this Tessera program.
    pub fn registered() -> bool {
        let Some(executable) = own_executable() else {
            return false;
        };
        let command = Key::read(
            HKEY_CURRENT_USER,
            &format!(r"{SCHEME_KEY}\shell\open\command"),
            REG_SAM_FLAGS(0),
        );
        let command = command.and_then(|key| key.text(""));
        command.is_some_and(|command| command.eq_ignore_ascii_case(&command_line(&executable)))
    }

    /// Hands `tessera://` links to this Tessera program, in place of
    /// whatever had them before.
    pub fn register() -> Result<()> {
        let executable = own_executable().context("Tessera does not know where it is")?;
        let failed = |what: &str| format!("the tessera:// links are not registered: {what}");
        let make = |path: &str| {
            Key::create(HKEY_CURRENT_USER, path, REG_SAM_FLAGS(0), false)
                .map(|(key, _)| key)
                .map_err(|failure| failure.into_error(&failed("a key was not made")))
        };
        let scheme = make(SCHEME_KEY)?;
        let write = |key: &Key, name: &str, text: &str| {
            key.set_text(name, text)
                .map_err(|failure| failure.into_error(&failed("a value was not written")))
        };
        write(&scheme, "", "URL:Tessera")?;
        write(&scheme, "URL Protocol", "")?;
        let icon = make(&format!(r"{SCHEME_KEY}\DefaultIcon"))?;
        write(&icon, "", &format!("\"{executable}\",0"))?;
        let command = make(&format!(r"{SCHEME_KEY}\shell\open\command"))?;
        write(&command, "", &command_line(&executable))?;
        associations_changed();
        Ok(())
    }

    /// Takes the scheme out of the user's registry again.
    pub fn unregister() -> Result<()> {
        let path = wide(SCHEME_KEY);
        // SAFETY: the path is NUL-terminated and outlives the call.
        let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr())) };
        if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
            let error = windows::core::Error::from(status);
            return Err(error).context("the tessera:// links stay registered");
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

/// Tells the shell that what opens which kind of file or link changed.
fn associations_changed() {
    // SAFETY: SHCNE_ASSOCCHANGED takes no items.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_is_handed_to_the_action_whole() {
        assert_eq!(
            command_line(r"C:\Program Files\Tessera\tessera.exe"),
            r#""C:\Program Files\Tessera\tessera.exe" do install-resource "%1""#
        );
    }
}
