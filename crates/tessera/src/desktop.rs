// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The icons of the desktop, drawn by Tessera: the files and folders of
//! the desktop folders and the items of the Shell Windows shows there,
//! arranged on a grid of each display, chosen, opened, renamed, dragged
//! and dropped, with the menus of an icon and of the desktop.
//!
//! One window per display draws its icons ([`view`]); what they show and
//! which of them are chosen is one [`DesktopModel`] for all displays, as
//! Explorer's desktop is one list over all of them. Where Tessera is the
//! shell, the windows are there on every display whatever the
//! configuration says, so that a click on the desktop always lands on
//! Tessera; with the icons hidden they draw nothing and only take the
//! clicks. Beside Explorer they are there only when `[desktop] icons` is
//! on, and Explorer's own icons are hidden meanwhile.
//!
//! Work on files goes through the Shell into the undo journal of the
//! process ([`UndoJournal`]) that the file manager uses too, so Ctrl+Z on
//! the desktop takes back what was done in a window of the file manager.

pub mod grid;
pub mod items;
mod menu;
mod view;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    App, AppContext, AsyncApp, Bounds, Context, Entity, Global, Task, WeakEntity, Window,
    WindowHandle, px, size,
};
use serde::Deserialize;

use crate::actions::Actions;
use crate::field::TextField;
use crate::files::Files;
use crate::i18n::Language;
use crate::icons::{Icon, icon_of_pixels};
use crate::panel::rewrite;
use crate::platform::{
    DesktopFolders, DesktopLayer, ExplorerIcons, FileClipboard, FileDeed, FileOperation, FileWork,
    FolderWatch, IconPixels, Launch, LayerKind, LinkTarget, NativeSurface, NewTemplate, Place,
    RecycleBin, ScreenRect, ShellThread, SmallBirth, UndoJournal, VirtualKind, display_frames,
    layer_frame, shell_mode, show_properties,
};
use crate::popup::floating_window;

use grid::{Arrangement, Grid, Positions, Spot, display_set_key, lay_out, ordered_displays};
use items::{DesktopItem, ItemKind, SortBy, read_desktop, rename_text, renamed, sorted_keys};
use view::DesktopIconsView;

/// The file the places of the icons are kept in, in the directory of the
/// configuration.
const POSITIONS_FILE: &str = "desktop-icons.toml";

/// How long changes of the desktop folders are let settle before they are
/// read, as a copy brings many at once.
const SETTLE: Duration = Duration::from_millis(250);

/// How often the arrangement of the displays, Explorer's icons and the
/// Recycle Bin are looked at.
const LOOK_AGAIN: Duration = Duration::from_secs(2);

/// How long typed letters add up to one name to find.
const TYPING_PAUSE: Duration = Duration::from_millis(1000);

/// The action Alt+F4 on the desktop runs.
const POWER_DIALOG: &str = "power-dialog";

/// How large the icons are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl IconSize {
    pub const ALL: [Self; 3] = [Self::Large, Self::Medium, Self::Small];

    pub fn name(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }

    /// What the menu calls it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "Small icons",
            Self::Medium => "Medium icons",
            Self::Large => "Large icons",
        }
    }

    /// The side of the icon, in logical pixels.
    pub fn icon(self) -> f32 {
        match self {
            Self::Small => 32.,
            Self::Medium => 48.,
            Self::Large => 96.,
        }
    }

    /// The width and height of the cell an icon stands in, with its name
    /// of two lines below it, in logical pixels.
    pub fn cell(self) -> (f32, f32) {
        match self {
            Self::Small => (78., 70.),
            Self::Medium => (86., 90.),
            Self::Large => (124., 140.),
        }
    }

    /// The size a step larger, or smaller; itself at either end.
    pub fn step(self, larger: bool) -> Self {
        let sizes = [Self::Small, Self::Medium, Self::Large];
        let at = sizes.iter().position(|size| *size == self).unwrap_or(1);
        let to = match larger {
            true => (at + 1).min(sizes.len() - 1),
            false => at.saturating_sub(1),
        };
        sizes[to]
    }
}

/// What `[desktop]` of the configuration says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DesktopConfig {
    /// Whether Tessera draws the icons of the desktop; by default where it
    /// is the shell, and not beside Explorer, which draws its own.
    pub icons: Option<bool>,
    pub size: IconSize,
    pub arrange: Arrangement,
    pub sort: SortBy,
    /// Whether the icons are shown ("Show desktop icons").
    pub show: bool,
    /// How the window of the icons is made part of the desktop.
    pub layer: LayerKind,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            icons: None,
            size: IconSize::default(),
            arrange: Arrangement::default(),
            sort: SortBy::default(),
            show: true,
            layer: LayerKind::default(),
        }
    }
}

impl DesktopConfig {
    /// Whether Tessera draws the icons.
    pub fn draws_icons(&self) -> bool {
        self.icons.unwrap_or_else(shell_mode)
    }

    /// Whether the windows of the icons are there: always where Tessera
    /// is the shell, so that the desktop is Tessera's, and beside Explorer
    /// only while Tessera draws the icons.
    fn has_windows(&self) -> bool {
        shell_mode() || self.draws_icons()
    }

    /// Whether the icons are drawn in the windows.
    fn shows_icons(&self) -> bool {
        self.draws_icons() && self.show
    }
}

/// The icons of the desktop: the windows that draw them and what they
/// draw.
#[derive(Default)]
pub struct Desktop {
    config: DesktopConfig,
    model: Option<Entity<DesktopModel>>,
    views: Vec<WindowHandle<DesktopIconsView>>,
    /// The arrangement of displays the windows were opened for.
    displays: String,
    /// Whether Explorer's icons were hidden.
    hid_explorer: bool,
}

