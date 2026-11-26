// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A window of the file manager: a title bar of its own, the way back and
//! the path above, the places and drives at the side, and what the folder
//! holds as rows or tiles, of which only those in sight are drawn.

use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local};
use gpui::{
    AnyElement, AnyWindowHandle, AppContext, AsyncApp, Context, Entity, FocusHandle, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement,
    Pixels, Point, Render, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, Task,
    UniformListScrollHandle, WeakEntity, Window, WindowControlArea, div, img,
    prelude::FluentBuilder, px, uniform_list,
};

use super::model::{
    Column, Entry, History, Listing, Location, Order, Selection, crumbs, free_name, is_valid_name,
    read_folder, size_text, stamp_of,
};
use super::{Files, FilesConfig, Look};
use crate::field::TextField;
use crate::i18n::Language;
use crate::icons::AppIcons;
use crate::menu::{MenuEntry, Menus};
use crate::platform::{
    Drive, DriveKind, FileClipboard, FileWork, Place, RecycleBin, Shortcut, open_terminal,
    reveal_in_explorer, show_properties,
};
use crate::popup::PopupAnchor;
use crate::theme::Theme;
use crate::ui::{icon, logo, one_line, place_look};

/// Measures of the window, in logical pixels.
const TITLE_HEIGHT: f32 = 40.;
const TOOLBAR_HEIGHT: f32 = 48.;
const SIDEBAR_WIDTH: f32 = 220.;
const ROW_HEIGHT: f32 = 32.;
const HEADER_HEIGHT: f32 = 30.;
const STATUS_HEIGHT: f32 = 28.;
const TILE_WIDTH: f32 = 112.;
const TILE_HEIGHT: f32 = 116.;
const CAPTION_BUTTON_WIDTH: f32 = 46.;

/// The widths of the columns after the name.
const MODIFIED_WIDTH: f32 = 150.;
const KIND_WIDTH: f32 = 90.;
const SIZE_WIDTH: f32 = 90.;

/// How often the folder shown is looked at for changes, while the window
/// is in front.
const WATCH_INTERVAL: Duration = Duration::from_millis(1500);

/// How many icons of files are looked up for one frame; the rest come on
/// the frames after, so that a folder of new kinds opens at once.
const ICONS_PER_FRAME: usize = 12;

/// Characters of the icon font.
const ICON_BACK: &str = "\u{e72b}";
const ICON_FORWARD: &str = "\u{e72a}";
const ICON_UP: &str = "\u{e74a}";
const ICON_REFRESH: &str = "\u{e72c}";
const ICON_SEARCH: &str = "\u{e721}";
const ICON_FOLDER: &str = "\u{e8b7}";
const ICON_NEW_FOLDER: &str = "\u{e8f4}";
const ICON_COMPUTER: &str = "\u{e977}";
const ICON_DRIVE: &str = "\u{eda2}";
const ICON_USB: &str = "\u{e88e}";
const ICON_NETWORK: &str = "\u{e968}";
const ICON_DISC: &str = "\u{e958}";
const ICON_BIN: &str = "\u{e74d}";
const ICON_DETAILS: &str = "\u{e8fd}";
const ICON_TILES: &str = "\u{f0e2}";
const ICON_HIDDEN: &str = "\u{e7b3}";
const ICON_OPEN: &str = "\u{e8e5}";
const ICON_CUT: &str = "\u{e8c6}";
const ICON_COPY: &str = "\u{e8c8}";
const ICON_PASTE: &str = "\u{e77f}";
const ICON_RENAME: &str = "\u{e8ac}";
const ICON_PROPERTIES: &str = "\u{e946}";
const ICON_TERMINAL: &str = "\u{e756}";
const ICON_NEW_WINDOW: &str = "\u{e78b}";
const ICON_EXPLORER: &str = "\u{ec50}";
const ICON_LINK: &str = "\u{e71b}";
const ICON_MINIMIZE: &str = "\u{e921}";
const ICON_MAXIMIZE: &str = "\u{e922}";
const ICON_CLOSE: &str = "\u{e8bb}";
const ICON_SORT_UP: &str = "\u{e70e}";
const ICON_SORT_DOWN: &str = "\u{e70d}";

/// A file or folder being renamed, with the field its new name is
/// written in.
struct Rename {
    path: PathBuf,
    field: Entity<TextField>,
}

/// What the side of the window offers.
#[derive(Default)]
struct Places {
    folders: Vec<(Place, PathBuf)>,
    drives: Vec<Drive>,
}

/// The root view of a window of the file manager.
pub struct FilesView {
    history: History,
    listing: Listing,
    selection: Selection,
    order: Order,
    hidden: bool,
    look: Look,
    places: Places,
    /// What the filter field narrows the list to.
    filter: Entity<TextField>,
    /// The path being written in place of the steps, when it is.
    address: Option<Entity<TextField>>,
    rename: Option<Rename>,
    /// Why the folder cannot be shown.
    problem: Option<String>,
    /// When the folder shown was changed, as it was read.
    stamp: Option<SystemTime>,
    reading: Option<Task<()>>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// How many tiles stand in a row, as last drawn.
    columns: usize,
    /// How many icons the frame being drawn may still look up, and
    /// whether one was left for the next.
    icons_left: Cell<usize>,
    icons_owed: Cell<bool>,
    /// The window this view is the root of.
    window: AnyWindowHandle,
    _watch: Task<()>,
}

