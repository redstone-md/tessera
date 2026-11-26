// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A pane of a window of the file manager: one location with its own way
//! back and forth, what it holds, in which order and which of it is
//! chosen, drawn as rows or tiles of which only those in sight are drawn.
//! A window holds one pane per tab, or two side by side.

use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local};
use gpui::{
    AnyElement, AnyWindowHandle, AppContext, AsyncApp, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement,
    Pixels, Point, Render, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, Task,
    UniformListScrollHandle, WeakEntity, Window, canvas, div, img, prelude::FluentBuilder, px,
    uniform_list,
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
    Drive, DriveKind, FileClipboard, FileWork, Launch, open_terminal, reveal_in_explorer,
    show_properties,
};
use crate::popup::PopupAnchor;
use crate::theme::Theme;
use crate::ui::{icon, one_line};

/// Measures of a pane, in logical pixels.
pub const ROW_HEIGHT: f32 = 32.;
const HEADER_HEIGHT: f32 = 30.;
const TILE_WIDTH: f32 = 112.;
const TILE_HEIGHT: f32 = 116.;

/// The widths of the columns after the name.
const MODIFIED_WIDTH: f32 = 150.;
const KIND_WIDTH: f32 = 90.;
const SIZE_WIDTH: f32 = 90.;

/// How often the folder shown is looked at for changes, while the window
/// is in front and the pane in sight.
const WATCH_INTERVAL: Duration = Duration::from_millis(1500);

/// How many icons of files are looked up for one frame; the rest come on
/// the frames after, so that a folder of new kinds opens at once.
const ICONS_PER_FRAME: usize = 12;

/// Characters of the icon font.
pub const ICON_SEARCH: &str = "\u{e721}";
pub const ICON_FOLDER: &str = "\u{e8b7}";
pub const ICON_NEW_FOLDER: &str = "\u{e8f4}";
pub const ICON_COMPUTER: &str = "\u{e977}";
pub const ICON_DRIVE: &str = "\u{eda2}";
pub const ICON_USB: &str = "\u{e88e}";
pub const ICON_NETWORK: &str = "\u{e968}";
pub const ICON_DISC: &str = "\u{e958}";
pub const ICON_BIN: &str = "\u{e74d}";
pub const ICON_HIDDEN: &str = "\u{e7b3}";
const ICON_OPEN: &str = "\u{e8e5}";
const ICON_CUT: &str = "\u{e8c6}";
const ICON_COPY: &str = "\u{e8c8}";
const ICON_PASTE: &str = "\u{e77f}";
const ICON_RENAME: &str = "\u{e8ac}";
const ICON_PROPERTIES: &str = "\u{e946}";
const ICON_TERMINAL: &str = "\u{e756}";
pub const ICON_NEW_WINDOW: &str = "\u{e78b}";
pub const ICON_NEW_TAB: &str = "\u{e8a7}";
const ICON_TO_OTHER: &str = "\u{e8ab}";
const ICON_EXPLORER: &str = "\u{ec50}";
const ICON_LINK: &str = "\u{e71b}";
const ICON_SORT_UP: &str = "\u{e70e}";
const ICON_SORT_DOWN: &str = "\u{e70d}";

/// What a pane tells the window it is in.
#[derive(Clone, Debug, PartialEq)]
pub enum PaneEvent {
    /// It went somewhere else, or what it shows was read anew.
    Navigated,
    /// The user turned to it, with a click or a key.
    Focused,
    /// The user asked for `Location` in a tab of its own.
    OpenInNewTab(Location),
    /// The user asked to copy, or with `cut` to move, what is chosen in
    /// the pane into the folder of the pane beside it.
    ToOtherPane { cut: bool },
}

/// A file or folder being renamed, with the field its new name is
/// written in.
struct Rename {
    path: PathBuf,
    field: Entity<TextField>,
}

/// One location of a window of the file manager, and what it holds.
pub struct FolderPane {
    history: History,
    listing: Listing,
    selection: Selection,
    order: Order,
    hidden: bool,
    look: Look,
    /// The drives, as the computer shows them.
    drives: Vec<Drive>,
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
    /// How wide the pane was last drawn, in logical pixels.
    width: Rc<Cell<f32>>,
    /// How many tiles stand in a row, as last drawn.
    columns: usize,
    /// How many icons the frame being drawn may still look up, and
    /// whether one was left for the next.
    icons_left: Cell<usize>,
    icons_owed: Cell<bool>,
    /// Whether the pane is in sight, in the tab shown; only then is its
    /// folder watched for changes.
    visible: bool,
    /// Whether another pane stands beside it, in a split tab.
    beside: bool,
    /// Whether it is the pane in front of a split tab, which is outlined.
    lit: bool,
    /// The window the pane is in.
    window: AnyWindowHandle,
    _watch: Task<()>,
}

