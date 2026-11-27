// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Commands: what is typed is run where it names a program, a file, a
//! folder or an address, as the Run dialog of Windows runs it. A text
//! after `>` runs in a terminal.
//!
//! A text is understood the way the Run dialog understands it: `%NAME%`
//! is replaced by the environment variable, a program in quotes is that
//! program, and otherwise the longest start of the text that names a
//! file, a folder or a program is what runs, with the rest as its
//! arguments. A program is found by its name alone among the App Paths
//! of the registry and in the folders of `PATH`. Addresses are those
//! with a scheme (`https://`, `mailto:`, `ms-settings:`), the places of
//! the Shell (`shell:Downloads`, `::{GUID}`), and those starting with
//! `www.`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::platform::{Launch, expand_variables, find_program};

use super::{Answer, AnswerAction, Query, Runner, fill};

const ICON_RUN: &str = "\u{e768}";
const ICON_FOLDER: &str = "\u{e8b7}";
const ICON_FILE: &str = "\u{e8a5}";
const ICON_LINK: &str = "\u{e71b}";
const ICON_TERMINAL: &str = "\u{e756}";

/// The scores of a command written to be run, and of a word that only
/// happens to name a program, which comes after the applications.
const SCORE_SURE: u16 = 850;
const SCORE_WORD: u16 = 300;
const SCORE_TERMINAL: u16 = 950;

/// The schemes of addresses that have no `//`.
const BARE_SCHEMES: [&str; 6] = [
    "mailto:",
    "ms-settings:",
    "shell:",
    "tel:",
    "news:",
    "steam:",
];

/// Something typed that can be run, and how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// The program, file, folder or address.
    pub target: String,
    pub arguments: Option<String>,
    /// Whether the target is a folder, which the file manager of Tessera
    /// may open.
    pub folder: bool,
    /// Whether it runs in a terminal unless told otherwise.
    pub terminal: bool,
    /// What was typed, with the variables replaced.
    pub typed: String,
}

impl Command {
    /// What `text` runs, if it names a program, a file, a folder or an
    /// address. It looks at the disk and the registry: not for the
    /// thread of GPUI.
    pub fn understand(text: &str) -> Option<Self> {
        let typed = expand_variables(text.trim());
        if typed.is_empty() {
            return None;
        }
        let command = |target: String, arguments: Option<&str>, folder: bool| Self {
            target,
            arguments: arguments.map(str::to_owned),
            folder,
            terminal: false,
            typed: typed.clone(),
        };
        if let Some(address) = address(&typed) {
            return Some(command(address, None, false));
        }
        for (start, rest) in splits(&typed) {
            let path = local_path(start);
            if let Some(path) = &path
                && let Ok(metadata) = fs::metadata(path)
            {
                let target = path.to_string_lossy().into_owned();
                return Some(command(target, rest, metadata.is_dir()));
            }
            if path.is_none()
                && let Some(program) = find_program(start)
            {
                return Some(command(program.to_string_lossy().into_owned(), rest, false));
            }
        }
        None
    }

    /// `text` as it is typed, with the variables replaced, for the Shell
    /// to make of it what it can where nothing in it was found.
    pub fn as_typed(text: &str) -> Self {
        let typed = expand_variables(text.trim());
        Self {
            target: typed.clone(),
            arguments: None,
            folder: false,
            terminal: false,
            typed,
        }
    }

    /// A command that runs `line` in a terminal.
    pub fn in_terminal(line: &str) -> Self {
        let typed = expand_variables(line.trim());
        Self {
            target: typed.clone(),
            arguments: None,
            folder: false,
            terminal: true,
            typed,
        }
    }

    /// Whether the target is an address rather than a program or a file.
    pub fn is_address(&self) -> bool {
        address(&self.target).is_some()
    }

    /// The target as a path.
    pub fn path(&self) -> PathBuf {
        PathBuf::from(&self.target)
    }

    /// The whole command line, as a terminal is to run it.
    pub fn line(&self) -> String {
        self.typed.clone()
    }

    /// What starts the command; with `admin`, as an administrator.
    pub fn launch(&self, admin: bool) -> Launch {
        let mut launch = Launch::new(&self.target).named(format!("`{}`", self.typed));
        if let Some(arguments) = &self.arguments {
            launch = launch.arguments(arguments);
        }
        if admin {
            launch = launch.verb("runas");
        }
        launch
    }
}

