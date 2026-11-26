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
//! Launch::new(r"%WINDIR%\notepad.exe").arguments("todo.txt").start();
//! Launch::new(path).verb("runas").named("Setup").start();
//! Launch::new(path).verb("properties").start();
//! Launch::app("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App").start();
//! Launch::new("https://example.org").start();
//! ```

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_INVOKEIDLIST, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SHOW_WINDOW_CMD, SW_HIDE, SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE, SW_SHOWNORMAL,
};
use windows::core::PCWSTR;

use super::ShellThread;
use super::packaged::app_path;

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
    name: Option<String>,
}

impl Launch {
    /// Starts `target` with its default verb, as a double click does.
    /// References to environment variables, `%NAME%`, are replaced in it
    /// and in the working directory.
    pub fn new(target: impl AsRef<OsStr>) -> Self {
        Self {
            target: target.as_ref().to_owned(),
            arguments: None,
            directory: None,
            verb: None,
            show: Show::Normal,
            quiet: false,
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

    /// What a failure is reported as: the name the user knows the target
    /// by. Without one, the target is named.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Starts the target on the shell thread and returns at once. A
    /// failure is reported.
    pub fn start(self) {
        ShellThread::run(move || {
            if let Err(error) = self.start_here() {
                report!("{error:#}");
            }
        });
    }

    /// The flags of `ShellExecuteEx` for this launch.
    fn mask(&self) -> u32 {
        // The call is not to return before the start is handed over, as
        // the work that follows on the thread may not wait for it.
        let mut mask = SEE_MASK_NOASYNC;
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
        mask
    }

    /// Starts the target on this thread, which is to have COM initialized
    /// apartment-threaded and handle messages, as the shell thread does.
    fn start_here(&self) -> Result<()> {
        let wide = |text: &OsStr| -> Vec<u16> { text.encode_wide().chain([0]).collect() };
        let target = wide(&expand_environment(&self.target));
        let arguments = self.arguments.as_deref().map(wide);
        let directory = { self.directory.as_deref() }
            .map(|directory| wide(&expand_environment(directory.as_os_str())));
        let verb = self.verb.as_deref().map(|verb| wide(OsStr::new(verb)));
        let pointer = |text: &Option<Vec<u16>>| {
            text.as_ref()
                .map_or(PCWSTR::null(), |text| PCWSTR(text.as_ptr()))
        };
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: self.mask(),
            hwnd: ShellThread::owner().unwrap_or_default(),
            lpVerb: pointer(&verb),
            lpFile: PCWSTR(target.as_ptr()),
            lpParameters: pointer(&arguments),
            lpDirectory: pointer(&directory),
            nShow: self.show.command().0,
            ..Default::default()
        };
        // SAFETY: the structure has its size, and every string it points
        // to is NUL-terminated and outlives the call; no process handle
        // is asked for, so none is to be closed.
        unsafe { ShellExecuteExW(&raw mut info) }.map_err(|error| {
            let name = { self.name.clone() }
                .unwrap_or_else(|| format!("`{}`", self.target.to_string_lossy()));
            anyhow!("{name} did not open: {error}")
        })
    }
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
        let plain = Launch::new("notes.txt");
        assert_eq!(plain.mask(), SEE_MASK_NOASYNC);
        assert_eq!(plain.clone().verb("Open").mask(), SEE_MASK_NOASYNC);
        let properties = plain.clone().verb("properties").mask();
        assert_eq!(properties, SEE_MASK_NOASYNC | SEE_MASK_INVOKEIDLIST);
        let quiet = plain.quiet().mask();
        assert_eq!(quiet, SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI);
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
