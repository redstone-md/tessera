// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The menus of the desktop, in Tessera's style: the menu of the icons
//! chosen (Open, Open with, Cut, Copy, Create shortcut, Rename, Delete,
//! Properties, Send to, the verbs of the Shell and the menu of the Shell
//! as Windows draws it under "Show more options"), and the menu of the
//! desktop itself (how the icons are shown and sorted, pasting, undoing,
//! New, widgets and cards, the wallpaper, the settings of the displays
//! and of the look, a terminal, and the verbs and the menu of the Shell
//! for the desktop folder).
//!
//! What the Shell adds is built by [`crate::shell_entries`], as for the
//! file manager: the menu opens once the Shell answered, or without it
//! after a moment. The verbs the menu of the Shell hands back
//! ([`Intercept`](crate::platform::Intercept)) are done by the desktop
//! itself, so that they stay in the undo journal.

use std::path::PathBuf;

use gpui::{AnyWindowHandle, App, AsyncApp, Context, Entity, Pixels, Point, Window};

use super::grid::{Arrangement, Spot};
use super::items::SortBy;
use super::{DesktopModel, IconSize};
use crate::actions::Actions;
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::module::WidgetRegistry;
use crate::panel::{Arranging, rewrite};
use crate::platform::{Launch, NewTemplate, RecycleBin, UndoJournal, VirtualKind, terminal};
use crate::popup::PopupAnchor;
use crate::shell_entries::{FileMenu, MenuTarget, ShellChoices, ShellEntries, one_kind, submenu};

/// Characters of the icon font.
const ICON_OPEN: &str = "\u{e8e5}";
const ICON_CUT: &str = "\u{e8c6}";
const ICON_COPY: &str = "\u{e8c8}";
const ICON_PASTE: &str = "\u{e77f}";
const ICON_LINK: &str = "\u{e71b}";
const ICON_SHARE: &str = "\u{e72d}";
const ICON_RENAME: &str = "\u{e8ac}";
const ICON_DELETE: &str = "\u{e74d}";
const ICON_PROPERTIES: &str = "\u{e946}";
const ICON_VIEW: &str = "\u{e890}";
const ICON_SORT: &str = "\u{e8cb}";
const ICON_REFRESH: &str = "\u{e72c}";
const ICON_UNDO: &str = "\u{e7a7}";
const ICON_REDO: &str = "\u{e7a6}";
const ICON_WIDGETS: &str = "\u{f0e2}";
const ICON_ARRANGE: &str = "\u{e8a9}";
const ICON_WALLPAPER: &str = "\u{e91b}";
const ICON_DISPLAY: &str = "\u{e7f4}";
const ICON_PERSONALIZE: &str = "\u{e771}";
const ICON_TERMINAL: &str = "\u{e756}";
const ICON_CHECK: &str = "\u{e73e}";
const ICON_NONE: &str = "";

/// The action that opens the settings of the wallpaper, and the one that
/// goes on to the next picture, which stands in for it where the settings
/// are not registered.
const WALLPAPER_SETTINGS: &str = "wallpaper-settings";
const WALLPAPER: &str = "wallpaper";

/// The action that opens the settings of the displays.
const DISPLAY_SETTINGS: &str = "display-settings";

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

impl FileMenu for DesktopModel {
    /// Where the menu of the desktop was opened, where new icons go.
    type Place = Option<Spot>;

    const OWN_NEW_FOLDER: bool = false;

    /// Create shortcut, and Empty the Recycle Bin of the bin's own menu.
    const OWN_VERBS: &'static [&'static str] = &["link", "empty"];

    fn chosen_files(&self) -> Vec<PathBuf> {
        self.chosen_paths()
    }

    fn make_new(
        &mut self,
        template: Option<NewTemplate>,
        place: &Option<Spot>,
        window: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        self.new_item(template, *place, window, cx);
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rename_chosen(window, cx);
    }

    fn delete_chosen(&mut self, cx: &mut Context<Self>) {
        self.delete(false, cx);
    }

    fn paste_here(&mut self, link: bool, place: &Option<Spot>, cx: &mut Context<Self>) {
        self.place_new_at(*place);
        self.paste(link, cx);
    }

    fn read_again(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
    }

    fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
    }

    fn take_back(&mut self, again: bool, cx: &mut Context<Self>) {
        self.undo(again, cx);
    }
}

