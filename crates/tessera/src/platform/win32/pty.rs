// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The pseudoconsole (ConPTY) of Tessera's terminal: a shell started in a
//! console of Windows that has no window, whose output is read on a thread
//! of its own and handed to the terminal, and which is written to and
//! resized from another; whether that shell runs a program of the user;
//! and where the windows of the screen are, for tabs dragged between
//! windows of the file manager.
//!
//! The console is made here rather than with `alacritty_terminal::tty`,
//! whose reader needs the `polling` crate to wait, so that the output can
//! be read with plain blocking reads and looked at before the terminal
//! parses it (the folder a shell reports with OSC 7 is not kept by the
//! terminal).

#![allow(unsafe_code)]

use std::ffi::{OsStr, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, WindowFromPoint};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// The programs a shell may keep running beside it without running a
/// program of the user: the hosts of consoles and of WSL.
const HELPERS: [&str; 3] = ["conhost.exe", "openconsole.exe", "wslhost.exe"];

/// How much of the output is read at once.
const READ_SIZE: usize = 64 * 1024;

/// How often the reader looks for output while the terminal waits for a
/// moment to pass, as during a synchronized update.
const PEEK_INTERVAL: Duration = Duration::from_millis(4);

/// How large a console is, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsoleSize {
    pub columns: u16,
    pub lines: u16,
}

impl ConsoleSize {
    fn coord(self) -> COORD {
        COORD {
            X: self.columns.clamp(1, i16::MAX as u16) as i16,
            Y: self.lines.clamp(1, i16::MAX as u16) as i16,
        }
    }
}

/// What a console starts: a command line, in a folder, with variables of
/// the environment beside those of Tessera.
pub struct ConsoleStart<'a> {
    pub command_line: &'a str,
    pub folder: Option<&'a Path>,
    pub environment: &'a [(String, String)],
}

/// What the writer of a console is asked to do.
enum Deed {
    Write(Vec<u8>),
    Resize(ConsoleSize),
    Close,
}

/// A handle of Windows that a thread of its own owns and closes.
struct Owned(isize);

impl Owned {
    fn handle(&self) -> HANDLE {
        HANDLE(self.0 as *mut c_void)
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle was opened for this and is closed once.
        unsafe {
            let _ = CloseHandle(self.handle());
        }
    }
}

/// What writes to a console: what is typed and pasted, and the answers the
/// terminal gives to the programs. It can be cloned and sent anywhere; a
/// console that is gone takes nothing.
#[derive(Clone)]
pub struct ConsoleWriter(Sender<Deed>);

impl ConsoleWriter {
    /// Writes `bytes` to the shell, as typed. Returns at once; the writing
    /// waits for the shell on the thread of the writer.
    pub fn write(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            let _ = self.0.send(Deed::Write(bytes));
        }
    }
}

/// A shell running in a console: it ends with the console, when this is
/// dropped.
pub struct Console {
    deeds: Sender<Deed>,
    shell: u32,
}

impl Console {
    /// Starts `start` in a console of `size`. `output` is given each piece
    /// the console writes, on a thread of its own, and says by when it
    /// wants to be called again without new output, which it then is with
    /// an empty piece. `exited` is told the exit code of the shell, where
    /// Windows gives one, once it has ended. Blocks while Windows makes
    /// the console and the process: not for the thread of GPUI.
    pub fn open(
        start: &ConsoleStart<'_>,
        size: ConsoleSize,
        output: impl FnMut(&[u8]) -> Option<Instant> + Send + 'static,
        exited: impl FnOnce(Option<u32>) + Send + 'static,
    ) -> io::Result<Self> {
        let (input_read, input_write) = pipe()?;
        let (output_read, output_write) = pipe()?;
        // SAFETY: the pipes are open; the console takes copies of the ends
        // it is given, which are closed here once it has them.
        let console = unsafe {
            CreatePseudoConsole(size.coord(), input_read.handle(), output_write.handle(), 0)
        }
        .map_err(io::Error::other)?;
        drop((input_read, output_write));
        let started = spawn_in(console, start);
        let (process, shell) = match started {
            Ok(started) => started,
            Err(error) => {
                // SAFETY: the console was made above and is closed once.
                unsafe { ClosePseudoConsole(console) };
                return Err(error);
            }
        };
        let (deeds, taken) = mpsc::channel();
        let console = console.0;
        thread::Builder::new()
            .name("tessera console writer".to_owned())
            .spawn(move || write_console(console, input_write, taken))?;
        thread::Builder::new()
            .name("tessera console reader".to_owned())
            .spawn(move || read_console(output_read, output))?;
        let closer = deeds.clone();
        thread::Builder::new()
            .name("tessera console waiter".to_owned())
            .spawn(move || {
                let code = wait_for(&process);
                exited(code);
                // The console is closed once its shell has ended, so that
                // the reader sees the end of the output.
                let _ = closer.send(Deed::Close);
            })?;
        Ok(Self { deeds, shell })
    }