impl Global for Desktop {}

impl Desktop {
    /// Opens the windows of the icons as `config` says, and keeps them
    /// with the displays from then on.
    pub fn start(config: &DesktopConfig, cx: &mut App) {
        cx.set_global(Self::default());
        Self::configure(config, cx);
        Self::answer_close_requests(cx);
        Self::look_again(cx);
        cx.on_app_quit(|_| async { ExplorerIcons::restore() })
            .detach();
    }

    /// Applies `config`: opens or closes the windows, and draws the icons
    /// as it says.
    pub fn configure(config: &DesktopConfig, cx: &mut App) {
        let before = cx.global::<Self>().config;
        cx.global_mut::<Self>().config = *config;
        let model = cx.global::<Self>().model.clone();
        if !config.has_windows() {
            Self::close(cx);
            return;
        }
        let model = match model {
            Some(model) => {
                model.update(cx, |model, cx| model.configure(*config, cx));
                model
            }
            None => {
                let model = cx.new(|cx| DesktopModel::new(*config, cx));
                cx.global_mut::<Self>().model = Some(model.clone());
                model
            }
        };
        if before.layer != config.layer || cx.global::<Self>().views.is_empty() {
            Self::open_views(&model, cx);
        }
        Self::keep_explorer(cx);
    }

    /// Closes the windows of the icons and lets go of what they showed.
    fn close(cx: &mut App) {
        let this = cx.global_mut::<Self>();
        let views = std::mem::take(&mut this.views);
        this.model = None;
        this.displays.clear();
        for view in views {
            view.update(cx, |view, window, _| view.close(window)).ok();
        }
        Self::keep_explorer(cx);
    }

    /// Hides Explorer's icons while Tessera draws its own beside it, and
    /// shows them again once it does not.
    fn keep_explorer(cx: &mut App) {
        let this = cx.global::<Self>();
        let drawing = !shell_mode() && this.config.draws_icons() && !this.views.is_empty();
        let hid = this.hid_explorer;
        if drawing {
            // Explorer shows them again when it starts again.
            ExplorerIcons::hide();
        } else if hid {
            ExplorerIcons::restore();
        }
        cx.global_mut::<Self>().hid_explorer = drawing;
    }

    /// Opens a window of the icons on every display, in place of those
    /// that were open.
    fn open_views(model: &Entity<DesktopModel>, cx: &mut App) {
        for view in std::mem::take(&mut cx.global_mut::<Self>().views) {
            view.update(cx, |view, window, _| view.close(window)).ok();
        }
        let frames = ordered_displays(display_frames());
        let key = display_set_key(&frames);
        model.update(cx, |model, cx| model.set_displays(frames.clone(), &key, cx));
        cx.global_mut::<Self>().displays = key;
        let kind = cx.global::<Self>().config.layer;
        for display in cx.displays() {
            let rough = Bounds {
                origin: display.bounds().origin,
                size: size(px(64.), px(64.)),
            };
            let options = floating_window(rough, Some(display.id()), false);
            let _small = SmallBirth::begin();
            let frames = frames.clone();
            let model = model.clone();
            let opened = cx.open_window(options, |window, cx| {
                let placed = NativeSurface::of(window).and_then(|native| {
                    let whole = native.display_frame()?;
                    let room = native.display().work_area().unwrap_or(whole);
                    let frame = layer_frame(room, whole);
                    let layer = native.adopt_as_desktop_layer(kind, frame)?;
                    let index = frames.iter().position(|frame| *frame == whole);
                    anyhow::Ok((layer, index.unwrap_or(0), frame))
                });
                let (layer, index, frame) = match placed {
                    Ok((layer, index, frame)) => (Some(layer), index, Some(frame)),
                    Err(error) => {
                        report!("the icons of a display are not on the desktop: {error:#}");
                        (None, 0, None)
                    }
                };
                cx.new(|cx| DesktopIconsView::new(model, index, layer, frame, window, cx))
            });
            match opened {
                Ok(view) => cx.global_mut::<Self>().views.push(view),
                Err(error) => report!("the icons of a display did not open: {error:#}"),
            }
        }
    }

    /// Opens the dialog that asks how to end the session whenever Alt+F4
    /// is pressed on the desktop.
    fn answer_close_requests(cx: &mut App) {
        let requests = DesktopLayer::close_requests();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while requests.recv().await.is_ok() {
                if cx
                    .update(|cx| Actions::run(POWER_DIALOG, None, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Looks every little while whether the displays were arranged
    /// otherwise, which needs new windows, whether Explorer showed its
    /// icons again, and whether the Recycle Bin filled or emptied.
    fn look_again(cx: &mut App) {
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(LOOK_AGAIN).await;
                let frames = { cx.background_executor() }
                    .spawn(async { display_set_key(&ordered_displays(display_frames())) })
                    .await;
                let looked = cx.update(|cx| {
                    let this = cx.global::<Self>();
                    let model = this.model.clone();
                    if let Some(model) = &model
                        && this.displays != frames
                    {
                        Self::open_views(model, cx);
                    }
                    Self::keep_explorer(cx);
                    model.map(|model| model.downgrade())
                });
                let Ok(model) = looked else {
                    break;
                };
                if let Some(model) = model {
                    DesktopModel::look_at_bin(model, cx).await;
                }
            }
        })
        .detach();
    }
}

/// The icon of `item`, `size` pixels high, as far as it was fetched: the
/// one the Shell draws for it, the Recycle Bin full or empty. It is the
/// one place the icons of the desktop are asked for, so that an icon pack
/// can be asked first.
pub fn icon_of(item: &DesktopItem, size: u32, cx: &App) -> Icon {
    let icons = cx.try_global::<DesktopIcons>()?;
    if icons.size != size {
        return None;
    }
    if item.virtual_kind() == Some(VirtualKind::RecycleBin) {
        return icons.bin[usize::from(icons.bin_full)].clone();
    }
    icons
        .by_key
        .get(&item.key)
        .filter(|(modified, _)| *modified == item.modified)
        .and_then(|(_, icon)| icon.clone())
}

/// The icons fetched for the desktop, at one size.
#[derive(Default)]
struct DesktopIcons {
    /// Their side, in physical pixels.
    size: u32,
    /// By key, with the time the file was changed when it was fetched.
    by_key: HashMap<String, (Option<SystemTime>, Icon)>,
    /// The Recycle Bin empty and full.
    bin: [Icon; 2],
    bin_full: bool,
    /// The arrow drawn over a shortcut.
    arrow: Icon,
}

impl Global for DesktopIcons {}

/// What the icons the Shell gave look like, before they are made ready to
/// draw on the thread of GPUI.
struct FetchedIcons {
    size: u32,
    files: Vec<(String, Option<SystemTime>, Option<IconPixels>)>,
    bin: [Option<IconPixels>; 2],
    arrow: Option<IconPixels>,
}

/// One display as its window sees it: where its room is, in physical
/// pixels, how many of them make a logical one, and its grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Room {
    pub frame: ScreenRect,
    pub scale: f32,
    pub grid: Grid,
}