impl FilesView {
    pub fn new(
        location: Location,
        config: FilesConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = Language::of(cx);
        let filter =
            cx.new(|cx| TextField::new(language.say("Filter").into(), cx).with_glyph(ICON_SEARCH));
        cx.observe(&filter, |this, _, cx| this.arrange(cx)).detach();
        let focus = cx.focus_handle();
        window.focus(&focus);
        let watch = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_INTERVAL).await;
                let stale = this.update_in(cx, |this, window, _| {
                    window
                        .is_window_active()
                        .then(|| this.history.current().clone())
                });
                let Ok(current) = stale else {
                    break;
                };
                let Some(Location::Folder(path)) = current else {
                    continue;
                };
                let stamp = cx
                    .background_executor()
                    .spawn(async move { stamp_of(&path) })
                    .await;
                let changed = this.update(cx, |this, cx| {
                    if this.stamp != stamp && this.reading.is_none() {
                        this.load(cx);
                    }
                });
                if changed.is_err() {
                    break;
                }
            }
        });
        let mut view = Self {
            history: History::new(location),
            listing: Listing::default(),
            selection: Selection::default(),
            order: Order {
                folders_first: config.folders_first,
                ..Order::default()
            },
            hidden: config.hidden,
            look: config.view,
            places: Places::default(),
            filter,
            address: None,
            rename: None,
            problem: None,
            stamp: None,
            reading: None,
            focus,
            scroll: UniformListScrollHandle::new(),
            columns: 1,
            icons_left: Cell::new(ICONS_PER_FRAME),
            icons_owed: Cell::new(false),
            window: window.window_handle(),
            _watch: watch,
        };
        view.read_places(cx);
        view.load(cx);
        view
    }

    /// Reads the known folders and the drives, away from the user
    /// interface, as a drive may take its time to answer.
    fn read_places(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let places = cx
                .background_executor()
                .spawn(async {
                    let found = |place: Place| Some((place, place.path()?));
                    Places {
                        folders: Place::ALL.into_iter().filter_map(found).collect(),
                        drives: Drive::all(),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.places = places;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads the folder shown anew, keeping what was chosen.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Location::Folder(path) = self.history.current().clone() else {
            self.listing = Listing::default();
            self.problem = None;
            self.read_places(cx);
            cx.notify();
            return;
        };
        self.reading = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let read = cx
                    .background_executor()
                    .spawn(async move {
                        let stamp = stamp_of(&path);
                        (read_folder(&path), stamp)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.reading = None;
                    let (entries, stamp) = read;
                    this.stamp = stamp;
                    match entries {
                        Ok(entries) => {
                            this.problem = None;
                            this.listing = Listing::new(entries);
                        }
                        Err(error) => {
                            this.problem = Some(error.to_string());
                            this.listing = Listing::default();
                        }
                    }
                    this.arrange(cx);
                })
                .ok();
            }),
        );
    }

    /// Orders and filters the list again, keeping the choice and the
    /// cursor on what they were on.
    fn arrange(&mut self, cx: &mut Context<Self>) {
        let cursor = { self.selection.cursor }
            .and_then(|position| self.listing.get(position))
            .map(|entry| entry.path.clone());
        let filter = self.filter.read(cx).text();
        self.listing.arrange(&self.order, &filter, self.hidden);
        self.selection.follow(cursor, &self.listing);
        cx.notify();
    }

    /// Goes to `location`, as a click or a key asks.
    fn go(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.go(location) {
            self.arrived(window, cx);
        }
    }

    /// Shows the location the history is at.
    fn arrived(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.clear();
        self.rename = None;
        self.address = None;
        self.filter.update(cx, |field, cx| field.clear(cx));
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        let title = match self.history.current() {
            Location::Computer => Language::of(cx).say("This PC").to_owned(),
            Location::Folder(path) => title_of(path),
        };
        window.set_window_title(&title);
        window.focus(&self.focus);
        self.load(cx);
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.back() {
            self.arrived(window, cx);
        }
    }

    fn forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.forward() {
            self.arrived(window, cx);
        }
    }

    fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.up() {
            self.arrived(window, cx);
        }
    }

    /// The folder shown, where it is one.
    fn folder(&self) -> Option<PathBuf> {
        self.history.current().folder().map(Path::to_owned)
    }

    /// What is chosen, in the order of the list.
    fn chosen(&self) -> Vec<PathBuf> {
        self.selection.in_order(&self.listing)
    }

    /// Opens what is at `position`: a folder here, anything else as a
    /// double click in Explorer does.
    fn open_at(&mut self, position: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.listing.get(position).cloned() else {
            return;
        };
        self.open(&entry, window, cx);
    }

    fn open(&mut self, entry: &Entry, window: &mut Window, cx: &mut Context<Self>) {
        if entry.folder {
            self.go(Location::Folder(entry.path.clone()), window, cx);
        } else {
            Shortcut {
                name: entry.name.clone(),
                path: entry.path.clone(),
            }
            .launch();
        }
    }

    /// Opens what is chosen: one folder here, several in windows of their
    /// own, files with their programs.
    fn open_chosen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen: Vec<Entry> = { self.chosen().into_iter() }
            .filter_map(|path| self.listing.position_of(&path))
            .filter_map(|position| self.listing.get(position).cloned())
            .collect();
        match chosen.as_slice() {
            [one] => self.open(one, window, cx),
            many => {
                for entry in many {
                    match entry.folder {
                        true => Files::open(Location::Folder(entry.path.clone()), cx),
                        false => self.open(entry, window, cx),
                    }
                }
            }
        }
    }

    /// Puts what is chosen on the clipboard, which Explorer shares.
    fn put_on_clipboard(&mut self, cut: bool) {
        let chosen = self.chosen();
        if chosen.is_empty() {
            return;
        }
        if let Err(error) = FileClipboard::put(&chosen, cut) {
            report!("the files are not on the clipboard: {error:#}");
        }
    }

    /// Copies or moves what is on the clipboard into the folder shown.
    fn paste(&mut self, cx: &mut Context<Self>) {
        let (Some(folder), Some((from, cut))) = (self.folder(), FileClipboard::take()) else {
            return;
        };
        let work = match cut {
            true => FileWork::Move { from, to: folder },
            false => FileWork::Copy { from, to: folder },
        };
        self.work(work, cx);
    }

    /// Has the Shell do `work`, and reads the folder when it is done.
    fn work(&mut self, work: FileWork, cx: &mut Context<Self>) {
        let (done, finished) = async_channel::bounded::<()>(1);
        work.start(move || {
            done.try_send(()).ok();
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            finished.recv().await.ok();
            this.update(cx, |this, cx| this.load(cx)).ok();
        })
        .detach();
    }

    /// Puts what is chosen into the Recycle Bin, or with `erase` deletes
    /// it for good, after the Shell asks.
    fn delete(&mut self, erase: bool, cx: &mut Context<Self>) {
        let chosen = self.chosen();
        if chosen.is_empty() {
            return;
        }
        let work = match erase {
            true => FileWork::Erase(chosen),
            false => FileWork::Recycle(chosen),
        };
        self.work(work, cx);
    }

    /// Makes a new folder in the folder shown and starts renaming it.
    fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.folder() else {
            return;
        };
        let base = Language::of(cx).say("New folder");
        let path = free_name(&folder, base);
        if let Err(error) = std::fs::create_dir(&path) {
            report!("{} cannot be made: {error}", path.display());
            return;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        self.load(cx);
        self.selection.clear();
        self.selection.chosen.insert(path.clone());
        self.begin_rename(path, name.unwrap_or_default(), window, cx);
    }

    /// Starts renaming the file or folder at `path`, called `name`.
    fn begin_rename(
        &mut self,
        path: PathBuf,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let field = cx.new(|cx| {
            let mut field = TextField::new(SharedString::default(), cx);
            field.set_text(&name, !path.is_dir(), cx);
            field
        });
        field.read(cx).focus(window);
        self.rename = Some(Rename { path, field });
        cx.notify();
    }

    /// Renames the chosen file or folder, where one is chosen.
    fn rename_chosen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = self.chosen();
        let [path] = chosen.as_slice() else {
            return;
        };
        let Some(name) = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            return;
        };
        self.begin_rename(path.clone(), name, window, cx);
    }

    /// Gives the file being renamed the name in its field; with `keep`,
    /// the one it had.
    fn end_rename(&mut self, keep: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        window.focus(&self.focus);
        cx.notify();
        if keep {
            return;
        }
        let name = rename.field.read(cx).text();
        let name = name.trim();
        let Some(parent) = rename.path.parent() else {
            return;
        };
        let target = parent.join(name);
        if target == rename.path || name.is_empty() {
            return;
        }
        if !is_valid_name(name) {
            report!("`{name}` cannot be the name of a file");
            return;
        }
        match std::fs::rename(&rename.path, &target) {
            Ok(()) => {
                self.selection.clear();
                self.selection.chosen.insert(target);
                self.load(cx);
            }
            Err(error) => report!("{} cannot be renamed: {error}", rename.path.display()),
        }
    }

    /// Writes the path of the folder shown in a field instead of its steps.
    fn edit_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .folder()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let field = cx.new(|cx| {
            let mut field = TextField::new(SharedString::default(), cx);
            field.set_text(&text, false, cx);
            field
        });
        field.read(cx).focus(window);
        self.address = Some(field);
        cx.notify();
    }

    /// Goes where the address field says, or back to the steps.
    fn end_address(&mut self, go: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.address.take() else {
            return;
        };
        window.focus(&self.focus);
        cx.notify();
        if !go {
            return;
        }
        let location = Files::named(&field.read(cx).text());
        match &location {
            Location::Folder(path) if path.is_file() => {
                if let Err(error) = reveal_in_explorer(path) {
                    report!("{error:#}");
                }
            }
            _ => self.go(location, window, cx),
        }
    }

    /// Whether a field of the window has the keys.
    fn typing(&self, window: &Window, cx: &Context<Self>) -> bool {
        let fields = [Some(&self.filter), self.address.as_ref()]
            .into_iter()
            .chain([self.rename.as_ref().map(|rename| &rename.field)]);
        fields
            .flatten()
            .any(|field| field.read(cx).is_focused(window))
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let held = &event.keystroke.modifiers;
        if self.rename.is_some() {
            match key {
                "enter" => self.end_rename(false, window, cx),
                "escape" => self.end_rename(true, window, cx),
                _ => {}
            }
            return;
        }
        if self.address.is_some() {
            match key {
                "enter" => self.end_address(true, window, cx),
                "escape" => self.end_address(false, window, cx),
                _ => {}
            }
            return;
        }
        if self.typing(window, cx) {
            match key {
                "escape" => {
                    self.filter.update(cx, |field, cx| field.clear(cx));
                    window.focus(&self.focus);
                }
                "enter" | "down" => window.focus(&self.focus),
                _ => {}
            }
            return;
        }
        let step = match self.look {
            Look::Details => 1,
            Look::Tiles => self.columns as isize,
        };
        let page = (window.viewport_size().height / px(ROW_HEIGHT)) as isize;
        match (key, held.control, held.alt, held.shift) {
            ("left", false, true, _) | ("backspace", false, false, _) => self.back(window, cx),
            ("right", false, true, _) => self.forward(window, cx),
            ("up", false, true, _) => self.up(window, cx),
            ("up", false, false, extend) => self.step(-step, extend, cx),
            ("down", false, false, extend) => self.step(step, extend, cx),
            ("left", false, false, extend) if self.look == Look::Tiles => self.step(-1, extend, cx),
            ("right", false, false, extend) if self.look == Look::Tiles => self.step(1, extend, cx),
            ("pageup", _, false, extend) => self.step(-page, extend, cx),
            ("pagedown", _, false, extend) => self.step(page, extend, cx),
            ("home", _, false, extend) => self.step(isize::MIN / 2, extend, cx),
            ("end", _, false, extend) => self.step(isize::MAX / 2, extend, cx),
            ("enter", false, false, false) => self.open_chosen(window, cx),
            ("enter", false, true, false) => self.properties(),
            ("f2", ..) => self.rename_chosen(window, cx),
            ("f5", ..) | ("r", true, false, false) => self.load(cx),
            ("delete", false, false, erase) => self.delete(erase, cx),
            ("a", true, false, false) => {
                self.selection.all(&self.listing);
                cx.notify();
            }
            ("c", true, false, false) => self.put_on_clipboard(false),
            ("x", true, false, false) => self.put_on_clipboard(true),
            ("v", true, false, false) => self.paste(cx),
            ("n", true, false, true) => self.new_folder(window, cx),
            ("n", true, false, false) => Files::open(self.history.current().clone(), cx),
            ("w", true, false, false) => window.remove_window(),
            ("h", true, false, false) => self.toggle_hidden(cx),
            ("l", true, false, false) | ("d", false, true, false) => self.edit_address(window, cx),
            ("f", true, false, false) | ("e", true, false, false) => {
                self.filter.read(cx).focus(window)
            }
            ("1", true, false, true) => self.set_look(Look::Details, cx),
            ("2", true, false, true) => self.set_look(Look::Tiles, cx),
            _ => {}
        }
    }

    fn step(&mut self, by: isize, extend: bool, cx: &mut Context<Self>) {
        self.selection.step(&self.listing, by, extend);
        if let Some(cursor) = self.selection.cursor {
            let row = match self.look {
                Look::Details => cursor,
                Look::Tiles => cursor / self.columns.max(1),
            };
            self.scroll.scroll_to_item(row, ScrollStrategy::Top);
        }
        cx.notify();
    }

    fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        self.hidden = !self.hidden;
        self.arrange(cx);
    }

    fn set_look(&mut self, look: Look, cx: &mut Context<Self>) {
        self.look = look;
        cx.notify();
    }

    fn sort_by(&mut self, column: Column, cx: &mut Context<Self>) {
        self.order = self.order.toggled(column);
        self.arrange(cx);
    }

    fn properties(&self) {
        let chosen = self.chosen();
        let path = chosen.first().cloned().or_else(|| self.folder());
        if let Some(path) = path
            && let Err(error) = show_properties(&path)
        {
            report!("{error:#}");
        }
    }

    /// A press of a button of the pointer on the item at `position`.
    fn press(
        &mut self,
        position: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus);
        if self.rename.is_some() {
            self.end_rename(false, window, cx);
        }
        let held = event.modifiers;
        if event.button == MouseButton::Right {
            let path = self.listing.get(position).map(|entry| entry.path.clone());
            if path.is_some_and(|path| !self.selection.contains(&path)) {
                self.selection.click(&self.listing, position, false, false);
            }
            cx.notify();
            self.open_menu(event.position, window, cx);
            return;
        }
        if event.click_count >= 2 {
            self.open_at(position, window, cx);
            return;
        }
        self.selection
            .click(&self.listing, position, held.control, held.shift);
        cx.notify();
    }

    /// A press on the list where it holds nothing.
    fn press_empty(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        if self.rename.is_some() {
            self.end_rename(false, window, cx);
        }
        self.selection.clear();
        cx.notify();
        if event.button == MouseButton::Right {
            self.open_menu(event.position, window, cx);
        }
    }

    /// Opens the menu of what is chosen, or of the folder shown where
    /// nothing is, under `position`.
    fn open_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = match PopupAnchor::under(window, position, cx) {
            Ok(anchor) => anchor,
            Err(error) => {
                report!("the menu has no place: {error:#}");
                return;
            }
        };
        let language = Language::of(cx);
        let view = cx.entity().downgrade();
        let entry =
            |glyph: &'static str, label: &'static str, deed: fn(&mut Self, &mut Context<Self>)| {
                let view = view.clone();
                MenuEntry::new(glyph, language.say(label), move |cx| {
                    view.update(cx, deed).ok();
                })
            };
        let chosen = self.chosen();
        let mut entries = Vec::new();
        if chosen.is_empty() {
            if self.folder().is_some() {
                entries.push(entry(ICON_NEW_FOLDER, "New folder", |this, cx| {
                    this.after_menu(cx, |this, window, cx| this.new_folder(window, cx))
                }));
                if FileClipboard::take().is_some() {
                    entries.push(entry(ICON_PASTE, "Paste", |this, cx| this.paste(cx)));
                }
                entries.push(entry(ICON_TERMINAL, "Open in terminal", |this, _| {
                    if let Some(folder) = this.folder()
                        && let Err(error) = open_terminal(&folder)
                    {
                        report!("{error:#}");
                    }
                }));
            }
            entries.push(entry(ICON_HIDDEN, "Show hidden files", |this, cx| {
                this.toggle_hidden(cx)
            }));
            entries.push(entry(ICON_PROPERTIES, "Properties", |this, _| {
                this.properties()
            }));
        } else {
            entries.push(entry(ICON_OPEN, "Open", |this, cx| {
                this.after_menu(cx, |this, window, cx| this.open_chosen(window, cx))
            }));
            let one_folder = matches!(chosen.as_slice(), [path] if path.is_dir());
            if one_folder {
                entries.push(entry(ICON_NEW_WINDOW, "Open in new window", |this, cx| {
                    for path in this.chosen() {
                        Files::open(Location::Folder(path), cx);
                    }
                }));
                entries.push(entry(ICON_TERMINAL, "Open in terminal", |this, _| {
                    if let Some(path) = this.chosen().first()
                        && let Err(error) = open_terminal(path)
                    {
                        report!("{error:#}");
                    }
                }));
            }
            entries.push(entry(ICON_CUT, "Cut", |this, _| {
                this.put_on_clipboard(true)
            }));
            entries.push(entry(ICON_COPY, "Copy", |this, _| {
                this.put_on_clipboard(false)
            }));
            entries.push(entry(ICON_LINK, "Copy path", |this, cx| {
                let paths: Vec<String> = { this.chosen().into_iter() }
                    .map(|path| path.display().to_string())
                    .collect();
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(paths.join("\r\n")));
            }));
            if chosen.len() == 1 {
                entries.push(entry(ICON_RENAME, "Rename", |this, cx| {
                    this.after_menu(cx, |this, window, cx| this.rename_chosen(window, cx))
                }));
            }
            entries.push(entry(ICON_BIN, "Delete", |this, cx| this.delete(false, cx)));
            entries.push(entry(ICON_EXPLORER, "Show in Explorer", |this, _| {
                if let Some(path) = this.chosen().first()
                    && let Err(error) = reveal_in_explorer(path)
                {
                    report!("{error:#}");
                }
            }));
            entries.push(entry(ICON_PROPERTIES, "Properties", |this, _| {
                this.properties()
            }));
        }
        Menus::open(entries, anchor, cx);
    }

    /// Does `deed`, which needs the window, once the menu that asked for
    /// it is gone and the window is in front again.
    fn after_menu(
        &mut self,
        cx: &mut Context<Self>,
        deed: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let view = cx.entity().downgrade();
        let window = self.window;
        cx.defer(move |cx| {
            window
                .update(cx, |_, window, cx| {
                    window.activate_window();
                    if let Some(view) = view.upgrade() {
                        view.update(cx, |this, cx| deed(this, window, cx));
                    }
                })
                .ok();
        });
    }
}

