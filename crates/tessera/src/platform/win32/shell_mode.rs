// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Whether Tessera is the shell of the session, in the place of Explorer,
//! or runs beside Explorer.
//!
//! [`shell_mode`] is decided once, when it is first asked, which `main`
//! does before anything is opened, and stays for the life of the process:
//! it is true when Tessera is the shell Winlogon starts (the `Shell` value
//! of `Software\Microsoft\Windows NT\CurrentVersion\Winlogon`, of the user
//! or of the machine, names `tessera.exe`) and no taskbar of Explorer
//! exists. [`explorer_running`] looks again each time it is asked.
//!
//! Parts of Tessera use them as follows:
//! - in shell mode there is no taskbar of Explorer to hide or to keep
//!   hidden, and nothing that answers its messages (show desktop, the
//!   tray, the notification area): `main` leaves the taskbar alone, and a
//!   feature that would ask Explorer does the work itself;
//! - a feature that asks Explorer for something while Tessera runs beside
//!   it (`Taskbar::toggle_desktop`, `Progman` messages) checks
//!   [`explorer_running`] or falls back when the window it asks is not
//!   there, since Explorer may be ended or started at any time;
//! - what only the shell of the session does (startup programs, the
//!   desktop window, `SetShellWindow`) is done only in shell mode.

use std::env;
use std::path::Path;
use std::sync::OnceLock;

use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    RegGetValueW,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::core::w;

use super::Taskbar;
use super::app_windows::executable_of;

/// The key whose `Shell` value names the shell Winlogon starts.
const WINLOGON: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Winlogon";

/// The longest `Shell` value that is read, in UTF-16 units.
const VALUE_LENGTH: usize = 1024;

/// Whether Tessera is the shell of the session; decided once, at the
/// first call.
pub fn shell_mode() -> bool {
    static SHELL: OnceLock<bool> = OnceLock::new();
    *SHELL.get_or_init(|| named_by_winlogon() && !explorer_running())
}

/// Whether Explorer runs as the shell at this moment: a taskbar window
/// that belongs to `explorer.exe` exists. Explorer started only as a file
/// manager, while another program is the shell, makes no taskbar.
pub fn explorer_running() -> bool {
    Taskbar::windows_of_class(w!("Shell_TrayWnd"))
        .into_iter()
        .any(|window| {
            let mut process = 0u32;
            // SAFETY: `process` outlives the call, which only reads window
            // state and tolerates a window that has since been destroyed.
            unsafe { GetWindowThreadProcessId(window, Some(&raw mut process)) };
            let program = executable_of(process);
            program
                .as_deref()
                .and_then(Path::file_name)
                .is_some_and(|name| name.eq_ignore_ascii_case("explorer.exe"))
        })
}

/// Whether the `Shell` value of Winlogon, of the user or of the machine,
/// names this program.
fn named_by_winlogon() -> bool {
    let program = env::current_exe()
        .ok()
        .and_then(|path| Some(path.file_name()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "tessera.exe".to_owned());
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .filter_map(shell_value)
        .any(|value| names_program(&value, &program))
}

/// The `Shell` value of Winlogon under `root`, as it is written.
fn shell_value(root: HKEY) -> Option<String> {
    let key: Vec<u16> = WINLOGON.encode_utf16().chain([0]).collect();
    let mut text = [0_u16; VALUE_LENGTH];
    let mut size = size_of_val(&text) as u32;
    // SAFETY: `size` is the size in bytes of the buffer `text`, both
    // outlive the call, and the names are NUL-terminated.
    let status = unsafe {
        RegGetValueW(
            root,
            windows::core::PCWSTR(key.as_ptr()),
            w!("Shell"),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    status.ok().ok()?;
    let length = text.iter().position(|&unit| unit == 0)?;
    Some(String::from_utf16_lossy(&text[..length]))
}

/// Whether the command line `shell` starts the program file `program`:
/// its name stands at the start, or after a separator of a path, a quote
/// or a space, and ends there or before a quote or a space.
fn names_program(shell: &str, program: &str) -> bool {
    let shell = shell.to_ascii_lowercase();
    let program = program.to_ascii_lowercase();
    shell.match_indices(&program).any(|(at, _)| {
        let before = shell[..at].chars().next_back();
        let after = shell[at + program.len()..].chars().next();
        let starts = before.is_none_or(|c| matches!(c, '\\' | '/' | '"' | ' ' | ','));
        let ends = after.is_none_or(|c| matches!(c, '"' | ' ' | ','));
        starts && ends
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_value_names_the_program_by_its_file() {
        let tessera = "tessera.exe";
        assert!(names_program("tessera.exe", tessera));
        assert!(names_program(
            r#""C:\Program Files\Tessera\Tessera.exe" --shell"#,
            tessera
        ));
        assert!(names_program(r"C:\Tools\tessera.exe", tessera));
        assert!(!names_program("explorer.exe", tessera));
        assert!(!names_program(r"C:\Tools\not-tessera.exe", tessera));
        assert!(!names_program(r"C:\Tools\tessera.exe.bak", tessera));
        assert!(!names_program("", tessera));
    }
}
