// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The jump list of an application as entries of a menu, the way the
//! taskbar of Windows shows it: what the user pinned, what Windows
//! recorded as opened often or lately (whichever the application asked
//! for), the categories the application made, and its tasks, each under
//! its title and with its icon.
//!
//! Reading a list loads shortcuts through COM and asks the Shell for the
//! names of their documents, and reading an icon asks the Shell too, which
//! can wait for long on a document on the network or a drive that is gone.
//! So both are done on threads of their own, set up as the shell thread
//! ([`ShellThread::call_alone`]), and hold up no other work. What was
//! read is kept here and given at once; a list is read anew when its files
//! changed. The first time the list of an application is asked for nothing
//! is known of it yet, so [`prefetch`] reads it ahead, as when the pointer
//! comes to the button of the application.
//!
//! Pinning to a jump list has no documented interface, so Tessera keeps
//! its own pins, for each application by its identifier, in
//! `jump-pins.toml` in the configuration directory, and shows them first
//! under Pinned, with the documents Explorer's taskbar pinned (read from
//! the pins of its record). An entry of Frequent, Recent or a category of
//! the application is pinned with the pin at its end or with "Pin to this
//! list", and a pinned one is unpinned the same way; Explorer's own pins
//! are then only no longer shown by Tessera, as Explorer's record is not
//! written. "Remove from this list" takes a recorded document off the
//! record of Windows (`IApplicationDestinations::RemoveDestination`, which
//! is documented), and hides an entry of a category of the application in
//! Tessera: only Explorer can tell the application, through an interface
//! it serves itself. A recorded shortcut that starts a program with
//! arguments is not named to Windows by a path, so it is hidden as well.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use gpui::{App, AsyncApp, Global};
use serde::{Deserialize, Serialize};

use crate::config::{config_dir, read_user_file};
use crate::files::Files;
use crate::i18n::Language;
use crate::icons::{Icon, icon_of_pixels};
use crate::menu::MenuEntry;
use crate::platform::{JumpItem, JumpList, JumpTarget, KnownCategory, ShellThread};

/// How many pixels wide and high the icons of the entries are read.
const PICTURE_PIXELS: u32 = 32;

/// How many entries a known category or a category of the application's
/// own shows at most; the pins and the tasks are shown whole.
const SHOWN: usize = 8;

/// How many icons are kept before they are read anew.
const ICONS_KEPT: usize = 512;

/// The file of Tessera's own pins, in the configuration directory.
const PINS_FILE: &str = "jump-pins.toml";

const ICON_PINNED: &str = "\u{e718}";
const ICON_UNPIN: &str = "\u{e77a}";
const ICON_FREQUENT: &str = "\u{e728}";
const ICON_RECENT: &str = "\u{e81c}";
const ICON_DOCUMENT: &str = "\u{e8a5}";
const ICON_TASK: &str = "\u{e945}";
const ICON_REMOVE: &str = "\u{e711}";

/// What the icon of an entry is read from: the file and number it asks
/// for, or else its document or program with a number of its own.
type IconKey = (PathBuf, i32);

/// The number that stands for the icon the Shell draws for a document.
const ICON_OF_ITEM: i32 = i32::MIN;

fn icon_key(item: &JumpItem) -> IconKey {
    match (&item.icon, &item.target) {
        (Some((file, index)), _) => (file.clone(), *index),
        (None, JumpTarget::Item(path)) => (path.clone(), ICON_OF_ITEM),
        (None, JumpTarget::Command { program, .. }) => (program.clone(), 0),
    }
}

/// What tells an entry from the others of its list: its document, or its
/// program with its arguments, compared without case as Windows compares
/// paths.
fn entry_key(item: &JumpItem) -> String {
    match &item.target {
        JumpTarget::Item(path) => path.to_string_lossy().to_lowercase(),
        JumpTarget::Command { program, arguments } => {
            format!("{} {arguments}", program.display()).to_lowercase()
        }
    }
}

/// An entry as `jump-pins.toml` keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredItem {
    name: String,
    /// The document, for an entry that opens one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
    /// The program, for an entry that starts one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    program: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    arguments: String,
    /// The file of its icon, and the number of the icon in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "is_zero")]
    icon_index: i32,
}

