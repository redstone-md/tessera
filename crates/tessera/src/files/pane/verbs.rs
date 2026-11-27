// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the Shell adds to the menu of a pane, after Tessera's own
//! entries: the verbs of the kinds of files and those programs add, New
//! with the kinds of new files Windows offers, Open with, Send to, and
//! the menu as Windows draws it ("Show more options").
//!
//! The verbs Tessera has entries of its own for are left out, and those
//! the menu of the Shell hands back ([`Intercept`]) are done by the pane,
//! so that renaming, deleting, pasting and making a folder stay in the
//! undo journal. The Shell is asked on its own thread; where it does not
//! answer within [`SHELL_WAIT`], as with a shell extension that hangs,
//! the menu opens without what it would have added.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use async_channel::Receiver;
use gpui::{App, AsyncApp, Context, WeakEntity};

use super::{FolderPane, ICON_NEW_FOLDER};
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::platform::{
    FileDeed, Intercept, MenuOutcome, NewTemplate, OpenWithHandler, SendToTarget, ShellMenu,
    ShellVerb, UndoJournal, choose_app,
};
use crate::popup::PopupAnchor;

/// How long the Shell is waited for before the menu opens without it.
const SHELL_WAIT: Duration = Duration::from_millis(1500);

/// Characters of the icon font.
const ICON_NEW: &str = "\u{e710}";
const ICON_OPEN_WITH: &str = "\u{e7ac}";
const ICON_SEND_TO: &str = "\u{e724}";
const ICON_MORE: &str = "\u{e712}";
const ICON_VERB: &str = "";

/// The verbs of the menu of the Shell that Tessera's own entries stand
/// for, which are left out of it, by their names that do not change with
/// the language.
const OWN_VERBS: [&str; 23] = [
    "open",
    "opennewwindow",
    "opennewtab",
    "explore",
    "cut",
    "copy",
    "copyaspath",
    "delete",
    "rename",
    "properties",
    "paste",
    "pastelink",
    "newfolder",
    "refresh",
    "view",
    "arrange",
    "sortby",
    "groupby",
    "undo",
    "redo",
    "openas",
    "pintohome",
    "unpinfromhome",
];

/// What the Shell offers for the menu of a pane, as far as it answered.
#[derive(Default)]
pub(super) struct ShellChoices {
    /// The menu of the Shell, kept on its thread while this lives.
    menu: Option<Rc<ShellMenu>>,
    /// The programs that open the files chosen, all of one kind.
    open_with: Vec<OpenWithHandler>,
    send_to: Vec<SendToTarget>,
    /// The kinds of new items, for the menu of the folder shown.
    templates: Vec<NewTemplate>,
}

impl ShellChoices {
    /// Asks the Shell, away from the user interface, what it offers for
    /// `chosen`, with the programs that open `open_with` where they are
    /// all files of its kind, or for the background of `folder` where
    /// nothing is chosen; `extended` asks for the verbs shown with Shift
    /// held.
    pub(super) async fn read(
        chosen: Vec<PathBuf>,
        open_with: Option<PathBuf>,
        folder: Option<PathBuf>,
        extended: bool,
        cx: &AsyncApp,
    ) -> Self {
        let until = Instant::now() + SHELL_WAIT;
        let mut choices = Self::default();
        if chosen.is_empty() {
            let Some(folder) = folder else {
                return choices;
            };
            let built = ShellMenu::for_background(folder, extended);
            let templates = { cx.background_executor() }.spawn(async { NewTemplate::all() });
            choices.menu = within(built, until, cx).await.and_then(shell_menu);
            choices.templates = templates.await;
            choices.templates.retain(|template| !template.is_folder());
            return choices;
        }
        let built = ShellMenu::for_items(chosen.clone(), extended);
        let handlers = open_with.as_deref().map(OpenWithHandler::of);
        let send_to = SendToTarget::all();
        choices.menu = within(built, until, cx).await.and_then(shell_menu);
        if let Some(handlers) = handlers {
            choices.open_with = within(handlers, until, cx).await.unwrap_or_default();
        }
        choices.send_to = within(send_to, until, cx).await.unwrap_or_default();
        choices
    }
}

