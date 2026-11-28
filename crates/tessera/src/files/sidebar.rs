// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The side of a window of the file manager: a tree of the places the
//! pane in front can be sent to, as Explorer has at its side. Home, then
//! OneDrive, the folders of Quick access as Explorer keeps them, the
//! places the user keeps (Control Panel, the libraries and the Gallery at
//! first), This PC with its drives, the phones and cameras, Network with
//! the shares of other computers visited, the saved searches, the Linux
//! distributions of WSL, and the Recycle Bin. Every folder of the tree,
//! and every place of the Shell that holds folders, opens below it with a
//! click on its chevron; what it holds is read only then, away from the
//! user interface.
//!
//! The places the user keeps ([`super::places`]) are renamed and removed
//! from their menus, ordered by dragging, and added from the menu of any
//! place; the groups Places, Remote, Devices and Searches are hidden and
//! shown again from the menu of a heading.

use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, Entity, EventEmitter, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, UniformListScrollHandle, WeakEntity, Window,
    div, prelude::FluentBuilder, px, uniform_list,
};

use super::model::{KnownShellPlace, Location, Shown, natural, read_folder};
use super::pane::{
    ICON_BIN, ICON_COMPUTER, ICON_DEVICE, ICON_FOLDER, ICON_NETWORK, ICON_NEW_TAB, ICON_NEW_WINDOW,
    ICON_PIN, ICON_SEARCH, ICON_UNPIN, drive_glyph,
};
use super::places::{KeptPlace, KeptPlaces, PlaceGroup, Places};
use super::session::Session;
use super::view::{DropPlaces, DropSpot};
use super::{Files, FilesConfig, HomeKind};
use crate::field::TextField;
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::platform::{
    Drive, ExplorerSettings, NameRules, Place, QuickAccess, QuickFolder, Reading, ShellFolder,
    ShellPath, ShellPlace, ShellRead, onedrive, wsl_distributions,
};
use crate::popup::PopupAnchor;
use crate::theme::Theme;
use crate::ui::{icon, one_line, place_look};

/// How wide the side is, in logical pixels.
pub const SIDEBAR_WIDTH: f32 = 230.;

/// How tall a row of the tree is, and how far a level is indented.
const ROW_HEIGHT: f32 = 30.;
const INDENT: f32 = 14.;

/// Characters of the icon font.
pub const ICON_HOME: &str = "\u{e80f}";
const ICON_CLOUD: &str = "\u{e753}";
const ICON_LINUX: &str = "\u{e756}";
const ICON_CLOSED: &str = "\u{e76b}";
const ICON_EXPANDED: &str = "\u{e70d}";
const ICON_PLACE: &str = "\u{e734}";
const ICON_PANEL: &str = "\u{e713}";
const ICON_LIBRARY: &str = "\u{e8f1}";
const ICON_GALLERY: &str = "\u{e91b}";
const ICON_RENAME: &str = "\u{e8ac}";
const ICON_REMOVE: &str = "\u{e738}";
const ICON_CHECK: &str = "\u{e73e}";
const ICON_NONE: &str = "";

/// The parsing name of the folder of saved searches.
const SAVED_SEARCHES: &str = "shell:Searches";

/// What the side asks of the window.
#[derive(Clone, Debug, PartialEq)]
pub enum SidebarEvent {
    /// To show `Location` in the pane in front.
    Go(Location),
    /// To show `Location` in a new tab, behind the one in front.
    OpenInNewTab(Location),
    /// To pin the folder `path` to Quick access, or with `pinned` false to
    /// unpin it.
    SetPinned { path: PathBuf, pinned: bool },
}

/// What a row of the tree does when it is clicked.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    Location(Location),
    /// Nothing but opening below it, as for the root of WSL.
    Nothing,
}

/// A place of the tree, with the folders it holds once they were read.
#[derive(Clone, Debug)]
struct Node {
    target: Target,
    label: String,
    glyph: &'static str,
    /// Whether it may hold folders, which a chevron opens.
    opens: bool,
    expanded: bool,
    /// The folders it holds, in order; `None` until they were asked for.
    children: Option<Vec<Node>>,
    /// Whether it is a folder the user pinned to Quick access.
    pinned: bool,
    /// Where it is among the places the user keeps, where it is one.
    kept: Option<usize>,
}

impl Node {
    fn folder(path: PathBuf, label: String, glyph: &'static str) -> Self {
        Self::place(Location::Folder(path), label, glyph)
    }

