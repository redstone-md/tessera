// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the Shell adds to a menu of files in Tessera's style, shared by
//! the menu of a pane of the file manager and the menus of the icons of
//! the desktop: New with the kinds of new files Windows offers, Open with,
//! Send to, the verbs of the kinds of files and those programs add (7-Zip,
//! Git...), and the menu as Windows draws it ("Show more options").
//!
//! The verbs Tessera has entries of its own for are left out, and those
//! the menu of the Shell hands back ([`Intercept`]) are done by what the
//! menu is about ([`FileMenu`]), so that renaming, deleting, pasting and
//! making a folder stay in the undo journal. The Shell is asked on its own
//! thread; where it does not answer within [`SHELL_WAIT`], as with a shell
//! extension that hangs, the menu opens without what it would have added.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use async_channel::Receiver;
use gpui::{AnyWindowHandle, App, AsyncApp, Context, WeakEntity, Window};

use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::platform::{
    Intercept, MenuOutcome, NewTemplate, OpenWithHandler, SendToTarget, ShellMenu, ShellVerb,
    choose_app,
};
use crate::popup::PopupAnchor;

/// How long the Shell is waited for before the menu opens without it.
pub const SHELL_WAIT: Duration = Duration::from_millis(1500);

/// Characters of the icon font.
const ICON_NEW: &str = "\u{e710}";
const ICON_FOLDER: &str = "\u{e8b7}";
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

/// What a menu of files is about, which does what the Shell hands back:
/// a pane of the file manager or the icons of the desktop.
pub trait FileMenu: Sized + 'static {
    /// Where the menu was opened, for what it makes there: nothing for a
    /// pane, the place under the pointer on the desktop.
    type Place: Clone + 'static;

    /// Whether the menu has an entry of its own for a new folder, so that
    /// New leaves the folder out.
    const OWN_NEW_FOLDER: bool;

    /// The verbs of the Shell the menu has entries of its own for, beside
    /// those every menu of files has, by their names that do not change
    /// with the language.
    const OWN_VERBS: &'static [&'static str] = &[];

    /// The files and folders chosen, which Open with and Send to act on.
    fn chosen_files(&self) -> Vec<PathBuf>;

    /// Makes a new item of the kind `template`, or a folder where it is
    /// none, at `place`, and starts renaming it in `window`.
    fn make_new(
        &mut self,
        template: Option<NewTemplate>,
        place: &Self::Place,
        window: AnyWindowHandle,
        cx: &mut Context<Self>,
    );

    /// Starts renaming the item chosen.
    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>);

    /// Puts what is chosen into the Recycle Bin.
    fn delete_chosen(&mut self, cx: &mut Context<Self>);

    /// Copies or moves what is on the clipboard to `place`, or with `link`
    /// makes shortcuts to it there.
    fn paste_here(&mut self, link: bool, place: &Self::Place, cx: &mut Context<Self>);

    /// Reads again what it shows, after a verb of the Shell.
    fn read_again(&mut self, cx: &mut Context<Self>);

    /// Reads again what it shows as F5 does.
    fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.read_again(cx);
    }

    /// Takes the last work on files back, or with `again` does it again.
    fn take_back(&mut self, again: bool, cx: &mut Context<Self>);

    /// Readies `window` for what the menu does in it once the menu is
    /// gone; the file manager brings its window in front again.
    fn ready_window(_window: &mut Window) {}
}

/// What the Shell offers for a menu, as far as it answered.
#[derive(Default)]
pub struct ShellChoices {
    /// The menu of the Shell, kept on its thread while this lives.
    menu: Option<Rc<ShellMenu>>,
    /// The programs that open the files chosen, all of one kind.
    open_with: Vec<OpenWithHandler>,
    send_to: Vec<SendToTarget>,
    /// The kinds of new items, for the menu of a folder.
    templates: Vec<NewTemplate>,
}