impl Room {
    /// The spot at the screen point `at`, in cells of `cell` logical
    /// pixels, where `at` lies in the room.
    fn spot_at(&self, at: (i32, i32), cell: (f32, f32), display: usize) -> Option<Spot> {
        let inside = (self.frame.left..self.frame.right).contains(&at.0)
            && (self.frame.top..self.frame.bottom).contains(&at.1);
        inside.then(|| Spot {
            column: (at.0 - self.frame.left) as f32 / self.scale / cell.0 - 0.5,
            row: (at.1 - self.frame.top) as f32 / self.scale / cell.1 - 0.5,
            display,
        })
    }
}

/// A file or folder of the desktop being renamed, with the field its new
/// name is written in.
pub struct Rename {
    pub key: String,
    pub field: Entity<TextField>,
}

/// What the desktop shows on all displays, where, and what of it is
/// chosen.
pub struct DesktopModel {
    config: DesktopConfig,
    folders: Option<DesktopFolders>,
    items: Vec<DesktopItem>,
    /// The keys of the items, sorted.
    order: Vec<String>,
    /// Where each icon stands now.
    spots: HashMap<String, Spot>,
    positions: Positions,
    /// The key of the arrangement of displays, and the displays in order.
    set: String,
    displays: Vec<ScreenRect>,
    primary: usize,
    /// What each window told of its display, by the number of the display.
    rooms: Vec<Option<Room>>,
    chosen: HashSet<String>,
    /// The icon the keys move from, and the one a range is chosen from.
    cursor: Option<String>,
    anchor: Option<String>,
    pub rename: Option<Rename>,
    /// What was typed to find an icon, and when last.
    typed: (String, Instant),
    /// Where the next new icons go: where something was dropped or made.
    new_from: Option<Spot>,
    /// Whether the places were read from the disk.
    positions_read: bool,
    /// The largest scale of a display, for the size icons are fetched at.
    scale: f32,
    reading: Option<Task<()>>,
    _watch: Option<(FolderWatch, Task<()>)>,
}

impl DesktopModel {
    fn new(config: DesktopConfig, cx: &mut Context<Self>) -> Self {
        let folders = DesktopFolders::find();
        let mut model = Self {
            config,
            folders,
            items: Vec::new(),
            order: Vec::new(),
            spots: HashMap::new(),
            positions: Positions::default(),
            set: String::new(),
            displays: Vec::new(),
            primary: 0,
            rooms: Vec::new(),
            chosen: HashSet::new(),
            cursor: None,
            anchor: None,
            rename: None,
            typed: (String::new(), Instant::now()),
            new_from: None,
            positions_read: false,
            scale: 1.,
            reading: None,
            _watch: None,
        };
        model.read_positions(cx);
        model.watch(cx);
        model.reload(cx);
        model
    }

    /// Takes `config` over: what is shown, how large and in which order.
    fn configure(&mut self, config: DesktopConfig, cx: &mut Context<Self>) {
        let before = self.config;
        self.config = config;
        if before.size != config.size {
            self.fetch_icons(true, cx);
        }
        if before.arrange == Arrangement::Auto && config.arrange != Arrangement::Auto {
            // The icons stay where auto arrangement put them.
            self.keep_places(cx);
        }
        self.order = sorted_keys(&self.items, config.sort);
        self.relayout(cx);
    }

    pub fn config(&self) -> DesktopConfig {
        self.config
    }

    /// The folder new things go into: the user's desktop folder.
    pub fn folder(&self) -> Option<PathBuf> {
        self.folders.as_ref().map(|folders| folders.user.clone())
    }

    pub fn items(&self) -> &[DesktopItem] {
        &self.items
    }

    pub fn item(&self, key: &str) -> Option<&DesktopItem> {
        self.items.iter().find(|item| item.key == key)
    }

    pub fn spot(&self, key: &str) -> Option<Spot> {
        self.spots.get(key).copied()
    }

    pub fn is_chosen(&self, key: &str) -> bool {
        self.chosen.contains(key)
    }

    /// Whether the icons are drawn at all.
    pub fn shows_icons(&self) -> bool {
        self.config.shows_icons()
    }

    /// The keys chosen, in the order of the icons.
    pub fn chosen(&self) -> Vec<String> {
        { self.order.iter() }
            .filter(|key| self.chosen.contains(*key))
            .cloned()
            .collect()
    }

