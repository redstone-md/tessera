// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The programs that start with the session, which Explorer starts when
//! it is the shell and Tessera starts when it is: [`StartupEntry`] lists
//! them and switches them as Task Manager does, [`SessionStartup`] starts
//! them once per session.
//!
//! What starts, in this order, which is the one Microsoft gives for
//! Explorer (KB 179365) with Active Setup before it:
//! 1. Active Setup: each component under
//!    `HKLM\SOFTWARE\Microsoft\Active Setup\Installed Components` (and its
//!    32-bit view) that is installed and newer than what the user's
//!    record says has its `StubPath` run, one after the other, each
//!    waited for up to [`WAIT`]; the user's record then gets the version.
//! 2. `RunOnce` of the machine, in its 64-bit view and then its 32-bit
//!    one (`WOW6432Node`), one after the other, each waited for; only
//!    where Tessera may delete its values, because a medium process may
//!    not, and they would otherwise run at every sign-in.
//! 3. `Run` and `Policies\Explorer\Run` of the machine (and the 32-bit
//!    `Run`), then of the user, then the Startup folders of all users and
//!    of the user, without waiting, skipping what was switched off in
//!    `Explorer\StartupApproved` (`Run`, `Run32`, `StartupFolder`) of the
//!    hive of the program, where a first byte that is odd means off, and,
//!    for a program of the machine, what the user switched off for
//!    themselves in Tessera's own `HKCU\Software\Tessera\StartupApproved`.
//! 4. `RunOnce` of the user, without waiting.
//! 5. The startup tasks of packaged applications (see [`tasks`]).
//!
//! A value of `RunOnce` is deleted before it runs, or after with a name
//! that starts with `!`; in Safe Mode only those whose name starts with
//! `*` run, and nothing else does. Commands are expanded (`%VAR%`) and
//! split into a program and its arguments the way `CreateProcess` reads
//! an unquoted path, then started with [`Launch`], quietly: a program
//! that is gone is reported, not shown in a dialog.
//!
//! Once per session: a volatile key under `HKCU\Software\Tessera\Startup`,
//! named by the session and the logon, is made before anything starts;
//! when it is there already, as after Tessera restarted, nothing starts
//! again. Windows deletes volatile keys at sign-out. A Tessera that runs
//! elevated, with the full token of an administrator whose other token is
//! filtered, starts nothing and makes no mark: its logon is not the one
//! of the session, and every program would get the administrator's
//! token.
//!
//! Not used: `runonce.exe /AlternateShellStartup`, which is said to run
//! Active Setup and `RunOnce` for shells other than Explorer but is not
//! documented; what it starts could not be checked, and it would start
//! some of them twice beside this. `RunOnceEx`, `RunServices` and the
//! `Load` and `Run` values of `Windows NT\CurrentVersion\Windows` are not
//! started.

#![allow(unsafe_code)]

mod registry;
mod tasks;

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION_TYPE, TOKEN_INFORMATION_CLASS, TOKEN_QUERY,
    TOKEN_STATISTICS, TokenElevationType, TokenElevationTypeFull, TokenStatistics,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WOW64_32KEY,
    KEY_WOW64_64KEY, REG_SAM_FLAGS,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonStartup, FOLDERID_Startup, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CLEANBOOT};
use windows::core::GUID;

use self::registry::{Failure, Key, Value};
use self::tasks::PackageTask;
use super::execute::expand_environment;
use super::packaged::take;
use super::{Launch, explorer_running};

/// How long a program the startup waits for may take before the startup
/// goes on without it.
const WAIT: Duration = Duration::from_secs(60);

/// The keys of the programs that start with every sign-in.
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_ONCE: &str = r"Software\Microsoft\Windows\CurrentVersion\RunOnce";
const POLICY_RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer\Run";

/// Where the choices of the user (and of Task Manager) are kept.
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

/// Where Tessera keeps what a user switched off for themselves of the
/// programs of the machine, apart from the marks Explorer keeps of the
/// user's own programs under the same names.
const OWN_APPROVED: &str = r"Software\Tessera\StartupApproved";

/// The components of Active Setup, of the machine and as the user has
/// them.
const ACTIVE_SETUP: &str = r"Software\Microsoft\Active Setup\Installed Components";
const ACTIVE_SETUP_32: &str = r"Software\Wow6432Node\Microsoft\Active Setup\Installed Components";

/// The key under which the session is marked as started.
const MARKS: &str = r"Software\Tessera\Startup";

/// The name `desktop.ini` of the file a folder keeps its look in.
const DESKTOP_INI: &str = "desktop.ini";