impl ShellChoices {
    /// Asks the Shell, away from the user interface, what it offers for
    /// `chosen` (paths, or the names the Shell parses items such as This
    /// PC by), with the programs that open `open_with` where they are all
    /// files of its kind and Send to where they are all paths, or for the
    /// background of `folder` where nothing is chosen; `extended` asks for
    /// the verbs shown with Shift held.
    pub async fn read(
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
            return choices;
        }
        let files = chosen.iter().all(|path| path.is_absolute());
        let built = ShellMenu::for_items(chosen, extended);
        let handlers = open_with.as_deref().map(OpenWithHandler::of);
        let send_to = files.then(SendToTarget::all);
        choices.menu = within(built, until, cx).await.and_then(shell_menu);
        if let Some(handlers) = handlers {
            choices.open_with = within(handlers, until, cx).await.unwrap_or_default();
        }
        if let Some(send_to) = send_to {
            choices.send_to = within(send_to, until, cx).await.unwrap_or_default();
        }
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
pub async fn within<T: Send + 'static>(
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
pub fn one_kind(items: &[(PathBuf, bool)]) -> Option<PathBuf> {
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
/// `more` names the verbs the menu has entries of its own for beside
/// [`OWN_VERBS`].
fn shown(verb: &ShellVerb, more: &[&str]) -> bool {
    if verb.separator || verb.disabled || verb.label.trim().is_empty() {
        return false;
    }
    let own = verb.verb.as_deref().is_some_and(|name| {
        { OWN_VERBS.iter().chain(more) }.any(|own| own.eq_ignore_ascii_case(name))
    });
    if own || verb.intercept == Some(Intercept::NewFolder) {
        return false;
    }
    // A submenu the Shell fills only when Windows draws it, as Send to
    // and Open with are, has nothing to list here; New is Tessera's own.
    match verb.submenu {
        true => {
            let new =
                { verb.children.iter() }.any(|child| child.intercept == Some(Intercept::NewFolder));
            !new && verb.children.iter().any(|child| shown(child, more))
        }
        false => true,
    }
}

/// An entry that opens the menu `entries` in place of the one it is in.
pub fn submenu(
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

/// The entries the Shell gives a menu: New and Open with, which a menu
/// may place among its own, and the rest, which go after them.
pub struct ShellEntries {
    pub new: Option<MenuEntry>,
    pub open_with: Option<MenuEntry>,
    /// Send to, the verbs of the Shell and "Show more options".
    pub rest: Vec<MenuEntry>,
}

impl ShellEntries {
    /// Every entry, New and Open with first.
    pub fn all(self) -> Vec<MenuEntry> {
        { self.new.into_iter().chain(self.open_with) }
            .chain(self.rest)
            .collect()
    }
}

/// A menu of files that is opened: what it is about, the window it was
/// opened in, where, and the anchor of its submenus.
pub struct MenuTarget<H: FileMenu> {
    pub host: WeakEntity<H>,
    pub window: AnyWindowHandle,
    pub place: H::Place,
    pub anchor: PopupAnchor,
}

impl<H: FileMenu> Clone for MenuTarget<H> {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            window: self.window,
            place: self.place.clone(),
            anchor: self.anchor,
        }
    }
}

impl<H: FileMenu> MenuTarget<H> {
    /// The entries of what the Shell offered.
    pub fn entries(&self, choices: ShellChoices, language: Language) -> ShellEntries {
        let mut rest = Vec::new();
        let kinds: Vec<MenuEntry> = { choices.templates.into_iter() }
            .filter(|template| !(H::OWN_NEW_FOLDER && template.is_folder()))
            .map(|template| {
                let glyph = if template.is_folder() {
                    ICON_FOLDER
                } else {
                    ICON_NEW
                };
                let target = self.clone();
                MenuEntry::new(glyph, template.name.clone(), move |cx| {
                    let template = template.clone();
                    let (place, window) = (target.place.clone(), target.window);
                    { target.host.clone() }
                        .update(cx, |host, cx| {
                            host.make_new(Some(template), &place, window, cx)
                        })
                        .ok();
                })
            })
            .collect();
        let new =
            (!kinds.is_empty()).then(|| submenu(ICON_NEW, language.say("New"), kinds, self.anchor));
        let open_with =
            (!choices.open_with.is_empty()).then(|| self.open_with(choices.open_with, language));
        if !choices.send_to.is_empty() {
            let targets = { choices.send_to.into_iter() }
                .map(|place| {
                    let target = self.clone();
                    MenuEntry::new(ICON_SEND_TO, place.name.clone(), move |cx| {
                        target.send_to(&place, cx);
                    })
                })
                .collect();
            let label = language.say("Send to");
            rest.push(submenu(ICON_SEND_TO, label, targets, self.anchor));
        }
        if let Some(menu) = choices.menu {
            rest.extend(self.verb_entries(menu.verbs(), &menu));
            let target = self.clone();
            rest.push(MenuEntry::new(
                ICON_MORE,
                language.say("Show more options"),
                move |cx| target.show_shell_menu(menu.clone(), cx),
            ));
        }
        ShellEntries {
            new,
            open_with,
            rest,
        }
    }

