// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The recorder of what other programs ask of the shell's windows and
//! the shell host does not answer.
//!
//! Explorer's desktop and task manager windows serve protocols that are
//! not documented, such as the server of the Shell's change notifications
//! (`SHChangeNotifyRegister`). Before Tessera answers one, a run on
//! Windows has to show which messages arrive. The window procedures of
//! the desktop (`Progman`), its children `WorkerW` and `SHELLDLL_DefView`
//! and the task manager window hand every message they leave to
//! `DefWindowProc` to [`note`], which counts it and writes it, with its
//! `wParam`, `lParam` and what is known of its sender, to
//! `%LOCALAPPDATA%\tessera\shell-messages.log`.
//!
//! The writing is bounded: the first [`FULL_LINES`] messages of each
//! number on each window are written whole, at most [`LINES_PER_MINUTE`]
//! lines a minute; the others are only counted, and the counts are
//! written every [`SUMMARY_EVERY`] while they change. The file is started
//! anew when it is larger than [`MAX_LOG_BYTES`], keeping the previous one
//! as `shell-messages.old.log`. The lines are written by a thread of their
//! own, so a slow disk never holds the host thread up.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::DataExchange::GetClipboardFormatNameW;
use windows::Win32::UI::WindowsAndMessaging::{
    ISMEX_CALLBACK, ISMEX_NOTIFY, ISMEX_REPLIED, ISMEX_SEND, InSendMessageEx, IsWindow,
};

use super::super::AppWindowId;
use super::super::app_windows::executable_of;
use super::locked;

/// How many messages of one number on one window are written whole.
pub(super) const FULL_LINES: u32 = 3;

/// How many lines are written whole in a minute, over all messages.
pub(super) const LINES_PER_MINUTE: u32 = 120;

/// How often the counts are written while they change.
pub(super) const SUMMARY_EVERY: Duration = Duration::from_secs(60);

/// How large the file may grow before it is started anew.
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

/// The name of the file in `%LOCALAPPDATA%\tessera`.
const LOG_FILE: &str = "shell-messages.log";
const OLD_LOG_FILE: &str = "shell-messages.old.log";

/// A window of the shell host whose messages are recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(in super::super) enum Surface {
    /// The desktop window, of the class `Progman`.
    Desktop,
    /// Its child `WorkerW`, which draws the wallpaper.
    Wallpaper,
    /// Its child `SHELLDLL_DefView`, which holds the icons.
    Icons,
    /// The hidden window of the shell hook, the task manager of the shell.
    TaskManager,
}

impl Surface {
    fn name(self) -> &'static str {
        match self {
            Self::Desktop => "Progman",
            Self::Wallpaper => "WorkerW",
            Self::Icons => "SHELLDLL_DefView",
            Self::TaskManager => "task manager",
        }
    }
}

/// What [`MessageLog::note`] decided for a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Noted {
    /// It is written whole.
    Written,
    /// It is only counted.
    Counted,
}

/// How often one message arrived at one window.
#[derive(Clone, Copy, Debug, Default)]
struct Tally {
    count: u64,
    written: u32,
}

/// The counts of the messages, and how many lines were written: the part
/// of the recorder that decides, without Windows.
#[derive(Debug)]
pub(super) struct MessageLog {
    tallies: HashMap<(Surface, u32), Tally>,
    /// When the minute whose lines are counted began.
    minute: Instant,
    lines_this_minute: u32,
    /// When the counts were last written.
    summarized: Instant,
    /// Whether a count changed since then.
    changed: bool,
}

impl MessageLog {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            tallies: HashMap::new(),
            minute: now,
            lines_this_minute: 0,
            summarized: now,
            changed: false,
        }
    }

    /// Counts `message` on `surface` at `now`, and says whether it is
    /// written whole.
    pub(super) fn note(&mut self, surface: Surface, message: u32, now: Instant) -> Noted {
        if now.saturating_duration_since(self.minute) >= Duration::from_secs(60) {
            self.minute = now;
            self.lines_this_minute = 0;
        }
        self.changed = true;
        let tally = self.tallies.entry((surface, message)).or_default();
        tally.count += 1;
        if tally.written >= FULL_LINES || self.lines_this_minute >= LINES_PER_MINUTE {
            return Noted::Counted;
        }
        tally.written += 1;
        self.lines_this_minute += 1;
        Noted::Written
    }

    /// The counts of every message so far, by window and number, once
    /// [`SUMMARY_EVERY`] has passed since they were last given and one of
    /// them changed.
    pub(super) fn summary(&mut self, now: Instant) -> Option<Vec<(Surface, u32, u64)>> {
        if !self.changed || now.saturating_duration_since(self.summarized) < SUMMARY_EVERY {
            return None;
        }
        self.summarized = now;
        self.changed = false;
        let mut counts: Vec<_> = { self.tallies.iter() }
            .map(|(&(surface, message), tally)| (surface, message, tally.count))
            .collect();
        counts.sort();
        Some(counts)
    }
}

