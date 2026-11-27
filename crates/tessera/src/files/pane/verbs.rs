// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the Shell adds to the menu of a pane, after Tessera's own
//! entries, through [`crate::shell_entries`], which the menus of the
//! desktop share: the verbs of the kinds of files and those programs add,
//! New with the kinds of new files Windows offers, Open with, Send to,
//! and the menu as Windows draws it ("Show more options"). Here is what
//! the pane does for them: the verbs the menu of the Shell hands back
//! are done by the pane, so that renaming, deleting, pasting and making
//! a folder stay in the undo journal.

use std::path::PathBuf;

use gpui::{AnyWindowHandle, AsyncApp, Context, WeakEntity, Window};

use super::FolderPane;
use crate::i18n::Language;
use crate::menu::MenuEntry;
use crate::platform::{
    FileClipboard, FileDeed, FileOperation, LinkTarget, NewTemplate, UndoJournal,
};
use crate::popup::PopupAnchor;
use crate::shell_entries::{FileMenu, MenuTarget, ShellChoices};

impl FileMenu for FolderPane {
    type Place = ();

    const OWN_NEW_FOLDER: bool = true;

    fn chosen_files(&self) -> Vec<PathBuf> {
        self.chosen()
    }

    fn make_new(
        &mut self,
        template: Option<NewTemplate>,
        _: &(),
        _: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        self.new_item(template, cx);
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rename_chosen(window, cx);
    }

    fn delete_chosen(&mut self, cx: &mut Context<Self>) {
        self.delete(false, cx);
    }

    fn paste_here(&mut self, link: bool, _: &(), cx: &mut Context<Self>) {
        match link {
            true => self.paste_links(cx),
            false => self.paste(cx),
        }
    }

    fn read_again(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
    }

    fn take_back(&mut self, again: bool, cx: &mut Context<Self>) {
        self.undo(again, cx);
    }

    fn ready_window(window: &mut Window) {
        window.activate_window();
    }
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
        let target = MenuTarget {
            host: cx.entity().downgrade(),
            window: self.window,
            place: (),
            anchor,
        };
        target.entries(choices, Language::of(cx)).all()
    }

    /// Makes shortcuts to what is on the clipboard in the folder shown,
    /// as Paste shortcut does in Explorer, and reads the folder after.
    fn paste_links(&mut self, cx: &mut Context<Self>) {
        let (Some(folder), Some((from, _))) = (self.folder(), FileClipboard::take()) else {
            return;
        };
        let operation = { from.into_iter() }.fold(FileOperation::new(), |operation, path| {
            operation.link(LinkTarget::Path(path), &folder)
        });
        if operation.is_empty() {
            return;
        }
        let done = operation.start();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Ok(outcome) = done.recv().await {
                outcome.report();
            }
            this.update(cx, |this, cx| this.load(cx)).ok();
        })
        .detach();
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
