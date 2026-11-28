// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The key combinations of the configuration and what each one does.
//!
//! Every combination Tessera holds is reported in one place. The ones of
//! the workspaces are passed on to them; the others are the `[[key]]`
//! entries of the configuration, each of which opens a popup, changes the
//! tiling, starts a program, goes through the windows or does one of the
//! named actions of [`crate::actions`].

use std::process::{Command, Stdio};
use std::time::Duration;

use gpui::{App, AsyncApp, Global};

use crate::actions::Actions;
use crate::config::KeyConfig;
use crate::files::Files;
use crate::i18n::Language;
use crate::modules::windows::Docks;
use crate::platform::{
    Commands, Display, Hotkey, Hotkeys, Modifiers, MouseBinding, MouseButtonName, MouseButtons,
    TakenKeys, pointer_at, shell_mode,
};
use crate::shell::Shell;
use crate::snap::anchor_at;
use crate::switcher::Switcher;
use crate::tiling::{Tiler, TilingCommand, Toward, parse_command};
use crate::tip::{TipKey, Tips};
use crate::workspaces::Workspaces;

/// The keys a combination ends with, as the configuration names them;
/// [`Hotkey::parse`] knows each of them.
pub const KEYS: &[&str] = &[
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "space",
    "tab",
    "enter",
    "left",
    "up",
    "right",
    "down",
    "home",
    "end",
    "pageup",
    "pagedown",
    "insert",
    "delete",
    "backspace",
    "escape",
    "pause",
    "printscreen",
    "period",
    "comma",
    "plus",
    "minus",
    "backquote",
    "f13",
    "f14",
    "f15",
    "f16",
    "f17",
    "f18",
    "f19",
    "f20",
    "f21",
    "f22",
    "f23",
    "f24",
    "semicolon",
    "slash",
    "backslash",
    "leftbracket",
    "rightbracket",
    "quote",
    "num0",
    "num1",
    "num2",
    "num3",
    "num4",
    "num5",
    "num6",
    "num7",
    "num8",
    "num9",
    "multiply",
    "add",
    "subtract",
    "decimal",
    "divide",
    "scrolllock",
    "menu",
];

/// The keys to hold, as they are shown, in the order they are written in.
pub const HELD: [&str; 4] = ["Ctrl", "Alt", "Shift", "Win"];

/// A key combination taken apart: the keys held and the key pressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Press {
    pub held: Modifiers,
    /// The key as the configuration names it.
    pub key: String,
}

impl Press {
    /// What `text` presses, if it is a combination Tessera can hold.
    pub fn parse(text: &str) -> Option<Self> {
        Hotkey::parse(0, text)?;
        let (held, key) = text.rsplit_once('+')?;
        Some(Self {
            held: Modifiers::parse(held)?,
            key: key.trim().to_ascii_lowercase(),
        })
    }

    /// Whether the key of [`HELD`] at `which` is held, to be changed.
    pub fn held_mut(&mut self, which: usize) -> Option<&mut bool> {
        let held = &mut self.held;
        let keys = [
            &mut held.control,
            &mut held.alt,
            &mut held.shift,
            &mut held.windows,
        ];
        keys.into_iter().nth(which)
    }

    fn held(&self) -> impl Iterator<Item = usize> {
        let held = self.held;
        let keys = [held.control, held.alt, held.shift, held.windows];
        (0..keys.len()).filter(move |&which| keys[which])
    }

    /// The combination as the configuration writes it; `None` where no key
    /// is held, which is no combination.
    pub fn written(&self) -> Option<String> {
        let mut names: Vec<String> = self
            .held()
            .map(|which| HELD[which].to_ascii_lowercase())
            .collect();
        if names.is_empty() {
            return None;
        }
        names.push(self.key.clone());
        Some(names.join("+"))
    }

    /// The combination as it is shown.
    pub fn shown(&self) -> String {
        let mut names: Vec<String> = self.held().map(|which| HELD[which].to_owned()).collect();
        names.push(key_label(&self.key));
        names.join(" + ")
    }

    /// What `text` presses as a step of a sequence after its first: a
    /// combination, or a key by itself, as `t`.
    pub fn parse_step(text: &str) -> Option<Self> {
        if let Some(press) = Self::parse(text) {
            return Some(press);
        }
        Hotkey::key_code(text)?;
        Some(Self {
            held: Modifiers::default(),
            key: text.trim().to_ascii_lowercase(),
        })
    }

    /// The step as the configuration writes it: the combination, or the
    /// key by itself where none is held.
    pub fn step_written(&self) -> String {
        self.written().unwrap_or_else(|| self.key.clone())
    }
}

/// The most combinations a sequence has, as in KDE Plasma.
pub const MOST_STEPS: usize = 4;

/// The mouse buttons a `press` can end with, by the names the
/// configuration gives them, and what each is called where it is shown.
pub const BUTTONS: [(&str, MouseButtonName, &str); 3] = [
    ("middle", MouseButtonName::Middle, "Middle button"),
    ("back", MouseButtonName::Back, "Back button"),
    ("forward", MouseButtonName::Forward, "Forward button"),
];

/// What the `press` of a `[[key]]` asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// One combination, or up to [`MOST_STEPS`] pressed one after the
    /// other, written with commas between them (`ctrl+alt+k, t`). The
    /// first holds keys; a later one may be a key by itself.
    Keys(Vec<Press>),
    /// A mouse button pressed while keys are held, as `ctrl+back`.
    Button(MouseBinding),
}

impl Gesture {
    /// What `text` asks for, if it is a press Tessera can answer.
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split(',').map(str::trim).collect();
        if let [part] = parts[..]
            && let Some(binding) = Self::button(part)
        {
            return Some(Self::Button(binding));
        }
        if parts.len() > MOST_STEPS {
            return None;
        }
        let (first, rest) = parts.split_first()?;
        let mut steps = vec![Press::parse(first)?];
        for part in rest {
            steps.push(Press::parse_step(part)?);
        }
        Some(Self::Keys(steps))
    }

    /// The mouse button and the keys held that `text` names, as
    /// `ctrl+back`; a button with no key held is none.
    fn button(text: &str) -> Option<MouseBinding> {
        let (held, name) = text.rsplit_once('+')?;
        let name = name.trim().to_ascii_lowercase();
        let (_, button, _) = BUTTONS.into_iter().find(|(known, ..)| *known == name)?;
        Some(MouseBinding {
            held: Modifiers::parse(held)?,
            button,
        })
    }

    /// The press as the configuration writes it.
    pub fn written(&self) -> String {
        match self {
            Self::Keys(steps) => {
                let steps: Vec<String> = steps.iter().map(Press::step_written).collect();
                steps.join(", ")
            }
            Self::Button(binding) => {
                let (name, ..) = Self::button_names(binding.button);
                format!("{}+{name}", binding.held.written())
            }
        }
    }

    /// The press as it is shown, in `language` for a mouse button.
    pub fn shown(&self, language: Language) -> String {
        match self {
            Self::Keys(steps) => {
                let steps: Vec<String> = steps.iter().map(Press::shown).collect();
                steps.join(", ")
            }
            Self::Button(binding) => {
                let (_, title) = Self::button_names(binding.button);
                let press = Press {
                    held: binding.held,
                    key: String::new(),
                };
                let mut names: Vec<String> =
                    press.held().map(|which| HELD[which].to_owned()).collect();
                names.push(language.say(title).to_owned());
                names.join(" + ")
            }
        }
    }

    fn button_names(button: MouseButtonName) -> (&'static str, &'static str) {
        let found = BUTTONS.into_iter().find(|(_, known, _)| *known == button);
        found.map_or(("middle", "Middle button"), |(name, _, title)| {
            (name, title)
        })
    }

    /// The combinations of the press, none for a mouse button.
    pub fn steps(&self) -> &[Press] {
        match self {
            Self::Keys(steps) => steps,
            Self::Button(_) => &[],
        }
    }

    /// Whether `self` and `other` cannot both be answered: they are the
    /// same, or one is a sequence that begins with the other, which is
    /// then answered at once and leaves no time for the rest.
    pub fn clashes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Keys(mine), Self::Keys(theirs)) => {
                mine.starts_with(theirs) || theirs.starts_with(mine)
            }
            _ => self == other,
        }
    }
}

