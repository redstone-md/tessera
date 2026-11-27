// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The menu of Win+X, which Explorer draws in Windows: the tools of the
//! system, the settings, the file manager, the search, the Run dialog,
//! signing out and shutting down, and the desktop, each with the letter
//! Windows gives it, so that Win+X and then U and U still shut down.
//!
//! The widget `winx-menu` fills the popup `winx`, which the action `winx`
//! opens (Win+X where the keys of Windows are in use): at the widget
//! `winx` where a surface has one, else at the Start button (the button
//! that opens `launcher`), else at the left end of a surface. "Shut down
//! or sign out" leads to a second page of the menu, as its submenu does.

use std::env;
use std::path::PathBuf;

use gpui::{
    App, AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};

use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, WidgetSpec};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::platform::{Launch, PowerAction, PowerChoices};
use crate::popup::{PopupAnchor, Popups};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{Chooser, Pick, choice_row, icon, pill};

/// The name of the popup, of the action and of the button.
const NAME: &str = "winx";

/// The widget in the popup.
const MENU: &str = "winx-menu";

/// The popup the Start button opens, next to which the menu opens.
const START: &str = "launcher";

/// The height of a row, less than in other lists: the menu is long.
const ROW: f32 = 32.;

/// The size of the popup.
const WIDTH: f32 = 300.;
const HEIGHT: f32 = 660.;

const ICON_MENU: &str = "\u{e700}";
const ICON_BACK: &str = "\u{e72b}";
const ICON_MORE: &str = "\u{e76c}";

/// What an entry of the menu does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Deed {
    /// Opens what the Shell opens for this text.
    Open(&'static str),
    /// Opens Windows Terminal, or PowerShell where it is missing, as an
    /// administrator if `admin`.
    Terminal {
        admin: bool,
    },
    /// Opens the Mobility Center of Windows, which only some computers
    /// have.
    Mobility,
    /// Does the action of Tessera of that name.
    Action(&'static str),
    /// Opens the Run dialog: Tessera's where an action `run` exists,
    /// else the one of the Shell.
    Run,
    /// Goes to the page of signing out and shutting down.
    PowerPage,
    Power(PowerAction),
}

/// An entry of the menu: its name, its icon, the letter that takes it
/// and what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    label: &'static str,
    glyph: &'static str,
    letter: char,
    deed: Deed,
}

const fn entry(label: &'static str, glyph: &'static str, letter: char, deed: Deed) -> Entry {
    Entry {
        label,
        glyph,
        letter,
        deed,
    }
}

/// The groups of the first page, as Windows 11 orders them.
const GROUPS: [&[Entry]; 3] = [
    &[
        entry(
            "Installed apps",
            "\u{e71d}",
            'p',
            Deed::Open("ms-settings:appsfeatures"),
        ),
        entry("Mobility Center", "\u{e8ea}", 'b', Deed::Mobility),
        entry(
            "Power Options",
            "\u{e945}",
            'o',
            Deed::Open("ms-settings:powersleep"),
        ),
        entry("Event Viewer", "\u{e7ba}", 'v', Deed::Open("eventvwr.msc")),
        entry("System", "\u{e770}", 'y', Deed::Open("ms-settings:about")),
        entry("Device Manager", "\u{e772}", 'm', Deed::Open("devmgmt.msc")),
        entry(
            "Network Connections",
            "\u{e839}",
            'w',
            Deed::Open("ms-settings:network"),
        ),
        entry(
            "Disk Management",
            "\u{eda2}",
            'k',
            Deed::Open("diskmgmt.msc"),
        ),
        entry(
            "Computer Management",
            "\u{e912}",
            'g',
            Deed::Open("compmgmt.msc"),
        ),
        entry("Terminal", "\u{e756}", 'i', Deed::Terminal { admin: false }),
        entry(
            "Terminal (Admin)",
            "\u{e7ef}",
            'a',
            Deed::Terminal { admin: true },
        ),
    ],
    &[
        entry("Task Manager", "\u{e9d9}", 't', Deed::Open("taskmgr.exe")),
        entry("Settings", "\u{e713}", 'n', Deed::Open("ms-settings:")),
        entry("File Explorer", "\u{ec50}", 'e', Deed::Action("files")),
        entry("Search", "\u{e721}", 's', Deed::Action("search")),
        entry("Run", "\u{e7c4}", 'r', Deed::Run),
    ],
    &[
        entry("Shut down or sign out", "\u{e7e8}", 'u', Deed::PowerPage),
        entry("Desktop", "\u{e7f4}", 'd', Deed::Action("desktop")),
    ],
];