    /// What writes to the shell.
    pub fn writer(&self) -> ConsoleWriter {
        ConsoleWriter(self.deeds.clone())
    }

    /// Makes the console `size` large. Returns at once.
    pub fn resize(&self, size: ConsoleSize) {
        let _ = self.deeds.send(Deed::Resize(size));
    }

    /// The number of the shell's process.
    pub fn shell_id(&self) -> u32 {
        self.shell
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.deeds.send(Deed::Close);
    }
}

/// The two ends of a new pipe: the one read from, and the one written to.
fn pipe() -> io::Result<(Owned, Owned)> {
    let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
    // SAFETY: both handles outlive the call, which fills them.
    unsafe { CreatePipe(&raw mut read, &raw mut write, None, 0) }.map_err(io::Error::other)?;
    Ok((Owned(read.0 as isize), Owned(write.0 as isize)))
}

/// `text` as Windows takes it: UTF-16 that ends in a zero.
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

/// The environment of Tessera with `extra` over it, as `CreateProcessW`
/// takes one: `NAME=value` texts that each end in a zero, sorted by name
/// in any case, and a zero after the last.
fn environment_block(extra: &[(String, String)]) -> Vec<u16> {
    let mut variables: Vec<(String, String)> = std::env::vars_os()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .filter(|(name, _)| !{ extra.iter() }.any(|(extra, _)| extra.eq_ignore_ascii_case(name)))
        .collect();
    variables.extend(extra.iter().cloned());
    variables.sort_by_key(|(name, _)| name.to_uppercase());
    let mut block = Vec::new();
    for (name, value) in variables {
        block.extend(format!("{name}={value}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}

/// Starts `start` attached to `console`; gives its process and number.
fn spawn_in(console: HPCON, start: &ConsoleStart<'_>) -> io::Result<(Owned, u32)> {
    let mut size = 0_usize;
    // SAFETY: asked without a list, the call only says how large one is;
    // it fails while saying so, which is expected.
    let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &raw mut size) };
    let mut list_memory = vec![0_u8; size.max(1)];
    let list = LPPROC_THREAD_ATTRIBUTE_LIST(list_memory.as_mut_ptr().cast());
    // SAFETY: the memory is as large as the call said and outlives the
    // list, which is deleted below before the memory goes.
    unsafe { InitializeProcThreadAttributeList(Some(list), 1, None, &raw mut size) }
        .map_err(io::Error::other)?;
    // SAFETY: the list was made above; the value is the console, which
    // outlives the start of the process, given by its size.
    let attributed = unsafe {
        UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            Some(console.0 as *const c_void),
            size_of::<HPCON>(),
            None,
            None,
        )
    };
    let started = attributed.map_err(io::Error::other).and_then(|()| {
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        // No handle of Tessera's is handed down: the console gives its own.
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.lpAttributeList = list;
        let mut line = wide(OsStr::new(start.command_line));
        let folder = start.folder.map(|folder| HSTRING::from(folder.as_os_str()));
        let folder = folder
            .as_ref()
            .map_or(PCWSTR::null(), |folder| PCWSTR(folder.as_ptr()));
        let block = (!start.environment.is_empty()).then(|| environment_block(start.environment));
        let mut info = PROCESS_INFORMATION::default();
        // SAFETY: every pointer given outlives the call: the command line,
        // which the call may change, the folder, the environment and the
        // structures it reads and fills.
        unsafe {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(line.as_mut_ptr())),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                block.as_ref().map(|block| block.as_ptr().cast::<c_void>()),
                folder,
                &raw const startup.StartupInfo,
                &raw mut info,
            )
        }
        .map_err(io::Error::other)?;
        drop(Owned(info.hThread.0 as isize));
        Ok((Owned(info.hProcess.0 as isize), info.dwProcessId))
    });
    // SAFETY: the list was initialized above and is deleted once.
    unsafe { DeleteProcThreadAttributeList(list) };
    started
}

