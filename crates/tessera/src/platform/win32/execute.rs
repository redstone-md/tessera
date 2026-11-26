// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Starting files, programs, links and applications the way the Shell of
//! Windows does, with `ShellExecuteEx` on the [`ShellThread`].
//!
//! Everything Tessera opens for the user goes through [`Launch`]: the
//! shortcuts of the Start menu and the launcher, the tasks of jump lists,
//! the files of the file manager.
//!
//! ```ignore
//! Launch::new(r"%WINDIR%\notepad.exe").expanding().arguments("todo.txt").start();
//! Launch::new(path).verb("runas").named("Setup").start();
//! Launch::new(path).verb("properties").start();
//! Launch::app("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App").start();
//! Launch::new("https://example.org").start();
//! // Where the caller wants to know how it went, and wait for the program:
//! let started = Launch::new(command).process().started();
//! cx.spawn(async move |cx| match started.recv().await { ... }).detach();
//! ```
//!
//! A start runs on the [`ShellThread`], except one that may wait without
//! handling messages, which gets a thread of its own: one as an
//! administrator, which waits for the consent of the user, and one of a
//! file on the network, which may wait for a server that does not answer.

use std::ffi::{OsStr, OsString, c_void};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use async_channel::Receiver;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcessId, GetProcessId, SetPriorityClass,
    WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    SEE_MASK_DOENVSUBST, SEE_MASK_FLAG_LOG_USAGE, SEE_MASK_FLAG_NO_UI, SEE_MASK_INVOKEIDLIST,
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, SHOW_WINDOW_CMD, SW_HIDE, SW_SHOWMAXIMIZED,
    SW_SHOWMINNOACTIVE, SW_SHOWNORMAL,
};
use windows::core::PCWSTR;

use super::ShellThread;
use super::packaged::app_path;

/// What `GetDriveTypeW` answers for a drive of the network.
const DRIVE_REMOTE: u32 = 4;

/// How the window of what is started first shows, where it heeds that.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(dead_code, reason = "for the launchers that ask for a window state")]
pub enum Show {
    #[default]
    Normal,
    /// Minimized, without taking the foreground.
    Minimized,
    Maximized,
    Hidden,
}

impl Show {
    fn command(self) -> SHOW_WINDOW_CMD {
        match self {
            Self::Normal => SW_SHOWNORMAL,
            Self::Minimized => SW_SHOWMINNOACTIVE,
            Self::Maximized => SW_SHOWMAXIMIZED,
            Self::Hidden => SW_HIDE,
        }
    }
}

/// Something to start, and how: a file, a folder, a program, a link
/// (`https:`, `mailto:`, `ms-settings:`), a place of the Shell
/// (`shell:Downloads`, `::{GUID}`) or a packaged application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    target: OsString,
    arguments: Option<OsString>,
    directory: Option<PathBuf>,
    verb: Option<String>,
    show: Show,
    quiet: bool,
    expanding: bool,
    process: bool,
    /// The window that owns what the Shell shows, as a number; `None` for
    /// the one in front where it is Tessera's, when the launch is handed
    /// over.
    owner: Option<isize>,
    name: Option<String>,
}

impl Launch {
    /// Starts `target` with its default verb, as a double click does. The
    /// target is taken as it is written: a file may have `%` in its name.
    pub fn new(target: impl AsRef<OsStr>) -> Self {
        Self {
            target: target.as_ref().to_owned(),
            arguments: None,
            directory: None,
            verb: None,
            show: Show::Normal,
            quiet: false,
            expanding: false,
            process: false,
            owner: None,
            name: None,
        }
    }