fn is_zero(number: &i32) -> bool {
    *number == 0
}

impl StoredItem {
    fn of(item: &JumpItem) -> Self {
        let (path, program, arguments) = match &item.target {
            JumpTarget::Item(path) => (Some(path.clone()), None, String::new()),
            JumpTarget::Command { program, arguments } => {
                (None, Some(program.clone()), arguments.clone())
            }
        };
        Self {
            name: item.name.clone(),
            path,
            program,
            arguments,
            icon: item.icon.as_ref().map(|(file, _)| file.clone()),
            icon_index: item.icon.as_ref().map_or(0, |(_, index)| *index),
        }
    }

    /// The entry it keeps; `None` for one that names neither a document
    /// nor a program.
    fn item(&self) -> Option<JumpItem> {
        let target = match (&self.path, &self.program) {
            (Some(path), _) => JumpTarget::Item(path.clone()),
            (None, Some(program)) => JumpTarget::Command {
                program: program.clone(),
                arguments: self.arguments.clone(),
            },
            (None, None) => return None,
        };
        Some(JumpItem {
            name: self.name.clone(),
            target,
            icon: self.icon.clone().map(|file| (file, self.icon_index)),
            recorded: false,
        })
    }
}

/// What Tessera keeps of the jump list of one application.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppPins {
    /// The entries the user pinned in Tessera, in their order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pinned: Vec<StoredItem>,
    /// The entries Tessera does not show, by [`entry_key`]: those taken
    /// off a category of the application and Explorer's pins unpinned.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hidden: Vec<String>,
}

impl AppPins {
    fn is_empty(&self) -> bool {
        self.pinned.is_empty() && self.hidden.is_empty()
    }

    /// Pins `item` last, and shows it again if it was hidden.
    fn pin(&mut self, item: &JumpItem) {
        let key = entry_key(item);
        self.hidden.retain(|hidden| *hidden != key);
        let pinned = { self.pinned.iter() }
            .filter_map(StoredItem::item)
            .any(|pinned| entry_key(&pinned) == key);
        if !pinned {
            self.pinned.push(StoredItem::of(item));
        }
    }

    /// Unpins `item`; one Explorer pinned (`by_explorer`) is hidden, as
    /// Explorer keeps it pinned.
    fn unpin(&mut self, item: &JumpItem, by_explorer: bool) {
        let key = entry_key(item);
        self.pinned
            .retain(|pinned| pinned.item().is_none_or(|kept| entry_key(&kept) != key));
        if by_explorer {
            self.hide(item);
        }
    }

    /// No longer shows `item`.
    fn hide(&mut self, item: &JumpItem) {
        let key = entry_key(item);
        if !self.hidden.contains(&key) {
            self.hidden.push(key);
        }
    }
}

/// Everything in `jump-pins.toml`: what Tessera keeps of each jump list,
/// by the identifier of the application.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PinsFile {
    #[serde(default)]
    app: BTreeMap<String, AppPins>,
}

/// `list` as Tessera shows it with what it keeps of it, `own`: its pins
/// first under Pinned, then those of Explorer it does not hide, each
/// once; the pinned entries and the hidden ones left out of the rest.
fn with_pins(list: &JumpList, own: Option<&AppPins>) -> JumpList {
    let Some(own) = own.filter(|own| !own.is_empty()) else {
        return list.clone();
    };
    let hidden: HashSet<&str> = own.hidden.iter().map(String::as_str).collect();
    let mut pinned: Vec<JumpItem> = own.pinned.iter().filter_map(StoredItem::item).collect();
    let mut pinned_keys: HashSet<String> = pinned.iter().map(entry_key).collect();
    for item in &list.pinned {
        let key = entry_key(item);
        if !hidden.contains(key.as_str()) && pinned_keys.insert(key) {
            pinned.push(item.clone());
        }
    }
    let shown = |items: &[JumpItem]| -> Vec<JumpItem> {
        { items.iter() }
            .filter(|item| {
                let key = entry_key(item);
                !hidden.contains(key.as_str()) && !pinned_keys.contains(&key)
            })
            .cloned()
            .collect()
    };
    JumpList {
        frequent: shown(&list.frequent),
        recent: shown(&list.recent),
        known: list.known.clone(),
        categories: { list.categories.iter() }
            .map(|(name, items)| (name.clone(), shown(items)))
            .filter(|(_, items)| !items.is_empty())
            .collect(),
        tasks: list.tasks.clone(),
        pinned,
    }
}