    /// Open with: the programs `handlers`, and "Choose another app".
    fn open_with(&self, handlers: Vec<OpenWithHandler>, language: Language) -> MenuEntry {
        let mut programs: Vec<MenuEntry> = { handlers.into_iter() }
            .map(|handler| {
                let host = self.host.clone();
                MenuEntry::new(ICON_OPEN_WITH, handler.name.clone(), move |cx| {
                    if let Ok(chosen) = host.read_with(cx, |host, _| host.chosen_files()) {
                        handler.open(chosen);
                    }
                })
            })
            .collect();
        let host = self.host.clone();
        programs.push(MenuEntry::new(
            ICON_OPEN_WITH,
            language.say("Choose another app"),
            move |cx| {
                let chosen = host.read_with(cx, |host, _| host.chosen_files());
                if let Some(path) = chosen.ok().and_then(|chosen| chosen.into_iter().next()) {
                    choose_app(&path);
                }
            },
        ));
        submenu(
            ICON_OPEN_WITH,
            language.say("Open with"),
            programs,
            self.anchor,
        )
    }

    /// The entries of `verbs` of `menu` that Tessera's menu shows: an
    /// entry for a verb, and one that opens a menu of its own for a
    /// submenu.
    fn verb_entries(&self, verbs: &[ShellVerb], menu: &Rc<ShellMenu>) -> Vec<MenuEntry> {
        { verbs.iter().filter(|verb| shown(verb, H::OWN_VERBS)) }
            .map(|verb| {
                if verb.submenu {
                    let children = self.verb_entries(&verb.children, menu);
                    return submenu(ICON_VERB, &verb.label, children, self.anchor);
                }
                let (menu, target, id) = (menu.clone(), self.clone(), verb.id);
                MenuEntry::new(ICON_VERB, verb.label.clone(), move |cx| {
                    let menu = menu.clone();
                    let done = menu.invoke(id, None);
                    target.outcome(done, menu, cx);
                })
            })
            .collect()
    }

