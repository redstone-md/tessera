// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The command lines of `explorer.exe`, read so that `tessera files`
//! takes the same words: `/select,<path>`, `/e,<path>`, `/n`,
//! `/root,<path>`, `/separate`, `shell:<known folder>`, `::{CLSID}`,
//! `search-ms:` and plain paths.
//!
//! What a command line asks for is first taken apart here, as text only
//! ([`ExplorerArgs::parse`]), and then turned into where the file manager
//! goes ([`ExplorerArgs::destination`]), which asks [`Places`] for what
//! only Windows knows: the paths of known folders and whether a path is a
//! folder. Whatever is no folder of the file system, such as Control
//! Panel, a library or another virtual folder of the Shell, is left to
//! Explorer, with the same words ([`Destination::Explorer`]).

use std::path::{Path, PathBuf};

use crate::files::Location;

/// The class of This PC, which the file manager shows as its drives.
const THIS_PC: &str = "{20d04fe0-3aea-1069-a2d8-08002b30309d}";

/// The classes of Home and of Quick access, which the file manager shows
/// as its Home.
const HOME: [&str; 2] = [
    "{f874310e-b6b7-47dc-bc84-b9e6b38f5903}",
    "{679f85cb-0220-4080-b29b-5540cc05aab6}",
];

/// The names of the known folders that are This PC.
const THIS_PC_NAMES: [&str; 1] = ["mycomputerfolder"];

/// The extensions of files that stand for virtual folders, which stay
/// with Explorer.
const VIRTUAL_FILES: [&str; 2] = ["library-ms", "search-ms"];

/// A switch of the command line of Explorer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Switch {
    Select,
    Explore,
    NewWindow,
    Root,
    Separate,
    /// One Explorer knows that Tessera has no use for, as `/idlist`; the
    /// value after it is passed over.
    Other,
}

impl Switch {
    fn named(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "select" => Self::Select,
            "e" => Self::Explore,
            "n" => Self::NewWindow,
            "root" => Self::Root,
            "separate" => Self::Separate,
            _ => Self::Other,
        }
    }

    /// Whether the switch takes the value after it.
    fn takes_value(self) -> bool {
        matches!(self, Self::Select | Self::Root | Self::Other)
    }
}

/// A piece of the command line: a switch or a value.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Switch(Switch, String),
    Value(String),
}

/// Where a command line points, as it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A path, or a name that `Files::named` reads (`~`, `home`,
    /// `computer`).
    Named(String),
    /// `shell:<name>`, with what follows it after a backslash.
    Shell { name: String, rest: Option<String> },
    /// `::{CLSID}` or `shell:::{CLSID}`, with what follows it.
    Class { id: String, rest: Option<String> },
    /// `search-ms:`: what to look for, and below which folder.
    Search {
        location: Option<String>,
        query: String,
    },
}

/// What a command line of Explorer asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExplorerArgs {
    /// Where to go; with `select`, the item to show in its folder.
    pub target: Option<Target>,
    /// `/select`: the target is chosen in the folder around it.
    pub select: bool,
    /// `/e`: Explorer with its tree, which a window of Tessera always has.
    pub explore: bool,
    /// `/n`: a new window, which Tessera always opens.
    pub new_window: bool,
    /// `/root`: the folder Explorer roots its tree at; Tessera opens it
    /// where nothing else is named.
    pub root: Option<Target>,
    /// `/separate`: a process of its own, which means nothing here.
    pub separate: bool,
}

/// What a known folder is, as Windows says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnownPlace {
    /// A folder of the file system at this path.
    Folder(PathBuf),
    /// A virtual folder, such as Control Panel or a library, or a name
    /// Windows does not know.
    Virtual,
}

