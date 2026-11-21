// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Locking the session, putting the computer to sleep and ending the
//! session.

use std::env;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::thread;

use anyhow::{Context as _, Result, bail};
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::Shutdown::LockWorkStation;

/// Keeps a console program from opening a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Something the user can do to the session or the computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PowerAction {
    Lock,
    Sleep,
    SignOut,
    Restart,
    ShutDown,
}

impl PowerAction {
    /// Starts the action. A failure is reported on the error output.
    ///
    /// Going to sleep returns only when the computer wakes, so the work is
    /// done on a thread of its own.
    pub fn perform(self) {
        thread::spawn(move || {
            if let Err(error) = self.run() {
                eprintln!("tessera: {error:#}");
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
            Self::SignOut => Self::shutdown("/l"),
            Self::Restart => Self::shutdown("/r"),
            Self::ShutDown => Self::shutdown("/s"),
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
}