/// Whether messages are recorded; set by `[shell_services] log_messages`.
static RECORDING: AtomicBool = AtomicBool::new(true);
static LOG: Mutex<Option<MessageLog>> = Mutex::new(None);
static WRITER: OnceLock<Option<Sender<String>>> = OnceLock::new();

/// Records the messages left unanswered from now on, or stops.
pub(super) fn set_recording(on: bool) {
    RECORDING.store(on, Ordering::Relaxed);
}

/// Notes that `surface`, the window `window`, left `message` to
/// `DefWindowProc`. Only as the shell, and only while recording.
pub(in super::super) fn note(surface: Surface, window: HWND, message: u32, w: usize, l: isize) {
    if !RECORDING.load(Ordering::Relaxed) || !super::ShellHost::is_shell() {
        return;
    }
    let now = Instant::now();
    let mut log = locked(&LOG);
    let log = log.get_or_insert_with(|| MessageLog::new(now));
    let noted = log.note(surface, message, now);
    let summary = log.summary(now);
    if noted == Noted::Written {
        write(format!(
            "{} {:#06x} {} to {:#x}: w={w:#x} l={l:#x}{}{}",
            surface.name(),
            message,
            message_name(message),
            window.0 as usize,
            how_sent(),
            senders(w, l),
        ));
    }
    if let Some(counts) = summary {
        write(summary_line(&counts));
    }
}

/// The counts as one line.
fn summary_line(counts: &[(Surface, u32, u64)]) -> String {
    let counts: Vec<String> = { counts.iter() }
        .map(|(surface, message, count)| format!("{} {message:#06x} x{count}", surface.name()))
        .collect();
    format!("counts so far: {}", counts.join(", "))
}

/// What a message number is known as: the name it was registered with,
/// or one of the private messages of the shell that the log is kept for.
fn message_name(message: u32) -> String {
    if (0xC000..=0xFFFF).contains(&message) {
        let mut name = [0u16; 128];
        // SAFETY: the buffer outlives the call, which writes at most its
        // length; registered messages share their names with the formats
        // of the clipboard.
        let length = unsafe { GetClipboardFormatNameW(message, &mut name) };
        if length > 0 {
            return format!("\"{}\"", String::from_utf16_lossy(&name[..length as usize]));
        }
        return "(registered)".to_owned();
    }
    let known = match message {
        // ReactOS shell32, changenotify.cpp and undocshell.h.
        0x0419 => "WM_DESKTOP_GET_CNOTIFY_SERVER?",
        0x052C => "(spawn WorkerW)",
        0x004A => "WM_COPYDATA",
        0x0400..=0x7FFF => "WM_USER+",
        _ => "",
    };
    match message {
        0x0400..=0x7FFF if known == "WM_USER+" => format!("WM_USER+{}", message - 0x0400),
        _ => known.to_owned(),
    }
}

/// How the message came: sent from another thread, or not.
fn how_sent() -> String {
    // SAFETY: the call only reads the state of the message being handled.
    let how = unsafe { InSendMessageEx(None) };
    if how == 0 {
        return " (posted or from this thread)".to_owned();
    }
    let mut kinds = Vec::new();
    for (flag, name) in [
        (ISMEX_SEND, "sent"),
        (ISMEX_NOTIFY, "notify"),
        (ISMEX_CALLBACK, "callback"),
        (ISMEX_REPLIED, "replied"),
    ] {
        if how & flag == flag {
            kinds.push(name);
        }
    }
    format!(" ({} from another thread)", kinds.join(", "))
}

/// What is known of the windows `w` and `l` name, where they name one:
/// a sender of `WM_COPYDATA` passes its window as `wParam`, and other
/// protocols pass windows too.
fn senders(w: usize, l: isize) -> String {
    let mut known = String::new();
    for (name, value) in [("w", w as isize), ("l", l)] {
        if value == 0 || value == -1 {
            continue;
        }
        // SAFETY: the call checks any number, which need not be a window.
        if !unsafe { IsWindow(Some(HWND(value as *mut core::ffi::c_void))) }.as_bool() {
            continue;
        }
        let window = AppWindowId::from_raw(value);
        let process = window.process();
        let program = executable_of(process)
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        known.push_str(&format!(
            "; {name} is a window of class {:?} of process {process} {program}",
            window.class()
        ));
    }
    known
}