    /// A place that holds folders: a folder, or a place of the Shell.
    fn place(location: Location, label: String, glyph: &'static str) -> Self {
        Self {
            target: Target::Location(location),
            label,
            glyph,
            opens: true,
            expanded: false,
            children: None,
            pinned: false,
            kept: None,
        }
    }

    fn leaf(target: Target, label: String, glyph: &'static str) -> Self {
        Self {
            target,
            label,
            glyph,
            opens: false,
            expanded: false,
            children: None,
            pinned: false,
            kept: None,
        }
    }

    fn path(&self) -> Option<&Path> {
        self.location().and_then(Location::folder)
    }

    fn location(&self) -> Option<&Location> {
        match &self.target {
            Target::Location(location) => Some(location),
            Target::Nothing => None,
        }
    }

    /// Where what is dropped on it goes: its folder, or the place of the
    /// Shell by its parsing name.
    fn drop_target(&self) -> Option<PathBuf> {
        self.location().and_then(Location::target)
    }
}

/// The glyph a place has at the side, by where it leads.
fn glyph_of(location: &Location) -> &'static str {
    let known = { location.shell() }.and_then(|place| KnownShellPlace::of_parsing(place.parsing()));
    match known.map(|known| known.word) {
        Some("control-panel" | "all-control-panel-items") => ICON_PANEL,
        Some("libraries") => ICON_LIBRARY,
        Some("gallery") => ICON_GALLERY,
        Some("network") => ICON_NETWORK,
        Some("windows-home") => ICON_HOME,
        Some("saved-searches") => ICON_SEARCH,
        _ => match location {
            Location::Computer => ICON_COMPUTER,
            Location::RecycleBin => ICON_BIN,
            Location::Home => ICON_HOME,
            Location::Shell(place) if place.parsing().starts_with("\\\\") => ICON_NETWORK,
            Location::Shell(place) if place.parsing().ends_with(".search-ms") => ICON_SEARCH,
            Location::Shell(_) => ICON_PLACE,
            Location::Folder(path) if path.to_string_lossy().starts_with("\\\\") => ICON_NETWORK,
            _ => ICON_FOLDER,
        },
    }
}

/// A place the user keeps that is dragged to another place among them.
struct PlaceDrag {
    index: usize,
    label: SharedString,
}

/// What follows the pointer while a place is dragged: its name.
struct PlaceGhost(SharedString);

impl Render for PlaceGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        div()
            .px(theme.spacing.medium)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .bg(theme.colors.secondary_container)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            .child(self.0.clone())
    }
}

/// A part of the tree under a heading.
#[derive(Clone, Debug)]
struct Group {
    heading: Option<&'static str>,
    nodes: Vec<Node>,
    /// The group of kept places it shows, which the user may hide.
    kind: Option<PlaceGroup>,
}

/// Where a row of the tree is: the group, then the index at each level.
type Address = Vec<usize>;

