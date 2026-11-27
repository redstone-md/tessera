// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The menu of an application, a file or a recommended entry of the
//! Start menu and the launcher, as the Start menu of Windows, Kickoff and
//! the applications menu of Seelen have one: open, run as an
//! administrator, open the file location, pin to Start or the dock, put
//! on the desktop, open the page of the application in the Settings app,
//! uninstall, show the properties, and take a recommended entry away.
//!
//! It is drawn inside the view that opens it, in front of the rest, since
//! a menu of a window of its own would take the keyboard from the popup
//! and close it. Its owner shows it with [`AppMenu::render_at`] and drops
//! it on [`DismissEvent`]. The arrows move the choice, Enter takes it and
//! Escape leaves; uninstalling asks for a second Enter or click.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Context, DismissEvent, Entity, EventEmitter,
    FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Point,
    Render, StatefulInteractiveElement, Styled, WeakEntity, Window, anchored, deferred, div, px,
};

use super::Pins;
use crate::banners::{Banners, Notice};
use crate::config::edit::ConfigFile;
use crate::files::Files;
use crate::i18n::Language;
use crate::platform::{
    Launch, Removal, ShellThread, Shortcut, app_id_in, app_settings, put_on_desktop, reveal,
};
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{Chooser, Pick, choice_row};

const ICON_OPEN: &str = "\u{e8a7}";
const ICON_ADMIN: &str = "\u{e7ef}";
const ICON_FOLDER: &str = "\u{e838}";
const ICON_PIN: &str = "\u{e718}";
const ICON_UNPIN: &str = "\u{e77a}";
const ICON_DOCK: &str = "\u{e75b}";
const ICON_DESKTOP: &str = "\u{e7f4}";
const ICON_SETTINGS: &str = "\u{e713}";
const ICON_UNINSTALL: &str = "\u{e74d}";
const ICON_PROPERTIES: &str = "\u{e946}";
const ICON_REMOVE: &str = "\u{e711}";

/// How wide the menu is.
const WIDTH: Pixels = px(248.);

/// How far the menu keeps from the edges of the window.
const MARGIN: Pixels = px(8.);

/// The extensions of the files that run as programs, and so may run as
/// an administrator.
const PROGRAMS: [&str; 6] = ["exe", "bat", "cmd", "msi", "lnk", "com"];

/// What takes an entry out of the list it stands in.
pub type Remover = Rc<dyn Fn(&mut App)>;

/// What the menu is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// An application of the Start menu.
    App,
    /// A file or a folder, as one found by the search.
    File,
}

/// Something the menu does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deed {
    Open,
    RunAsAdministrator,
    OpenFileLocation,
    PinToStart,
    UnpinFromStart,
    PinToDock,
    UnpinFromDock,
    AddToDesktop,
    AppSettings,
    Uninstall,
    Properties,
    /// Takes the entry out of the recommended ones.
    Remove,
}

impl Deed {
    fn look(self) -> (&'static str, &'static str) {
        match self {
            Self::Open => (ICON_OPEN, "Open"),
            Self::RunAsAdministrator => (ICON_ADMIN, "Run as administrator"),
            Self::OpenFileLocation => (ICON_FOLDER, "Open file location"),
            Self::PinToStart => (ICON_PIN, "Pin to Start"),
            Self::UnpinFromStart => (ICON_UNPIN, "Unpin from Start"),
            Self::PinToDock => (ICON_DOCK, "Pin to the dock"),
            Self::UnpinFromDock => (ICON_UNPIN, "Unpin from the dock"),
            Self::AddToDesktop => (ICON_DESKTOP, "Add to desktop"),
            Self::AppSettings => (ICON_SETTINGS, "App settings"),
            Self::Uninstall => (ICON_UNINSTALL, "Uninstall"),
            Self::Properties => (ICON_PROPERTIES, "Properties"),
            Self::Remove => (ICON_REMOVE, "Remove from list"),
        }
    }
}

