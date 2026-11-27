// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The menus of the desktop, in Tessera's style: the menu of the icons
//! chosen (Open, Open with, Cut, Copy, Create shortcut, Rename, Delete,
//! Properties, and the menu of the Shell as Windows draws it under "Show
//! more options"), and the menu of the desktop itself (how the icons are
//! shown and sorted, pasting, undoing, New, widgets and cards, the
//! wallpaper, the settings of the displays and of the look, a terminal,
//! and the menu of the Shell for the desktop folder).
//!
//! What only the Shell knows (the programs that open a file, the kinds of
//! new files) is asked when its submenu is opened, so that the menu opens
//! at once. The verbs the menu of the Shell hands back ([`Intercept`])
//! are done by the desktop itself, so that they stay in the undo journal.

use std::path::PathBuf;
use std::rc::Rc;

use async_channel::Receiver;
use gpui::{AnyWindowHandle, App, AsyncApp, Entity, Pixels, Point, Window};

use super::grid::{Arrangement, Spot};
use super::items::SortBy;
use super::{DesktopModel, IconSize};
use crate::actions::Actions;
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::module::WidgetRegistry;
use crate::panel::{Arranging, rewrite};
use crate::platform::{
    Intercept, Launch, MenuOutcome, NewTemplate, OpenWithHandler, RecycleBin, ShellMenu,
    UndoJournal, VirtualKind, choose_app, terminal,
};
use crate::popup::PopupAnchor;

/// Characters of the icon font.
const ICON_OPEN: &str = "\u{e8e5}";
const ICON_OPEN_WITH: &str = "\u{e7ac}";
const ICON_CUT: &str = "\u{e8c6}";
const ICON_COPY: &str = "\u{e8c8}";
const ICON_PASTE: &str = "\u{e77f}";
const ICON_LINK: &str = "\u{e71b}";
const ICON_RENAME: &str = "\u{e8ac}";
const ICON_DELETE: &str = "\u{e74d}";
const ICON_PROPERTIES: &str = "\u{e946}";
const ICON_MORE: &str = "\u{e712}";
const ICON_VIEW: &str = "\u{e890}";
const ICON_SORT: &str = "\u{e8cb}";
const ICON_REFRESH: &str = "\u{e72c}";
const ICON_UNDO: &str = "\u{e7a7}";
const ICON_REDO: &str = "\u{e7a6}";
const ICON_NEW: &str = "\u{e710}";
const ICON_FOLDER: &str = "\u{e8b7}";
const ICON_WIDGETS: &str = "\u{f0e2}";
const ICON_ARRANGE: &str = "\u{e8a9}";
const ICON_WALLPAPER: &str = "\u{e91b}";
const ICON_DISPLAY: &str = "\u{e7f4}";
const ICON_PERSONALIZE: &str = "\u{e771}";
const ICON_TERMINAL: &str = "\u{e756}";
const ICON_CHECK: &str = "\u{e73e}";
const ICON_NONE: &str = "";

/// The action of the wallpaper package that opens its settings, and the
/// one that goes on to the next picture, which stands in for it until
/// that package is there.
const WALLPAPER_SETTINGS: &str = "wallpaper-settings";
const WALLPAPER: &str = "wallpaper";

/// The widgets the menu offers to put on a card of their own, with what
/// the menu calls them; only those a module provides are offered.
const CARD_WIDGETS: [(&str, &str); 8] = [
    ("clock", "Clock"),
    ("calendar", "Calendar"),
    ("media", "Now playing"),
    ("cpu", "Processor"),
    ("memory", "Memory"),
    ("battery", "Battery"),
    ("network", "Network"),
    ("notifications", "Notifications"),
];

/// The glyph of an entry that is on, as a check mark.
fn checked(on: bool) -> &'static str {
    if on { ICON_CHECK } else { ICON_NONE }
}

/// An entry that opens the menu `entries` in place of the one it is in.
fn submenu(
    glyph: &'static str,
    label: &'static str,
    entries: Vec<MenuEntry>,
    anchor: PopupAnchor,
    language: Language,
) -> MenuEntry {
    later_submenu(glyph, label, anchor, language, move |_| {
        let (sent, entries_ready) = async_channel::bounded(1);
        sent.try_send(entries.clone()).ok();
        entries_ready
    })
}