    /// Sends what is chosen to `place`, and reads again once it is there.
    fn send_to(&self, place: &SendToTarget, cx: &mut App) {
        let Ok(chosen) = self.host.read_with(cx, |host, _| host.chosen_files()) else {
            return;
        };
        let sent = place.send(chosen);
        let host = self.host.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            if let Ok(Err(error)) = sent.recv().await {
                report!("the files were not sent: {error}");
            }
            host.update(cx, |host, cx| host.read_again(cx)).ok();
        })
        .detach();
    }

    /// Does `deed` with the window, once the menu that asked for it is
    /// gone.
    fn after_menu(
        &self,
        deed: impl FnOnce(&mut H, &mut Window, &mut Context<H>) + 'static,
        cx: &mut App,
    ) {
        let (host, window) = (self.host.clone(), self.window);
        cx.defer(move |cx| {
            window
                .update(cx, |_, window, cx| {
                    H::ready_window(window);
                    host.update(cx, |host, cx| deed(host, window, cx)).ok();
                })
                .ok();
        });
    }

    /// Shows `menu` as Windows draws it, once the menu of Tessera is gone,
    /// and does what is chosen there.
    fn show_shell_menu(&self, menu: Rc<ShellMenu>, cx: &mut App) {
        let target = self.clone();
        self.after_menu(
            move |_, window, cx| {
                let shown = menu.show(None, Some(window));
                target.outcome(shown, menu, cx);
            },
            cx,
        );
    }

    /// Does what came of a verb of the menu of the Shell, which `menu` is
    /// kept alive for meanwhile: an intercepted verb as Tessera's own
    /// entry for it, and reading again after any other.
    fn outcome(
        &self,
        done: Receiver<Result<MenuOutcome, String>>,
        menu: Rc<ShellMenu>,
        cx: &mut App,
    ) {
        let target = self.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            let outcome = done.recv().await;
            drop(menu);
            match outcome {
                Ok(Ok(MenuOutcome::Intercepted(intercept))) => {
                    cx.update(|cx| target.intercepted(intercept, cx)).ok();
                }
                Ok(Ok(MenuOutcome::Invoked { .. })) => {
                    target.host.update(cx, |host, cx| host.read_again(cx)).ok();
                }
                Ok(Ok(MenuOutcome::Dismissed)) => {}
                Ok(Err(error)) => report!("{error}"),
                Err(_) => report!("the thread of the Shell is gone"),
            }
        })
        .detach();
    }

    /// Does a verb the menu of the Shell handed back, as Tessera's entry
    /// for it does, so that it is undone like that one.
    fn intercepted(&self, intercept: Intercept, cx: &mut App) {
        let (place, window) = (self.place.clone(), self.window);
        let host = self.host.clone();
        let mut deed = |deed: &dyn Fn(&mut H, &mut Context<H>)| {
            host.update(cx, |host, cx| deed(host, cx)).ok();
        };
        match intercept {
            Intercept::Rename => {
                self.after_menu(|host, window, cx| host.start_rename(window, cx), cx)
            }
            Intercept::NewFolder => deed(&|host, cx| host.make_new(None, &place, window, cx)),
            Intercept::Delete => deed(&|host, cx| host.delete_chosen(cx)),
            Intercept::Paste => deed(&|host, cx| host.paste_here(false, &place, cx)),
            Intercept::PasteLink => deed(&|host, cx| host.paste_here(true, &place, cx)),
            Intercept::Refresh => deed(&|host, cx| host.refresh_all(cx)),
            Intercept::Undo => deed(&|host, cx| host.take_back(false, cx)),
            Intercept::Redo => deed(&|host, cx| host.take_back(true, cx)),
            // What only Explorer's own view of a folder does; Tessera's
            // entries for the look and the order stand for them.
            Intercept::View | Intercept::Sort | Intercept::Group => {}
        }
    }
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

    fn shown_alone(verb: &ShellVerb) -> bool {
        shown(verb, &[])
    }

    #[test]
    fn the_verbs_tessera_has_entries_for_are_left_out() {
        assert!(!shown_alone(&verb("Open", Some("open"))));
        assert!(!shown_alone(&verb("Rename", Some("Rename"))));
        assert!(!shown_alone(&verb("Properties", Some("properties"))));
        assert!(shown_alone(&verb("Run as administrator", Some("runas"))));
        assert!(shown_alone(&verb("Scan with Defender", None)));
        assert!(!shown_alone(&verb("", None)));
        let separator = ShellVerb {
            separator: true,
            ..ShellVerb::default()
        };
        assert!(!shown_alone(&separator));
        let shortcut = verb("Create shortcut", Some("link"));
        assert!(shown_alone(&shortcut));
        assert!(!shown(&shortcut, &["link"]));
    }

    #[test]
    fn a_submenu_is_shown_with_something_in_it_and_new_is_not() {
        let mut empty = verb("Send to", None);
        empty.submenu = true;
        assert!(!shown_alone(&empty));
        let mut archive = verb("7-Zip", None);
        archive.submenu = true;
        archive.children = vec![verb("Add to archive", None)];
        assert!(shown_alone(&archive));
        let mut new = verb("New", None);
        new.submenu = true;
        new.children = vec![
            verb("Folder", Some("NewFolder")),
            verb("Text Document", None),
        ];
        assert!(!shown_alone(&new));
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