/// What is known of the item when the menu is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Standing {
    /// Whether the item is a packaged application.
    pub packaged: bool,
    /// Whether it is pinned to Start, where something shows the pins.
    pub on_start: Option<bool>,
    /// Whether it is pinned to the dock, once that is read and where the
    /// layout has a dock.
    pub on_dock: Option<bool>,
    /// Whether it can be taken out of a list it stands in.
    pub removable: bool,
    /// Whether it is a program, which may run as an administrator.
    pub program: bool,
}

/// What the menu offers for an item of `kind` that stands as `standing`
/// says, in the order of the Start menu of Windows.
pub fn deeds(kind: ItemKind, standing: Standing) -> Vec<Deed> {
    let mut deeds = vec![Deed::Open];
    let app = kind == ItemKind::App;
    if standing.program {
        deeds.push(Deed::RunAsAdministrator);
    }
    // A packaged application lies in no folder.
    if !standing.packaged {
        deeds.push(Deed::OpenFileLocation);
    }
    if app {
        match standing.on_start {
            Some(true) => deeds.push(Deed::UnpinFromStart),
            Some(false) => deeds.push(Deed::PinToStart),
            None => {}
        }
        match standing.on_dock {
            Some(true) => deeds.push(Deed::UnpinFromDock),
            Some(false) => deeds.push(Deed::PinToDock),
            None => {}
        }
        deeds.push(Deed::AddToDesktop);
        if standing.packaged {
            deeds.push(Deed::AppSettings);
        }
        deeds.push(Deed::Uninstall);
    }
    if !standing.packaged {
        deeds.push(Deed::Properties);
    }
    if standing.removable {
        deeds.push(Deed::Remove);
    }
    deeds
}

/// Whether one of `pins` of the dock stands for `shortcut`: its name, as
/// the Start menu shows it, its path, or the path of `target`, the
/// program it starts, which the dock pins a window by.
fn docked(pins: &[String], shortcut: &Shortcut, target: Option<&Path>) -> bool {
    let slashed = |path: &Path| path.to_string_lossy().replace('\\', "/");
    let paths: Vec<String> = { [Some(shortcut.path.as_path()), target].into_iter() }
        .flatten()
        .map(slashed)
        .collect();
    { pins.iter() }.any(|pin| {
        let pin_path = pin.replace('\\', "/");
        pin.eq_ignore_ascii_case(&shortcut.name) || { paths.iter() }
            .any(|path| pin_path.eq_ignore_ascii_case(path))
    })
}

/// `pins` with `shortcut` pinned by its name, or unpinned in every way it
/// is pinned, `target` being the program it starts.
fn repinned(pins: &[String], shortcut: &Shortcut, target: Option<&Path>, pin: bool) -> Vec<String> {
    let mut pins: Vec<String> = { pins.iter() }
        .filter(|pin| !docked(std::slice::from_ref(pin), shortcut, target))
        .cloned()
        .collect();
    if pin {
        pins.push(shortcut.name.clone());
    }
    pins
}

/// The applications pinned to the dock, as the configuration file has
/// them; `None` where the layout has no dock or the file cannot be read.
fn dock_pins() -> Option<Vec<String>> {
    ConfigFile::open().ok()?.config().ok()?.pinned()
}

/// Pins `shortcut`, which starts `target`, to the dock or unpins it, in
/// the configuration file, on a thread of the background.
fn set_on_dock(shortcut: Shortcut, target: Option<PathBuf>, pin: bool, cx: &mut App) {
    let changed = cx.background_executor().spawn(async move {
        ConfigFile::change_pinned(|pins| repinned(pins, &shortcut, target.as_deref(), pin))
    });
    cx.spawn(async move |cx: &mut AsyncApp| match changed.await {
        Ok(()) => {
            cx.update(Shell::config_changed).ok();
        }
        Err(error) => report!("{error}"),
    })
    .detach();
}

/// Opens the folder `path` lies in, with it chosen where Explorer opens
/// it.
fn open_location(path: &Path, cx: &mut App) {
    let path = path.to_owned();
    cx.background_executor()
        .spawn(async move {
            if let Err(error) = reveal(&path) {
                report!("{error:#}");
            }
        })
        .detach();
}

