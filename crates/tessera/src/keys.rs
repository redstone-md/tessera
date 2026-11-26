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

use gpui::{App, AsyncApp, Global};

use crate::actions::Actions;
use crate::config::KeyConfig;
use crate::files::Files;
use crate::modules::windows::Docks;
use crate::platform::{Commands, Hotkey, Hotkeys, Modifiers, TakenKeys};
use crate::shell::Shell;
use crate::switcher::Switcher;
use crate::tiling::{TileLayout, Tiler, TilingMode, Toward};
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
        "space" | "tab" | "enter" | "home" | "end" | "insert" | "delete" | "backspace"
        | "pause" => {
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
            ["tiling", name] => key.tiling = said(name),
            ["focus", side] => key.focus = said(side),
            ["move", side] => key.carry = said(side),
            // A word that is no number is one no application has.
            ["dock", number] => key.dock = Some(number.parse().unwrap_or(0)),
            ["files"] => key.files = said(""),
            ["files", folder] => key.files = said(folder),
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

/// What a `[[key]]` does.
#[derive(Clone, Debug, PartialEq)]
enum Deed {
    Popup(String),
    /// The tiling mode to take; `None` for the one after the mode in use.
    Tiling(Option<TilingMode>),
    /// Take the window in front out of the tiles, or put it in again.
    Float,
    /// Gives the first window of the `master` layout more of the width,
    /// or less.
    Resize(bool),
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
    /// Opens the file manager at the folder named so.
    Files(String),
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
            key.run.as_ref().map(|run| match run.is_empty() {
                true => Err("`run` names no program".to_owned()),
                false => Ok(Self::Run(run.clone())),
            }),
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
                .map(|folder| Ok(Self::Files(folder.clone()))),
            key.action
                .as_deref()
                .map(|name| Self::action(name, key.arg.as_deref(), actions)),
        ];
        if key.arg.is_some() && key.action.is_none() {
            return Err("`arg` is what an `action` is done with, and there is none".to_owned());
        }
        let mut deeds = deeds.into_iter().flatten();
        match (deeds.next(), deeds.next()) {
            (Some(deed), None) => deed,
            _ => Err(
                "it needs one of `popup`, `tiling`, `run`, `focus`, `move`, `switch`, `dock`, `files` and `action`"
                    .to_owned(),
            ),
        }
    }

    fn action(name: &str, arg: Option<&str>, actions: &Actions) -> Result<Self, String> {
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

    fn tiling(name: &str) -> Result<Self, String> {
        let mode = |enabled, layout| Self::Tiling(Some(TilingMode { enabled, layout }));
        // A layout is named in the configuration without its capital.
        let named = |layout: &&TileLayout| layout.name().eq_ignore_ascii_case(name);
        let layout = TileLayout::ALL.iter().find(named);
        match (name, layout) {
            ("next", _) => Ok(Self::Tiling(None)),
            ("off", _) => Ok(mode(false, TileLayout::ALL[0])),
            ("float", _) => Ok(Self::Float),
            ("wider", _) => Ok(Self::Resize(true)),
            ("narrower", _) => Ok(Self::Resize(false)),
            (_, Some(&layout)) => Ok(mode(true, layout)),
            _ => Err(format!(
                "`tiling` is `next`, `off`, `float`, `wider`, `narrower` or a layout, not `{name}`"
            )),
        }
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

    fn done(&self, cx: &mut App) {
        match self {
            Self::Switch { back, held } => Switcher::step(*back, *held, cx),
            Self::Popup(name) => Shell::toggle_popup(name, cx),
            Self::Tiling(mode) => {
                let mode = mode.unwrap_or_else(|| cx.global::<TilingMode>().next());
                Tiler::set_mode(mode, cx);
            }
            Self::Float => Tiler::float(cx),
            Self::Resize(wider) => Tiler::resize(*wider, cx),
            Self::Focus(toward) => Tiler::focus(*toward, cx),
            Self::Carry(toward) => Tiler::carry(*toward, cx),
            Self::Dock(number) => Docks::press(*number, cx),
            Self::Files(folder) => Files::open(Files::named(folder), cx),
            Self::Action { name, arg } => Actions::run(name, arg.as_deref(), cx),
            Self::Run(command) => {
                let started = Command::new(&command[0])
                    .args(&command[1..])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                if let Err(error) = started {
                    report!("`{}` did not start: {error}", command[0]);
                }
            }
        }
    }
}

/// The `[[key]]` entries in use.
#[derive(Default)]
pub struct Shortcuts {
    config: Vec<KeyConfig>,
    /// What each entry does; `None` for one that cannot be used.
    deeds: Vec<Option<Deed>>,
}

impl Global for Shortcuts {}

impl Shortcuts {
    /// Takes the combinations of `keys` and answers every combination
    /// Tessera holds from now on.
    pub fn start(keys: &[KeyConfig], cx: &mut App) {
        cx.set_global(Self::default());
        let presses = Hotkeys::presses();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(pressed) = presses.recv().await {
                let answered = cx.update(|cx| match pressed.checked_sub(FIRST) {
                    Some(index) => Self::pressed(index as usize, cx),
                    None => Workspaces::pressed(pressed, cx),
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
        Self::configure(keys, cx);
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
    /// they are other than the ones before.
    pub fn configure(keys: &[KeyConfig], cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if this.config == keys {
            return;
        }
        this.config = keys.to_vec();
        let actions = Self::actions(cx);
        let (deeds, hotkeys) = Self::read(keys, actions);
        // Windows keeps Alt+Tab, the Windows key with a digit and Win+E,
        // which opens Explorer, to itself. The keys of the window
        // switcher, of the dock, of the file manager and of the actions
        // that ask for it that it refuses are taken from it; a refused
        // combination of anything else is left to whoever holds it.
        let switches = |id: u32| {
            let deed = deeds.get((id - FIRST) as usize);
            deed.is_some_and(|deed| {
                deed.as_ref()
                    .is_some_and(|deed| deed.takes_reserved_keys(actions))
            })
        };
        let refused = Hotkeys::set("keys", hotkeys.clone()).unwrap_or_else(|error| {
            report!("the `[[key]]` entries have no keys: {error:#}");
            hotkeys.iter().map(|hotkey| hotkey.id).collect()
        });
        let (taken, refused): (Vec<u32>, Vec<u32>) =
            refused.into_iter().partition(|&id| switches(id));
        for id in refused {
            let press = &keys[(id - FIRST) as usize].press;
            report!("Windows did not give the keys `{press}`: they are held already");
        }
        let taken = hotkeys
            .into_iter()
            .filter(|hotkey| taken.contains(&hotkey.id));
        if let Err(error) = TakenKeys::set(taken.collect()) {
            report!("the keys Windows keeps to itself stay with it: {error:#}");
        }
        cx.global_mut::<Self>().deeds = deeds;
    }

    /// The actions a key can name; none before the shell has collected
    /// them.
    fn actions(cx: &mut App) -> &Actions {
        cx.default_global::<Actions>()
    }

    /// What each entry does and the combination of each one that can be
    /// used; the others are reported.
    fn read(keys: &[KeyConfig], actions: &Actions) -> (Vec<Option<Deed>>, Vec<Hotkey>) {
        let mut hotkeys = Vec::new();
        let deeds = keys.iter().enumerate().map(|(index, key)| {
            let press = &key.press;
            let Some(hotkey) = Hotkey::parse(FIRST + index as u32, press) else {
                report!("a `[[key]]` presses keys to hold and a key joined by +, not `{press}`");
                return None;
            };
            match Deed::of(key, actions) {
                Ok(deed) => {
                    hotkeys.push(hotkey);
                    Some(deed)
                }
                Err(why) => {
                    report!("the `[[key]]` of `{press}` does nothing: {why}");
                    None
                }
            }
        });
        (deeds.collect(), hotkeys)
    }

    fn pressed(index: usize, cx: &mut App) {
        let deed = cx.global::<Self>().deeds.get(index).cloned().flatten();
        if let Some(deed) = deed {
            deed.done(cx);
        }
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
        assert_eq!(asked("tiling\nfloat"), Ok(Asked::Deed(Deed::Float)));
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
        let nothing = crate::actions::Action::new(|_, _| {});
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
        assert_eq!(deed(&key(None, Some("next"), None)), Ok(Deed::Tiling(None)));
        let grid = TilingMode {
            enabled: true,
            layout: TileLayout::Grid,
        };
        assert_eq!(
            deed(&key(None, Some("grid"), None)),
            Ok(Deed::Tiling(Some(grid)))
        );
        assert_eq!(deed(&key(None, Some("float"), None)), Ok(Deed::Float));
        assert_eq!(
            deed(&key(None, Some("wider"), None)),
            Ok(Deed::Resize(true))
        );
        assert_eq!(
            deed(&key(None, Some("narrower"), None)),
            Ok(Deed::Resize(false))
        );
        let off = deed(&key(None, Some("off"), None));
        assert!(matches!(off, Ok(Deed::Tiling(Some(mode))) if !mode.enabled));
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
        assert!(deed(&key(None, Some("spiral"), None)).is_err());
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
        let keys = [
            key(Some("launcher"), None, None),
            unreadable,
            key(None, None, None),
            key(None, Some("next"), None),
        ];
        let (deeds, hotkeys) = Shortcuts::read(&keys, &actions());
        let held: Vec<u32> = hotkeys.iter().map(|key| key.id).collect();
        assert_eq!(held, [FIRST, FIRST + 3]);
        let usable: Vec<bool> = deeds.iter().map(Option::is_some).collect();
        assert_eq!(usable, [true, false, false, true]);
    }
}