/// `text` as an address to open, where it is one.
fn address(text: &str) -> Option<String> {
    if text.contains(char::is_whitespace) {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    if let Some(at) = lower.find("://") {
        let scheme = &lower[..at];
        let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.');
        let valid = scheme.len() > 1 && scheme.chars().all(plain) && at + 3 < text.len();
        return valid.then(|| text.to_owned());
    }
    if BARE_SCHEMES
        .iter()
        .any(|scheme| lower.starts_with(scheme) && lower.len() > scheme.len())
    {
        return Some(text.to_owned());
    }
    if text.starts_with("::{") {
        return Some(text.to_owned());
    }
    let host = lower.strip_prefix("www.")?;
    host.contains('.').then(|| format!("https://{text}"))
}

/// The ways `text` may be split into what runs and its arguments, the
/// longest first: a program in quotes, with what follows; or else each
/// start of the text that ends before a space, from the whole of it down
/// to its first word.
fn splits(text: &str) -> Vec<(&str, Option<&str>)> {
    fn rest(rest: &str) -> Option<&str> {
        let rest = rest.trim();
        (!rest.is_empty()).then_some(rest)
    }
    if let Some(quoted) = text.strip_prefix('"') {
        return match quoted.split_once('"') {
            Some((program, after)) => vec![(program, rest(after))],
            None => vec![(quoted, None)],
        };
    }
    let mut splits = vec![(text, None)];
    let spaces = text.char_indices().filter(|(_, c)| c.is_whitespace());
    let mut ends: Vec<usize> = spaces.map(|(at, _)| at).collect();
    ends.reverse();
    for end in ends {
        let start = text[..end].trim_end();
        if !start.is_empty() && splits.last().is_none_or(|(last, _)| *last != start) {
            splits.push((start, rest(&text[end..])));
        }
    }
    splits
}

/// The path `text` names where it names one: a path from a drive or a
/// server, one from the folder of the user (`~`, `.` and what has a
/// folder in it), which is where the Run dialog starts. A bare word
/// names no path: it is a program to look for.
fn local_path(text: &str) -> Option<PathBuf> {
    let home = || std::env::var_os("USERPROFILE").map(PathBuf::from);
    let bytes = text.as_bytes();
    let from_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if from_drive || text.starts_with("\\\\") {
        return Some(PathBuf::from(text));
    }
    if text == "~" {
        return home();
    }
    if let Some(rest) = text.strip_prefix("~\\").or_else(|| text.strip_prefix("~/")) {
        return Some(home()?.join(rest));
    }
    if text.starts_with('.') || text.contains(['\\', '/']) {
        return Some(home()?.join(text));
    }
    None
}

/// The entries of the folder that `prefix` is typed into whose names
/// start with what follows its last `\`, folders with a `\` after them,
/// at most `most` of them in the order of their names; hidden ones are
/// left out. It reads the disk: not for the thread of GPUI.
pub fn completions(prefix: &str, most: usize) -> Vec<String> {
    let Some(at) = prefix.rfind(['\\', '/']) else {
        return Vec::new();
    };
    let (folder, stem) = (&prefix[..=at], prefix[at + 1..].to_lowercase());
    let Some(path) = local_path(folder) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&path) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let metadata = entry.metadata().ok()?;
            let shown = !name.starts_with('.') && !crate::platform::is_hidden(&metadata);
            (shown && name.to_lowercase().starts_with(&stem)).then(|| {
                let slash = if metadata.is_dir() { "\\" } else { "" };
                format!("{folder}{name}{slash}")
            })
        })
        .collect();
    found.sort_by_cached_key(|name| name.to_lowercase());
    found.truncate(most);
    found
}

/// Whether `path` is the folder of a drive or one in it, which needs no
/// look at the disk to be shown as such.
fn shows_as_folder(path: &Path) -> bool {
    path.to_string_lossy().ends_with(['\\', '/'])
}

pub struct Commands;