/// What only Windows knows of the places a command line names.
pub trait Places {
    /// The known folder of the canonical name `name` (`Downloads`,
    /// `Desktop`, `ControlPanelFolder` and so on).
    fn known(&self, name: &str) -> KnownPlace;
    /// The location a plain word names, as `Files::named` reads it.
    fn named(&self, word: &str) -> Location;
    /// Whether `path` is a folder.
    fn is_dir(&self, path: &Path) -> bool;
    /// Whether `path` is there at all.
    fn exists(&self, path: &Path) -> bool;
}

/// Where the file manager goes for a command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    /// No place is named: a window as `tessera files` opens one.
    Default,
    /// A location of the file manager.
    Open(Location),
    /// The folder around this path, with the path chosen.
    Select(PathBuf),
    /// Nothing the file manager shows: Explorer gets the same words.
    Explorer,
}

impl ExplorerArgs {
    /// Takes `words` apart as Explorer reads its command line. A word may
    /// hold a switch and its value after a comma (`/select,C:\a.txt`), or
    /// several switches (`/e,/root,C:\x`); a value after a switch keeps
    /// its commas, so a name with a comma is read whole.
    pub fn parse(words: &[String]) -> Self {
        let mut args = Self::default();
        let mut tokens = tokens(words).into_iter().peekable();
        while let Some(token) = tokens.next() {
            match token {
                Token::Switch(switch, _) => {
                    let value = match tokens.peek() {
                        Some(Token::Value(_)) if switch.takes_value() => match tokens.next() {
                            Some(Token::Value(value)) => Some(value),
                            _ => None,
                        },
                        _ => None,
                    };
                    match switch {
                        Switch::Select => {
                            args.select = true;
                            if let Some(value) = value {
                                args.target = Some(target_of(&value));
                            }
                        }
                        Switch::Root => args.root = value.as_deref().map(target_of),
                        Switch::Explore => args.explore = true,
                        Switch::NewWindow => args.new_window = true,
                        Switch::Separate => args.separate = true,
                        Switch::Other => {}
                    }
                }
                // The first place named counts, as with Explorer; a
                // selection is not replaced by a place after it.
                Token::Value(value) => {
                    if args.target.is_none() {
                        args.target = Some(target_of(&value));
                    }
                }
            }
        }
        args
    }

    /// Where the file manager goes for these arguments, with `places`
    /// asked what only Windows knows. It may wait for the file system, so
    /// it is not done on the thread of the user interface.
    pub fn destination(&self, places: &dyn Places) -> Destination {
        let Some(target) = self.target.as_ref().or(self.root.as_ref()) else {
            return Destination::Default;
        };
        let place = match place_of(target, places) {
            Some(place) => place,
            None => return Destination::Explorer,
        };
        match place {
            Location::Folder(path) if is_virtual_file(&path) => Destination::Explorer,
            Location::Folder(path) if self.select && self.target.is_some() => {
                Destination::Select(path)
            }
            // A file named without `/select` is shown in its folder, as
            // the address field of the file manager does.
            Location::Folder(path) if !places.is_dir(&path) && places.exists(&path) => {
                Destination::Select(path)
            }
            place => Destination::Open(place),
        }
    }
}

/// The location `target` names, or `None` where only Explorer shows it.
fn place_of(target: &Target, places: &dyn Places) -> Option<Location> {
    match target {
        Target::Named(word) => Some(places.named(word)),
        Target::Shell { name, rest } => {
            let lower = name.to_ascii_lowercase();
            if THIS_PC_NAMES.contains(&lower.as_str()) && rest.is_none() {
                return Some(Location::Computer);
            }
            match places.known(name) {
                KnownPlace::Folder(path) => Some(Location::Folder(joined(path, rest.as_deref()))),
                KnownPlace::Virtual => None,
            }
        }
        Target::Class { id, rest } => {
            let id = id.to_ascii_lowercase();
            match rest.as_deref() {
                None if id == THIS_PC => Some(Location::Computer),
                None if HOME.contains(&id.as_str()) => Some(Location::Home),
                // A path below This PC, as `::{20D04FE0-...}\C:\`, is a path.
                Some(rest) if id == THIS_PC && !rest.starts_with("::") => Some(places.named(rest)),
                _ => None,
            }
        }
        Target::Search { location, query } => {
            let location = location.as_deref()?;
            let root = match places.named(location) {
                Location::Folder(path) => path,
                _ => return None,
            };
            Some(match query.trim().is_empty() {
                true => Location::Folder(root),
                false => Location::Search {
                    root,
                    query: query.trim().to_owned(),
                },
            })
        }
    }
}

