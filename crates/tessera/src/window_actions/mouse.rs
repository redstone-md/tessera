// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! More of what the pointer does to windows, as KWin's window actions
//! have it: a double click and a turn of the wheel on a title bar, a turn
//! of the wheel and the middle button with the key of `modifier` held over
//! any window, and a click on a window that is not in front. The hook of
//! the pointer takes these only where an action is set, so every other
//! press and turn reaches the windows as it came.

use gpui::App;
use serde::Deserialize;

use crate::actions::Actions;
use crate::platform::{AppWindowId, AudioOutput, Display, HookAsks};
use crate::workspaces::{Toward, Workspaces};

/// How much a turn of the wheel changes how opaque a window is.
const OPACITY_STEP: f32 = 0.1;

/// The least a turn of the wheel makes a window opaque.
const LEAST_OPACITY: f32 = 0.1;

/// What a double click on a title bar does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TitleDouble {
    /// What Windows does: maximizes or restores the window.
    #[default]
    Windows,
    /// Maximizes the window, or restores a maximized one.
    Maximize,
    Minimize,
    Close,
    /// Puts the window below the others.
    Lower,
    /// Keeps the window above the others, or no longer.
    Above,
    /// Nothing, not even what Windows does.
    Nothing,
}

/// What a turn of the wheel over a title bar does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TitleWheel {
    /// Nothing: the window gets the turn.
    #[default]
    Nothing,
    /// Makes the window more opaque away from the user, less toward.
    Opacity,
    /// Away from the user keeps the window above the others or ends
    /// keeping it below; toward, the other way round.
    AboveBelow,
    /// Sends the window to the next workspace of Tessera's own, or the one
    /// before.
    Workspace,
    /// Away from the user maximizes the window; toward restores a
    /// maximized one and minimizes one that is not.
    MinimizeRestore,
}

/// What a turn of the wheel with the key of `modifier` held does over a
/// window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModifierWheel {
    #[default]
    Nothing,
    /// Makes the window under the pointer more or less opaque.
    Opacity,
    /// Goes to the next workspace, or the one before.
    Workspace,
    /// Turns the volume up or down.
    Volume,
}

/// What the middle button does with the key of `modifier` held over a
/// window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModifierMiddle {
    #[default]
    Nothing,
    Close,
    Minimize,
    /// Puts the window below the others.
    Lower,
    /// Brings the window in front and opens its window menu.
    WindowMenu,
}

/// What a click on a window that is not in front does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InactiveClick {
    /// What Windows does: brings the window in front, and the window gets
    /// the click.
    #[default]
    Windows,
    /// Brings the window in front; the window gets nothing of the click.
    ActivateOnly,
}

/// The mouse actions of `[window_actions]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseActions {
    pub titlebar_double: TitleDouble,
    pub titlebar_wheel: TitleWheel,
    pub modifier_wheel: ModifierWheel,
    pub modifier_middle: ModifierMiddle,
    pub inactive_click: InactiveClick,
}

impl MouseActions {
    /// What the hook of the pointer is to take for these; the actions of
    /// the key held need a key.
    pub fn asks(self, has_key: bool) -> HookAsks {
        HookAsks {
            title_double: self.titlebar_double != TitleDouble::Windows,
            title_wheel: self.titlebar_wheel != TitleWheel::Nothing,
            modifier_wheel: has_key && self.modifier_wheel != ModifierWheel::Nothing,
            modifier_middle: has_key && self.modifier_middle != ModifierMiddle::Nothing,
            inactive_click: self.inactive_click == InactiveClick::ActivateOnly,
        }
    }

    /// Whether an action of the key held is set, which needs `modifier`.
    pub fn need_key(self) -> bool {
        self.modifier_wheel != ModifierWheel::Nothing
            || self.modifier_middle != ModifierMiddle::Nothing
    }
}

/// How opaque a window drawn `now` is after a turn of the wheel, away
/// from the user where `up`.
pub fn stepped_opacity(now: f32, up: bool) -> f32 {
    let step = if up { OPACITY_STEP } else { -OPACITY_STEP };
    ((now + step) * 100.)
        .round()
        .clamp(LEAST_OPACITY * 100., 100.)
        / 100.
}

/// Makes `window` a step more or less opaque.
fn step_opacity(window: AppWindowId, up: bool) {
    window.set_opacity(stepped_opacity(window.opacity(), up));
}

/// Does what `double` says to `window`, whose title bar was clicked twice.
pub fn title_double(double: TitleDouble, window: AppWindowId) {
    match double {
        TitleDouble::Windows | TitleDouble::Nothing => {}
        TitleDouble::Maximize if window.is_maximized() => window.restore(),
        TitleDouble::Maximize => window.maximize(),
        TitleDouble::Minimize => window.minimize(),
        TitleDouble::Close => window.close(),
        TitleDouble::Lower => {
            window.lower();
        }
        TitleDouble::Above => {
            window.set_topmost(!window.is_topmost());
        }
    }
}

