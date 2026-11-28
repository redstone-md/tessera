// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The rest of the keys Explorer answers with the Windows key, answered by
//! Tessera where its keys of Windows are in use (`windows_keys`, by
//! default when Tessera is the shell), as actions a `[[key]]` can name as
//! well:
//!
//! - `date-and-time` (Win+Alt+D) opens the popup `calendar`, and
//!   `notifications-keyboard` (Win+Shift+V) the same popup, the
//!   notification centre;
//! - `sound-output` (Win+Ctrl+V) opens the popup `sound`, with the outputs
//!   to choose from;
//! - `window-height` stretches the window in front to the full height of
//!   the work area of its display with `full` (Win+Shift+Up) and gives it
//!   its height back with `back` (Win+Shift+Down);
//! - `window-half` puts it in the top half with `top` (Win+Alt+Up) or the
//!   bottom half with `bottom` (Win+Alt+Down), through `snap` where it
//!   takes `top-half` and `bottom-half`;
//! - `magnifier` starts Magnifier with `in` (Win+Plus), which then answers
//!   Win+Plus and Win+Minus (`out`) itself, and closes it with `close`
//!   (Win+Esc);
//! - `narrator` (Win+Ctrl+Enter), `color-filters` (Win+Ctrl+C),
//!   `quick-assist` (Win+Ctrl+Q), `feedback-hub` (Win+F) and `copilot`
//!   (Win+C) start those programs of Windows;
//! - `bar-settings` (Win+Alt+Enter) opens the settings at Bars;
//! - `find-computers` (Win+Ctrl+F) opens the search for computers of the
//!   network;
//! - `emoji-panel`, which brings no key, opens the popup `emoji-panel`: the
//!   runner with only its emoji, for Win+period where Windows' own panel
//!   does not answer.

use std::collections::HashMap;
use std::path::Path;

use anyhow::anyhow;
use gpui::{App, Global};

use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, PopupPlace, WidgetSpec};
use crate::files::Files;
use crate::module::{Module, WidgetRegistry};
use crate::modules::Settings;
use crate::platform::{AppWindow, AppWindowId, Display, Launch, ScreenRect, start_explorer};
use crate::shell::Shell;

/// The parsing name of the search for computers of the network.
const FIND_COMPUTERS: &str = "::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}";

/// The program of Magnifier.
const MAGNIFIER: &str = "magnify.exe";

/// The popup of `emoji-panel`, and its size.
const EMOJI_PANEL: &str = "emoji-panel";
const EMOJI_WIDTH: f32 = 520.;
const EMOJI_HEIGHT: f32 = 380.;

/// What the actions that start a program of Windows start, and the key
/// each brings.
const STARTED: [(&str, &str, &str); 5] = [
    ("narrator", "narrator.exe", "win+ctrl+enter"),
    (
        "color-filters",
        "ms-settings:easeofaccess-colorfilter",
        "win+ctrl+c",
    ),
    ("quick-assist", "ms-quick-assist:", "win+ctrl+q"),
    ("feedback-hub", "feedback-hub:", "win+f"),
    ("copilot", "ms-copilot:", "win+c"),
];

/// The actions that open a popup, the popup each opens and its key.
const POPUPS: [(&str, &str, &str); 3] = [
    ("date-and-time", "calendar", "win+alt+d"),
    ("notifications-keyboard", "calendar", "win+shift+v"),
    ("sound-output", "sound", "win+ctrl+v"),
];

pub struct MoreWindowsKeysModule;