/// Where a program that starts with the session is written down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StartupSource {
    /// `Run` of the machine.
    MachineRun,
    /// The 32-bit view of `Run` of the machine (`WOW6432Node`).
    MachineRun32,
    /// `Policies\Explorer\Run` of the machine.
    MachinePolicy,
    /// `Run` of the user.
    UserRun,
    /// `Policies\Explorer\Run` of the user.
    UserPolicy,
    /// The Startup folder of all users.
    CommonFolder,
    /// The Startup folder of the user.
    UserFolder,
    /// A startup task of a packaged application.
    Packaged,
}

impl StartupSource {
    /// The sources in the order their programs start in.
    const IN_ORDER: [Self; 8] = [
        Self::MachinePolicy,
        Self::MachineRun,
        Self::MachineRun32,
        Self::UserPolicy,
        Self::UserRun,
        Self::CommonFolder,
        Self::UserFolder,
        Self::Packaged,
    ];

    /// What the source is called, in English.
    pub fn title(self) -> &'static str {
        match self {
            Self::MachineRun => "Registry, all users",
            Self::MachineRun32 => "Registry, all users, 32-bit",
            Self::MachinePolicy => "Policy, all users",
            Self::UserRun => "Registry, this user",
            Self::UserPolicy => "Policy, this user",
            Self::CommonFolder => "Startup folder, all users",
            Self::UserFolder => "Startup folder, this user",
            Self::Packaged => "Store app",
        }
    }

    /// Whether what it holds is the machine's, for every user.
    pub fn for_all_users(self) -> bool {
        matches!(
            self,
            Self::MachineRun | Self::MachineRun32 | Self::MachinePolicy | Self::CommonFolder
        )
    }

    /// The key that lists its programs: root, path and view.
    fn key(self) -> Option<(HKEY, &'static str, REG_SAM_FLAGS)> {
        match self {
            Self::MachineRun => Some((HKEY_LOCAL_MACHINE, RUN, KEY_WOW64_64KEY)),
            Self::MachineRun32 => Some((HKEY_LOCAL_MACHINE, RUN, KEY_WOW64_32KEY)),
            Self::MachinePolicy => Some((HKEY_LOCAL_MACHINE, POLICY_RUN, REG_SAM_FLAGS(0))),
            Self::UserRun => Some((HKEY_CURRENT_USER, RUN, REG_SAM_FLAGS(0))),
            Self::UserPolicy => Some((HKEY_CURRENT_USER, POLICY_RUN, REG_SAM_FLAGS(0))),
            Self::CommonFolder | Self::UserFolder | Self::Packaged => None,
        }
    }

    /// The folder that holds its programs.
    fn folder(self) -> Option<GUID> {
        match self {
            Self::CommonFolder => Some(FOLDERID_CommonStartup),
            Self::UserFolder => Some(FOLDERID_Startup),
            _ => None,
        }
    }

    /// The last part of the keys that hold the choices for its programs;
    /// `None` for a policy, which the user cannot switch off.
    fn approval_leaf(self) -> Option<&'static str> {
        match self {
            Self::MachineRun | Self::UserRun => Some("Run"),
            Self::MachineRun32 => Some("Run32"),
            Self::CommonFolder | Self::UserFolder => Some("StartupFolder"),
            Self::MachinePolicy | Self::UserPolicy | Self::Packaged => None,
        }
    }

    /// The key of `StartupApproved` that holds the choices for its
    /// programs, under the root of the source.
    fn approvals(self) -> Option<String> {
        Some(format!(r"{APPROVED}\{}", self.approval_leaf()?))
    }

    /// For a source of the machine, the key of the user under which
    /// Tessera keeps what the user switched off for themselves.
    fn overrides(self) -> Option<String> {
        let leaf = self.approval_leaf().filter(|_| self.for_all_users())?;
        Some(format!(r"{OWN_APPROVED}\{leaf}"))
    }

    fn root(self) -> HKEY {
        if self.for_all_users() {
            HKEY_LOCAL_MACHINE
        } else {
            HKEY_CURRENT_USER
        }
    }
}

/// What an entry is, beyond its name and its command.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Origin {
    /// A value of a key, by its name.
    Value(String),
    /// A file of a Startup folder.
    File(PathBuf),
    Task(PackageTask),
}

/// A program that starts with the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupEntry {
    pub source: StartupSource,
    /// What it is called: the name of the value, of the file without its
    /// extension, or of the task.
    pub name: String,
    /// What starts: the command, the file, or the identifier of the
    /// application.
    pub command: String,
    /// Whether it starts.
    pub enabled: bool,
    /// Whether the user may switch it: a policy decides for itself.
    pub switchable: bool,
    origin: Origin,
}

