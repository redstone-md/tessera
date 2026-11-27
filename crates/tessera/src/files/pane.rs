// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A pane of a window of the file manager: one location with its own way
//! back and forth, what it holds, in which order and which of it is
//! chosen, drawn as rows or tiles of which only those in sight are drawn.
//! A window holds one pane per tab, or two side by side.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Local};
use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, AsyncApp, Context, Entity, EventEmitter,
    FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, ListAlignment, ListOffset,
    ListState, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, Render,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Task, WeakEntity, Window,
    canvas, div, img, list, prelude::FluentBuilder, px,
};
use gpui::{NavigationDirection, ObjectFit, RenderImage, StyledImage, Subscription};

use super::columns::Columns;
use super::model::{
    Arrangement, Column, Entry, GroupLabel, Grouping, History, Line, Listing, Location, Look,
    Order, Selection, Shape, Shown, TypeAhead, crumbs, is_valid_name, read_folder, size_text,
    stamp_of,
};
use super::recycle_view::{self, Bin, BinDeed};
use super::rename::{BatchRename, RenameEvent, apply, parent_key};
use super::search::{FOUND_MOST, Found, Search, SearchState};
use super::session::{FolderView, FolderViews};
use super::thumbs::{FileViewConfig, Picture, Thumbs};
use super::view::DropPlaces;
use super::{Files, FilesConfig};
use crate::field::TextField;
use crate::i18n::Language;
use crate::icons::{AppIcons, Icon, icon_of_pixels};
use crate::menu::{MenuEntry, Menus};
use crate::platform::{DragOut, FileDeed, UndoJournal};
use crate::platform::{
    Drive, DriveKind, ExplorerSettings, FileClipboard, FileWork, IconPixels, Launch, NameRules,
    QuickAccess, disk_space, recent_files, reveal_in_explorer, show_properties, terminal,
};
use crate::platform::{FileProperty, Overlays, PropertyValue, RecycleBin};
use crate::popup::PopupAnchor;
use crate::theme::Theme;
use crate::ui::{icon, one_line};

use super::sidebar::ICON_HOME;

mod header;
mod verbs;

use verbs::ShellChoices;
pub(super) use verbs::within;

/// Measures of a pane, in logical pixels.
const HEADER_HEIGHT: f32 = 30.;
const GROUP_HEIGHT: f32 = 34.;
/// The room between items side by side, and around them.
const GAP: f32 = 8.;
const MARGIN: f32 = 16.;

/// How far the wheel turns, with Ctrl held, for one step of zoom.
const WHEEL_STEP: f32 = 40.;

/// How often the folder shown is looked at for changes, while the window
/// is in front and the pane in sight.
const WATCH_INTERVAL: Duration = Duration::from_millis(1500);

/// How many of the files used lately Home shows.
const RECENT_SHOWN: usize = 20;

/// How far the pointer moves with the button held, in logical pixels,
/// before what is chosen is dragged.
const DRAG_DISTANCE: f32 = 4.;

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
pub const ICON_PIN: &str = "\u{e718}";
pub const ICON_UNPIN: &str = "\u{e77a}";
const ICON_EXPLORER: &str = "\u{ec50}";
const ICON_LINK: &str = "\u{e71b}";
const ICON_SORT_UP: &str = "\u{e70e}";
const ICON_SORT_DOWN: &str = "\u{e70d}";
const ICON_CHECK: &str = "\u{e73e}";
const ICON_NONE: &str = "";
const ICON_SELECT_ALL: &str = "\u{e8b3}";
const ICON_INVERT: &str = "\u{e8b2}";
const ICON_PATTERN: &str = "\u{e8b9}";
const ICON_UNDO: &str = "\u{e7a7}";
const ICON_REDO: &str = "\u{e7a6}";

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
    /// The user asked to pin the folder `path` to Quick access, or with
    /// `pinned` false to unpin it.
    SetPinned { path: PathBuf, pinned: bool },
}

/// The arrow Windows draws over the icon of a shortcut, looked up once.
struct LinkArrow(Icon);

/// The overlays programs draw over the icons of a folder, by the names
/// of its items.
#[derive(Default)]
struct Overlaid {
    marks: HashMap<OsString, usize>,
    pictures: Vec<Arc<RenderImage>>,
}

impl gpui::Global for LinkArrow {}

/// A file or folder being renamed, with the field its new name is
/// written in.
struct Rename {
    path: PathBuf,
    field: Entity<TextField>,
}

/// A press of the left button on an item, which may become a drag.
#[derive(Clone, Copy)]
struct Pressed {
    at: Point<Pixels>,
    position: usize,
    /// Whether the item was chosen among others, which a click without
    /// keys narrows the choice to once the button is let go, so that the
    /// others can be dragged with it.
    narrow: bool,
}