/// What the title of a window at `path` says: the name of the folder.
fn title_of(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    }
}

/// When a file was changed, as the list says it.
fn moment(time: Option<SystemTime>) -> String {
    match time {
        Some(time) => DateTime::<Local>::from(time)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        None => String::new(),
    }
}

/// The glyph of a drive of `kind`.
fn drive_glyph(kind: DriveKind) -> &'static str {
    match kind {
        DriveKind::Fixed => ICON_DRIVE,
        DriveKind::Removable => ICON_USB,
        DriveKind::Network => ICON_NETWORK,
        DriveKind::Optical => ICON_DISC,
    }
}

/// How much of a drive is used, from 0 to 1, where it can be told.
fn used_share(drive: &Drive) -> Option<f32> {
    let (total, free) = (drive.total?, drive.free?);
    (total > 0).then(|| (1. - free as f32 / total as f32).clamp(0., 1.))
}

/// A square button of the window with `glyph`, dimmed unless `enabled`.
fn tool_button(
    theme: &Theme,
    id: &'static str,
    glyph: &'static str,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    let color = match enabled {
        true => theme.colors.on_surface,
        false => theme.colors.on_surface_variant.opacity(0.45),
    };
    div()
        .id(id)
        .size(px(34.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.shape.small)
        .text_color(color)
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(theme.colors.surface_container_highest))
        })
        .child(icon(theme, glyph))
}