/// Why an entry was not switched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitchRefusal {
    /// It is the machine's, and only an administrator may change that.
    NeedsAdministrator,
    /// A policy decides it.
    Fixed,
    Failed(String),
}

impl StartupEntry {
    /// Every program that starts with the session, but for those of
    /// Active Setup and `RunOnce`, which start once: in the order they
    /// start in. A source that cannot be read is reported and left out.
    pub fn all() -> Vec<Self> {
        let mut entries = Vec::new();
        for source in StartupSource::IN_ORDER {
            entries.extend(Self::of(source));
        }
        entries
    }

    /// The programs `source` holds.
    fn of(source: StartupSource) -> Vec<Self> {
        if let Some((root, path, view)) = source.key() {
            let Some(key) = Key::read(root, path, view) else {
                return Vec::new();
            };
            let values = key.values().into_iter();
            let commands = values.filter_map(|(name, value)| Some((name, value.text()?)));
            let commands = commands.filter(|(_, command)| !command.trim().is_empty());
            return commands
                .map(|(name, command)| {
                    let enabled = approved_by(source, &name);
                    Self {
                        source,
                        name: name.clone(),
                        command,
                        enabled,
                        switchable: source.approvals().is_some(),
                        origin: Origin::Value(name),
                    }
                })
                .collect();
        }
        if let Some(folder) = source.folder() {
            return folder_files(&folder)
                .into_iter()
                .map(|path| {
                    let file = path.file_name().unwrap_or_default().to_string_lossy();
                    let name = { path.file_stem() }.map_or_else(
                        || file.clone().into_owned(),
                        |stem| stem.to_string_lossy().into_owned(),
                    );
                    Self {
                        source,
                        name,
                        command: path.display().to_string(),
                        enabled: approved_by(source, &file),
                        switchable: true,
                        origin: Origin::File(path.clone()),
                    }
                })
                .collect();
        }
        match PackageTask::all() {
            Ok(tasks) => tasks
                .into_iter()
                .map(|task| Self {
                    source,
                    name: task.name.clone(),
                    command: task.app(),
                    enabled: task.on,
                    switchable: task.switchable,
                    origin: Origin::Task(task),
                })
                .collect(),
            Err(error) => {
                report!("the startup tasks of the Store apps are not known: {error:#}");
                Vec::new()
            }
        }
    }

    /// What `StartupApproved` knows the entry by: the name of the value,
    /// or of the file.
    fn approval_name(&self) -> Option<String> {
        match &self.origin {
            Origin::Value(name) => Some(name.clone()),
            Origin::File(path) => Some(path.file_name()?.to_string_lossy().into_owned()),
            Origin::Task(_) => None,
        }
    }

    /// Switches the entry on or off, where Task Manager does: in
    /// `StartupApproved` of its own hive. A program of the machine, which
    /// only an administrator may switch there, is switched off for the
    /// user alone in Tessera's own key where the machine's is refused,
    /// and the user's choice there is dropped whenever the machine's is
    /// written or the program is switched back on. A startup task has its
    /// state written as the user's choice.
    pub fn set_enabled(&self, on: bool) -> Result<(), SwitchRefusal> {
        if !self.switchable {
            return Err(SwitchRefusal::Fixed);
        }
        if let Origin::Task(task) = &self.origin {
            return task.set_on(on).map_err(refusal);
        }
        let (Some(approvals), Some(name)) = (self.source.approvals(), self.approval_name()) else {
            return Err(SwitchRefusal::Fixed);
        };
        let mark = approval_mark(on, now());
        let own = write_mark(self.source.root(), &approvals, &name, &mark);
        let Some(overrides) = self.source.overrides() else {
            return own.map_err(refusal);
        };
        let off_for_user = |off: bool| {
            let written = match off {
                true => write_mark(HKEY_CURRENT_USER, &overrides, &name, &mark),
                false => clear_mark(HKEY_CURRENT_USER, &overrides, &name),
            };
            written.map_err(refusal)
        };
        match own {
            // The machine's mark now decides for this user as well.
            Ok(()) => off_for_user(false),
            // Off for this user, though not for the others.
            Err(Failure::Denied) if !on => off_for_user(true),
            Err(Failure::Denied) => {
                let off_for_all = read_mark(HKEY_LOCAL_MACHINE, &approvals, &name)
                    .is_some_and(|mark| !approved(Some(&mark)));
                match off_for_all {
                    true => Err(SwitchRefusal::NeedsAdministrator),
                    false => off_for_user(false),
                }
            }
            Err(failure) => Err(refusal(failure)),
        }
    }