/// Where Tessera's pins stand with their file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Disk {
    #[default]
    Unread,
    /// Being read; what changes meanwhile is merged with what it holds.
    Reading,
    /// Read, or not there: it is written after every change.
    Read,
    /// There but unreadable, which was reported; it is not written over.
    Unusable,
}

/// The jump lists read so far, by the identifier of the application, and
/// the icons of their entries.
#[derive(Default)]
struct JumpLists {
    lists: HashMap<String, Arc<JumpList>>,
    /// The icons read, `None` for those that are being read or have none.
    icons: HashMap<IconKey, Icon>,
    /// The applications whose lists are being read.
    reading: HashSet<String>,
    /// Tessera's own pins, and how far their file was read.
    pins: PinsFile,
    disk: Disk,
    /// The applications whose pins changed while the file was being read.
    changed: HashSet<String>,
}

impl Global for JumpLists {}

/// Reads the jump list of the application `app_id` on a thread of its own, or
/// again where its files changed, for [`entries`] to give.
pub fn prefetch(app_id: &str, cx: &mut App) {
    read_pins(cx);
    if !cx
        .default_global::<JumpLists>()
        .reading
        .insert(app_id.to_owned())
    {
        return;
    }
    let id = app_id.to_owned();
    let read = ShellThread::call_alone("jump list", move || JumpList::cached(&id));
    let id = app_id.to_owned();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let list = read.recv().await;
        cx.update(|cx| {
            let lists = cx.global_mut::<JumpLists>();
            lists.reading.remove(&id);
            if let Ok(list) = list {
                lists.lists.insert(id, list.clone());
                read_icons(&list, cx);
            }
        })
        .ok();
    })
    .detach();
}

/// Has the file of Tessera's pins read aside, once.
fn read_pins(cx: &mut App) {
    let lists = cx.default_global::<JumpLists>();
    if lists.disk != Disk::Unread {
        return;
    }
    lists.disk = Disk::Reading;
    let read = cx.background_executor().spawn(async move {
        let there = config_dir().is_some_and(|directory| directory.join(PINS_FILE).exists());
        match there {
            true => read_user_file::<PinsFile>(PINS_FILE),
            false => Some(PinsFile::default()),
        }
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        let read = read.await;
        cx.update(|cx| {
            let lists = cx.global_mut::<JumpLists>();
            let Some(mut file) = read else {
                lists.disk = Disk::Unusable;
                return;
            };
            // What changed while the file was read is kept over it.
            let changed: Vec<String> = lists.changed.drain().collect();
            for app in &changed {
                match lists.pins.app.get(app) {
                    Some(pins) => file.app.insert(app.clone(), pins.clone()),
                    None => file.app.remove(app),
                };
            }
            lists.pins = file;
            lists.disk = Disk::Read;
            if !changed.is_empty() {
                write_later(lists.pins.clone(), cx);
            }
        })
        .ok();
    })
    .detach();
}

/// Changes what Tessera keeps of the jump list of `app_id` with `change`,
/// and writes the file aside once it was read.
fn change_pins(app_id: &str, cx: &mut App, change: impl FnOnce(&mut AppPins, &JumpList)) {
    let lists = cx.default_global::<JumpLists>();
    let list = lists.lists.get(app_id).cloned().unwrap_or_default();
    let pins = lists.pins.app.entry(app_id.to_owned()).or_default();
    change(pins, &list);
    if pins.is_empty() {
        lists.pins.app.remove(app_id);
    }
    match lists.disk {
        Disk::Read => write_later(lists.pins.clone(), cx),
        Disk::Unread | Disk::Reading => {
            lists.changed.insert(app_id.to_owned());
        }
        Disk::Unusable => report!("{PINS_FILE} could not be read, so what was pinned is not kept"),
    }
}

