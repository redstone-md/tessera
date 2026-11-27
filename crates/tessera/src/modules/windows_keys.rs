// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The keys of Windows that Explorer answers, answered by Tessera where
//! its keys of Windows are in use (`windows_keys`, by default when
//! Tessera is the shell), as actions a `[[key]]` can name as well:
//!
//! - `open` opens what its `arg` names through the Shell: Win+I the
//!   settings, Win+U those of accessibility, Win+K the wireless displays,
//!   Win+G the Game Bar, Win+Shift+S the snipping of the screen and
//!   Win+Pause the page About;
//! - `start` (Ctrl+Esc), `search` (Win+S, Win+Q), `quick-settings`
//!   (Win+A) and `notification-center` (Win+N) open the popup their
//!   `arg` names, or without one, for `start`, the popup of the Windows
//!   key (`windows_key`, or else the launcher, as the Windows key opens
//!   it), and `launcher`, `quick-settings` and `calendar`; the module
//!   brings the popup `quick-settings`, made of the widgets of the
//!   connections, the radios, the volume and the brightness;
//! - `screenshot` (Win+PrtScn) saves a picture of all displays in the
//!   Screenshots folder;
//! - `dock` (Win+1 to Win+9) does what a click on the button of that
//!   number in the dock does, as `dock = n` of a `[[key]]` does; the dock
//!   brings its other keys.
//!
//! Win+L stays with Windows, which locks the session itself; so do
//! Ctrl+Shift+Esc and the keys that switch the input language.

use anyhow::anyhow;
use gpui::App;

use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, WidgetSpec};
use crate::module::{Module, WidgetRegistry};
use crate::modules::windows::Docks;
use crate::platform::{Launch, save_screenshot};
use crate::shell::Shell;

/// What the keys of `open` open, as Explorer opens them.
const OPENED: [(&str, &str); 6] = [
    ("win+i", "ms-settings:"),
    ("win+u", "ms-settings:easeofaccess"),
    ("win+k", "ms-settings-connectabledevices:devicediscovery"),
    ("win+g", "ms-gamebar:"),
    ("win+shift+s", "ms-screenclip:"),
    ("win+pause", "ms-settings:about"),
];

/// The actions that open a popup: the name of each, the popup it opens
/// without an `arg` (for `start`, `None`: that of the Windows key) and
/// the keys it brings.
const POPUPS: [(&str, Option<&str>, &[&str]); 4] = [
    ("start", None, &["ctrl+escape"]),
    ("search", Some("launcher"), &["win+s", "win+q"]),
    ("quick-settings", Some(QUICK_SETTINGS), &["win+a"]),
    ("notification-center", Some("calendar"), &["win+n"]),
];

/// The popup of Win+A and the widgets it holds, from the top, each with
/// the radio it switches where it is a `radio-switch`.
const QUICK_SETTINGS: &str = "quick-settings";
const QUICK_SETTINGS_CONTENT: [(&str, Option<&str>); 5] = [
    ("connections", None),
    ("radio-switch", Some("wifi")),
    ("radio-switch", Some("bluetooth")),
    ("volume-slider", None),
    ("brightness-slider", None),
];

pub struct WindowsKeysModule;

