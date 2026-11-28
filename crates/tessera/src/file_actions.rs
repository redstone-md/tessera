// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The actions of the menus of files, as KDE's service menus are: each
//! `[[file_action]]` of the configuration names a program to run on what is
//! chosen, for the files and folders it fits (by name, kind, extension and
//! how many are chosen), and is offered in the menus of the file manager
//! and of the desktop before the verbs of the Shell, grouped under its
//! `submenu` where it has one.
//!
//! ```toml
//! [[file_action]]
//! name = "Optimize the picture"
//! command = "oxipng.exe -o 4 %F"
//! match = { extensions = ["png"], min = 1 }
//! submenu = "Pictures"
//! icon = "E91B"
//! ```
//!
//! The command is the program and its arguments, in which `%f` is the
//! first path chosen, `%F` all of them, `%d` the folder of the first, `%n`
//! its name, `%N` its name without its extension, `%u` its `file:` address
//! and `%%` a `%`. Paths are put in quotes unless the command has them in
//! quotes already; environment variables such as `%USERPROFILE%` are
//! expanded. With `terminal = true` it runs in a console window that stays
//! open, with `elevated = true` as an administrator, and with `confirm` it
//! asks first. The command starts in the folder of the first item.

pub mod archive;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::Deserialize;

use crate::icons::{Icon, icon_of_pixels, image_of_bytes};
use crate::platform::{IconPixels, Launch, expand_variables};

/// How large the icon of an action is read, in pixels.
const ICON_SIZE: u32 = 32;

/// One `[[file_action]]` of the configuration.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileAction {
    /// What the menu calls it.
    pub name: String,
    /// The program and its arguments, with the placeholders of the module.
    pub command: String,
    /// What it is offered for; for any one or more items without it.
    #[serde(default, rename = "match")]
    pub matching: ActionMatch,
    /// The submenu it stands in, with the other actions of that name.
    #[serde(default)]
    pub submenu: Option<String>,
    /// Its icon: an `.ico` or `.png` file, a program or library with the
    /// number of an icon in it (`shell32.dll,4`), or the code of a glyph
    /// of the Segoe Fluent Icons font (`E91B`).
    #[serde(default)]
    pub icon: Option<String>,
    /// Whether it runs in a console window that stays open.
    #[serde(default)]
    pub terminal: bool,
    /// Whether it runs as an administrator, which Windows asks for.
    #[serde(default)]
    pub elevated: bool,
    /// What is asked before it runs, where it is to be asked.
    #[serde(default)]
    pub confirm: Option<String>,
}

/// Which items an action is for.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MatchKind {
    File,
    Folder,
    #[default]
    Any,
}

/// What the items chosen must be for an action to be offered.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActionMatch {
    /// Patterns of names, any of which each item matches, as `*.png` or
    /// `README*`, in any case.
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub kind: MatchKind,
    /// Extensions, any of which each file has, without the dot.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// How many items are chosen at least and at most.
    #[serde(default)]
    pub min: Option<usize>,
    #[serde(default)]
    pub max: Option<usize>,
    /// Whether it is offered in the menu of a folder's background too,
    /// for the folder shown.
    #[serde(default)]
    pub background: bool,
}

/// An item chosen, as the menu knows it: its path (or the name the Shell
/// parses it by), and whether it is a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chosen {
    pub path: PathBuf,
    pub folder: bool,
}

impl ActionMatch {
    /// Whether the action is offered for `chosen`; `background` says
    /// whether they are the folder of the menu of a background.
    pub fn fits(&self, chosen: &[Chosen], background: bool) -> bool {
        if chosen.is_empty() || (background && !self.background) {
            return false;
        }
        let count = chosen.len();
        let least = self.min.unwrap_or(1).max(1);
        if count < least || self.max.is_some_and(|most| count > most) {
            return false;
        }
        chosen.iter().all(|item| self.fits_item(item))
    }