/// Writes `pins` on a thread of the background, unless what was asked
/// after it has been written already.
fn write_later(pins: PinsFile, cx: &App) {
    static ASKED: AtomicU64 = AtomicU64::new(0);
    static WRITTEN: Mutex<u64> = Mutex::new(0);
    let asked = ASKED.fetch_add(1, Ordering::Relaxed) + 1;
    cx.background_executor()
        .spawn(async move {
            let mut written = WRITTEN.lock().unwrap_or_else(PoisonError::into_inner);
            if *written > asked {
                return;
            }
            match write_pins(&pins) {
                Ok(()) => *written = asked,
                Err(error) => report!("{PINS_FILE} was not written: {error}"),
            }
        })
        .detach();
}

/// Writes `pins` to their file, through a file beside it, so that a file
/// cut short is never read.
fn write_pins(pins: &PinsFile) -> Result<(), String> {
    let directory = config_dir().ok_or("there is no configuration directory")?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let text = toml::to_string(pins).map_err(|error| error.to_string())?;
    let written = directory.join(format!("{PINS_FILE}.new"));
    fs::write(&written, text).map_err(|error| error.to_string())?;
    fs::rename(&written, directory.join(PINS_FILE)).map_err(|error| error.to_string())
}

/// Reads the icons of the entries of `list` that are not known yet.
fn read_icons(list: &JumpList, cx: &mut App) {
    let own = cx.global::<JumpLists>().pins.app.values();
    let pinned: Vec<JumpItem> = { own.flat_map(|pins| pins.pinned.iter()) }
        .filter_map(StoredItem::item)
        .collect();
    let lists = cx.global_mut::<JumpLists>();
    if lists.icons.len() > ICONS_KEPT {
        lists.icons.clear();
    }
    let mut wanted: Vec<(IconKey, JumpItem)> = Vec::new();
    let items = { sections(list).into_iter() }.flat_map(|section| section.items.iter());
    for item in items.chain(&pinned) {
        let key = icon_key(item);
        if !lists.icons.contains_key(&key) {
            lists.icons.insert(key.clone(), None);
            wanted.push((key, item.clone()));
        }
    }
    if wanted.is_empty() {
        return;
    }
    let read = ShellThread::call_alone("jump list icons", move || {
        { wanted.into_iter() }
            .map(|(key, item)| (key, item.icon_pixels(PICTURE_PIXELS)))
            .collect::<Vec<_>>()
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        let Ok(read) = read.recv().await else {
            return;
        };
        cx.update(|cx| {
            let icons = &mut cx.global_mut::<JumpLists>().icons;
            for (key, pixels) in read {
                icons.insert(key, pixels.as_ref().and_then(icon_of_pixels));
            }
        })
        .ok();
    })
    .detach();
}

/// What can be done to an entry of the menu, besides opening it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    Pin,
    Unpin,
    /// Taken off the record of Windows.
    Remove,
    /// No longer shown by Tessera.
    Hide,
}

impl Change {
    /// Does it to `item` of the jump list of `app_id`.
    fn apply(self, app_id: &str, item: &JumpItem, cx: &mut App) {
        match self {
            Self::Pin => change_pins(app_id, cx, |pins, _| pins.pin(item)),
            Self::Unpin => change_pins(app_id, cx, |pins, list| {
                let key = entry_key(item);
                let by_explorer = list.pinned.iter().any(|pinned| entry_key(pinned) == key);
                pins.unpin(item, by_explorer);
            }),
            Self::Remove => remove(app_id, item, cx),
            Self::Hide => change_pins(app_id, cx, |pins, _| pins.hide(item)),
        }
    }

    /// What is done to an entry of `section` with "Remove from this list",
    /// if anything.
    fn removal(section: &Section, item: &JumpItem) -> Option<Self> {
        match section.title {
            Title::Pinned | Title::Tasks => None,
            Title::Known(_) if item.removable() => Some(Self::Remove),
            Title::Known(_) | Title::Named(_) => Some(Self::Hide),
        }
    }
}