    /// Starts the packaged (Store) application, or the application of
    /// the Start menu, that has the identifier `app_id`, through
    /// `shell:AppsFolder`.
    #[allow(
        dead_code,
        reason = "for the launchers that start applications by identifier"
    )]
    pub fn app(app_id: &str) -> Self {
        Self::new(app_path(app_id))
    }

    /// The command line handed to what is started, as one text with its
    /// own quoting.
    pub fn arguments(mut self, arguments: impl AsRef<OsStr>) -> Self {
        let arguments = arguments.as_ref();
        self.arguments = (!arguments.is_empty()).then(|| arguments.to_owned());
        self
    }

    /// The folder what is started works in.
    #[allow(dead_code, reason = "for the launchers that give a folder to work in")]
    pub fn directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.directory = Some(directory.into());
        self
    }

    /// The verb to do instead of the default one: `open`, `runas` (as an
    /// administrator), `edit`, `print`, `explore`, `openas` (the choice of
    /// a program), `properties`, or any verb of the context menu of the
    /// file.
    pub fn verb(mut self, verb: &str) -> Self {
        self.verb = Some(verb.to_owned());
        self
    }

    /// How the window of what is started first shows.
    #[allow(dead_code, reason = "for the launchers that ask for a window state")]
    pub fn show(mut self, show: Show) -> Self {
        self.show = show;
        self
    }

    /// Keeps the Shell from showing its own dialogs about a failure, such
    /// as the one that asks for a program for a file of an unknown kind;
    /// the failure is only reported.
    #[allow(dead_code, reason = "for callers that report failures themselves")]
    pub fn quiet(mut self) -> Self {
        self.quiet = true;
        self
    }

    /// Has the Shell replace references to environment variables,
    /// `%NAME%`, in the target and the working directory, for a target
    /// that is written as a template, as the command of a shortcut or of
    /// a task of a jump list may be. Never for the name of a file as it is
    /// on the disk, which may have `%` in it.
    #[allow(dead_code, reason = "for the launchers that start written commands")]
    pub fn expanding(mut self) -> Self {
        self.expanding = true;
        self
    }

    /// Keeps the process that is started, where there is one, for
    /// [`Self::started`] to give.
    #[allow(dead_code, reason = "for the starts that wait for a program")]
    pub fn process(mut self) -> Self {
        self.process = true;
        self
    }

    /// The window of Tessera that what the Shell shows belongs to, such
    /// as the prompt for an administrator, which Windows brings in front
    /// only when its owner is in front. Without one, the window in front
    /// is taken when it is Tessera's, and the hidden window of the shell
    /// thread otherwise.
    #[allow(dead_code, reason = "for the launchers that know their window")]
    pub fn owner(mut self, window: HWND) -> Self {
        self.owner = Some(window.0 as isize);
        self
    }

    /// What a failure is reported as: the name the user knows the target
    /// by. Without one, the target is named.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Starts the target on the shell thread and returns at once. A
    /// failure is reported.
    pub fn start(self) {
        self.hand_over(|started| {
            if let Err(error) = started {
                report!("{error}");
            }
        });
    }

    /// Starts the target as [`Self::start`] does, and tells how it went:
    /// the receiver yields once, the process where [`Self::process`]
    /// asked for it and there is one (a document opened in a program that
    /// runs already, or a link, may have none), or why nothing started.
    /// Nothing is reported; that is the caller's to do. Await it from a
    /// task of GPUI; never block the thread of GPUI on it.
    #[allow(dead_code, reason = "for the launchers that tell how a start went")]
    pub fn started(self) -> Receiver<Result<Option<Process>, String>> {
        let (answer, answered) = async_channel::bounded(1);
        self.hand_over(move |started| {
            answer.try_send(started).ok();
        });
        answered
    }

    /// Starts the target on the thread that suits it and has `then` told
    /// how it went, there.
    fn hand_over(mut self, then: impl FnOnce(Result<Option<Process>, String>) + Send + 'static) {
        if self.owner.is_none() {
            self.owner = own_window_in_front();
        }
        let alone = self.may_wait(is_network_drive);
        let work = move || then(self.start_here().map_err(|error| format!("{error:#}")));
        match alone {
            true => ShellThread::run_alone("launch", work),
            false => ShellThread::run(work),
        }
    }

    /// Whether the start may wait without handling messages, for long
    /// enough to hold up the work after it on the shell thread: as an
    /// administrator it waits for the consent of the user, and on the
    /// network for a server; `network_drive` tells whether a drive letter
    /// names a drive of the network.
    fn may_wait(&self, network_drive: impl Fn(char) -> bool) -> bool {
        let elevated =
            { self.verb.as_deref() }.is_some_and(|verb| verb.eq_ignore_ascii_case("runas"));
        let places = [
            Some(self.target.as_os_str()),
            self.directory.as_deref().map(|d| d.as_os_str()),
        ];
        elevated
            || places
                .into_iter()
                .flatten()
                .any(|place| on_network(place, &network_drive))
    }

    /// The flags of `ShellExecuteEx` for this launch.
    fn mask(&self) -> u32 {
        // The call is not to return before the start is handed over, as
        // the work that follows on the thread may not wait for it, and a
        // thread of its own ends after it. What the user starts counts
        // for the programs the Start menu shows as used often.
        let mut mask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_LOG_USAGE;
        // A verb other than open may come from the context menu of the
        // item only, which the Shell asks only with this flag; it is
        // also what `properties` needs.
        let plain = { self.verb.as_deref() }.is_none_or(|verb| verb.eq_ignore_ascii_case("open"));
        if !plain {
            mask |= SEE_MASK_INVOKEIDLIST;
        }
        if self.quiet {
            mask |= SEE_MASK_FLAG_NO_UI;
        }
        if self.expanding {
            mask |= SEE_MASK_DOENVSUBST;
        }
        if self.process {
            mask |= SEE_MASK_NOCLOSEPROCESS;
        }
        mask
    }

    /// Starts the target on this thread, which is to have COM initialized
    /// apartment-threaded, as the shell thread does.
    fn start_here(&self) -> Result<Option<Process>> {
        let wide = |text: &OsStr| -> Vec<u16> { text.encode_wide().chain([0]).collect() };
        let target = wide(&self.target);
        let arguments = self.arguments.as_deref().map(wide);
        let directory = { self.directory.as_deref() }.map(|directory| wide(directory.as_os_str()));
        let verb = self.verb.as_deref().map(|verb| wide(OsStr::new(verb)));
        let pointer = |text: &Option<Vec<u16>>| {
            text.as_ref()
                .map_or(PCWSTR::null(), |text| PCWSTR(text.as_ptr()))
        };
        let owner = { self.owner.map(|window| HWND(window as *mut c_void)) }
            .or_else(ShellThread::owner)
            .unwrap_or_default();
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: self.mask(),
            hwnd: owner,
            lpVerb: pointer(&verb),
            lpFile: PCWSTR(target.as_ptr()),
            lpParameters: pointer(&arguments),
            lpDirectory: pointer(&directory),
            nShow: self.show.command().0,
            ..Default::default()
        };
        // SAFETY: the structure has its size, and every string it points
        // to is NUL-terminated and outlives the call; a process handle is
        // given only when asked for, and then closed by `Process`.
        unsafe { ShellExecuteExW(&raw mut info) }.map_err(|error| {
            let name = { self.name.clone() }
                .unwrap_or_else(|| format!("`{}`", self.target.to_string_lossy()));
            anyhow!("{name} did not open: {error}")
        })?;
        let process = !info.hProcess.is_invalid() && !info.hProcess.0.is_null();
        Ok(process.then(|| Process(info.hProcess.0 as isize)))
    }
}

