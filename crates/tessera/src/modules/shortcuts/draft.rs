// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A key being made or changed on the Shortcuts page: its press, and what
//! it does, of the kinds a `[[key]]` has: an action with its argument, a
//! program with its arguments, a popup, words for the tiling, a side to
//! bring forward or move to, a way through the windows, a button of the
//! dock or a folder.

use gpui::{App, AppContext, Context, Entity, SharedString};

use crate::config::KeyConfig;
use crate::field::TextField;
use crate::i18n::Language;

/// What kind of thing a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Action,
    Program,
    Popup,
    Tiling,
    Focus,
    Move,
    Switch,
    Dock,
    Files,
}

impl Kind {
    pub const ALL: [Self; 9] = [
        Self::Action,
        Self::Program,
        Self::Popup,
        Self::Tiling,
        Self::Focus,
        Self::Move,
        Self::Switch,
        Self::Dock,
        Self::Files,
    ];

    /// What the page calls it, in English.
    pub fn title(self) -> &'static str {
        match self {
            Self::Action => "Action",
            Self::Program => "Program",
            Self::Popup => "Popup",
            Self::Tiling => "Tiling",
            Self::Focus => "Focus",
            Self::Move => "Move",
            Self::Switch => "Windows",
            Self::Dock => "Dock",
            Self::Files => "Files",
        }
    }

    /// The kind of what `key` does; an action where it names nothing.
    pub fn of(key: &KeyConfig) -> Self {
        let kinds = [
            (key.run.is_some(), Self::Program),
            (key.popup.is_some(), Self::Popup),
            (key.tiling.is_some(), Self::Tiling),
            (key.focus.is_some(), Self::Focus),
            (key.carry.is_some(), Self::Move),
            (key.switch.is_some(), Self::Switch),
            (key.dock.is_some(), Self::Dock),
            (key.files.is_some(), Self::Files),
        ];
        { kinds.into_iter() }
            .find(|(given, _)| *given)
            .map_or(Self::Action, |(_, kind)| kind)
    }

    /// The words chosen among for the kinds that take one of a few.
    pub fn choices(self) -> &'static [&'static str] {
        match self {
            Self::Focus | Self::Move => &["left", "right", "up", "down"],
            Self::Switch => &["next", "previous"],
            Self::Dock => &["1", "2", "3", "4", "5", "6", "7", "8", "9"],
            _ => &[],
        }
    }

    /// What the field of the kind holds, in English, where it has one.
    pub fn field(self) -> Option<&'static str> {
        match self {
            Self::Action => Some("Argument"),
            Self::Popup => Some("The name of a popup"),
            Self::Tiling => Some("The words of a tiling key"),
            Self::Files => Some("A folder"),
            _ => None,
        }
    }
}

/// The arguments typed after a program, taken apart where spaces stand
/// outside quotes, the quotes left out.
pub fn split_arguments(text: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let (mut quoted, mut begun) = (false, false);
    for letter in text.chars() {
        match letter {
            '"' => {
                quoted = !quoted;
                begun = true;
            }
            ' ' | '\t' if !quoted => {
                if begun {
                    arguments.push(std::mem::take(&mut current));
                    begun = false;
                }
            }
            _ => {
                current.push(letter);
                begun = true;
            }
        }
    }
    if begun {
        arguments.push(current);
    }
    arguments
}

/// `arguments` as they are typed, one with a space in quotes.
pub fn joined_arguments(arguments: &[String]) -> String {
    let quoted = arguments.iter().map(|argument| {
        match argument.is_empty() || argument.contains([' ', '\t']) {
            true => format!("\"{argument}\""),
            false => argument.clone(),
        }
    });
    quoted.collect::<Vec<String>>().join(" ")
}

/// The `[[key]]` that presses `press` and does what `kind` with the other
/// parts says: the action `action` with `value` as its argument, the
/// program `program` with `arguments`, the popup, tiling words or folder
/// `value`, or the side, way or number `choice`.
pub fn key_of(
    kind: Kind,
    press: &str,
    action: Option<&str>,
    value: &str,
    program: &str,
    arguments: &str,
    choice: Option<&str>,
) -> KeyConfig {
    let value = value.trim();
    let text = || (!value.is_empty()).then(|| value.to_owned());
    let chosen = || choice.map(str::to_owned);
    let mut key = KeyConfig {
        press: press.trim().to_owned(),
        ..KeyConfig::default()
    };
    match kind {
        Kind::Action => {
            key.action = action.map(str::to_owned);
            key.arg = text();
        }
        Kind::Program => {
            let program = program.trim().trim_matches('"');
            let mut run = vec![program.to_owned()];
            run.extend(split_arguments(arguments));
            key.run = (!program.is_empty()).then_some(run);
        }
        Kind::Popup => key.popup = text(),
        Kind::Tiling => key.tiling = text(),
        Kind::Files => key.files = text(),
        Kind::Focus => key.focus = chosen(),
        Kind::Move => key.carry = chosen(),
        Kind::Switch => key.switch = chosen(),
        Kind::Dock => key.dock = choice.and_then(|number| number.parse().ok()),
    }
    key
}