    /// The files and folders chosen, in the order of the icons.
    pub fn chosen_paths(&self) -> Vec<PathBuf> {
        let chosen = self.chosen();
        { chosen.iter() }
            .filter_map(|key| self.item(key)?.path().map(Path::to_owned))
            .collect()
    }

    /// The size icons are fetched at, in physical pixels.
    fn icon_pixels(&self) -> u32 {
        (self.config.size.icon() * self.scale)
            .round()
            .clamp(16., 256.) as u32
    }

    /// Takes the displays in their order, and the places kept for them.
    fn set_displays(&mut self, displays: Vec<ScreenRect>, set: &str, cx: &mut Context<Self>) {
        self.primary = { displays.iter() }
            .position(|frame| {
                frame.left <= 0 && 0 < frame.right && frame.top <= 0 && 0 < frame.bottom
            })
            .unwrap_or(0);
        self.rooms = vec![None; displays.len()];
        self.displays = displays;
        self.set = set.to_owned();
        self.relayout(cx);
    }

    /// What the window of display `display` tells of it.
    pub fn set_room(&mut self, display: usize, room: Room, cx: &mut Context<Self>) {
        if self.rooms.get(display) == Some(&Some(room)) {
            return;
        }
        if let Some(slot) = self.rooms.get_mut(display) {
            *slot = Some(room);
        }
        if room.scale > self.scale {
            self.scale = room.scale;
            self.fetch_icons(true, cx);
        }
        self.relayout(cx);
    }

    /// Whether every display told its room.
    fn rooms_known(&self) -> bool {
        !self.rooms.is_empty() && self.rooms.iter().all(Option::is_some)
    }

    /// Places the icons anew, and keeps the places where every display
    /// is known.
    fn relayout(&mut self, cx: &mut Context<Self>) {
        let grids: Vec<Grid> = { self.rooms.iter() }
            .map(|room| {
                room.map_or(
                    Grid {
                        columns: 1,
                        rows: 1,
                    },
                    |room| room.grid,
                )
            })
            .collect();
        let kept = self.positions.of_set(&self.set);
        self.spots = lay_out(
            &self.order,
            &kept,
            self.config.arrange,
            &grids,
            self.primary,
            self.new_from,
        );
        if self.rooms_known() && self.positions_read {
            self.new_from = None;
            if self.config.arrange != Arrangement::Auto {
                self.keep_places(cx);
            }
        }
        cx.notify();
    }

    /// Keeps where the icons stand now for the arrangement of displays,
    /// and writes it away from the user interface.
    fn keep_places(&mut self, cx: &mut Context<Self>) {
        if !self.positions_read || self.set.is_empty() || self.items.is_empty() {
            return;
        }
        if !self.positions.keep(&self.set, &self.spots) {
            return;
        }
        let Some(path) = positions_path() else {
            return;
        };
        let positions = self.positions.clone();
        { cx.background_executor() }
            .spawn(async move {
                if let Err(error) = positions.save(&path) {
                    report!("the places of the icons are not written: {error}");
                }
            })
            .detach();
    }

