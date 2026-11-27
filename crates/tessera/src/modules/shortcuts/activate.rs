// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The action `activate-window`: the window of a program, a class, a title
//! or a packaged application used last is brought forward, restored where
//! it is minimized and shown with its workspace where it lives on another
//! one; with `,start` the program is started where it has no window. This
//! is what a shortcut for a window, as KWin gives one, comes to.

use std::collections::HashSet;
use std::path::Path;

use gpui::App;

use crate::actions::Action;
use crate::platform::{AppWindow, AppWindowId, Display, Launch};
use crate::workspaces::Workspaces;

/// The name of the action.
pub const NAME: &str = "activate-window";

/// What `activate-window` looks for in a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sought {
    /// The program file, by its name, as `code.exe`; `.exe` may be left
    /// out, and a path stands for the name it ends with.
    Exe(String),
    /// The window class, as `Notepad`, in any case.
    Class(String),
    /// The title, where `*` stands for any letters and `?` for one, in
    /// any case.
    Title(String),
    /// The packaged application, by its AppUserModelID.
    App(String),
}

/// What the argument of `activate-window` asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub sought: Sought,
    /// Whether the program is started where no window is found.
    pub start: bool,
}

impl Wanted {
    /// What `arg` asks for, or why it asks for nothing.
    pub fn parse(arg: &str) -> Result<Self, String> {
        let arg = arg.trim();
        let (asked, start) = match arg.strip_suffix(",start") {
            Some(asked) => (asked.trim_end(), true),
            None => (arg, false),
        };
        let why = || {
            format!(
                "`{NAME}` takes `exe:`, `class:`, `title:` or `app:` and what to look for, with `,start` after it or not, not `{arg}`"
            )
        };
        let (kind, what) = asked.split_once(':').ok_or_else(why)?;
        let what = what.trim().to_owned();
        if what.is_empty() {
            return Err(why());
        }
        let sought = match kind.trim() {
            "exe" => Sought::Exe(what),
            "class" => Sought::Class(what),
            "title" => Sought::Title(what),
            "app" => Sought::App(what),
            _ => return Err(why()),
        };
        if start && matches!(sought, Sought::Class(_) | Sought::Title(_)) {
            return Err(format!(
                "`,start` needs `exe:` or `app:`, which name what to start; `{arg}` names none"
            ));
        }
        Ok(Self { sought, start })
    }

    /// Whether a window of the program file named `exe`, of the class
    /// `class`, titled `title` and of the packaged application `app` is
    /// one this looks for.
    fn matches(&self, exe: Option<&str>, class: &str, title: &str, app: Option<&str>) -> bool {
        match &self.sought {
            Sought::Exe(sought) => exe.is_some_and(|exe| same_program(sought, exe)),
            Sought::Class(sought) => sought.eq_ignore_ascii_case(class),
            Sought::Title(pattern) => glob(pattern, title),
            Sought::App(sought) => app.is_some_and(|app| sought.eq_ignore_ascii_case(app)),
        }
    }

    /// What is started where no window is found, with `,start`.
    fn started(&self) -> Option<String> {
        match (&self.sought, self.start) {
            (Sought::Exe(exe), true) => Some(exe.clone()),
            (Sought::App(app), true) => Some(format!("shell:AppsFolder\\{app}")),
            _ => None,
        }
    }
}

