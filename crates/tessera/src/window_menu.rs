// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The window menu, as KWin has one: for the window in front, the action
//! `window-menu` (Alt+F3 where the keys of Windows are in use) opens a
//! menu under its title bar that keeps it above or below the others or no
//! longer, makes it more or less see-through, sends it to another workspace
//! of Tessera's own or to the next display, snaps it to the left or right
//! half, maximizes or restores it, has it fill its display, takes its title
//! bar away, moves or resizes it with the arrow keys, minimizes or closes
//! it, ends its program once that is confirmed, and writes a `[[rule]]`
//! for windows like it into the configuration.

use gpui::App;

use crate::actions::{Action, Actions};
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::platform::{AppWindow, AppWindowId, Display, Launch};
use crate::rules::write_rule_for;
use crate::shell::Shell;
use crate::snap::{Snapper, anchor_at};
use crate::tiling::Tiler;
use crate::window_actions::ask_to_kill;
use crate::workspaces::Workspaces;

const ICON_ABOVE: &str = "\u{e718}";
const ICON_NOT_ABOVE: &str = "\u{e77a}";
const ICON_BELOW: &str = "\u{e70d}";
const ICON_NOT_BELOW: &str = "\u{e70e}";
const ICON_FULL_SCREEN: &str = "\u{e740}";
const ICON_WINDOWED: &str = "\u{e73f}";
const ICON_TITLE_BAR: &str = "\u{e737}";
const ICON_MOVE: &str = "\u{e7c2}";
const ICON_RESIZE: &str = "\u{e744}";
const ICON_KILL: &str = "\u{e7ba}";
const ICON_OPACITY: &str = "\u{e790}";
const ICON_WORKSPACE: &str = "\u{e8a9}";
const ICON_DISPLAY: &str = "\u{e7f4}";
const ICON_LEFT: &str = "\u{e90c}";
const ICON_RIGHT: &str = "\u{e90d}";
const ICON_MAXIMIZE: &str = "\u{e922}";
const ICON_RESTORE: &str = "\u{e923}";
const ICON_MINIMIZE: &str = "\u{e921}";
const ICON_CLOSE: &str = "\u{e8bb}";
const ICON_RULE: &str = "\u{e713}";

/// How opaque the menu can make a window, in percent.
const OPACITIES: [u32; 4] = [100, 85, 70, 50];

/// How far below the top of the window the menu hangs, in logical pixels:
/// under a title bar of the usual height.
const BELOW: f32 = 32.;

/// Registers the action `window-menu`, and `kill-window` beside it.
pub fn register(actions: &mut Actions) -> anyhow::Result<()> {
    let menu = Action::new(|_, cx| open_for_front(cx)).by_default("alt+f3", None);
    actions.register("window-menu", menu)?;
    crate::window_actions::register(actions)
}

/// Opens the window menu for the window in front, unless it is one of
/// Tessera.
fn open_for_front(cx: &mut App) {
    let front = AppWindowId::foreground().filter(|id| id.process() != std::process::id());
    if let Some(id) = front {
        open(id, cx);
    }
}

/// Which of the opacities of the menu `opacity` is, as near as can be.
fn nearest_opacity(opacity: f32) -> u32 {
    let percent = (opacity * 100.).round() as i64;
    OPACITIES
        .into_iter()
        .min_by_key(|&choice| (i64::from(choice) - percent).abs())
        .unwrap_or(100)
}