/// An entry that opens a menu of what `entries` yields once it is asked
/// for, as what only the Shell knows takes a moment.
fn later_submenu(
    glyph: &'static str,
    label: &'static str,
    anchor: PopupAnchor,
    language: Language,
    entries: impl Fn(&mut App) -> Receiver<Vec<MenuEntry>> + 'static,
) -> MenuEntry {
    MenuEntry::new(glyph, language.say(label), move |cx| {
        let ready = entries(cx);
        // The menu that holds the entry closes once the click is handled.
        cx.spawn(async move |cx: &mut AsyncApp| {
            if let Ok(entries) = ready.recv().await {
                cx.update(|cx| Menus::open(entries, anchor, cx)).ok();
            }
        })
        .detach();
    })
}

/// Does `deed`, which needs the window of the icons, once the menu that
/// asked for it is gone.
fn after_menu(
    window: AnyWindowHandle,
    deed: impl FnOnce(&mut Window, &mut App) + 'static,
    cx: &mut App,
) {
    cx.defer(move |cx| {
        window.update(cx, |_, window, cx| deed(window, cx)).ok();
    });
}

/// Opens the menu of the icons chosen under `position` of `window`; with
/// `extended`, "Show more options" has the verbs shown with Shift held.
pub fn open_item_menu(
    model: &Entity<DesktopModel>,
    position: Point<Pixels>,
    extended: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let anchor = match PopupAnchor::under(window, position, cx) {
        Ok(anchor) => anchor,
        Err(error) => {
            report!("the menu has no place: {error:#}");
            return;
        }
    };
    let entries = item_entries(model, anchor, extended, window.window_handle(), cx);
    Menus::open(entries, anchor, cx);
}

/// The entries of the menu of the icons chosen.
fn item_entries(
    model: &Entity<DesktopModel>,
    anchor: PopupAnchor,
    extended: bool,
    window: AnyWindowHandle,
    cx: &mut App,
) -> Vec<MenuEntry> {
    let language = Language::of(cx);
    let weak = model.downgrade();
    let entry = |glyph: &'static str,
                 label: &'static str,
                 deed: fn(&mut DesktopModel, &mut gpui::Context<DesktopModel>)| {
        let model = weak.clone();
        MenuEntry::new(glyph, language.say(label), move |cx| {
            model.update(cx, deed).ok();
        })
    };
    let read = model.read(cx);
    let chosen = read.chosen();
    let paths = read.chosen_paths();
    let kinds: Vec<VirtualKind> = { chosen.iter() }
        .filter_map(|key| read.item(key)?.virtual_kind())
        .collect();
    let mut entries = vec![entry(ICON_OPEN, "Open", |model, cx| model.open_chosen(cx))];
    if let Some(path) = one_kind(&paths, read) {
        entries.push(open_with(path, weak.clone(), anchor, language));
    }
    if !paths.is_empty() {
        entries.push(entry(ICON_CUT, "Cut", |model, _| {
            model.put_on_clipboard(true)
        }));
        entries.push(entry(ICON_COPY, "Copy", |model, _| {
            model.put_on_clipboard(false)
        }));
    }
    entries.push(entry(ICON_LINK, "Create shortcut", |model, cx| {
        model.make_shortcuts(cx)
    }));
    if let [_] = paths.as_slice()
        && chosen.len() == 1
    {
        let model = weak.clone();
        entries.push(MenuEntry::new(
            ICON_RENAME,
            language.say("Rename"),
            move |cx| {
                let model = model.clone();
                after_menu(
                    window,
                    move |window, cx| {
                        model
                            .update(cx, |model, cx| model.rename_chosen(window, cx))
                            .ok();
                    },
                    cx,
                );
            },
        ));
    }
    if !paths.is_empty() {
        entries.push(entry(ICON_DELETE, "Delete", |model, cx| {
            model.delete(false, cx)
        }));
    }
    if kinds == [VirtualKind::RecycleBin] {
        entries.push(MenuEntry::new(
            ICON_DELETE,
            language.say("Empty the Recycle Bin"),
            |cx| {
                let emptied = RecycleBin::empty();
                cx.background_executor()
                    .spawn(async move {
                        if let Ok(Err(error)) = emptied.recv().await {
                            report!("the Recycle Bin was not emptied: {error}");
                        }
                    })
                    .detach();
            },
        ));
    }
    entries.push(entry(ICON_PROPERTIES, "Properties", |model, _| {
        model.properties()
    }));
    if !paths.is_empty() {
        let model = weak.clone();
        entries.push(MenuEntry::new(
            ICON_MORE,
            language.say("Show more options"),
            move |cx| {
                let built = ShellMenu::for_items(paths.clone(), extended);
                show_shell_menu(built, model.clone(), window, cx);
            },
        ));
    }
    entries
}

