// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The key combinations of the configuration and what each one does.
//!
//! Every combination Tessera holds is reported in one place. The ones of
//! the workspaces are passed on to them; the others are the `[[key]]`
//! entries of the configuration, each of which opens a popup, changes the
//! tiling or starts a program.

use std::process::{Command, Stdio};

use gpui::{App, AsyncApp, Global};

use crate::config::KeyConfig;
use crate::platform::{Hotkey, Hotkeys, Modifiers};
use crate::shell::Shell;
use crate::tiling::{TileLayout, Tiler, TilingMode, Toward};
use crate::workspaces::Workspaces;

/// The keys a combination ends with, as the configuration names them.
pub const KEYS: [&str; 55] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
    "t", "u", "v", "w", "x", "y", "z", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "f1",
    "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12", "space", "tab", "enter",
    "left", "up", "right", "down",
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
        "space" | "tab" | "enter" => {
            let (first, rest) = key.split_at(1);
            format!("{}{rest}", first.to_ascii_uppercase())
        }
        _ => key.to_ascii_uppercase(),
    }
}

/// The number the first `[[key]]` is reported by; the workspaces have the
/// numbers below it.
const FIRST: u32 = 1 << 16;

/// What a `[[key]]` does.
#[derive(Clone, Debug, PartialEq)]
enum Deed {
    Popup(String),
    /// The tiling mode to take; `None` for the one after the mode in use.
    Tiling(Option<TilingMode>),
    Run(Vec<String>),
    Focus(Toward),
    Carry(Toward),
}

impl Deed {
    /// What `key` does, or why it does nothing.
    fn of(key: &KeyConfig) -> Result<Self, String> {
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
        ];
        let mut deeds = deeds.into_iter().flatten();
        match (deeds.next(), deeds.next()) {
            (Some(deed), None) => deed,
            _ => Err("it needs one of `popup`, `tiling`, `run`, `focus` and `move`".to_owned()),
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
            (_, Some(&layout)) => Ok(mode(true, layout)),
            _ => Err(format!(
                "`tiling` is `next`, `off` or a layout, not `{name}`"
            )),
        }
    }

    fn done(&self, cx: &mut App) {
        match self {
            Self::Popup(name) => Shell::toggle_popup(name, cx),
            Self::Tiling(mode) => {
                let mode = mode.unwrap_or_else(|| cx.global::<TilingMode>().next());
                Tiler::set_mode(mode, cx);
            }
            Self::Focus(toward) => Tiler::focus(*toward, cx),
            Self::Carry(toward) => Tiler::carry(*toward, cx),
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
        Self::configure(keys, cx);
    }

    /// Takes over the entries of a configuration that was read again, if
    /// they are other than the ones before.
    pub fn configure(keys: &[KeyConfig], cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if this.config == keys {
            return;
        }
        this.config = keys.to_vec();
        let (deeds, hotkeys) = Self::read(keys);
        this.deeds = deeds;
        match Hotkeys::set("keys", hotkeys) {
            Ok(refused) => {
                for id in refused {
                    let press = &keys[(id - FIRST) as usize].press;
                    report!("Windows did not give the keys `{press}`: they are held already");
                }
            }
            Err(error) => report!("the `[[key]]` entries have no keys: {error:#}"),
        }
    }

    /// What each entry does and the combination of each one that can be
    /// used; the others are reported.
    fn read(keys: &[KeyConfig]) -> (Vec<Option<Deed>>, Vec<Hotkey>) {
        let mut hotkeys = Vec::new();
        let deeds = keys.iter().enumerate().map(|(index, key)| {
            let press = &key.press;
            let Some(hotkey) = Hotkey::parse(FIRST + index as u32, press) else {
                report!("a `[[key]]` presses keys to hold and a key joined by +, not `{press}`");
                return None;
            };
            match Deed::of(key) {
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
        assert_eq!(Press::parse("ctrl+escape"), None);
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
    }

    fn key(popup: Option<&str>, tiling: Option<&str>, run: Option<&[&str]>) -> KeyConfig {
        KeyConfig {
            press: "ctrl+alt+k".to_owned(),
            popup: popup.map(str::to_owned),
            tiling: tiling.map(str::to_owned),
            run: run.map(|run| run.iter().map(|&part| part.to_owned()).collect()),
            focus: None,
            carry: None,
        }
    }

    #[test]
    fn a_key_does_the_one_thing_it_names() {
        assert_eq!(
            Deed::of(&key(Some("launcher"), None, None)),
            Ok(Deed::Popup("launcher".to_owned()))
        );
        assert_eq!(
            Deed::of(&key(None, Some("next"), None)),
            Ok(Deed::Tiling(None))
        );
        let grid = TilingMode {
            enabled: true,
            layout: TileLayout::Grid,
        };
        assert_eq!(
            Deed::of(&key(None, Some("grid"), None)),
            Ok(Deed::Tiling(Some(grid)))
        );
        let off = Deed::of(&key(None, Some("off"), None));
        assert!(matches!(off, Ok(Deed::Tiling(Some(mode))) if !mode.enabled));
        assert_eq!(
            Deed::of(&key(None, None, Some(&["wt.exe", "-d", "."]))),
            Ok(Deed::Run(vec!["wt.exe".into(), "-d".into(), ".".into()]))
        );
    }

    #[test]
    fn a_key_that_names_nothing_or_several_things_does_nothing() {
        assert!(Deed::of(&key(None, None, None)).is_err());
        assert!(Deed::of(&key(Some("launcher"), Some("next"), None)).is_err());
        assert!(Deed::of(&key(None, None, Some(&[]))).is_err());
        assert!(Deed::of(&key(None, Some("spiral"), None)).is_err());
        let mut both = key(Some("launcher"), None, None);
        both.focus = Some("left".to_owned());
        assert!(Deed::of(&both).is_err());
    }

    #[test]
    fn a_key_names_the_side_of_the_window_it_is_about() {
        let mut focus = key(None, None, None);
        focus.focus = Some("left".to_owned());
        assert_eq!(Deed::of(&focus), Ok(Deed::Focus(Toward::Left)));
        let mut carry = key(None, None, None);
        carry.carry = Some("down".to_owned());
        assert_eq!(Deed::of(&carry), Ok(Deed::Carry(Toward::Down)));
        carry.carry = Some("away".to_owned());
        assert!(Deed::of(&carry).is_err());
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
        let (deeds, hotkeys) = Shortcuts::read(&keys);
        let held: Vec<u32> = hotkeys.iter().map(|key| key.id).collect();
        assert_eq!(held, [FIRST, FIRST + 3]);
        let usable: Vec<bool> = deeds.iter().map(Option::is_some).collect();
        assert_eq!(usable, [true, false, false, true]);
    }
}