/// The second page, in the order of the submenu of Windows.
const POWER: [Entry; 7] = [
    entry(
        "Sign out",
        "\u{f3b1}",
        'i',
        Deed::Power(PowerAction::SignOut),
    ),
    entry("Sleep", "\u{e708}", 's', Deed::Power(PowerAction::Sleep)),
    entry(
        "Hibernate",
        "\u{e74e}",
        'h',
        Deed::Power(PowerAction::Hibernate),
    ),
    entry(
        "Update and shut down",
        "\u{e895}",
        'p',
        Deed::Power(PowerAction::UpdateShutDown),
    ),
    entry(
        "Shut down",
        "\u{e7e8}",
        'u',
        Deed::Power(PowerAction::ShutDown),
    ),
    entry(
        "Update and restart",
        "\u{e895}",
        'e',
        Deed::Power(PowerAction::UpdateRestart),
    ),
    entry(
        "Restart",
        "\u{e777}",
        'r',
        Deed::Power(PowerAction::Restart),
    ),
];

pub struct WinxModule;

impl Module for WinxModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register(NAME, |_, cx| cx.new(|_| WinxButton).into())?;
        widgets.open_with(NAME, NAME);
        widgets.register_panel(MENU, |_, cx| cx.new(WinxMenu::new).into())?;
        widgets.register_popup(PopupConfig {
            name: NAME.to_owned(),
            place: Default::default(),
            width: WIDTH,
            height: HEIGHT,
            content: vec![WidgetSpec {
                widget: MENU.to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        // Explorer keeps Win+X to itself while it runs.
        let open = Action::new(|_, cx| open_menu(cx))
            .taking_reserved_keys()
            .by_default("win+x", None);
        actions.register(NAME, open)
    }
}

/// Opens the menu, or closes it when it is open.
fn open_menu(cx: &mut App) {
    let at_widget = |name: &str, cx: &mut App| match Shell::popup_anchor(name, cx) {
        Some((true, anchor)) => Ok(anchor),
        other => Err(other.map(|(_, anchor)| anchor)),
    };
    let anchor = at_widget(NAME, cx)
        .or_else(|_| at_widget(START, cx))
        .map_or_else(|anywhere| anywhere.map(at_left), Some);
    match anchor {
        Some(anchor) => Popups::toggle(NAME, anchor, cx),
        None => report!("the menu of Win+X has nowhere to open"),
    }
}

/// `anchor` moved to the left end of its surface; the popup is kept on
/// the display, so it stands in the corner, as the menu of Windows 10 did.
fn at_left(mut anchor: PopupAnchor) -> PopupAnchor {
    anchor.x = anchor.surface.left;
    anchor
}

/// What the menu learns of the computer when it opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Found {
    /// Whether Windows has its Mobility Center, as only some laptops do.
    mobility: bool,
    /// Whether Windows Terminal is installed.
    terminal: bool,
    power: PowerChoices,
}

/// The folder of the programs of Windows.
fn system_folder() -> Option<PathBuf> {
    Some(PathBuf::from(env::var_os("SystemRoot")?).join("System32"))
}

/// Where the alias of Windows Terminal lies when it is installed.
fn terminal_alias() -> Option<PathBuf> {
    let local = PathBuf::from(env::var_os("LOCALAPPDATA")?);
    Some(local.join(r"Microsoft\WindowsApps\wt.exe"))
}

impl Found {
    fn read() -> Self {
        // The alias of an application is a link that only Windows can
        // follow, so it is looked at itself.
        let present =
            |path: Option<PathBuf>| path.is_some_and(|path| path.symlink_metadata().is_ok());
        Self {
            mobility: present(system_folder().map(|folder| folder.join("mblctr.exe"))),
            terminal: present(terminal_alias()),
            power: PowerChoices::read(),
        }
    }

    /// Whether the menu shows `entry`.
    fn shows(self, entry: &Entry) -> bool {
        match entry.deed {
            Deed::Mobility => self.mobility,
            Deed::Power(action) => self.power.offers(action),
            _ => true,
        }
    }

    /// The name of `entry`: PowerShell stands for the terminal where
    /// Windows Terminal is missing, as in Windows 10.
    fn label(self, entry: &Entry) -> &'static str {
        match entry.deed {
            Deed::Terminal { admin: false } if !self.terminal => "Windows PowerShell",
            Deed::Terminal { admin: true } if !self.terminal => "Windows PowerShell (Admin)",
            _ => entry.label,
        }
    }
}