impl Runner for Commands {
    fn id(&self) -> &'static str {
        "command"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        if let Some(line) = query.text().strip_prefix('>') {
            let line = line.trim();
            if line.is_empty() {
                return Vec::new();
            }
            let command = Command::in_terminal(line);
            let title = command.typed.clone();
            let detail = query.say("Runs in a terminal");
            let answer = Answer::new(title, detail, ICON_TERMINAL, SCORE_TERMINAL, {
                AnswerAction::Run(command)
            });
            return vec![answer.keyed(format!("terminal:{}", line.to_lowercase()))];
        }
        let Some(command) = Command::understand(query.text()) else {
            return Vec::new();
        };
        // A bare word that names a program may be meant for the search
        // of applications as well.
        let written = command.arguments.is_some()
            || command.typed.starts_with('"')
            || local_path(&command.typed).is_some()
            || address(&command.typed).is_some();
        let score = if written { SCORE_SURE } else { SCORE_WORD };
        let (glyph, detail) = if address(&command.typed).is_some() {
            (ICON_LINK, query.say("Opens the address"))
        } else if command.folder || shows_as_folder(&command.path()) {
            (ICON_FOLDER, query.say("Opens the folder"))
        } else if is_program(&command.target) {
            (ICON_RUN, query.say("Runs the program"))
        } else {
            (ICON_FILE, query.say("Opens the file"))
        };
        let detail = match command.target == command.typed {
            true => detail.to_owned(),
            false => fill("{} · {}", &[detail, &command.target]),
        };
        let key = format!("command:{}", command.typed.to_lowercase());
        let title = command.typed.clone();
        let mut answer = Answer::new(
            title,
            detail,
            glyph,
            score,
            AnswerAction::Run(command.clone()),
        );
        if is_program(&command.target) {
            answer = answer.with_icon(command.path());
        }
        vec![answer.keyed(key)]
    }
}

/// Whether `target` is a program file, by its extension.
fn is_program(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    [".exe", ".com", ".bat", ".cmd", ".msc", ".cpl"]
        .iter()
        .any(|extension| lower.ends_with(extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_known_by_their_scheme() {
        assert_eq!(
            address("https://example.org/a?b").as_deref(),
            Some("https://example.org/a?b")
        );
        assert_eq!(
            address("ms-settings:display").as_deref(),
            Some("ms-settings:display")
        );
        assert_eq!(
            address("shell:Downloads").as_deref(),
            Some("shell:Downloads")
        );
        assert_eq!(
            address("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}").as_deref(),
            Some("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}")
        );
        assert_eq!(
            address("www.example.org").as_deref(),
            Some("https://www.example.org")
        );
        assert_eq!(address("C:\\Windows"), None);
        assert_eq!(address("notepad"), None);
        assert_eq!(address("shell:"), None);
        assert_eq!(address("a://"), None);
        assert_eq!(address("see https://example.org"), None);
    }

    #[test]
    fn a_command_is_split_from_the_longest_start_down() {
        assert_eq!(
            splits(r"C:\Program Files\App\app.exe -x"),
            [
                (r"C:\Program Files\App\app.exe -x", None),
                (r"C:\Program Files\App\app.exe", Some("-x")),
                (r"C:\Program", Some(r"Files\App\app.exe -x")),
            ]
        );
        assert_eq!(
            splits(r#""C:\Program Files\App\app.exe" -x  y"#),
            [(r"C:\Program Files\App\app.exe", Some("-x  y"))]
        );
        assert_eq!(splits("notepad"), [("notepad", None)]);
        assert_eq!(splits(r#""unclosed"#), [("unclosed", None)]);
    }

    #[test]
    fn paths_are_from_a_drive_a_server_or_the_folder_of_the_user() {
        assert_eq!(local_path(r"D:\music"), Some(PathBuf::from(r"D:\music")));
        assert_eq!(
            local_path(r"\\server\share"),
            Some(PathBuf::from(r"\\server\share"))
        );
        assert_eq!(local_path("notepad"), None);
        if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
            assert_eq!(local_path("~"), Some(home.clone()));
            assert_eq!(local_path(r"~\Documents"), Some(home.join("Documents")));
            assert_eq!(
                local_path(r"Documents\a.txt"),
                Some(home.join(r"Documents\a.txt"))
            );
        }
    }

    #[test]
    fn a_folder_completes_with_its_entries() {
        let folder = std::env::temp_dir().join(format!("tessera-run-{}", std::process::id()));
        fs::create_dir_all(folder.join("Reports")).unwrap();
        fs::write(folder.join("readme.txt"), "").unwrap();
        fs::write(folder.join("other.txt"), "").unwrap();
        let typed = format!("{}\\re", folder.display());
        let found = completions(&typed, 10);
        fs::remove_dir_all(&folder).unwrap();
        let prefix = format!("{}\\", folder.display());
        assert_eq!(
            found,
            [format!("{prefix}readme.txt"), format!("{prefix}Reports\\")]
        );
        assert!(completions("no folder here", 10).is_empty());
    }

    #[test]
    fn programs_are_known_by_their_extension() {
        assert!(is_program(r"C:\Windows\System32\NOTEPAD.EXE"));
        assert!(is_program("devmgmt.msc"));
        assert!(!is_program(r"C:\notes.txt"));
    }
}