/// The entries of the jump list of the application `app_id`, as it was
/// read last, each section under its title, and after them the entries
/// that pin, unpin and take entries off; empty where it has none or it
/// was not read yet. A document opens with `program`, the program of the
/// application, or else the way the Shell opens it. The list is read
/// again meanwhile, for the next time.
pub fn entries(app_id: &str, program: Option<&Path>, cx: &mut App) -> Vec<MenuEntry> {
    prefetch(app_id, cx);
    let Some(read) = cx.default_global::<JumpLists>().lists.get(app_id).cloned() else {
        return Vec::new();
    };
    let lists = cx.global::<JumpLists>();
    let list = with_pins(&read, lists.pins.app.get(app_id));
    let language = Language::of(cx);
    let icons = &lists.icons;
    let mut entries = Vec::new();
    // The entries of "Pin to this list", "Unpin from this list" and
    // "Remove from this list".
    let mut changes: [Vec<MenuEntry>; 3] = Default::default();
    for section in sections(&list) {
        let title = match section.title {
            Title::Pinned => language.say("Pinned").to_owned(),
            Title::Known(KnownCategory::Frequent) => language.say("Frequent").to_owned(),
            Title::Known(KnownCategory::Recent) => language.say("Recent").to_owned(),
            Title::Named(name) => name.to_owned(),
            Title::Tasks => language.say("Tasks").to_owned(),
        };
        entries.push(MenuEntry::heading(title));
        for item in section.items {
            let picture = icons.get(&icon_key(item)).cloned().flatten();
            let open = {
                let item = item.clone();
                let program = program.map(Path::to_path_buf);
                move |cx: &mut App| {
                    if !path_of(&item).is_some_and(|path| Files::open_folder(path, cx)) {
                        item.open(program.as_deref());
                    }
                }
            };
            let doing = |change: Change| {
                let (app, item) = (app_id.to_owned(), item.clone());
                move |cx: &mut App| change.apply(&app, &item, cx)
            };
            let named = |glyph: &'static str, change: Change| {
                MenuEntry::new(glyph, item.name.clone(), doing(change))
                    .with_picture(picture.clone())
            };
            let entry = MenuEntry::new(section.glyph, item.name.clone(), open)
                .with_picture(picture.clone());
            let entry = match section.title {
                Title::Pinned => {
                    changes[1].push(named(ICON_UNPIN, Change::Unpin));
                    entry.with_aside(ICON_UNPIN, doing(Change::Unpin))
                }
                Title::Tasks => entry,
                _ => {
                    changes[0].push(named(ICON_PINNED, Change::Pin));
                    entry.with_aside(ICON_PINNED, doing(Change::Pin))
                }
            };
            if let Some(change) = Change::removal(&section, item) {
                changes[2].push(named(ICON_REMOVE, change));
            }
            entries.push(entry);
        }
    }
    let [pin, unpin, remove] = changes;
    let offered = [
        (ICON_PINNED, "Pin to this list", pin),
        (ICON_UNPIN, "Unpin from this list", unpin),
        (ICON_REMOVE, "Remove from this list", remove),
    ];
    for (glyph, label, items) in offered {
        if !items.is_empty() {
            entries.push(MenuEntry::submenu(glyph, language.say(label), items));
        }
    }
    entries
}

/// The path of `item` where it names something for an application to
/// open, which the file manager opens where it is a folder.
fn path_of(item: &JumpItem) -> Option<&Path> {
    match &item.target {
        JumpTarget::Item(path) => Some(path),
        JumpTarget::Command { .. } => None,
    }
}