/// The first of `paths` where all are files of one kind, by extension,
/// which "Open with" is offered for.
fn one_kind(paths: &[PathBuf], model: &DesktopModel) -> Option<PathBuf> {
    let kind = |path: &PathBuf| {
        let folder = { model.items().iter() }
            .any(|item| item.path() == Some(path.as_path()) && item.is_folder());
        let extension = path.extension()?.to_string_lossy().to_lowercase();
        (!folder).then_some(extension)
    };
    let first = paths.first()?;
    let wanted = kind(first)?;
    { paths.iter() }
        .all(|path| kind(path).as_ref() == Some(&wanted))
        .then(|| first.clone())
}

/// "Open with": the programs that open files of the kind of `path`,
/// asked of the Shell when the submenu opens, and "Choose another app".
fn open_with(
    path: PathBuf,
    model: gpui::WeakEntity<DesktopModel>,
    anchor: PopupAnchor,
    language: Language,
) -> MenuEntry {
    later_submenu(ICON_OPEN_WITH, "Open with", anchor, language, move |cx| {
        let handlers = OpenWithHandler::of(&path);
        let (ready, entries) = async_channel::bounded(1);
        let (path, model) = (path.clone(), model.clone());
        cx.spawn(async move |_: &mut AsyncApp| {
            let handlers = handlers.recv().await.unwrap_or_default();
            let mut programs: Vec<MenuEntry> = { handlers.into_iter() }
                .map(|handler| {
                    let model = model.clone();
                    MenuEntry::new(ICON_OPEN_WITH, handler.name.clone(), move |cx| {
                        if let Ok(paths) = model.read_with(cx, |model, _| model.chosen_paths()) {
                            handler.open(paths);
                        }
                    })
                })
                .collect();
            programs.push(MenuEntry::new(
                ICON_OPEN_WITH,
                language.say("Choose another app"),
                move |_| choose_app(&path),
            ));
            ready.try_send(programs).ok();
        })
        .detach();
        entries
    })
}

/// Shows the menu of the Shell `built` as Windows draws it, once the menu
/// of Tessera is gone, and does what is chosen there.
fn show_shell_menu(
    built: Receiver<Result<ShellMenu, String>>,
    model: gpui::WeakEntity<DesktopModel>,
    window: AnyWindowHandle,
    cx: &mut App,
) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let menu = match built.recv().await {
            Ok(Ok(menu)) => Rc::new(menu),
            Ok(Err(error)) => {
                report!("the menu of the Shell is not shown: {error}");
                return;
            }
            Err(_) => return,
        };
        let shown = window.update(cx, |_, window, _| menu.show(None, Some(window)));
        let Ok(done) = shown else {
            return;
        };
        let outcome = done.recv().await;
        drop(menu);
        match outcome {
            Ok(Ok(MenuOutcome::Intercepted(intercept))) => {
                window
                    .update(cx, |_, window, cx| {
                        intercepted(intercept, &model, window, cx)
                    })
                    .ok();
            }
            Ok(Ok(MenuOutcome::Invoked { .. })) => {
                model.update(cx, |model, cx| model.reload(cx)).ok();
            }
            Ok(Ok(MenuOutcome::Dismissed)) => {}
            Ok(Err(error)) => report!("{error}"),
            Err(_) => report!("the thread of the Shell is gone"),
        }
    })
    .detach();
}

/// Does a verb the menu of the Shell handed back as the desktop's own
/// entry for it does, so that it is undone like that one.
fn intercepted(
    intercept: Intercept,
    model: &gpui::WeakEntity<DesktopModel>,
    window: &mut Window,
    cx: &mut App,
) {
    let handle = window.window_handle();
    model
        .update(cx, |model, cx| match intercept {
            Intercept::Rename => model.rename_chosen(window, cx),
            Intercept::NewFolder => model.new_item(None, None, handle, cx),
            Intercept::Delete => model.delete(false, cx),
            Intercept::Paste => model.paste(false, cx),
            Intercept::PasteLink => model.paste(true, cx),
            Intercept::Refresh => model.reload(cx),
            Intercept::Undo => model.undo(false, cx),
            Intercept::Redo => model.undo(true, cx),
            // The desktop's own View and Sort by stand for these.
            Intercept::View | Intercept::Sort | Intercept::Group => {}
        })
        .ok();
}

