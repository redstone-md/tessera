// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How a rule names windows, as the AppIdentifier of Seelen UI does: by
//! the file name of their program (`exe`), its whole path (`path`), the
//! packaged application they belong to (`app`), their title and their
//! window class, each matched as `match` says (a pattern with `*` and `?`
//! by default), with `not = { ... }` for what a window must not be, and
//! `any = [ ... ]` and `all = [ ... ]` for names of which one or all are
//! to fit. Small and capital letters are alike everywhere. `type` names
//! the kind of window as its styles tell it (see [`WindowKind`]), which is
//! compared as it is.

use std::fmt;
use std::path::Path;

use regex::{Regex, RegexBuilder};
use serde::Deserialize;

use crate::platform::{AppWindow, WindowKind};

/// How the names of a rule are matched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    /// All of the name is the text.
    Equals,
    /// The name begins with the text.
    Starts,
    /// The name ends with the text.
    Ends,
    /// The text is somewhere in the name.
    Contains,
    /// `*` stands for any text and `?` for one character; a text that
    /// begins with `re:` is a regular expression instead.
    #[default]
    Glob,
    /// The text is a regular expression found in the name.
    Regex,
}

impl MatchMode {
    fn word(self) -> &'static str {
        match self {
            Self::Equals => "equals",
            Self::Starts => "starts",
            Self::Ends => "ends",
            Self::Contains => "contains",
            Self::Glob => "glob",
            Self::Regex => "regex",
        }
    }
}

/// A name of windows as a rule gives it, ready to be matched.
#[derive(Clone, Debug)]
pub struct Pattern {
    text: String,
    mode: MatchMode,
    matcher: Matcher,
}

#[derive(Clone, Debug)]
enum Matcher {
    /// The text in lowercase, matched as the mode says.
    Plain(String),
    /// The pattern in lowercase.
    Glob(String),
    Regex(Regex),
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.mode == other.mode
    }
}

impl Eq for Pattern {}

impl Pattern {
    /// The pattern `text` is as a glob, or as a regular expression after
    /// `re:`, or why it is none.
    pub fn parse(text: &str) -> Result<Self, String> {
        Self::with_mode(text, MatchMode::Glob)
    }

    /// The pattern as it was written.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The pattern `text` is when matched as `mode` says, or why it is
    /// none.
    pub fn with_mode(text: &str, mode: MatchMode) -> Result<Self, String> {
        let regex = |expression: &str| {
            let built = RegexBuilder::new(expression).case_insensitive(true).build();
            built.map_err(|error| format!("`{text}` is no regular expression: {error}"))
        };
        let matcher = match (mode, text.strip_prefix("re:")) {
            (MatchMode::Glob, Some(expression)) => Matcher::Regex(regex(expression)?),
            (MatchMode::Glob, None) => Matcher::Glob(text.to_lowercase()),
            (MatchMode::Regex, _) => Matcher::Regex(regex(text)?),
            _ => Matcher::Plain(text.to_lowercase()),
        };
        Ok(Self {
            text: text.to_owned(),
            mode,
            matcher,
        })
    }

    /// Whether `name` is one the pattern names.
    pub fn matches(&self, name: &str) -> bool {
        match &self.matcher {
            Matcher::Glob(pattern) => glob(pattern, &name.to_lowercase()),
            Matcher::Regex(expression) => expression.is_match(name),
            Matcher::Plain(text) => {
                let name = name.to_lowercase();
                match self.mode {
                    MatchMode::Equals => name == *text,
                    MatchMode::Starts => name.starts_with(text.as_str()),
                    MatchMode::Ends => name.ends_with(text.as_str()),
                    _ => name.contains(text.as_str()),
                }
            }
        }
    }
}

/// Whether all of `text` is what `pattern` describes, `*` standing for any
/// text and `?` for any one character. Both are compared as they are, so
/// what is to be found without regard to case is given in lowercase.
pub fn glob(pattern: &str, text: &str) -> bool {
    let (pattern, text): (Vec<char>, Vec<char>) =
        (pattern.chars().collect(), text.chars().collect());
    let (mut at, mut from) = (0, 0);
    // Where the last star was, and where in the text it began to stand.
    let mut star: Option<(usize, usize)> = None;
    while from < text.len() {
        match pattern.get(at) {
            Some('*') => {
                star = Some((at, from));
                at += 1;
            }
            Some(&one) if one == '?' || one == text[from] => {
                at += 1;
                from += 1;
            }
            _ => match star {
                // The star stands for one character more.
                Some((place, began)) => {
                    at = place + 1;
                    from = began + 1;
                    star = Some((place, began + 1));
                }
                None => return false,
            },
        }
    }
    pattern[at.min(pattern.len())..]
        .iter()
        .all(|&rest| rest == '*')
}