/// The press `text` names, written as the configuration writes it, so that
/// two ways of writing the same keys compare equal; `None` for no press.
pub fn written_press(text: &str) -> Option<String> {
    Gesture::parse(text).map(|gesture| gesture.written())
}

/// Whether `key` can be used, and why not: what it presses, and what it
/// does with the actions of `actions`.
pub fn check(key: &KeyConfig, actions: &Actions) -> Result<(), String> {
    if Gesture::parse(&key.press).is_none() {
        return Err(format!(
            "`{}` is no press: keys to hold and a key joined by +, up to four of them after one another with commas, or keys to hold and `middle`, `back` or `forward`",
            key.press
        ));
    }
    Deed::of(key, actions).map(drop)
}

/// What stands on the key the configuration calls `key`.
pub fn key_label(key: &str) -> String {
    match key {
        "left" => "\u{2190}".to_owned(),
        "up" => "\u{2191}".to_owned(),
        "right" => "\u{2192}".to_owned(),
        "down" => "\u{2193}".to_owned(),
        "pageup" => "Page Up".to_owned(),
        "pagedown" => "Page Down".to_owned(),
        "printscreen" => "Print Screen".to_owned(),
        "escape" => "Esc".to_owned(),
        "period" => ".".to_owned(),
        "comma" => ",".to_owned(),
        "plus" => "+".to_owned(),
        "minus" => "-".to_owned(),
        "backquote" => "`".to_owned(),
        "semicolon" => ";".to_owned(),
        "slash" => "/".to_owned(),
        "backslash" => "\\".to_owned(),
        "leftbracket" => "[".to_owned(),
        "rightbracket" => "]".to_owned(),
        "quote" => "'".to_owned(),
        "multiply" => "Num *".to_owned(),
        "add" => "Num +".to_owned(),
        "subtract" => "Num -".to_owned(),
        "decimal" => "Num .".to_owned(),
        "divide" => "Num /".to_owned(),
        "scrolllock" => "Scroll Lock".to_owned(),
        _ if key.starts_with("num") => format!("Num {}", &key[3..]),
        "space" | "tab" | "enter" | "home" | "end" | "insert" | "delete" | "backspace"
        | "pause" | "menu" => {
            let (first, rest) = key.split_at(1);
            format!("{}{rest}", first.to_ascii_uppercase())
        }
        _ => key.to_ascii_uppercase(),
    }
}

/// The number the first `[[key]]` is reported by; the workspaces have the
/// numbers below it.
const FIRST: u32 = 1 << 16;

/// The words a start of the program may begin with to hand them to the
/// Tessera that runs already.
const COMMANDS: [&str; 9] = [
    "popup", "tiling", "focus", "move", "dock", "files", "do", "quit", "restart",
];

/// What separates the words on their way.
const BETWEEN: char = '\n';

/// Hands `words` to the running Tessera if they begin with a command.
/// Says whether that worked; nothing if they are no command.
pub fn send_if_requested(words: impl Iterator<Item = String>) -> Option<bool> {
    let words: Vec<String> = words.collect();
    let first = words.first()?;
    if !COMMANDS.contains(&first.as_str()) {
        return None;
    }
    let text = words.join(&BETWEEN.to_string());
    Some(Commands::send(&text).is_ok())
}

/// What words handed to the running Tessera ask for.
#[derive(Debug, PartialEq)]
enum Asked {
    Quit,
    Restart,
    Deed(Deed),
}

impl Asked {
    /// What `text` asks for, or why it asks for nothing; `actions` are
    /// those `do` can name.
    fn of(text: &str, actions: &Actions) -> Result<Self, String> {
        let words: Vec<&str> = text.split(BETWEEN).collect();
        let said = |word: &str| Some(word.to_owned());
        let mut key = KeyConfig::default();
        match words[..] {
            ["quit"] => return Ok(Self::Quit),
            ["restart"] => return Ok(Self::Restart),
            ["popup", name] => key.popup = said(name),
            ["tiling", ref words @ ..] if !words.is_empty() => key.tiling = Some(words.join(" ")),
            ["focus", side] => key.focus = said(side),
            ["move", side] => key.carry = said(side),
            // A word that is no number is one no application has.
            ["dock", number] => key.dock = Some(number.parse().unwrap_or(0)),
            // Every word is kept, as Explorer's command line may have
            // several (`/n /e,C:\x`).
            ["files", ref words @ ..] => {
                let words = words.iter().map(|word| (*word).to_owned()).collect();
                return Ok(Self::Deed(Deed::Files(words)));
            }
            ["do", name] => key.action = said(name),
            // An argument with spaces may come as several words.
            ["do", name, ref arg @ ..] => {
                key.action = said(name);
                key.arg = Some(arg.join(" "));
            }
            _ => {
                let why = "it is `quit`, or one of `popup`, `tiling`, `focus`, `move` and `dock` with one word after it, `files` with a folder or none, or `do` with an action and what it is done with";
                return Err(why.to_owned());
            }
        }
        Deed::of(&key, actions).map(Self::Deed)
    }
}

/// What a `[[key]]` does; a `[[corner]]` does one of the same deeds.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Deed {
    Popup(String),
    /// What is asked of the tiling: a layout, none, the next one, floating
    /// the window in front, a size or where the next window goes.
    Tiling(TilingCommand),
    Run(Vec<String>),
    Focus(Toward),
    Carry(Toward),
    /// Go through the windows, backwards with `back`, until none of
    /// `held` is down.
    Switch {
        back: bool,
        held: Modifiers,
    },
    /// What a click on the button of this number in the dock does.
    Dock(usize),
    /// Opens the file manager where these words name, read as Explorer
    /// reads its command line: a folder, or `/select,<path>` and the like.
    Files(Vec<String>),
    /// Does the action registered as `name`, with `arg`.
    Action {
        name: String,
        arg: Option<String>,
    },
}