/// `path` with `rest` below it, where there is a rest.
fn joined(path: PathBuf, rest: Option<&str>) -> PathBuf {
    match rest {
        Some(rest) if !rest.is_empty() => path.join(rest),
        _ => path,
    }
}

/// Whether `path` is a file that stands for a virtual folder.
fn is_virtual_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            VIRTUAL_FILES
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

/// A word as a program got it from Windows: a path in quotes that ends
/// with a backslash, as `"%1"` makes of a drive (`"C:\"`), comes as
/// `C:"`, since the backslash escapes the quote. That backslash is put
/// back, and quotes around the whole word are taken off.
fn mended(word: &str) -> String {
    let word = word.trim();
    let word = match word.strip_suffix('"') {
        Some(stem) if !stem.contains('"') => format!("{stem}\\"),
        _ => word.to_owned(),
    };
    match word
        .strip_prefix('"')
        .and_then(|word| word.strip_suffix('"'))
    {
        Some(inner) => inner.to_owned(),
        None => word,
    }
}

/// The switches and values of `words`.
fn tokens(words: &[String]) -> Vec<Token> {
    let mut tokens = Vec::new();
    for word in words {
        split_into(&mended(word), &mut tokens);
    }
    tokens
}

/// Adds the switches and the value `word` holds to `tokens`.
fn split_into(word: &str, tokens: &mut Vec<Token>) {
    let word = word.trim_start_matches(',').trim();
    if word.is_empty() {
        return;
    }
    let Some(switch) = word.strip_prefix('/') else {
        tokens.push(Token::Value(word.to_owned()));
        return;
    };
    let (name, rest) = match switch.split_once(',') {
        Some((name, rest)) => (name, rest),
        None => (switch, ""),
    };
    tokens.push(Token::Switch(Switch::named(name), name.to_owned()));
    // What follows a switch is a value, quotes and all, or more switches.
    split_into(&mended(rest), tokens);
}

/// What a value of the command line names.
fn target_of(value: &str) -> Target {
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("search-ms:") {
        return search_of(&value["search-ms:".len()..]);
    }
    if let Some(rest) = lower.strip_prefix("file:") {
        // `file:///C:/x` is a path of this computer, `file://server/x` a
        // share of another.
        let share = rest.starts_with("//") && !rest.starts_with("///");
        let path = rest.trim_start_matches('/');
        let start = value.len() - path.len();
        let path = percent_decoded(&value[start..], false).replace('/', "\\");
        return Target::Named(match share {
            true => format!("\\\\{path}"),
            false => path,
        });
    }
    if lower.starts_with("shell:::") {
        return class_of(&value["shell:".len()..]);
    }
    if lower.starts_with("shell:") {
        let named = &value["shell:".len()..];
        let (name, rest) = split_rest(named);
        return Target::Shell {
            name: name.to_owned(),
            rest,
        };
    }
    if value.starts_with("::") {
        return class_of(value);
    }
    Target::Named(value.to_owned())
}

/// `text` split at its first backslash or slash into a name and what
/// follows; an empty rest is none.
fn split_rest(text: &str) -> (&str, Option<String>) {
    match text.find(['\\', '/']) {
        Some(at) => {
            let rest = text[at + 1..].trim_matches(['\\', '/']);
            (&text[..at], (!rest.is_empty()).then(|| rest.to_owned()))
        }
        None => (text, None),
    }
}

