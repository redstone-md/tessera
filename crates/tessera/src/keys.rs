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
use crate::platform::{Hotkey, Hotkeys};
use crate::shell::Shell;
use crate::tiling::{TileLayout, Tiler, TilingMode};
use crate::workspaces::Workspaces;

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
}

impl Deed {
    /// What `key` does, or why it does nothing.
    fn of(key: &KeyConfig) -> Result<Self, String> {
        match (&key.popup, &key.tiling, &key.run) {
            (Some(popup), None, None) => Ok(Self::Popup(popup.clone())),
            (None, Some(tiling), None) => Self::tiling(tiling),
            (None, None, Some(run)) if !run.is_empty() => Ok(Self::Run(run.clone())),
            (None, None, Some(_)) => Err("`run` names no program".to_owned()),
            _ => Err("it needs one of `popup`, `tiling` and `run`".to_owned()),
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

    fn key(popup: Option<&str>, tiling: Option<&str>, run: Option<&[&str]>) -> KeyConfig {
        KeyConfig {
            press: "ctrl+alt+k".to_owned(),
            popup: popup.map(str::to_owned),
            tiling: tiling.map(str::to_owned),
            run: run.map(|run| run.iter().map(|&part| part.to_owned()).collect()),
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