impl Deed {
    /// Does it; the menu has closed before.
    fn run(self, found: Found, cx: &mut App) {
        match self {
            Self::Open(target) => Launch::new(target).start(),
            Self::Terminal { admin } => {
                let program = if found.terminal {
                    "wt.exe"
                } else {
                    "powershell.exe"
                };
                let launch = Launch::new(program);
                match admin {
                    true => launch.verb("runas").start(),
                    false => launch.start(),
                }
            }
            Self::Mobility => {
                if let Some(folder) = system_folder() {
                    Launch::new(folder.join("mblctr.exe")).start();
                }
            }
            Self::Action(name) => Actions::run(name, None, cx),
            Self::Run => {
                let own = cx
                    .try_global::<Actions>()
                    .is_some_and(|actions| actions.get("run").is_some());
                match own {
                    true => Actions::run("run", None, cx),
                    // The Run dialog of the Shell, which needs no Explorer.
                    false => Launch::new("rundll32.exe")
                        .arguments("shell32.dll,#61")
                        .named("The Run dialog")
                        .start(),
                }
            }
            Self::Power(action) => action.perform(),
            Self::PowerPage => {}
        }
    }
}

/// A row of the menu: an entry, and whether a line stands above it to
/// part its group from the one before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Row {
    entry: Entry,
    parted: bool,
}

/// The rows of the first page, or of the second with `power`, as far as
/// `found` shows them.
fn rows(found: Found, power: bool) -> Vec<Row> {
    let mut rows = Vec::new();
    if power {
        let back = entry("Shut down or sign out", ICON_BACK, 'u', Deed::PowerPage);
        rows.push(Row {
            entry: back,
            parted: false,
        });
        let shown = POWER.iter().filter(|entry| found.shows(entry));
        rows.extend(shown.map(|&entry| Row {
            entry,
            parted: false,
        }));
        return rows;
    }
    for (index, group) in GROUPS.iter().enumerate() {
        let shown = group.iter().filter(|entry| found.shows(entry));
        for (place, &entry) in shown.enumerate() {
            rows.push(Row {
                entry,
                parted: index > 0 && place == 0,
            });
        }
    }
    rows
}

/// The row of `rows` the letter `letter` takes.
fn lettered(rows: &[Row], letter: char) -> Option<usize> {
    rows.iter().position(|row| row.entry.letter == letter)
}

/// The menu itself.
struct WinxMenu {
    found: Found,
    /// Whether the page of signing out and shutting down is shown.
    power: bool,
    chooser: Chooser,
}

impl WinxMenu {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let found = cx.background_executor().spawn(async { Found::read() });
            let found = found.await;
            this.update(cx, |this, cx| {
                this.found = found;
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            // Until the computer has been asked, Windows Terminal is taken
            // to be there, as it is in Windows 11, and the actions that
            // need asking are left out.
            found: Found {
                terminal: true,
                ..Found::default()
            },
            power: false,
            chooser: Chooser::new(cx),
        }
    }

    fn rows(&self) -> Vec<Row> {
        rows(self.found, self.power)
    }

    /// Goes to the second page, or back to the first.
    fn turn(&mut self, power: bool) {
        self.power = power;
        // On the second page the first action is chosen, back on the first
        // page the entry that leads there.
        let rows = self.rows();
        self.chooser.chosen = match power {
            true => 1.min(rows.len().saturating_sub(1)),
            false => { rows.iter() }
                .position(|row| row.entry.deed == Deed::PowerPage)
                .unwrap_or(0),
        };
    }

    /// Takes the row `index`: a page is turned, anything else is done
    /// once the menu has closed.
    fn take(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows().get(index).copied() else {
            return;
        };
        if row.entry.deed == Deed::PowerPage {
            self.turn(!self.power);
            cx.notify();
            return;
        }
        let found = self.found;
        cx.defer(move |cx| {
            Popups::close(cx);
            row.entry.deed.run(found, cx);
        });
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let chosen = self.rows().get(self.chooser.chosen).copied();
        let on_power_entry = chosen.is_some_and(|row| row.entry.deed == Deed::PowerPage);
        match (key, self.power) {
            ("right", false) if on_power_entry => return self.take(self.chooser.chosen, cx),
            ("left", true) => {
                self.turn(false);
                return cx.notify();
            }
            _ => {}
        }
        match Pick::of(event) {
            Some(Pick::Take) => self.take(self.chooser.chosen, cx),
            Some(Pick::Leave) if self.power => self.turn(false),
            Some(Pick::Leave) => cx.defer(Popups::close),
            Some(Pick::Letter(letter)) => {
                // The letter of the page back is that of the entry it
                // came from; on the second page the actions come first.
                let rows = self.rows();
                let skip = usize::from(self.power);
                let found =
                    lettered(&rows[skip.min(rows.len())..], letter).map(|index| index + skip);
                if let Some(index) = found {
                    self.take(index, cx);
                }
            }
            Some(pick) => self.chooser.pick(pick, self.rows().len()),
            None => return,
        }
        cx.notify();
    }
}

