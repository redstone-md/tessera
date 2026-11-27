// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Locking the session, putting the computer to sleep or into
//! hibernation, switching the user and ending the session, with the
//! updates of Windows installed on the way where some wait for it.

use std::env;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::thread;

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Power::{
    GetPwrCapabilities, SYSTEM_POWER_CAPABILITIES, SetSuspendState,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_DWORD, RegCloseKey, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTSDisconnectSession,
};
use windows::Win32::System::Shutdown::{
    InitiateShutdownW, LockWorkStation, SHTDN_REASON_FLAG_PLANNED,
    SHTDN_REASON_MAJOR_OPERATINGSYSTEM, SHTDN_REASON_MINOR_UPGRADE, SHUTDOWN_FLAGS,
    SHUTDOWN_INSTALL_UPDATES, SHUTDOWN_POWEROFF, SHUTDOWN_RESTART,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

/// Keeps a console program from opening a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The keys whose presence says that updates of Windows wait for a
/// restart: one Windows Update writes, one the servicing of components
/// writes. Neither is documented as such; they are what tools that ask
/// the same question read.
const REBOOT_PENDING: [&str; 2] = [
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired",
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending",
];

/// Where the power options of Windows say which choices its power menu
/// shows ("Show sleep", "Show hibernate" in the Control Panel).
const FLYOUT_MENU: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\FlyoutMenuSettings";

/// Something the user can do to the session or the computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PowerAction {
    Lock,
    Sleep,
    Hibernate,
    SignOut,
    /// Shows the sign-in screen, where another account can sign in, and
    /// keeps this session as it is.
    SwitchUser,
    Restart,
    ShutDown,
    /// Restarts and has Windows install the updates that wait for it.
    UpdateRestart,
    /// Shuts down and has Windows install the updates that wait for it.
    UpdateShutDown,
}

/// What this computer can do at this moment, as its power menu would
/// show it. Reading it asks the registry and the power manager, so it is
/// done away from the thread that draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerChoices {
    pub sleep: bool,
    pub hibernate: bool,
    /// Whether updates of Windows wait for a restart.
    pub updates: bool,
}

impl PowerChoices {
    /// Reads what can be done now.
    pub fn read() -> Self {
        let mut capabilities = SYSTEM_POWER_CAPABILITIES::default();
        // SAFETY: the structure outlives the call, which only fills it.
        let known = unsafe { GetPwrCapabilities(&raw mut capabilities) };
        let can_sleep = known
            && (capabilities.SystemS1
                || capabilities.SystemS2
                || capabilities.SystemS3
                || capabilities.AoAc);
        let can_hibernate = known && capabilities.SystemS4 && capabilities.HiberFilePresent;
        Self {
            sleep: can_sleep && flyout_option("ShowSleepOption").unwrap_or(true),
            hibernate: can_hibernate && flyout_option("ShowHibernateOption").unwrap_or(false),
            updates: updates_pending(),
        }
    }

    /// Whether `action` is to be offered: sleep and hibernation where the
    /// computer can and the power options show them, the two updating
    /// ones where updates wait, and the others always.
    pub fn offers(self, action: PowerAction) -> bool {
        match action {
            PowerAction::Sleep => self.sleep,
            PowerAction::Hibernate => self.hibernate,
            PowerAction::UpdateRestart | PowerAction::UpdateShutDown => self.updates,
            _ => true,
        }
    }
}

