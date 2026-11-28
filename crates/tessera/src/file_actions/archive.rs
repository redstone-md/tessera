// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Archives in the menus of files: "Extract here", "Extract to <name>\"
//! and "Compress to" a ZIP, 7z or tar.gz file, done by the `tar` Windows
//! brings (`%SystemRoot%\System32\tar.exe`, libarchive's bsdtar) in a
//! process of its own, without a window.
//!
//! The entries of an archive are counted first (`tar -tf`), or the items
//! to pack, so that the job's progress can be told through
//! [`FileJobs`]; `tar` names each entry as it goes (`-v`). A job asked to
//! stop ends the process, and an archive it was writing is removed. A
//! name that is taken gets a number, and files that exist where an
//! archive is extracted are kept as they are (`-k`). What `tar` says went
//! wrong is handed back for the user to read.

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use async_channel::Receiver;

use crate::platform::{FileJobEvent, FileJobState, FileJobs, unique_name};

/// Has a console program start without a window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How often a job looks whether it was asked to stop, and publishes how
/// far it got.
const TICK: Duration = Duration::from_millis(200);

/// How many lines of what `tar` says went wrong are handed back.
const ERROR_LINES: usize = 6;

/// The extensions of the archives `tar` reads, in small letters, the
/// longest first.
const ARCHIVES: [&str; 15] = [
    "tar.gz", "tar.bz2", "tar.xz", "tar.zst", "zip", "7z", "rar", "tar", "tgz", "tbz2", "txz",
    "gz", "bz2", "xz", "cab",
];

/// The kinds of archives Tessera packs into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    SevenZip,
    TarGz,
}

impl ArchiveFormat {
    pub const ALL: [Self; 3] = [Self::Zip, Self::SevenZip, Self::TarGz];

    /// The extension of its files, by which `tar -a` chooses it.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::SevenZip => "7z",
            Self::TarGz => "tar.gz",
        }
    }

    /// What the menu calls it, in English, as the table of texts has it.
    pub fn title(self) -> &'static str {
        match self {
            Self::Zip => "ZIP file",
            Self::SevenZip => "7z file",
            Self::TarGz => "tar.gz file",
        }
    }
}

/// The extension of the archive named `name`, in small letters, where
/// `tar` reads it.
pub fn archive_extension(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    { ARCHIVES.into_iter() }.find(|ext| {
        lower.len() > ext.len() + 1
            && lower.ends_with(ext)
            && lower[..lower.len() - ext.len()].ends_with('.')
    })
}

/// Whether the file at `path` is an archive `tar` reads.
pub fn is_archive(path: &Path) -> bool {
    { path.file_name() }.is_some_and(|name| archive_extension(&name.to_string_lossy()).is_some())
}

/// The name of the archive `name` without its extension, as
/// `photos.tar.gz` gives `photos`.
pub fn archive_stem(name: &str) -> String {
    match archive_extension(name) {
        Some(ext) => name[..name.len() - ext.len() - 1].to_owned(),
        None => name.to_owned(),
    }
}

/// A job on an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveJob {
    /// Extracts `archive` into the folder it is in, or with `own_folder`
    /// into a new folder there named after it.
    Extract { archive: PathBuf, own_folder: bool },
    /// Packs `items` into an archive of `format` beside the first.
    Compress {
        items: Vec<PathBuf>,
        format: ArchiveFormat,
    },
}

/// How a job on an archive ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveEnd {
    /// It is done; what it made, the folder or the archive.
    Done(PathBuf),
    Cancelled,
}

impl ArchiveJob {
    /// The folder the job puts what it makes in.
    fn folder(&self) -> Option<PathBuf> {
        let first = match self {
            Self::Extract { archive, .. } => archive,
            Self::Compress { items, .. } => items.first()?,
        };
        first.parent().map(Path::to_owned)
    }

    /// Starts the job on a thread of its own, its progress published as
    /// the job `title`; the receiver yields once, how it ended or what
    /// went wrong.
    pub fn start(self, title: String) -> Receiver<Result<ArchiveEnd, String>> {
        let (answer, answered) = async_channel::bounded(1);
        let spawned = thread::Builder::new()
            .name("tessera-archive".to_owned())
            .spawn(move || {
                let id = FileJobs::next_id();
                let mut progress = Progress::new(id, title, self.folder());
                let ended = self.run(&mut progress);
                progress.end(&ended);
                answer.send_blocking(ended).ok();
            });
        if let Err(error) = spawned {
            report!("the archive is not worked on: {error}");
        }
        answered
    }