/// Opens the menu of the desktop under `position` of `window`; what is
/// made goes to `at`.
pub fn open_background_menu(
    model: &Entity<DesktopModel>,
    position: Point<Pixels>,
    at: Option<Spot>,
    extended: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let anchor = match PopupAnchor::under(window, position, cx) {
        Ok(anchor) => anchor,
        Err(error) => {
            report!("the menu has no place: {error:#}");
            return;
        }
    };
    let entries = background_entries(model, anchor, at, extended, window.window_handle(), cx);
    Menus::open(entries, anchor, cx);
}

/// The entries of the menu of the desktop.
fn background_entries(
    model: &Entity<DesktopModel>,
    anchor: PopupAnchor,
    at: Option<Spot>,
    extended: bool,
    window: AnyWindowHandle,
    cx: &mut App,
) -> Vec<MenuEntry> {
    let language = Language::of(cx);
    let weak = model.downgrade();
    let config = model.read(cx).config();
    let folder = model.read(cx).folder();
    let mut entries = vec![
        submenu(
            ICON_VIEW,
            "View",
            view_entries(config, language),
            anchor,
            language,
        ),
        submenu(
            ICON_SORT,
            "Sort by",
            sort_entries(&weak, config.sort, language),
            anchor,
            language,
        ),
    ];
    let refresh = weak.clone();
    entries.push(MenuEntry::new(
        ICON_REFRESH,
        language.say("Refresh"),
        move |cx| {
            refresh.update(cx, |model, cx| model.reload(cx)).ok();
        },
    ));
    if DesktopModel::can_paste() {
        for (glyph, label, link) in [
            (ICON_PASTE, "Paste", false),
            (ICON_LINK, "Paste shortcut", true),
        ] {
            let model = weak.clone();
            entries.push(MenuEntry::new(glyph, language.say(label), move |cx| {
                model
                    .update(cx, |model, cx| {
                        model.place_new_at(at);
                        model.paste(link, cx)
                    })
                    .ok();
            }));
        }
    }
    // Plain "Undo" and "Redo": the journal does not tell what it would
    // take back.
    for (glyph, label, again, can) in [
        (ICON_UNDO, "Undo", false, UndoJournal::can_undo()),
        (ICON_REDO, "Redo", true, UndoJournal::can_redo()),
    ] {
        if can {
            let model = weak.clone();
            entries.push(MenuEntry::new(glyph, language.say(label), move |cx| {
                model.update(cx, |model, cx| model.undo(again, cx)).ok();
            }));
        }
    }
    if folder.is_some() {
        entries.push(new_entries(weak.clone(), at, window, anchor, language));
    }
    let widgets = widget_entries(cx, language);
    if !widgets.is_empty() {
        entries.push(submenu(
            ICON_WIDGETS,
            "Add widgets",
            widgets,
            anchor,
            language,
        ));
    }
    entries.push(MenuEntry::new(
        ICON_ARRANGE,
        language.say("Arrange cards"),
        |cx| {
            // The menu closes first; arranging closes any menu.
            cx.defer(|cx| Arranging::set(true, cx));
        },
    ));
    entries.extend(wallpaper_entry(cx, language));
    entries.push(MenuEntry::new(
        ICON_DISPLAY,
        language.say("Display settings"),
        |_| Launch::new("ms-settings:display").start(),
    ));
    entries.push(MenuEntry::new(
        ICON_PERSONALIZE,
        language.say("Personalize"),
        |_| Launch::new("ms-settings:personalization").start(),
    ));
    if let Some(folder) = folder {
        let here = folder.clone();
        entries.push(MenuEntry::new(
            ICON_TERMINAL,
            language.say("Open in terminal"),
            move |cx| {
                let folder = here.clone();
                { cx.background_executor() }
                    .spawn(async move { terminal(Some(&folder), None, false).start() })
                    .detach();
            },
        ));
        let model = weak.clone();
        entries.push(MenuEntry::new(
            ICON_MORE,
            language.say("Show more options"),
            move |cx| {
                let built = ShellMenu::for_background(folder.clone(), extended);
                show_shell_menu(built, model.clone(), window, cx);
            },
        ));
    }
    entries
}