impl Deed {
    /// What `key` does, or why it does nothing; `actions` are those it
    /// can name.
    fn of(key: &KeyConfig, actions: &Actions) -> Result<Self, String> {
        let side = |name: &String| {
            Toward::parse(name)
                .ok_or_else(|| format!("a side is `left`, `right`, `up` or `down`, not `{name}`"))
        };
        let deeds = [
            key.popup
                .as_ref()
                .map(|popup| Ok(Self::Popup(popup.clone()))),
            key.tiling.as_deref().map(Self::tiling),
            key.run.as_deref().map(Self::program),
            key.focus.as_ref().map(|name| side(name).map(Self::Focus)),
            key.carry.as_ref().map(|name| side(name).map(Self::Carry)),
            key.switch
                .as_deref()
                .map(|name| Self::switch(name, &key.press)),
            key.dock.map(|number| match number {
                1..=9 => Ok(Self::Dock(number as usize)),
                _ => Err(format!("`dock` is a number from 1 to 9, not {number}")),
            }),
            key.files
                .as_ref()
                .map(|folder| Ok(Self::Files(vec![folder.clone()]))),
            key.action
                .as_deref()
                .map(|name| Self::action(name, key.arg.as_deref(), actions)),
        ];
        Self::one_of(
            deeds,
            key.arg.is_some() && key.action.is_none(),
            "`popup`, `tiling`, `run`, `focus`, `move`, `switch`, `dock`, `files` and `action`",
        )
    }

    /// The one deed that is given among `deeds`, or why there is not
    /// exactly one; `names` lists the entries that give one. With
    /// `stray_arg`, an `arg` was given without an `action`, which is
    /// refused.
    pub(crate) fn one_of(
        deeds: impl IntoIterator<Item = Option<Result<Self, String>>>,
        stray_arg: bool,
        names: &str,
    ) -> Result<Self, String> {
        if stray_arg {
            return Err("`arg` is what an `action` is done with, and there is none".to_owned());
        }
        let mut deeds = deeds.into_iter().flatten();
        match (deeds.next(), deeds.next()) {
            (Some(deed), None) => deed,
            _ => Err(format!("it needs one of {names}")),
        }
    }

    /// Starting the program `run` names, with the arguments after it.
    pub(crate) fn program(run: &[String]) -> Result<Self, String> {
        match run.is_empty() {
            true => Err("`run` names no program".to_owned()),
            false => Ok(Self::Run(run.to_vec())),
        }
    }

    pub(crate) fn action(name: &str, arg: Option<&str>, actions: &Actions) -> Result<Self, String> {
        actions.check(name, arg)?;
        Ok(Self::Action {
            name: name.to_owned(),
            arg: arg.map(str::to_owned),
        })
    }

    /// Whether the keys of this deed are taken from Windows where it keeps
    /// them to itself: those of the window switcher, of the dock, of the
    /// file manager and of the actions that ask for it.
    fn takes_reserved_keys(&self, actions: &Actions) -> bool {
        match self {
            Self::Switch { .. } | Self::Dock(_) | Self::Files(_) => true,
            Self::Action { name, .. } => actions.takes_reserved_keys(name),
            _ => false,
        }
    }

    /// What the words of a `tiling` entry ask of the tiling.
    fn tiling(words: &str) -> Result<Self, String> {
        let words: Vec<&str> = words.split_whitespace().collect();
        parse_command(&words).map(Self::Tiling)
    }

    fn switch(name: &str, press: &str) -> Result<Self, String> {
        let back = match name {
            "next" => false,
            "previous" => true,
            _ => return Err(format!("`switch` is `next` or `previous`, not `{name}`")),
        };
        let press = Press::parse(press).ok_or("its keys are no combination")?;
        Ok(Self::Switch {
            back,
            held: press.held,
        })
    }

    pub(crate) fn done(&self, cx: &mut App) {
        match self {
            Self::Switch { back, held } => Switcher::step(*back, *held, cx),
            Self::Popup(name) => Shell::toggle_popup(name, cx),
            Self::Tiling(command) => Tiler::command(command.clone(), cx),
            Self::Focus(toward) => Tiler::focus(*toward, cx),
            Self::Carry(toward) => Tiler::carry(*toward, cx),
            Self::Dock(number) => Docks::press(*number, cx),
            Self::Files(words) => Files::open_command(words, cx),
            Self::Action { name, arg } => Actions::run(name, arg.as_deref(), cx),
            Self::Run(command) => start_program(command),
        }
    }
}

/// Starts the program `command` names, with the arguments after it, as
/// `run` of a `[[key]]` or a `[[corner]]` asks; it gets no console of its
/// own to read from or write to.
pub(crate) fn start_program(command: &[String]) {
    let Some((program, arguments)) = command.split_first() else {
        return;
    };
    let started = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(error) = started {
        report!("`{program}` did not start: {error}");
    }
}

/// What became of an entry in use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyState {
    /// Tessera holds its keys.
    Held,
    /// Windows keeps its keys to itself; Tessera's keyboard hook takes
    /// them from it.
    Taken,
    /// Another program holds its keys, so it does nothing.
    Refused,
    /// Its action is `none`: nothing is held, and the keys stay with
    /// whoever has them.
    Off,
    /// A mouse button, taken by the mouse hook.
    Button,
    /// Held back while `pause-keys` pauses the keys.
    Paused,
    /// It does nothing, for this reason.
    Broken(String),
}

/// The number the first step after the first of a sequence is reported
/// by while it is waited for; the others follow it.
const FIRST_STEP: u32 = 1 << 24;

/// How long the next combination of a sequence is waited for.
const STEP_WAIT: Duration = Duration::from_millis(1500);

/// What the tip of a sequence under way is known by.
const STEPS_TIP: TipKey = ("key-sequence", 0);

/// The action that does nothing, which turns off a key an action brings.
pub const NO_ACTION: &str = "none";

/// The action that pauses the keys, whose own keys are never paused.
pub const PAUSE_ACTION: &str = "pause-keys";

/// A sequence under way: the combinations pressed so far, and the entries
/// whose press begins with them.
#[derive(Clone, Debug, PartialEq)]
struct Steps {
    pressed: Vec<Press>,
    entries: Vec<usize>,
}

/// What pressing a combination of a sequence comes to.
#[derive(Debug, PartialEq)]
enum Stepped {
    /// The press of this entry is complete.
    Done(usize),
    /// More is waited for.
    Waiting(Steps),
    /// No press begins so.
    Nothing,
}