/// The menu of the Shell that was built, or nothing, reporting why.
fn shell_menu(built: Result<ShellMenu, String>) -> Option<Rc<ShellMenu>> {
    match built {
        Ok(menu) => Some(Rc::new(menu)),
        Err(error) => {
            report!("the menu of the Shell is not shown: {error}");
            None
        }
    }
}

/// What `answer` yields before `until`; `None` once that passed, or where
/// it yields nothing.
async fn within<T: Send + 'static>(
    answer: Receiver<T>,
    until: Instant,
    cx: &AsyncApp,
) -> Option<T> {
    let (first, taken) = async_channel::bounded(1);
    let executor = cx.background_executor();
    let answered = first.clone();
    executor
        .spawn(async move {
            if let Ok(value) = answer.recv().await {
                answered.try_send(Some(value)).ok();
            }
        })
        .detach();
    let timer = executor.timer(until.saturating_duration_since(Instant::now()));
    executor
        .spawn(async move {
            timer.await;
            first.try_send(None).ok();
        })
        .detach();
    taken.recv().await.ok().flatten()
}

/// The first of `items`, each a path and whether it is a folder, where
/// all are files of one kind, by extension, which "Open with" is offered
/// for.
pub(super) fn one_kind(items: &[(PathBuf, bool)]) -> Option<PathBuf> {
    let kind = |(path, folder): &(PathBuf, bool)| {
        let extension = path.extension()?.to_string_lossy().to_lowercase();
        (!folder).then_some(extension)
    };
    let first = items.first()?;
    let wanted = kind(first)?;
    { items.iter() }
        .all(|item| kind(item).as_ref() == Some(&wanted))
        .then(|| first.0.clone())
}

/// Whether `verb` of the menu of the Shell is shown in Tessera's menu:
/// not a separator, nor something Tessera has an entry of its own for,
/// nor what cannot be drawn in Tessera's style.
fn shown(verb: &ShellVerb) -> bool {
    if verb.separator || verb.disabled || verb.label.trim().is_empty() {
        return false;
    }
    let own = { verb.verb.as_deref() }
        .is_some_and(|name| OWN_VERBS.iter().any(|own| own.eq_ignore_ascii_case(name)));
    if own || verb.intercept == Some(Intercept::NewFolder) {
        return false;
    }
    // A submenu the Shell fills only when Windows draws it, as Send to
    // and Open with are, has nothing to list here; New is Tessera's own.
    match verb.submenu {
        true => {
            let new =
                { verb.children.iter() }.any(|child| child.intercept == Some(Intercept::NewFolder));
            !new && verb.children.iter().any(shown)
        }
        false => true,
    }
}

/// An entry that opens the menu `entries` in place of the one it is in.
fn submenu(
    glyph: &'static str,
    label: &str,
    entries: Vec<MenuEntry>,
    anchor: PopupAnchor,
) -> MenuEntry {
    MenuEntry::new(glyph, label.to_owned(), move |cx| {
        let entries = entries.clone();
        // The menu that holds the entry closes once the click is handled.
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.update(|cx| Menus::open(entries, anchor, cx)).ok();
        })
        .detach();
    })
}