impl Module for WindowsKeysModule {
    fn name(&self) -> &'static str {
        "windows-keys"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        let spec = |(widget, radio): (&str, Option<&str>)| {
            let mut options = toml::Table::new();
            if let Some(radio) = radio {
                options.insert("radio".to_owned(), radio.into());
            }
            WidgetSpec {
                widget: widget.to_owned(),
                popup: None,
                options,
            }
        };
        widgets.register_popup(PopupConfig {
            name: QUICK_SETTINGS.to_owned(),
            place: Default::default(),
            width: 360.,
            height: 420.,
            content: QUICK_SETTINGS_CONTENT.into_iter().map(spec).collect(),
            side: Vec::new(),
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        // Each is registered by itself, so that a name another part took
        // costs only that action.
        let failed: Vec<String> = { Self::all().into_iter() }
            .filter_map(|(name, action)| actions.register(name, action).err())
            .map(|error| format!("{error:#}"))
            .collect();
        match failed.is_empty() {
            true => Ok(()),
            false => Err(anyhow!(failed.join("; "))),
        }
    }
}

impl WindowsKeysModule {
    /// The actions of the module, by name.
    fn all() -> Vec<(&'static str, Action)> {
        // Every one of these keys is kept by Explorer while it runs.
        let mut open = Action::new(|target, _| open(target.unwrap_or_default()))
            .taking_reserved_keys()
            .checking(|target| match target {
                Some(target) if !target.trim().is_empty() => Ok(()),
                _ => Err(
                    "`open` takes what to open as `arg`: a file, a program or an address"
                        .to_owned(),
                ),
            });
        for (press, target) in OPENED {
            open = open.by_default(press, Some(target));
        }
        let mut all = vec![("open", open)];
        for (name, popup, presses) in POPUPS {
            let mut action = Action::new(move |arg, cx| match arg.or(popup) {
                Some(name) => toggle(name, cx),
                // Ctrl+Esc opens what the Windows key opens.
                None => Shell::start_requested(cx),
            })
            .taking_reserved_keys()
            .checking(|arg| match arg {
                Some(name) if name.trim().is_empty() => {
                    Err("the name of a popup is not empty".to_owned())
                }
                _ => Ok(()),
            });
            for press in presses {
                action = action.by_default(press, None);
            }
            all.push((name, action));
        }
        let screenshot = Action::new(|_, cx| screenshot(cx))
            .taking_reserved_keys()
            .by_default("win+printscreen", None);
        all.push(("screenshot", screenshot));
        let mut dock = Action::new(|number, cx| {
            let number = number.and_then(|number| number.parse().ok());
            Docks::press(number.unwrap_or(0), cx);
        })
        .taking_reserved_keys()
        .checking(|number| match number.map(str::parse::<usize>) {
            Some(Ok(1..=9)) => Ok(()),
            _ => Err("`dock` takes a number from 1 to 9 as `arg`".to_owned()),
        });
        // Win+1 to Win+9 press the buttons of the dock, as on the taskbar.
        for number in 1..=9 {
            let number = number.to_string();
            dock = dock.by_default(&format!("win+{number}"), Some(&number));
        }
        all.push(("dock", dock));
        all
    }
}

/// Opens `target` as the Shell opens it.
fn open(target: &str) {
    Launch::new(target.trim()).start();
}

/// Opens the popup `name`, or closes it when it is open.
fn toggle(name: &str, cx: &mut App) {
    Shell::toggle_popup(name.trim(), cx);
}

/// Saves a picture of every display, away from the thread that draws.
fn screenshot(cx: &mut App) {
    cx.background_executor()
        .spawn(async {
            if let Err(error) = save_screenshot() {
                report!("no screenshot was saved: {error:#}");
            }
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions() -> Actions {
        let mut actions = Actions::default();
        WindowsKeysModule.actions(&mut actions).unwrap();
        actions
    }

    #[test]
    fn the_keys_of_windows_are_bound_to_what_they_do_there() {
        let keys = actions().default_keys();
        let bound = |press: &str| {
            let key = keys.iter().find(|key| key.press == press);
            key.map(|key| (key.action.clone().unwrap(), key.arg.clone()))
        };
        assert_eq!(
            bound("win+i"),
            Some(("open".to_owned(), Some("ms-settings:".to_owned())))
        );
        assert_eq!(bound("win+q"), Some(("search".to_owned(), None)));
        assert_eq!(bound("ctrl+escape"), Some(("start".to_owned(), None)));
        assert_eq!(
            bound("win+printscreen"),
            Some(("screenshot".to_owned(), None))
        );
        assert_eq!(bound("win+l"), None, "Windows locks by itself");
        let mut presses: Vec<&str> = keys.iter().map(|key| key.press.as_str()).collect();
        let count = presses.len();
        presses.sort_unstable();
        presses.dedup();
        assert_eq!(presses.len(), count, "a key is bound twice");
    }

    #[test]
    fn every_key_brought_is_one_tessera_can_hold() {
        for key in actions().default_keys() {
            assert!(
                crate::keys::Press::parse(&key.press).is_some(),
                "`{}` is no combination",
                key.press
            );
        }
    }

    #[test]
    fn the_actions_say_what_they_take() {
        let actions = actions();
        assert!(actions.check("open", None).is_err());
        assert_eq!(actions.check("open", Some("ms-settings:display")), Ok(()));
        assert_eq!(actions.check("quick-settings", None), Ok(()));
        assert_eq!(actions.check("search", Some("runner")), Ok(()));
        assert!(actions.check("search", Some(" ")).is_err());
        assert!(actions.check("screenshot", Some("now")).is_err());
        assert_eq!(actions.check("dock", Some("3")), Ok(()));
        assert!(actions.check("dock", Some("10")).is_err());
        assert!(actions.check("dock", None).is_err());
        for name in ["open", "start", "search", "screenshot", "dock"] {
            assert!(actions.takes_reserved_keys(name));
        }
    }

    #[test]
    fn a_name_taken_before_costs_only_that_action() {
        let mut actions = Actions::default();
        actions.register("open", Action::new(|_, _| {})).unwrap();
        assert!(WindowsKeysModule.actions(&mut actions).is_err());
        for name in ["start", "search", "quick-settings", "screenshot", "dock"] {
            assert!(actions.get(name).is_some(), "`{name}` is missing");
        }
    }

    #[test]
    fn the_popup_of_win_a_is_brought_by_the_module() {
        let mut widgets = WidgetRegistry::default();
        WindowsKeysModule.register(&mut widgets).unwrap();
        let popup = widgets.popup(QUICK_SETTINGS).unwrap();
        let radios: Vec<_> = { popup.content.iter() }
            .filter_map(|spec| spec.options.get("radio"))
            .filter_map(|radio| radio.as_str())
            .collect();
        assert_eq!(radios, ["wifi", "bluetooth"]);
    }
}