/// A process a [`Launch`] started, kept until this is dropped.
#[derive(Debug)]
pub struct Process(isize);

#[allow(dead_code, reason = "for the starts that wait for a program")]
impl Process {
    fn handle(&self) -> HANDLE {
        HANDLE(self.0 as *mut c_void)
    }

    /// The number Windows knows the process by.
    pub fn id(&self) -> u32 {
        // SAFETY: the handle is open for as long as `self` lives.
        unsafe { GetProcessId(self.handle()) }
    }

    /// Waits until the process ends, for at most `timeout`, and says
    /// whether it ended. It blocks: never call it on the thread of GPUI.
    pub fn wait(&self, timeout: Duration) -> bool {
        let milliseconds = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: the handle is open for as long as `self` lives.
        unsafe { WaitForSingleObject(self.handle(), milliseconds) == WAIT_OBJECT_0 }
    }

    /// Has the process run below the normal priority, as the programs
    /// that start with the session do while the user begins to work.
    pub fn lower_priority(&self) -> Result<()> {
        // SAFETY: the handle is open for as long as `self` lives.
        unsafe { SetPriorityClass(self.handle(), BELOW_NORMAL_PRIORITY_CLASS) }
            .map_err(|error| anyhow!("the priority was not lowered: {error}"))
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle was given by ShellExecuteEx for this and is
        // closed once.
        unsafe {
            let _ = CloseHandle(self.handle());
        }
    }
}

/// The window in front, as a number, where it is a window of Tessera.
fn own_window_in_front() -> Option<isize> {
    let mut process = 0u32;
    // SAFETY: the calls only read which window is in front and whose it
    // is; `process` outlives the call that fills it.
    unsafe {
        let window = GetForegroundWindow();
        if window.is_invalid() {
            return None;
        }
        GetWindowThreadProcessId(window, Some(&raw mut process));
        (process == GetCurrentProcessId()).then_some(window.0 as isize)
    }
}

/// Whether `place` is on the network: a UNC path (`\\server\share`,
/// `\\?\UNC\server\share`), or a path on a drive letter that
/// `network_drive` says is one of the network.
fn on_network(place: &OsStr, network_drive: impl Fn(char) -> bool) -> bool {
    let place = place.to_string_lossy();
    let device = ["\\\\?\\", "\\\\.\\"]
        .iter()
        .find_map(|prefix| place.strip_prefix(prefix));
    match device {
        Some(rest) => rest
            .get(..4)
            .is_some_and(|unc| unc.eq_ignore_ascii_case("UNC\\")),
        None if place.starts_with("\\\\") => true,
        None => {
            let mut chars = place.chars();
            match (chars.next(), chars.next()) {
                (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => network_drive(letter),
                _ => false,
            }
        }
    }
}

/// Whether the drive `letter` is a drive of the network.
fn is_network_drive(letter: char) -> bool {
    let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
    // SAFETY: the root is NUL-terminated; the call only reads what kind
    // of drive it names.
    unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_REMOTE }
}