/// What is known of a window to match it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Named {
    /// The file name of its program.
    pub exe: Option<String>,
    /// The whole path of its program.
    pub path: Option<String>,
    /// The packaged application it belongs to.
    pub app: Option<String>,
    pub title: String,
    pub class: String,
    /// The kind of window its styles tell it is.
    pub kind: Option<WindowKind>,
}

impl Named {
    pub fn of(window: &AppWindow) -> Self {
        let path = window.executable.as_deref();
        let exe = path.and_then(Path::file_name);
        Self {
            exe: exe.map(|name| name.to_string_lossy().into_owned()),
            path: path.map(|path| path.to_string_lossy().into_owned()),
            app: window.app_id.clone(),
            title: window.id.title(),
            class: window.id.class(),
            kind: window.id.kind(),
        }
    }
}

/// What of a window a name is matched with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Exe,
    Path,
    App,
    Title,
    Class,
}

impl Field {
    fn of(self, named: &Named) -> Option<&str> {
        match self {
            Self::Exe => named.exe.as_deref(),
            Self::Path => named.path.as_deref(),
            Self::App => named.app.as_deref(),
            Self::Title => Some(&named.title),
            Self::Class => Some(&named.class),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Exe => "exe",
            Self::Path => "path",
            Self::App => "app",
            Self::Title => "title",
            Self::Class => "class",
        }
    }
}

/// Which windows a rule names.
#[derive(Clone, Debug, PartialEq)]
pub enum Condition {
    /// The window has a name that fits: one it has not does not.
    Name(Field, Pattern),
    /// The window is of this kind.
    Kind(WindowKind),
    /// The window is not what the condition names.
    Not(Box<Condition>),
    /// The window is what one of the conditions names; none for an empty
    /// list.
    Any(Vec<Condition>),
    /// The window is what all of the conditions name; every window for an
    /// empty list.
    All(Vec<Condition>),
}

impl Default for Condition {
    fn default() -> Self {
        Self::All(Vec::new())
    }
}

impl Condition {
    /// Whether the window `named` is what this names.
    pub fn holds(&self, named: &Named) -> bool {
        match self {
            Self::Name(field, pattern) => field.of(named).is_some_and(|name| pattern.matches(name)),
            Self::Kind(kind) => named.kind == Some(*kind),
            Self::Not(condition) => !condition.holds(named),
            Self::Any(conditions) => conditions.iter().any(|condition| condition.holds(named)),
            Self::All(conditions) => conditions.iter().all(|condition| condition.holds(named)),
        }
    }

    /// Whether it gives a name at all, so that it does not name every
    /// window.
    pub fn names_something(&self) -> bool {
        match self {
            Self::Name(..) | Self::Kind(_) => true,
            Self::Not(condition) => condition.names_something(),
            Self::Any(conditions) | Self::All(conditions) => {
                conditions.iter().any(Self::names_something)
            }
        }
    }
}

impl fmt::Display for Condition {
    /// The condition written out in one line, the same for the same rule,
    /// which is how what is remembered for a rule is kept.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |formatter: &mut fmt::Formatter<'_>, word: &str, conditions: &[Self]| {
            write!(formatter, "{word}(")?;
            for (index, condition) in conditions.iter().enumerate() {
                if index > 0 {
                    write!(formatter, ", ")?;
                }
                write!(formatter, "{condition}")?;
            }
            write!(formatter, ")")
        };
        match self {
            Self::Name(field, pattern) => {
                let (key, mode) = (field.key(), pattern.mode.word());
                write!(formatter, "{key} {mode} {:?}", pattern.text)
            }
            Self::Kind(kind) => write!(formatter, "type {}", kind.word()),
            Self::Not(condition) => write!(formatter, "not({condition})"),
            Self::Any(conditions) => list(formatter, "any", conditions),
            Self::All(conditions) => list(formatter, "all", conditions),
        }
    }
}

/// A table of names as a rule writes it, and as `not`, `any` and `all`
/// hold them.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Naming {
    exe: Option<String>,
    path: Option<String>,
    app: Option<String>,
    title: Option<String>,
    class: Option<String>,
    /// The kind of window.
    #[serde(rename = "type")]
    kind: Option<WindowKind>,
    /// How the names of this table and of the tables in it are matched,
    /// unless one of those says otherwise.
    #[serde(rename = "match")]
    mode: Option<MatchMode>,
    not: Option<Box<Naming>>,
    #[serde(default)]
    any: Vec<Naming>,
    #[serde(default)]
    all: Vec<Naming>,
}

