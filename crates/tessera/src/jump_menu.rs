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
//! Pinning to a jump list has no documented interface, so no entry offers
//! it. Taking a recorded document off the list has one
//! (`IApplicationDestinations::RemoveDestination`), and each recorded
//! document that is not pinned has a button for it; a recorded shortcut
//! that starts a program with arguments has none, as it is not named to
//! Windows by a path.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, AsyncApp, Global};

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

const ICON_PINNED: &str = "\u{e718}";
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

/// The jump lists read so far, by the identifier of the application, and
/// the icons of their entries.
#[derive(Default)]
struct JumpLists {
    lists: HashMap<String, Arc<JumpList>>,
    /// The icons read, `None` for those that are being read or have none.
    icons: HashMap<IconKey, Icon>,
    /// The applications whose lists are being read.
    reading: HashSet<String>,
}

impl Global for JumpLists {}

/// Reads the jump list of the application `app_id` on a thread of its own, or
/// again where its files changed, for [`entries`] to give.
pub fn prefetch(app_id: &str, cx: &mut App) {
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

/// Reads the icons of the entries of `list` that are not known yet.
fn read_icons(list: &JumpList, cx: &mut App) {
    let lists = cx.global_mut::<JumpLists>();
    if lists.icons.len() > ICONS_KEPT {
        lists.icons.clear();
    }
    let mut wanted: Vec<(IconKey, JumpItem)> = Vec::new();
    for item in sections(list).iter().flat_map(|section| section.items) {
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

/// The entries of the jump list of the application `app_id`, as it was
/// read last, each section under its title; empty where it has none or it
/// was not read yet. A document opens with `program`, the program of the
/// application, or else the way the Shell opens it. The list is read again
/// meanwhile, for the next time.
pub fn entries(app_id: &str, program: Option<&Path>, cx: &mut App) -> Vec<MenuEntry> {
    prefetch(app_id, cx);
    let Some(list) = cx.default_global::<JumpLists>().lists.get(app_id).cloned() else {
        return Vec::new();
    };
    let language = Language::of(cx);
    let icons = &cx.global::<JumpLists>().icons;
    let mut entries = Vec::new();
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
            let entry =
                MenuEntry::new(section.glyph, item.name.clone(), open).with_picture(picture);
            entries.push(match section.removable && item.removable() {
                true => {
                    let (app, item) = (app_id.to_owned(), item.clone());
                    entry.with_aside(ICON_REMOVE, move |cx| remove(&app, &item, cx))
                }
                false => entry,
            });
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
}