/// Hands `line` to the thread that writes the file.
fn write(line: String) {
    let writer = WRITER.get_or_init(start_writer);
    if let Some(writer) = writer {
        let at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let _ = writer.send(format!("{at} {line}"));
    }
}

/// Where the log is written; `None` without `%LOCALAPPDATA%`.
fn log_path() -> Option<PathBuf> {
    let folder = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("tessera");
    fs::create_dir_all(&folder).ok()?;
    Some(folder.join(LOG_FILE))
}

/// Starts the thread that writes the lines, once.
fn start_writer() -> Option<Sender<String>> {
    let path = log_path()?;
    let (sender, lines) = mpsc::channel::<String>();
    let started = thread::Builder::new()
        .name("shell messages".to_owned())
        .spawn(move || {
            let mut file = None::<File>;
            let mut written = 0;
            for line in lines {
                if file.is_none() || written > MAX_LOG_BYTES {
                    (file, written) = open_log(&path);
                }
                let Some(open) = file.as_mut() else {
                    continue;
                };
                if writeln!(open, "{line}").is_ok() {
                    written += line.len() as u64 + 2;
                }
            }
        });
    match started {
        Ok(_) => Some(sender),
        Err(error) => {
            report!("the messages to the shell are not recorded: {error}");
            None
        }
    }
}

/// Opens the log to add to it, starting it anew where it grew too large,
/// with a first line that says which Tessera wrote it.
fn open_log(path: &std::path::Path) -> (Option<File>, u64) {
    let size = fs::metadata(path).map_or(0, |metadata| metadata.len());
    if size > MAX_LOG_BYTES {
        let _ = fs::rename(path, path.with_file_name(OLD_LOG_FILE));
    }
    let file = OpenOptions::new().create(true).append(true).open(path);
    match file {
        Ok(mut file) => {
            let size = fs::metadata(path).map_or(0, |metadata| metadata.len());
            let _ = writeln!(
                file,
                "{} Tessera {} records the messages its shell windows leave unanswered",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                env!("CARGO_PKG_VERSION")
            );
            (Some(file), size)
        }
        Err(error) => {
            report!("{} was not opened: {error}", path.display());
            (None, 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_messages_of_a_number_are_written_and_then_counted() {
        let start = Instant::now();
        let mut log = MessageLog::new(start);
        for _ in 0..FULL_LINES {
            assert_eq!(log.note(Surface::Desktop, 0x419, start), Noted::Written);
        }
        assert_eq!(log.note(Surface::Desktop, 0x419, start), Noted::Counted);
        // The same number on another window is counted apart.
        assert_eq!(log.note(Surface::TaskManager, 0x419, start), Noted::Written);
    }

    #[test]
    fn the_lines_of_a_minute_are_bounded() {
        let start = Instant::now();
        let mut log = MessageLog::new(start);
        for message in 0..LINES_PER_MINUTE {
            assert_eq!(log.note(Surface::Icons, message, start), Noted::Written);
        }
        assert_eq!(
            log.note(Surface::Icons, 0x9999, start),
            Noted::Counted,
            "a new number over the bound is only counted"
        );
        let later = start + Duration::from_secs(61);
        assert_eq!(log.note(Surface::Icons, 0x9998, later), Noted::Written);
    }

    #[test]
    fn the_counts_are_given_once_a_minute_while_they_change() {
        let start = Instant::now();
        let mut log = MessageLog::new(start);
        log.note(Surface::Desktop, 0x401, start);
        log.note(Surface::Desktop, 0x401, start);
        log.note(Surface::Wallpaper, 0x10, start);
        assert_eq!(log.summary(start + Duration::from_secs(30)), None);
        let minute = start + SUMMARY_EVERY;
        assert_eq!(
            log.summary(minute),
            Some(vec![
                (Surface::Desktop, 0x401, 2),
                (Surface::Wallpaper, 0x10, 1)
            ])
        );
        // Nothing changed since.
        assert_eq!(log.summary(minute + SUMMARY_EVERY), None);
    }

    #[test]
    fn the_private_messages_of_the_shell_are_named() {
        assert_eq!(message_name(0x0419), "WM_DESKTOP_GET_CNOTIFY_SERVER?");
        assert_eq!(message_name(0x0403), "WM_USER+3");
        assert_eq!(message_name(0x004A), "WM_COPYDATA");
        assert_eq!(message_name(0x0010), "");
    }
}