impl Render for WinxMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        self.chooser.take_focus(window);
        let rows = self.rows();
        let page = usize::from(self.power) * 100;
        let mut elements = Vec::with_capacity(rows.len() * 2);
        for (index, row) in rows.iter().enumerate() {
            if row.parted {
                elements.push(
                    div()
                        .flex_none()
                        .mx(theme.spacing.small)
                        .my(px(3.))
                        .h(px(1.))
                        .bg(theme.colors.outline.opacity(0.4))
                        .into_any_element(),
                );
            }
            let entry = row.entry;
            let state = self.chooser.state(index, &theme, window);
            let chosen = index == self.chooser.chosen;
            let label = language.say(self.found.label(&entry));
            let leads_on = entry.deed == Deed::PowerPage && !self.power;
            // The row that leads back from the second page is its title,
            // and shows no letter: U there shuts down.
            let title = self.power && index == 0;
            let letter = (!title).then_some(entry.letter);
            let element = choice_row(&theme, entry.glyph, label, letter, chosen, state);
            let element = match title {
                true => element.font_weight(FontWeight::MEDIUM),
                false => element,
            };
            let element = element
                .h(px(ROW))
                .children(leads_on.then(|| {
                    icon(&theme, ICON_MORE)
                        .flex_none()
                        .text_size(theme.typography.label_size)
                        .text_color(theme.colors.on_surface_variant)
                }))
                .id(("winx-entry", page + index))
                .on_click(cx.listener(move |this, _, _, cx| this.take(index, cx)));
            let element = Chooser::track(element, index, cx, |this: &mut Self| &mut this.chooser);
            elements.push(element.into_any_element());
        }
        div()
            .id(MENU)
            .track_focus(self.chooser.focus())
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(elements)
    }
}

/// A button for a bar that opens the menu, as a right click on the Start
/// button of Windows does.
struct WinxButton;

impl Render for WinxButton {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        pill(&theme).child(icon(&theme, ICON_MENU))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn everything() -> Found {
        Found {
            mobility: true,
            terminal: true,
            power: PowerChoices {
                sleep: true,
                hibernate: true,
                updates: true,
            },
        }
    }

    #[test]
    fn the_letters_of_a_page_are_its_own() {
        for power in [false, true] {
            let rows = rows(everything(), power);
            let letters = rows
                .iter()
                .skip(usize::from(power))
                .map(|row| row.entry.letter);
            let mut letters: Vec<char> = letters.collect();
            let count = letters.len();
            letters.sort_unstable();
            letters.dedup();
            assert_eq!(letters.len(), count, "a letter stands twice on a page");
        }
    }

    #[test]
    fn u_and_u_shut_down_as_in_windows() {
        let first = rows(Found::default(), false);
        let page = lettered(&first, 'u').unwrap();
        assert_eq!(first[page].entry.deed, Deed::PowerPage);
        let second = rows(Found::default(), true);
        let shut = lettered(&second[1..], 'u').unwrap() + 1;
        assert_eq!(second[shut].entry.deed, Deed::Power(PowerAction::ShutDown));
    }

    #[test]
    fn what_the_computer_lacks_is_left_out() {
        let plain = Found::default();
        let first = rows(plain, false);
        assert!(first.iter().all(|row| row.entry.deed != Deed::Mobility));
        let second: Vec<Deed> = rows(plain, true).iter().map(|row| row.entry.deed).collect();
        assert_eq!(
            second,
            [
                Deed::PowerPage,
                Deed::Power(PowerAction::SignOut),
                Deed::Power(PowerAction::ShutDown),
                Deed::Power(PowerAction::Restart)
            ]
        );
        assert_eq!(plain.label(&GROUPS[0][9]), "Windows PowerShell");
        assert_eq!(everything().label(&GROUPS[0][9]), "Terminal");
    }

    #[test]
    fn a_line_parts_the_groups() {
        let first = rows(everything(), false);
        let parted = first.iter().filter(|row| row.parted).count();
        assert_eq!(parted, GROUPS.len() - 1);
        assert!(!first[0].parted);
    }

    #[test]
    fn the_action_brings_win_x() {
        let mut actions = Actions::default();
        WinxModule.actions(&mut actions).unwrap();
        assert!(actions.takes_reserved_keys(NAME));
        assert_eq!(actions.default_keys()[0].press, "win+x");
    }
}