impl Module for MoreWindowsKeysModule {
    fn name(&self) -> &'static str {
        "more-windows-keys"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        let mut options = toml::Table::new();
        options.insert("runners".to_owned(), vec!["emoji"].into());
        options.insert("recent".to_owned(), false.into());
        widgets.register_popup(PopupConfig {
            name: EMOJI_PANEL.to_owned(),
            place: PopupPlace::Top,
            width: EMOJI_WIDTH,
            height: EMOJI_HEIGHT,
            content: vec![WidgetSpec {
                widget: "runner".to_owned(),
                popup: None,
                options,
            }],
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

impl MoreWindowsKeysModule {
    /// The actions of the module, by name. Every key of them is one
    /// Explorer keeps while it runs.
    fn all() -> Vec<(&'static str, Action)> {
        let mut all = Vec::new();
        for (name, popup, press) in POPUPS {
            let action = Action::new(move |_, cx| Shell::toggle_popup(popup, cx))
                .taking_reserved_keys()
                .by_default(press, None);
            all.push((name, action));
        }
        for (name, target, press) in STARTED {
            let action = Action::new(move |_, _| Launch::new(target).start())
                .taking_reserved_keys()
                .by_default(press, None);
            all.push((name, action));
        }
        let height = Action::new(|arg, cx| stretch(arg == Some("full"), cx))
            .taking_reserved_keys()
            .checking(|arg| one_of(arg, "window-height", &["full", "back"]))
            .by_default("win+shift+up", Some("full"))
            .by_default("win+shift+down", Some("back"));
        all.push(("window-height", height));
        let half = Action::new(|arg, cx| half(arg == Some("top"), cx))
            .taking_reserved_keys()
            .checking(|arg| one_of(arg, "window-half", &["top", "bottom"]))
            .by_default("win+alt+up", Some("top"))
            .by_default("win+alt+down", Some("bottom"));
        all.push(("window-half", half));
        let magnifier = Action::new(|arg, _| magnify(arg.unwrap_or_default()))
            .taking_reserved_keys()
            .checking(|arg| one_of(arg, "magnifier", &["in", "out", "close"]))
            .by_default("win+plus", Some("in"))
            .by_default("win+minus", Some("out"))
            .by_default("win+escape", Some("close"));
        all.push(("magnifier", magnifier));
        let bars = Action::new(|_, cx| Settings::open_at("Bars", None, cx))
            .taking_reserved_keys()
            .by_default("win+alt+enter", None);
        all.push(("bar-settings", bars));
        let find = Action::new(|_, cx| find_computers(cx))
            .taking_reserved_keys()
            .by_default("win+ctrl+f", None);
        all.push(("find-computers", find));
        let emoji = Action::new(|_, cx| Shell::toggle_popup(EMOJI_PANEL, cx));
        all.push(("emoji-panel", emoji));
        all
    }
}

/// Whether `arg` is one of `words`, which the action `name` takes.
fn one_of(arg: Option<&str>, name: &str, words: &[&str]) -> Result<(), String> {
    match arg {
        Some(arg) if words.contains(&arg) => Ok(()),
        _ => Err(format!("`{name}` takes `{}`", words.join("` or `"))),
    }
}

/// The window in front, unless it is one of Tessera's own.
fn front() -> Option<AppWindowId> {
    AppWindowId::foreground().filter(|id| id.process() != std::process::id())
}

/// The heights the windows had before Win+Shift+Up stretched them: their
/// top and bottom, by window.
#[derive(Default)]
struct Heights(HashMap<isize, (i32, i32)>);

impl Global for Heights {}

/// What `frame` becomes stretched from the top to the bottom of `work`.
fn stretched(frame: ScreenRect, work: ScreenRect) -> ScreenRect {
    ScreenRect {
        top: work.top,
        bottom: work.bottom,
        ..frame
    }
}

/// The top or the bottom half of `work`.
fn half_of(work: ScreenRect, top: bool) -> ScreenRect {
    let middle = work.top + (work.bottom - work.top) / 2;
    match top {
        true => ScreenRect {
            bottom: middle,
            ..work
        },
        false => ScreenRect {
            top: middle,
            ..work
        },
    }
}

/// Stretches the window in front to the full height of the work area of
/// its display with `full`, keeping its sides, and gives it the height it
/// had before otherwise.
fn stretch(full: bool, cx: &mut App) {
    let Some(id) = front() else {
        return;
    };
    let (Some(frame), Some(display)) = (id.restored_frame(), Display::of_window(id)) else {
        return;
    };
    let heights = &mut cx.default_global::<Heights>().0;
    heights.retain(|raw, _| AppWindowId::from_raw(*raw).exists());
    if full {
        heights.entry(id.raw()).or_insert((frame.top, frame.bottom));
        id.place_restored(stretched(frame, display.work), false);
    } else if let Some((top, bottom)) = heights.remove(&id.raw()) {
        id.place_restored(
            ScreenRect {
                top,
                bottom,
                ..frame
            },
            false,
        );
    }
}

/// Puts the window in front in the top or the bottom half of its display:
/// through `snap` where it takes the halves, and otherwise by placing it.
fn half(top: bool, cx: &mut App) {
    let word = match top {
        true => "top-half",
        false => "bottom-half",
    };
    let snaps = { cx.try_global::<Actions>() }
        .is_some_and(|actions| actions.check("snap", Some(word)).is_ok());
    if snaps {
        Actions::run("snap", Some(word), cx);
        return;
    }
    let Some(id) = front() else {
        return;
    };
    if let Some(display) = Display::of_window(id) {
        id.place_restored(half_of(display.work, top), false);
    }
}

/// Whether `window` is one of Magnifier.
fn is_magnifier(window: &AppWindow) -> bool {
    { window.executable.as_deref().and_then(Path::file_name) }
        .is_some_and(|name| name.eq_ignore_ascii_case(MAGNIFIER))
}

/// Starts Magnifier with `in` where it is not running; once it runs it
/// answers its keys itself, which then do not reach Tessera. `close`
/// closes its windows.
fn magnify(arg: &str) {
    let windows: Vec<AppWindow> = { AppWindow::enumerate().into_iter() }
        .filter(is_magnifier)
        .collect();
    match arg {
        "close" => windows.iter().for_each(|window| window.id.close()),
        "in" if windows.is_empty() => Launch::new(MAGNIFIER).start(),
        _ => {}
    }
}

/// Opens the search for computers of the network in Tessera's file
/// manager, or in Explorer where it does not take it.
fn find_computers(cx: &mut App) {
    if Files::open_shell(FIND_COMPUTERS, cx) {
        return;
    }
    if let Err(error) = start_explorer(FIND_COMPUTERS, true) {
        report!("the search for computers did not open: {error:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions() -> Actions {
        let mut actions = Actions::default();
        MoreWindowsKeysModule.actions(&mut actions).unwrap();
        actions
    }

    #[test]
    fn every_key_is_one_of_the_table_of_wave_three() {
        let presses: Vec<String> = { actions().default_keys().into_iter() }
            .map(|key| key.press)
            .collect();
        let table = [
            "win+alt+d",
            "win+shift+up",
            "win+shift+down",
            "win+alt+up",
            "win+alt+down",
            "win+plus",
            "win+minus",
            "win+escape",
            "win+ctrl+enter",
            "win+ctrl+c",
            "win+ctrl+q",
            "win+f",
            "win+c",
            "win+ctrl+v",
            "win+shift+v",
            "win+alt+enter",
            "win+ctrl+f",
        ];
        let mut sorted = presses.clone();
        sorted.sort();
        let mut expected = table.map(str::to_owned).to_vec();
        expected.sort();
        assert_eq!(sorted, expected);
    }

    #[test]
    fn the_actions_take_their_words() {
        let actions = actions();
        assert_eq!(actions.check("window-height", Some("full")), Ok(()));
        assert!(actions.check("window-height", None).is_err());
        assert_eq!(actions.check("magnifier", Some("close")), Ok(()));
        assert!(actions.check("magnifier", Some("zoom")).is_err());
        assert!(actions.check("emoji-panel", Some("x")).is_err());
        assert!(actions.takes_reserved_keys("copilot"));
    }

    #[test]
    fn a_window_is_stretched_and_halved_in_the_work_area() {
        let work = ScreenRect {
            left: 0,
            top: 40,
            right: 1920,
            bottom: 1080,
        };
        let frame = ScreenRect {
            left: 100,
            top: 200,
            right: 900,
            bottom: 700,
        };
        let tall = stretched(frame, work);
        assert_eq!(
            (tall.left, tall.top, tall.right, tall.bottom),
            (100, 40, 900, 1080)
        );
        let top = half_of(work, true);
        assert_eq!(
            (top.top, top.bottom, top.left, top.right),
            (40, 560, 0, 1920)
        );
        let bottom = half_of(work, false);
        assert_eq!((bottom.top, bottom.bottom), (560, 1080));
    }
}
