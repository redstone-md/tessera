// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Words handed to the running Tessera by another start of the program,
//! through a named pipe.
//!
//! The pipe has the access Windows gives one by default, under which only
//! the user who made it, administrators and the system may write to it,
//! and it takes nothing from another machine.

use std::fs::OpenOptions;
use std::io::Write;
use std::thread;

use anyhow::{Context, bail};
use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE};
use windows::Win32::Storage::FileSystem::{PIPE_ACCESS_INBOUND, ReadFile};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows::core::w;

/// Where the pipe is, for the one who writes.
const PATH: &str = r"\\.\pipe\tessera-commands";

/// The most bytes one message may have.
const LONGEST: usize = 1024;

/// How many messages may wait to be answered.
const WAITING: usize = 8;

/// The handle of the pipe, which only its thread uses.
struct Pipe(HANDLE);

// SAFETY: a pipe handle is not bound to the thread that made it, and it is
// moved to the one thread that uses it from then on.
unsafe impl Send for Pipe {}

impl Drop for Pipe {
    fn drop(&mut self) {
        // SAFETY: the handle is the open one of this pipe, closed once.
        unsafe { CloseHandle(self.0).ok() };
    }
}

impl Pipe {
    /// The next message someone writes, once they did; nothing if the
    /// message was not text or reading failed.
    fn next(&self) -> anyhow::Result<Option<String>> {
        let mut bytes = [0u8; LONGEST];
        let mut read = 0u32;
        // SAFETY: the buffer and `read` outlive the calls, which are made
        // on the open handle of this pipe by its only thread.
        let taken = unsafe {
            if let Err(error) = ConnectNamedPipe(self.0, None)
                && error.code() != ERROR_PIPE_CONNECTED.to_hresult()
            {
                bail!("no one can reach the pipe: {error}");
            }
            let taken = ReadFile(self.0, Some(&mut bytes), Some(&raw mut read), None);
            DisconnectNamedPipe(self.0).ok();
            taken
        };
        let text = taken
            .ok()
            .and_then(|()| String::from_utf8(bytes[..read as usize].to_vec()).ok());
        Ok(text)
    }
}

/// The words other starts of the program hand to this one.
pub struct Commands;

impl Commands {
    /// Listens for words from now on; each message arrives as it was
    /// written.
    pub fn listen() -> anyhow::Result<Receiver<String>> {
        // SAFETY: the name is a constant, and no security attributes are
        // passed, which gives the pipe its default access.
        let handle = unsafe {
            CreateNamedPipeW(
                w!(r"\\.\pipe\tessera-commands"),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                LONGEST as u32,
                0,
                None,
            )
        };
        if handle.is_invalid() {
            let error = windows::core::Error::from_win32();
            bail!("the pipe was not made: {error}");
        }
        let pipe = Pipe(handle);
        let (sender, receiver): (Sender<String>, _) = async_channel::bounded(WAITING);
        thread::Builder::new()
            .name("commands".to_owned())
            .spawn(move || {
                loop {
                    match pipe.next() {
                        Ok(Some(text)) => {
                            if sender.send_blocking(text).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            report!("commands are no longer listened for: {error:#}");
                            break;
                        }
                    }
                }
            })
            .context("the thread that listens for commands did not start")?;
        Ok(receiver)
    }

    /// Hands `text` to the Tessera that listens, which fails if none does.
    pub fn send(text: &str) -> std::io::Result<()> {
        let mut pipe = OpenOptions::new().write(true).open(PATH)?;
        pipe.write_all(text.as_bytes())
    }
}
