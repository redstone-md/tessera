// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Which program Windows starts as the shell of a session, and handing
//! the shell to Explorer.
//!
//! Userinit starts the program the `Shell` value of
//! `Software\Microsoft\Windows NT\CurrentVersion\Winlogon` names, read
//! through the mapping of `system.ini` under `IniFileMapping`: a mapping
//! that starts with `USR:` reads the value of the user first, one that
//! starts with `SYS:` (as Windows comes) names the value of the machine.
//! Windows is widely reported to read the value of the user even then;
//! that is not verified here. The policy "Custom User Interface" (a
//! `Shell` value under `Policies\System`) comes before both.
//!
//! [`ShellRegistration`] reads all of these, and writes and removes the
//! value of the user only: that needs no administrator, and removing it
//! gives the user back what the machine says, which is Explorer unless
//! an administrator changed it. Nothing here writes the value of the
//! machine or the mapping.
//!
//! [`ExplorerShell`] starts Explorer as the shell when Tessera stops
//! being it. Explorer decides when it starts whether it is the shell of
//! the session, from the same value; while the value names Tessera it
//! may only open a folder window. So the value of the user is set to
//! `explorer.exe` for the moment Explorer takes to show its taskbar, and
//! put back afterwards.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::os::windows::io::IntoRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, HANDLE};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    SetStdHandle,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
};
use windows::Win32::System::Threading::{CREATE_NO_WINDOW, GetCurrentProcessId};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CLEANBOOT};
use windows::core::HSTRING;

use super::registry::registry_text_as_written;
use super::shell_mode::{WINLOGON, explorer_running, names_program};
use super::taskbar::wait_for;

/// The key of the policy "Custom User Interface", of the user and of the
/// machine.
const POLICY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\System";

/// The key that says where Windows reads the `Shell` of `system.ini`.
const MAPPING: &str =
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\IniFileMapping\system.ini\boot";

/// The value all of these keys hold.
const SHELL: &str = "Shell";

/// The program Windows starts as the shell when nothing else is named.
const EXPLORER: &str = "explorer.exe";

/// The flag Tessera is started with as the shell, so that it knows it
/// is the shell even before it finds no taskbar.
pub const SHELL_FLAG: &str = "--shell";

/// The flag of the short process that starts Explorer as the shell once
/// the process whose number follows it is gone.
const EXPLORER_FLAG: &str = "--explorer-after";

/// The longest a `Shell` value is read, in UTF-16 units.
const VALUE_LENGTH: usize = 2048;

/// How long Explorer gets to show its taskbar before the value of the
/// user is put back.
const EXPLORER_TIME: Duration = Duration::from_secs(30);

/// How often it is looked whether Explorer shows its taskbar.
const EXPLORER_LOOK: Duration = Duration::from_millis(250);

/// Which program Windows starts as the shell at the next sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NextShell {
    Tessera,
    Explorer,
    /// Another program, as the value names it.
    Other(String),
}

/// Where the program Windows starts as the shell is named, as read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellRegistration {
    /// The `Shell` value of Winlogon of the user.
    pub user: Option<String>,
    /// The `Shell` value of Winlogon of the machine.
    pub machine: Option<String>,
    /// The policy "Custom User Interface", of the user or else of the
    /// machine; it comes before both.
    pub policy: Option<String>,
    /// Where Windows is told to read the `Shell` of `system.ini`.
    pub mapping: Option<String>,
}

impl ShellRegistration {
    /// What the registry says now.
    pub fn read() -> Self {
        let policy = read_text(HKEY_CURRENT_USER, POLICY, SHELL)
            .or_else(|| read_text(HKEY_LOCAL_MACHINE, POLICY, SHELL));
        Self {
            user: read_text(HKEY_CURRENT_USER, WINLOGON, SHELL),
            machine: read_text(HKEY_LOCAL_MACHINE, WINLOGON, SHELL),
            policy,
            mapping: read_text(HKEY_LOCAL_MACHINE, MAPPING, SHELL),
        }
    }

    /// Makes this Tessera the shell of the user's next sessions: the
    /// value of the user names this program with [`SHELL_FLAG`]. Gives
    /// what the registry says afterwards.
    pub fn install() -> Result<Self> {
        let program = std::env::current_exe().context("the Tessera program was not found")?;
        write_user_value(&shell_command(&program))?;
        Ok(Self::read())
    }

