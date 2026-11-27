// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The side of a window of the file manager: a tree of the places the
//! pane in front can be sent to, as Explorer has at its side. Home, then
//! OneDrive, the folders of Quick access as Explorer keeps them, This PC
//! with its drives, the shares of other computers visited, the Linux
//! distributions of WSL, and the Recycle Bin. Every folder of the tree
//! opens below it with a click on its chevron; what it holds is read
//! only then, away from the user interface.

use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AsyncApp, Context, EventEmitter, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Render, Styled, UniformListScrollHandle, WeakEntity, Window,
    div, prelude::FluentBuilder, px, uniform_list,
};

use super::FilesConfig;
use super::model::{Location, Shown, natural, read_folder};
use super::pane::{
    ICON_BIN, ICON_COMPUTER, ICON_FOLDER, ICON_NETWORK, ICON_NEW_TAB, ICON_NEW_WINDOW, ICON_PIN,
    ICON_UNPIN, drive_glyph,
};
use super::session::Session;
use super::view::{DropPlaces, DropSpot};
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::platform::{
    Drive, ExplorerSettings, NameRules, Place, QuickAccess, QuickFolder, ShellPlace, onedrive,
    wsl_distributions,
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
}

impl Node {
    fn folder(path: PathBuf, label: String, glyph: &'static str) -> Self {
        Self {
            target: Target::Location(Location::Folder(path)),
            label,
            glyph,
            opens: true,
            expanded: false,
            children: None,
            pinned: false,
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
        }
    }

    fn path(&self) -> Option<&Path> {
        match &self.target {
            Target::Location(location) => location.folder(),
            _ => None,
        }
    }
}

/// A part of the tree under a heading.
#[derive(Clone, Debug)]
struct Group {
    heading: Option<&'static str>,
    nodes: Vec<Node>,
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
const COMPUTER: usize = 1;
const NETWORK: usize = 2;
const LINUX: usize = 3;

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
        let mut quick = Node::leaf(
            Target::Location(Location::Home),
            say("Quick access"),
            ICON_HOME,
        );
        (quick.opens, quick.expanded) = (true, true);
        let groups = vec![
            Group {
                heading: None,
                nodes: vec![quick],
            },
            Group {
                heading: Some("This PC"),
                nodes: vec![computer],
            },
            Group {
                heading: Some("Network"),
                nodes: Vec::new(),
            },
            Group {
                heading: None,
                nodes: Vec::new(),
            },
            Group {
                heading: None,
                nodes: vec![Node::leaf(
                    Target::Location(Location::RecycleBin),
                    say("Recycle Bin"),
                    ICON_BIN,
                )],
            },
        ];
        let mut sidebar = Self {
            groups,
            rows: Vec::new(),
            current,
            scroll: UniformListScrollHandle::new(),
            config,
            places: DropPlaces::default(),
            drag_over: None,
        };
        sidebar.read_places(cx);
        sidebar.read_quick_access(cx);
        sidebar.read_drives(cx);
        sidebar.flatten();
        sidebar
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

    /// Puts the shares of other computers `roots` under Network, those
    /// there already kept.
    pub fn add_network(&mut self, roots: &[PathBuf], cx: &mut Context<Self>) {
        let nodes = &mut self.groups[NETWORK].nodes;
        for root in roots {
            if nodes.iter().any(|node| node.path() == Some(root)) {
                continue;
            }
            let label = root.display().to_string();
            nodes.push(Node::folder(root.clone(), label, ICON_NETWORK));
        }
        self.flatten();
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

    /// The place showing the folder `path` that waits for what it holds:
    /// the one at `address`, where the tree was not built anew since, or
    /// else the first open one of that folder not read yet.
    fn waiting(&mut self, address: &[usize], path: &Path) -> Option<&mut Node> {
        fn find<'a>(nodes: &'a mut [Node], path: &Path) -> Option<&'a mut Node> {
            for node in nodes {
                if node.expanded && node.children.is_none() && node.path() == Some(path) {
                    return Some(node);
                }
                if let Some(found) = node.children.as_mut().and_then(|nodes| find(nodes, path)) {
                    return Some(found);
                }
            }
            None
        }
        if self.node(address).and_then(Node::path) == Some(path) {
            return self.node_mut(address);
        }
        { self.groups.iter_mut() }.find_map(|group| find(&mut group.nodes, path))
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
            if group.nodes.is_empty() {
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
        let path = node.path().map(Path::to_owned);
        self.flatten();
        cx.notify();
        if let (true, Some(path)) = (unread, path) {
            self.read_children(address, path, cx);
        }
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
                if let Some(node) = this.waiting(&address, &path) {
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
        Menus::open(entries, anchor, cx);
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
                                .h(px(ROW_HEIGHT))
                                .flex()
                                .items_end()
                                .pb(theme.spacing.tiny)
                                .px(theme.spacing.medium)
                                .text_size(theme.typography.label_size * 0.85)
                                .text_color(theme.colors.on_surface_variant)
                                .child(language.say(text))
                                .into_any_element(),
                        );
                    }
                    Row::Node { address, depth } => (address, depth),
                };
                let node = self.node(&address)?;
                let lit = node.target == Target::Location(self.current.clone());
                let folder = node.path().map(Path::to_owned);
                let over = folder.is_some() && folder == self.drag_over;
                let zone = folder.map(|folder| self.places.zone(DropSpot::Side, folder));
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
                    .child(div().flex_1().min_w_0().child(one_line(node.label.clone())))
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
                Some(item.into_any_element())
            })
            .collect()
    }
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