    /// The entry as it stands now: whether it is on read again, the rest
    /// as it was.
    pub fn refreshed(&self) -> Self {
        let mut entry = self.clone();
        match &mut entry.origin {
            Origin::Task(task) => {
                *task = task.refreshed();
                (entry.enabled, entry.switchable) = (task.on, task.switchable);
            }
            Origin::Value(_) | Origin::File(_) => {
                if let Some(name) = self.approval_name() {
                    entry.enabled = approved_by(self.source, &name);
                }
            }
        }
        entry
    }

    /// What starts the entry.
    fn launch(&self) -> Launch {
        let launch = match &self.origin {
            Origin::Value(_) => command_launch(&self.command),
            Origin::File(path) => Launch::new(path),
            Origin::Task(task) => Launch::app(&task.app()),
        };
        launch.quiet().named(format!("`{}`", self.name))
    }
}

/// What a failure to write comes to for the user.
fn refusal(failure: Failure) -> SwitchRefusal {
    match failure {
        Failure::Denied => SwitchRefusal::NeedsAdministrator,
        Failure::Other(error) => SwitchRefusal::Failed(format!("{error:#}")),
    }
}

/// Writes the mark `mark` of `name` into the key `approvals` of `root`.
fn write_mark(root: HKEY, approvals: &str, name: &str, mark: &[u8]) -> Result<(), Failure> {
    let (key, _) = Key::create(root, approvals, REG_SAM_FLAGS(0), false)?;
    key.set_bytes(name, mark)
}

/// Removes the mark of `name` from the key `approvals` of `root`, where
/// there is one.
fn clear_mark(root: HKEY, approvals: &str, name: &str) -> Result<(), Failure> {
    match Key::open(root, approvals, KEY_SET_VALUE)? {
        Some(key) => key.delete(name),
        None => Ok(()),
    }
}

/// The mark of `name` in the key `approvals` of `root`.
fn read_mark(root: HKEY, approvals: &str, name: &str) -> Option<Vec<u8>> {
    Key::read(root, approvals, REG_SAM_FLAGS(0))?.bytes(name)
}

/// Whether the program `name` of `source` is on: neither the key of
/// `StartupApproved` of its own hive nor, for a program of the machine,
/// Tessera's key of the user marks it off.
fn approved_by(source: StartupSource, name: &str) -> bool {
    let Some(approvals) = source.approvals() else {
        return true;
    };
    let user = source.overrides().map(|path| (HKEY_CURRENT_USER, path));
    let keys = [Some((source.root(), approvals)), user];
    { keys.into_iter().flatten() }
        .all(|(root, path)| approved(read_mark(root, &path, name).as_deref()))
}

/// Whether a mark of `StartupApproved` leaves a program on: none does,
/// and one whose first byte is even (2 or 6, as Task Manager writes).
fn approved(mark: Option<&[u8]>) -> bool {
    mark.and_then(<[u8]>::first)
        .is_none_or(|first| first & 1 == 0)
}

/// The mark Task Manager writes: 2 for on, 3 for off with the time it was
/// switched off, as a `FILETIME`, in the last eight bytes.
fn approval_mark(on: bool, now: u64) -> [u8; 12] {
    let mut mark = [0u8; 12];
    if on {
        mark[0] = 2;
    } else {
        mark[0] = 3;
        mark[4..].copy_from_slice(&now.to_le_bytes());
    }
    mark
}

/// The time now, as a `FILETIME`.
fn now() -> u64 {
    // SAFETY: the call only reads the clock.
    let time = unsafe { GetSystemTimeAsFileTime() };
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// The files of the known folder `folder`, by name, but for the
/// `desktop.ini` a folder keeps its look in.
fn folder_files(folder: &GUID) -> Vec<PathBuf> {
    // SAFETY: the identifier outlives the call, and the text Windows
    // allocates is freed once by `take`.
    let path = unsafe { SHGetKnownFolderPath(folder, KF_FLAG_DEFAULT, None) }
        .ok()
        .and_then(take)
        .map(PathBuf::from);
    let Some(entries) = path.and_then(|path| fs::read_dir(path).ok()) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| !kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| {
            !{ path.file_name() }.is_some_and(|name| name.eq_ignore_ascii_case(DESKTOP_INI))
        })
        .collect();
    files.sort_by_key(|path| path.file_name().map(|name| name.to_ascii_lowercase()));
    files
}