    fn fits_item(&self, item: &Chosen) -> bool {
        let kind = match self.kind {
            MatchKind::Any => true,
            MatchKind::File => !item.folder,
            MatchKind::Folder => item.folder,
        };
        let name = name_of(&item.path);
        let named = self.names.is_empty() || { self.names.iter() }
            .any(|pattern| glob_matches(pattern, &name));
        let extension = self.extensions.is_empty()
            || (!item.folder
                && Path::new(&name).extension().is_some_and(|ext| {
                    { self.extensions.iter() }.any(|wanted| {
                        wanted
                            .trim_start_matches('.')
                            .eq_ignore_ascii_case(&ext.to_string_lossy())
                    })
                }));
        kind && named && extension
    }
}

/// The name of the item at `path`, or the whole of a name without one.
fn name_of(path: &Path) -> String {
    { path.file_name() }
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Whether `name` matches `pattern`, where `*` stands for any letters and
/// `?` for one, in any case.
pub fn glob_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let name: Vec<char> = name.to_lowercase().chars().collect();
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
            }
            Some(&c) if c == '?' || c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                Some((at, from)) => {
                    p = at + 1;
                    n = from + 1;
                    star = Some((at, from + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

impl FileAction {
    /// Whether the command needs paths of the file system: an item the
    /// Shell names otherwise, as This PC, cannot be given to it.
    pub fn needs_paths(&self) -> bool {
        placeholders(&self.command).any(|letter| matches!(letter, 'f' | 'F' | 'd' | 'u'))
    }

    /// The command line for `chosen`, with the placeholders filled and the
    /// environment variables expanded.
    pub fn line(&self, chosen: &[PathBuf]) -> String {
        expand(&expand_variables(&self.command), chosen)
    }

    /// Runs the action on `chosen`, away from the user interface.
    pub fn run(&self, chosen: &[PathBuf]) {
        let line = self.line(chosen);
        let folder = chosen
            .first()
            .and_then(|first| first.parent())
            .map(Path::to_owned);
        let launch = match self.terminal {
            true => {
                let shell = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_owned());
                // `/s` takes the outer quotes away and keeps the rest as
                // it is written; `/k` keeps the window open.
                Launch::new(shell).arguments(format!("/s /k \"{line}\""))
            }
            false => {
                let (program, arguments) = split_program(&line);
                let launch = Launch::new(program);
                match arguments.is_empty() {
                    true => launch,
                    false => launch.arguments(arguments),
                }
            }
        };
        let launch = launch.named(self.name.clone());
        let launch = match folder {
            Some(folder) if folder.is_absolute() => launch.directory(folder),
            _ => launch,
        };
        match self.elevated {
            true => launch.verb("runas").start(),
            false => launch.start(),
        }
    }

    /// The icon of the action, read from its file where it names one. It
    /// reads the disk: never call it on the thread of the user interface.
    pub fn picture(&self) -> Icon {
        let spec = self.icon.as_deref()?.trim();
        if glyph_of(spec).is_some() {
            return None;
        }
        let spec = expand_variables(spec);
        let (file, index) = match spec.rsplit_once(',') {
            Some((file, index)) if index.trim().parse::<i32>().is_ok() => {
                (file.trim().to_owned(), index.trim().parse().unwrap_or(0))
            }
            _ => (spec.clone(), 0),
        };
        let file = PathBuf::from(file.trim_matches('"'));
        let extension = { file.extension() }.map(|ext| ext.to_string_lossy().to_lowercase());
        match extension.as_deref() {
            Some("png" | "jpg" | "jpeg" | "bmp" | "gif") => {
                image_of_bytes(&std::fs::read(&file).ok()?, ICON_SIZE).ok()?
            }
            _ => icon_of_pixels(&IconPixels::of_file(&file, index, ICON_SIZE)?),
        }
    }

    /// The glyph of the icon font the action is shown with: the one its
    /// icon names, or `fallback`.
    pub fn glyph(&self, fallback: &'static str) -> &'static str {
        self.icon.as_deref().and_then(glyph_of).unwrap_or(fallback)
    }
}

/// The glyphs of the icon font named by the codes of actions, kept for the
/// life of Tessera so that menus can hold them.
static GLYPHS: Mutex<Option<HashMap<u32, &'static str>>> = Mutex::new(None);

/// The glyph of the icon font `spec` names by its code, as `E91B`,
/// `U+E91B` or `\u{e91b}`, or by itself.
fn glyph_of(spec: &str) -> Option<&'static str> {
    let spec = spec.trim();
    let code = { spec.strip_prefix("U+").or_else(|| spec.strip_prefix("u+")) }
        .or_else(|| {
            spec.strip_prefix("\\u{")
                .and_then(|rest| rest.strip_suffix('}'))
        })
        .unwrap_or(spec);
    let character = match code.len() {
        4 | 5 if code.chars().all(|c| c.is_ascii_hexdigit()) => {
            char::from_u32(u32::from_str_radix(code, 16).ok()?)?
        }
        _ => {
            let mut chars = spec.chars();
            let one = chars.next()?;
            // One character of the private area of the font, as written.
            (chars.next().is_none() && ('\u{e000}'..='\u{f8ff}').contains(&one)).then_some(one)?
        }
    };
    let mut glyphs = GLYPHS.lock().unwrap_or_else(PoisonError::into_inner);
    let glyphs = glyphs.get_or_insert_with(HashMap::new);
    let glyph =
        { glyphs.entry(character as u32) }.or_insert_with(|| String::leak(character.to_string()));
    Some(glyph)
}

/// The letters of the placeholders in `command`: `%` and one of `fFdnNu`,
/// not followed by a letter or digit (so that `%USERPROFILE%` is none).
fn placeholders(command: &str) -> impl Iterator<Item = char> + '_ {
    let chars: Vec<char> = command.chars().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if chars[at] == '%' {
            match chars.get(at + 1) {
                Some('%') => {
                    at += 2;
                    continue;
                }
                Some(&letter) if "fFdnNu".contains(letter) => {
                    let after = chars.get(at + 2);
                    if after.is_none_or(|c| !c.is_alphanumeric()) {
                        found.push(letter);
                        at += 2;
                        continue;
                    }
                }
                _ => {}
            }
        }
        at += 1;
    }
    found.into_iter()
}