/// A row of the tree as it is drawn.
#[derive(Clone, Debug)]
enum Row {
    Heading(&'static str),
    Node { address: Address, depth: usize },
}

/// The groups of the tree, by their place in it. The first place of the
/// first is Quick access, which holds its folders.
const HOME: usize = 0;
const PLACES: usize = 1;
const COMPUTER: usize = 2;
const DEVICES: usize = 3;
const NETWORK: usize = 4;
const SEARCHES: usize = 5;
const LINUX: usize = 6;

/// The group of the tree that shows the kept places of `group`.
fn group_index(group: PlaceGroup) -> usize {
    match group {
        PlaceGroup::Places => PLACES,
        PlaceGroup::Remote => NETWORK,
        PlaceGroup::Devices => DEVICES,
        PlaceGroup::Searches => SEARCHES,
    }
}

/// The side of a window.
pub struct Sidebar {
    groups: Vec<Group>,
    rows: Vec<Row>,
    /// Where the pane in front is, which is lit.
    current: Location,
    scroll: UniformListScrollHandle,
    /// Which folders the tree shows and how it names them.
    config: FilesConfig,
    /// Where the window takes drops, which the folders of the tree are
    /// written into as they are drawn, and the folder a drag is over.
    places: DropPlaces,
    drag_over: Option<PathBuf>,
    /// The places of the groups that are not kept: the shares visited, the
    /// devices of This PC without a letter, the saved searches.
    visited: Vec<PathBuf>,
    devices: Vec<(String, Location)>,
    searches: Vec<(String, Location)>,
    /// The kept place being renamed, by its place among them, with the
    /// field its new name is written in.
    renaming: Option<(usize, Entity<TextField>)>,
    /// The groups of kept places the user hid.
    hidden: Vec<PlaceGroup>,
    /// The window the side was last pressed in, which a field takes the
    /// keys of once a menu is gone.
    window: Option<gpui::AnyWindowHandle>,
    _kept: Subscription,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl Sidebar {
    pub fn new(current: Location, config: FilesConfig, cx: &mut Context<Self>) -> Self {
        let language = Language::of(cx);
        let say = |text: &'static str| language.say(text).to_owned();
        let mut computer = Node::leaf(
            Target::Location(Location::Computer),
            say("This PC"),
            ICON_COMPUTER,
        );
        (computer.opens, computer.expanded) = (true, true);
        // Quick access is Explorer's own; Home is Tessera's or Windows'.
        let home = match config.home {
            HomeKind::Tessera => Location::Home,
            HomeKind::Windows => KnownShellPlace::of_word("windows-home")
                .map_or(Location::Home, KnownShellPlace::location),
        };
        let mut quick = Node::leaf(Target::Location(home), say("Quick access"), ICON_HOME);
        (quick.opens, quick.expanded) = (true, true);
        let group =
            |heading: Option<&'static str>, nodes: Vec<Node>, kind: Option<PlaceGroup>| Group {
                heading,
                nodes,
                kind,
            };
        let groups = vec![
            group(None, vec![quick], None),
            group(Some("Places"), Vec::new(), Some(PlaceGroup::Places)),
            group(Some("This PC"), vec![computer], None),
            group(Some("Devices"), Vec::new(), Some(PlaceGroup::Devices)),
            group(Some("Remote"), Vec::new(), Some(PlaceGroup::Remote)),
            group(Some("Searches"), Vec::new(), Some(PlaceGroup::Searches)),
            group(None, Vec::new(), None),
            group(
                None,
                vec![Node::leaf(
                    Target::Location(Location::RecycleBin),
                    say("Recycle Bin"),
                    ICON_BIN,
                )],
                None,
            ),
        ];
        KeptPlaces::load(cx);
        let kept = cx.observe_global::<KeptPlaces>(|this: &mut Self, cx| {
            this.build_kept(cx);
            cx.notify();
        });
        let mut sidebar = Self {
            groups,
            rows: Vec::new(),
            current,
            scroll: UniformListScrollHandle::new(),
            config,
            places: DropPlaces::default(),
            drag_over: None,
            visited: Vec::new(),
            devices: Vec::new(),
            searches: Vec::new(),
            renaming: None,
            hidden: Vec::new(),
            window: None,
            _kept: kept,
        };
        sidebar.read_places(cx);
        sidebar.read_quick_access(cx);
        sidebar.read_drives(cx);
        sidebar.read_shell_places(cx);
        sidebar.build_kept(cx);
        sidebar
    }

    /// Lays out the groups of kept places anew, with the places of those
    /// groups that are not kept after them; a place that was open stays
    /// open.
    fn build_kept(&mut self, cx: &mut Context<Self>) {
        let language = Language::of(cx);
        // Until the places are read, none is kept and no group hidden.
        let places = KeptPlaces::get(cx).cloned().unwrap_or_else(|| Places {
            places: Vec::new(),
            hidden: Vec::new(),
        });
        for group in PlaceGroup::ALL {
            let index = group_index(group);
            let old = std::mem::take(&mut self.groups[index].nodes);
            let keep = |location: Location, label: String, kept: Option<usize>| {
                let found = { old.iter() }.find(|node| node.location() == Some(&location));
                let mut node = match found {
                    Some(found) => found.clone(),
                    None => Node::place(location.clone(), label.clone(), glyph_of(&location)),
                };
                (node.label, node.kept) = (label, kept);
                node
            };
            let mut nodes: Vec<Node> = { places.of(group).into_iter() }
                .map(|(index, place)| {
                    keep(
                        Files::named(&place.location),
                        language.translate(&place.name),
                        Some(index),
                    )
                })
                .collect();
            let found: Vec<(String, Location)> = match group {
                PlaceGroup::Remote => { self.visited.iter() }
                    .map(|root| (root.display().to_string(), Location::Folder(root.clone())))
                    .collect(),
                PlaceGroup::Devices => self.devices.clone(),
                PlaceGroup::Searches => self.searches.clone(),
                PlaceGroup::Places => Vec::new(),
            };
            for (label, location) in found {
                if !nodes.iter().any(|node| node.location() == Some(&location)) {
                    nodes.push(keep(location, label, None));
                }
            }
            if !places.shows(group) {
                nodes.clear();
            }
            self.groups[index].nodes = nodes;
        }
        self.hidden = places.hidden;
        self.flatten();
    }