/// Does what `wheel` says to `window`, whose title bar the wheel turned
/// over, away from the user where `up`.
pub fn title_wheel(wheel: TitleWheel, window: AppWindowId, up: bool, cx: &mut App) {
    match wheel {
        TitleWheel::Nothing => {}
        TitleWheel::Opacity => step_opacity(window, up),
        TitleWheel::AboveBelow => match (up, window.is_kept_below(), window.is_topmost()) {
            (true, true, _) => window.keep_below(false),
            (true, false, false) => {
                window.set_topmost(true);
            }
            (false, _, true) => {
                window.set_topmost(false);
            }
            (false, false, false) => window.keep_below(true),
            _ => {}
        },
        TitleWheel::Workspace => send_to_next_workspace(window, up, cx),
        TitleWheel::MinimizeRestore => match (up, window.is_maximized()) {
            (true, false) => window.maximize(),
            (true, true) => {}
            (false, true) => window.restore(),
            (false, false) => window.minimize(),
        },
    }
}

/// Does what `wheel` says for a turn of the wheel with the key held over
/// `window`, away from the user where `up`.
pub fn modifier_wheel(wheel: ModifierWheel, window: AppWindowId, up: bool, cx: &mut App) {
    match wheel {
        ModifierWheel::Nothing => {}
        ModifierWheel::Opacity => step_opacity(window, up),
        ModifierWheel::Workspace => {
            let display = Display::of_window(window).map(|display| display.key);
            let toward = if up { Toward::Previous } else { Toward::Next };
            Workspaces::step(toward, display, cx);
        }
        ModifierWheel::Volume => {
            // The device is asked off the thread of the interface.
            cx.background_executor()
                .spawn(async move {
                    let stepped = AudioOutput::default_device().and_then(|output| output.step(up));
                    if let Err(error) = stepped {
                        report!("the volume is not changed: {error:#}");
                    }
                })
                .detach();
        }
    }
}

/// Does what `middle` says to `window`, pressed with the middle button
/// with the key held.
pub fn modifier_middle(middle: ModifierMiddle, window: AppWindowId, cx: &mut App) {
    match middle {
        ModifierMiddle::Nothing => {}
        ModifierMiddle::Close => window.close(),
        ModifierMiddle::Minimize => window.minimize(),
        ModifierMiddle::Lower => {
            window.lower();
        }
        ModifierMiddle::WindowMenu => {
            // The window menu opens for the window in front, by its name,
            // as a key would ask for it.
            window.activate();
            Actions::run("window-menu", None, cx);
        }
    }
}

/// Sends `window` to the workspace of Tessera's own after the one it is
/// on, or before it where `up`. The virtual desktops of Windows take no
/// window of another program, so there nothing is done.
fn send_to_next_workspace(window: AppWindowId, up: bool, cx: &mut App) {
    let Some(rows) = Workspaces::residents(None, cx) else {
        return;
    };
    let Some(on) = rows
        .iter()
        .position(|residents| residents.iter().any(|resident| resident.id == window))
    else {
        return;
    };
    let to = match up {
        true => on.checked_sub(1),
        false => Some(on + 1).filter(|&next| next < rows.len()),
    };
    if let Some(to) = to {
        Workspaces::move_window(window, to, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wheel_steps_the_opacity_within_its_bounds() {
        assert_eq!(stepped_opacity(1., true), 1.);
        assert_eq!(stepped_opacity(1., false), 0.9);
        assert_eq!(stepped_opacity(0.15, false), 0.1);
        assert_eq!(stepped_opacity(0.1, false), 0.1);
        assert_eq!(stepped_opacity(0.5, true), 0.6);
    }

    #[test]
    fn the_hook_takes_only_what_has_an_action() {
        let none = MouseActions::default();
        assert_eq!(none.asks(true), HookAsks::default());
        assert!(!none.need_key());
        let some = MouseActions {
            titlebar_double: TitleDouble::Nothing,
            modifier_wheel: ModifierWheel::Volume,
            inactive_click: InactiveClick::ActivateOnly,
            ..MouseActions::default()
        };
        let asks = some.asks(true);
        assert!(asks.title_double && asks.modifier_wheel && asks.inactive_click);
        assert!(!asks.title_wheel && !asks.modifier_middle);
        // Without a key the actions of the key held are not taken.
        assert!(!some.asks(false).modifier_wheel);
        assert!(some.need_key());
    }

    #[test]
    fn the_actions_are_read_by_their_words() {
        let read = |word: &str| toml::Value::String(word.to_owned());
        assert_eq!(
            read("above-below").try_into::<TitleWheel>().unwrap(),
            TitleWheel::AboveBelow
        );
        assert_eq!(
            read("minimize-restore").try_into::<TitleWheel>().unwrap(),
            TitleWheel::MinimizeRestore
        );
        assert_eq!(
            read("window-menu").try_into::<ModifierMiddle>().unwrap(),
            ModifierMiddle::WindowMenu
        );
        assert_eq!(
            read("activate-only").try_into::<InactiveClick>().unwrap(),
            InactiveClick::ActivateOnly
        );
        assert!(read("shade").try_into::<TitleDouble>().is_err());
    }
}