/// `path` in quotes, unless the command has it between quotes already.
fn quoted(path: &str, in_quotes: bool) -> String {
    match in_quotes {
        true => path.to_owned(),
        false => format!("\"{path}\""),
    }
}

/// `template` with its placeholders filled for `chosen`.
pub fn expand(template: &str, chosen: &[PathBuf]) -> String {
    let chars: Vec<char> = template.chars().collect();
    let first = chosen.first();
    let text = |path: &Path| path.display().to_string();
    let mut line = String::with_capacity(template.len());
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        let next = chars.get(at + 1).copied();
        let after = chars.get(at + 2);
        let placeholder = c == '%'
            && next.is_some_and(|letter| "fFdnNu".contains(letter))
            && after.is_none_or(|c| !c.is_alphanumeric());
        if c == '%' && next == Some('%') {
            line.push('%');
            at += 2;
            continue;
        }
        if !placeholder {
            line.push(c);
            at += 1;
            continue;
        }
        let in_quotes = at > 0 && chars[at - 1] == '"' && after == Some(&'"');
        let filled = match next {
            Some('f') => first.map(|path| quoted(&text(path), in_quotes)),
            Some('F') => Some(
                { chosen.iter() }
                    .map(|path| quoted(&text(path), in_quotes && chosen.len() == 1))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            Some('d') => { first.and_then(|path| path.parent()) }
                .map(|folder| quoted(&text(folder), in_quotes)),
            Some('n') => first.map(|path| name_of(path)),
            Some('N') => first.map(|path| {
                let name = name_of(path);
                match name.rsplit_once('.') {
                    Some((stem, _)) if !stem.is_empty() => stem.to_owned(),
                    _ => name,
                }
            }),
            Some('u') => first.map(|path| file_url(path)),
            _ => None,
        };
        line.push_str(&filled.unwrap_or_default());
        at += 2;
    }
    line
}