impl Steps {
    /// What having pressed `pressed` comes to among the entries `among`,
    /// each with the combinations of its press: an entry that presses
    /// exactly these is done at once.
    fn after<'a>(
        pressed: Vec<Press>,
        among: impl IntoIterator<Item = (usize, &'a [Press])>,
    ) -> Stepped {
        let begun: Vec<(usize, &[Press])> = { among.into_iter() }
            .filter(|(_, steps)| steps.starts_with(&pressed))
            .collect();
        if let Some((index, _)) = begun.iter().find(|(_, steps)| steps.len() == pressed.len()) {
            return Stepped::Done(*index);
        }
        match begun.is_empty() {
            true => Stepped::Nothing,
            false => Stepped::Waiting(Self {
                pressed,
                entries: begun.into_iter().map(|(index, _)| index).collect(),
            }),
        }
    }

    /// The combinations that can come next, each once, among `gestures`.
    fn next(&self, gestures: &[Option<Gesture>]) -> Vec<Press> {
        let mut next: Vec<Press> = Vec::new();
        for &index in &self.entries {
            let steps = { gestures.get(index).and_then(Option::as_ref) }.map(Gesture::steps);
            let Some(step) = steps.and_then(|steps| steps.get(self.pressed.len())) else {
                continue;
            };
            if !next.contains(step) {
                next.push(step.clone());
            }
        }
        next
    }
}

/// The `[[key]]` entries in use.
#[derive(Default)]
pub struct Shortcuts {
    /// The entries of the user and `windows_keys`, as last configured.
    asked: (Vec<KeyConfig>, Option<bool>),
    config: Vec<KeyConfig>,
    /// What each entry does; `None` for one that cannot be used.
    deeds: Vec<Option<Deed>>,
    /// What each entry presses; `None` for one that cannot be read.
    gestures: Vec<Option<Gesture>>,
    /// What became of each entry.
    states: Vec<KeyState>,
    /// Whether `pause-keys` pauses the keys.
    paused: bool,
    /// The sequence under way and the combinations that may come next.
    steps: Option<(Steps, Vec<Press>)>,
    /// Counts the sequences begun, so that the end of the wait for one
    /// that is over already changes nothing.
    sequences: u64,
}

impl Global for Shortcuts {}

/// What the entries in use come to, before Windows is asked for them.
struct Read {
    deeds: Vec<Option<Deed>>,
    gestures: Vec<Option<Gesture>>,
    states: Vec<KeyState>,
    /// The combinations to hold: the first of each press, once for all
    /// the entries whose press begins with it.
    hotkeys: Vec<Hotkey>,
    /// For each entry, the one whose combination is held for it.
    holders: Vec<Option<usize>>,
    /// The entries pressed with a mouse button, and each button.
    buttons: Vec<(usize, MouseBinding)>,
}