/// View: the sizes, the arrangement and whether the icons are shown, the
/// one in use checked.
fn view_entries(config: super::DesktopConfig, language: Language) -> Vec<MenuEntry> {
    let mut entries: Vec<MenuEntry> = { IconSize::ALL.into_iter() }
        .map(|size| {
            MenuEntry::new(
                checked(config.size == size),
                language.say(size.label()),
                move |cx| DesktopModel::set_size(size, cx),
            )
        })
        .collect();
    let auto = config.arrange == Arrangement::Auto;
    entries.push(MenuEntry::new(
        checked(auto),
        language.say("Auto arrange icons"),
        move |cx| {
            let to = if auto {
                Arrangement::Grid
            } else {
                Arrangement::Auto
            };
            DesktopModel::set_arrangement(to, cx)
        },
    ));
    let aligned = config.arrange != Arrangement::Free;
    entries.push(MenuEntry::new(
        checked(aligned),
        language.say("Align icons to grid"),
        move |cx| {
            let to = if aligned {
                Arrangement::Free
            } else {
                Arrangement::Grid
            };
            DesktopModel::set_arrangement(to, cx)
        },
    ));
    let shown = config.show;
    entries.push(MenuEntry::new(
        checked(shown),
        language.say("Show desktop icons"),
        move |cx| DesktopModel::set_shown(!shown, cx),
    ));
    entries
}

/// Sort by: puts the icons in that order once, and keeps sorting so.
fn sort_entries(
    model: &gpui::WeakEntity<DesktopModel>,
    current: SortBy,
    language: Language,
) -> Vec<MenuEntry> {
    { SortBy::ALL.into_iter() }
        .map(|sort| {
            let model = model.clone();
            MenuEntry::new(
                checked(current == sort),
                language.say(sort.label()),
                move |cx| {
                    model
                        .update(cx, |model, cx| model.arrange_by(sort, cx))
                        .ok();
                    if sort != current {
                        DesktopModel::set_sort(sort, cx);
                    }
                },
            )
        })
        .collect()
}

/// New: a folder, a shortcut and the kinds of files Windows offers, read
/// when the submenu opens.
fn new_entries(
    model: gpui::WeakEntity<DesktopModel>,
    at: Option<Spot>,
    window: AnyWindowHandle,
    anchor: PopupAnchor,
    language: Language,
) -> MenuEntry {
    later_submenu(ICON_NEW, "New", anchor, language, move |cx| {
        let (ready, entries) = async_channel::bounded(1);
        let model = model.clone();
        let templates = { cx.background_executor() }.spawn(async { NewTemplate::all() });
        cx.spawn(async move |_: &mut AsyncApp| {
            let made = { templates.await.into_iter() }
                .map(|template| {
                    let model = model.clone();
                    let glyph = if template.is_folder() {
                        ICON_FOLDER
                    } else {
                        ICON_NEW
                    };
                    MenuEntry::new(glyph, template.name.clone(), move |cx| {
                        let template = template.clone();
                        model
                            .update(cx, |model, cx| {
                                model.new_item(Some(template), at, window, cx)
                            })
                            .ok();
                    })
                })
                .collect();
            ready.try_send(made).ok();
        })
        .detach();
        entries
    })
}

/// Add widgets: a card of its own with one of the widgets for cards.
fn widget_entries(cx: &App, language: Language) -> Vec<MenuEntry> {
    let known = cx
        .try_global::<WidgetRegistry>()
        .map(WidgetRegistry::names)
        .unwrap_or_default();
    { CARD_WIDGETS.into_iter() }
        .filter(|(widget, _)| known.iter().any(|name| name == widget))
        .map(|(widget, label)| {
            MenuEntry::new(ICON_WIDGETS, language.say(label), move |cx| {
                rewrite(cx, |file| {
                    let Ok(config) = file.config() else {
                        return false;
                    };
                    let card = config.cards.len();
                    // A new card holds the clock; another widget takes
                    // its place.
                    file.add_card()
                        && (widget == "clock"
                            || (file.remove_from_card(card, 0) && file.add_to_card(card, widget)))
                });
            })
        })
        .collect()
}

/// The settings of the wallpaper where a package gives them, or else the
/// next picture of the wallpaper.
fn wallpaper_entry(cx: &App, language: Language) -> Option<MenuEntry> {
    let actions = cx.try_global::<Actions>()?;
    if actions.get(WALLPAPER_SETTINGS).is_some() {
        return Some(MenuEntry::new(
            ICON_WALLPAPER,
            language.say("Wallpaper settings"),
            |cx| Actions::run(WALLPAPER_SETTINGS, None, cx),
        ));
    }
    actions.get(WALLPAPER)?;
    Some(MenuEntry::new(
        ICON_WALLPAPER,
        language.say("Next wallpaper"),
        |cx| Actions::run(WALLPAPER, Some("next"), cx),
    ))
}