/// The command line `command` as something to start: expanded, and split
/// into the program and its arguments.
fn command_launch(command: &str) -> Launch {
    let expanded = expand_environment(OsStr::new(command));
    let expanded = expanded.to_string_lossy();
    let (program, arguments) = split_command(&expanded, |path| Path::new(path).is_file());
    Launch::new(program).arguments(arguments)
}

/// The program and the arguments of the command line `command`. A program
/// in quotes ends at the closing quote; one without them ends, as
/// `CreateProcess` reads it, at the first space before which `exists`
/// finds a file (as written, or with `.exe`), and otherwise at the first
/// space.
fn split_command(command: &str, exists: impl Fn(&str) -> bool) -> (String, &str) {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        return match rest.split_once('"') {
            Some((program, arguments)) => (program.to_owned(), arguments.trim_start()),
            None => (rest.to_owned(), ""),
        };
    }
    for (at, _) in command.match_indices(' ') {
        let program = &command[..at];
        let arguments = command[at + 1..].trim_start();
        if exists(program) {
            return (program.to_owned(), arguments);
        }
        let with_extension = format!("{program}.exe");
        if exists(&with_extension) {
            return (with_extension, arguments);
        }
    }
    if exists(command) {
        return (command.to_owned(), "");
    }
    match command.split_once(' ') {
        Some((program, arguments)) => (program.to_owned(), arguments.trim_start()),
        None => (command.to_owned(), ""),
    }
}

/// What the name of a value of `RunOnce` asks for: `!` deletes the value
/// only after the program ran, `*` runs it in Safe Mode too; either may
/// come first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Once {
    delete_after: bool,
    in_safe_mode: bool,
}

impl Once {
    fn of(name: &str) -> Self {
        let mut once = Self::default();
        for mark in name.chars().take_while(|&c| c == '!' || c == '*') {
            match mark {
                '!' => once.delete_after = true,
                _ => once.in_safe_mode = true,
            }
        }
        once
    }
}

/// The numbers of a version of Active Setup, as `1,0,2,4` or `1.0`; a
/// part that is no number counts as 0.
fn version(text: &str) -> Vec<u64> {
    let parts = text.split([',', '.']);
    parts.map(|part| part.trim().parse().unwrap_or(0)).collect()
}

/// Whether a component of Active Setup is to run for the user: the user
/// has no record of it (`None`), or the machine's version is newer than
/// the one recorded (a record without a version counts as 0).
fn setup_pending(machine: Option<&str>, user: Option<Option<&str>>) -> bool {
    let Some(user) = user else {
        return true;
    };
    let Some(machine) = machine else {
        return false;
    };
    let (mut machine, mut user) = (version(machine), version(user.unwrap_or("")));
    let length = machine.len().max(user.len());
    machine.resize(length, 0);
    user.resize(length, 0);
    machine > user
}

/// The name of the mark of a session: its number and the logon it is of,
/// which a profile that stays loaded after sign-out does not repeat.
fn mark_name(session: u32, logon: LUID) -> String {
    format!("{session}-{:08x}{:08x}", logon.HighPart, logon.LowPart)
}

/// Whether Windows started in Safe Mode.
fn in_safe_mode() -> bool {
    // SAFETY: the call only reads a setting of the system.
    unsafe { GetSystemMetrics(SM_CLEANBOOT) != 0 }
}

/// The token of this process, open to be read, closed when dropped.
struct Token(HANDLE);

impl Token {
    fn of_process() -> Result<Self> {
        let mut token = HANDLE::default();
        // SAFETY: `token` outlives the call and is closed once, on drop.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }
            .context("the token of Tessera did not open")?;
        Ok(Self(token))
    }

    /// The logon of the token, which is new at every sign-in.
    fn logon(&self) -> Result<LUID> {
        let statistics: TOKEN_STATISTICS = self
            .information(TokenStatistics)
            .context("the logon of Tessera is not known")?;
        Ok(statistics.AuthenticationId)
    }

    /// Whether it is the full token of an administrator whose sign-in
    /// also has a filtered one, as a program run as administrator has.
    fn elevated(&self) -> Result<bool> {
        let kind: TOKEN_ELEVATION_TYPE = self
            .information(TokenElevationType)
            .context("whether Tessera runs elevated is not known")?;
        Ok(kind == TokenElevationTypeFull)
    }