impl EventEmitter<PaneEvent> for FolderPane {}

impl FolderPane {
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
        let watch = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_INTERVAL).await;
                let stale = this.update_in(cx, |this, window, _| {
                    (window.is_window_active() && this.visible)
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
        let mut pane = Self {
            history: History::new(location),
            listing: Listing::default(),
            selection: Selection::default(),
            order: Order {
                folders_first: config.folders_first,
                ..Order::default()
            },
            hidden: config.hidden,
            look: config.view,
            drives: Vec::new(),
            filter,
            address: None,
            rename: None,
            problem: None,
            stamp: None,
            reading: None,
            focus,
            scroll: UniformListScrollHandle::new(),
            width: Rc::new(Cell::new(0.)),
            columns: 1,
            icons_left: Cell::new(ICONS_PER_FRAME),
            icons_owed: Cell::new(false),
            visible: true,
            beside: false,
            lit: false,
            window: window.window_handle(),
            _watch: watch,
        };
        pane.load(cx);
        pane
    }

    /// Where the pane is.
    pub fn location(&self) -> &Location {
        self.history.current()
    }

    /// What the title of the window and of the tab say of the pane.
    pub fn title(&self, cx: &gpui::App) -> String {
        match self.history.current() {
            Location::Computer => Language::of(cx).say("This PC").to_owned(),
            Location::Folder(path) => title_of(path),
        }
    }

    /// Gives the pane the keys.
    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    /// Says whether the pane is in sight, whether another stands beside
    /// it, and whether it is the one in front of the two.
    pub fn place(&mut self, visible: bool, beside: bool, lit: bool, cx: &mut Context<Self>) {
        if (self.visible, self.beside, self.lit) != (visible, beside, lit) {
            (self.visible, self.beside, self.lit) = (visible, beside, lit);
            cx.notify();
        }
    }

    /// Reads the drives, away from the user interface, as a drive may
    /// take its time to answer.
    fn read_drives(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let drives = cx.background_executor().spawn(async { Drive::all() }).await;
            this.update(cx, |this, cx| {
                this.drives = drives;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads the location shown anew, keeping what was chosen.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let Location::Folder(path) = self.history.current().clone() else {
            self.listing = Listing::default();
            self.problem = None;
            self.read_drives(cx);
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
    pub fn go(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
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
        window.focus(&self.focus);
        self.load(cx);
        cx.emit(PaneEvent::Navigated);
    }

    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.back() {
            self.arrived(window, cx);
        }
    }

    pub fn forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.forward() {
            self.arrived(window, cx);
        }
    }

    pub fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.up() {
            self.arrived(window, cx);
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.history.can_go_back()
    }

    pub fn can_go_forward(&self) -> bool {
        self.history.can_go_forward()
    }

    pub fn can_go_up(&self) -> bool {
        self.history.current().up().is_some()
    }

    /// The folder shown, where it is one.
    pub fn folder(&self) -> Option<PathBuf> {
        self.history.current().folder().map(Path::to_owned)
    }

    /// What is chosen, in the order of the list.
    pub fn chosen(&self) -> Vec<PathBuf> {
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
            Launch::new(&entry.path)
                .named(format!("`{}`", entry.name))
                .start();
        }
    }

    /// The chosen entries, in the order of the list.
    fn chosen_entries(&self) -> Vec<Entry> {
        { self.chosen().into_iter() }
            .filter_map(|path| self.listing.position_of(&path))
            .filter_map(|position| self.listing.get(position).cloned())
            .collect()
    }

    /// Opens what is chosen: one folder here, several in windows of their
    /// own, files with their programs.
    fn open_chosen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = self.chosen_entries();
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
    pub fn work(&mut self, work: FileWork, cx: &mut Context<Self>) {
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
    pub fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub fn edit_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Whether a field of the pane has the keys.
    fn typing(&self, window: &Window, cx: &Context<Self>) -> bool {
        let fields = [Some(&self.filter), self.address.as_ref()]
            .into_iter()
            .chain([self.rename.as_ref().map(|rename| &rename.field)]);
        fields
            .flatten()
            .any(|field| field.read(cx).is_focused(window))
    }

    /// Does what the key of `event` asks of the pane, and says whether it
    /// was one for it; the window takes the others.
    pub fn key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = event.keystroke.key.as_str();
        let held = &event.keystroke.modifiers;
        if self.rename.is_some() {
            match key {
                "enter" => self.end_rename(false, window, cx),
                "escape" => self.end_rename(true, window, cx),
                _ => {}
            }
            return true;
        }
        if self.address.is_some() {
            match key {
                "enter" => self.end_address(true, window, cx),
                "escape" => self.end_address(false, window, cx),
                _ => {}
            }
            return true;
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
            return true;
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
            ("f5", false, false, false) | ("r", true, false, false) => self.load(cx),
            ("delete", false, false, erase) => self.delete(erase, cx),
            ("a", true, false, false) => {
                self.selection.all(&self.listing);
                cx.notify();
            }
            ("c", true, false, false) => self.put_on_clipboard(false),
            ("x", true, false, false) => self.put_on_clipboard(true),
            ("v", true, false, false) => self.paste(cx),
            ("n", true, false, true) => self.new_folder(window, cx),
            ("h", true, false, false) => self.toggle_hidden(cx),
            ("l", true, false, false) | ("d", false, true, false) => self.edit_address(window, cx),
            ("f", true, false, false) | ("e", true, false, false) => {
                self.filter.read(cx).focus(window)
            }
            ("1", true, false, true) => self.set_look(Look::Details, cx),
            ("2", true, false, true) => self.set_look(Look::Tiles, cx),
            _ => return false,
        }
        true
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

    pub fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        self.hidden = !self.hidden;
        self.arrange(cx);
    }

    pub fn hidden(&self) -> bool {
        self.hidden
    }

    pub fn look(&self) -> Look {
        self.look
    }

    pub fn set_look(&mut self, look: Look, cx: &mut Context<Self>) {
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

    /// Takes the keys, where a press in the pane gives them to it, and
    /// tells the window so.
    fn take_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        cx.emit(PaneEvent::Focused);
        if self.rename.is_some() {
            self.end_rename(false, window, cx);
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
        self.take_focus(window, cx);
        let held = event.modifiers;
        if event.button == MouseButton::Middle {
            if let Some(entry) = self.listing.get(position).filter(|entry| entry.folder) {
                cx.emit(PaneEvent::OpenInNewTab(Location::Folder(
                    entry.path.clone(),
                )));
            }
            return;
        }
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
        self.take_focus(window, cx);
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
        let entries = self.menu_entries(cx);
        Menus::open(entries, anchor, cx);
    }

    /// The entries of the menu of what is chosen, or of the folder shown
    /// where nothing is.
    fn menu_entries(&self, cx: &mut Context<Self>) -> Vec<MenuEntry> {
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
            return entries;
        }
        entries.push(entry(ICON_OPEN, "Open", |this, cx| {
            this.after_menu(cx, |this, window, cx| this.open_chosen(window, cx))
        }));
        let one_folder = matches!(chosen.as_slice(), [path] if path.is_dir());
        if one_folder {
            entries.push(entry(ICON_NEW_TAB, "Open in new tab", |this, cx| {
                for path in this.chosen() {
                    cx.emit(PaneEvent::OpenInNewTab(Location::Folder(path)));
                }
            }));
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
            this.copy_paths(cx)
        }));
        if self.beside {
            entries.push(entry(ICON_TO_OTHER, "Copy to the other pane", |_, cx| {
                cx.emit(PaneEvent::ToOtherPane { cut: false })
            }));
            entries.push(entry(ICON_TO_OTHER, "Move to the other pane", |_, cx| {
                cx.emit(PaneEvent::ToOtherPane { cut: true })
            }));
        }
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
        entries
    }

    /// Puts the paths of what is chosen on the clipboard as text, a line
    /// each.
    fn copy_paths(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = { self.chosen().into_iter() }
            .map(|path| path.display().to_string())
            .collect();
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(paths.join("\r\n")));
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
pub fn title_of(path: &Path) -> String {
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
pub fn drive_glyph(kind: DriveKind) -> &'static str {
    match kind {
        DriveKind::Fixed => ICON_DRIVE,
        DriveKind::Removable => ICON_USB,
        DriveKind::Network => ICON_NETWORK,
        DriveKind::Optical => ICON_DISC,
    }
}