/// Takes `item` off the jump list of `app_id`: at once from what is kept
/// here, and from the record of Windows on a thread of its own, as Windows
/// looks for the document, which may wait on the network. A failure is
/// reported.
fn remove(app_id: &str, item: &JumpItem, cx: &mut App) {
    let lists = &mut cx.default_global::<JumpLists>().lists;
    if let Some(list) = lists.get_mut(app_id) {
        let mut changed = JumpList::clone(list);
        changed.frequent.retain(|kept| kept != item);
        changed.recent.retain(|kept| kept != item);
        *list = Arc::new(changed);
    }
    let (app, taken) = (app_id.to_owned(), item.clone());
    let removed = ShellThread::call_alone("jump list removal", move || taken.remove(&app));
    let (app, name) = (app_id.to_owned(), item.name.clone());
    cx.spawn(async move |cx: &mut AsyncApp| {
        if let Ok(Err(error)) = removed.recv().await {
            report!("`{name}` stays in the jump list: {error:#}");
        }
        cx.update(|cx| prefetch(&app, cx)).ok();
    })
    .detach();
}

/// What a section of a jump list is titled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Title<'a> {
    Pinned,
    Known(KnownCategory),
    /// A category of the application's own, by its name.
    Named(&'a str),
    Tasks,
}

/// A section of a jump list as the menu shows it.
#[derive(Debug, PartialEq, Eq)]
struct Section<'a> {
    title: Title<'a>,
    /// The glyph of an entry that has no icon.
    glyph: &'static str,
    items: &'a [JumpItem],
    /// Whether its recorded entries can be taken off the list.
    removable: bool,
}