    /// What Windows tells of the token for `class`, whose answer must be
    /// a `T` of fixed size: `TOKEN_STATISTICS` for `TokenStatistics`,
    /// `TOKEN_ELEVATION_TYPE` for `TokenElevationType`.
    fn information<T: Default>(&self, class: TOKEN_INFORMATION_CLASS) -> Result<T> {
        let mut information = T::default();
        let mut length = 0u32;
        // SAFETY: the buffer is a `T` of the size given, which is what
        // Windows writes for `class` as the callers above ask it; the
        // token is open.
        unsafe {
            GetTokenInformation(
                self.0,
                class,
                Some((&raw mut information).cast()),
                size_of::<T>() as u32,
                &raw mut length,
            )
        }?;
        Ok(information)
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by this value and is closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Starts the programs of the session, in the order of the module.
#[derive(Clone, Copy, Debug)]
pub struct SessionStartup {
    /// Whether the components of Active Setup run.
    pub active_setup: bool,
    /// Whether the startup tasks of packaged applications start.
    pub packaged: bool,
}

impl SessionStartup {
    /// Starts the programs, unless this session started them already,
    /// Explorer runs as the shell, which starts them itself, or Tessera
    /// runs elevated; tells whether it started them. It waits for some of
    /// them: call it on a thread of its own.
    pub fn run_once_per_session(self) -> Result<bool> {
        if explorer_running() {
            return Ok(false);
        }
        let token = Token::of_process()?;
        if token.elevated()? {
            report!(
                "Tessera runs as administrator and starts none of the programs of the session, \
                 which would all get its token; they start when Tessera runs without it"
            );
            return Ok(false);
        }
        if !Self::first_of_session(&token)? {
            return Ok(false);
        }
        self.run();
        Ok(true)
    }

    /// Marks the session as started, and tells whether it was not yet.
    fn first_of_session(token: &Token) -> Result<bool> {
        let mut session = 0u32;
        // SAFETY: `session` outlives the call.
        unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &raw mut session) }
            .context("the session of Tessera is not known")?;
        let name = mark_name(session, token.logon()?);
        // The key of Tessera is made lasting first: a key made under a
        // volatile one must be volatile as well.
        let lasting = Key::create(
            HKEY_CURRENT_USER,
            r"Software\Tessera",
            REG_SAM_FLAGS(0),
            false,
        );
        lasting.map_err(|failure| failure.into_error("the key of Tessera was not made"))?;
        let marked = Key::create(
            HKEY_CURRENT_USER,
            &format!(r"{MARKS}\{name}"),
            REG_SAM_FLAGS(0),
            true,
        );
        let (_, made) =
            marked.map_err(|failure| failure.into_error("the session was not marked"))?;
        Ok(made)
    }

    /// Starts the programs, without asking whether they started already.
    fn run(self) {
        let safe = in_safe_mode();
        if self.active_setup && !safe {
            Self::active_setup(KEY_WOW64_64KEY, ACTIVE_SETUP);
            Self::active_setup(KEY_WOW64_32KEY, ACTIVE_SETUP_32);
        }
        Self::run_once(HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, true, safe);
        Self::run_once(HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, true, safe);
        let start = |source: StartupSource| {
            let entries = StartupEntry::of(source).into_iter();
            for entry in entries.filter(|entry| entry.enabled) {
                entry.launch().start();
            }
        };
        if !safe {
            let sources = StartupSource::IN_ORDER.into_iter();
            sources
                .filter(|&source| source != StartupSource::Packaged)
                .for_each(start);
        }
        // The user's `RunOnce` comes after the Startup folders.
        Self::run_once(HKEY_CURRENT_USER, REG_SAM_FLAGS(0), false, safe);
        if self.packaged && !safe {
            start(StartupSource::Packaged);
        }
    }

    /// Runs the components of Active Setup in the view `view` of the
    /// machine that the user's record under `user` is older for.
    fn active_setup(view: REG_SAM_FLAGS, user: &str) {
        let Some(components) = Key::read(HKEY_LOCAL_MACHINE, ACTIVE_SETUP, view) else {
            return;
        };
        for id in components.subkeys() {
            let Some(component) =
                Key::read(HKEY_LOCAL_MACHINE, &format!(r"{ACTIVE_SETUP}\{id}"), view)
            else {
                continue;
            };
            if component.number("IsInstalled") == Some(0) {
                continue;
            }
            let Some(stub) = component
                .text("StubPath")
                .filter(|stub| !stub.trim().is_empty())
            else {
                continue;
            };
            let machine = component.text("Version");
            let path = format!(r"{user}\{id}");
            let record = Key::read(HKEY_CURRENT_USER, &path, REG_SAM_FLAGS(0));
            let recorded = record.as_ref().map(|record| record.text("Version"));
            let recorded = recorded.as_ref().map(Option::as_deref);
            if !setup_pending(machine.as_deref(), recorded) {
                continue;
            }
            // The record is written before the stub runs, as a stub that
            // fails at every sign-in would otherwise run at every one.
            if let Err(error) = Self::record(&path, machine.as_deref(), &component) {
                report!("Active Setup of `{id}` is left for the next sign-in: {error:#}");
                continue;
            }
            let name = component
                .text("")
                .filter(|name| !name.is_empty())
                .unwrap_or(id);
            let launch = command_launch(&stub).quiet().named(format!("`{name}`"));
            if let Err(error) = start_and_wait(launch) {
                report!("Active Setup: {error}");
            }
        }
    }