    fn read_positions(&mut self, cx: &mut Context<Self>) {
        let Some(path) = positions_path() else {
            self.positions_read = true;
            return;
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = { cx.background_executor() }
                .spawn(async move { Positions::load(&path) })
                .await;
            this.update(cx, |this, cx| {
                this.positions = read;
                this.positions_read = true;
                this.relayout(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Reads the desktop folders again whenever they change.
    fn watch(&mut self, cx: &mut Context<Self>) {
        let Some(folders) = &self.folders else {
            return;
        };
        let (watch, changes) = match FolderWatch::start(&folders.all()) {
            Ok(started) => started,
            Err(error) => {
                report!("the desktop is not read again when it changes: {error:#}");
                return;
            }
        };
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while changes.recv().await.is_ok() {
                cx.background_executor().timer(SETTLE).await;
                // What changed meanwhile is read with it.
                while changes.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    break;
                }
            }
        });
        self._watch = Some((watch, task));
    }

    /// Reads what the desktop shows, on a thread of the Shell.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(folders) = self.folders.clone() else {
            return;
        };
        let read = ShellThread::call_alone("desktop", move || read_desktop(&folders));
        self.reading = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let Ok(items) = read.recv().await else {
                    return;
                };
                this.update(cx, |this, cx| this.take_items(items, cx)).ok();
            }),
        );
    }

    /// Shows `items` in place of what was shown.
    fn take_items(&mut self, items: Vec<DesktopItem>, cx: &mut Context<Self>) {
        let keys: HashSet<&String> = items.iter().map(|item| &item.key).collect();
        self.chosen.retain(|key| keys.contains(key));
        if self.cursor.as_ref().is_some_and(|key| !keys.contains(key)) {
            self.cursor = None;
        }
        self.items = items;
        self.order = sorted_keys(&self.items, self.config.sort);
        self.relayout(cx);
        self.fetch_icons(false, cx);
    }

    /// Fetches the icons of the items that have none at the size they are
    /// drawn at, or with `anew` those of all, on a thread of the Shell.
    fn fetch_icons(&mut self, anew: bool, cx: &mut Context<Self>) {
        let size = self.icon_pixels();
        let have = cx.default_global::<DesktopIcons>();
        if have.size != size {
            *have = DesktopIcons {
                size,
                ..DesktopIcons::default()
            };
        }
        let have = cx.global::<DesktopIcons>();
        let missing: Vec<(String, PathBuf, Option<SystemTime>)> = { self.items.iter() }
            .filter(|item| {
                anew || have
                    .by_key
                    .get(&item.key)
                    .is_none_or(|(modified, _)| *modified != item.modified)
            })
            .filter_map(|item| {
                let path = match &item.kind {
                    ItemKind::File { path, .. } => path.clone(),
                    ItemKind::Virtual(VirtualKind::RecycleBin) => return None,
                    ItemKind::Virtual(kind) => PathBuf::from(kind.parsing_name()),
                };
                Some((item.key.clone(), path, item.modified))
            })
            .collect();
        let bin = anew || have.bin.iter().all(Option::is_none);
        let arrow = have.arrow.is_none();
        if missing.is_empty() && !bin && !arrow {
            return;
        }
        let fetched = ShellThread::call_alone("desktop icons", move || FetchedIcons {
            size,
            files: { missing.into_iter() }
                .map(|(key, path, modified)| (key, modified, IconPixels::of_item(&path, size)))
                .collect(),
            bin: match bin {
                true => [false, true].map(|full| IconPixels::recycle_bin(full, size)),
                false => [None, None],
            },
            arrow: arrow.then(IconPixels::link_arrow).flatten(),
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let Ok(fetched) = fetched.recv().await else {
                return;
            };
            cx.update(|cx| {
                let icons = cx.default_global::<DesktopIcons>();
                if icons.size != fetched.size {
                    return;
                }
                for (key, modified, pixels) in fetched.files {
                    let icon = pixels.as_ref().and_then(icon_of_pixels);
                    icons.by_key.insert(key, (modified, icon));
                }
                for (slot, pixels) in icons.bin.iter_mut().zip(fetched.bin) {
                    if let Some(pixels) = pixels {
                        *slot = icon_of_pixels(&pixels);
                    }
                }
                if let Some(arrow) = fetched.arrow {
                    icons.arrow = icon_of_pixels(&arrow);
                }
            })
            .ok();
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Looks whether the Recycle Bin, where it is shown, is full or empty,
    /// away from the user interface.
    async fn look_at_bin(this: WeakEntity<Self>, cx: &mut AsyncApp) {
        let shown = this.read_with(cx, |this, _| {
            { this.items.iter() }.any(|item| item.virtual_kind() == Some(VirtualKind::RecycleBin))
        });
        if !shown.unwrap_or(false) {
            return;
        }
        let usage = cx
            .background_executor()
            .spawn(async { RecycleBin::usage() })
            .await;
        let full = usage.is_some_and(|usage| usage.items > 0);
        let changed = cx.update(|cx| {
            let icons = cx.default_global::<DesktopIcons>();
            let changed = icons.bin_full != full;
            icons.bin_full = full;
            changed
        });
        if changed.unwrap_or(false) {
            this.update(cx, |_, cx| cx.notify()).ok();
        }
    }

    /// The arrow drawn over the icon of a shortcut.
    pub fn arrow(cx: &App) -> Icon {
        cx.try_global::<DesktopIcons>()
            .and_then(|icons| icons.arrow.clone())
    }

    /// The spot at the screen point `at`, on the display whose room holds
    /// it.
    pub fn spot_at(&self, at: (i32, i32)) -> Option<Spot> {
        let cell = self.config.size.cell();
        { self.rooms.iter().enumerate() }
            .find_map(|(display, room)| room.as_ref()?.spot_at(at, cell, display))
    }

    /// Has the next new icons go to `spot`.
    pub fn place_new_at(&mut self, spot: Option<Spot>) {
        self.new_from = spot;
    }

    // What is chosen.

    /// A click on the icon `key`: it alone is chosen, or with `toggle` it
    /// is added or taken away, or with `range` the icons from the one
    /// chosen before to it are.
    pub fn click(&mut self, key: &str, toggle: bool, range: bool, cx: &mut Context<Self>) {
        if range && let Some(anchor) = self.anchor.clone() {
            let at = |key: &str| self.order.iter().position(|other| other == key);
            if let (Some(from), Some(to)) = (at(&anchor), at(key)) {
                let (from, to) = (from.min(to), from.max(to));
                if !toggle {
                    self.chosen.clear();
                }
                self.chosen.extend(self.order[from..=to].iter().cloned());
            }
        } else if toggle {
            if !self.chosen.remove(key) {
                self.chosen.insert(key.to_owned());
            }
            self.anchor = Some(key.to_owned());
        } else {
            self.chosen.clear();
            self.chosen.insert(key.to_owned());
            self.anchor = Some(key.to_owned());
        }
        self.cursor = Some(key.to_owned());
        cx.notify();
    }

    /// Chooses `keys`, with `adding` beside what was chosen before.
    pub fn choose(
        &mut self,
        keys: impl IntoIterator<Item = String>,
        adding: bool,
        cx: &mut Context<Self>,
    ) {
        if !adding {
            self.chosen.clear();
        }
        self.chosen.extend(keys);
        cx.notify();
    }

    pub fn choose_all(&mut self, cx: &mut Context<Self>) {
        self.chosen = self.order.iter().cloned().collect();
        cx.notify();
    }

    pub fn clear_choice(&mut self, cx: &mut Context<Self>) {
        if !self.chosen.is_empty() {
            self.chosen.clear();
            cx.notify();
        }
    }

    /// Moves the choice by an arrow key, or with `extend` adds to it.
    pub fn step(&mut self, toward: grid::Toward, extend: bool, cx: &mut Context<Self>) {
        let from = { self.cursor.as_ref() }.and_then(|key| self.spots.get(key).copied());
        let Some(from) = from else {
            if let Some(first) = self.order.first().cloned() {
                self.click(&first, false, false, cx);
            }
            return;
        };
        let next = grid::neighbour(&self.spots, from, toward).cloned();
        if let Some(next) = next {
            match extend {
                true => {
                    self.chosen.insert(next.clone());
                    self.cursor = Some(next);
                    cx.notify();
                }
                false => self.click(&next, false, false, cx),
            }
        }
    }

    /// Typing on the desktop: chooses the next icon whose name begins with
    /// what was typed in a row.
    pub fn type_letter(&mut self, letter: &str, cx: &mut Context<Self>) {
        let (typed, when) = &mut self.typed;
        if when.elapsed() > TYPING_PAUSE {
            typed.clear();
        }
        *when = Instant::now();
        // The same letter again goes on to the next icon it begins.
        let repeated = typed.as_str() == letter;
        if !repeated {
            typed.push_str(letter);
        }
        let after = repeated.then(|| self.cursor.clone()).flatten();
        let found = items::typed_match(&self.items, &self.order, &self.typed.0, after.as_deref());
        if let Some(key) = found {
            self.click(&key, false, false, cx);
        }
    }

    // Moving icons.

    /// Puts the icons `keys` `by` cells further, on `display` where they
    /// move to another one, as a drag of them ends. With auto arrangement
    /// they stay in their order.
    pub fn shift(
        &mut self,
        keys: &[String],
        by: (f32, f32),
        display: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if self.config.arrange == Arrangement::Auto || !self.rooms_known() {
            return;
        }
        // The others keep their cells; the moved ones go where they were
        // dropped, or to the free cell nearest.
        let mut kept = self.spots.clone();
        for key in keys {
            let Some(spot) = self.spots.get(key) else {
                continue;
            };
            let moved = Spot {
                column: spot.column + by.0,
                row: spot.row + by.1,
                display: display.unwrap_or(spot.display),
            };
            kept.insert(key.clone(), moved);
        }
        // Those moved win their cells over those that stood there.
        let mut order: Vec<String> = keys.to_vec();
        order.extend(self.order.iter().filter(|key| !keys.contains(key)).cloned());
        let grids: Vec<Grid> = { self.rooms.iter() }
            .map(|room| {
                room.map_or(
                    Grid {
                        columns: 1,
                        rows: 1,
                    },
                    |room| room.grid,
                )
            })
            .collect();
        self.spots = lay_out(
            &order,
            &kept,
            self.config.arrange,
            &grids,
            self.primary,
            None,
        );
        self.keep_places(cx);
        cx.notify();
    }

    /// The icon at `spot`, but for those of `besides`.
    pub fn item_at(&self, spot: Spot, besides: &[String]) -> Option<&DesktopItem> {
        let (column, row) = spot.nearest_cell();
        let key = { self.spots.iter() }
            .filter(|(key, _)| !besides.contains(key))
            .find(|(_, at)| at.display == spot.display && at.nearest_cell() == (column, row))
            .map(|(key, _)| key)?;
        self.item(key)
    }

    // What is done with the icons.

    /// Opens what is chosen.
    pub fn open_chosen(&self, cx: &mut App) {
        for key in self.chosen() {
            if let Some(item) = self.item(&key) {
                open_item(item, cx);
            }
        }
    }

    /// Shows the properties of what is chosen.
    pub fn properties(&self) {
        let chosen = self.chosen();
        let Some(item) = chosen.first().and_then(|key| self.item(key)) else {
            return;
        };
        match (item.path(), item.virtual_kind()) {
            (Some(path), _) => {
                if let Err(error) = show_properties(path) {
                    report!("{error:#}");
                }
            }
            (None, Some(kind)) => Launch::new(shell_name(kind)).verb("properties").start(),
            (None, None) => {}
        }
    }

    /// Puts what is chosen on the clipboard, which Explorer shares.
    pub fn put_on_clipboard(&self, cut: bool) {
        let paths = self.chosen_paths();
        if paths.is_empty() {
            return;
        }
        if let Err(error) = FileClipboard::put(&paths, cut) {
            report!("the files are not on the clipboard: {error:#}");
        }
    }

    /// Copies or moves what is on the clipboard onto the desktop, or with
    /// `link` makes shortcuts to it there.
    pub fn paste(&mut self, link: bool, cx: &mut Context<Self>) {
        let (Some(folder), Some((from, cut))) = (self.folder(), FileClipboard::take()) else {
            return;
        };
        if link {
            let operation = { from.into_iter() }.fold(FileOperation::new(), |operation, path| {
                operation.link(LinkTarget::Path(path), &folder)
            });
            self.run(operation, cx);
            return;
        }
        let work = match cut {
            true => FileWork::Move { from, to: folder },
            false => FileWork::Copy { from, to: folder },
        };
        self.work(work, cx);
    }

    /// Whether there are files on the clipboard to paste.
    pub fn can_paste() -> bool {
        FileClipboard::take().is_some()
    }

    /// Puts what is chosen into the Recycle Bin, or with `erase` deletes
    /// it for good, after the Shell asks.
    pub fn delete(&mut self, erase: bool, cx: &mut Context<Self>) {
        let paths = self.chosen_paths();
        if paths.is_empty() {
            return;
        }
        let work = match erase {
            true => FileWork::Erase(paths),
            false => FileWork::Recycle(paths),
        };
        self.work(work, cx);
    }

    /// Moves or copies the icons `keys` into the folder of the icon
    /// `onto`, or into the Recycle Bin, as a drag of them onto it does.
    pub fn drop_onto(&mut self, onto: &str, keys: &[String], copy: bool, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = { keys.iter() }
            .filter_map(|key| self.item(key)?.path().map(Path::to_owned))
            .collect();
        let Some(target) = self.item(onto).cloned() else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        let work = match (&target.kind, copy) {
            (ItemKind::Virtual(VirtualKind::RecycleBin), _) => FileWork::Recycle(paths),
            (
                ItemKind::File {
                    path, folder: true, ..
                },
                false,
            ) => FileWork::Move {
                from: paths,
                to: path.clone(),
            },
            (
                ItemKind::File {
                    path, folder: true, ..
                },
                true,
            ) => FileWork::Copy {
                from: paths,
                to: path.clone(),
            },
            _ => return,
        };
        self.work(work, cx);
    }

    /// Makes shortcuts to what is chosen on the desktop.
    pub fn make_shortcuts(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.folder() else {
            return;
        };
        let chosen = self.chosen();
        let kinds: Vec<VirtualKind> = { chosen.iter() }
            .filter_map(|key| self.item(key)?.virtual_kind())
            .collect();
        let paths = self.chosen_paths();
        let ids = ShellThread::call(move || {
            kinds
                .into_iter()
                .filter_map(VirtualKind::id)
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let ids = ids.recv().await.unwrap_or_default();
            let targets = { paths.into_iter().map(LinkTarget::Path) }
                .chain(ids.into_iter().map(LinkTarget::Item));
            let operation = targets.fold(FileOperation::new(), |operation, target| {
                operation.link(target, &folder)
            });
            this.update(cx, |this, cx| this.run(operation, cx)).ok();
        })
        .detach();
    }

    /// Has the Shell do `work`, and reads the desktop when it is done.
    fn work(&mut self, work: FileWork, cx: &mut Context<Self>) {
        self.run(work.operation(), cx);
    }

    /// Has the Shell do `operation`, and reads the desktop when it is done.
    fn run(&mut self, operation: FileOperation, cx: &mut Context<Self>) {
        if operation.is_empty() {
            return;
        }
        let done = operation.start();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Ok(outcome) = done.recv().await {
                outcome.report();
            }
            this.update(cx, |this, cx| this.reload(cx)).ok();
        })
        .detach();
    }

    /// Takes the last work on files back, or with `again` does it again.
    pub fn undo(&mut self, again: bool, cx: &mut Context<Self>) {
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
            this.update(cx, |this, cx| this.reload(cx)).ok();
        })
        .detach();
    }

    /// Makes a new item of the kind `template`, or a folder where it is
    /// none, on the desktop at `at`, away from the user interface, and
    /// starts renaming it in `window`, as New does in Explorer. It goes
    /// into the undo journal where it can be made again.
    pub fn new_item(
        &mut self,
        template: Option<NewTemplate>,
        at: Option<Spot>,
        window: gpui::AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        let Some(folder) = self.folder() else {
            return;
        };
        let new = Language::of(cx).say("New");
        self.new_from = at;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let made = { cx.background_executor() }.spawn(async move {
                let template = match template {
                    Some(template) => template,
                    None => NewTemplate::all()
                        .into_iter()
                        .find(NewTemplate::is_folder)
                        .ok_or_else(|| anyhow::anyhow!("Windows offers no new folder"))?,
                };
                let path = template.create(&folder, new)?;
                anyhow::Ok((path, template))
            });
            let (path, template) = match made.await {
                Ok(made) => made,
                Err(error) => {
                    report!("nothing new was made: {error:#}");
                    return;
                }
            };
            if let Some(made) = template.made() {
                UndoJournal::note(vec![FileDeed::Created {
                    path: path.clone(),
                    made,
                }]);
            }
            let Some(key) = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                return;
            };
            let renamed_here = !template.made_by_command();
            this.update(cx, |this, cx| {
                this.chosen = HashSet::from([key.clone()]);
                this.cursor = Some(key.clone());
                this.reload(cx);
            })
            .ok();
            if !renamed_here {
                return;
            }
            // Renamed once it is read and shown.
            for _ in 0..20 {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let shown = this.read_with(cx, |this, _| this.item(&key).is_some());
                if shown.unwrap_or(true) {
                    break;
                }
            }
            window
                .update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| this.begin_rename(&key, window, cx))
                        .ok();
                })
                .ok();
        })
        .detach();
    }

    /// Puts the icons in the order `sort` gives, column after column,
    /// once, as Sort by does in Explorer: the places kept for this
    /// arrangement of displays are let go.
    pub fn arrange_by(&mut self, sort: SortBy, cx: &mut Context<Self>) {
        self.config.sort = sort;
        self.order = sorted_keys(&self.items, sort);
        self.positions.keep(&self.set, &HashMap::new());
        self.new_from = None;
        self.relayout(cx);
    }

    // Renaming.

    /// Starts renaming the file or folder chosen, where one is.
    pub fn rename_chosen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = self.chosen();
        if let [key] = chosen.as_slice() {
            let key = key.clone();
            self.begin_rename(&key, window, cx);
        }
    }

    /// Starts renaming the icon `key` in its place.
    pub fn begin_rename(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.item(key) else {
            return;
        };
        if item.path().is_none() {
            return;
        }
        let text = rename_text(&item.key, &item.name);
        let stem = !item.is_folder() && text == item.key;
        let field = cx.new(|cx| {
            let mut field = TextField::new(Default::default(), cx);
            field.set_text(&text, stem, cx);
            field
        });
        field.read(cx).focus(window);
        self.rename = Some(Rename {
            key: key.to_owned(),
            field,
        });
        cx.notify();
    }

    /// Gives the file being renamed the name in its field; with `keep`,
    /// the one it had. Its icon keeps its place under the new name.
    pub fn end_rename(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        cx.notify();
        if keep {
            return;
        }
        let Some(item) = self.item(&rename.key).cloned() else {
            return;
        };
        let Some(path) = item.path().map(Path::to_owned) else {
            return;
        };
        let typed = rename.field.read(cx).text();
        let name = renamed(&item.key, &item.name, &typed);
        if name.is_empty() || name == item.key {
            return;
        }
        if !is_valid_name(&name) {
            report!("`{name}` cannot be the name of a file");
            return;
        }
        if let Some(spot) = self.spots.get(&item.key).copied() {
            let mut kept = self.positions.of_set(&self.set);
            kept.insert(name.clone(), spot);
            self.positions.keep(&self.set, &kept);
        }
        self.chosen = HashSet::from([name.clone()]);
        self.cursor = Some(name.clone());
        self.work(FileWork::Rename { path, name }, cx);
    }

    // The look of the icons, written into the configuration.

    pub fn set_size(size: IconSize, cx: &mut App) {
        write_desktop_key(cx, "size", size.name().into());
    }

    pub fn set_arrangement(arrangement: Arrangement, cx: &mut App) {
        write_desktop_key(cx, "arrange", arrangement.name().into());
    }

    pub fn set_sort(sort: SortBy, cx: &mut App) {
        write_desktop_key(cx, "sort", sort.name().into());
    }

    pub fn set_shown(shown: bool, cx: &mut App) {
        write_desktop_key(cx, "show", shown.into());
    }
}