    fn run(&self, progress: &mut Progress) -> Result<ArchiveEnd, String> {
        let folder = self.folder().ok_or("there is no folder to work in")?;
        match self {
            Self::Extract {
                archive,
                own_folder,
            } => {
                progress.total(count_entries(archive));
                let into = match own_folder {
                    false => folder,
                    true => {
                        let name = archive_stem(&name_of(archive));
                        let name = unique_name(&name, |name| folder.join(name).exists());
                        let into = folder.join(name);
                        fs::create_dir(&into).map_err(|error| {
                            format!("{} cannot be made: {error}", into.display())
                        })?;
                        into
                    }
                };
                let ended = run_tar(extract_arguments(archive, &into), progress);
                // A folder made for an archive that was not extracted goes.
                if *own_folder && !matches!(ended, Ok(true)) {
                    let _ = fs::remove_dir_all(&into);
                }
                match ended? {
                    true => Ok(ArchiveEnd::Done(into)),
                    false => Ok(ArchiveEnd::Cancelled),
                }
            }
            Self::Compress { items, format } => {
                progress.total(items.iter().map(|item| count_items(item)).sum());
                let first = items.first().ok_or("nothing is chosen")?;
                let stem = match items.len() {
                    1 => name_of(first),
                    _ => { folder.file_name() }
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| archive_stem(&name_of(first))),
                };
                let name = format!("{stem}.{}", format.extension());
                let name = unique_name(&name, |name| folder.join(name).exists());
                let archive = folder.join(name);
                let ended = run_tar(compress_arguments(&archive, items), progress);
                if !matches!(ended, Ok(true)) {
                    let _ = fs::remove_file(&archive);
                }
                match ended? {
                    true => Ok(ArchiveEnd::Done(archive)),
                    false => Ok(ArchiveEnd::Cancelled),
                }
            }
        }
    }
}

/// The name of the item at `path`.
fn name_of(path: &Path) -> String {
    { path.file_name() }
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// What `tar` is given to extract `archive` into the folder `into`,
/// keeping the files there.
pub fn extract_arguments(archive: &Path, into: &Path) -> Vec<OsString> {
    vec![
        "-x".into(),
        "-v".into(),
        "-k".into(),
        "-f".into(),
        archive.into(),
        "-C".into(),
        into.into(),
    ]
}

/// What `tar` is given to pack `items` into `archive`, its kind chosen by
/// its extension, each item by its name in its folder.
pub fn compress_arguments(archive: &Path, items: &[PathBuf]) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = vec![
        "-a".into(),
        "-c".into(),
        "-v".into(),
        "-f".into(),
        archive.into(),
    ];
    let mut folder: Option<&Path> = None;
    for item in items {
        let (Some(parent), Some(name)) = (item.parent(), item.file_name()) else {
            continue;
        };
        if folder != Some(parent) {
            arguments.push("-C".into());
            arguments.push(parent.into());
            folder = Some(parent);
        }
        arguments.push(name.into());
    }
    arguments
}

/// The program of Windows that does the work.
fn tar_program() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(root).join("System32").join("tar.exe")
}

/// A `tar` without a window, whose output and errors are read.
fn tar(arguments: &[OsString]) -> Command {
    let mut command = Command::new(tar_program());
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    command
}

/// How many entries `archive` holds; 0 where `tar` cannot list them.
fn count_entries(archive: &Path) -> u64 {
    let arguments = ["-t".into(), "-f".into(), archive.as_os_str().to_owned()];
    match tar(&arguments).output() {
        Ok(output) if output.status.success() => {
            let listed = String::from_utf8_lossy(&output.stdout);
            listed
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count() as u64
        }
        _ => 0,
    }
}

/// How many entries packing `item` makes: itself and all below it.
fn count_items(item: &Path) -> u64 {
    let mut count = 1;
    let mut folders = vec![item.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(items) = fs::read_dir(&folder) else {
            continue;
        };
        for item in items.flatten() {
            count += 1;
            if item
                .file_type()
                .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
            {
                folders.push(item.path());
            }
        }
    }
    count
}

/// Whether a line `tar -v` writes names an entry it did, as `x name` or
/// `a name`.
fn is_entry_line(line: &str) -> bool {
    line.starts_with("x ") || line.starts_with("a ")
}