impl FolderPane {
    /// The entries the Shell adds after Tessera's own: New where nothing
    /// is chosen, Open with and Send to for what is, the verbs of the
    /// Shell, and "Show more options".
    pub(super) fn shell_entries(
        &self,
        choices: ShellChoices,
        anchor: PopupAnchor,
        cx: &mut Context<Self>,
    ) -> Vec<MenuEntry> {
        let language = Language::of(cx);
        let view = cx.entity().downgrade();
        let mut entries = Vec::new();
        if !choices.templates.is_empty() {
            let kinds = { choices.templates.into_iter() }
                .map(|template| {
                    let view = view.clone();
                    let label = template.name.clone();
                    MenuEntry::new(ICON_NEW_FOLDER, label, move |cx| {
                        let template = template.clone();
                        view.update(cx, |this, cx| this.new_item(Some(template), cx))
                            .ok();
                    })
                })
                .collect();
            entries.push(submenu(ICON_NEW, language.say("New"), kinds, anchor));
        }
        if !choices.open_with.is_empty() {
            let mut programs: Vec<MenuEntry> = { choices.open_with.into_iter() }
                .map(|handler| {
                    let view = view.clone();
                    MenuEntry::new(ICON_OPEN_WITH, handler.name.clone(), move |cx| {
                        if let Ok(chosen) = view.read_with(cx, |this, _| this.chosen()) {
                            handler.open(chosen);
                        }
                    })
                })
                .collect();
            let other = view.clone();
            programs.push(MenuEntry::new(
                ICON_OPEN_WITH,
                language.say("Choose another app"),
                move |cx| {
                    let chosen = other.read_with(cx, |this, _| this.chosen());
                    if let Some(path) = chosen.ok().and_then(|chosen| chosen.into_iter().next()) {
                        choose_app(&path);
                    }
                },
            ));
            entries.push(submenu(
                ICON_OPEN_WITH,
                language.say("Open with"),
                programs,
                anchor,
            ));
        }
        if !choices.send_to.is_empty() {
            let targets = { choices.send_to.into_iter() }
                .map(|target| {
                    let view = view.clone();
                    MenuEntry::new(ICON_SEND_TO, target.name.clone(), move |cx| {
                        Self::send_to(&target, view.clone(), cx);
                    })
                })
                .collect();
            entries.push(submenu(
                ICON_SEND_TO,
                language.say("Send to"),
                targets,
                anchor,
            ));
        }
        if let Some(menu) = choices.menu {
            entries.extend(verb_entries(menu.verbs(), &menu, &view, anchor));
            let more = view.clone();
            entries.push(MenuEntry::new(
                ICON_MORE,
                language.say("Show more options"),
                move |cx| {
                    let menu = menu.clone();
                    more.update(cx, |this, cx| this.show_shell_menu(menu, cx))
                        .ok();
                },
            ));
        }
        entries
    }

    /// Sends what is chosen to `target`, and reads the folder again once
    /// it is there.
    fn send_to(target: &SendToTarget, view: WeakEntity<Self>, cx: &mut App) {
        let Ok(chosen) = view.read_with(cx, |this, _| this.chosen()) else {
            return;
        };
        let sent = target.send(chosen);
        cx.spawn(async move |cx: &mut AsyncApp| {
            if let Ok(Err(error)) = sent.recv().await {
                report!("the files were not sent: {error}");
            }
            view.update(cx, |this, cx| this.load(cx)).ok();
        })
        .detach();
    }

    /// Shows `menu` as Windows draws it, once the menu of Tessera is gone,
    /// and does what is chosen there.
    fn show_shell_menu(&mut self, menu: Rc<ShellMenu>, cx: &mut Context<Self>) {
        self.after_menu(cx, move |_, window, cx| {
            let shown = menu.show(None, Some(window));
            Self::outcome(shown, menu, cx);
        });
    }

    /// Does what came of a verb of the menu of the Shell, which `menu`
    /// is kept alive for meanwhile: the pane does an intercepted verb
    /// itself, and reads the folder after any other.
    fn outcome(
        done: Receiver<Result<MenuOutcome, String>>,
        menu: Rc<ShellMenu>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome = done.recv().await;
            drop(menu);
            this.update(cx, |this, cx| match outcome {
                Ok(Ok(MenuOutcome::Intercepted(intercept))) => this.intercepted(intercept, cx),
                Ok(Ok(MenuOutcome::Invoked { .. })) => this.load(cx),
                Ok(Ok(MenuOutcome::Dismissed)) => {}
                Ok(Err(error)) => report!("{error}"),
                Err(_) => report!("the thread of the Shell is gone"),
            })
            .ok();
        })
        .detach();
    }

    /// Does a verb the menu of the Shell handed back, as the entry of
    /// Tessera for it does, so that it is undone like that one.
    fn intercepted(&mut self, intercept: Intercept, cx: &mut Context<Self>) {
        match intercept {
            Intercept::Rename => {
                self.after_menu(cx, |this, window, cx| this.rename_chosen(window, cx));
            }
            Intercept::NewFolder => self.new_item(None, cx),
            Intercept::Delete => self.delete(false, cx),
            Intercept::Paste => self.paste(cx),
            Intercept::Refresh => self.load(cx),
            Intercept::Undo => self.undo(false, cx),
            Intercept::Redo => self.undo(true, cx),
            // What only Explorer's own view of a folder does; Tessera's
            // entries for the look and the order stand for them.
            Intercept::PasteLink | Intercept::View | Intercept::Sort | Intercept::Group => {}
        }
    }

    /// Makes a new item of the kind `template`, or a folder where it is
    /// none, in the folder shown, away from the user interface, and starts
    /// renaming it, as New does in Explorer. It goes into the undo
    /// journal where it can be made again.
    pub(super) fn new_item(&mut self, template: Option<NewTemplate>, cx: &mut Context<Self>) {
        let Some(folder) = self.folder() else {
            return;
        };
        let new = Language::of(cx).say("New");
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let made = { cx.background_executor() }.spawn(async move {
                let template = match template {
                    Some(template) => template,
                    // There is always a folder, as Windows offers it where
                    // its key is missing.
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
            this.update(cx, |this, cx| {
                this.load(cx);
                this.selection.clear();
                this.selection.chosen.insert(path.clone());
                cx.notify();
                if template.made_by_command() {
                    return;
                }
                let name = { path.file_name() }.map(|name| name.to_string_lossy().into_owned());
                this.after_menu(cx, move |this, window, cx| {
                    this.begin_rename(path, name.unwrap_or_default(), window, cx)
                });
            })
            .ok();
        })
        .detach();
    }
}