/// Writes `key` of `[desktop]` of the configuration, which the shell
/// takes over at once.
fn write_desktop_key(cx: &mut App, key: &str, value: toml_edit::Value) {
    rewrite(cx, |file| file.set_in("desktop", key, Some(value)));
}

/// Where the places of the icons are kept.
fn positions_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|directory| directory.join(POSITIONS_FILE))
}

/// The name of the item of the Shell `kind` as a start takes it.
fn shell_name(kind: VirtualKind) -> String {
    format!("shell:{}", kind.parsing_name())
}

/// Opens `item`: a folder in the file manager where the configuration
/// gives folders to it, anything else as a double click in Explorer does.
pub fn open_item(item: &DesktopItem, cx: &mut App) {
    match &item.kind {
        ItemKind::File {
            path, folder: true, ..
        } => {
            if !Files::open_folder(path, cx) {
                Launch::new(path).named(format!("`{}`", item.name)).start();
            }
        }
        ItemKind::File { path, .. } => Launch::new(path).named(format!("`{}`", item.name)).start(),
        ItemKind::Virtual(VirtualKind::UserFiles) => match Place::Home.path() {
            Some(home) if Files::open_folder(&home, cx) => {}
            _ => Launch::new(shell_name(VirtualKind::UserFiles)).start(),
        },
        ItemKind::Virtual(VirtualKind::RecycleBin) => {
            if let Err(error) = RecycleBin::open() {
                report!("the Recycle Bin does not open: {error}");
            }
        }
        ItemKind::Virtual(kind) => Launch::new(shell_name(*kind))
            .named(format!("`{}`", item.name))
            .start(),
    }
}