    /// Gives the shell of the user's next sessions back to what the
    /// machine names: the value of the user is removed, and where the
    /// machine names Tessera, it is set to Explorer instead.
    pub fn revert() -> Result<Self> {
        remove_user_value()?;
        let after = Self::read();
        if after.next_shell(&own_program_name()) == NextShell::Tessera && after.policy.is_none() {
            write_user_value(EXPLORER)?;
            return Ok(Self::read());
        }
        Ok(after)
    }

    /// The program Windows starts at the next sign-in, `program` being
    /// the file name of Tessera: the policy if there is one, else the
    /// value of the user, else that of the machine, else Explorer. An
    /// empty value counts as none.
    pub fn next_shell(&self, program: &str) -> NextShell {
        let named = [&self.policy, &self.user, &self.machine]
            .into_iter()
            .flatten()
            .map(|value| value.trim())
            .find(|value| !value.is_empty())
            .unwrap_or(EXPLORER);
        if names_program(named, program) {
            NextShell::Tessera
        } else if names_program(named, EXPLORER) {
            NextShell::Explorer
        } else {
            NextShell::Other(named.to_owned())
        }
    }

    /// Whether the mapping of `system.ini` reads the value of the user
    /// first: `Some(true)` for `USR:`, `Some(false)` for `SYS:` and
    /// `None` where it is missing or says something else.
    pub fn user_is_mapped(&self) -> Option<bool> {
        let mapping = self.mapping.as_deref()?.trim_start();
        let prefix = mapping.get(..4)?.to_ascii_uppercase();
        match prefix.as_str() {
            "USR:" => Some(true),
            "SYS:" => Some(false),
            _ => None,
        }
    }

    /// What `tessera shell status` prints, `program` being the file name
    /// of Tessera.
    pub fn describe(&self, program: &str) -> Vec<String> {
        let shown = |value: &Option<String>| match value {
            Some(value) => format!("\"{value}\""),
            None => "not set".to_owned(),
        };
        let mut lines = vec![
            format!("The user's Shell value of Winlogon: {}", shown(&self.user)),
            format!(
                "The machine's Shell value of Winlogon: {}",
                shown(&self.machine)
            ),
            format!(
                "The policy \"Custom User Interface\": {}",
                shown(&self.policy)
            ),
            format!(
                "The mapping of system.ini (IniFileMapping): {}",
                shown(&self.mapping)
            ),
        ];
        let next = match self.next_shell(program) {
            NextShell::Tessera => "Tessera".to_owned(),
            NextShell::Explorer => "Explorer".to_owned(),
            NextShell::Other(named) => format!("another program, \"{named}\""),
        };
        lines.push(format!("At the next sign-in Windows starts {next}."));
        if self.policy.is_some() {
            lines.push(
                "A policy names the shell; it comes before the values above, and only an administrator can change it."
                    .to_owned(),
            );
        } else if self.user.is_some() && self.user_is_mapped() != Some(true) {
            lines.push(
                "The mapping does not start with USR:, so Windows is told to read the machine's value. Windows is widely reported to read the user's value all the same; if it does not on this computer, an administrator has to change the mapping or the machine's value, which Tessera does not do."
                    .to_owned(),
            );
        }
        lines
    }
}

/// The value of the user that starts `program` as the shell.
fn shell_command(program: &Path) -> String {
    format!("\"{}\" {SHELL_FLAG}", program.display())
}

/// The file name of this program, `tessera.exe` where it cannot be read.
pub fn own_program_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| Some(path.file_name()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "tessera.exe".to_owned())
}

/// Whether Windows was started in Safe Mode.
pub fn is_safe_mode() -> bool {
    // SAFETY: the call only reads a metric of the system.
    unsafe { GetSystemMetrics(SM_CLEANBOOT) != 0 }
}

/// Whether Ctrl and Shift are both held down at this moment, which asks a
/// Tessera that starts as the shell to give the session to Explorer.
pub fn recovery_keys_held() -> bool {
    // SAFETY: the calls only read the state of keys.
    let held = |key: u16| unsafe { GetAsyncKeyState(i32::from(key)) } < 0;
    held(VK_CONTROL.0) && held(VK_SHIFT.0)
}