/// One location of a window of the file manager, and what it holds.
pub struct FolderPane {
    history: History,
    listing: Listing,
    selection: Selection,
    order: Order,
    /// The order of Home, the latest first.
    home_order: Order,
    config: FilesConfig,
    /// Whether hidden files are shown, where the user switched it; else
    /// the configuration and the folder options of Explorer say.
    hidden: Option<bool>,
    /// The folder options of Explorer, as read with the folder last.
    explorer: ExplorerSettings,
    look: Look,
    /// The drives, as the computer shows them.
    drives: Vec<Drive>,
    /// The folders of Home the user pinned there, which are unpinned; the
    /// others there are used often.
    pins: HashSet<PathBuf>,
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
    /// How large the items of the look are drawn, as a step of its sizes.
    zoom: usize,
    /// What the list is grouped by.
    grouping: Grouping,
    /// The lines the list is drawn in, and what the list keeps of them.
    lines: Vec<Line>,
    list: ListState,
    /// How many items stand side by side in a line.
    across: usize,
    /// How wide the pane was last drawn, in logical pixels.
    width: Rc<Cell<f32>>,
    /// How far the wheel was turned with Ctrl held, toward a step of zoom.
    wheel: Cell<f32>,
    /// The size of the drive the folder is on and what of it is free.
    space: Option<(u64, u64)>,
    /// The field a pattern to choose by is written in, while it is.
    pattern: Option<Entity<TextField>>,
    /// The search that fills the list, at a location of a search, and
    /// how it went.
    search: Option<Search>,
    searched: Option<SearchState>,
    /// What to choose once the folder was read, as after "Open file
    /// location".
    pending: Option<PathBuf>,
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
    /// The dialog that renames many files at once, while it is open.
    batch: Option<(Entity<BatchRename>, gpui::Subscription)>,
    /// The press of the left button on an item, until it is let go or
    /// becomes a drag.
    pressed: Option<Pressed>,
    /// The scale of the display the window was last drawn on.
    scale: Cell<f32>,
    /// Where the window takes drops, which the pane writes its folder and
    /// the rows of its folders into as it draws.
    zones: DropPlaces,
    /// The folder a drag from another program is over, which is marked.
    drag_over: Option<PathBuf>,
    /// The thumbnails of the window, which the larger looks draw.
    thumbs: Entity<Thumbs>,
    _thumbs: Option<Subscription>,
    /// The overlays programs draw over the icons of the folder shown, and
    /// their reading while it runs.
    overlays: Overlaid,
    _overlays: Option<Task<()>>,
    /// The columns of the details, as chosen for the location, and
    /// whether their widths changed since they were kept.
    columns: Columns,
    columns_changed: bool,
    /// Whether the folder holds files of a cloud folder, whose state a
    /// column then shows.
    cloudy: bool,
    /// The values of the columns of the Shell, by path and property, as
    /// they were read; those being read; and the rows that wanted values
    /// in the frame drawn.
    properties: HashMap<(PathBuf, FileProperty), Option<PropertyValue>>,
    properties_asked: HashSet<(PathBuf, FileProperty)>,
    wanted: RefCell<HashSet<PathBuf>>,
    /// What the Recycle Bin holds, where it is the place shown.
    bin: Bin,
    /// The letters typed over the list to choose by name.
    typed: TypeAhead,
    /// The names of items to choose once the folder was read, as
    /// `select_when_loaded` asks.
    pending_names: Option<Vec<OsString>>,
    /// The language texts are said in, as of the frame drawn.
    language: Language,
    /// Waits for the looks of folders to be read, where looks are kept per
    /// folder and they were not read yet.
    _views: Option<gpui::Subscription>,
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
        // The columns of each folder are kept whatever `per_folder` says.
        let views = (!FolderViews::loaded(cx)).then(|| {
            FolderViews::load(cx);
            cx.observe_global::<FolderViews>(|this: &mut Self, cx| {
                if FolderViews::loaded(cx) {
                    this._views = None;
                    this.take_view(cx);
                    this.arrange(cx);
                    this.relayout(true);
                }
            })
        });
        if !cx.has_global::<LinkArrow>() {
            let arrow = IconPixels::link_arrow().and_then(|pixels| icon_of_pixels(&pixels));
            cx.set_global(LinkArrow(arrow));
        }
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
            home_order: Order {
                column: Column::Modified,
                descending: true,
                folders_first: true,
            },
            config,
            hidden: None,
            explorer: ExplorerSettings::default(),
            look: config.view,
            zoom: config.view.first_zoom(),
            grouping: config.group,
            lines: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            across: 1,
            wheel: Cell::new(0.),
            space: None,
            pattern: None,
            search: None,
            searched: None,
            pending: None,
            drives: Vec::new(),
            pins: HashSet::new(),
            filter,
            address: None,
            rename: None,
            problem: None,
            stamp: None,
            reading: None,
            focus,
            width: Rc::new(Cell::new(0.)),
            icons_left: Cell::new(ICONS_PER_FRAME),
            icons_owed: Cell::new(false),
            visible: true,
            beside: false,
            lit: false,
            window: window.window_handle(),
            batch: None,
            pressed: None,
            scale: Cell::new(window.scale_factor()),
            zones: DropPlaces::default(),
            drag_over: None,
            thumbs: cx.new(|_| Thumbs::new()),
            _thumbs: None,
            overlays: Overlaid::default(),
            _overlays: None,
            columns: Columns::of(&Column::ALL),
            columns_changed: false,
            cloudy: false,
            properties: HashMap::new(),
            properties_asked: HashSet::new(),
            wanted: RefCell::new(HashSet::new()),
            bin: Bin::default(),
            typed: TypeAhead::default(),
            pending_names: None,
            language,
            _views: views,
            _watch: watch,
        };
        pane.take_view(cx);
        pane.load(cx);
        pane
    }

    /// Has the pane write where it takes drops into `zones`, those of the
    /// window it is in.
    pub fn take_drops(&mut self, zones: DropPlaces) {
        self.zones = zones;
    }

    /// Has the pane draw the thumbnails of `thumbs`, those of the window
    /// it is in, and draw again when one comes.
    pub fn take_thumbs(&mut self, thumbs: Entity<Thumbs>, cx: &mut Context<Self>) {
        self._thumbs = Some(cx.observe(&thumbs, |_, _, cx| cx.notify()));
        self.thumbs = thumbs;
    }

    /// Marks the folder `folder` as the one a drag is over, or none.
    pub fn mark_drag_over(&mut self, folder: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.drag_over != folder {
            self.drag_over = folder;
            cx.notify();
        }
    }

    /// Chooses the items called `names` in the folder shown once it was
    /// read, and shows the first; at once where it is read already. This
    /// is how `/select` and the windows of the Shell choose items.
    pub fn select_when_loaded(&mut self, names: Vec<OsString>, cx: &mut Context<Self>) {
        self.pending_names = Some(names);
        if self.reading.is_none() {
            self.take_pending_names(cx);
        }
    }

    /// Chooses the items `select_when_loaded` named, where they are shown.
    fn take_pending_names(&mut self, cx: &mut Context<Self>) {
        let Some(names) = self.pending_names.take() else {
            return;
        };
        let wanted: Vec<String> = { names.iter() }
            .map(|name| name.to_string_lossy().to_lowercase())
            .collect();
        let chosen: Vec<PathBuf> = { self.listing.entries().iter() }
            .filter(|entry| wanted.contains(&entry.name.to_lowercase()))
            .map(|entry| entry.path.clone())
            .collect();
        if chosen.is_empty() {
            return;
        }
        self.selection.clear();
        self.selection.chosen.extend(chosen);
        self.reveal_chosen();
        cx.notify();
    }

    /// Scrolls to the first item chosen, and puts the cursor there.
    fn reveal_chosen(&mut self) {
        let first = (0..self.listing.len()).find(|&position| {
            { self.listing.get(position) }.is_some_and(|entry| self.selection.contains(&entry.path))
        });
        if let Some(position) = first {
            self.selection.cursor = Some(position);
            if let Some(line) = self.line_of(position) {
                self.list.scroll_to_reveal_item(line);
            }
        }
    }

    /// The entry the cursor is on, or else the first chosen, for the
    /// panes beside the list to show.
    pub fn focused_entry(&self) -> Option<Entry> {
        let at_cursor = { self.selection.cursor }
            .and_then(|position| self.listing.get(position))
            .filter(|entry| self.selection.contains(&entry.path));
        at_cursor
            .or_else(|| {
                (0..self.listing.len())
                    .filter_map(|position| self.listing.get(position))
                    .find(|entry| self.selection.contains(&entry.path))
            })
            .cloned()
    }

    /// How many items are chosen.
    pub fn chosen_count(&self) -> usize {
        self.selection.chosen.len()
    }

    /// Forgets the thumbnails of `paths`, whose files changed, so that
    /// they are pictured anew.
    pub fn refresh_pictures(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        self.thumbs
            .update(cx, |thumbs, cx| thumbs.forget(paths, cx));
        self.properties.retain(|(path, _), _| !paths.contains(path));
        cx.notify();
    }

    /// Where the pane is.
    pub fn location(&self) -> &Location {
        self.history.current()
    }

    /// What the title of the window and of the tab say of the pane.
    pub fn title(&self, cx: &gpui::App) -> String {
        match self.history.current() {
            Location::Home => Language::of(cx).say("Quick access").to_owned(),
            Location::Computer => Language::of(cx).say("This PC").to_owned(),
            Location::Folder(path) => title_of(path),
            Location::Search { root, .. } => format!(
                "{} {}",
                Language::of(cx).say("Search results in"),
                title_of(root)
            ),
            Location::RecycleBin => Language::of(cx).say("Recycle Bin").to_owned(),
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
        // A read still running is of the place left: dropping it stops it
        // from filling the place now shown.
        self.reading = None;
        self.search = None;
        self.searched = None;
        self._overlays = None;
        let path = match self.history.current().clone() {
            Location::Folder(path) => path,
            Location::Home => return self.load_home(cx),
            Location::RecycleBin => return self.load_bin(cx),
            Location::Search { root, query } => {
                self.listing = Listing::default();
                self.problem = None;
                self.space = None;
                self.searched = Some(SearchState::Running);
                let (follow, shown) = (self.config.explorer, self.shown());
                self.search = Some(Search::start(root, query, follow, shown, cx));
                self.arrange(cx);
                return;
            }
            Location::Computer => {
                self.listing = Listing::default();
                self.problem = None;
                self.space = None;
                self.relayout(true);
                self.read_drives(cx);
                cx.notify();
                return;
            }
        };
        let follow = self.config.explorer;
        self.reading = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let read = cx
                    .background_executor()
                    .spawn(async move {
                        let stamp = stamp_of(&path);
                        let explorer = ExplorerSettings::read();
                        let mut rules = NameRules::new(explorer, follow);
                        let entries = read_folder(&path, &mut rules);
                        (entries, stamp, explorer, disk_space(&path))
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.reading = None;
                    let (entries, stamp, explorer, space) = read;
                    this.stamp = stamp;
                    this.explorer = explorer;
                    this.space = space;
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
                    this.took_entries(cx);
                    if let Some(pending) = this.pending.take() {
                        this.selection.clear();
                        this.selection.chosen.insert(pending);
                    }
                    this.arrange(cx);
                    this.take_pending_names(cx);
                    if let Some(line) = { this.selection.chosen.iter().next() }
                        .and_then(|path| this.listing.position_of(path))
                        .and_then(|position| this.line_of(position))
                    {
                        this.list.scroll_to_reveal_item(line);
                    }
                })
                .ok();
            }),
        );
    }

    /// Asks the folder just read for the overlays programs draw over the
    /// icons of its items, away from the user interface; those drawn
    /// before stay until the answer comes.
    fn took_entries(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.folder() else {
            return;
        };
        if !FileViewConfig::of(cx).overlays || self.listing.entries().is_empty() {
            return;
        }
        // The arrow of a shortcut is drawn by Tessera itself.
        let names: Vec<OsString> = { self.listing.entries().iter() }
            .filter(|entry| !entry.link)
            .map(|entry| OsString::from(&entry.name))
            .collect();
        let side = (self.look.shape(self.zoom).icon * self.scale.get()) as u32;
        let marks = Overlays::of_folder(folder, names, side);
        self._overlays = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let Ok(marks) = marks.recv().await else {
                    return;
                };
                this.update(cx, |this, cx| {
                    let pictures = { marks.pictures.iter() }
                        .filter_map(icon_of_pixels)
                        .collect::<Vec<_>>();
                    // A picture that cannot be made leaves its marks out.
                    if pictures.len() == marks.pictures.len() {
                        this.overlays = Overlaid {
                            marks: marks.marks,
                            pictures,
                        };
                        cx.notify();
                    }
                })
                .ok();
            }),
        );
    }

    /// Reads what lies in the Recycle Bin, away from the user interface.
    fn load_bin(&mut self, cx: &mut Context<Self>) {
        self.problem = None;
        self.space = None;
        let items = RecycleBin::list();
        self.reading = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let items = items.recv().await.unwrap_or_else(|_| Ok(Vec::new()));
                let read = cx
                    .background_executor()
                    .spawn(async move { items.map(Bin::read) })
                    .await;
                this.update(cx, |this, cx| {
                    this.reading = None;
                    match read {
                        Ok((entries, bin)) => {
                            this.listing = Listing::new(entries);
                            this.bin = bin;
                        }
                        Err(error) => {
                            this.problem = Some(error);
                            this.listing = Listing::default();
                            this.bin = Bin::default();
                        }
                    }
                    this.arrange(cx);
                })
                .ok();
            }),
        );
    }

    /// Does `deed` of the bar of the Recycle Bin, and reads the bin again
    /// once it is done.
    fn bin_deed(&mut self, deed: BinDeed, cx: &mut Context<Self>) {
        let chosen = self.bin.items(&self.chosen());
        let reread = |cx: &mut AsyncApp, this: &WeakEntity<Self>| {
            this.update(cx, |this, cx| this.load(cx)).ok();
        };
        match deed {
            BinDeed::Restore | BinDeed::RestoreAll => {
                let items = match deed {
                    BinDeed::Restore => chosen,
                    _ => self.bin.all(),
                };
                if items.is_empty() {
                    return;
                }
                let done = recycle_view::restore(items);
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    if let Ok(outcome) = done.recv().await {
                        outcome.report();
                    }
                    reread(cx, &this);
                })
                .detach();
            }
            BinDeed::Erase => {
                if chosen.is_empty() {
                    return;
                }
                let done = recycle_view::erase(chosen);
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    if let Ok(Err(error)) = done.recv().await {
                        report!("the items were not deleted: {error}");
                    }
                    reread(cx, &this);
                })
                .detach();
            }
            BinDeed::Empty => {
                let done = recycle_view::empty();
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    if let Ok(Err(error)) = done.recv().await {
                        report!("{error}");
                    }
                    reread(cx, &this);
                })
                .detach();
            }
        }
    }

    /// Takes what a search found.
    pub fn found(&mut self, found: Found, cx: &mut Context<Self>) {
        match found {
            Found::Some(entries) => self.listing.extend(entries),
            Found::Done { indexed, capped } => {
                self.searched = Some(SearchState::Done { indexed, capped });
            }
        }
        self.arrange(cx);
    }

    /// Searches below the folder shown, or the one searched, for what the
    /// filter field holds.
    /// With nothing written, or nowhere to search, the keys go back to
    /// the list.
    fn search_below(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).text().trim().to_owned();
        let root = self.history.current().root().map(Path::to_owned);
        match root {
            Some(root) if !query.is_empty() => {
                self.go(Location::Search { root, query }, window, cx)
            }
            _ => window.focus(&self.focus),
        }
    }

    /// Shows the folder the chosen result of a search is in, with it
    /// chosen.
    fn open_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.chosen().into_iter().next() else {
            return;
        };
        let Some(folder) = path.parent().map(Path::to_owned) else {
            return;
        };
        self.go(Location::Folder(folder), window, cx);
        self.pending = Some(path);
    }

    /// Reads Home: the folders of Quick access, as Explorer keeps them,
    /// and the files used lately, which follow them.
    fn load_home(&mut self, cx: &mut Context<Self>) {
        let follow = self.config.explorer;
        self.reading = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let folders = QuickAccess::folders().await;
                let files = recent_files(RECENT_SHOWN).await;
                let read = cx
                    .background_executor()
                    .spawn(async move {
                        let explorer = ExplorerSettings::read();
                        let mut rules = NameRules::new(explorer, follow);
                        let (folders, problem) = match folders {
                            Ok(folders) => (folders, None),
                            Err(error) => (Vec::new(), Some(format!("{error:#}"))),
                        };
                        let pins: HashSet<PathBuf> = { folders.iter() }
                            .filter(|folder| folder.pinned)
                            .map(|folder| folder.place.path.clone())
                            .collect();
                        let folders = folders.into_iter().map(|folder| folder.place);
                        let entries: Vec<Entry> = { folders.chain(files) }
                            .filter_map(|place| {
                                let metadata = std::fs::metadata(&place.path).ok()?;
                                let mut entry = Entry::of(place.path, &metadata, &mut rules);
                                if entry.folder {
                                    entry.display = place.name;
                                }
                                Some(entry)
                            })
                            .collect();
                        (entries, pins, problem, explorer)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    let (entries, pins, problem, explorer) = read;
                    if let Some(problem) = problem {
                        report!("Quick access cannot be read: {problem}");
                    }
                    this.reading = None;
                    this.problem = None;
                    this.space = None;
                    this.explorer = explorer;
                    this.pins = pins;
                    this.listing = Listing::new(entries);
                    this.arrange(cx);
                })
                .ok();
            }),
        );
    }

    /// The order the list is in: Home has its own, as the files used
    /// lately are ordered by when they were.
    fn order(&self) -> Order {
        match self.history.current() {
            Location::Home => self.home_order,
            _ => self.order,
        }
    }

    /// Orders and filters the list again, keeping the choice and the
    /// cursor on what they were on.
    fn arrange(&mut self, cx: &mut Context<Self>) {
        let cursor = { self.selection.cursor }
            .and_then(|position| self.listing.get(position))
            .map(|entry| entry.path.clone());
        let filter = self.filter.read(cx).text();
        let order = self.order();
        let keys = self.sort_keys(cx);
        self.cloudy = { self.listing.entries().iter() }.any(|entry| entry.cloud.is_some());
        self.listing.arrange(Arrangement {
            order: &order,
            filter: &filter,
            shown: self.shown(),
            grouping: self.grouping(),
            today: Local::now().date_naive(),
            keys: keys.as_ref(),
        });
        self.selection.follow(cursor, &self.listing);
        self.relayout(false);
        cx.notify();
    }

    /// What the list is grouped by: Home is grouped into Quick access and
    /// the files used lately.
    fn grouping(&self) -> Grouping {
        match self.history.current() {
            Location::Home => Grouping::Home,
            _ => self.grouping,
        }
    }

    /// Lays the shown entries out in lines anew, as many side by side as
    /// the width holds, and has the list measure them again where they
    /// changed, or always with `measure`, as after a change of size.
    fn relayout(&mut self, measure: bool) {
        let shape = self.look.shape(self.zoom);
        self.across = match self.look.is_grid() {
            true => (((self.width.get() - MARGIN + GAP) / (shape.width + GAP)) as usize).max(1),
            false => 1,
        };
        let lines = self.listing.lines(self.across);
        if measure || lines != self.lines {
            let old = self.lines.len();
            self.lines = lines;
            self.list.splice(0..old, self.lines.len());
        }
    }

    /// The line the entry at `position` is drawn in.
    fn line_of(&self, position: usize) -> Option<usize> {
        { self.lines.iter() }
            .position(|line| matches!(line, Line::Items(range) if range.contains(&position)))
    }

    /// Takes the look the folder had where looks are kept per folder, or
    /// else the one the configuration gives.
    fn take_view(&mut self, cx: &mut Context<Self>) {
        self.take_columns(cx);
        if !self.config.per_folder {
            return;
        }
        let view = { self.folder() }.and_then(|folder| FolderViews::get(&folder, cx));
        let config = self.config;
        let view = view.unwrap_or(FolderView {
            look: config.view,
            zoom: config.view.first_zoom(),
            column: Column::Name,
            descending: false,
            grouping: config.group,
        });
        self.look = view.look;
        self.zoom = view.zoom;
        self.order.column = view.column;
        self.order.descending = view.descending;
        self.grouping = view.grouping;
    }

    /// Keeps the look of the folder shown, where looks are kept per
    /// folder.
    fn keep_view(&self, cx: &mut Context<Self>) {
        if let (true, Some(folder)) = (self.config.per_folder, self.folder()) {
            let view = FolderView {
                look: self.look,
                zoom: self.zoom,
                column: self.order.column,
                descending: self.order.descending,
                grouping: self.grouping,
            };
            FolderViews::put(&folder, view, cx);
        }
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
        self.forget_properties();
        self.overlays = Overlaid::default();
        self.bin = Bin::default();
        self.pending_names = None;
        self.rename = None;
        self.address = None;
        self.pattern = None;
        self.filter.update(cx, |field, cx| field.clear(cx));
        self.list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
        window.focus(&self.focus);
        self.take_view(cx);
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
        // What lies in the Recycle Bin is brought back before it opens.
        if *self.history.current() == Location::RecycleBin {
            return;
        }
        if entry.folder {
            self.go(Location::Folder(entry.path.clone()), window, cx);
        } else {
            Launch::new(&entry.path)
                .named(format!("`{}`", entry.display))
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
        Self::work_for([cx.weak_entity()], work, cx);
    }

    /// Has the Shell do `work`, and reads the folders of `panes` again
    /// when it is done.
    pub fn work_for(
        panes: impl IntoIterator<Item = WeakEntity<Self>> + 'static,
        work: FileWork,
        cx: &mut gpui::App,
    ) {
        let (done, finished) = async_channel::bounded::<()>(1);
        work.start(move || {
            done.try_send(()).ok();
        });
        cx.spawn(async move |cx: &mut AsyncApp| {
            finished.recv().await.ok();
            for pane in panes {
                pane.update(cx, |pane, cx| pane.load(cx)).ok();
            }
        })
        .detach();
    }

    /// Puts what is chosen into the Recycle Bin, or with `erase` deletes
    /// it for good, after the Shell asks.
    fn delete(&mut self, erase: bool, cx: &mut Context<Self>) {
        if *self.history.current() == Location::RecycleBin {
            return self.bin_deed(BinDeed::Erase, cx);
        }
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

    /// Makes a new folder in the folder shown, as New > Folder of
    /// Windows does, and starts renaming it.
    pub fn new_folder(&mut self, cx: &mut Context<Self>) {
        self.new_item(None, cx);
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
        if chosen.len() > 1 {
            self.begin_batch_rename(chosen, window, cx);
            return;
        }
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

    /// Opens the dialog that renames the files at `paths` at once.
    fn begin_batch_rename(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let renamed: std::collections::HashSet<&PathBuf> = paths.iter().collect();
        let entries = self.listing.entries();
        let taken = { entries.iter() }
            .filter(|entry| !renamed.contains(&entry.path))
            .map(|entry| parent_key(&entry.path, &entry.name))
            .collect();
        let folders = { entries.iter() }
            .filter(|entry| entry.folder && renamed.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect();
        let dialog = cx.new(|cx| BatchRename::new(paths, folders, taken, window, cx));
        let heard = cx.subscribe_in(
            &dialog,
            window,
            |this, _, event: &RenameEvent, window, cx| {
                this.batch = None;
                window.focus(&this.focus);
                if let RenameEvent::Apply(plan) = event {
                    this.batch_rename(plan.clone(), cx);
                }
                cx.notify();
            },
        );
        self.batch = Some((dialog, heard));
        cx.notify();
    }

    /// Renames the files of `plan`, away from the user interface, and
    /// chooses them under their new names once the folder is read again.
    /// The renames done go into the undo journal as one piece of work, so
    /// that one Ctrl+Z takes them all back.
    fn batch_rename(&mut self, plan: Vec<(PathBuf, PathBuf)>, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let renamed: Vec<PathBuf> = plan.iter().map(|(_, new)| new.clone()).collect();
            let applied = cx
                .background_executor()
                .spawn(async move { apply(&plan) })
                .await;
            UndoJournal::note(
                { applied.done.into_iter() }
                    .map(|(from, to)| FileDeed::Renamed { from, to })
                    .collect(),
            );
            for problem in applied.problems {
                report!("a file was not renamed: {problem}");
            }
            this.update(cx, |this, cx| {
                this.selection.clear();
                this.selection.chosen.extend(renamed);
                this.load(cx);
            })
            .ok();
        })
        .detach();
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
        self.selection.clear();
        self.selection.chosen.insert(target);
        let work = FileWork::Rename {
            path: rename.path,
            name: name.to_owned(),
        };
        self.work(work, cx);
    }

    /// Takes the last work on files back, or with `again` does again what
    /// was taken back, and reads the folder when it is done.
    fn undo(&mut self, again: bool, cx: &mut Context<Self>) {
        let done = match again {
            true => UndoJournal::redo(),
            false => UndoJournal::undo(),
        };
        let Some(done) = done else {
            return;
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Ok(outcome) = done.recv().await {
                outcome.report();
            }
            this.update(cx, |this, cx| this.load(cx)).ok();
        })
        .detach();
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
        let fields = [
            Some(&self.filter),
            self.address.as_ref(),
            self.pattern.as_ref(),
        ]
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
        if let Some((dialog, _)) = &self.batch {
            let dialog = dialog.clone();
            match key {
                "enter" => dialog.update(cx, |dialog, cx| dialog.confirm(cx)),
                "escape" => dialog.update(cx, |dialog, cx| dialog.cancel(cx)),
                _ => {}
            }
            return true;
        }
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
        if self.pattern.is_some() {
            match key {
                "enter" => self.end_pattern(true, window, cx),
                "escape" => self.end_pattern(false, window, cx),
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
                "enter" if self.filter.read(cx).is_focused(window) => {
                    // The filter narrows what is shown; Enter searches below.
                    self.search_below(window, cx);
                }
                "enter" | "down" => window.focus(&self.focus),
                _ => {}
            }
            return true;
        }
        let step = self.across as isize;
        let grid = self.look.is_grid();
        let height = self.look.shape(self.zoom).height;
        let page = (window.viewport_size().height / px(height)) as isize * step;
        match (key, held.control, held.alt, held.shift) {
            ("left", false, true, _) | ("backspace", false, false, _) => self.back(window, cx),
            ("right", false, true, _) => self.forward(window, cx),
            ("up", false, true, _) => self.up(window, cx),
            ("up", false, false, extend) => self.step(-step, extend, cx),
            ("down", false, false, extend) => self.step(step, extend, cx),
            ("left", false, false, extend) if grid => self.step(-1, extend, cx),
            ("right", false, false, extend) if grid => self.step(1, extend, cx),
            ("pageup", _, false, extend) => self.step(-page, extend, cx),
            ("pagedown", _, false, extend) => self.step(page, extend, cx),
            ("home", _, false, extend) => self.step(isize::MIN / 2, extend, cx),
            ("end", _, false, extend) => self.step(isize::MAX / 2, extend, cx),
            ("enter", false, false, false) => self.open_chosen(window, cx),
            ("enter", false, true, false) => self.properties(),
            ("f2", ..) => self.rename_chosen(window, cx),
            ("f5", false, false, false) | ("r", true, false, false) => self.load(cx),
            ("delete", false, false, erase) => self.delete(erase, cx),
            ("a", true, false, false) => self.choose_all(cx),
            ("a", true, false, true) => self.invert_choice(cx),
            ("s", true, false, true) => self.begin_pattern(window, cx),
            ("c", true, false, true) => self.copy_paths(true, cx),
            ("c", true, false, false) => self.put_on_clipboard(false),
            ("x", true, false, false) => self.put_on_clipboard(true),
            ("v", true, false, false) => self.paste(cx),
            ("z", true, false, false) => self.undo(false, cx),
            ("y", true, false, false) | ("z", true, false, true) => self.undo(true, cx),
            ("n", true, false, true) => self.new_folder(cx),
            ("h", true, false, false) => self.toggle_hidden(cx),
            ("l", true, false, false) | ("d", false, true, false) => self.edit_address(window, cx),
            ("f", true, false, false) | ("e", true, false, false) => {
                self.filter.read(cx).focus(window)
            }
            // The keys of the looks of Explorer.
            ("1", true, false, true) => self.set_look_zoomed(Look::Icons, 5, cx),
            ("2", true, false, true) => self.set_look_zoomed(Look::Icons, 3, cx),
            ("3", true, false, true) => self.set_look_zoomed(Look::Icons, 1, cx),
            ("4", true, false, true) => self.set_look_zoomed(Look::Icons, 0, cx),
            ("5", true, false, true) => self.set_look(Look::List, cx),
            ("6", true, false, true) => self.set_look(Look::Details, cx),
            ("7", true, false, true) => self.set_look(Look::Tiles, cx),
            ("=" | "+", true, false, _) => self.zoom_by(true, cx),
            ("-", true, false, _) => self.zoom_by(false, cx),
            ("0", true, false, false) => self.set_look(self.look, cx),
            _ => return self.type_ahead(event, cx),
        }
        true
    }

    /// Chooses the next item whose name begins with the letters typed
    /// over the list within a second of each other; says whether the key
    /// was such a letter.
    fn type_ahead(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let held = &event.keystroke.modifiers;
        if held.control || held.alt || held.platform || held.function {
            return false;
        }
        let Some(letters) = { event.keystroke.key_char.as_deref() }
            .filter(|letters| letters.chars().all(|letter| !letter.is_control()))
            .filter(|letters| !letters.is_empty())
        else {
            return false;
        };
        // A space starts nothing; within a word it is part of the name.
        let typed = self.typed.push(letters, Instant::now()).to_owned();
        if typed.trim().is_empty() {
            return false;
        }
        let found = TypeAhead::find(&self.listing, self.selection.cursor, &typed);
        if let Some(position) = found {
            self.selection.click(&self.listing, position, false, false);
            if let Some(line) = self.line_of(position) {
                self.list.scroll_to_reveal_item(line);
            }
            cx.notify();
        }
        true
    }

    fn step(&mut self, by: isize, extend: bool, cx: &mut Context<Self>) {
        self.selection.step(&self.listing, by, extend);
        if let Some(line) = { self.selection.cursor }.and_then(|cursor| self.line_of(cursor)) {
            self.list.scroll_to_reveal_item(line);
        }
        cx.notify();
    }

    pub fn choose_all(&mut self, cx: &mut Context<Self>) {
        self.selection.all(&self.listing);
        cx.notify();
    }

    pub fn invert_choice(&mut self, cx: &mut Context<Self>) {
        self.selection.invert(&self.listing);
        cx.notify();
    }

    /// Opens the field a pattern to choose by is written in.
    pub fn begin_pattern(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = Language::of(cx).say("Patterns to select, as *.jpg; IMG_????.*");
        let field = cx.new(|cx| TextField::new(placeholder.into(), cx).with_glyph(ICON_PATTERN));
        field.read(cx).focus(window);
        self.pattern = Some(field);
        cx.notify();
    }

    /// Adds to the choice what matches the pattern written, or with `keep`
    /// false only closes the field.
    fn end_pattern(&mut self, choose: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.pattern.take() else {
            return;
        };
        window.focus(&self.focus);
        if choose {
            let patterns = field.read(cx).text();
            self.selection.add_matching(&self.listing, &patterns);
        }
        cx.notify();
    }

    pub fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        self.hidden = Some(!self.hidden());
        self.arrange(cx);
    }

    /// Whether hidden files are shown: as the user switched it, or else
    /// where the configuration or the folder options of Explorer show
    /// them.
    pub fn hidden(&self) -> bool {
        self.shown().hidden
    }

    /// Which hidden files are shown.
    fn shown(&self) -> Shown {
        Shown::of(&self.config, self.explorer, self.hidden)
    }

    pub fn look(&self) -> Look {
        self.look
    }

    /// Shows the items in `look`, at the size it starts at.
    pub fn set_look(&mut self, look: Look, cx: &mut Context<Self>) {
        self.set_look_zoomed(look, look.first_zoom(), cx);
    }

    fn set_look_zoomed(&mut self, look: Look, zoom: usize, cx: &mut Context<Self>) {
        // A zoom past the sizes of the look is its largest.
        (self.look, self.zoom) = (look, look.zoomed(zoom, true).min(zoom));
        self.relayout(true);
        self.keep_view(cx);
        cx.notify();
    }

    /// Draws the items a size larger, with `bigger`, or smaller.
    pub fn zoom_by(&mut self, bigger: bool, cx: &mut Context<Self>) {
        let zoom = self.look.zoomed(self.zoom, bigger);
        if zoom != self.zoom {
            self.set_look_zoomed(self.look, zoom, cx);
            if let Some(line) = { self.selection.cursor }.and_then(|cursor| self.line_of(cursor)) {
                self.list.scroll_to_reveal_item(line);
            }
        }
    }

    /// Zooms as the wheel turns with Ctrl held, a step for every notch.
    fn wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if !event.modifiers.control {
            return;
        }
        let turned = self.wheel.get() + event.delta.pixel_delta(px(WHEEL_STEP)).y / px(1.);
        if turned.abs() < WHEEL_STEP {
            self.wheel.set(turned);
            return;
        }
        self.wheel.set(0.);
        self.zoom_by(turned > 0., cx);
    }

    pub fn set_grouping(&mut self, grouping: Grouping, cx: &mut Context<Self>) {
        self.grouping = grouping;
        self.keep_view(cx);
        self.arrange(cx);
    }

    fn sort_by(&mut self, column: Column, cx: &mut Context<Self>) {
        match self.history.current() {
            Location::Home => self.home_order = self.home_order.toggled(column),
            _ => self.order = self.order.toggled(column),
        }
        self.keep_view(cx);
        self.arrange(cx);
    }

    /// The entries of the menu of how the pane shows what it holds: the
    /// looks, the groups and the hidden files.
    pub fn view_menu(&self, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let language = Language::of(cx);
        let pane = cx.entity().downgrade();
        let mark = |lit: bool| if lit { ICON_CHECK } else { ICON_NONE };
        let mut entries: Vec<MenuEntry> = { Look::ALL.into_iter() }
            .map(|look| {
                let pane = pane.clone();
                MenuEntry::new(
                    mark(look == self.look),
                    language.say(look.title()),
                    move |cx| {
                        pane.update(cx, |pane, cx| pane.set_look(look, cx)).ok();
                    },
                )
            })
            .collect();
        entries.extend(Grouping::CHOSEN.into_iter().map(|grouping| {
            let pane = pane.clone();
            let lit = grouping == self.grouping;
            MenuEntry::new(mark(lit), language.say(grouping.title()), move |cx| {
                pane.update(cx, |pane, cx| pane.set_grouping(grouping, cx))
                    .ok();
            })
        }));
        entries.push(MenuEntry::new(
            mark(self.hidden()),
            language.say("Show hidden files"),
            move |cx| {
                pane.update(cx, |pane, cx| pane.toggle_hidden(cx)).ok();
            },
        ));
        entries
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
            self.open_menu(event.position, held.shift, window, cx);
            return;
        }
        if event.click_count >= 2 {
            self.open_at(position, window, cx);
            return;
        }
        let chosen = { self.listing.get(position) }
            .is_some_and(|entry| self.selection.contains(&entry.path));
        let narrow = event.button == MouseButton::Left && chosen && !held.control && !held.shift;
        if !narrow {
            self.selection
                .click(&self.listing, position, held.control, held.shift);
        }
        if event.button == MouseButton::Left {
            self.pressed = Some(Pressed {
                at: event.position,
                position,
                narrow,
            });
        }
        cx.notify();
    }

    /// The left button let go: a press on an item chosen among others
    /// that did not become a drag narrows the choice to it.
    fn release(&mut self, cx: &mut Context<Self>) {
        if let Some(pressed) = self.pressed.take()
            && pressed.narrow
        {
            self.selection
                .click(&self.listing, pressed.position, false, false);
            cx.notify();
        }
    }

    /// Drags what is chosen out, into Explorer, any program or a folder of
    /// Tessera, once the pointer moved far enough with the left button
    /// held on an item; the folder is read again after the drop.
    fn pointer_moved(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pressed) = self.pressed else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.pressed = None;
            return;
        }
        let moved = event.position - pressed.at;
        let distance = (moved.x / px(1.)).hypot(moved.y / px(1.));
        if distance < DRAG_DISTANCE {
            return;
        }
        self.pressed = None;
        let chosen = self.chosen();
        if chosen.is_empty() {
            return;
        }
        let dragged = DragOut::new(chosen).start(window, cx);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Err(error) = dragged.await {
                report!("the files were not dragged: {error}");
            }
            this.update(cx, |this, cx| this.load(cx)).ok();
        })
        .detach();
    }

    /// A press on the list where it holds nothing.
    fn press_empty(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.take_focus(window, cx);
        self.selection.clear();
        cx.notify();
        if event.button == MouseButton::Right {
            self.open_menu(event.position, event.modifiers.shift, window, cx);
        }
    }

    /// Opens the menu of what is chosen, or of the folder shown where
    /// nothing is, under `position`: Tessera's entries, then what the
    /// Shell adds, with the verbs shown with Shift where `extended`.
    fn open_menu(
        &mut self,
        position: Point<Pixels>,
        extended: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let anchor = match PopupAnchor::under(window, position, cx) {
            Ok(anchor) => anchor,
            Err(error) => {
                report!("the menu has no place: {error:#}");
                return;
            }
        };
        let entries = self.menu_entries(cx);
        let (chosen, folder) = (self.chosen(), self.folder());
        let kinds: Vec<(PathBuf, bool)> = { self.chosen_entries().into_iter() }
            .map(|entry| (entry.path, entry.folder))
            .collect();
        let open_with = verbs::one_kind(&kinds);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let choices = ShellChoices::read(chosen, open_with, folder, extended, cx).await;
            this.update(cx, |this, cx| {
                let mut entries = entries;
                entries.extend(this.shell_entries(choices, anchor, cx));
                Menus::open(entries, anchor, cx);
            })
            .ok();
        })
        .detach();
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
        if *self.history.current() == Location::RecycleBin {
            let deeds = match chosen.is_empty() {
                true => [BinDeed::RestoreAll, BinDeed::Empty],
                false => [BinDeed::Restore, BinDeed::Erase],
            };
            for deed in deeds {
                let view = view.clone();
                entries.push(MenuEntry::new(
                    ICON_NONE,
                    language.say(deed.title()),
                    move |cx| {
                        view.update(cx, |this, cx| this.bin_deed(deed, cx)).ok();
                    },
                ));
            }
            return entries;
        }
        if chosen.is_empty() {
            if self.folder().is_some() {
                entries.push(entry(ICON_NEW_FOLDER, "New folder", |this, cx| {
                    this.new_folder(cx)
                }));
                if FileClipboard::take().is_some() {
                    entries.push(entry(ICON_PASTE, "Paste", |this, cx| this.paste(cx)));
                }
                // Undo and redo say what they take back, as Explorer's do.
                let undo = UndoJournal::undo_label().map(|label| (ICON_UNDO, label, false));
                let redo = UndoJournal::redo_label().map(|label| (ICON_REDO, label, true));
                for (glyph, label, again) in undo.into_iter().chain(redo) {
                    let view = view.clone();
                    entries.push(MenuEntry::new(glyph, language.say(label), move |cx| {
                        view.update(cx, |this, cx| this.undo(again, cx)).ok();
                    }));
                }
                entries.push(entry(ICON_TERMINAL, "Open in terminal", |this, cx| {
                    if let Some(folder) = this.folder() {
                        open_terminal(folder, cx);
                    }
                }));
            }
            entries.push(entry(ICON_SELECT_ALL, "Select all", |this, cx| {
                this.choose_all(cx)
            }));
            entries.push(entry(ICON_PATTERN, "Select by pattern", |this, cx| {
                this.after_menu(cx, |this, window, cx| this.begin_pattern(window, cx))
            }));
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
        if matches!(
            self.history.current(),
            Location::Search { .. } | Location::Home
        ) {
            entries.push(entry(ICON_FOLDER, "Open file location", |this, cx| {
                this.after_menu(cx, |this, window, cx| this.open_location(window, cx))
            }));
        }
        let one_folder = matches!(chosen.as_slice(), [path] if path.is_dir());
        if one_folder {
            entries.push(entry(ICON_NEW_TAB, "Open in new tab", |this, cx| {
                for path in this.chosen() {
                    cx.emit(PaneEvent::OpenInNewTab(Location::Folder(path)));
                }
            }));
            // Only a folder pinned to Home unpins; one used often there
            // pins, as elsewhere.
            let pinned = *self.history.current() == Location::Home
                && chosen.first().is_some_and(|path| self.pins.contains(path));
            let (glyph, label) = match pinned {
                true => (ICON_UNPIN, "Unpin from Quick access"),
                false => (ICON_PIN, "Pin to Quick access"),
            };
            let pane = view.clone();
            entries.push(MenuEntry::new(glyph, language.say(label), move |cx| {
                pane.update(cx, |this, cx| {
                    if let Some(path) = this.chosen().into_iter().next() {
                        cx.emit(PaneEvent::SetPinned {
                            path,
                            pinned: !pinned,
                        });
                    }
                })
                .ok();
            }));
            entries.push(entry(ICON_NEW_WINDOW, "Open in new window", |this, cx| {
                for path in this.chosen() {
                    Files::open(Location::Folder(path), cx);
                }
            }));
            entries.push(entry(ICON_TERMINAL, "Open in terminal", |this, cx| {
                if let Some(path) = this.chosen().into_iter().next() {
                    open_terminal(path, cx);
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
            this.copy_paths(false, cx)
        }));
        entries.push(entry(ICON_INVERT, "Invert selection", |this, cx| {
            this.invert_choice(cx)
        }));
        if self.beside {
            entries.push(entry(ICON_TO_OTHER, "Copy to the other pane", |_, cx| {
                cx.emit(PaneEvent::ToOtherPane { cut: false })
            }));
            entries.push(entry(ICON_TO_OTHER, "Move to the other pane", |_, cx| {
                cx.emit(PaneEvent::ToOtherPane { cut: true })
            }));
        }
        let rename = match chosen.len() {
            1 => "Rename",
            _ => "Rename all",
        };
        entries.push(entry(ICON_RENAME, rename, |this, cx| {
            this.after_menu(cx, |this, window, cx| this.rename_chosen(window, cx))
        }));
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
    /// each, and with `quoted` each in quotes, as "Copy as path" of
    /// Explorer does.
    fn copy_paths(&mut self, quoted: bool, cx: &mut Context<Self>) {
        let paths: Vec<String> = { self.chosen().into_iter() }
            .map(|path| match quoted {
                true => format!("\"{}\"", path.display()),
                false => path.display().to_string(),
            })
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
        if *self.history.current() == Location::Home {
            let home = div()
                .flex()
                .items_center()
                .gap(theme.spacing.tiny)
                .px(theme.spacing.small)
                .child(icon(theme, ICON_HOME))
                .child(language.say("Quick access"));
            steps.push(home.into_any_element());
        } else {
            steps.push(root.into_any_element());
        }
        if let Some(path) = self.history.current().root() {
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
            .children(match self.history.current() {
                Location::Search { query, .. } => Some(
                    div()
                        .flex_none()
                        .px(theme.spacing.small)
                        .text_color(theme.colors.primary)
                        .child(format!(
                            "\u{203a}  {} “{query}”",
                            language.say("Search results")
                        )),
                ),
                _ => None,
            })
            .child(div().flex_1().h_full())
            .on_click(cx.listener(|this, _, window, cx| this.edit_address(window, cx)))
            .into_any_element()
    }

    /// The field the list is filtered with.
    pub fn filter_field(&self) -> Entity<TextField> {
        self.filter.clone()
    }

    /// The picture of `entry`, `size` wide: its thumbnail where the look
    /// is large enough and the Shell has one, else the icon of its kind,
    /// with the overlay a program draws over it; `priority` is the place
    /// of its line among those in sight.
    fn entry_icon(
        &self,
        entry: &Entry,
        size: Pixels,
        priority: u32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let side = size / px(1.);
        let scale = self.scale.get();
        let thumbnail = match self.history.current() {
            // What lies in the bin is pictured by its icon only.
            Location::RecycleBin => Picture::Icon,
            _ => { self.thumbs.clone() }.update(cx, |thumbs, cx| {
                thumbs.picture(entry, side, scale, priority, cx)
            }),
        };
        let picture = match thumbnail {
            Picture::Drawn(image) => img(image)
                .size(size)
                .flex_none()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            Picture::Icon => self.kind_icon(entry, size, theme, cx),
        };
        let overlay = { self.overlays.marks.get(std::ffi::OsStr::new(&entry.name)) }
            .and_then(|index| self.overlays.pictures.get(*index).cloned());
        match overlay {
            Some(overlay) => div()
                .relative()
                .size(size)
                .flex_none()
                .child(picture)
                .child(img(overlay).absolute().left_0().top_0().size(size))
                .into_any_element(),
            None => picture,
        }
    }

    /// The icon of the kind of `entry`, `size` wide: the folder of the
    /// icon font for a folder, the one the Shell draws for a file; a blank
    /// one until it was looked up.
    fn kind_icon(
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
        let picture = match image {
            Some(image) => img(image).size(size).flex_none().into_any_element(),
            None => div()
                .size(size)
                .flex_none()
                .rounded(theme.shape.small)
                .bg(theme.colors.outline.opacity(0.3))
                .into_any_element(),
        };
        let arrow = { cx.try_global::<LinkArrow>() }.and_then(|arrow| arrow.0.clone());
        match (entry.link, arrow) {
            (true, Some(arrow)) => div()
                .relative()
                .size(size)
                .flex_none()
                .child(picture)
                .child(img(arrow).absolute().left_0().bottom_0().size(size * 0.5))
                .into_any_element(),
            _ => picture,
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
                .child(one_line(entry.display.clone()))
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

    /// The heading of a group, with how many it holds.
    fn render_heading(
        &self,
        label: &GroupLabel,
        count: usize,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let language = Language::of(cx);
        let label: SharedString = match label {
            GroupLabel::Said(text) => language.say(text).into(),
            GroupLabel::Text(text) => text.clone().into(),
        };
        div()
            .h(px(GROUP_HEIGHT))
            .mx(theme.spacing.medium)
            .flex()
            .items_end()
            .gap(theme.spacing.small)
            .pb(px(4.))
            .border_b_1()
            .border_color(theme.colors.outline.opacity(0.25))
            .text_size(theme.typography.label_size)
            .child(div().text_color(theme.colors.primary).child(label))
            .child(
                div()
                    .text_size(theme.typography.label_size * 0.85)
                    .text_color(theme.colors.on_surface_variant)
                    .child(format!("({count})")),
            )
            .into_any_element()
    }

    /// The box of the item at `position`, lit as it is chosen, which
    /// takes the presses of the pointer.
    fn item_box(
        &self,
        position: usize,
        entry: &Entry,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let chosen = self.selection.contains(&entry.path);
        let cursor = self.selection.cursor == Some(position);
        let over = entry.folder && self.drag_over.as_ref() == Some(&entry.path);
        let item = div()
            .id(("files-item", position))
            .flex_none()
            .rounded(theme.shape.small)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            .border_1()
            .border_color(match (over, cursor) {
                (true, _) => theme.colors.primary,
                (false, true) => theme.colors.primary.opacity(0.5),
                (false, false) => gpui::transparent_black(),
            })
            .when(over, |item| item.bg(theme.colors.primary.opacity(0.15)))
            .when(entry.hidden, |item| item.opacity(0.6))
            .when(chosen, |item| item.bg(theme.colors.secondary_container))
            .when(!chosen, |item| {
                item.hover(|style| style.bg(theme.colors.surface_container_highest))
            })
            .when(entry.folder, |item| {
                item.relative().child(self.drop_zone(entry.path.clone()))
            });
        Self::pressable(item, position, cx)
    }

    /// An element over what holds it, which writes it down as a place
    /// that takes drops into `folder` where it is drawn.
    fn drop_zone(&self, folder: PathBuf) -> impl IntoElement + use<> {
        self.zones.zone(folder)
    }

    /// The name of `entry` under its icon, or the field it is renamed in.
    fn entry_label(&self, entry: &Entry) -> AnyElement {
        match &self.rename {
            Some(rename) if rename.path == entry.path => div()
                .w_full()
                .child(rename.field.clone())
                .into_any_element(),
            _ => div()
                .w_full()
                .flex()
                .justify_center()
                .child(one_line(entry.display.clone()))
                .into_any_element(),
        }
    }

    /// The item at `position` as `shape` of the look draws it.
    fn render_item(
        &self,
        position: usize,
        shape: Shape,
        priority: u32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let entry = self.listing.get(position)?.clone();
        let item = self.item_box(position, &entry, theme, cx);
        let picture = self.entry_icon(&entry, px(shape.icon), priority, theme, cx);
        let dim = theme.colors.on_surface_variant;
        let small = theme.typography.label_size * 0.85;
        let item = match self.look {
            Look::Details => {
                let columns = self.shown_columns();
                let cells: Vec<AnyElement> = { columns.shown().iter().skip(1) }
                    .map(|shown| {
                        Self::cell(*shown, theme)
                            .text_color(dim)
                            .child(self.cell_content(&entry, shown.column, theme))
                            .into_any_element()
                    })
                    .collect();
                item.h(px(shape.height))
                    .mx(theme.spacing.small)
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .px(theme.spacing.small)
                    .child(picture)
                    .child(self.entry_name(&entry))
                    .children(cells)
            }
            Look::List => item
                .w(px(shape.width))
                .h(px(shape.height))
                .flex()
                .items_center()
                .gap(theme.spacing.small)
                .px(theme.spacing.small)
                .child(picture)
                .child(self.entry_name(&entry)),
            Look::Tiles => {
                let details = match entry.folder {
                    true => self.cell_text(&entry, Column::Kind),
                    false => format!("{}  ·  {}", entry.kind(), size_text(entry.size)),
                };
                item.w(px(shape.width))
                    .h(px(shape.height))
                    .flex()
                    .items_center()
                    .gap(theme.spacing.medium)
                    .px(theme.spacing.small)
                    .child(picture)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(self.entry_name(&entry))
                            .child(
                                div()
                                    .text_size(small)
                                    .text_color(dim)
                                    .child(one_line(details)),
                            ),
                    )
            }
            Look::Icons => item
                .w(px(shape.width))
                .h(px(shape.height))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(theme.spacing.tiny)
                .p(theme.spacing.tiny)
                .child(picture)
                .child(self.entry_label(&entry)),
        };
        Some(item.into_any_element())
    }

    /// The line `index` of the list: a heading of a group, or the items
    /// that stand side by side.
    fn render_line(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let shape = self.look.shape(self.zoom);
        self.scale.set(window.scale_factor());
        // The rows nearest the top of what is in sight are pictured first.
        let top = self.list.logical_scroll_top().item_ix;
        let priority = index.saturating_sub(top) as u32;
        let element = match self.lines.get(index).cloned() {
            Some(Line::Heading { label, count }) => self.render_heading(&label, count, &theme, cx),
            Some(Line::Items(range)) if self.look == Look::Details => {
                let item = self.render_item(range.start, shape, priority, &theme, cx);
                item.unwrap_or_else(|| div().into_any_element())
            }
            Some(Line::Items(range)) => {
                let items: Vec<AnyElement> = range
                    .filter_map(|position| self.render_item(position, shape, priority, &theme, cx))
                    .collect();
                div()
                    .px(px(MARGIN / 2.))
                    .py(px(GAP / 4.))
                    .flex()
                    .gap(px(GAP))
                    .children(items)
                    .into_any_element()
            }
            None => div().into_any_element(),
        };
        if self.icons_owed.get() {
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        let wanted = self
            .wanted
            .try_borrow()
            .is_ok_and(|wanted| !wanted.is_empty());
        if wanted && self.reads_properties() {
            cx.on_next_frame(window, |this, _, cx| this.read_wanted(cx));
        }
        element
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
        match self.searched {
            Some(SearchState::Running) => {
                text = format!("{}  ·  {text}", language.say("Searching…"));
            }
            Some(SearchState::Done { indexed, capped }) => {
                let how = match indexed {
                    true => language.say("found by the Windows Search index"),
                    false => language.say("found by reading the folders"),
                };
                text.push_str(&format!("  ·  {how}"));
                if capped {
                    let most = language.say("the first shown");
                    text.push_str(&format!("  ·  {most} {FOUND_MOST}"));
                }
            }
            None => {}
        }
        if chosen > 0 {
            let bytes = self.listing.size_of(&self.selection.chosen);
            text.push_str(&format!("  ·  {chosen} {}", language.say("selected")));
            if bytes > 0 {
                text.push_str(&format!("  ·  {}", size_text(bytes)));
            }
        }
        text
    }

    /// The size of the drive the folder shown is on and what of it is
    /// free, where it was read.
    pub fn space(&self) -> Option<(u64, u64)> {
        self.space
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

    /// The bar of the Recycle Bin over its list, where it is shown.
    fn render_bin_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if *self.history.current() != Location::RecycleBin {
            return None;
        }
        let pane = cx.entity().downgrade();
        let pressed = move |deed: BinDeed, _: &mut Window, cx: &mut App| {
            pane.update(cx, |pane, cx| pane.bin_deed(deed, cx)).ok();
        };
        let chosen = !self.selection.chosen.is_empty();
        Some(recycle_view::render_bar(
            theme,
            Language::of(cx),
            chosen,
            self.bin.is_empty(),
            pressed,
        ))
    }

    /// The bar over the list in which a pattern to choose by is written.
    fn render_pattern(&self, theme: &Theme) -> Option<AnyElement> {
        let field = self.pattern.clone()?;
        Some(
            div()
                .flex_none()
                .p(theme.spacing.small)
                .border_b_1()
                .border_color(theme.colors.outline.opacity(0.25))
                .child(field)
                .into_any_element(),
        )
    }

    fn render_list(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity();
        let lines = list(self.list.clone(), move |index, window, cx| {
            this.update(cx, |this, cx| this.render_line(index, window, cx))
        })
        .flex_1()
        .py(theme.spacing.tiny);
        div()
            .id("files-main")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .when(self.look == Look::Details, |main| {
                main.child(self.render_header(theme, cx))
            })
            .child(lines)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.press_empty(event, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event, window, cx| this.press_empty(event, window, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.release(cx)),
            )
            .on_mouse_move(
                cx.listener(|this, event, window, cx| this.pointer_moved(event, window, cx)),
            )
            .into_any_element()
    }
}

impl Render for FolderPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        self.icons_left.set(ICONS_PER_FRAME);
        self.icons_owed.set(false);
        let language = Language::of(cx);
        self.language = language;
        let content = match (self.history.current(), &self.problem) {
            (Location::Computer, _) => self.render_computer(&theme, cx),
            (_, Some(problem)) => {
                let text = format!("{}: {problem}", language.say("This folder cannot be read"));
                self.notice(text, &theme, cx)
            }
            (Location::Search { .. }, None)
                if self.listing.is_empty() && self.searched != Some(SearchState::Running) =>
            {
                let text = language.say("Nothing was found").to_owned();
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
            .on_scroll_wheel(cx.listener(|this, event, _, cx| this.wheel(event, cx)))
            // The buttons of the pointer that go back and forth, as in a
            // browser.
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, window, cx| this.back(window, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, window, cx| this.forward(window, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.columns_changed {
                        this.keep_columns(cx);
                    }
                }),
            )
            // The folder shown takes drops beneath the rows of its folders.
            .children(self.folder().map(|folder| self.drop_zone(folder)))
            .children(self.render_bin_bar(&theme, cx))
            .children(self.render_pattern(&theme))
            .child(content)
            .children(self.batch.as_ref().map(|(dialog, _)| dialog.clone()))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let wide = bounds.size.width / px(1.);
                        if (width.get() - wide).abs() >= 1. {
                            width.set(wide);
                            entity
                                .update(cx, |this, cx| {
                                    this.relayout(false);
                                    cx.notify();
                                })
                                .ok();
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }
}

/// Opens a terminal in `folder`; which terminal there is is read from
/// the disk, away from the user interface.
fn open_terminal(folder: PathBuf, cx: &App) {
    { cx.background_executor() }
        .spawn(async move { terminal(Some(&folder), None, false).start() })
        .detach();
}