/// How much of a drive is used, from 0 to 1, where it can be told.
pub fn used_share(total: Option<u64>, free: Option<u64>) -> Option<f32> {
    let (total, free) = (total?, free?);
    (total > 0).then(|| (1. - free as f32 / total as f32).clamp(0., 1.))
}

/// A bar of how full a drive is, red when it is nearly full.
pub fn fill_bar(theme: &Theme, share: Option<f32>, height: f32) -> gpui::Div {
    let color = match share {
        Some(share) if share > 0.9 => gpui::rgb(0xe5534b).into(),
        _ => theme.colors.primary,
    };
    div()
        .h(px(height))
        .w_full()
        .rounded(px(height / 2.))
        .bg(theme.colors.outline.opacity(0.3))
        .children(share.map(|share| {
            div()
                .h_full()
                .w(gpui::relative(share))
                .rounded(px(height / 2.))
                .bg(color)
        }))
}

impl FolderPane {
    /// The steps of the path, each a button to that folder; a click on
    /// what is left of the bar writes the path instead. While the path is
    /// written, its field.
    pub fn render_address(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if let Some(field) = &self.address {
            return div()
                .flex_1()
                .min_w_0()
                .child(field.clone())
                .into_any_element();
        }
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

    /// The field the list is filtered with.
    pub fn filter_field(&self) -> Entity<TextField> {
        self.filter.clone()
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

    /// Has an element of the item at `position` take the presses of both
    /// buttons of the pointer.
    fn pressable(
        element: gpui::Stateful<gpui::Div>,
        position: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let mut element = element;
        for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
            element = element.on_mouse_down(
                button,
                cx.listener(move |this, event, window, cx| {
                    cx.stop_propagation();
                    this.press(position, event, window, cx)
                }),
            );
        }
        element
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
                let size = match entry.folder {
                    true => String::new(),
                    false => size_text(entry.size),
                };
                let row = div()
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
                    .when(entry.hidden, |row| row.opacity(0.6))
                    .when(chosen, |row| row.bg(theme.colors.secondary_container))
                    .when(!chosen, |row| {
                        row.hover(|style| style.bg(theme.colors.surface_container_highest))
                    })
                    .when(cursor, |row| {
                        row.border_1()
                            .border_color(theme.colors.primary.opacity(0.5))
                    });
                Some(
                    Self::pressable(row, position, cx)
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
                        let tile = div()
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
                                tile.hover(|style| style.bg(theme.colors.surface_container_highest))
                            });
                        Some(
                            Self::pressable(tile, position, cx)
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
        let cards = self.drives.iter().enumerate().map(|(index, drive)| {
            let target = Location::Folder(drive.root.clone());
            let share = used_share(drive.total, drive.free);
            let free = match (drive.free, drive.total) {
                (Some(free), Some(total)) => format!(
                    "{} {} {}",
                    size_text(free),
                    language.say("free of"),
                    size_text(total)
                ),
                _ => String::new(),
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
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.take_focus(window, cx)),
                )
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
                        .child(fill_bar(theme, share, 6.))
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

    /// What the status bar says of the pane: how many items it shows,
    /// how many of them are chosen and what they take.
    pub fn status_text(&self, cx: &gpui::App) -> String {
        let language = Language::of(cx);
        if self.reading.is_some() {
            return language.say("Reading…").to_owned();
        }
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
        text
    }

    /// A line in the middle of the pane, where it shows no list.
    fn notice(&self, text: String, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.colors.on_surface_variant)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.press_empty(event, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event, window, cx| this.press_empty(event, window, cx)),
            )
            .child(text)
            .into_any_element()
    }

    fn render_list(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
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
                main.child(self.render_header(theme, cx))
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
}

impl Render for FolderPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        self.icons_left.set(ICONS_PER_FRAME);
        self.icons_owed.set(false);
        let columns = ((self.width.get() - 16.) / TILE_WIDTH) as usize;
        self.columns = columns.max(1);
        let language = Language::of(cx);
        let content = match (self.history.current(), &self.problem) {
            (Location::Computer, _) => self.render_computer(&theme, cx),
            (_, Some(problem)) => {
                let text = format!("{}: {problem}", language.say("This folder cannot be read"));
                self.notice(text, &theme, cx)
            }
            (_, None) if self.listing.is_empty() && self.reading.is_none() => {
                let text = language.say("This folder is empty").to_owned();
                self.notice(text, &theme, cx)
            }
            _ => self.render_list(&theme, cx),
        };
        let width = self.width.clone();
        let entity = cx.entity().downgrade();
        div()
            .id("files-pane")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .rounded(theme.shape.large)
            .border_1()
            .border_color(match self.lit {
                true => theme.colors.primary.opacity(0.6),
                false => gpui::transparent_black(),
            })
            .child(content)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let wide = bounds.size.width / px(1.);
                        if (width.get() - wide).abs() >= 1. {
                            width.set(wide);
                            entity.update(cx, |_, cx| cx.notify()).ok();
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }
}