/// Tells in a notice how something done for `name` went: `done` over its
/// name where it went well, `failed` and why otherwise.
fn tell(name: &str, done: Result<&'static str, (&'static str, String)>, cx: &mut App) {
    let language = Language::of(cx);
    let (title, body) = match done {
        Ok(done) => (language.say(done).to_owned(), name.to_owned()),
        Err((failed, why)) => (language.say(failed).to_owned(), format!("{name}: {why}")),
    };
    let notice = Notice {
        app: language.say("Start menu").to_owned(),
        title,
        body,
        glyph: Some(ICON_OPEN),
        ..Notice::default()
    };
    Banners::post(notice, cx);
}

/// The menu of one item.
pub struct AppMenu {
    shortcut: Shortcut,
    kind: ItemKind,
    standing: Standing,
    /// The program the shortcut starts, once it is read.
    target: Option<PathBuf>,
    pins: Entity<Pins>,
    /// What takes the entry out of the list it stands in, where it can be.
    remove: Option<Remover>,
    chooser: Chooser,
    /// What had the keys before the menu took them, which gets them back
    /// once it goes; `None` until the menu is drawn.
    before: Option<Option<FocusHandle>>,
    /// Whether uninstalling waits for its confirmation.
    armed: bool,
}

impl EventEmitter<DismissEvent> for AppMenu {}

impl AppMenu {
    /// The menu of `shortcut`, an item of `kind`; `remove` takes it out of
    /// the list it stands in, where it can be.
    pub fn new(
        shortcut: Shortcut,
        kind: ItemKind,
        remove: Option<Remover>,
        cx: &mut Context<Self>,
    ) -> Self {
        let pins = Pins::shared(cx);
        let read = pins.read(cx);
        let packaged = app_id_in(&shortcut.path).is_some();
        let extension = shortcut
            .path
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase());
        let runs = extension.is_some_and(|ext| PROGRAMS.contains(&ext.as_str()));
        let standing = Standing {
            packaged,
            on_start: (read.is_shown() && kind == ItemKind::App)
                .then(|| read.holds(&shortcut.name)),
            on_dock: None,
            removable: remove.is_some(),
            program: !packaged && (kind == ItemKind::App || runs),
        };
        if kind == ItemKind::App {
            // Whether it is on the dock is in the configuration file, by
            // the shortcut or by the program it starts, which the Shell
            // reads from it.
            let link = ShellThread::call({
                let shortcut = shortcut.clone();
                move || shortcut.link().target
            });
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let pins = cx.background_executor().spawn(async { dock_pins() });
                let target = link.recv().await.ok().flatten();
                let pins = pins.await;
                this.update(cx, |this, cx| {
                    let docked =
                        |pins: Vec<String>| docked(&pins, &this.shortcut, target.as_deref());
                    this.standing.on_dock = pins.map(docked);
                    this.target = target;
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        Self {
            shortcut,
            kind,
            standing,
            target: None,
            pins,
            remove,
            chooser: Chooser::new(cx),
            before: None,
            armed: false,
        }
    }

    fn deeds(&self) -> Vec<Deed> {
        deeds(self.kind, self.standing)
    }

    /// Closes the menu and gives the keys back to what had them before,
    /// such as the search field of the launcher, unless something else
    /// has taken them since.
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Some(before)) = self.before.take()
            && (self.chooser.focus().is_focused(window) || window.focused(cx).is_none())
        {
            window.focus(&before);
        }
        cx.emit(DismissEvent);
    }