/// `text` with every `%NAME%` replaced by the environment variable of
/// that name; a reference to a variable that is not set stays as it is,
/// and so does the whole text where the replacing fails.
pub(super) fn expand_environment(text: &OsStr) -> OsString {
    if !text.encode_wide().any(|unit| unit == u16::from(b'%')) {
        return text.to_owned();
    }
    let source: Vec<u16> = text.encode_wide().chain([0]).collect();
    // SAFETY: the source is NUL-terminated; without a buffer the call
    // only tells how long the result is.
    let needed = unsafe { ExpandEnvironmentStringsW(PCWSTR(source.as_ptr()), None) } as usize;
    if needed == 0 {
        return text.to_owned();
    }
    let mut expanded = vec![0u16; needed];
    // SAFETY: as above, and the buffer carries its length.
    let written =
        unsafe { ExpandEnvironmentStringsW(PCWSTR(source.as_ptr()), Some(&mut expanded)) } as usize;
    if written == 0 || written > expanded.len() {
        return text.to_owned();
    }
    // The length counts the NUL at the end.
    OsString::from_wide(&expanded[..written - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_asks_the_context_menu_only_for_verbs_other_than_open() {
        let always = SEE_MASK_NOASYNC | SEE_MASK_FLAG_LOG_USAGE;
        let plain = Launch::new("notes.txt");
        assert_eq!(plain.mask(), always);
        assert_eq!(plain.clone().verb("Open").mask(), always);
        let properties = plain.clone().verb("properties").mask();
        assert_eq!(properties, always | SEE_MASK_INVOKEIDLIST);
        let quiet = plain.clone().quiet().mask();
        assert_eq!(quiet, always | SEE_MASK_FLAG_NO_UI);
        let process = plain.process().mask();
        assert_eq!(process, always | SEE_MASK_NOCLOSEPROCESS);
    }

    #[test]
    fn a_target_is_taken_as_written_unless_it_is_a_template() {
        let file = Launch::new("50%OS%.txt");
        assert_eq!(file.mask() & SEE_MASK_DOENVSUBST, 0);
        assert_eq!(file.target, OsString::from("50%OS%.txt"));
        let template = Launch::new(r"%WINDIR%\notepad.exe").expanding();
        assert_ne!(template.mask() & SEE_MASK_DOENVSUBST, 0);
        assert_eq!(template.target, OsString::from(r"%WINDIR%\notepad.exe"));
    }

    #[test]
    fn a_start_that_may_wait_gets_a_thread_of_its_own() {
        let network = |letter: char| letter.eq_ignore_ascii_case(&'n');
        let waits = |launch: Launch| launch.may_wait(network);
        assert!(!waits(Launch::new(r"C:\Tools\tool.exe")));
        assert!(waits(Launch::new(r"C:\Tools\tool.exe").verb("RunAs")));
        assert!(waits(Launch::new(r"\\server\share\setup.exe")));
        assert!(waits(Launch::new(r"\\?\UNC\server\share\a.txt")));
        assert!(!waits(Launch::new(r"\\?\C:\long\a.txt")));
        assert!(waits(Launch::new(r"n:\report.docx")));
        assert!(waits(Launch::new("tool.exe").directory(r"\\server\share")));
        assert!(!waits(Launch::new("https://example.org")));
        assert!(!waits(Launch::new("shell:Downloads")));
    }

    #[test]
    fn an_application_is_started_through_the_folder_of_applications() {
        let launch = Launch::app("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App");
        assert_eq!(
            launch.target,
            OsString::from(r"shell:AppsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App")
        );
        assert_eq!(Launch::new("x").arguments("").arguments, None);
    }

    #[test]
    fn variables_of_the_environment_are_replaced() {
        // SAFETY: the variable is one of this test's own.
        unsafe { std::env::set_var("TESSERA_LAUNCH_TEST", r"C:\Tools") };
        let expanded = expand_environment(OsStr::new(r"%TESSERA_LAUNCH_TEST%\tool.exe"));
        assert_eq!(expanded, OsString::from(r"C:\Tools\tool.exe"));
        let unset = OsStr::new(r"%TESSERA_NOT_SET_ANYWHERE%\a");
        assert_eq!(expand_environment(unset), unset.to_owned());
        assert_eq!(
            expand_environment(OsStr::new("plain")),
            OsString::from("plain")
        );
    }
}