/// The sections of `list` the menu shows, in the order of the taskbar of
/// Windows: the pins, the known categories, the categories of the
/// application and the tasks, each that has entries, a long one cut to
/// [`SHOWN`] entries.
fn sections(list: &JumpList) -> Vec<Section<'_>> {
    let cut = |items: &[JumpItem]| -> usize { items.len().min(SHOWN) };
    let mut sections = vec![Section {
        title: Title::Pinned,
        glyph: ICON_PINNED,
        items: &list.pinned,
        removable: false,
    }];
    for known in &list.known {
        let items = list.of_known(*known);
        sections.push(Section {
            title: Title::Known(*known),
            glyph: match known {
                KnownCategory::Frequent => ICON_FREQUENT,
                KnownCategory::Recent => ICON_RECENT,
            },
            items: &items[..cut(items)],
            removable: true,
        });
    }
    for (name, items) in &list.categories {
        sections.push(Section {
            title: Title::Named(name),
            glyph: ICON_DOCUMENT,
            items: &items[..cut(items)],
            removable: false,
        });
    }
    sections.push(Section {
        title: Title::Tasks,
        glyph: ICON_TASK,
        items: &list.tasks,
        removable: false,
    });
    sections.retain(|section| !section.items.is_empty());
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, recorded: bool) -> JumpItem {
        JumpItem {
            name: name.to_owned(),
            target: JumpTarget::Item(PathBuf::from(format!(r"C:\{name}.txt"))),
            icon: None,
            recorded,
        }
    }

    fn titles<'a>(sections: &[Section<'a>]) -> Vec<(Title<'a>, usize)> {
        { sections.iter() }
            .map(|section| (section.title, section.items.len()))
            .collect()
    }

    fn names(items: &[JumpItem]) -> Vec<&str> {
        items.iter().map(|item| item.name.as_str()).collect()
    }

    #[test]
    fn the_sections_come_in_the_order_of_the_taskbar() {
        let many: Vec<JumpItem> = (0..12).map(|n| item(&n.to_string(), true)).collect();
        let list = JumpList {
            pinned: vec![item("pinned", true)],
            frequent: many.clone(),
            recent: vec![item("last", true)],
            known: vec![KnownCategory::Recent, KnownCategory::Frequent],
            categories: vec![
                ("Empty".to_owned(), Vec::new()),
                ("Media".to_owned(), vec![item("song", false)]),
            ],
            tasks: many,
        };
        let sections = sections(&list);
        assert_eq!(
            titles(&sections),
            [
                (Title::Pinned, 1),
                (Title::Known(KnownCategory::Recent), 1),
                (Title::Known(KnownCategory::Frequent), SHOWN),
                (Title::Named("Media"), 1),
                (Title::Tasks, 12),
            ]
        );
        let removable: Vec<bool> = sections.iter().map(|section| section.removable).collect();
        assert_eq!(removable, [false, true, true, false, false]);
        // A recorded document is taken off the record, an entry of the
        // application's own category only hidden, and a pin or a task
        // not offered.
        let removal: Vec<Option<Change>> = { sections.iter() }
            .map(|section| Change::removal(section, &section.items[0]))
            .collect();
        assert_eq!(
            removal,
            [
                None,
                Some(Change::Remove),
                Some(Change::Remove),
                Some(Change::Hide),
                None
            ]
        );
    }

    #[test]
    fn a_list_without_entries_has_no_sections() {
        let list = JumpList {
            recent: vec![item("hidden", true)],
            ..JumpList::default()
        };
        // The record is shown only in the known categories the list has.
        assert!(sections(&list).is_empty());
        assert!(sections(&JumpList::default()).is_empty());
    }

    #[test]
    fn entries_share_the_icon_they_ask_for() {
        let mut first = item("a", true);
        let second = item("b", true);
        assert_ne!(icon_key(&first), icon_key(&second));
        first.icon = Some((PathBuf::from(r"C:\app.exe"), 3));
        let task = JumpItem {
            name: "New window".to_owned(),
            target: JumpTarget::Command {
                program: PathBuf::from(r"C:\app.exe"),
                arguments: String::new(),
            },
            icon: None,
            recorded: false,
        };
        assert_eq!(icon_key(&first), (PathBuf::from(r"C:\app.exe"), 3));
        assert_eq!(icon_key(&task), (PathBuf::from(r"C:\app.exe"), 0));
    }

    #[test]
    fn tessera_pins_come_first_and_leave_the_rest() {
        let list = JumpList {
            pinned: vec![item("explorer-pin", true), item("both", true)],
            frequent: vec![item("often", true), item("mine", true)],
            recent: vec![item("last", true)],
            known: vec![KnownCategory::Recent],
            categories: vec![("Media".to_owned(), vec![item("song", false)])],
            tasks: vec![item("task", false)],
        };
        let mut own = AppPins::default();
        own.pin(&item("mine", true));
        own.pin(&item("both", true));
        own.pin(&item("mine", true));
        own.hide(&item("song", false));
        let shown = with_pins(&list, Some(&own));
        assert_eq!(names(&shown.pinned), ["mine", "both", "explorer-pin"]);
        assert_eq!(names(&shown.frequent), ["often"]);
        assert!(shown.categories.is_empty());
        assert_eq!(names(&shown.tasks), ["task"]);
        assert_eq!(with_pins(&list, None), list);

        // Unpinning one of Explorer's pins hides it; pinning it again
        // shows it.
        own.unpin(&item("explorer-pin", true), true);
        own.unpin(&item("mine", true), false);
        let shown = with_pins(&list, Some(&own));
        assert_eq!(names(&shown.pinned), ["both"]);
        assert_eq!(names(&shown.frequent), ["often", "mine"]);
        own.pin(&item("explorer-pin", true));
        assert!(!own.hidden.contains(&entry_key(&item("explorer-pin", true))));
    }

    #[test]
    fn the_pins_are_kept_as_toml_and_read_back() {
        let command = JumpItem {
            name: "Private window".to_owned(),
            target: JumpTarget::Command {
                program: PathBuf::from(r"C:\Browser\browser.exe"),
                arguments: "--private".to_owned(),
            },
            icon: Some((PathBuf::from(r"C:\Browser\browser.exe"), -3)),
            recorded: false,
        };
        let mut file = PinsFile::default();
        let pins = file.app.entry("Browser.App".to_owned()).or_default();
        pins.pin(&command);
        pins.pin(&item("notes", true));
        pins.hide(&item("song", false));
        let text = toml::to_string(&file).unwrap();
        let read: PinsFile = toml::from_str(&text).unwrap();
        assert_eq!(read, file);
        let items: Vec<JumpItem> = { read.app["Browser.App"].pinned.iter() }
            .filter_map(StoredItem::item)
            .collect();
        assert_eq!(items[0], command);
        assert_eq!(items[1].target, item("notes", true).target);
        assert!(toml::from_str::<PinsFile>("other = 1").is_err());
        assert_eq!(
            entry_key(&item("Notes", true)),
            entry_key(&item("notes", true))
        );
    }
}