    /// Does `deed`, and closes the menu, or the popup where the deed
    /// leaves it; uninstalling waits for a second time.
    fn take(&mut self, deed: Deed, window: &mut Window, cx: &mut Context<Self>) {
        if deed == Deed::Uninstall && !self.armed {
            self.armed = true;
            cx.notify();
            return;
        }
        let shortcut = self.shortcut.clone();
        let leaves = match deed {
            Deed::Open => {
                if !Files::open_folder(&shortcut.path, cx) {
                    shortcut.launch();
                }
                true
            }
            Deed::RunAsAdministrator => {
                Launch::new(&shortcut.path)
                    .verb("runas")
                    .named(format!("`{}`", shortcut.name))
                    .start();
                true
            }
            Deed::OpenFileLocation => {
                open_location(&shortcut.path, cx);
                true
            }
            Deed::PinToStart | Deed::UnpinFromStart => {
                let name = shortcut.name.clone();
                self.pins.update(cx, |pins, cx| pins.toggle(&name, cx));
                false
            }
            Deed::PinToDock | Deed::UnpinFromDock => {
                let target = self.target.clone();
                set_on_dock(shortcut, target, deed == Deed::PinToDock, cx);
                false
            }
            Deed::AddToDesktop => {
                let placed = put_on_desktop(shortcut.clone());
                cx.spawn(async move |_, cx: &mut AsyncApp| {
                    let Ok(placed) = placed.recv().await else {
                        return;
                    };
                    let told = placed.map(|_| "Added to the desktop");
                    let told = told.map_err(|why| ("Not added to the desktop", why));
                    cx.update(|cx| tell(&shortcut.name, told, cx)).ok();
                })
                .detach();
                false
            }
            Deed::AppSettings => {
                if let Some(page) = app_id_in(&shortcut.path).and_then(app_settings) {
                    page.start();
                }
                true
            }
            Deed::Uninstall => {
                let removed = Removal::remove(shortcut.clone());
                cx.spawn(async move |_, cx: &mut AsyncApp| {
                    let Ok(removed) = removed.recv().await else {
                        return;
                    };
                    // A program is removed by its own uninstaller, which
                    // tells of itself; only a package is gone already.
                    let told = match removed {
                        Ok(Removal::Package { .. }) => Ok("Uninstalled"),
                        Ok(_) => return,
                        Err(why) => Err(("Not uninstalled", why)),
                    };
                    cx.update(|cx| tell(&shortcut.name, told, cx)).ok();
                })
                .detach();
                true
            }
            Deed::Properties => {
                Launch::new(&shortcut.path)
                    .verb("properties")
                    .named(format!("`{}`", shortcut.name))
                    .start();
                true
            }
            Deed::Remove => {
                if let Some(remove) = &self.remove {
                    remove(cx);
                }
                false
            }
        };
        self.dismiss(window, cx);
        if leaves {
            cx.defer(Popups::close);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pick) = Pick::of(event) else {
            return;
        };
        cx.stop_propagation();
        let deeds = self.deeds();
        match pick {
            Pick::Leave => self.dismiss(window, cx),
            Pick::Take => {
                if let Some(&deed) = deeds.get(self.chooser.chosen) {
                    self.take(deed, window, cx);
                }
            }
            Pick::Letter(_) => {}
            pick => {
                self.chooser.pick(pick, deeds.len());
                self.armed = false;
                cx.notify();
            }
        }
    }

    /// The menu `menu` hanging from `at`, a point of the window, in front
    /// of everything else of the view drawing it.
    pub fn render_at(menu: &Entity<Self>, at: Point<Pixels>) -> AnyElement {
        let placed = anchored()
            .position(at)
            .snap_to_window_with_margin(MARGIN)
            .child(menu.clone());
        deferred(placed).with_priority(1).into_any_element()
    }
}

impl Render for AppMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        self.before.get_or_insert_with(|| window.focused(cx));
        self.chooser.take_focus(window);
        let deeds = self.deeds();
        let rows: Vec<AnyElement> = { deeds.iter().enumerate() }
            .map(|(index, &deed)| {
                let (glyph, label) = deed.look();
                let label = match (deed, self.armed) {
                    (Deed::Uninstall, true) => language.say("Press Enter again to confirm"),
                    _ => language.say(label),
                };
                let chosen = index == self.chooser.chosen;
                let state = self.chooser.state(index, &theme, window);
                let row = choice_row(&theme, glyph, label, None, chosen, state)
                    .id(("app-menu", index))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.chooser.chosen != index {
                            this.chooser.chosen = index;
                            this.armed &= deed == Deed::Uninstall;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.take(deed, window, cx)));
                Chooser::track(row, index, cx, |this: &mut Self| &mut this.chooser)
                    .into_any_element()
            })
            .collect();
        div()
            .id("app-menu")
            .track_focus(self.chooser.focus())
            .w(WIDTH)
            .p(theme.spacing.small)
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .border_1()
            .border_color(theme.colors.outline)
            .bg(theme.colors.surface_container)
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .on_key_down(cx.listener(|this, event, window, cx| this.key_down(event, window, cx)))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.dismiss(window, cx)))
            .children(rows)
    }
}