/// The entries of `verbs` of `menu` that Tessera's menu shows: an entry
/// for a verb, and one that opens a menu of its own for a submenu.
fn verb_entries(
    verbs: &[ShellVerb],
    menu: &Rc<ShellMenu>,
    view: &WeakEntity<FolderPane>,
    anchor: PopupAnchor,
) -> Vec<MenuEntry> {
    { verbs.iter().filter(|verb| shown(verb)) }
        .map(|verb| {
            if verb.submenu {
                let children = verb_entries(&verb.children, menu, view, anchor);
                return submenu(ICON_VERB, &verb.label, children, anchor);
            }
            let (menu, view, id) = (menu.clone(), view.clone(), verb.id);
            MenuEntry::new(ICON_VERB, verb.label.clone(), move |cx| {
                let menu = menu.clone();
                view.update(cx, |_, cx| {
                    let done = menu.invoke(id, None);
                    FolderPane::outcome(done, menu, cx);
                })
                .ok();
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verb(label: &str, name: Option<&str>) -> ShellVerb {
        ShellVerb {
            id: 1,
            label: label.to_owned(),
            verb: name.map(str::to_owned),
            intercept: name.and_then(Intercept::of),
            ..ShellVerb::default()
        }
    }

    #[test]
    fn the_verbs_tessera_has_entries_for_are_left_out() {
        assert!(!shown(&verb("Open", Some("open"))));
        assert!(!shown(&verb("Rename", Some("Rename"))));
        assert!(!shown(&verb("Properties", Some("properties"))));
        assert!(shown(&verb("Run as administrator", Some("runas"))));
        assert!(shown(&verb("Scan with Defender", None)));
        assert!(!shown(&verb("", None)));
        let separator = ShellVerb {
            separator: true,
            ..ShellVerb::default()
        };
        assert!(!shown(&separator));
    }

    #[test]
    fn a_submenu_is_shown_with_something_in_it_and_new_is_not() {
        let mut empty = verb("Send to", None);
        empty.submenu = true;
        assert!(!shown(&empty));
        let mut archive = verb("7-Zip", None);
        archive.submenu = true;
        archive.children = vec![verb("Add to archive", None)];
        assert!(shown(&archive));
        let mut new = verb("New", None);
        new.submenu = true;
        new.children = vec![
            verb("Folder", Some("NewFolder")),
            verb("Text Document", None),
        ];
        assert!(!shown(&new));
    }

    #[test]
    fn open_with_is_offered_for_files_of_one_kind() {
        let items = |names: &[(&str, bool)]| -> Vec<(PathBuf, bool)> {
            { names.iter() }
                .map(|&(name, folder)| (PathBuf::from(format!(r"C:\missing\{name}")), folder))
                .collect()
        };
        let same = items(&[("a.txt", false), ("b.TXT", false)]);
        assert_eq!(one_kind(&same), Some(same[0].0.clone()));
        assert_eq!(one_kind(&items(&[("a.txt", false), ("b.md", false)])), None);
        assert_eq!(one_kind(&items(&[("a.txt", false), ("c.txt", true)])), None);
        assert_eq!(one_kind(&items(&[("README", false)])), None);
        assert_eq!(one_kind(&[]), None);
    }
}