/// `::{CLSID}` with what follows it.
fn class_of(text: &str) -> Target {
    let text = text.trim_start_matches(':');
    let (id, rest) = match text.find('}') {
        Some(end) => (&text[..=end], &text[end + 1..]),
        None => (text, ""),
    };
    let rest = rest.trim_start_matches(['\\', '/']);
    Target::Class {
        id: id.to_owned(),
        rest: (!rest.is_empty()).then(|| rest.to_owned()),
    }
}

/// What a `search-ms:` address asks for: the `query`, the words of its
/// `crumb`s (`System.Generic.String:…`, `name:…`, `System.FileName:…`),
/// and the folder its `location` (a parameter of its own or a crumb)
/// names.
fn search_of(text: &str) -> Target {
    let mut location = None;
    let mut words: Vec<String> = Vec::new();
    for pair in text.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "query" => words.push(percent_decoded(value, true)),
            "location" => location = location.or(Some(percent_decoded(value, true))),
            "crumb" => {
                // A crumb may end with how it counts, as `,include`; a
                // comma of its own value is escaped.
                let crumb = percent_decoded(value.split(',').next().unwrap_or_default(), true);
                let Some((name, said)) = crumb.split_once(':') else {
                    continue;
                };
                let said = said.to_owned();
                match name.to_ascii_lowercase().as_str() {
                    "location" => location = location.or(Some(said)),
                    "name"
                    | "system.generic.string"
                    | "system.filename"
                    | "system.itemnamedisplay" => words.push(said),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    let words: Vec<&str> = { words.iter() }
        .map(|word| word.trim())
        .filter(|word| !word.is_empty())
        .collect();
    Target::Search {
        location,
        query: words.join(" "),
    }
}

/// `text` with its `%xx` escapes turned back into what they stand for,
/// and `+` into a space where `plus_is_space`, as in the parameters of a
/// `search-ms:` address; in a `file:` address `+` is itself. An escape
/// that is no byte stays as written.
fn percent_decoded(text: &str, plus_is_space: bool) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let escaped = (byte == b'%')
            .then(|| text.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (byte, escaped) {
            (_, Some(value)) => {
                decoded.push(value);
                index += 3;
            }
            (b'+', None) if plus_is_space => {
                decoded.push(b' ');
                index += 1;
            }
            (byte, None) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// The names `Files::named` reads as places, not as paths.
const PLACE_NAMES: [&str; 3] = ["~", "home", "computer"];

/// `words` as the same switches and values, one word each, with every
/// value that names a relative path of the file system made absolute by
/// `absolute`, so that a Tessera with another working directory reads
/// them as this process does. Switches, `shell:`, `::{CLSID}`,
/// `search-ms:` and `file:` values, the names of places (`~`, `home`,
/// `computer`) and the value of a switch Tessera has no use for are kept
/// as they are.
pub fn absolute_words(
    words: &[String],
    absolute: &dyn Fn(&Path) -> Option<PathBuf>,
) -> Vec<String> {
    let mut made = Vec::new();
    let mut tokens = tokens(words).into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            Token::Switch(switch, name) => {
                made.push(format!("/{name}"));
                if switch.takes_value()
                    && matches!(tokens.peek(), Some(Token::Value(_)))
                    && let Some(Token::Value(value)) = tokens.next()
                {
                    made.push(match switch {
                        Switch::Other => value,
                        _ => made_absolute(value, absolute),
                    });
                }
            }
            Token::Value(value) => made.push(made_absolute(value, absolute)),
        }
    }
    made
}

/// `value` made absolute by `absolute` where it is a relative path of the
/// file system, and as it is otherwise.
fn made_absolute(value: String, absolute: &dyn Fn(&Path) -> Option<PathBuf>) -> String {
    let lower = value.to_ascii_lowercase();
    let path = matches!(target_of(&value), Target::Named(_))
        && !lower.starts_with("file:")
        && !PLACE_NAMES.contains(&lower.as_str())
        && !value.starts_with("~\\")
        && !value.starts_with("~/")
        && Path::new(&value).is_relative();
    if !path {
        return value;
    }
    { absolute(Path::new(&value)) }
        .and_then(|path| path.into_os_string().into_string().ok())
        .unwrap_or(value)
}

/// The command line that gives `words` to Explorer as it reads them: a
/// value after a switch follows its comma, and a value with a space or a
/// comma is put in quotes.
pub fn explorer_command_line(words: &[String]) -> String {
    let quoted = |value: &str| match value.contains([' ', ',', '\t']) {
        true => format!("\"{value}\""),
        false => value.to_owned(),
    };
    let mut parts: Vec<String> = Vec::new();
    let mut tokens = tokens(words).into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            Token::Switch(switch, name) => {
                let value = match tokens.peek() {
                    Some(Token::Value(_)) if switch.takes_value() => match tokens.next() {
                        Some(Token::Value(value)) => Some(value),
                        _ => None,
                    },
                    _ => None,
                };
                parts.push(match value {
                    Some(value) => format!("/{name},{}", quoted(&value)),
                    None => format!("/{name}"),
                });
            }
            Token::Value(value) => parts.push(quoted(&value)),
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    fn parse(list: &[&str]) -> ExplorerArgs {
        ExplorerArgs::parse(&words(list))
    }

    fn named(text: &str) -> Option<Target> {
        Some(Target::Named(text.to_owned()))
    }

    /// Places as a test sets them: two folders and one file.
    struct Fake;

    impl Places for Fake {
        fn known(&self, name: &str) -> KnownPlace {
            match name.to_ascii_lowercase().as_str() {
                "downloads" => KnownPlace::Folder(PathBuf::from(r"C:\Users\me\Downloads")),
                _ => KnownPlace::Virtual,
            }
        }

        fn named(&self, word: &str) -> Location {
            match word {
                "computer" => Location::Computer,
                "home" => Location::Home,
                _ => Location::Folder(PathBuf::from(word)),
            }
        }

        fn is_dir(&self, path: &Path) -> bool {
            [r"C:\Users\me\Downloads", r"C:\x", r"C:\"]
                .iter()
                .any(|dir| Path::new(dir) == path)
        }

        fn exists(&self, path: &Path) -> bool {
            self.is_dir(path) || path == Path::new(r"C:\x\a.txt")
        }
    }

    fn destination(list: &[&str]) -> Destination {
        parse(list).destination(&Fake)
    }

    #[test]
    fn a_switch_takes_its_value_after_a_comma_or_in_the_next_word() {
        let args = parse(&[r"/select,C:\x\a.txt"]);
        assert!(args.select);
        assert_eq!(args.target, named(r"C:\x\a.txt"));
        let apart = parse(&["/select,", r"C:\x\a.txt"]);
        assert_eq!(apart, args);
        let spaced = parse(&["/select", r"C:\x\a b.txt"]);
        assert_eq!(spaced.target, named(r"C:\x\a b.txt"));
    }

    #[test]
    fn a_value_after_a_switch_keeps_its_commas() {
        let args = parse(&[r"/select,C:\x\a,b.txt"]);
        assert_eq!(args.target, named(r"C:\x\a,b.txt"));
    }

    #[test]
    fn several_switches_may_stand_in_one_word() {
        let args = parse(&[r"/n,/e,C:\x"]);
        assert!(args.new_window && args.explore && !args.select);
        assert_eq!(args.target, named(r"C:\x"));
        let rooted = parse(&[r"/e,/root,C:\x"]);
        assert_eq!(rooted.root, named(r"C:\x"));
        assert_eq!(rooted.target, None);
        assert_eq!(rooted.destination(&Fake), destination(&[r"C:\x"]));
        let separate = parse(&["/separate,", r"C:\x"]);
        assert!(separate.separate);
        assert_eq!(separate.target, named(r"C:\x"));
    }

    #[test]
    fn quotes_and_a_lost_backslash_are_mended() {
        // `"%1"` for `C:\` reaches the program as `C:"`.
        assert_eq!(parse(&["C:\""]).target, named("C:\\"));
        assert_eq!(parse(&["\"C:\\x\""]).target, named(r"C:\x"));
        assert_eq!(
            parse(&["/select,\"C:\\x\\a b.txt\""]).target,
            named(r"C:\x\a b.txt")
        );
        assert_eq!(parse(&[r"/root,C:\x"]).root, named(r"C:\x"));
    }

    #[test]
    fn the_first_place_named_counts() {
        let args = parse(&[r"C:\x", r"D:\y"]);
        assert_eq!(args.target, named(r"C:\x"));
        let chosen = parse(&[r"/select,C:\x\a.txt", r"D:\y"]);
        assert_eq!(chosen.target, named(r"C:\x\a.txt"));
    }

    #[test]
    fn unknown_switches_and_their_values_are_passed_over() {
        let args = parse(&["/idlist,:123:456", r"C:\x"]);
        assert_eq!(args.target, named(r"C:\x"));
        assert!(!args.select);
    }

    #[test]
    fn shell_names_and_classes_are_read() {
        assert_eq!(
            parse(&[r"shell:Downloads\Music"]).target,
            Some(Target::Shell {
                name: "Downloads".to_owned(),
                rest: Some("Music".to_owned())
            })
        );
        assert_eq!(
            parse(&["shell:::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"]).target,
            Some(Target::Class {
                id: "{20D04FE0-3AEA-1069-A2D8-08002B30309D}".to_owned(),
                rest: None
            })
        );
        assert_eq!(
            parse(&[r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\C:\"]).target,
            Some(Target::Class {
                id: "{20D04FE0-3AEA-1069-A2D8-08002B30309D}".to_owned(),
                rest: Some("C:\\".to_owned())
            })
        );
    }

    #[test]
    fn a_search_names_its_words_and_its_folder() {
        let address = "search-ms:displayname=Found&crumb=System.Generic.String%3Ahello%20world&crumb=location:C%3A%5Cx";
        assert_eq!(
            parse(&[address]).target,
            Some(Target::Search {
                location: Some(r"C:\x".to_owned()),
                query: "hello world".to_owned()
            })
        );
        let plain = "search-ms:query=report+2026&location=C%3A%5Cx";
        assert_eq!(
            destination(&[plain]),
            Destination::Open(Location::Search {
                root: PathBuf::from(r"C:\x"),
                query: "report 2026".to_owned()
            })
        );
        // A search without a folder is Explorer's.
        assert_eq!(
            destination(&["search-ms:query=report"]),
            Destination::Explorer
        );
    }

    #[test]
    fn folders_of_the_file_system_go_to_tessera() {
        assert_eq!(
            destination(&[r"C:\x"]),
            Destination::Open(Location::Folder(PathBuf::from(r"C:\x")))
        );
        assert_eq!(
            destination(&[r"shell:Downloads"]),
            Destination::Open(Location::Folder(PathBuf::from(r"C:\Users\me\Downloads")))
        );
        assert_eq!(
            destination(&["shell:MyComputerFolder"]),
            Destination::Open(Location::Computer)
        );
        assert_eq!(
            destination(&["::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"]),
            Destination::Open(Location::Computer)
        );
        assert_eq!(
            destination(&["shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}"]),
            Destination::Open(Location::Home)
        );
        assert_eq!(destination(&[]), Destination::Default);
        assert_eq!(destination(&["/n"]), Destination::Default);
    }

    #[test]
    fn a_file_is_shown_in_its_folder() {
        assert_eq!(
            destination(&[r"/select,C:\x\a.txt"]),
            Destination::Select(PathBuf::from(r"C:\x\a.txt"))
        );
        assert_eq!(
            destination(&[r"C:\x\a.txt"]),
            Destination::Select(PathBuf::from(r"C:\x\a.txt"))
        );
        // A path that is not there opens, and the window says so.
        assert_eq!(
            destination(&[r"C:\gone"]),
            Destination::Open(Location::Folder(PathBuf::from(r"C:\gone")))
        );
    }

    #[test]
    fn virtual_folders_stay_with_explorer() {
        assert_eq!(
            destination(&["shell:ControlPanelFolder"]),
            Destination::Explorer
        );
        assert_eq!(
            destination(&["::{26EE0668-A00A-44D7-9371-BEB064C98683}"]),
            Destination::Explorer
        );
        assert_eq!(
            destination(&["shell:::{645FF040-5081-101B-9F08-00AA002F954E}"]),
            Destination::Explorer
        );
        assert_eq!(
            destination(&[r"C:\Users\me\Libraries\Documents.library-ms"]),
            Destination::Explorer
        );
        assert_eq!(
            destination(&[
                r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\::{21EC2020-3AEA-1069-A2DD-08002B30309D}"
            ]),
            Destination::Explorer
        );
    }

    #[test]
    fn a_file_address_is_a_path() {
        assert_eq!(
            parse(&["file:///C:/My%20Files/x"]).target,
            named(r"C:\My Files\x")
        );
        assert_eq!(
            parse(&["file://server/share/a"]).target,
            named(r"\\server\share\a")
        );
    }

    #[test]
    fn explorer_gets_the_words_as_it_reads_them() {
        let line = explorer_command_line(&words(&[r"/select,C:\x\a b.txt", "/n"]));
        assert_eq!(line, r#"/select,"C:\x\a b.txt" /n"#);
        let line = explorer_command_line(&words(&["shell:ControlPanelFolder"]));
        assert_eq!(line, "shell:ControlPanelFolder");
        let line = explorer_command_line(&words(&["/e,", r"C:\a,b"]));
        assert_eq!(line, r#"/e "C:\a,b""#);
    }

    #[test]
    fn escapes_are_decoded() {
        assert_eq!(percent_decoded("a%20b+c%zz%41", true), "a b c%zzA");
        assert_eq!(percent_decoded("a+b", false), "a+b");
        assert_eq!(percent_decoded("%D0%AF", false), "Я");
    }

    #[test]
    fn a_plus_in_a_file_address_is_itself() {
        assert_eq!(parse(&["file:///C:/C++/src"]).target, named(r"C:\C++\src"));
        assert_eq!(
            parse(&["search-ms:query=a+b&location=C:%5Cx"]).target,
            Some(Target::Search {
                location: Some(r"C:\x".to_owned()),
                query: "a b".to_owned(),
            })
        );
    }

    #[test]
    fn relative_paths_are_made_absolute_and_nothing_else() {
        let absolute = |path: &Path| Some(Path::new(r"C:\here").join(path));
        let made = absolute_words(
            &words(&[r"/select,.\a b.txt", "/e,/root,sub", "/idlist,:1:2"]),
            &absolute,
        );
        assert_eq!(
            made,
            words(&[
                "/select",
                r"C:\here\.\a b.txt",
                "/e",
                "/root",
                r"C:\here\sub",
                "/idlist",
                ":1:2",
            ])
        );
        let kept = [
            r"C:\x",
            "shell:Downloads",
            "::{20d04fe0-3aea-1069-a2d8-08002b30309d}",
            "search-ms:query=x",
            "file:///C:/x",
            "~",
            r"~\Music",
            "home",
            "computer",
        ];
        for word in kept {
            assert_eq!(absolute_words(&words(&[word]), &absolute), words(&[word]));
        }
        // The words made read as the words they were made of.
        let line = words(&[r"/select,C:\x\a,b.txt", "/n"]);
        let made = absolute_words(&line, &absolute);
        assert_eq!(ExplorerArgs::parse(&made), ExplorerArgs::parse(&line));
    }
}