/// Runs `tar` with `arguments` until it ends or the job is asked to stop,
/// telling `progress` of every entry it names. `Ok(false)` where it was
/// stopped.
fn run_tar(arguments: Vec<OsString>, progress: &mut Progress) -> Result<bool, String> {
    let mut child = tar(&arguments)
        .spawn()
        .map_err(|error| format!("Windows' tar cannot be started: {error}"))?;
    let (lines, read) = mpsc::channel::<String>();
    // `tar` names its entries on the error output; both are read, so that
    // neither fills up and holds it.
    let mut readers = Vec::new();
    let outputs: [Option<Box<dyn std::io::Read + Send>>; 2] = [
        child
            .stderr
            .take()
            .map(|err| Box::new(err) as Box<dyn std::io::Read + Send>),
        child
            .stdout
            .take()
            .map(|out| Box::new(out) as Box<dyn std::io::Read + Send>),
    ];
    for output in outputs.into_iter().flatten() {
        let lines = lines.clone();
        readers.push(thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if lines.send(line).is_err() {
                    break;
                }
            }
        }));
    }
    drop(lines);
    let mut errors: Vec<String> = Vec::new();
    loop {
        match read.recv_timeout(TICK) {
            Ok(line) if is_entry_line(&line) => progress.step(),
            Ok(line) if !line.trim().is_empty() => {
                errors.push(line.trim().to_owned());
                if errors.len() > ERROR_LINES {
                    errors.remove(0);
                }
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if progress.cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(false);
        }
        progress.publish_now();
    }
    for reader in readers {
        let _ = reader.join();
    }
    let status = child
        .wait()
        .map_err(|error| format!("Windows' tar did not end: {error}"))?;
    match status.success() {
        true => Ok(true),
        false if errors.is_empty() => Err(format!("Windows' tar failed ({status})")),
        false => Err(errors.join("\n")),
    }
}

/// How far a job got, as it is published.
struct Progress {
    event: FileJobEvent,
    /// Whether it changed since it was last published.
    changed: bool,
}

impl Progress {
    fn new(id: u64, title: String, destination: Option<PathBuf>) -> Self {
        let event = FileJobEvent {
            id,
            title,
            items: (0, 0),
            bytes: (0, 0),
            state: FileJobState::Running,
            destination,
            can_cancel: true,
        };
        FileJobs::publish(event.clone());
        Self {
            event,
            changed: false,
        }
    }

    fn total(&mut self, total: u64) {
        self.event.items.1 = total;
        self.changed = true;
    }

    fn step(&mut self) {
        self.event.items.0 += 1;
        // An archive may name more entries than were counted.
        self.event.items.1 = self.event.items.1.max(self.event.items.0);
        self.changed = true;
    }

    fn cancelled(&self) -> bool {
        FileJobs::is_cancelled(self.event.id)
    }

    fn publish_now(&mut self) {
        if self.changed {
            self.changed = false;
            FileJobs::publish(self.event.clone());
        }
    }

    fn end(&mut self, ended: &Result<ArchiveEnd, String>) {
        self.event.state = match ended {
            Ok(ArchiveEnd::Done(_)) => FileJobState::Done,
            Ok(ArchiveEnd::Cancelled) => FileJobState::Cancelled,
            Err(error) => FileJobState::Failed(error.clone()),
        };
        self.event.can_cancel = false;
        FileJobs::publish(self.event.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_are_told_by_their_extensions() {
        assert_eq!(archive_extension("Photos.TAR.GZ"), Some("tar.gz"));
        assert_eq!(archive_extension("a.zip"), Some("zip"));
        assert_eq!(archive_extension("zip"), None);
        assert_eq!(archive_extension(".zip"), None);
        assert_eq!(archive_extension("notes.txt"), None);
        assert_eq!(archive_stem("photos.tar.gz"), "photos");
        assert_eq!(archive_stem("a.b.7z"), "a.b");
        assert_eq!(archive_stem("plain"), "plain");
        assert!(is_archive(Path::new(r"C:\x\backup.7z")));
    }

    #[test]
    fn tar_is_given_each_item_in_its_folder() {
        let items = [
            PathBuf::from(r"C:\Work\a.txt"),
            PathBuf::from(r"C:\Work\b"),
            PathBuf::from(r"D:\Other\c.txt"),
        ];
        let arguments = compress_arguments(Path::new(r"C:\Work\a.txt.zip"), &items);
        let written: Vec<String> = { arguments.iter() }
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            written,
            [
                "-a",
                "-c",
                "-v",
                "-f",
                r"C:\Work\a.txt.zip",
                "-C",
                r"C:\Work",
                "a.txt",
                "b",
                "-C",
                r"D:\Other",
                "c.txt"
            ]
        );
        let extract = extract_arguments(Path::new(r"C:\a.zip"), Path::new(r"C:\a"));
        assert_eq!(extract.len(), 7);
        assert!(is_entry_line("x folder/file.txt") && !is_entry_line("tar: Error"));
    }
}