    /// Writes the user's record of a component of Active Setup.
    fn record(path: &str, version: Option<&str>, component: &Key) -> Result<()> {
        let (record, _) = Key::create(HKEY_CURRENT_USER, path, REG_SAM_FLAGS(0), false)
            .map_err(|failure| failure.into_error("the record was not made"))?;
        if let Some(version) = version {
            record
                .set_text("Version", version)
                .map_err(|failure| failure.into_error("the version was not written"))?;
        }
        if let Some(locale) = component.text("Locale") {
            record.set_text("Locale", &locale).ok();
        }
        Ok(())
    }

    /// Runs the programs of `RunOnce` under `root`, in the view `view`,
    /// and deletes their values; waits for each with `wait`. A key whose
    /// values Tessera may not delete is left alone.
    fn run_once(root: HKEY, view: REG_SAM_FLAGS, wait: bool, safe: bool) {
        let key = match Key::open(root, RUN_ONCE, KEY_READ | KEY_SET_VALUE | view) {
            Ok(Some(key)) => key,
            Ok(None) => return,
            Err(Failure::Denied) => {
                let waiting = Key::read(root, RUN_ONCE, view)
                    .map(|key| key.values().len())
                    .unwrap_or(0);
                if waiting > 0 {
                    report!(
                        "{waiting} programs that Windows runs once for all users are left for \
                         an administrator: Tessera may not delete them, and they would run at \
                         every sign-in"
                    );
                }
                return;
            }
            Err(Failure::Other(error)) => {
                report!("RunOnce is not read: {error:#}");
                return;
            }
        };
        for (name, value) in key.values() {
            let Value::Text(command) = value else {
                continue;
            };
            let once = Once::of(&name);
            if safe && !once.in_safe_mode {
                continue;
            }
            if !once.delete_after
                && let Err(failure) = key.delete(&name)
            {
                let error = failure.into_error("its value was not deleted");
                report!("`{name}` of RunOnce does not run: {error:#}");
                continue;
            }
            let launch = command_launch(&command).quiet().named(format!("`{name}`"));
            let started = match wait {
                true => start_and_wait(launch),
                false => start(launch),
            };
            match started {
                Ok(()) if once.delete_after => {
                    if let Err(failure) = key.delete(&name) {
                        let error = failure.into_error("its value was not deleted");
                        report!("`{name}` of RunOnce may run again: {error:#}");
                    }
                }
                Ok(()) => {}
                Err(error) => report!("RunOnce: {error}"),
            }
        }
    }
}

/// Starts `launch` and tells whether it started, without waiting for it.
fn start(launch: Launch) -> Result<(), String> {
    let started = launch.started().recv_blocking();
    started
        .map_err(|_| "the start was dropped".to_owned())?
        .map(drop)
}

