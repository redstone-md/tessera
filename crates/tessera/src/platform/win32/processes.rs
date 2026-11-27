// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The processes that run on the machine, and ending one, as the
//! `kill` answers of the launcher do.

#![allow(unsafe_code)]

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_TERMINATE, TerminateProcess,
};

/// The numbers of the idle and the system process, which are no programs
/// to end.
const SYSTEM_PROCESSES: [u32; 2] = [0, 4];

/// The code a process ended by the user exits with, as Task Manager
/// gives it.
const ENDED: u32 = 1;

/// A process that runs, by its number and the file name of its program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningProcess {
    pub id: u32,
    /// As `notepad.exe`.
    pub name: String,
}

/// A handle that is closed when it is dropped.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle was opened for this and is closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

impl RunningProcess {
    /// Every process that runs, but the idle and the system process and
    /// Tessera itself, in the order Windows lists them. Empty where the
    /// list cannot be read. It takes a few milliseconds: not for the
    /// thread of GPUI.
    pub fn all() -> Vec<Self> {
        let own = Self::own_id();
        // SAFETY: the call has no preconditions; the snapshot it gives is
        // closed by `Owned`.
        let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
            return Vec::new();
        };
        let snapshot = Owned(snapshot);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = Vec::new();
        // SAFETY: the entry has its size set and outlives each call, which
        // fills it.
        let mut more = unsafe { Process32FirstW(snapshot.0, &raw mut entry) }.is_ok();
        while more {
            let id = entry.th32ProcessID;
            if id != own && !SYSTEM_PROCESSES.contains(&id) {
                let length = { entry.szExeFile.iter() }
                    .position(|&unit| unit == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
                found.push(Self { id, name });
            }
            // SAFETY: as above.
            more = unsafe { Process32NextW(snapshot.0, &raw mut entry) }.is_ok();
        }
        found
    }

    /// The number of Tessera's own process.
    fn own_id() -> u32 {
        // SAFETY: the call has no preconditions.
        unsafe { GetCurrentProcessId() }
    }

    /// Ends the process `id` at once, as End task of Task Manager does:
    /// what it has not saved is lost. A process of another user or of
    /// the system cannot be ended without the rights to it.
    pub fn kill(id: u32) -> Result<()> {
        if id == Self::own_id() || SYSTEM_PROCESSES.contains(&id) {
            bail!("the process {id} is not one to end");
        }
        // SAFETY: the call only opens the process; the handle is closed by
        // `Owned`.
        let process = unsafe { OpenProcess(PROCESS_TERMINATE, false, id) }
            .with_context(|| format!("the process {id} cannot be ended"))?;
        let process = Owned(process);
        // SAFETY: the handle is open with the right to end the process.
        unsafe { TerminateProcess(process.0, ENDED) }
            .with_context(|| format!("the process {id} did not end"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_leaves_out_tessera_and_the_system() {
        let all = RunningProcess::all();
        let own = RunningProcess::own_id();
        assert!(all.iter().all(|process| process.id != own));
        assert!(
            all.iter()
                .all(|process| !SYSTEM_PROCESSES.contains(&process.id))
        );
        assert!(RunningProcess::kill(own).is_err());
        assert!(RunningProcess::kill(4).is_err());
    }
}