/// The name of the file a path or a name ends with, in lower case and
/// without `.exe`.
fn program_name(text: &str) -> String {
    let name = Path::new(text.trim()).file_name().map_or_else(
        || text.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let name = name.to_lowercase();
    match name.strip_suffix(".exe") {
        Some(stem) => stem.to_owned(),
        None => name,
    }
}

/// Whether `sought`, as written in the argument, names the program `exe`.
fn same_program(sought: &str, exe: &str) -> bool {
    program_name(sought) == program_name(exe)
}

/// Whether `text` is what `pattern` describes, in any case: `*` stands
/// for any letters, none included, and `?` for exactly one.
pub fn glob(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();
    // Where the last `*` stood, and how far into the text it reached.
    let (mut star, mut reached) = (None, 0);
    let (mut at, mut into) = (0, 0);
    while into < text.len() {
        match pattern.get(at) {
            Some('*') => {
                star = Some(at);
                reached = into;
                at += 1;
            }
            Some(&letter) if letter == '?' || letter == text[into] => {
                at += 1;
                into += 1;
            }
            _ => match star {
                Some(star) => {
                    at = star + 1;
                    reached += 1;
                    into = reached;
                }
                None => return false,
            },
        }
    }
    pattern[at..].iter().all(|&letter| letter == '*')
}

/// The programs that are hosts of the windows of many programs, so that
/// their name tells no window apart from the others; such a window is
/// sought by its class instead.
const SHARED_HOSTS: [&str; 10] = [
    "applicationframehost",
    "java",
    "javaw",
    "python",
    "pythonw",
    "mmc",
    "rundll32",
    "dllhost",
    "wscript",
    "cscript",
];

/// The argument of `activate-window` that seeks a window of the program
/// file named `exe`, of the packaged application `app` and of the class
/// `class`: the application where it is one, the program unless it hosts
/// many, and the class otherwise.
pub fn sought_for(exe: Option<&str>, app: Option<&str>, class: &str) -> Option<String> {
    if let Some(app) = app.filter(|app| !app.is_empty()) {
        return Some(format!("app:{app}"));
    }
    if let Some(exe) = exe.filter(|exe| !SHARED_HOSTS.contains(&program_name(exe).as_str())) {
        return Some(format!("exe:{exe}"));
    }
    (!class.is_empty()).then(|| format!("class:{class}"))
}

/// The argument of `activate-window` that seeks the window `id`.
pub fn sought_for_window(id: AppWindowId) -> Option<String> {
    let window = AppWindow::enumerate()
        .into_iter()
        .find(|window| window.id == id);
    let exe = {
        window
            .as_ref()
            .and_then(|window| window.executable.as_deref())
    }
    .and_then(Path::file_name)
    .map(|name| name.to_string_lossy().into_owned());
    let app = window.and_then(|window| window.app_id);
    sought_for(exe.as_deref(), app.as_deref(), &id.class())
}

/// The action `activate-window`.
pub fn action() -> Action {
    Action::new(|arg, cx| {
        if let Some(wanted) = arg.and_then(|arg| Wanted::parse(arg).ok()) {
            activate(&wanted, cx);
        }
    })
    .taking_reserved_keys()
    .checking(|arg| match arg {
        Some(arg) => Wanted::parse(arg).map(drop),
        None => Err(format!("`{NAME}` takes what to look for as `arg`")),
    })
}

/// Brings forward the window `wanted` asks for that was used last, or
/// starts its program where it asks for that and none is found.
pub fn activate(wanted: &Wanted, cx: &mut App) {
    match find(wanted, cx) {
        Some(id) => Workspaces::show_window(id, cx),
        None => {
            if let Some(started) = wanted.started() {
                Launch::new(started).start();
            }
        }
    }
}

/// The window `wanted` asks for that was used last: of the windows shown,
/// the one nearest the top, and then those of the other workspaces of
/// Tessera's own.
fn find(wanted: &Wanted, cx: &App) -> Option<AppWindowId> {
    let file_name = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    let shown = AppWindow::enumerate();
    for window in &shown {
        let exe = window.executable.as_deref().and_then(file_name);
        let (class, title) = (window.id.class(), window.id.title());
        if wanted.matches(exe.as_deref(), &class, &title, window.app_id.as_deref()) {
            return Some(window.id);
        }
    }
    let seen: HashSet<AppWindowId> = shown.iter().map(|window| window.id).collect();
    let displays = Display::all().into_iter().map(|display| Some(display.key));
    for display in std::iter::once(None).chain(displays) {
        let rows = Workspaces::residents(display, cx).unwrap_or_default();
        for resident in rows.into_iter().flatten() {
            if seen.contains(&resident.id) || !resident.id.exists() {
                continue;
            }
            let exe = resident.program.as_deref().and_then(file_name);
            let (class, title) = (resident.id.class(), resident.id.title());
            if wanted.matches(exe.as_deref(), &class, &title, None) {
                return Some(resident.id);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_argument_names_what_to_look_for() {
        let wanted = Wanted::parse("exe:code.exe").unwrap();
        assert_eq!(wanted.sought, Sought::Exe("code.exe".to_owned()));
        assert!(!wanted.start);
        let started = Wanted::parse("app:Microsoft.WindowsCalculator_8wekyb3d8bbwe!App,start");
        let started = started.unwrap();
        assert!(started.start);
        assert_eq!(
            started.started().as_deref(),
            Some("shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App")
        );
        assert_eq!(
            Wanted::parse("title:*Mozilla Firefox").unwrap().sought,
            Sought::Title("*Mozilla Firefox".to_owned())
        );
        assert!(Wanted::parse("class:Notepad,start").is_err());
        assert!(Wanted::parse("title:").is_err());
        assert!(Wanted::parse("window:x").is_err());
        assert!(Wanted::parse("notepad").is_err());
    }

    #[test]
    fn a_window_is_found_by_its_program_class_title_or_application() {
        let wanted = |arg: &str| Wanted::parse(arg).unwrap();
        let notepad = |wanted: &Wanted| {
            wanted.matches(
                Some("Notepad.exe"),
                "Notepad",
                "notes.txt - Notepad",
                Some("Microsoft.WindowsNotepad_8wekyb3d8bbwe!App"),
            )
        };
        assert!(notepad(&wanted("exe:notepad")));
        assert!(notepad(&wanted("exe:NOTEPAD.EXE")));
        assert!(notepad(&wanted(r"exe:C:\Windows\notepad.exe")));
        assert!(!notepad(&wanted("exe:note")));
        assert!(notepad(&wanted("class:notepad")));
        assert!(notepad(&wanted("title:*- Notepad")));
        assert!(notepad(&wanted("title:notes.???*")));
        assert!(!notepad(&wanted("title:Notepad")));
        assert!(notepad(&wanted(
            "app:microsoft.windowsnotepad_8wekyb3d8bbwe!app"
        )));
        assert!(!wanted("exe:code").matches(None, "x", "y", None));
    }

    #[test]
    fn a_window_is_sought_by_what_tells_it_apart() {
        assert_eq!(
            sought_for(Some("Code.exe"), None, "Chrome_WidgetWin_1").as_deref(),
            Some("exe:Code.exe")
        );
        assert_eq!(
            sought_for(Some("javaw.exe"), None, "SunAwtFrame").as_deref(),
            Some("class:SunAwtFrame")
        );
        assert_eq!(
            sought_for(Some("ApplicationFrameHost.exe"), Some("Calc!App"), "x").as_deref(),
            Some("app:Calc!App")
        );
        assert_eq!(sought_for(None, None, ""), None);
        for sought in ["exe:Code.exe", "class:SunAwtFrame", "app:Calc!App"] {
            assert!(Wanted::parse(sought).is_ok(), "{sought}");
        }
    }

    #[test]
    fn a_glob_stands_for_letters() {
        assert!(glob("*", ""));
        assert!(glob("a*c", "abbbc"));
        assert!(glob("a*c", "ac"));
        assert!(!glob("a*c", "abd"));
        assert!(glob("?b*", "ABcd"));
        assert!(!glob("?", ""));
        assert!(glob("*x*y", "aaxbby"));
        assert!(!glob("*x*y", "aaxbb"));
    }
}