/// Whether updates of Windows wait for a restart to be installed.
pub fn updates_pending() -> bool {
    REBOOT_PENDING
        .iter()
        .any(|key| key_exists(HKEY_LOCAL_MACHINE, key))
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Whether the key `path` exists under `root` and can be read.
fn key_exists(root: HKEY, path: &str) -> bool {
    let path = wide(path);
    let mut key = HKEY::default();
    // SAFETY: the name is NUL-terminated and outlives the call; `key`
    // receives a handle that is closed below.
    let status =
        unsafe { RegOpenKeyExW(root, PCWSTR(path.as_ptr()), None, KEY_READ, &raw mut key) };
    if status != ERROR_SUCCESS {
        return false;
    }
    // SAFETY: the handle was opened above and is closed once.
    unsafe {
        let _ = RegCloseKey(key);
    }
    true
}

/// The yes-or-no `name` of the power menu settings of the machine; `None`
/// where it is not written.
fn flyout_option(name: &str) -> Option<bool> {
    let (path, name) = (wide(FLYOUT_MENU), wide(name));
    let mut value = 0_u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the names are NUL-terminated, and `value` and `size`, the
    // size of `value` in bytes, outlive the call.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value != 0)
}

impl PowerAction {
    /// Starts the action. A failure is reported on the error output.
    ///
    /// Going to sleep returns only when the computer wakes, so the work is
    /// done on a thread of its own.
    pub fn perform(self) {
        thread::spawn(move || {
            if let Err(error) = self.run() {
                report!("{error:#}");
            }
        });
    }

    fn run(self) -> Result<()> {
        match self {
            // SAFETY: the call has no preconditions.
            Self::Lock => unsafe { LockWorkStation() }.context("the session was not locked"),
            Self::Sleep => {
                // SAFETY: the call has no preconditions. It asks for sleep
                // rather than hibernation, lets applications object, and
                // leaves wake events on.
                if !unsafe { SetSuspendState(false, false, false) } {
                    bail!("the computer did not go to sleep");
                }
                Ok(())
            }
            Self::Hibernate => {
                // SAFETY: the call has no preconditions. It asks for
                // hibernation, lets applications object, and leaves wake
                // events on.
                if !unsafe { SetSuspendState(true, false, false) } {
                    bail!("the computer did not hibernate");
                }
                Ok(())
            }
            // Disconnecting the session of the console shows the screen
            // to sign in on, as "Switch user" of Windows does.
            // SAFETY: the handle and the number stand for this server and
            // this session; the call does not wait for the disconnection.
            Self::SwitchUser => unsafe {
                WTSDisconnectSession(Some(WTS_CURRENT_SERVER_HANDLE), WTS_CURRENT_SESSION, false)
            }
            .context("the user was not switched"),
            Self::SignOut => Self::shutdown("/l"),
            Self::Restart => Self::shutdown("/r"),
            Self::ShutDown => Self::shutdown("/s"),
            Self::UpdateRestart => Self::install_updates(SHUTDOWN_RESTART),
            Self::UpdateShutDown => Self::install_updates(SHUTDOWN_POWEROFF),
        }
    }

    /// Runs the system's own program for ending the session, which holds
    /// the privilege that shutting down needs.
    fn shutdown(what: &str) -> Result<()> {
        let root = env::var_os("SystemRoot").context("the Windows directory is not known")?;
        let program = PathBuf::from(root).join(r"System32\shutdown.exe");
        let mut command = Command::new(program);
        command.arg(what).creation_flags(CREATE_NO_WINDOW);
        if what != "/l" {
            // Without a delay, which signing out does not take.
            command.args(["/t", "0"]);
        }
        let status = command.status().context("shutdown.exe did not start")?;
        if !status.success() {
            bail!("shutdown.exe {what} failed: {status}");
        }
        Ok(())
    }

    /// Restarts or shuts down (`how`) and asks Windows to install the
    /// updates that wait on the way, which shutdown.exe cannot ask for.
    fn install_updates(how: SHUTDOWN_FLAGS) -> Result<()> {
        enable_shutdown_privilege()?;
        let reason = SHTDN_REASON_MAJOR_OPERATINGSYSTEM
            | SHTDN_REASON_MINOR_UPGRADE
            | SHTDN_REASON_FLAG_PLANNED;
        // SAFETY: no machine name and no message are passed, which means
        // this computer and none.
        let status = unsafe {
            InitiateShutdownW(
                PCWSTR::null(),
                PCWSTR::null(),
                0,
                how | SHUTDOWN_INSTALL_UPDATES,
                reason,
            )
        };
        if status != ERROR_SUCCESS.0 {
            let error = windows::core::Error::from(windows::core::HRESULT::from_win32(status));
            bail!("Windows did not begin to install the updates: {error}");
        }
        Ok(())
    }
}

/// Enables the privilege to shut down in the token of this process, which
/// every user who may shut down has but which is off until asked for.
fn enable_shutdown_privilege() -> Result<()> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle of this process needs no closing; `token`
    // receives a handle that is closed below.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &raw mut token,
        )
    }
    .context("the token of Tessera could not be opened")?;
    let mut luid = LUID::default();
    // SAFETY: the name is a constant of Windows, and `luid` outlives the
    // call that fills it.
    let found = unsafe { LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &raw mut luid) };
    let enabled = found.and_then(|()| {
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        // SAFETY: the structure outlives the call, and no previous state
        // is asked for.
        unsafe { AdjustTokenPrivileges(token, false, Some(&raw const privileges), 0, None, None) }
    });
    // SAFETY: the handle was opened above and is closed once.
    unsafe {
        let _ = CloseHandle(token);
    }
    // AdjustTokenPrivileges succeeds without enabling a privilege the
    // token lacks; InitiateShutdownW then says so itself.
    enabled.context("the privilege to shut down could not be enabled")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_offered_follows_what_the_computer_can_do() {
        let none = PowerChoices::default();
        assert!(!none.offers(PowerAction::Sleep));
        assert!(!none.offers(PowerAction::Hibernate));
        assert!(!none.offers(PowerAction::UpdateRestart));
        assert!(none.offers(PowerAction::ShutDown));
        assert!(none.offers(PowerAction::SwitchUser));
        let all = PowerChoices {
            sleep: true,
            hibernate: true,
            updates: true,
        };
        assert!(all.offers(PowerAction::UpdateShutDown));
        assert!(all.offers(PowerAction::Hibernate));
    }
}