/// Opens the window menu for the window `id`, under its title bar.
pub fn open(id: AppWindowId, cx: &mut App) {
    let Some(display) = Display::of_window(id) else {
        return;
    };
    let say = |text: &'static str| Language::of(cx).say(text);
    let mut entries = Vec::new();
    let above = id.is_topmost();
    let (glyph, label) = match above {
        false => (ICON_ABOVE, say("Keep above other windows")),
        true => (ICON_NOT_ABOVE, say("Stop keeping above other windows")),
    };
    entries.push(MenuEntry::new(glyph, label, move |_| {
        id.set_topmost(!above);
    }));
    let below = id.is_kept_below();
    let (glyph, label) = match below {
        false => (ICON_BELOW, say("Keep below other windows")),
        true => (ICON_NOT_BELOW, say("Stop keeping below other windows")),
    };
    entries.push(MenuEntry::new(glyph, label, move |_| id.keep_below(!below)));
    let current = nearest_opacity(id.opacity());
    for opacity in OPACITIES.into_iter().filter(|&opacity| opacity != current) {
        let label = format!("{} {opacity} %", say("Window opacity"));
        entries.push(MenuEntry::new(ICON_OPACITY, label, move |_| {
            id.set_opacity(opacity as f32 / 100.);
        }));
    }
    entries.extend(workspace_entries(id, cx));
    if Display::all().len() > 1 {
        let label = say("Move to the next display");
        entries.push(MenuEntry::new(ICON_DISPLAY, label, move |cx| {
            Snapper::move_to_display(id, false, cx);
        }));
    }
    let snap = |glyph, label, arg: &'static str| {
        MenuEntry::new(glyph, label, move |cx| Snapper::snap_window(id, arg, cx))
    };
    entries.push(snap(ICON_LEFT, say("Snap to the left half"), "left-half"));
    entries.push(snap(
        ICON_RIGHT,
        say("Snap to the right half"),
        "right-half",
    ));
    entries.push(match id.is_maximized() {
        true => snap(
            ICON_RESTORE,
            say("Restore the size of the window"),
            "restore",
        ),
        false => snap(ICON_MAXIMIZE, say("Maximize the window"), "maximize"),
    });
    entries.extend(shape_entries(id, display, cx));
    entries.push(MenuEntry::new(
        ICON_MINIMIZE,
        say("Minimize the window"),
        move |_| {
            id.minimize();
        },
    ));
    entries.push(MenuEntry::new(
        ICON_CLOSE,
        say("Close the window"),
        move |_| {
            id.close();
        },
    ));
    entries.push(MenuEntry::new(ICON_KILL, say("Kill process"), move |cx| {
        // The menu closes first, so that the question takes the keyboard.
        cx.defer(move |cx| ask_to_kill(id, cx));
    }));
    let label = say("Create a rule for this window");
    entries.push(MenuEntry::new(ICON_RULE, label, move |cx| {
        create_rule(id, cx)
    }));
    let point = match id.frame() {
        Some(frame) => (
            (frame.left + frame.right) / 2,
            frame.top.max(display.work.top) + (BELOW * display.scale) as i32,
        ),
        None => crate::platform::pointer_at().unwrap_or((display.work.left, display.work.top)),
    };
    let anchor = anchor_at(&display, point, cx);
    Menus::open(entries, anchor, cx);
}

/// The entries that change the shape of the window `id` on `display`:
/// fill the display or no longer, take the title bar away or give it back,
/// and move or resize it with the arrow keys. A window the tiling places
/// is not made to fill its display, as the tiling would put it back in its
/// tile.
fn shape_entries(id: AppWindowId, display: Display, cx: &App) -> Vec<MenuEntry> {
    let say = |text: &'static str| Language::of(cx).say(text);
    let filled = id.fills_display();
    let (glyph, label) = match filled {
        false => (ICON_FULL_SCREEN, say("Fill the display")),
        true => (ICON_WINDOWED, say("Stop filling the display")),
    };
    let fill = MenuEntry::new(glyph, label, move |_| {
        id.fill_display((!filled).then_some(display.frame));
    });
    let taken = id.title_bar_taken();
    let label = match taken {
        false => say("Hide the title bar"),
        true => say("Show the title bar"),
    };
    let title_bar = MenuEntry::new(ICON_TITLE_BAR, label, move |_| {
        id.show_title_bar(taken);
    });
    // Windows moves and resizes only the window in front with the keys.
    let moving = MenuEntry::new(ICON_MOVE, say("Move with the keyboard"), move |cx| {
        cx.defer(move |_| {
            id.activate();
            id.move_with_keys();
        });
    });
    let resizing = MenuEntry::new(ICON_RESIZE, say("Resize with the keyboard"), move |cx| {
        cx.defer(move |_| {
            id.activate();
            id.resize_with_keys();
        });
    });
    let fill = (filled || !Tiler::manages(id, cx)).then_some(fill);
    fill.into_iter()
        .chain([title_bar, moving, resizing])
        .collect()
}

/// An entry for each workspace of Tessera's own other than the one of
/// the window `id`, which sends it there; none where the workspaces are
/// the virtual desktops of Windows, which take no window of another
/// program.
fn workspace_entries(id: AppWindowId, cx: &App) -> Vec<MenuEntry> {
    let display = { AppWindow::enumerate().into_iter() }
        .find(|window| window.id == id)
        .map(|window| window.display);
    let Some(rows) = Workspaces::residents(display, cx) else {
        return Vec::new();
    };
    let home = rows
        .iter()
        .position(|row| row.iter().any(|resident| resident.id == id));
    let say = Language::of(cx).say("Move to workspace");
    (0..rows.len())
        .filter(|&index| Some(index) != home)
        .map(|index| {
            let label = format!("{say} {}", Workspaces::label(index, cx));
            MenuEntry::new(ICON_WORKSPACE, label, move |cx| {
                Workspaces::move_window(id, index, cx);
            })
        })
        .collect()
}

/// Writes a rule for windows like `id` and opens the configuration file
/// to show it.
fn create_rule(id: AppWindowId, cx: &mut App) {
    match write_rule_for(id) {
        Ok(path) => {
            Shell::config_changed(cx);
            Launch::new(&path).start();
        }
        Err(error) => report!("no rule was written for the window: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_opacity_is_one_of_the_menu() {
        assert_eq!(nearest_opacity(1.), 100);
        assert_eq!(nearest_opacity(0.9), 85);
        assert_eq!(nearest_opacity(0.71), 70);
        assert_eq!(nearest_opacity(0.2), 50);
    }
}