/// The `file:` address of `path`: `file:///C:/My%20Files/a.txt`, or
/// `file://server/share/a.txt` for a file of the network.
pub fn file_url(path: &Path) -> String {
    let text = path.display().to_string().replace('\\', "/");
    let (start, rest) = match text.strip_prefix("//") {
        Some(rest) => ("file://", rest.to_owned()),
        None => ("file:///", text),
    };
    let mut url = start.to_owned();
    for byte in rest.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                url.push(byte as char)
            }
            other => url.push_str(&format!("%{other:02X}")),
        }
    }
    url
}

/// The program of a command line and the rest of it: the first part, in
/// quotes where it starts with one.
pub fn split_program(line: &str) -> (String, String) {
    let line = line.trim_start();
    if let Some(rest) = line.strip_prefix('"') {
        return match rest.split_once('"') {
            Some((program, rest)) => (program.to_owned(), rest.trim().to_owned()),
            None => (rest.to_owned(), String::new()),
        };
    }
    match line.split_once(char::is_whitespace) {
        Some((program, rest)) => (program.to_owned(), rest.trim().to_owned()),
        None => (line.to_owned(), String::new()),
    }
}

/// The actions of `all` offered for `chosen`, in their order; those that
/// need paths of the file system are left out where a name chosen is none.
pub fn offered<'a>(
    all: &'a [FileAction],
    chosen: &[Chosen],
    background: bool,
) -> Vec<&'a FileAction> {
    let paths = chosen.iter().all(|item| item.path.is_absolute());
    { all.iter() }
        .filter(|action| !action.name.trim().is_empty() && !action.command.trim().is_empty())
        .filter(|action| paths || !action.needs_paths())
        .filter(|action| action.matching.fits(chosen, background))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chosen(items: &[(&str, bool)]) -> Vec<Chosen> {
        { items.iter() }
            .map(|&(path, folder)| Chosen {
                path: PathBuf::from(path),
                folder,
            })
            .collect()
    }

    #[test]
    fn actions_are_read_from_the_configuration() {
        let text = r#"
            [[file_action]]
            name = "Optimize"
            command = "oxipng.exe %F"
            match = { names = ["*.png"], kind = "file", min = 1, max = 20 }
            submenu = "Pictures"
            icon = "E91B"
            terminal = true
            elevated = false
            confirm = "Optimize the pictures?"
        "#;
        #[derive(Deserialize)]
        struct File {
            file_action: Vec<FileAction>,
        }
        let file: File = toml::from_str(text).unwrap();
        let action = &file.file_action[0];
        assert_eq!(action.matching.kind, MatchKind::File);
        assert_eq!(action.matching.max, Some(20));
        assert_eq!(action.glyph("x"), "\u{e91b}");
        assert!(action.terminal && action.needs_paths());
        assert!(
            toml::from_str::<File>("[[file_action]]\nname = \"a\"\ncommand = \"b\"\nwhat = 1")
                .is_err()
        );
    }

    #[test]
    fn names_match_patterns_in_any_case() {
        assert!(glob_matches("*.png", "Shot.PNG"));
        assert!(glob_matches("readme*", "README.md"));
        assert!(glob_matches("a?c", "abc"));
        assert!(glob_matches("*", ""));
        assert!(!glob_matches("*.png", "shot.png.txt"));
        assert!(glob_matches("*a*b*", "xxaYYbzz"));
        assert!(!glob_matches("a?c", "ac"));
    }

    #[test]
    fn an_action_is_offered_for_what_it_fits() {
        let matching = ActionMatch {
            kind: MatchKind::File,
            extensions: vec![".png".to_owned(), "jpg".to_owned()],
            max: Some(2),
            ..ActionMatch::default()
        };
        assert!(matching.fits(&chosen(&[(r"C:\a.PNG", false)]), false));
        assert!(!matching.fits(&chosen(&[(r"C:\a.gif", false)]), false));
        assert!(!matching.fits(&chosen(&[(r"C:\png", true)]), false));
        let three = chosen(&[
            (r"C:\a.png", false),
            (r"C:\b.png", false),
            (r"C:\c.png", false),
        ]);
        assert!(!matching.fits(&three, false));
        assert!(!matching.fits(&[], false));
        // The background of a folder is offered only what asks for it.
        let folder = chosen(&[(r"C:\Work", true)]);
        assert!(!ActionMatch::default().fits(&folder, true));
        let background = ActionMatch {
            background: true,
            kind: MatchKind::Folder,
            ..ActionMatch::default()
        };
        assert!(background.fits(&folder, true));
    }

    #[test]
    fn items_without_paths_get_only_actions_that_need_none() {
        let named = |name: &str, command: &str| FileAction {
            name: name.to_owned(),
            command: command.to_owned(),
            ..FileAction::default()
        };
        let all = vec![
            named("path", "tool.exe %f"),
            named("name", "tool.exe --name %n"),
            named("folder", "tool.exe %d"),
            named("", "nameless.exe"),
        ];
        let this_pc = chosen(&[("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}", true)]);
        let names: Vec<&str> = { offered(&all, &this_pc, false).into_iter() }
            .map(|action| action.name.as_str())
            .collect();
        assert_eq!(names, ["name"]);
        let file = chosen(&[(r"C:\a.txt", false)]);
        assert_eq!(offered(&all, &file, false).len(), 3);
    }

    #[test]
    fn placeholders_are_filled_and_quoted() {
        let chosen = [
            PathBuf::from(r"C:\My Files\photo.big.jpg"),
            PathBuf::from(r"D:\b.png"),
        ];
        assert_eq!(
            expand("tool %f %d %n %N", &chosen),
            r#"tool "C:\My Files\photo.big.jpg" "C:\My Files" photo.big.jpg photo.big"#
        );
        assert_eq!(
            expand(r#"tool "%f" %F 100%%"#, &chosen),
            r#"tool "C:\My Files\photo.big.jpg" "C:\My Files\photo.big.jpg" "D:\b.png" 100%"#
        );
        assert_eq!(expand("%USERPROFILE% %fx", &chosen), "%USERPROFILE% %fx");
        assert_eq!(
            expand("open %u", &chosen),
            "open file:///C:/My%20Files/photo.big.jpg"
        );
        assert_eq!(
            file_url(Path::new(r"\\server\share\a b.txt")),
            "file://server/share/a%20b.txt"
        );
        let placed: Vec<char> = placeholders("a %f %F %% %USERPROFILE% %N").collect();
        assert_eq!(placed, ['f', 'F', 'N']);
    }

    #[test]
    fn the_program_is_the_first_part_of_the_line() {
        assert_eq!(
            split_program(r#""C:\Program Files\Tool\tool.exe" -x "a b""#),
            (
                r"C:\Program Files\Tool\tool.exe".to_owned(),
                r#"-x "a b""#.to_owned()
            )
        );
        assert_eq!(
            split_program("notepad.exe  a.txt"),
            ("notepad.exe".to_owned(), "a.txt".to_owned())
        );
        assert_eq!(split_program("calc"), ("calc".to_owned(), String::new()));
    }

    #[test]
    fn icons_name_glyphs_by_their_codes() {
        assert_eq!(glyph_of("E91B"), Some("\u{e91b}"));
        assert_eq!(glyph_of("U+e710"), Some("\u{e710}"));
        assert_eq!(glyph_of("\\u{e8a5}"), Some("\u{e8a5}"));
        assert_eq!(glyph_of("\u{e8a5}"), Some("\u{e8a5}"));
        assert_eq!(glyph_of("shell32.dll,4"), None);
        assert_eq!(glyph_of("C:\\icons\\a.ico"), None);
    }
}