    /// Reads the devices of This PC without a letter (phones, cameras) and
    /// the saved searches, away from the user interface, each on a thread
    /// of the places of the Shell.
    fn read_shell_places(&mut self, cx: &mut Context<Self>) {
        let computer = ShellPath::named(super::model::COMPUTER_PARSING, "This PC");
        let devices = ShellFolder::open(&computer, Reading::default());
        let searches = ShellFolder::open(
            &ShellPath::named(SAVED_SEARCHES, "Searches"),
            Reading::default(),
        );
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut found = Vec::new();
            while let Some(read) = devices.next().await {
                match read {
                    ShellRead::Resolved(_) => {}
                    ShellRead::Items(children) => found.extend(
                        { children.into_iter() }
                            .filter(|child| child.path.is_none() && child.attributes.folder)
                            .map(|child| {
                                let location = Location::of_shell(computer.child(&child));
                                (child.name, location)
                            }),
                    ),
                    _ => break,
                }
            }
            let mut saved = Vec::new();
            let mut place = ShellPath::named(SAVED_SEARCHES, "Searches");
            while let Some(read) = searches.next().await {
                match read {
                    ShellRead::Resolved(resolved) => place = resolved,
                    ShellRead::Items(children) => {
                        saved.extend({ children.into_iter() }.map(|child| {
                            let location = Location::of_shell(place.child(&child));
                            (child.name, location)
                        }))
                    }
                    _ => break,
                }
            }
            this.update(cx, |this, cx| {
                this.devices = found;
                this.searches = saved;
                this.build_kept(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Lights the place the pane in front is at.
    /// Has the folders of the tree take drops, into the places of the
    /// window.
    pub fn take_drops(&mut self, places: DropPlaces) {
        self.places = places;
    }

    /// Marks the place of `folder` as the one a drag is over, or none.
    pub fn mark_drag_over(&mut self, folder: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.drag_over != folder {
            self.drag_over = folder;
            cx.notify();
        }
    }

    pub fn set_current(&mut self, current: Location, cx: &mut Context<Self>) {
        if self.current != current {
            self.current = current;
            cx.notify();
        }
    }

    /// Puts the shares of other computers `roots` under Remote, those
    /// there already kept.
    pub fn add_network(&mut self, roots: &[PathBuf], cx: &mut Context<Self>) {
        for root in roots {
            if !self.visited.contains(root) {
                self.visited.push(root.clone());
            }
        }
        self.build_kept(cx);
        cx.notify();
    }

    /// Reads OneDrive, the distributions of WSL and the shares visited
    /// before, away from the user interface.
    fn read_places(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (onedrive, linux, network) = cx
                .background_executor()
                .spawn(async { (onedrive(), wsl_distributions(), Session::load().network) })
                .await;
            this.update(cx, |this, cx| {
                this.add_network(&network, cx);
                if let Some(path) = onedrive {
                    let node = Node::folder(path, "OneDrive".to_owned(), ICON_CLOUD);
                    this.groups[HOME].nodes.push(node);
                }
                if !linux.is_empty() {
                    let children = { linux.into_iter() }
                        .map(|ShellPlace { name, path }| Node::folder(path, name, ICON_LINUX))
                        .collect();
                    this.groups[LINUX].nodes = vec![Node {
                        opens: true,
                        children: Some(children),
                        ..Node::leaf(Target::Nothing, "Linux".to_owned(), ICON_LINUX)
                    }];
                }
                this.flatten();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads the folders of Quick access as Explorer keeps them; where
    /// they cannot be read, the known folders of the user stand there.
    pub fn read_quick_access(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let folders = match QuickAccess::folders().await {
                Ok(folders) => folders,
                Err(error) => {
                    report!(
                        "Quick access cannot be read, the known folders stand there: {error:#}"
                    );
                    let known = cx
                        .background_executor()
                        .spawn(async {
                            let found = |place: Place| Some((place, place.path()?));
                            Place::ALL.into_iter().filter_map(found).collect::<Vec<_>>()
                        })
                        .await;
                    this.update(cx, |this, cx| {
                        let language = Language::of(cx);
                        this.groups[HOME].nodes[0].children = { known.into_iter() }
                            .map(|(place, path)| {
                                let (label, glyph) = place_look(place);
                                Node::folder(path, language.say(label).to_owned(), glyph)
                            })
                            .collect::<Vec<_>>()
                            .into();
                        this.flatten();
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            this.update(cx, |this, cx| {
                let quick = &mut this.groups[HOME].nodes[0];
                let old = quick.children.take().unwrap_or_default();
                let folders: Vec<Node> = { folders.into_iter() }
                    .map(|QuickFolder { place, pinned }| {
                        let ShellPlace { name, path } = place;
                        // A folder that was open stays open.
                        let kept = old.iter().find(|node| node.path() == Some(&path));
                        let mut node = match kept {
                            Some(kept) => kept.clone(),
                            None => Node::folder(path, name.clone(), ICON_FOLDER),
                        };
                        (node.label, node.pinned) = (name, pinned);
                        node
                    })
                    .collect();
                quick.opens = !folders.is_empty();
                quick.children = Some(folders);
                this.flatten();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads the drives under This PC, away from the user interface, as a
    /// drive may take its time to answer.
    pub fn read_drives(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let drives = cx.background_executor().spawn(async { Drive::all() }).await;
            this.update(cx, |this, cx| {
                let computer = &mut this.groups[COMPUTER].nodes[0];
                let old = computer.children.take().unwrap_or_default();
                let children = { drives.into_iter() }
                    .map(|drive| {
                        // A drive that was open stays open.
                        let kept = old.iter().find(|node| node.path() == Some(&drive.root));
                        let mut node = match kept {
                            Some(kept) => kept.clone(),
                            None => Node::folder(drive.root.clone(), String::new(), ICON_FOLDER),
                        };
                        (node.label, node.glyph) = (drive.title(), drive_glyph(drive.kind));
                        node
                    })
                    .collect();
                computer.children = Some(children);
                this.flatten();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn node(&self, address: &[usize]) -> Option<&Node> {
        let (&group, path) = address.split_first()?;
        let (&first, rest) = path.split_first()?;
        let mut node = self.groups.get(group)?.nodes.get(first)?;
        for &index in rest {
            node = node.children.as_ref()?.get(index)?;
        }
        Some(node)
    }

    /// The place showing `location` that waits for what it holds: the one
    /// at `address`, where the tree was not built anew since, or else the
    /// first open one of that place not read yet.
    fn waiting(&mut self, address: &[usize], location: &Location) -> Option<&mut Node> {
        fn find<'a>(nodes: &'a mut [Node], location: &Location) -> Option<&'a mut Node> {
            for node in nodes {
                if node.expanded && node.children.is_none() && node.location() == Some(location) {
                    return Some(node);
                }
                if let Some(found) =
                    { node.children.as_mut() }.and_then(|nodes| find(nodes, location))
                {
                    return Some(found);
                }
            }
            None
        }
        if self.node(address).and_then(Node::location) == Some(location) {
            return self.node_mut(address);
        }
        { self.groups.iter_mut() }.find_map(|group| find(&mut group.nodes, location))
    }

    fn node_mut(&mut self, address: &[usize]) -> Option<&mut Node> {
        let (&group, path) = address.split_first()?;
        let (&first, rest) = path.split_first()?;
        let mut node = self.groups.get_mut(group)?.nodes.get_mut(first)?;
        for &index in rest {
            node = node.children.as_mut()?.get_mut(index)?;
        }
        Some(node)
    }

    /// Lays the tree out as the rows that are drawn: the headings of the
    /// groups that have places, and every place under an open one.
    fn flatten(&mut self) {
        fn walk(nodes: &[Node], address: &mut Address, depth: usize, rows: &mut Vec<Row>) {
            for (index, node) in nodes.iter().enumerate() {
                address.push(index);
                rows.push(Row::Node {
                    address: address.clone(),
                    depth,
                });
                if node.expanded
                    && let Some(children) = &node.children
                {
                    walk(children, address, depth + 1, rows);
                }
                address.pop();
            }
        }
        let mut rows = Vec::new();
        for (index, group) in self.groups.iter().enumerate() {
            // A group of kept places shows its heading while it is empty, so
            // that a place can be added to it; one hidden shows nothing.
            let kept = group.kind.is_some_and(|kind| !self.hidden.contains(&kind));
            if group.nodes.is_empty() && !kept {
                continue;
            }
            if let Some(heading) = group.heading {
                rows.push(Row::Heading(heading));
            }
            walk(&group.nodes, &mut vec![index], 0, &mut rows);
        }
        self.rows = rows;
    }

    /// Opens the place at `address` below it, or closes it; what it holds
    /// is read the first time.
    fn toggle(&mut self, address: Address, cx: &mut Context<Self>) {
        let Some(node) = self.node_mut(&address) else {
            return;
        };
        node.expanded = !node.expanded;
        let unread = node.expanded && node.children.is_none();
        let location = node.location().cloned();
        self.flatten();
        cx.notify();
        match (unread, location) {
            (true, Some(Location::Folder(path))) => self.read_children(address, path, cx),
            (true, Some(Location::Shell(place))) => self.read_shell_children(address, place, cx),
            _ => {}
        }
    }

    /// Reads the places that hold items in the place of the Shell `place`,
    /// the place at `address`, on a thread of the places of the Shell.
    fn read_shell_children(&mut self, address: Address, place: ShellPath, cx: &mut Context<Self>) {
        let listing = ShellFolder::open(&place, Reading::default());
        let asked = Location::Shell(place.clone());
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut place = place;
            let mut folders: Vec<(String, Location)> = Vec::new();
            while let Some(read) = listing.next().await {
                match read {
                    ShellRead::Resolved(resolved) => place = resolved,
                    ShellRead::Items(children) => folders.extend(
                        { children.into_iter() }
                            .filter(|child| child.attributes.is_browsed(false))
                            .map(|child| {
                                (child.name.clone(), Location::of_shell(place.child(&child)))
                            }),
                    ),
                    _ => break,
                }
            }
            folders.sort_by(|a, b| natural(&a.0, &b.0));
            this.update(cx, |this, cx| {
                if let Some(node) = this.waiting(&address, &asked) {
                    let children: Vec<Node> = { folders.into_iter() }
                        .map(|(label, location)| {
                            let glyph = glyph_of(&location);
                            Node::place(location, label, glyph)
                        })
                        .collect();
                    node.opens = !children.is_empty();
                    node.children = Some(children);
                }
                this.flatten();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads the folders in `path`, the place at `address`, named and
    /// hidden as the file manager shows them.
    fn read_children(&mut self, address: Address, path: PathBuf, cx: &mut Context<Self>) {
        let config = self.config;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = path.clone();
            let folders = cx
                .background_executor()
                .spawn(async move {
                    let explorer = ExplorerSettings::read();
                    let mut rules = NameRules::new(explorer, config.explorer);
                    let shown = Shown::of(&config, explorer, None);
                    let mut folders: Vec<(String, PathBuf)> = read_folder(&read, &mut rules)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|entry| entry.folder && shown.shows(entry))
                        .map(|entry| (entry.display, entry.path))
                        .collect();
                    folders.sort_by(|a, b| natural(&a.0, &b.0));
                    folders
                })
                .await;
            this.update(cx, |this, cx| {
                // The tree may have been built anew while the folder was
                // read, which moves the place.
                if let Some(node) = this.waiting(&address, &Location::Folder(path)) {
                    let children: Vec<Node> = { folders.into_iter() }
                        .map(|(label, path)| Node::folder(path, label, ICON_FOLDER))
                        .collect();
                    node.opens = !children.is_empty();
                    node.children = Some(children);
                }
                this.flatten();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A press of a button of the pointer on the place at `address`.
    fn press(
        &mut self,
        address: Address,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.window = Some(window.window_handle());
        let Some(node) = self.node(&address).cloned() else {
            return;
        };
        match (event.button, &node.target) {
            (MouseButton::Left, Target::Location(location)) => {
                cx.emit(SidebarEvent::Go(location.clone()))
            }
            (MouseButton::Left, Target::Nothing) => self.toggle(address, cx),
            (MouseButton::Middle, Target::Location(location)) => {
                cx.emit(SidebarEvent::OpenInNewTab(location.clone()))
            }
            (MouseButton::Right, Target::Location(location)) => {
                self.open_menu(node.clone(), location.clone(), event, window, cx)
            }
            _ => {}
        }
    }

    /// Opens the menu of a place of the tree.
    fn open_menu(
        &mut self,
        node: Node,
        location: Location,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let anchor = match PopupAnchor::under(window, event.position, cx) {
            Ok(anchor) => anchor,
            Err(error) => {
                report!("the menu has no place: {error:#}");
                return;
            }
        };
        let language = Language::of(cx);
        let this = cx.entity().downgrade();
        let mut entries = Vec::new();
        let tab = location.clone();
        let sidebar = this.clone();
        entries.push(MenuEntry::new(
            ICON_NEW_TAB,
            language.say("Open in new tab"),
            move |cx| {
                sidebar
                    .update(cx, |_, cx| cx.emit(SidebarEvent::OpenInNewTab(tab.clone())))
                    .ok();
            },
        ));
        let alone = location.clone();
        entries.push(MenuEntry::new(
            ICON_NEW_WINDOW,
            language.say("Open in new window"),
            move |cx| super::Files::open(alone.clone(), cx),
        ));
        if let Some(path) = node.path().map(Path::to_owned) {
            let (glyph, label) = match node.pinned {
                true => (ICON_UNPIN, "Unpin from Quick access"),
                false => (ICON_PIN, "Pin to Quick access"),
            };
            let pinned = node.pinned;
            let this = this.clone();
            entries.push(MenuEntry::new(glyph, language.say(label), move |cx| {
                let path = path.clone();
                this.update(cx, |_, cx| {
                    cx.emit(SidebarEvent::SetPinned {
                        path,
                        pinned: !pinned,
                    })
                })
                .ok();
            }));
        }
        match node.kept {
            Some(index) => {
                let renamed = this.clone();
                entries.push(MenuEntry::new(
                    ICON_RENAME,
                    language.say("Rename"),
                    move |cx| {
                        let renamed = renamed.clone();
                        // The menu is gone before the field takes the keys.
                        cx.defer(move |cx| {
                            renamed
                                .update(cx, |this, cx| this.begin_rename(index, cx))
                                .ok();
                        });
                    },
                ));
                entries.push(MenuEntry::new(
                    ICON_REMOVE,
                    language.say("Remove from Places"),
                    move |cx| KeptPlaces::change(cx, |places| places.remove(index)),
                ));
            }
            None => {
                let group = match node.glyph {
                    ICON_DEVICE => PlaceGroup::Devices,
                    _ => PlaceGroup::of_location(&location.name()),
                };
                entries.push(add_to_places(
                    node.label.clone(),
                    &location,
                    group,
                    language,
                ));
            }
        }
        Menus::open(entries, anchor, cx);
    }

    /// Opens the menu of a heading: each group of kept places, with a
    /// check where it is shown, to show or hide it.
    fn open_heading_menu(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let anchor = match PopupAnchor::under(window, event.position, cx) {
            Ok(anchor) => anchor,
            Err(error) => {
                report!("the menu has no place: {error:#}");
                return;
            }
        };
        let language = Language::of(cx);
        let entries = { PlaceGroup::ALL.into_iter() }
            .map(|group| {
                let shown = !self.hidden.contains(&group);
                let glyph = if shown { ICON_CHECK } else { ICON_NONE };
                MenuEntry::new(glyph, language.say(group.title()), move |cx| {
                    KeptPlaces::change(cx, |places| places.show(group, !shown))
                })
            })
            .collect();
        Menus::open(entries, anchor, cx);
    }

    /// Writes the name of the kept place `index` in a field in its row.
    fn begin_rename(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(name) = { KeptPlaces::get(cx) }
            .and_then(|places| places.places.get(index))
            .map(|place| Language::of(cx).translate(&place.name))
        else {
            return;
        };
        let field = cx.new(|cx| {
            let mut field = TextField::new(SharedString::default(), cx);
            field.set_text(&name, false, cx);
            field
        });
        if let Some(window) = self.window {
            let focused = field.clone();
            window
                .update(cx, |_, window, cx| {
                    window.activate_window();
                    focused.read(cx).focus(window);
                })
                .ok();
        }
        self.renaming = Some((index, field));
        cx.notify();
    }

    /// Takes a key of the field a place is renamed in: Enter gives the
    /// name, Escape keeps the old one.
    fn rename_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let done = match event.keystroke.key.as_str() {
            "enter" => true,
            "escape" => false,
            _ => return,
        };
        cx.stop_propagation();
        let Some((index, field)) = self.renaming.take() else {
            return;
        };
        if done {
            let name = field.read(cx).text();
            KeptPlaces::change(cx, |places| places.rename(index, &name));
        }
        window.blur();
        cx.notify();
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = *cx.global::<Theme>();
        let theme = &theme;
        let language = Language::of(cx);
        range
            .filter_map(|index| {
                let row = self.rows.get(index)?.clone();
                let (address, depth) = match row {
                    Row::Heading(text) => {
                        return Some(
                            div()
                                .id(("files-side-heading", index))
                                .h(px(ROW_HEIGHT))
                                .flex()
                                .items_end()
                                .pb(theme.spacing.tiny)
                                .px(theme.spacing.medium)
                                .text_size(theme.typography.label_size * 0.85)
                                .text_color(theme.colors.on_surface_variant)
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(|this, event, window, cx| {
                                        this.open_heading_menu(event, window, cx)
                                    }),
                                )
                                .child(language.say(text))
                                .into_any_element(),
                        );
                    }
                    Row::Node { address, depth } => (address, depth),
                };
                let node = self.node(&address)?;
                let lit = node.target == Target::Location(self.current.clone());
                let folder = node.drop_target();
                let over = folder.is_some() && folder == self.drag_over;
                let zone = folder.map(|folder| self.places.zone(DropSpot::Side, folder));
                let renaming = { self.renaming.as_ref() }
                    .filter(|(kept, _)| Some(*kept) == node.kept)
                    .map(|(_, field)| field.clone());
                let chevron = match (node.opens, node.expanded) {
                    (false, _) => "",
                    (true, true) => ICON_EXPANDED,
                    (true, false) => ICON_CLOSED,
                };
                let toggled = address.clone();
                let chevron = div()
                    .id(("files-side-chevron", index))
                    .size(px(18.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme.shape.small)
                    .font_family(theme.typography.icon_family)
                    .text_size(px(9.))
                    .text_color(theme.colors.on_surface_variant)
                    .when(node.opens, |chevron| {
                        chevron
                            .hover(|style| style.bg(theme.colors.surface_container_highest))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle(toggled.clone(), cx)
                                }),
                            )
                    })
                    .child(chevron);
                let mut item = div()
                    .id(("files-side-row", index))
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .gap(theme.spacing.tiny)
                    .pl(px(4. + INDENT * depth as f32))
                    .pr(theme.spacing.small)
                    .rounded(theme.shape.medium)
                    .cursor_pointer()
                    .text_size(theme.typography.label_size)
                    .text_color(theme.colors.on_surface)
                    .relative()
                    .when(lit, |item| item.bg(theme.colors.secondary_container))
                    .when(!lit, |item| {
                        item.hover(|style| style.bg(theme.colors.surface_container_highest))
                    })
                    .when(over, |item| {
                        item.bg(theme.colors.primary.opacity(0.2))
                            .border_1()
                            .border_color(theme.colors.primary)
                    })
                    .children(zone)
                    .child(chevron)
                    .child(
                        div()
                            .text_color(theme.colors.primary)
                            .child(icon(theme, node.glyph)),
                    )
                    .child(match renaming {
                        Some(field) => div()
                            .flex_1()
                            .min_w_0()
                            .on_key_down(cx.listener(|this, event, window, cx| {
                                this.rename_key(event, window, cx)
                            }))
                            .child(field)
                            .into_any_element(),
                        None => div()
                            .flex_1()
                            .min_w_0()
                            .child(one_line(node.label.clone()))
                            .into_any_element(),
                    })
                    .when(node.pinned, |item| {
                        item.child(
                            div()
                                .text_color(theme.colors.on_surface_variant)
                                .child(icon(theme, ICON_PIN).text_size(px(10.))),
                        )
                    });
                for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
                    let address = address.clone();
                    item = item.on_mouse_down(
                        button,
                        cx.listener(move |this, event, window, cx| {
                            this.press(address.clone(), event, window, cx)
                        }),
                    );
                }
                // A kept place is dragged to another place among them.
                if let Some(kept) = node.kept {
                    let marker = theme.colors.primary;
                    let drag = PlaceDrag {
                        index: kept,
                        label: node.label.clone().into(),
                    };
                    item = item
                        .on_drag(drag, |drag: &PlaceDrag, _, _, cx| {
                            let label = drag.label.clone();
                            cx.new(|_| PlaceGhost(label))
                        })
                        .drag_over::<PlaceDrag>(move |style, _, _, _| {
                            style.border_t_2().border_color(marker)
                        })
                        .on_drop(cx.listener(move |_, drag: &PlaceDrag, _, cx| {
                            let from = drag.index;
                            KeptPlaces::change(cx, |places| places.move_to(from, kept));
                        }));
                }
                Some(item.into_any_element())
            })
            .collect()
    }
}

/// The entry of a menu that keeps `location`, called `name`, among the
/// places of `group` at the side.
pub fn add_to_places(
    name: String,
    location: &Location,
    group: PlaceGroup,
    language: Language,
) -> MenuEntry {
    let place = KeptPlace {
        name,
        location: location.name(),
        group,
    };
    MenuEntry::new(ICON_PLACE, language.say("Add to Places"), move |cx| {
        let place = place.clone();
        KeptPlaces::change(cx, |places| {
            places.add(place);
        });
    })
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let rows = self.rows.len();
        div()
            .id("files-side")
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .py(theme.spacing.small)
            .px(theme.spacing.tiny)
            .child(
                uniform_list(
                    "files-side-list",
                    rows,
                    cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
                )
                .track_scroll(self.scroll.clone())
                .flex_1(),
            )
    }
}