/// Whether `name` can be the name of a file: something, and none of the
/// characters Windows keeps out of names.
fn is_valid_name(name: &str) -> bool {
    !name.trim().is_empty()
        && !name.ends_with('.')
        && !name
            .chars()
            .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_is_read_from_the_configuration() {
        let config: DesktopConfig = toml::from_str(
            "icons = true\nsize = \"large\"\narrange = \"free\"\nsort = \"date\"\nshow = false\nlayer = \"window\"",
        )
        .unwrap();
        assert_eq!(config.icons, Some(true));
        assert_eq!(config.size, IconSize::Large);
        assert_eq!(config.arrange, Arrangement::Free);
        assert_eq!(config.sort, SortBy::Date);
        assert!(!config.show);
        assert_eq!(config.layer, LayerKind::Window);
        let config: DesktopConfig = toml::from_str("").unwrap();
        assert_eq!(config, DesktopConfig::default());
        assert!(config.show);
        assert!(toml::from_str::<DesktopConfig>("columns = 4").is_err());
    }

    #[test]
    fn every_value_is_written_as_it_is_read() {
        for size in IconSize::ALL {
            let text = format!("size = \"{}\"", size.name());
            assert_eq!(toml::from_str::<DesktopConfig>(&text).unwrap().size, size);
        }
        for sort in SortBy::ALL {
            let text = format!("sort = \"{}\"", sort.name());
            assert_eq!(toml::from_str::<DesktopConfig>(&text).unwrap().sort, sort);
        }
        for arrange in [Arrangement::Auto, Arrangement::Grid, Arrangement::Free] {
            let text = format!("arrange = \"{}\"", arrange.name());
            assert_eq!(
                toml::from_str::<DesktopConfig>(&text).unwrap().arrange,
                arrange
            );
        }
    }

    #[test]
    fn the_size_steps_up_and_down_and_stops_at_the_ends() {
        assert_eq!(IconSize::Medium.step(true), IconSize::Large);
        assert_eq!(IconSize::Large.step(true), IconSize::Large);
        assert_eq!(IconSize::Medium.step(false), IconSize::Small);
        assert_eq!(IconSize::Small.step(false), IconSize::Small);
    }

    #[test]
    fn a_spot_is_found_in_the_room_that_holds_the_point() {
        let room = Room {
            frame: ScreenRect {
                left: 1000,
                top: 0,
                right: 3000,
                bottom: 1000,
            },
            scale: 2.,
            grid: Grid {
                columns: 10,
                rows: 5,
            },
        };
        let spot = room.spot_at((1000 + 300, 200), (100., 100.), 1).unwrap();
        assert_eq!((spot.column, spot.row, spot.display), (1., 0.5, 1));
        assert_eq!(room.spot_at((10, 10), (100., 100.), 1), None);
    }

    #[test]
    fn names_windows_keeps_out_are_refused() {
        assert!(is_valid_name("Report 2026.docx"));
        assert!(!is_valid_name("  "));
        assert!(!is_valid_name("a:b"));
        assert!(!is_valid_name("what?"));
        assert!(!is_valid_name("trailing."));
    }
}