/// Where a view keeps the menu it shows, if it shows one.
#[derive(Default)]
pub struct MenuSlot(Option<(Entity<AppMenu>, Point<Pixels>)>);

impl MenuSlot {
    /// Shows the menu `make` makes at `at`, a point of the window, in
    /// place of the one shown before; it goes once it is dismissed.
    /// `find` gives the slot of the view `V`.
    pub fn open<V: 'static>(
        &mut self,
        at: Point<Pixels>,
        make: impl FnOnce(&mut Context<AppMenu>) -> AppMenu,
        find: fn(&mut V) -> &mut Self,
        cx: &mut Context<V>,
    ) {
        // A menu shown in place of another gives the keys back to what
        // had them before the first.
        let before = { self.0.take() }
            .and_then(|(shown, _)| shown.update(cx, |shown, _| shown.before.take()));
        let menu = cx.new(make);
        if let Some(before) = before {
            menu.update(cx, |menu, _| menu.before = Some(before));
        }
        cx.subscribe(&menu, move |view, menu, _: &DismissEvent, cx| {
            let slot = find(view);
            if slot.0.as_ref().is_some_and(|(shown, _)| *shown == menu) {
                slot.0 = None;
                cx.notify();
            }
        })
        .detach();
        self.0 = Some((menu, at));
        cx.notify();
    }

    /// The menu shown, in front of the view.
    pub fn render(&self) -> Option<AnyElement> {
        let (menu, at) = self.0.as_ref()?;
        Some(AppMenu::render_at(menu, *at))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_menu_of_an_application_offers_what_the_start_menu_does() {
        let program = Standing {
            on_start: Some(false),
            on_dock: Some(true),
            program: true,
            ..Standing::default()
        };
        assert_eq!(
            deeds(ItemKind::App, program),
            [
                Deed::Open,
                Deed::RunAsAdministrator,
                Deed::OpenFileLocation,
                Deed::PinToStart,
                Deed::UnpinFromDock,
                Deed::AddToDesktop,
                Deed::Uninstall,
                Deed::Properties,
            ]
        );
        let packaged = Standing {
            packaged: true,
            on_start: Some(true),
            removable: true,
            ..Standing::default()
        };
        assert_eq!(
            deeds(ItemKind::App, packaged),
            [
                Deed::Open,
                Deed::UnpinFromStart,
                Deed::AddToDesktop,
                Deed::AppSettings,
                Deed::Uninstall,
                Deed::Remove,
            ]
        );
        assert_eq!(
            deeds(ItemKind::File, Standing::default()),
            [Deed::Open, Deed::OpenFileLocation, Deed::Properties]
        );
    }

    #[test]
    fn the_dock_pins_an_application_by_its_name_and_unpins_it_in_every_way() {
        let shortcut = Shortcut {
            name: "Paint".to_owned(),
            path: PathBuf::from(r"C:\Start\Paint.lnk"),
        };
        let pins = [
            "Notepad".to_owned(),
            "C:/Start/Paint.lnk".to_owned(),
            "paint".to_owned(),
        ];
        assert!(docked(&pins, &shortcut, None));
        assert_eq!(repinned(&pins, &shortcut, None, false), ["Notepad"]);
        assert_eq!(
            repinned(&["Notepad".to_owned()], &shortcut, None, true),
            ["Notepad", "Paint"]
        );
        assert!(!docked(&["Notepad".to_owned()], &shortcut, None));
        // The dock pins a window by the path of its program.
        let program = Path::new(r"C:\Windows\System32\mspaint.exe");
        let by_program = ["c:/windows/system32/MSPAINT.exe".to_owned()];
        assert!(docked(&by_program, &shortcut, Some(program)));
        assert!(!docked(&by_program, &shortcut, None));
        assert!(repinned(&by_program, &shortcut, Some(program), false).is_empty());
    }
}