impl Naming {
    /// The keys a table of names has, which a rule leaves to it.
    pub const KEYS: [&'static str; 10] = [
        "exe", "path", "app", "title", "class", "type", "match", "not", "any", "all",
    ];

    /// The condition the table stands for, its names matched as
    /// `inherited` says unless it says otherwise, or why it is none.
    pub fn condition(&self, inherited: MatchMode) -> Result<Condition, String> {
        let mode = self.mode.unwrap_or(inherited);
        let names = [
            (Field::Exe, &self.exe),
            (Field::Path, &self.path),
            (Field::App, &self.app),
            (Field::Title, &self.title),
            (Field::Class, &self.class),
        ];
        let mut all = Vec::new();
        for (field, text) in names {
            if let Some(text) = text {
                all.push(Condition::Name(field, Pattern::with_mode(text, mode)?));
            }
        }
        if let Some(kind) = self.kind {
            all.push(Condition::Kind(kind));
        }
        if let Some(not) = &self.not {
            all.push(Condition::Not(Box::new(not.condition(mode)?)));
        }
        if !self.any.is_empty() {
            let any = { self.any.iter() }
                .map(|naming| naming.condition(mode))
                .collect::<Result<Vec<_>, _>>()?;
            all.push(Condition::Any(any));
        }
        for naming in &self.all {
            all.push(naming.condition(mode)?);
        }
        Ok(match all.len() {
            1 => all.remove(0),
            _ => Condition::All(all),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(exe: &str, title: &str, class: &str) -> Named {
        Named {
            exe: Some(exe.to_owned()),
            path: Some(format!(r"C:\Program Files\Some\{exe}")),
            app: None,
            title: title.to_owned(),
            class: class.to_owned(),
            kind: Some(WindowKind::Normal),
        }
    }

    fn condition(text: &str) -> Condition {
        toml::from_str::<Naming>(text)
            .unwrap()
            .condition(MatchMode::Glob)
            .unwrap()
    }

    #[test]
    fn a_pattern_stands_for_names() {
        assert!(glob("*", ""));
        assert!(glob("note*.exe", "notepad.exe"));
        assert!(glob("*pad.exe", "notepad.exe"));
        assert!(glob("n?tepad.exe", "notepad.exe"));
        assert!(glob("*e*e*", "notepad.exe"));
        assert!(!glob("note*.exe", "notepad.com"));
        assert!(!glob("n?tepad", "notepad.exe"));
        assert!(!glob("", "a"));
        let pattern = Pattern::with_mode("*Picture-in-Picture*", MatchMode::Glob).unwrap();
        assert!(pattern.matches("Firefox — picture-in-picture"));
        assert!(!pattern.matches("Firefox"));
        let expression = Pattern::with_mode("re:^(code|cursor)\\.exe$", MatchMode::Glob).unwrap();
        assert!(expression.matches("Code.exe"));
        assert!(!expression.matches("vscode.exe"));
        assert!(Pattern::with_mode("re:(", MatchMode::Glob).is_err());
    }

    #[test]
    fn each_mode_matches_as_it_says() {
        let mode = |text: &str, mode| Pattern::with_mode(text, mode).unwrap();
        assert!(mode("Notepad.exe", MatchMode::Equals).matches("notepad.EXE"));
        assert!(!mode("Notepad", MatchMode::Equals).matches("notepad.exe"));
        assert!(mode("note", MatchMode::Starts).matches("Notepad.exe"));
        assert!(!mode("pad", MatchMode::Starts).matches("Notepad.exe"));
        assert!(mode(".EXE", MatchMode::Ends).matches("notepad.exe"));
        assert!(!mode("note", MatchMode::Ends).matches("notepad.exe"));
        assert!(mode("tepa", MatchMode::Contains).matches("Notepad.exe"));
        assert!(!mode("x", MatchMode::Contains).matches("Notepad.exe"));
        // A star is a star where the mode is not `glob`.
        assert!(!mode("*", MatchMode::Equals).matches("notepad.exe"));
        assert!(mode("*", MatchMode::Glob).matches("notepad.exe"));
        assert!(mode("^note.*exe$", MatchMode::Regex).matches("NOTEPAD.exe"));
        assert!(Pattern::with_mode("(", MatchMode::Regex).is_err());
        // `re:` is a regular expression only where the mode is `glob`.
        assert!(mode("re:x", MatchMode::Contains).matches("are:x"));
    }

    #[test]
    fn names_of_one_table_all_have_to_fit() {
        let both = condition("exe = \"code.exe\"\ntitle = \"*Visual Studio Code\"");
        assert!(both.holds(&named(
            "Code.exe",
            "main.rs - Visual Studio Code",
            "Chrome_WidgetWin_1"
        )));
        assert!(!both.holds(&named("Code.exe", "Settings", "Chrome_WidgetWin_1")));
        assert!(!both.holds(&named("notepad.exe", "x - Visual Studio Code", "Notepad")));
    }

    #[test]
    fn a_path_names_the_whole_program() {
        let path = condition("path = 'C:\\Program Files\\Some\\*'");
        assert!(path.holds(&named("tool.exe", "", "")));
        let unknown = Named::default();
        assert!(!path.holds(&unknown));
    }

    #[test]
    fn not_takes_away_what_it_names() {
        let not = condition("exe = \"chrome.exe\"\nnot = { title = \"*Picture-in-picture*\" }");
        assert!(not.holds(&named("chrome.exe", "New Tab", "Chrome_WidgetWin_1")));
        assert!(!not.holds(&named(
            "chrome.exe",
            "Picture-in-picture",
            "Chrome_WidgetWin_1"
        )));
        assert!(!not.holds(&named("firefox.exe", "New Tab", "MozillaWindowClass")));
    }

    #[test]
    fn any_needs_one_of_its_names() {
        let any = condition("any = [{ exe = \"chrome.exe\" }, { exe = \"msedge.exe\" }]");
        assert!(any.holds(&named("chrome.exe", "", "")));
        assert!(any.holds(&named("msedge.exe", "", "")));
        assert!(!any.holds(&named("firefox.exe", "", "")));
        assert!(!Condition::Any(Vec::new()).holds(&Named::default()));
    }

    #[test]
    fn all_needs_every_one_of_its_names() {
        let all = condition("all = [{ exe = \"steam*.exe\" }, { class = \"SDL_app\" }]");
        assert!(all.holds(&named("steamwebhelper.exe", "", "SDL_app")));
        assert!(!all.holds(&named("steamwebhelper.exe", "", "Chrome")));
        assert!(Condition::All(Vec::new()).holds(&Named::default()));
    }

    #[test]
    fn a_mode_goes_down_into_the_tables_in_it() {
        let inherited = condition(
            "match = \"equals\"\nany = [{ exe = \"a.exe\" }, { exe = \"b\", match = \"starts\" }]",
        );
        assert!(inherited.holds(&named("A.exe", "", "")));
        assert!(!inherited.holds(&named("a.exe.bak", "", "")));
        assert!(inherited.holds(&named("b.exe", "", "")));
        let nested = condition("exe = \"x*\"\nnot = { match = \"contains\", title = \"setup\" }");
        assert!(nested.holds(&named("xtool.exe", "Main", "")));
        assert!(!nested.holds(&named("xtool.exe", "Setup wizard", "")));
    }

    #[test]
    fn a_table_is_checked() {
        let naming = |text: &str| toml::from_str::<Naming>(text);
        assert!(naming("exe = \"a\"\nfloat = true").is_err());
        assert!(naming("match = \"fuzzy\"").is_err());
        let bad = naming("not = { title = \"(\", match = \"regex\" }").unwrap();
        assert!(bad.condition(MatchMode::Glob).is_err());
        assert!(!condition("").names_something());
        assert!(!condition("any = []").names_something());
        assert!(condition("not = { exe = \"a\" }").names_something());
    }

    #[test]
    fn a_type_names_the_kind_of_window() {
        let dialog = condition("type = \"dialog\"");
        let mut window = named("app.exe", "Save as", "#32770");
        assert!(!dialog.holds(&window));
        window.kind = Some(WindowKind::Dialog);
        assert!(dialog.holds(&window));
        window.kind = None;
        assert!(!dialog.holds(&window));
        assert!(dialog.names_something());
        let not_splash = condition("exe = \"app.exe\"\nnot = { type = \"splash\" }");
        assert!(not_splash.holds(&named("app.exe", "", "")));
        assert_eq!(dialog.to_string(), "type dialog");
        assert!(toml::from_str::<Naming>("type = \"window\"").is_err());
    }

    #[test]
    fn a_condition_is_written_out_the_same_each_time() {
        let text = "exe = \"a.exe\"\nnot = { title = \"b\", match = \"equals\" }";
        assert_eq!(condition(text).to_string(), condition(text).to_string());
        assert_eq!(
            condition(text).to_string(),
            "all(exe glob \"a.exe\", not(title equals \"b\"))"
        );
    }
}