/// Has what this process prints reach the console it was started from,
/// if it was started from one: a program for the Windows subsystem has
/// none of its own. Output that goes to a file or a pipe already stays
/// there.
pub fn attach_console() {
    // SAFETY: attaching fails without effect where there is no console
    // of the parent, or one is attached already.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_err() {
        return;
    }
    for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: the call only reads a handle of the process.
        let current = unsafe { GetStdHandle(which) };
        let missing = current.is_err() || current.is_ok_and(|handle| handle.is_invalid());
        if !missing {
            continue;
        }
        let Ok(console) = OpenOptions::new().write(true).open("CONOUT$") else {
            continue;
        };
        // The handle stays open for the life of the process.
        let handle = HANDLE(console.into_raw_handle());
        // SAFETY: `handle` is an open handle of the console that is never
        // closed.
        let _ = unsafe { SetStdHandle(which, handle) };
    }
}

/// Starting Explorer as the shell of the session.
pub struct ExplorerShell;

/// Whether this process has had Explorer started as the shell once it is
/// gone.
static HANDED: AtomicBool = AtomicBool::new(false);

impl ExplorerShell {
    /// Hands the session to Explorer, once per process: Explorer starts
    /// as the shell once this Tessera is gone. Says whether this call did
    /// it; `false` where an earlier one has. A failure is given back and
    /// lets a later call try again.
    pub fn hand_over() -> Result<bool> {
        if HANDED.swap(true, Ordering::SeqCst) {
            return Ok(false);
        }
        Self::start_after_exit()
            .inspect_err(|_| HANDED.store(false, Ordering::SeqCst))
            .map(|()| true)
    }

    /// Starts a short process that waits for this one to be gone and then
    /// starts Explorer as the shell, for a Tessera that stops being the
    /// shell: its desktop window must be gone first, or Explorer finds a
    /// shell and stays a file manager.
    fn start_after_exit() -> Result<()> {
        let program = std::env::current_exe().context("the Tessera program was not found")?;
        // SAFETY: the call takes no arguments and cannot fail.
        let own = unsafe { GetCurrentProcessId() };
        Command::new(program)
            .arg(EXPLORER_FLAG)
            .arg(own.to_string())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .context("the process that starts Explorer did not start")?;
        Ok(())
    }

    /// Is that short process if the arguments ask for it, and says
    /// whether they did; the caller exits then. Once Tessera is gone,
    /// `before` is done, then Explorer is started.
    pub fn run_if_requested(
        mut arguments: impl Iterator<Item = String>,
        before: impl FnOnce(),
    ) -> bool {
        if arguments.next().as_deref() != Some(EXPLORER_FLAG) {
            return false;
        }
        if let Some(process) = arguments.next().and_then(|value| value.parse().ok()) {
            wait_for(process);
        }
        before();
        if let Err(error) = Self::start_now() {
            report!("Explorer was not started as the shell: {error:#}");
        }
        true
    }

    /// Starts Explorer and waits, at most [`EXPLORER_TIME`], until it
    /// shows its taskbar. While the next shell is not Explorer, the value
    /// of the user names Explorer meanwhile and is put back afterwards.
    /// A policy that names the shell cannot be gone around: Explorer is
    /// started all the same, and may come as a file manager only.
    pub fn start_now() -> Result<()> {
        let before = ShellRegistration::read();
        let switched = before.next_shell(&own_program_name()) != NextShell::Explorer
            && before.policy.is_none();
        if switched {
            write_user_value(EXPLORER)?;
        }
        let started = Command::new(explorer_program()).spawn();
        if started.is_ok() {
            let until = Instant::now() + EXPLORER_TIME;
            while !explorer_running() && Instant::now() < until {
                thread::sleep(EXPLORER_LOOK);
            }
        }
        if switched {
            let restored = match &before.user {
                Some(value) => write_user_value(value),
                None => remove_user_value(),
            };
            if let Err(error) = restored {
                report!("the user's shell was not put back after Explorer started: {error:#}");
            }
        }
        started.context("Explorer did not start")?;
        Ok(())
    }
}

/// The path of Explorer, in the folder of Windows.
fn explorer_program() -> PathBuf {
    let windows = std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .unwrap_or_else(|| OsString::from(r"C:\Windows"));
    PathBuf::from(windows).join(EXPLORER)
}

/// The text value `name` of the key `key` under `root`, as written; `None`
/// where it is missing.
fn read_text(root: HKEY, key: &str, name: &str) -> Option<String> {
    registry_text_as_written(root, key, Some(name), VALUE_LENGTH)
}