/// Writes what is asked to the console until it is to close, then closes
/// it, which ends the shell where it still runs.
fn write_console(console: isize, input: Owned, deeds: Receiver<Deed>) {
    let console = HPCON(console);
    while let Ok(deed) = deeds.recv() {
        match deed {
            Deed::Write(bytes) => {
                let mut left = bytes.as_slice();
                while !left.is_empty() {
                    let mut written = 0_u32;
                    // SAFETY: the pipe is open and the bytes outlive the
                    // call, which blocks until the console takes them.
                    let wrote = unsafe {
                        WriteFile(input.handle(), Some(left), Some(&raw mut written), None)
                    };
                    if wrote.is_err() || written == 0 {
                        break;
                    }
                    left = &left[written as usize..];
                }
            }
            Deed::Resize(size) => {
                // SAFETY: the console is open until this thread closes it.
                let _ = unsafe { ResizePseudoConsole(console, size.coord()) };
            }
            Deed::Close => break,
        }
    }
    // SAFETY: the console was made for this thread, which closes it once;
    // the reader goes on reading, so that the console can give the rest
    // of its output, as closing it may wait for that.
    unsafe { ClosePseudoConsole(console) };
    drop(input);
}

/// Reads the output of the console until it ends, handing each piece to
/// `output`, and an empty piece whenever the time `output` asked for has
/// come without output.
fn read_console(pipe: Owned, mut output: impl FnMut(&[u8]) -> Option<Instant>) {
    let mut buffer = vec![0_u8; READ_SIZE];
    let mut due: Option<Instant> = None;
    loop {
        if let Some(when) = due {
            match waiting(&pipe) {
                Some(0) if Instant::now() < when => {
                    thread::sleep(PEEK_INTERVAL);
                    continue;
                }
                Some(0) => {
                    due = output(&[]);
                    continue;
                }
                Some(_) => {}
                None => break,
            }
        }
        let mut read = 0_u32;
        // SAFETY: the pipe is open and the buffer outlives the call, which
        // blocks until there is output or the console closed.
        let done = unsafe { ReadFile(pipe.handle(), Some(&mut buffer), Some(&raw mut read), None) };
        if done.is_err() || read == 0 {
            break;
        }
        due = output(&buffer[..read as usize]);
    }
}

/// How many bytes wait in `pipe` to be read; none where it closed.
fn waiting(pipe: &Owned) -> Option<u32> {
    let mut available = 0_u32;
    // SAFETY: the pipe is open and the count outlives the call, which
    // only looks.
    unsafe { PeekNamedPipe(pipe.handle(), None, 0, None, Some(&raw mut available), None) }
        .ok()
        .map(|()| available)
}

/// Waits for `process` to end; gives its exit code, where Windows has it.
fn wait_for(process: &Owned) -> Option<u32> {
    // SAFETY: the process handle is open until `process` is dropped.
    unsafe {
        WaitForSingleObject(process.handle(), INFINITE);
        let mut code = 0_u32;
        GetExitCodeProcess(process.handle(), &raw mut code)
            .ok()
            .map(|()| code)
    }
}

/// A snapshot of the processes that is closed when it is dropped.
struct Snapshot(HANDLE);

impl Drop for Snapshot {
    fn drop(&mut self) {
        // SAFETY: the handle was opened for this and is closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Whether the shell of the process `shell` waits at its prompt: no
/// process of the user runs as its child, the hosts of consoles and of
/// WSL aside. A list that cannot be read says it does not, so that
/// nothing is typed into a program. It takes a few milliseconds: not
/// for the thread of GPUI.
pub fn shell_is_idle(shell: u32) -> bool {
    // SAFETY: the call has no preconditions; the snapshot it gives is
    // closed by `Snapshot`.
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return false;
    };
    let snapshot = Snapshot(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: the entry has its size set and outlives each call, which
    // fills it.
    let mut more = unsafe { Process32FirstW(snapshot.0, &raw mut entry) }.is_ok();
    while more {
        if entry.th32ParentProcessID == shell && entry.th32ProcessID != shell {
            let length = { entry.szExeFile.iter() }
                .position(|&unit| unit == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..length]).to_lowercase();
            if !HELPERS.contains(&name.as_str()) {
                return false;
            }
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot.0, &raw mut entry) }.is_ok();
    }
    true
}

/// Where the top-left corner of what the window `hwnd` draws in is on
/// the screen, in physical pixels.
pub fn client_origin(hwnd: isize) -> Option<(i32, i32)> {
    let mut point = POINT::default();
    // SAFETY: `point` outlives the call, which only reads where the window
    // is; a window that is gone makes it fail.
    unsafe { ClientToScreen(HWND(hwnd as *mut c_void), &raw mut point) }
        .as_bool()
        .then_some((point.x, point.y))
}

/// The window of the screen at the point `at`, in physical pixels: the
/// top-level one that holds the window there, which is seen at that point
/// over the others.
pub fn root_window_at(at: (i32, i32)) -> Option<isize> {
    // SAFETY: the calls only read which window is at a point and which
    // window holds it.
    let root = unsafe {
        let window = WindowFromPoint(POINT { x: at.0, y: at.1 });
        if window.is_invalid() {
            return None;
        }
        GetAncestor(window, GA_ROOT)
    };
    (!root.is_invalid()).then_some(root.0 as isize)
}