/// A key being made, or a `[[key]]` of the user being changed.
pub struct Draft {
    /// The entry it changes, and what that entry was, so that a file
    /// changed meanwhile is not written over; `None` for a new key.
    pub changes: Option<(usize, KeyConfig)>,
    pub kind: Kind,
    pub press: Entity<TextField>,
    /// The action chosen, for [`Kind::Action`].
    pub action: Option<String>,
    /// What finds an action among them all.
    pub finder: Entity<TextField>,
    /// The argument, the popup, the tiling words or the folder.
    pub value: Entity<TextField>,
    pub program: Entity<TextField>,
    pub arguments: Entity<TextField>,
    /// The side, the way or the number chosen.
    pub choice: Option<String>,
}

impl Draft {
    /// A draft of `key`, the user's entry at its place, or of a new key
    /// that does what `like` does where it is given.
    pub fn new<V: 'static>(
        changes: Option<(usize, KeyConfig)>,
        like: Option<&KeyConfig>,
        cx: &mut Context<V>,
    ) -> Self {
        let language = Language::of(cx);
        let field = |placeholder: &'static str, cx: &mut Context<V>| {
            let placeholder = SharedString::new_static(language.say(placeholder));
            let field = cx.new(|cx| TextField::new(placeholder, cx));
            cx.observe(&field, |_, _, cx| cx.notify()).detach();
            field
        };
        let press = field("The keys, as ctrl+alt+k", cx);
        let finder = field("Find an action", cx);
        let value = field("Argument", cx);
        let program = field("The program to start", cx);
        let arguments = field("Its arguments", cx);
        let key = changes.as_ref().map(|(_, key)| key).or(like).cloned();
        let mut draft = Self {
            changes,
            kind: Kind::Action,
            press,
            action: None,
            finder,
            value,
            program,
            arguments,
            choice: None,
        };
        if let Some(key) = key {
            draft.fill(&key, cx);
        }
        draft
    }

    /// Shows what `key` presses and does.
    fn fill<V: 'static>(&mut self, key: &KeyConfig, cx: &mut Context<V>) {
        let put = |field: &Entity<TextField>, text: &str, cx: &mut Context<V>| {
            field.update(cx, |field, cx| field.put_text(text, cx));
        };
        self.kind = Kind::of(key);
        put(&self.press, &key.press, cx);
        self.action = key.action.clone();
        let value = { key.arg.as_ref().or(key.popup.as_ref()) }
            .or(key.tiling.as_ref())
            .or(key.files.as_ref());
        put(&self.value, value.map_or("", String::as_str), cx);
        if let Some((program, arguments)) = key.run.as_deref().and_then(<[String]>::split_first) {
            put(&self.program, program, cx);
            put(&self.arguments, &joined_arguments(arguments), cx);
        }
        let dock = key.dock.map(|number| number.to_string());
        self.choice = { key.focus.clone().or(key.carry.clone()) }
            .or(key.switch.clone())
            .or(dock);
    }

    /// The `[[key]]` the draft makes, as it stands.
    pub fn key(&self, cx: &App) -> KeyConfig {
        let text = |field: &Entity<TextField>| field.read(cx).text();
        key_of(
            self.kind,
            &text(&self.press),
            self.action.as_deref(),
            &text(&self.value),
            &text(&self.program),
            &text(&self.arguments),
            self.choice.as_deref(),
        )
    }

    /// Changes what kind of thing the key does; what was chosen for
    /// another kind is forgotten.
    pub fn set_kind(&mut self, kind: Kind) {
        if self.kind != kind {
            self.kind = kind;
            self.choice = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_taken_apart_and_joined_again() {
        let split = split_arguments(r#"-d "C:\My Files" --x  "" y"#);
        assert_eq!(split, ["-d", r"C:\My Files", "--x", "", "y"]);
        assert_eq!(joined_arguments(&split), r#"-d "C:\My Files" --x "" y"#);
        assert!(split_arguments("   ").is_empty());
    }

    #[test]
    fn a_draft_makes_the_key_of_its_kind() {
        let key = |kind, action, value, choice| {
            key_of(kind, " ctrl+alt+k ", action, value, "", "", choice)
        };
        let action = key(Kind::Action, Some("snap"), " left ", None);
        assert_eq!(action.press, "ctrl+alt+k");
        assert_eq!(
            (action.action.as_deref(), action.arg.as_deref()),
            (Some("snap"), Some("left"))
        );
        assert_eq!(key(Kind::Action, Some("run"), "", None).arg, None);
        assert_eq!(
            key(Kind::Popup, None, "launcher", None).popup.as_deref(),
            Some("launcher")
        );
        assert_eq!(key(Kind::Dock, None, "", Some("3")).dock, Some(3));
        assert_eq!(
            key(Kind::Move, None, "", Some("up")).carry.as_deref(),
            Some("up")
        );
        let program = key_of(
            Kind::Program,
            "ctrl+alt+t",
            None,
            "",
            r#""C:\wt.exe""#,
            "-p Ubuntu",
            None,
        );
        assert_eq!(
            program.run,
            Some(vec![
                r"C:\wt.exe".to_owned(),
                "-p".to_owned(),
                "Ubuntu".to_owned()
            ])
        );
        assert_eq!(
            key_of(Kind::Program, "x", None, "", " ", "", None).run,
            None
        );
        for kind in Kind::ALL {
            let made = key_of(kind, "ctrl+k", Some("a"), "v", "p", "", Some("1"));
            assert_eq!(Kind::of(&made), kind, "{kind:?}");
        }
    }
}