/// Starts `launch` and waits until what it started ends, for up to
/// [`WAIT`].
fn start_and_wait(launch: Launch) -> Result<(), String> {
    let started = launch.process().started().recv_blocking();
    let process = started.map_err(|_| "the start was dropped".to_owned())??;
    if let Some(process) = process
        && !process.wait(WAIT)
    {
        report!(
            "a program of the startup still runs after {} s; the startup goes on",
            WAIT.as_secs()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_with_an_odd_first_byte_switches_a_program_off() {
        assert!(approved(None));
        assert!(approved(Some(&[])));
        assert!(approved(Some(&[2, 0, 0, 0])));
        assert!(approved(Some(&[6, 0, 0, 0])));
        assert!(!approved(Some(&[3, 0, 0, 0])));
        assert!(!approved(Some(&[7, 0, 0, 0])));
        assert!(!approved(Some(&[1])));
    }

    #[test]
    fn a_program_is_marked_as_task_manager_marks_it() {
        assert_eq!(
            approval_mark(true, 99),
            [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        let off = approval_mark(false, 0x0102_0304_0506_0708);
        assert_eq!(off, [3, 0, 0, 0, 8, 7, 6, 5, 4, 3, 2, 1]);
        assert!(approved(Some(&approval_mark(true, 1))));
        assert!(!approved(Some(&off)));
    }

    #[test]
    fn a_command_is_split_where_its_program_ends() {
        let files = [r"C:\Program Files\App\app.exe", r"C:\Tools\tool.exe"];
        let exists = |path: &str| files.contains(&path);
        let split = |command| split_command(command, exists);
        assert_eq!(
            split(r#""C:\Program Files\App\app.exe" /background"#),
            (r"C:\Program Files\App\app.exe".to_owned(), "/background")
        );
        assert_eq!(
            split(r"C:\Program Files\App\app.exe -min --tray"),
            (r"C:\Program Files\App\app.exe".to_owned(), "-min --tray")
        );
        assert_eq!(
            split(r"C:\Program Files\App\app -min"),
            (r"C:\Program Files\App\app.exe".to_owned(), "-min")
        );
        assert_eq!(
            split(r"C:\Program Files\App\app.exe"),
            (r"C:\Program Files\App\app.exe".to_owned(), "")
        );
        assert_eq!(
            split("rundll32.exe shell32.dll,Control_RunDLL"),
            ("rundll32.exe".to_owned(), "shell32.dll,Control_RunDLL")
        );
        assert_eq!(
            split(r#"  "C:\Tools\tool.exe""#),
            (r"C:\Tools\tool.exe".to_owned(), "")
        );
        assert_eq!(split(r#""unclosed"#), ("unclosed".to_owned(), ""));
    }

    #[test]
    fn the_name_of_a_value_of_run_once_says_when_it_is_deleted() {
        assert_eq!(Once::of("Setup"), Once::default());
        let after = Once::of("!Setup");
        assert!(after.delete_after && !after.in_safe_mode);
        let safe = Once::of("*Setup");
        assert!(!safe.delete_after && safe.in_safe_mode);
        assert_eq!(Once::of("*!Setup"), Once::of("!*Setup"));
        assert!(Once::of("!*Setup").delete_after && Once::of("!*Setup").in_safe_mode);
        assert_eq!(Once::of("Set!up"), Once::default());
    }

    #[test]
    fn a_component_of_active_setup_runs_when_the_machine_has_a_newer_one() {
        assert!(setup_pending(Some("1,0,0,0"), None));
        assert!(setup_pending(None, None));
        assert!(!setup_pending(None, Some(None)));
        assert!(!setup_pending(Some("1,0,0,0"), Some(Some("1,0,0,0"))));
        assert!(!setup_pending(Some("1,0"), Some(Some("1,0,0,0"))));
        assert!(setup_pending(Some("1,0,0,1"), Some(Some("1,0,0,0"))));
        assert!(setup_pending(Some("2.0"), Some(Some("1,9,9,9"))));
        assert!(!setup_pending(Some("1,2"), Some(Some("1,10"))));
        assert!(setup_pending(Some("1"), Some(None)));
        assert!(setup_pending(Some("1,0"), Some(Some(""))));
        assert_eq!(version(" 4, 2 ,x"), [4, 2, 0]);
    }

    #[test]
    fn a_session_is_marked_by_its_number_and_its_logon() {
        let logon = LUID {
            LowPart: 0x3e7,
            HighPart: 1,
        };
        assert_eq!(mark_name(2, logon), "2-00000001000003e7");
    }

    #[test]
    fn programs_start_in_the_order_explorer_starts_them() {
        let order = StartupSource::IN_ORDER;
        let at = |source| order.iter().position(|&s| s == source).unwrap();
        assert!(at(StartupSource::MachineRun) < at(StartupSource::UserRun));
        assert!(at(StartupSource::UserRun) < at(StartupSource::CommonFolder));
        assert!(at(StartupSource::CommonFolder) < at(StartupSource::UserFolder));
        assert_eq!(order.last(), Some(&StartupSource::Packaged));
        for source in order {
            let fixed = matches!(
                source,
                StartupSource::MachinePolicy | StartupSource::UserPolicy | StartupSource::Packaged
            );
            assert_eq!(source.approvals().is_none(), fixed);
            // Tessera's own choices of the user are kept for the
            // programs of the machine alone, apart from Explorer's.
            assert_eq!(
                source.overrides().is_some(),
                !fixed && source.for_all_users()
            );
            if let Some(overrides) = source.overrides() {
                assert!(overrides.starts_with(OWN_APPROVED));
            }
        }
    }
}