impl Shortcuts {
    /// Takes the combinations of `keys` and answers every combination
    /// Tessera holds from now on.
    pub fn start(keys: &[KeyConfig], windows_keys: Option<bool>, cx: &mut App) {
        cx.set_global(Self::default());
        let presses = Hotkeys::presses();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(pressed) = presses.recv().await {
                let answered = cx.update(|cx| match pressed {
                    FIRST_STEP.. => Self::step_pressed((pressed - FIRST_STEP) as usize, cx),
                    FIRST.. => Self::pressed((pressed - FIRST) as usize, cx),
                    _ => Workspaces::pressed(pressed, cx),
                });
                if answered.is_err() {
                    break;
                }
            }
        })
        .detach();
        let taken = TakenKeys::presses();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(pressed) = taken.recv().await {
                let index = pressed.saturating_sub(FIRST) as usize;
                if cx.update(|cx| Self::pressed(index, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self::listen(cx);
        Self::configure(keys, windows_keys, cx);
    }

    /// Answers the commands other starts of the program hand over.
    fn listen(cx: &mut App) {
        let commands = match Commands::listen() {
            Ok(commands) => commands,
            Err(error) => return report!("commands do not reach Tessera: {error:#}"),
        };
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(text) = commands.recv().await {
                let answered = cx.update(|cx| match Asked::of(&text, Self::actions(cx)) {
                    Ok(Asked::Quit) => cx.quit(),
                    Ok(Asked::Restart) => crate::shell::restart(cx),
                    Ok(Asked::Deed(deed)) => deed.done(cx),
                    Err(why) => report!("a command does nothing: {why}"),
                });
                if answered.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Takes over the entries of a configuration that was read again, if
    /// they are other than the ones before, with the keys the actions
    /// bind by default where `windows_keys` asks for them (by default, when
    /// Tessera is the shell).
    pub fn configure(keys: &[KeyConfig], windows_keys: Option<bool>, cx: &mut App) {
        let defaults = windows_keys.unwrap_or_else(shell_mode);
        let asked = (keys.to_vec(), windows_keys);
        let keys = Self::in_use(keys, defaults, Self::actions(cx));
        let this = cx.global_mut::<Self>();
        this.asked = asked;
        if this.config == keys {
            return;
        }
        this.config = keys;
        Self::hold(cx);
    }

    /// Asks Windows for the combinations of the entries in use, or, while
    /// the keys are paused, for those of `pause-keys` alone, and binds the
    /// mouse buttons.
    fn hold(cx: &mut App) {
        Self::end_steps(cx);
        let keys = cx.global::<Self>().config.clone();
        let paused = cx.global::<Self>().paused;
        let actions = Self::actions(cx);
        let mut read = Self::read(&keys, actions);
        let pausing = |deed: &Option<Deed>| matches!(deed, Some(Deed::Action { name, .. }) if name == PAUSE_ACTION);
        let kept: Vec<u32> = { read.hotkeys.iter() }
            .filter(|hotkey| pausing(&read.deeds[(hotkey.id - FIRST) as usize]))
            .map(|hotkey| hotkey.id)
            .collect();
        if let Err(error) = Hotkeys::pause(paused.then(|| kept.clone())) {
            report!("the keys cannot be paused: {error:#}");
        }
        // Windows keeps Alt+Tab, the Windows key with a digit and Win+E,
        // which opens Explorer, to itself. The keys of the window
        // switcher, of the dock, of the file manager and of the actions
        // that ask for it that it refuses are taken from it; a refused
        // combination of anything else is left to whoever holds it.
        let switches = |id: u32| {
            let deed = read.deeds.get((id - FIRST) as usize);
            deed.is_some_and(|deed| {
                deed.as_ref()
                    .is_some_and(|deed| deed.takes_reserved_keys(actions))
            })
        };
        let hotkeys = read.hotkeys.clone();
        let refused = Hotkeys::set("keys", hotkeys.clone()).unwrap_or_else(|error| {
            report!("the `[[key]]` entries have no keys: {error:#}");
            hotkeys.iter().map(|hotkey| hotkey.id).collect()
        });
        let held_now = |id: &u32| !paused || kept.contains(id);
        let refused: Vec<u32> = refused.into_iter().filter(held_now).collect();
        let (taken, refused): (Vec<u32>, Vec<u32>) =
            refused.into_iter().partition(|&id| switches(id));
        for &id in &refused {
            let press = &keys[(id - FIRST) as usize].press;
            report!("Windows did not give the keys `{press}`: they are held already");
        }
        let taken_keys = { hotkeys.iter() }.filter(|hotkey| taken.contains(&hotkey.id));
        if let Err(error) = TakenKeys::set(taken_keys.copied().collect()) {
            report!("the keys Windows keeps to itself stay with it: {error:#}");
        }
        for (index, holder) in read.holders.iter().enumerate() {
            let Some(id) = holder.map(|holder| FIRST + holder as u32) else {
                continue;
            };
            read.states[index] = if refused.contains(&id) {
                KeyState::Refused
            } else if taken.contains(&id) {
                KeyState::Taken
            } else if !held_now(&id) {
                KeyState::Paused
            } else {
                KeyState::Held
            };
        }
        let buttons = match paused {
            true => Vec::new(),
            false => read.buttons.clone(),
        };
        if paused {
            for (index, _) in &read.buttons {
                read.states[*index] = KeyState::Paused;
            }
        }
        Self::bind_buttons(buttons, cx);
        let this = cx.global_mut::<Self>();
        this.deeds = read.deeds;
        this.gestures = read.gestures;
        this.states = read.states;
    }

    /// Has the mouse buttons of `buttons` answered, each with what its
    /// entry does; the buttons bound before are given up.
    fn bind_buttons(buttons: Vec<(usize, MouseBinding)>, cx: &mut App) {
        let bindings = buttons.iter().map(|(_, binding)| *binding).collect();
        let presses = MouseButtons::bind(bindings);
        if buttons.is_empty() {
            return;
        }
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(pressed) = presses.recv().await {
                let Some(&(index, _)) = buttons.get(pressed) else {
                    continue;
                };
                if cx.update(|cx| Self::pressed(index, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Configures the entries last configured again, after an action
    /// began or stopped resting its keys by default. Nothing is done
    /// before the entries are first configured.
    pub fn again(cx: &mut App) {
        let Some(this) = cx.try_global::<Self>() else {
            return;
        };
        let (keys, windows_keys) = this.asked.clone();
        Self::configure(&keys, windows_keys, cx);
    }

    /// Pauses every combination of Tessera but the keys of `pause-keys`
    /// with `Some(true)`, holds them again with `Some(false)`, and does
    /// the other of the two with `None`.
    pub fn pause(pause: Option<bool>, cx: &mut App) {
        if cx.try_global::<Self>().is_none() {
            return;
        }
        let this = cx.global_mut::<Self>();
        let paused = pause.unwrap_or(!this.paused);
        if this.paused == paused {
            return;
        }
        this.paused = paused;
        Self::hold(cx);
    }

    /// Whether the keys are paused.
    pub fn paused(cx: &App) -> bool {
        cx.try_global::<Self>().is_some_and(|this| this.paused)
    }

    /// The entries of the user and whether the keys of Windows are asked
    /// for (`windows_keys`), as last configured.
    pub fn asked(cx: &App) -> (&[KeyConfig], Option<bool>) {
        match cx.try_global::<Self>() {
            Some(this) => (&this.asked.0, this.asked.1),
            None => (&[], None),
        }
    }

    /// The entries in use, each with what became of it: those of the user,
    /// then the keys the actions bring that are bound.
    pub fn in_use_now(cx: &App) -> Vec<(KeyConfig, KeyState)> {
        let Some(this) = cx.try_global::<Self>() else {
            return Vec::new();
        };
        let states = this
            .states
            .iter()
            .cloned()
            .chain(std::iter::repeat(KeyState::Held));
        this.config.iter().cloned().zip(states).collect()
    }

    /// The entries of the user, and after them, where `defaults`, the
    /// keys the actions bind by default that press other keys than every
    /// entry of the user does.
    fn in_use(keys: &[KeyConfig], defaults: bool, actions: &Actions) -> Vec<KeyConfig> {
        let mut in_use = keys.to_vec();
        if !defaults {
            return in_use;
        }
        let taken: Vec<Option<String>> = keys.iter().map(|key| written_press(&key.press)).collect();
        for key in actions.default_keys() {
            let press = written_press(&key.press);
            if press.is_none() || !taken.contains(&press) {
                in_use.push(key);
            }
        }
        in_use
    }

    /// The actions a key can name; none before the shell has collected
    /// them.
    fn actions(cx: &mut App) -> &Actions {
        cx.default_global::<Actions>()
    }

    /// What each entry does and presses and the combinations to hold for
    /// them; the entries that cannot be used are reported.
    fn read(keys: &[KeyConfig], actions: &Actions) -> Read {
        let mut read = Read {
            deeds: Vec::new(),
            gestures: Vec::new(),
            states: Vec::new(),
            hotkeys: Vec::new(),
            holders: Vec::new(),
            buttons: Vec::new(),
        };
        // The first combination of every press held so far, and the entry
        // it is held for.
        let mut firsts: Vec<(Press, usize)> = Vec::new();
        for (index, key) in keys.iter().enumerate() {
            let press = &key.press;
            let gesture = Gesture::parse(press);
            let deed = Deed::of(key, actions);
            let (deed, state, holder) = match (&gesture, deed) {
                (None, _) => {
                    report!(
                        "a `[[key]]` presses keys to hold and a key joined by +, not `{press}`"
                    );
                    let why = "its keys are no combination".to_owned();
                    (None, KeyState::Broken(why), None)
                }
                (Some(_), Err(why)) => {
                    report!("the `[[key]]` of `{press}` does nothing: {why}");
                    (None, KeyState::Broken(why), None)
                }
                (Some(_), Ok(Deed::Action { name, .. })) if name == NO_ACTION => {
                    (None, KeyState::Off, None)
                }
                (Some(Gesture::Button(binding)), Ok(deed)) => {
                    read.buttons.push((index, *binding));
                    (Some(deed), KeyState::Button, None)
                }
                (Some(Gesture::Keys(steps)), Ok(deed)) => {
                    let first = &steps[0];
                    let holder = match firsts.iter().find(|(held, _)| held == first) {
                        Some((_, holder)) => *holder,
                        None => {
                            let written = first.written().unwrap_or_default();
                            match Hotkey::parse(FIRST + index as u32, &written) {
                                Some(hotkey) => read.hotkeys.push(hotkey),
                                None => report!("the keys `{written}` cannot be held"),
                            }
                            firsts.push((first.clone(), index));
                            index
                        }
                    };
                    (Some(deed), KeyState::Held, Some(holder))
                }
            };
            read.deeds.push(deed);
            read.gestures.push(gesture);
            read.states.push(state);
            read.holders.push(holder);
        }
        read
    }

    /// Answers the combination held for the entry at `index`: what the
    /// entry does, or, where presses go on after it, the beginning of a
    /// sequence.
    fn pressed(index: usize, cx: &mut App) {
        let this = cx.global::<Self>();
        let first = match this.gestures.get(index).and_then(Option::as_ref) {
            Some(Gesture::Keys(steps)) => steps[..1].to_vec(),
            Some(Gesture::Button(_)) => return Self::done(index, cx),
            None => return,
        };
        let among = Self::sequences(this);
        match Steps::after(first, among) {
            Stepped::Done(index) => Self::done(index, cx),
            Stepped::Waiting(steps) => Self::wait_for_step(steps, cx),
            Stepped::Nothing => {}
        }
    }

    /// The entries that do something, each with the combinations of its
    /// press.
    fn sequences(this: &Self) -> Vec<(usize, &[Press])> {
        let entries = this.gestures.iter().zip(&this.deeds).enumerate();
        { entries }
            .filter(|(_, (_, deed))| deed.is_some())
            .filter_map(|(index, (gesture, _))| Some((index, gesture.as_ref()?.steps())))
            .collect()
    }

    /// Does what the entry at `index` does.
    fn done(index: usize, cx: &mut App) {
        let this = cx.global::<Self>();
        let deed = this.deeds.get(index).cloned().flatten();
        let steps = { this.gestures.get(index).and_then(Option::as_ref) }.map(Gesture::steps);
        let held = match this.gestures.get(index).and_then(Option::as_ref) {
            Some(Gesture::Button(binding)) => binding.held,
            _ => { steps.and_then(<[Press]>::last) }
                .map(|press| press.held)
                .unwrap_or_default(),
        };
        if let Some(deed) = deed {
            Actions::pressing(held, cx, |cx| deed.done(cx));
        }
    }

    /// Holds the combinations that can come after `steps`, and Esc, for a
    /// moment, with a tip at the pointer of what was pressed so far.
    fn wait_for_step(steps: Steps, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        let next = steps.next(&this.gestures);
        let escape = Press {
            held: Modifiers::default(),
            key: "escape".to_owned(),
        };
        let shown: Vec<String> = steps.pressed.iter().map(Press::shown).collect();
        this.sequences += 1;
        let sequence = this.sequences;
        let mut held: Vec<Press> = next.clone();
        if !held.contains(&escape) {
            held.push(escape);
        }
        this.steps = Some((steps, next));
        let hotkeys = { held.iter().enumerate() }
            .filter_map(|(at, press)| Hotkey::step(FIRST_STEP + at as u32, &press.step_written()))
            .collect();
        if let Err(error) = Hotkeys::set("key steps", hotkeys) {
            report!("the rest of a sequence of keys cannot be waited for: {error:#}");
        }
        let text = format!("{}, \u{2026}", shown.join(", "));
        if let Some(point) = pointer_at()
            && let Some(display) = Display::at(point)
        {
            let anchor = anchor_at(&display, point, cx);
            Tips::request(STEPS_TIP, text.into(), anchor, cx);
        }
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor().timer(STEP_WAIT).await;
            cx.update(|cx| {
                if cx.global::<Self>().sequences == sequence {
                    Self::end_steps(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Answers the combination at `at` of those held after the steps of
    /// a sequence pressed so far; the one after them is Esc, which, as a
    /// combination no entry goes on with, ends the sequence.
    fn step_pressed(at: usize, cx: &mut App) {
        let Some((steps, next)) = Self::end_steps(cx) else {
            return;
        };
        let Some(step) = next.get(at) else {
            return;
        };
        let mut pressed = steps.pressed;
        pressed.push(step.clone());
        let this = cx.global::<Self>();
        let among = { Self::sequences(this).into_iter() }
            .filter(|(index, _)| steps.entries.contains(index));
        let among: Vec<(usize, &[Press])> = among.collect();
        match Steps::after(pressed, among) {
            Stepped::Done(index) => Self::done(index, cx),
            Stepped::Waiting(steps) => Self::wait_for_step(steps, cx),
            Stepped::Nothing => {}
        }
    }

    /// Ends the sequence under way, if there is one, which is answered:
    /// the combinations that could come next are given up and the tip
    /// goes.
    fn end_steps(cx: &mut App) -> Option<(Steps, Vec<Press>)> {
        let this = cx.global_mut::<Self>();
        let waiting = this.steps.take()?;
        this.sequences += 1;
        if let Err(error) = Hotkeys::set("key steps", Vec::new()) {
            report!("{error:#}");
        }
        Tips::leave(STEPS_TIP, cx);
        Some(waiting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_combination_is_taken_apart_and_written_again() {
        let mut press = Press::parse("Shift + ALT+f5").unwrap();
        assert_eq!(press.written().as_deref(), Some("alt+shift+f5"));
        assert_eq!(press.shown(), "Alt + Shift + F5");
        *press.held_mut(3).unwrap() = true;
        *press.held_mut(1).unwrap() = false;
        press.key = "left".to_owned();
        assert_eq!(press.written().as_deref(), Some("shift+win+left"));
        assert_eq!(press.shown(), "Shift + Win + \u{2190}");
        *press.held_mut(2).unwrap() = false;
        *press.held_mut(3).unwrap() = false;
        assert_eq!(press.written(), None);
        assert_eq!(Press::parse("space"), None);
        assert_eq!(Press::parse("ctrl+capslock"), None);
        assert_eq!(Press::parse("ctrl+f25"), None);
    }

    #[test]
    fn every_key_offered_can_be_held() {
        for key in KEYS {
            let text = format!("ctrl+{key}");
            let press = Press::parse(&text).unwrap_or_else(|| panic!("`{key}` is no key"));
            assert_eq!(press.written(), Some(text));
        }
        assert_eq!(key_label("space"), "Space");
        assert_eq!(key_label("a"), "A");
        assert_eq!(key_label("pageup"), "Page Up");
        assert_eq!(key_label("home"), "Home");
        assert_eq!(key_label("period"), ".");
        assert_eq!(key_label("f24"), "F24");
        let mut seen = std::collections::HashSet::new();
        assert!(
            KEYS.iter().all(|key| seen.insert(key)),
            "a key is offered twice"
        );
    }

    #[test]
    fn handed_words_ask_for_what_a_key_does() {
        assert_eq!(asked("quit"), Ok(Asked::Quit));
        assert_eq!(asked("restart"), Ok(Asked::Restart));
        assert_eq!(
            asked("popup\nlauncher"),
            Ok(Asked::Deed(Deed::Popup("launcher".to_owned())))
        );
        assert_eq!(
            asked("tiling\nfloat"),
            Ok(Asked::Deed(Deed::Tiling(TilingCommand::Float)))
        );
        let reserve = parse_command(&["reserve", "left"]).unwrap();
        assert_eq!(
            asked("tiling\nreserve\nleft"),
            Ok(Asked::Deed(Deed::Tiling(reserve)))
        );
        assert!(asked("tiling").is_err());
        assert_eq!(asked("dock\n3"), Ok(Asked::Deed(Deed::Dock(3))));
        assert!(asked("dock\n10").is_err());
        assert!(asked("dock\nfirst").is_err());
        assert!(asked("focus\nsideways").is_err());
        assert!(asked("run\ncmd.exe").is_err());
        assert!(asked("popup").is_err());
    }

    #[test]
    fn handed_words_ask_for_an_action() {
        let lean = |arg: &str| Deed::Action {
            name: "lean".to_owned(),
            arg: Some(arg.to_owned()),
        };
        assert_eq!(asked("do\nlean\nleft"), Ok(Asked::Deed(lean("left"))));
        assert_eq!(
            asked("do\nnothing"),
            Ok(Asked::Deed(Deed::Action {
                name: "nothing".to_owned(),
                arg: None
            }))
        );
        assert_eq!(
            asked("do\nnothing\nC:\\My\nFiles"),
            Ok(Asked::Deed(Deed::Action {
                name: "nothing".to_owned(),
                arg: Some("C:\\My Files".to_owned())
            }))
        );
        assert!(asked("do\nlean\nsideways").is_err());
        assert!(asked("do\nfly").is_err());
        assert!(asked("do").is_err());
        assert!(COMMANDS.contains(&"do"));
    }

    #[test]
    fn a_key_names_an_action_and_what_it_is_done_with() {
        let mut leaning = key(None, None, None);
        leaning.action = Some("lean".to_owned());
        leaning.arg = Some("up".to_owned());
        let lean = deed(&leaning).unwrap();
        assert_eq!(
            lean,
            Deed::Action {
                name: "lean".to_owned(),
                arg: Some("up".to_owned())
            }
        );
        assert!(lean.takes_reserved_keys(&actions()));
        leaning.arg = Some("away".to_owned());
        assert!(deed(&leaning).is_err(), "the action refuses the argument");
        leaning.action = Some("fly".to_owned());
        assert!(deed(&leaning).is_err(), "no action has the name");
        leaning.action = None;
        assert!(deed(&leaning).is_err(), "an argument without an action");
        let mut both = key(Some("launcher"), None, None);
        both.action = Some("nothing".to_owned());
        assert!(deed(&both).is_err());
        let mut nothing = key(None, None, None);
        nothing.action = Some("nothing".to_owned());
        assert!(!deed(&nothing).unwrap().takes_reserved_keys(&actions()));
        let popup = deed(&key(Some("launcher"), None, None)).unwrap();
        assert!(!popup.takes_reserved_keys(&actions()));
    }

    #[test]
    fn the_keys_of_the_actions_give_way_to_those_of_the_user() {
        let mut actions = actions();
        let fold = crate::actions::Action::new(|_, _| {})
            .by_default("win+d", None)
            .by_default("Win+Shift+M", None);
        actions.register("fold", fold).unwrap();
        let mut own = key(Some("launcher"), None, None);
        own.press = "shift+win+m".to_owned();
        let user = [own];
        let presses = |defaults| -> Vec<String> {
            let keys = Shortcuts::in_use(&user, defaults, &actions);
            keys.into_iter().map(|key| key.press).collect()
        };
        assert_eq!(presses(false), ["shift+win+m"]);
        assert_eq!(presses(true), ["shift+win+m", "win+d"]);
    }

    fn key(popup: Option<&str>, tiling: Option<&str>, run: Option<&[&str]>) -> KeyConfig {
        KeyConfig {
            press: "ctrl+alt+k".to_owned(),
            popup: popup.map(str::to_owned),
            tiling: tiling.map(str::to_owned),
            run: run.map(|run| run.iter().map(|&part| part.to_owned()).collect()),
            ..KeyConfig::default()
        }
    }

    /// Actions as the shell would have them: one that takes reserved keys
    /// and a side, and one that takes anything.
    fn actions() -> Actions {
        let mut actions = Actions::default();
        let sides = |side: Option<&str>| match side.and_then(Toward::parse) {
            Some(_) => Ok(()),
            None => Err("a side".to_owned()),
        };
        let lean = crate::actions::Action::new(|_, _| {})
            .taking_reserved_keys()
            .checking(sides);
        actions.register("lean", lean).unwrap();
        let nothing = crate::actions::Action::new(|_, _| {}).taking_any_arg();
        actions.register("nothing", nothing).unwrap();
        actions
    }

    fn deed(key: &KeyConfig) -> Result<Deed, String> {
        Deed::of(key, &actions())
    }

    fn asked(text: &str) -> Result<Asked, String> {
        Asked::of(text, &actions())
    }

    #[test]
    fn a_key_that_switches_windows_knows_the_keys_held_with_it() {
        let mut switching = key(None, None, None);
        switching.press = "alt+shift+tab".to_owned();
        switching.switch = Some("previous".to_owned());
        let held = Modifiers::parse("alt+shift").unwrap();
        assert_eq!(deed(&switching), Ok(Deed::Switch { back: true, held }));
        switching.switch = Some("sideways".to_owned());
        assert!(deed(&switching).is_err());
    }

    #[test]
    fn a_key_does_the_one_thing_it_names() {
        assert_eq!(
            deed(&key(Some("launcher"), None, None)),
            Ok(Deed::Popup("launcher".to_owned()))
        );
        let tiling = |command| Ok(Deed::Tiling(command));
        assert_eq!(
            deed(&key(None, Some("next"), None)),
            tiling(TilingCommand::Next)
        );
        let grid = parse_command(&["grid"]).unwrap();
        assert!(matches!(grid, TilingCommand::Layout(_)));
        assert_eq!(deed(&key(None, Some("grid"), None)), tiling(grid));
        assert_eq!(
            deed(&key(None, Some("float"), None)),
            tiling(TilingCommand::Float)
        );
        assert_eq!(
            deed(&key(None, Some("wider"), None)),
            tiling(TilingCommand::Wider(true))
        );
        assert_eq!(
            deed(&key(None, Some("narrower"), None)),
            tiling(TilingCommand::Wider(false))
        );
        assert_eq!(
            deed(&key(None, Some("off"), None)),
            tiling(TilingCommand::Off)
        );
        assert_eq!(
            deed(&key(None, Some("height more"), None)),
            tiling(TilingCommand::Height(true))
        );
        assert_eq!(
            deed(&key(None, None, Some(&["wt.exe", "-d", "."]))),
            Ok(Deed::Run(vec!["wt.exe".into(), "-d".into(), ".".into()]))
        );
    }

    #[test]
    fn a_key_that_names_nothing_or_several_things_does_nothing() {
        assert!(deed(&key(None, None, None)).is_err());
        assert!(deed(&key(Some("launcher"), Some("next"), None)).is_err());
        assert!(deed(&key(None, None, Some(&[]))).is_err());
        assert!(deed(&key(None, Some("grid now"), None)).is_err());
        let mut both = key(Some("launcher"), None, None);
        both.focus = Some("left".to_owned());
        assert!(deed(&both).is_err());
    }

    #[test]
    fn a_key_names_the_side_of_the_window_it_is_about() {
        let mut focus = key(None, None, None);
        focus.focus = Some("left".to_owned());
        assert_eq!(deed(&focus), Ok(Deed::Focus(Toward::Left)));
        let mut carry = key(None, None, None);
        carry.carry = Some("down".to_owned());
        assert_eq!(deed(&carry), Ok(Deed::Carry(Toward::Down)));
        carry.carry = Some("away".to_owned());
        assert!(deed(&carry).is_err());
    }

    #[test]
    fn only_the_keys_that_can_be_used_are_held() {
        let mut unreadable = key(Some("launcher"), None, None);
        unreadable.press = "k".to_owned();
        let mut tiling = key(None, Some("next"), None);
        tiling.press = "ctrl+alt+t".to_owned();
        let keys = [
            key(Some("launcher"), None, None),
            unreadable,
            key(None, None, None),
            tiling,
        ];
        let read = Shortcuts::read(&keys, &actions());
        let held: Vec<u32> = read.hotkeys.iter().map(|key| key.id).collect();
        assert_eq!(held, [FIRST, FIRST + 3]);
        let usable: Vec<bool> = read.deeds.iter().map(Option::is_some).collect();
        assert_eq!(usable, [true, false, false, true]);
        assert!(matches!(read.states[1], KeyState::Broken(_)));
        assert_eq!(read.holders, [Some(0), None, None, Some(3)]);
    }

    #[test]
    fn presses_that_begin_alike_hold_their_first_combination_once() {
        let pressing = |press: &str, popup: &str| {
            let mut key = key(Some(popup), None, None);
            key.press = press.to_owned();
            key
        };
        let mut off = key(None, None, None);
        off.press = "win+d".to_owned();
        off.action = Some(NO_ACTION.to_owned());
        let mut mouse = key(Some("media"), None, None);
        mouse.press = "ctrl+back".to_owned();
        let keys = [
            pressing("ctrl+alt+k, t", "launcher"),
            pressing("Ctrl+Alt+K, Shift+U", "calendar"),
            off,
            mouse,
        ];
        let mut actions = actions();
        actions
            .register(NO_ACTION, crate::actions::Action::new(|_, _| {}))
            .unwrap();
        let read = Shortcuts::read(&keys, &actions);
        let held: Vec<u32> = read.hotkeys.iter().map(|key| key.id).collect();
        assert_eq!(held, [FIRST], "only the first combination, once");
        assert_eq!(read.holders, [Some(0), Some(0), None, None]);
        assert_eq!(read.states[2], KeyState::Off);
        assert_eq!(read.states[3], KeyState::Button);
        let button = MouseBinding {
            held: Modifiers::parse("ctrl").unwrap(),
            button: MouseButtonName::Back,
        };
        assert_eq!(read.buttons, [(3, button)]);
    }

    #[test]
    fn a_press_is_a_combination_a_sequence_or_a_mouse_button() {
        let written = |text: &str| written_press(text);
        assert_eq!(written("Win + Shift + Up").as_deref(), Some("shift+win+up"));
        assert_eq!(written("ctrl+alt+k ,T").as_deref(), Some("ctrl+alt+k, t"));
        assert_eq!(
            written("ctrl+alt+k, shift+num1, escape, f2").as_deref(),
            Some("ctrl+alt+k, shift+num1, escape, f2")
        );
        assert_eq!(written("ctrl+alt+k, t, u, v, w"), None, "five steps");
        assert_eq!(written("k, t"), None, "the first step holds keys");
        assert_eq!(written("ctrl+alt+k, "), None);
        assert_eq!(written("Ctrl+Back").as_deref(), Some("ctrl+back"));
        assert_eq!(
            written("alt+shift+middle").as_deref(),
            Some("alt+shift+middle")
        );
        assert_eq!(written("forward"), None, "a button needs keys held");
        assert_eq!(written("ctrl+back, t"), None);
        let gesture = Gesture::parse("ctrl+alt+k, t").unwrap();
        assert_eq!(gesture.shown(Language::English), "Ctrl + Alt + K, T");
        let mouse = Gesture::parse("win+forward").unwrap();
        assert_eq!(mouse.shown(Language::English), "Win + Forward button");
        assert!(mouse.steps().is_empty());
    }

    #[test]
    fn a_sequence_clashes_with_what_it_begins_with() {
        let gesture = |text| Gesture::parse(text).unwrap();
        let single = gesture("ctrl+alt+k");
        let sequence = gesture("ctrl+alt+k, t");
        assert!(single.clashes(&sequence));
        assert!(sequence.clashes(&single));
        assert!(sequence.clashes(&gesture("Ctrl+Alt+K, T")));
        assert!(!sequence.clashes(&gesture("ctrl+alt+k, u")));
        assert!(!single.clashes(&gesture("ctrl+back")));
        assert!(gesture("ctrl+back").clashes(&gesture("Ctrl+Back")));
    }

    #[test]
    fn a_sequence_goes_on_until_one_press_is_complete() {
        let steps = |text: &str| Gesture::parse(text).unwrap().steps().to_vec();
        let presses = [
            steps("ctrl+alt+k, t"),
            steps("ctrl+alt+k, u, v"),
            steps("ctrl+alt+j"),
        ];
        let among = || {
            presses
                .iter()
                .enumerate()
                .map(|(index, steps)| (index, &steps[..]))
        };
        let first = steps("ctrl+alt+k");
        let Stepped::Waiting(waiting) = Steps::after(first, among()) else {
            panic!("two presses go on");
        };
        assert_eq!(waiting.entries, [0, 1]);
        let gestures: Vec<Option<Gesture>> = { presses.iter() }
            .map(|steps| Some(Gesture::Keys(steps.clone())))
            .collect();
        let next = waiting.next(&gestures);
        let next: Vec<String> = next.iter().map(Press::step_written).collect();
        assert_eq!(next, ["t", "u"]);
        assert_eq!(
            Steps::after(steps("ctrl+alt+k, t"), among()),
            Stepped::Done(0)
        );
        assert_eq!(Steps::after(steps("ctrl+alt+j"), among()), Stepped::Done(2));
        assert_eq!(
            Steps::after(steps("ctrl+alt+k, w"), among()),
            Stepped::Nothing
        );
        let Stepped::Waiting(later) = Steps::after(steps("ctrl+alt+k, u"), among()) else {
            panic!("one press goes on");
        };
        assert_eq!(later.entries, [1]);
    }

    #[test]
    fn every_key_windows_can_name_is_offered() {
        for name in Hotkey::key_names() {
            assert!(KEYS.contains(&name), "`{name}` is not offered");
        }
        assert_eq!(key_label("num4"), "Num 4");
        assert_eq!(key_label("add"), "Num +");
        assert_eq!(key_label("menu"), "Menu");
        assert_eq!(key_label("backslash"), "\\");
    }
}