impl FilesView {
    fn render_title(&self, theme: &Theme, window: &Window) -> impl IntoElement + use<> {
        let title: SharedString = window.window_title().into();
        let caption =
            |id: &'static str, glyph: &'static str, area: WindowControlArea, close: bool| {
                let hover: Hsla = match close {
                    true => gpui::rgb(0xc42b1c).into(),
                    false => theme.colors.surface_container_highest,
                };
                div()
                    .id(id)
                    .w(px(CAPTION_BUTTON_WIDTH))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.))
                    .font_family(theme.typography.icon_family)
                    .text_color(theme.colors.on_surface)
                    .hover(move |style| style.bg(hover))
                    .window_control_area(area)
                    .child(glyph)
            };
        div()
            .h(px(TITLE_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .child(
                div()
                    .id("files-drag")
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .px(theme.spacing.medium)
                    .window_control_area(WindowControlArea::Drag)
                    .child(logo(px(16.), theme.colors.primary))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(theme.typography.label_size)
                            .text_color(theme.colors.on_surface)
                            .child(one_line(title)),
                    ),
            )
            .child(caption(
                "files-min",
                ICON_MINIMIZE,
                WindowControlArea::Min,
                false,
            ))
            .child(caption(
                "files-max",
                ICON_MAXIMIZE,
                WindowControlArea::Max,
                false,
            ))
            .child(caption(
                "files-close",
                ICON_CLOSE,
                WindowControlArea::Close,
                true,
            ))
    }

    fn render_toolbar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let history = &self.history;
        let navigation = div()
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .child(
                tool_button(theme, "files-back", ICON_BACK, history.can_go_back())
                    .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
            )
            .child(
                tool_button(
                    theme,
                    "files-forward",
                    ICON_FORWARD,
                    history.can_go_forward(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.forward(window, cx))),
            )
            .child(
                tool_button(theme, "files-up", ICON_UP, history.current().up().is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.up(window, cx))),
            )
            .child(
                tool_button(theme, "files-refresh", ICON_REFRESH, true)
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
            );
        let address = match &self.address {
            Some(field) => div()
                .flex_1()
                .min_w_0()
                .child(field.clone())
                .into_any_element(),
            None => self.render_crumbs(theme, cx),
        };
        let looks = div()
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .child(
                tool_button(theme, "files-new", ICON_NEW_FOLDER, self.folder().is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.new_folder(window, cx))),
            )
            .child(
                self.look_button(theme, Look::Details, ICON_DETAILS)
                    .on_click(cx.listener(|this, _, _, cx| this.set_look(Look::Details, cx))),
            )
            .child(
                self.look_button(theme, Look::Tiles, ICON_TILES)
                    .on_click(cx.listener(|this, _, _, cx| this.set_look(Look::Tiles, cx))),
            )
            .child(
                tool_button(theme, "files-hidden", ICON_HIDDEN, true)
                    .when(self.hidden, |button| {
                        button.text_color(theme.colors.primary)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_hidden(cx))),
            );
        div()
            .h(px(TOOLBAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .px(theme.spacing.small)
            .child(navigation)
            .child(address)
            .child(div().w(px(220.)).flex_none().child(self.filter.clone()))
            .child(looks)
    }

    fn look_button(
        &self,
        theme: &Theme,
        look: Look,
        glyph: &'static str,
    ) -> gpui::Stateful<gpui::Div> {
        let id = match look {
            Look::Details => "files-details",
            Look::Tiles => "files-tiles",
        };
        tool_button(theme, id, glyph, true).when(self.look == look, |button| {
            button
                .bg(theme.colors.secondary_container)
                .text_color(theme.colors.on_surface)
        })
    }

    /// The steps of the path, each a button to that folder; a click on
    /// what is left of the bar writes the path instead.
    fn render_crumbs(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let language = Language::of(cx);
        let mut steps: Vec<AnyElement> = Vec::new();
        let root = div()
            .id("files-crumb-computer")
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .px(theme.spacing.small)
            .h(px(28.))
            .rounded(theme.shape.small)
            .cursor_pointer()
            .hover(|style| style.bg(theme.colors.surface_container_highest))
            .child(icon(theme, ICON_COMPUTER))
            .child(language.say("This PC"))
            .on_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.go(Location::Computer, window, cx)
            }));
        steps.push(root.into_any_element());
        if let Location::Folder(path) = self.history.current() {
            for (index, (name, target)) in crumbs(path).into_iter().enumerate() {
                steps.push(
                    div()
                        .text_color(theme.colors.on_surface_variant)
                        .child("\u{203a}")
                        .into_any_element(),
                );
                steps.push(
                    div()
                        .id(("files-crumb", index))
                        .flex_none()
                        .px(theme.spacing.small)
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .rounded(theme.shape.small)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.colors.surface_container_highest))
                        .child(name)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.go(Location::Folder(target.clone()), window, cx)
                        }))
                        .into_any_element(),
                );
            }
        }
        div()
            .id("files-address")
            .flex_1()
            .min_w_0()
            .h(px(34.))
            .flex()
            .items_center()
            .gap(px(2.))
            .px(theme.spacing.tiny)
            .overflow_hidden()
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container_highest.opacity(0.6))
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            .children(steps)
            .child(div().flex_1().h_full())
            .on_click(cx.listener(|this, _, window, cx| this.edit_address(window, cx)))
            .into_any_element()
    }

    fn render_sidebar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let language = Language::of(cx);
        let current = self.history.current().clone();
        let item = |id: (&'static str, usize), glyph: &'static str, label: String, lit: bool| {
            div()
                .id(id)
                .h(px(34.))
                .flex_none()
                .flex()
                .items_center()
                .gap(theme.spacing.small)
                .px(theme.spacing.medium)
                .rounded(theme.shape.medium)
                .cursor_pointer()
                .text_color(theme.colors.on_surface)
                .when(lit, |item| item.bg(theme.colors.secondary_container))
                .when(!lit, |item| {
                    item.hover(|style| style.bg(theme.colors.surface_container_highest))
                })
                .child(
                    div()
                        .text_color(theme.colors.primary)
                        .child(icon(theme, glyph)),
                )
                .child(div().flex_1().min_w_0().child(one_line(label)))
        };
        let heading = |text: &'static str| {
            div()
                .pt(theme.spacing.medium)
                .pb(theme.spacing.tiny)
                .px(theme.spacing.medium)
                .text_size(theme.typography.label_size * 0.85)
                .text_color(theme.colors.on_surface_variant)
                .child(language.say(text))
        };
        let mut column = div()
            .id("files-side")
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(theme.spacing.small)
            .overflow_y_scroll()
            .text_size(theme.typography.label_size)
            .child(heading("Places"));
        for (index, (place, path)) in self.places.folders.iter().enumerate() {
            let (label, glyph) = place_look(*place);
            let target = Location::Folder(path.clone());
            let lit = current == target;
            column = column.child(
                item(
                    ("files-place", index),
                    glyph,
                    language.say(label).to_owned(),
                    lit,
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.go(target.clone(), window, cx)),
                ),
            );
        }
        column = column.child(heading("This PC")).child(
            item(
                ("files-computer", 0),
                ICON_COMPUTER,
                language.say("This PC").to_owned(),
                current == Location::Computer,
            )
            .on_click(cx.listener(|this, _, window, cx| this.go(Location::Computer, window, cx))),
        );
        for (index, drive) in self.places.drives.iter().enumerate() {
            let target = Location::Folder(drive.root.clone());
            let lit = current == target;
            column = column.child(
                item(
                    ("files-drive", index),
                    drive_glyph(drive.kind),
                    drive.title(),
                    lit,
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.go(target.clone(), window, cx)),
                ),
            );
        }
        column.child(
            item(
                ("files-bin", 0),
                ICON_BIN,
                language.say("Recycle Bin").to_owned(),
                false,
            )
            .on_click(|_, _, _| {
                if let Err(error) = RecycleBin::open() {
                    report!("the Recycle Bin did not open: {error}");
                }
            }),
        )
    }

    fn render_header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let language = Language::of(cx);
        let order = self.order;
        let heading = |column: Column| {
            let arrow = (order.column == column).then(|| {
                let glyph = if order.descending {
                    ICON_SORT_DOWN
                } else {
                    ICON_SORT_UP
                };
                icon(theme, glyph).text_size(px(9.))
            });
            let cell = div()
                .id(("files-heading", column as usize))
                .h_full()
                .flex()
                .items_center()
                .gap(theme.spacing.tiny)
                .px(theme.spacing.small)
                .cursor_pointer()
                .hover(|style| style.text_color(theme.colors.on_surface))
                .child(language.say(column.title()))
                .children(arrow)
                .on_click(cx.listener(move |this, _, _, cx| this.sort_by(column, cx)));
            match column {
                Column::Name => cell.flex_1().min_w_0(),
                Column::Modified => cell.w(px(MODIFIED_WIDTH)).flex_none(),
                Column::Kind => cell.w(px(KIND_WIDTH)).flex_none(),
                Column::Size => cell.w(px(SIZE_WIDTH)).flex_none().justify_end(),
            }
        };
        div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(36.))
            .pr(theme.spacing.medium)
            .text_size(theme.typography.label_size * 0.85)
            .text_color(theme.colors.on_surface_variant)
            .border_b_1()
            .border_color(theme.colors.outline.opacity(0.25))
            .children(Column::ALL.map(heading))
    }

    /// The icon of `entry`, `size` wide: the folder of the icon font for a
    /// folder, the one the Shell draws for a file; a blank one until it
    /// was looked up.
    fn entry_icon(
        &self,
        entry: &Entry,
        size: Pixels,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        if entry.folder {
            return div()
                .size(size)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .font_family(theme.typography.icon_family)
                .text_size(size * 0.85)
                .text_color(theme.colors.primary)
                .child(ICON_FOLDER)
                .into_any_element();
        }
        let icons = cx.global::<AppIcons>();
        let known = icons.has_of_file(&entry.path);
        let image = match known || self.icons_left.get() > 0 {
            true => {
                if !known {
                    self.icons_left.set(self.icons_left.get() - 1);
                }
                icons.of_file(&entry.path)
            }
            false => {
                self.icons_owed.set(true);
                None
            }
        };
        match image {
            Some(image) => img(image).size(size).flex_none().into_any_element(),
            None => div()
                .size(size)
                .flex_none()
                .rounded(theme.shape.small)
                .bg(theme.colors.outline.opacity(0.3))
                .into_any_element(),
        }
    }

    /// The name of `entry`, or the field it is renamed in.
    fn entry_name(&self, entry: &Entry) -> AnyElement {
        match &self.rename {
            Some(rename) if rename.path == entry.path => div()
                .flex_1()
                .min_w_0()
                .child(rename.field.clone())
                .into_any_element(),
            _ => div()
                .flex_1()
                .min_w_0()
                .child(one_line(entry.name.clone()))
                .into_any_element(),
        }
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        range
            .filter_map(|position| {
                let entry = self.listing.get(position)?.clone();
                let chosen = self.selection.contains(&entry.path);
                let cursor = self.selection.cursor == Some(position);
                let dim = entry.hidden;
                let size = match entry.folder {
                    true => String::new(),
                    false => size_text(entry.size),
                };
                Some(
                    div()
                        .id(("files-row", position))
                        .h(px(ROW_HEIGHT))
                        .mx(theme.spacing.small)
                        .flex()
                        .items_center()
                        .gap(theme.spacing.small)
                        .px(theme.spacing.small)
                        .rounded(theme.shape.small)
                        .text_size(theme.typography.label_size)
                        .text_color(theme.colors.on_surface)
                        .when(dim, |row| row.opacity(0.6))
                        .when(chosen, |row| row.bg(theme.colors.secondary_container))
                        .when(!chosen, |row| {
                            row.hover(|style| style.bg(theme.colors.surface_container_highest))
                        })
                        .when(cursor, |row| {
                            row.border_1()
                                .border_color(theme.colors.primary.opacity(0.5))
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event, window, cx| {
                                cx.stop_propagation();
                                this.press(position, event, window, cx)
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event, window, cx| {
                                cx.stop_propagation();
                                this.press(position, event, window, cx)
                            }),
                        )
                        .child(self.entry_icon(&entry, px(20.), theme, cx))
                        .child(self.entry_name(&entry))
                        .child(
                            div()
                                .w(px(MODIFIED_WIDTH))
                                .flex_none()
                                .px(theme.spacing.small)
                                .text_color(theme.colors.on_surface_variant)
                                .child(moment(entry.modified)),
                        )
                        .child(
                            div()
                                .w(px(KIND_WIDTH))
                                .flex_none()
                                .px(theme.spacing.small)
                                .overflow_hidden()
                                .text_color(theme.colors.on_surface_variant)
                                .child(one_line(entry.kind())),
                        )
                        .child(
                            div()
                                .w(px(SIZE_WIDTH))
                                .flex_none()
                                .flex()
                                .justify_end()
                                .px(theme.spacing.small)
                                .text_color(theme.colors.on_surface_variant)
                                .child(size),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_tile_rows(
        &mut self,
        range: Range<usize>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let columns = self.columns.max(1);
        range
            .map(|row| {
                let first = row * columns;
                let tiles =
                    (first..(first + columns).min(self.listing.len())).filter_map(|position| {
                        let entry = self.listing.get(position)?.clone();
                        let chosen = self.selection.contains(&entry.path);
                        Some(
                            div()
                                .id(("files-tile", position))
                                .w(px(TILE_WIDTH - 8.))
                                .h(px(TILE_HEIGHT - 8.))
                                .flex_none()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap(theme.spacing.tiny)
                                .p(theme.spacing.tiny)
                                .rounded(theme.shape.medium)
                                .text_size(theme.typography.label_size * 0.92)
                                .text_color(theme.colors.on_surface)
                                .when(entry.hidden, |tile| tile.opacity(0.6))
                                .when(chosen, |tile| tile.bg(theme.colors.secondary_container))
                                .when(!chosen, |tile| {
                                    tile.hover(|style| {
                                        style.bg(theme.colors.surface_container_highest)
                                    })
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event, window, cx| {
                                        cx.stop_propagation();
                                        this.press(position, event, window, cx)
                                    }),
                                )
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, event, window, cx| {
                                        cx.stop_propagation();
                                        this.press(position, event, window, cx)
                                    }),
                                )
                                .child(self.entry_icon(&entry, px(48.), theme, cx))
                                .child(
                                    div().w_full().flex().justify_center().child(
                                        match &self.rename {
                                            Some(rename) if rename.path == entry.path => div()
                                                .w_full()
                                                .child(rename.field.clone())
                                                .into_any_element(),
                                            _ => one_line(entry.name.clone()).into_any_element(),
                                        },
                                    ),
                                ),
                        )
                    });
                div()
                    .h(px(TILE_HEIGHT))
                    .px(theme.spacing.small)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .children(tiles)
                    .into_any_element()
            })
            .collect()
    }

    /// The drives of the computer as cards with how full each is.
    fn render_computer(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let language = Language::of(cx);
        let cards = self.places.drives.iter().enumerate().map(|(index, drive)| {
            let target = Location::Folder(drive.root.clone());
            let share = used_share(drive);
            let free = match (drive.free, drive.total) {
                (Some(free), Some(total)) => format!(
                    "{} {} {}",
                    size_text(free),
                    language.say("free of"),
                    size_text(total)
                ),
                _ => String::new(),
            };
            let bar_color = match share {
                Some(share) if share > 0.9 => gpui::rgb(0xe5534b).into(),
                _ => theme.colors.primary,
            };
            div()
                .id(("files-drive-card", index))
                .w(px(260.))
                .flex_none()
                .flex()
                .items_center()
                .gap(theme.spacing.medium)
                .p(theme.spacing.medium)
                .rounded(theme.shape.large)
                .bg(theme.colors.surface_container_highest.opacity(0.7))
                .cursor_pointer()
                .hover(|style| style.bg(theme.colors.secondary_container))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.go(target.clone(), window, cx)),
                )
                .child(
                    div()
                        .font_family(theme.typography.icon_family)
                        .text_size(px(34.))
                        .text_color(theme.colors.primary)
                        .child(drive_glyph(drive.kind)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(theme.spacing.tiny)
                        .child(one_line(drive.title()))
                        .child(
                            div()
                                .h(px(6.))
                                .w_full()
                                .rounded(px(3.))
                                .bg(theme.colors.outline.opacity(0.3))
                                .children(share.map(|share| {
                                    div()
                                        .h_full()
                                        .w(gpui::relative(share))
                                        .rounded(px(3.))
                                        .bg(bar_color)
                                })),
                        )
                        .child(
                            div()
                                .text_size(theme.typography.label_size * 0.85)
                                .text_color(theme.colors.on_surface_variant)
                                .child(free),
                        ),
                )
        });
        div()
            .id("files-computer-view")
            .flex_1()
            .p(theme.spacing.medium)
            .flex()
            .flex_wrap()
            .content_start()
            .gap(theme.spacing.medium)
            .overflow_y_scroll()
            .children(cards)
            .into_any_element()
    }

    fn render_status(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let language = Language::of(cx);
        let count = self.listing.len();
        let chosen = self.selection.chosen.len();
        let mut text = format!("{count} {}", language.say("items"));
        if chosen > 0 {
            let bytes = self.listing.size_of(&self.selection.chosen);
            text.push_str(&format!("  ·  {chosen} {}", language.say("selected")));
            if bytes > 0 {
                text.push_str(&format!("  ·  {}", size_text(bytes)));
            }
        }
        if self.reading.is_some() {
            text = language.say("Reading…").to_owned();
        }
        div()
            .h(px(STATUS_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .px(theme.spacing.medium)
            .text_size(theme.typography.label_size * 0.85)
            .text_color(theme.colors.on_surface_variant)
            .child(text)
    }
}

impl Render for FilesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        self.icons_left.set(ICONS_PER_FRAME);
        self.icons_owed.set(false);
        let width = window.viewport_size().width / px(1.) - SIDEBAR_WIDTH - 24.;
        self.columns = ((width / TILE_WIDTH) as usize).max(1);
        let language = Language::of(cx);
        let content: AnyElement = match (self.history.current(), &self.problem) {
            (Location::Computer, _) => self.render_computer(&theme, cx),
            (_, Some(problem)) => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.colors.on_surface_variant)
                .child(format!(
                    "{}: {problem}",
                    language.say("This folder cannot be read")
                ))
                .into_any_element(),
            (_, None) if self.listing.is_empty() && self.reading.is_none() => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.colors.on_surface_variant)
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, event, window, cx| this.press_empty(event, window, cx)),
                )
                .child(language.say("This folder is empty"))
                .into_any_element(),
            _ => {
                let look = self.look;
                let count = match look {
                    Look::Details => self.listing.len(),
                    Look::Tiles => self.listing.len().div_ceil(self.columns),
                };
                let list = uniform_list(
                    "files-list",
                    count,
                    cx.processor(move |this, range: Range<usize>, window, cx| {
                        let theme = *cx.global::<Theme>();
                        let rows = match look {
                            Look::Details => this.render_rows(range, &theme, cx),
                            Look::Tiles => this.render_tile_rows(range, &theme, cx),
                        };
                        if this.icons_owed.get() {
                            cx.on_next_frame(window, |_, _, cx| cx.notify());
                        }
                        rows
                    }),
                )
                .track_scroll(self.scroll.clone())
                .flex_1()
                .py(theme.spacing.tiny);
                div()
                    .id("files-main")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .when(look == Look::Details, |main| {
                        main.child(self.render_header(&theme, cx))
                    })
                    .child(list)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event, window, cx| this.press_empty(event, window, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event, window, cx| this.press_empty(event, window, cx)),
                    )
                    .into_any_element()
            }
        };
        div()
            .id("files")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, window, cx| this.key_down(event, window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.surface_container)
            .font_family(theme.typography.family)
            .text_color(theme.colors.on_surface)
            .child(self.render_title(&theme, window))
            .child(self.render_toolbar(&theme, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_sidebar(&theme, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .mr(theme.spacing.small)
                            .rounded(theme.shape.large)
                            .bg(theme.colors.surface_container_highest.opacity(0.35))
                            .overflow_hidden()
                            .child(content),
                    ),
            )
            .child(self.render_status(&theme, cx))
    }
}