/// Writes the `Shell` value of Winlogon of the user.
fn write_user_value(value: &str) -> Result<()> {
    let key = HSTRING::from(WINLOGON);
    let name = HSTRING::from(SHELL);
    let data: Vec<u16> = value.encode_utf16().chain([0]).collect();
    // SAFETY: the data is the NUL-terminated text with its size in bytes,
    // and the names are NUL-terminated; all outlive the call.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &key,
            &name,
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * size_of::<u16>()) as u32,
        )
    };
    if status == ERROR_ACCESS_DENIED {
        bail!("the user's Shell value of Winlogon may not be written here (access denied)");
    }
    status
        .ok()
        .context("the user's Shell value of Winlogon was not written")
}

/// Removes the `Shell` value of Winlogon of the user; one that is missing
/// is as good.
fn remove_user_value() -> Result<()> {
    let key = HSTRING::from(WINLOGON);
    let name = HSTRING::from(SHELL);
    // SAFETY: the names are NUL-terminated and outlive the call.
    let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &key, &name) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    if status == ERROR_ACCESS_DENIED {
        bail!("the user's Shell value of Winlogon may not be removed here (access denied)");
    }
    status
        .ok()
        .context("the user's Shell value of Winlogon was not removed")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registration(user: Option<&str>, machine: Option<&str>) -> ShellRegistration {
        ShellRegistration {
            user: user.map(str::to_owned),
            machine: machine.map(str::to_owned),
            policy: None,
            mapping: Some(r"SYS:Microsoft\Windows NT\CurrentVersion\Winlogon".to_owned()),
        }
    }

    #[test]
    fn the_next_shell_is_the_users_then_the_machines_then_explorer() {
        let tessera = "tessera.exe";
        let installed = registration(Some(r#""C:\Tessera\tessera.exe" --shell"#), Some(EXPLORER));
        assert_eq!(installed.next_shell(tessera), NextShell::Tessera);
        assert_eq!(
            registration(None, Some(EXPLORER)).next_shell(tessera),
            NextShell::Explorer
        );
        assert_eq!(
            registration(None, None).next_shell(tessera),
            NextShell::Explorer
        );
        assert_eq!(
            registration(Some("  "), Some(r"C:\Tessera\tessera.exe")).next_shell(tessera),
            NextShell::Tessera
        );
        assert_eq!(
            registration(Some("cmd.exe"), None).next_shell(tessera),
            NextShell::Other("cmd.exe".to_owned())
        );
    }

    #[test]
    fn a_policy_comes_before_the_values() {
        let mut governed = registration(Some("tessera.exe --shell"), None);
        governed.policy = Some("explorer.exe".to_owned());
        assert_eq!(governed.next_shell("tessera.exe"), NextShell::Explorer);
        let lines = governed.describe("tessera.exe");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("only an administrator"))
        );
    }

    #[test]
    fn the_mapping_says_whose_value_is_read() {
        let mut read = registration(None, None);
        assert_eq!(read.user_is_mapped(), Some(false));
        read.mapping = Some(r"usr:Microsoft\Windows NT\CurrentVersion\Winlogon".to_owned());
        assert_eq!(read.user_is_mapped(), Some(true));
        read.mapping = None;
        assert_eq!(read.user_is_mapped(), None);
        read.mapping = Some("odd".to_owned());
        assert_eq!(read.user_is_mapped(), None);
    }

    #[test]
    fn the_status_warns_when_the_users_value_may_be_ignored() {
        let installed = registration(Some("tessera.exe --shell"), None);
        let lines = installed.describe("tessera.exe");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Windows starts Tessera"))
        );
        assert!(lines.iter().any(|line| line.contains("USR:")));
        let mut mapped = installed.clone();
        mapped.mapping = Some("USR:Microsoft".to_owned());
        assert!(
            !mapped
                .describe("tessera.exe")
                .iter()
                .any(|line| line.contains("USR:"))
        );
    }

    #[test]
    fn the_users_value_quotes_the_program_and_says_shell() {
        let command = shell_command(Path::new(r"C:\Program Files\Tessera\tessera.exe"));
        assert_eq!(command, r#""C:\Program Files\Tessera\tessera.exe" --shell"#);
        assert!(names_program(&command, "tessera.exe"));
    }
}