/// Asks the Shell what it adds for `chosen`, or for the background of
/// `folder` where nothing is chosen, and opens the menu once it answered
/// or the wait is over, with what `arrange` makes of the desktop's own
/// entries and those of the Shell.
fn open_with_shell(
    target: MenuTarget<DesktopModel>,
    (chosen, open_with, folder): (Vec<PathBuf>, Option<PathBuf>, Option<PathBuf>),
    extended: bool,
    arrange: impl FnOnce(ShellEntries) -> Vec<MenuEntry> + 'static,
    cx: &mut App,
) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let choices = ShellChoices::read(chosen, open_with, folder, extended, cx).await;
        cx.update(|cx| {
            let entries = arrange(target.entries(choices, Language::of(cx)));
            Menus::open(entries, target.anchor, cx);
        })
        .ok();
    })
    .detach();
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
    let handle = window.window_handle();
    let mut own = item_entries(model, handle, cx);
    let read = model.read(cx);
    let chosen = read.chosen();
    let kinds: Vec<(PathBuf, bool)> = { chosen.iter() }
        .filter_map(|key| {
            let item = read.item(key)?;
            Some((item.path()?.to_owned(), item.is_folder()))
        })
        .collect();
    let open_with = one_kind(&kinds);
    // The Shell's own verbs, of the files and of the items of the Shell
    // (This PC, the Recycle Bin...) by the names it parses them by.
    let virtuals = { chosen.iter() }
        .filter_map(|key| read.item(key)?.virtual_kind())
        .map(|kind| PathBuf::from(kind.parsing_name()));
    let shell_items: Vec<PathBuf> = { kinds.into_iter().map(|(path, _)| path) }
        .chain(virtuals)
        .collect();
    let target = MenuTarget {
        host: model.downgrade(),
        window: handle,
        place: None,
        anchor,
    };
    if shell_items.is_empty() {
        Menus::open(own, anchor, cx);
        return;
    }
    let asked = (shell_items, open_with, None);
    open_with_shell(
        target,
        asked,
        extended,
        move |shell| {
            // Open with goes right after Open, as in Explorer.
            let at = own.len().min(1);
            own.splice(at..at, shell.open_with);
            own.extend(shell.rest);
            own
        },
        cx,
    );
}

/// The desktop's own entries of the menu of the icons chosen.
fn item_entries(
    model: &Entity<DesktopModel>,
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
    if !paths.is_empty() {
        entries.push(entry(ICON_CUT, "Cut", |model, _| {
            model.put_on_clipboard(true)
        }));
        entries.push(entry(ICON_COPY, "Copy", |model, _| {
            model.put_on_clipboard(false)
        }));
        entries.push(entry(ICON_SHARE, "Share", |model, cx| {
            crate::files::Files::share(model.chosen_paths(), cx)
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
    entries
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
    let handle = window.window_handle();
    let (mut own, after_new) = background_entries(model, anchor, at, cx);
    let target = MenuTarget {
        host: model.downgrade(),
        window: handle,
        place: at,
        anchor,
    };
    let Some(folder) = model.read(cx).folder() else {
        own.extend(after_new);
        Menus::open(own, anchor, cx);
        return;
    };
    open_with_shell(
        target,
        (Vec::new(), None, Some(folder)),
        extended,
        move |shell| {
            own.extend(shell.new);
            own.extend(after_new);
            own.extend(shell.rest);
            own
        },
        cx,
    );
}

/// The desktop's own entries of the menu of the desktop: those before
/// New, and those after it.
fn background_entries(
    model: &Entity<DesktopModel>,
    anchor: PopupAnchor,
    at: Option<Spot>,
    cx: &mut App,
) -> (Vec<MenuEntry>, Vec<MenuEntry>) {
    let language = Language::of(cx);
    let weak = model.downgrade();
    let config = model.read(cx).config();
    let folder = model.read(cx).folder();
    let mut entries = vec![
        submenu(
            ICON_VIEW,
            language.say("View"),
            view_entries(config, language),
            anchor,
        ),
        submenu(
            ICON_SORT,
            language.say("Sort by"),
            sort_entries(&weak, config.sort, language),
            anchor,
        ),
    ];
    let refresh = weak.clone();
    entries.push(MenuEntry::new(
        ICON_REFRESH,
        language.say("Refresh"),
        move |cx| {
            refresh.update(cx, |model, cx| model.refresh(cx)).ok();
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
    // Undo and redo say what they take back, as Explorer's do.
    let undo = UndoJournal::next_undo().map(|label| (ICON_UNDO, label, false));
    let redo = UndoJournal::next_redo().map(|label| (ICON_REDO, label, true));
    for (glyph, label, again) in undo.into_iter().chain(redo) {
        let model = weak.clone();
        entries.push(MenuEntry::new(glyph, language.say(label), move |cx| {
            model.update(cx, |model, cx| model.undo(again, cx)).ok();
        }));
    }
    let mut after_new = Vec::new();
    let widgets = widget_entries(cx, language);
    if !widgets.is_empty() {
        after_new.push(submenu(
            ICON_WIDGETS,
            language.say("Add widgets"),
            widgets,
            anchor,
        ));
    }
    after_new.push(MenuEntry::new(
        ICON_ARRANGE,
        language.say("Arrange cards"),
        |cx| {
            // The menu closes first; arranging closes any menu.
            cx.defer(|cx| Arranging::set(true, cx));
        },
    ));
    after_new.extend(wallpaper_entry(cx, language));
    after_new.push(MenuEntry::new(
        ICON_DISPLAY,
        language.say("Display settings"),
        // Tessera's own popup, where it is there, as Windows' page needs
        // the Settings app.
        |cx| match cx
            .try_global::<Actions>()
            .and_then(|actions| actions.get(DISPLAY_SETTINGS))
        {
            Some(_) => Actions::run(DISPLAY_SETTINGS, None, cx),
            None => Launch::new("ms-settings:display").start(),
        },
    ));
    after_new.push(MenuEntry::new(
        ICON_PERSONALIZE,
        language.say("Personalize"),
        |_| Launch::new("ms-settings:personalization").start(),
    ));
    if let Some(folder) = folder {
        after_new.push(MenuEntry::new(
            ICON_TERMINAL,
            language.say("Open in terminal"),
            move |cx| {
                let folder = folder.clone();
                { cx.background_executor() }
                    .spawn(async move { terminal(Some(&folder), None, false).start() })
                    .detach();
            },
        ));
    }
    (entries, after_new)
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
