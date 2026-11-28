// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The rules of notifications, as a page of the settings: widget
//! `notification-rules`, drawn in the section Notifications. It lists the
//! entries of `[[notification_rule]]` in their order, with what each does,
//! and adds, edits, moves and removes them. The application of a rule is
//! typed (a name or an identifier, with `*` and `?`) or picked from the
//! applications whose notifications are in the history.
//!
//! The entries are read from the file away from the thread of the
//! interface, and every change is checked as Tessera reads a rule before
//! it is written, with the rest of the file and its comments as they
//! were. The shell takes the file over at once.

use gpui::{
    AppContext, AsyncApp, Context, Div, Entity, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled, WeakEntity,
    Window, div,
};

use super::quick::chip;
use super::settings_parts::{Feedback, chip_row, note_line};
use crate::config::edit::ConfigFile;
use crate::field::TextField;
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::notifications_store::NotificationStore;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{list_title, one_line, state_layer};

/// The name of the page.
const NAME: &str = "notification-rules";

/// How many applications of the history are offered to pick from.
const MOST_PICKS: usize = 12;

pub struct NotificationRulesModule;

impl Module for NotificationRulesModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |_, cx| cx.new(NotificationRulesPage::new).into())
    }
}

/// A change of the entries of the file.
#[derive(Clone, Debug)]
enum Change {
    Add(toml::Table),
    Set(usize, toml::Table),
    Move(usize, usize),
    Remove(usize),
}

impl Change {
    /// Makes the change in `file`, or says why it cannot be made.
    fn apply(self, file: &mut ConfigFile) -> Result<(), String> {
        let changed = || "the rules of notifications have changed in the meantime".to_owned();
        match self {
            Self::Add(rule) => file.add_notification_rule(&rule).map(drop),
            Self::Set(index, rule) => file.set_notification_rule(index, &rule),
            Self::Move(from, to) => file
                .move_notification_rule(from, to)
                .then_some(())
                .ok_or_else(changed),
            Self::Remove(index) => file
                .remove_notification_rule(index)
                .then_some(())
                .ok_or_else(changed),
        }
    }
}

/// The entries of `[[notification_rule]]` in `file`, a configuration as a
/// table.
fn entries_of(file: &toml::Table) -> Vec<toml::Table> {
    let list = file
        .get("notification_rule")
        .and_then(toml::Value::as_array);
    { list.into_iter().flatten() }
        .filter_map(|entry| entry.as_table().cloned())
        .collect()
}

/// What a rule says, as the editor shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Draft {
    app: String,
    banners: bool,
    show: bool,
    priority: bool,
    badge: bool,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            app: String::new(),
            banners: true,
            show: true,
            priority: false,
            badge: true,
        }
    }
}

/// What can be switched of a rule: its English label, whether it is on
/// in a draft, and how it is switched.
type Flag = (&'static str, bool, fn(&mut Draft));

impl Draft {
    /// What `entry` says; a key it lacks has its default.
    fn of(entry: &toml::Table) -> Self {
        let flag = |key: &str, default: bool| {
            { entry.get(key) }
                .and_then(toml::Value::as_bool)
                .unwrap_or(default)
        };
        Self {
            app: { entry.get("app") }
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            banners: flag("banners", true),
            show: flag("show", true),
            priority: flag("priority", false),
            badge: flag("badge", true),
        }
    }

    /// The entry it writes: only what is not the default.
    fn table(&self) -> Result<toml::Table, String> {
        let app = self.app.trim();
        if app.is_empty() {
            return Err("a rule names an application".to_owned());
        }
        let mut table = toml::Table::new();
        table.insert("app".to_owned(), toml::Value::String(app.to_owned()));
        for (key, value, default) in [
            ("banners", self.banners, true),
            ("show", self.show, true),
            ("priority", self.priority, false),
            ("badge", self.badge, true),
        ] {
            if value != default {
                table.insert(key.to_owned(), toml::Value::Boolean(value));
            }
        }
        Ok(table)
    }

    /// The switches of the editor.
    fn flags(&self) -> [Flag; 4] {
        [
            ("Banners", self.banners, |draft| {
                draft.banners = !draft.banners
            }),
            ("Listed", self.show, |draft| draft.show = !draft.show),
            ("Through do not disturb", self.priority, |draft| {
                draft.priority = !draft.priority
            }),
            ("Counted on the dock", self.badge, |draft| {
                draft.badge = !draft.badge
            }),
        ]
    }

    /// What it does, in a few words: what is unlike the default.
    fn summary(&self, language: Language) -> String {
        let mut said = Vec::new();
        if !self.show {
            said.push(language.say("Not shown"));
        } else if !self.banners {
            said.push(language.say("No banners"));
        }
        if self.priority {
            said.push(language.say("Through do not disturb"));
        }
        if !self.badge {
            said.push(language.say("Not counted on the dock"));
        }
        match said.is_empty() {
            true => language.say("Everything as usual").to_owned(),
            false => said.join(" \u{b7} "),
        }
    }
}

/// The names of the applications whose notifications are in `history`,
/// each once, in the order of their newest notification.
fn apps_seen<'a>(history: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for (app, app_id) in history {
        let name = if app.trim().is_empty() { app_id } else { app };
        let name = name.trim();
        if !name.is_empty() && !seen.iter().any(|known| known.eq_ignore_ascii_case(name)) {
            seen.push(name.to_owned());
        }
    }
    seen
}

/// The editor of the entry at `index`.
struct Editor {
    index: usize,
    draft: Draft,
    field: Entity<TextField>,
}

/// The page.
struct NotificationRulesPage {
    /// The entries, once they were read.
    entries: Option<Vec<toml::Table>>,
    editor: Option<Editor>,
    /// The entry that waits for a second click to be removed.
    removing: Option<usize>,
    /// What is wrong with what was last asked, or why it was not written.
    failure: Option<SharedString>,
    /// The entry whose editor opens once the entries are read again.
    open_once_read: Option<usize>,
    feedback: Feedback,
}

impl NotificationRulesPage {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let mut this = Self {
            entries: None,
            editor: None,
            removing: None,
            failure: None,
            open_once_read: None,
            feedback: Feedback::new(motion),
        };
        this.read(cx);
        this
    }

    /// Reads the entries from the file away from the thread of the
    /// interface.
    fn read(&mut self, cx: &mut Context<Self>) {
        let read = cx.background_executor().spawn(async move {
            let file = ConfigFile::open()?;
            { file.table() }
                .map(|table| entries_of(&table))
                .ok_or_else(|| "the configuration cannot be read".to_owned())
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = read.await;
            this.update(cx, |this, cx| {
                match read {
                    Ok(entries) => this.entries = Some(entries),
                    Err(error) => this.failure = Some(error.into()),
                }
                if let Some(index) = this.open_once_read.take() {
                    this.editor = None;
                    this.toggle(index, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Writes `change` away from the thread of the interface, has the
    /// shell take it over and reads the entries again; `then` is done once
    /// it was written.
    fn write(
        &mut self,
        change: Change,
        then: impl FnOnce(&mut Self) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.failure = None;
        self.removing = None;
        let written = { cx.background_executor() }
            .spawn(async move { ConfigFile::change(|file| change.apply(file)) });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let written = written.await;
            this.update(cx, |this, cx| {
                match written {
                    Ok(()) => {
                        cx.defer(Shell::config_changed);
                        then(this);
                    }
                    Err(error) => {
                        report!("the rules of notifications are not written: {error}");
                        this.failure = Some(error.into());
                    }
                }
                this.read(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn list(&self) -> &[toml::Table] {
        self.entries.as_deref().unwrap_or_default()
    }

    /// Opens the editor of the entry at `index`, or closes it where it is
    /// open.
    fn toggle(&mut self, index: usize, cx: &mut Context<Self>) {
        self.removing = None;
        let open = { self.editor.as_ref() }.is_some_and(|editor| editor.index == index);
        self.editor = match (open, self.list().get(index)) {
            (false, Some(entry)) => {
                let draft = Draft::of(entry);
                let text = draft.app.clone();
                let field = cx.new(|cx| {
                    let mut field = TextField::new("Teams, com.example.*, *".into(), cx);
                    field.set_text(&text, false, cx);
                    field
                });
                Some(Editor {
                    index,
                    draft,
                    field,
                })
            }
            _ => None,
        };
        cx.notify();
    }

    /// The draft of the editor with what its field holds now.
    fn draft(&self, cx: &gpui::App) -> Option<(usize, Draft)> {
        let editor = self.editor.as_ref()?;
        let mut draft = editor.draft.clone();
        draft.app = editor.field.read(cx).text();
        Some((editor.index, draft))
    }

    /// Writes what the editor holds.
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some((index, draft)) = self.draft(cx) else {
            return;
        };
        match draft.table() {
            Ok(table) => self.write(Change::Set(index, table), |this| this.editor = None, cx),
            Err(error) => {
                self.failure = Some(error.into());
                cx.notify();
            }
        }
    }

    /// Has the field of the editor name the application `app`.
    fn pick(&mut self, app: String, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            editor
                .field
                .update(cx, |field, cx| field.set_text(&app, false, cx));
            cx.notify();
        }
    }

    /// A chip that does `act` when clicked.
    fn chip(
        &mut self,
        key: (&'static str, usize),
        label: impl Into<SharedString>,
        chosen: bool,
        act: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let state = self.feedback.state(key, window);
        let element = chip(&theme, key, label, chosen, state)
            .on_click(cx.listener(move |this, _, _, cx| act(this, cx)));
        Feedback::track(element, key, cx, |this: &mut Self| &mut this.feedback)
    }

    /// The row of an entry, which folds and unfolds its editor.
    fn header(
        &mut self,
        index: usize,
        entry: &toml::Table,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let unfolded = { self.editor.as_ref() }.is_some_and(|editor| editor.index == index);
        let draft = Draft::of(entry);
        let title = match draft.app.trim() {
            "" => language.say("A rule without an application").to_owned(),
            app => app.to_owned(),
        };
        let key = ("rule", index);
        let state = self.feedback.state(key, window);
        let ground = match unfolded {
            true => theme.colors.secondary_container,
            false => theme.colors.surface_container,
        };
        let row = div()
            .id(key)
            .flex()
            .flex_col()
            .px(theme.spacing.small)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .bg(state_layer(&theme, ground, state))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(one_line(format!("{}. {title}", index + 1))),
            )
            .child(
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(one_line(draft.summary(language))),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(index, cx)));
        Feedback::track(row, key, cx, |this: &mut Self| &mut this.feedback)
    }

    /// The editor of the entry it is open for, under its row.
    fn editor(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let (index, draft) = self.draft(cx)?;
        let field = self.editor.as_ref()?.field.clone();
        let mut column = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .pl(theme.spacing.medium)
            .child(note_line(&theme, language.say("Application")))
            .child(div().px(theme.spacing.small).child(field))
            .child(note_line(
                &theme,
                language.say("A name or an identifier; * stands for any letters and ? for one."),
            ));

        let seen = { cx.try_global::<NotificationStore>() }.map(|store| {
            let history = store
                .entries()
                .iter()
                .map(|entry| (entry.app.as_str(), entry.app_id.as_str()));
            apps_seen(history)
        });
        let seen = seen.unwrap_or_default();
        if !seen.is_empty() {
            let mut picks = chip_row(&theme);
            for (at, app) in seen.into_iter().take(MOST_PICKS).enumerate() {
                let chosen = draft.app.trim().eq_ignore_ascii_case(&app);
                let picked = app.clone();
                let chip = self.chip(
                    ("pick", at),
                    app,
                    chosen,
                    move |this, cx| this.pick(picked.clone(), cx),
                    window,
                    cx,
                );
                picks = picks.child(chip);
            }
            column = column
                .child(note_line(&theme, language.say("Seen in the history")))
                .child(picks);
        }

        let mut switches = chip_row(&theme);
        for (at, (label, on, flip)) in draft.flags().into_iter().enumerate() {
            let chip = self.chip(
                ("flag", at),
                language.say(label),
                on,
                move |this, cx| {
                    if let Some(editor) = &mut this.editor {
                        flip(&mut editor.draft);
                        cx.notify();
                    }
                },
                window,
                cx,
            );
            switches = switches.child(chip);
        }
        column = column.child(switches);

        let mut actions = chip_row(&theme).items_center();
        let save = self.chip(
            ("save", index),
            language.say("Save"),
            true,
            |this, cx| this.save(cx),
            window,
            cx,
        );
        actions = actions.child(save);
        let count = self.list().len();
        if index > 0 {
            let up = self.chip(
                ("up", index),
                language.say("Move up"),
                false,
                move |this, cx| {
                    this.write(
                        Change::Move(index, index - 1),
                        |this| this.editor = None,
                        cx,
                    )
                },
                window,
                cx,
            );
            actions = actions.child(up);
        }
        if index + 1 < count {
            let down = self.chip(
                ("down", index),
                language.say("Move down"),
                false,
                move |this, cx| {
                    this.write(
                        Change::Move(index, index + 1),
                        |this| this.editor = None,
                        cx,
                    )
                },
                window,
                cx,
            );
            actions = actions.child(down);
        }
        if self.removing == Some(index) {
            let remove = self.chip(
                ("remove", 1),
                language.say("Remove"),
                true,
                move |this, cx| this.write(Change::Remove(index), |this| this.editor = None, cx),
                window,
                cx,
            );
            let cancel = self.chip(
                ("remove", 2),
                language.say("Cancel"),
                false,
                |this, cx| {
                    this.removing = None;
                    cx.notify();
                },
                window,
                cx,
            );
            actions = actions
                .child(one_line(language.say("Remove this rule?")))
                .child(remove)
                .child(cancel);
        } else {
            let remove = self.chip(
                ("remove", 0),
                language.say("Remove"),
                false,
                move |this, cx| {
                    this.removing = Some(index);
                    cx.notify();
                },
                window,
                cx,
            );
            actions = actions.child(remove);
        }
        Some(column.child(actions))
    }
}

impl Render for NotificationRulesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut page = div()
            .id(NAME)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .p(theme.spacing.medium)
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container_highest)
            .text_color(theme.colors.on_surface)
            .child(list_title(&theme, language.say("Notification rules")))
            .child(note_line(
                &theme,
                language.say(
                    "The first rule that names an application decides what is done with its notifications.",
                ),
            ));
        if let Some(failure) = &self.failure {
            page = page.child(note_line(&theme, failure.clone()).text_color(theme.colors.primary));
        }
        // Nothing is listed until the entries were read, a moment later.
        let Some(entries) = self.entries.clone() else {
            return page;
        };
        if entries.is_empty() {
            page = page.child(note_line(&theme, language.say("There are no rules yet.")));
        }
        for (index, entry) in entries.iter().enumerate() {
            page = page.child(self.header(index, entry, window, cx));
            let open = { self.editor.as_ref() }.is_some_and(|editor| editor.index == index);
            if open && let Some(editor) = self.editor(window, cx) {
                page = page.child(editor);
            }
        }
        let count = entries.len();
        let add = self.chip(
            ("add", 0),
            language.say("Add a rule"),
            false,
            move |this, cx| {
                let mut rule = toml::Table::new();
                rule.insert("app".to_owned(), toml::Value::String("*".to_owned()));
                rule.insert("banners".to_owned(), toml::Value::Boolean(false));
                this.write(
                    Change::Add(rule),
                    // The new rule is opened once it is read.
                    move |this| {
                        this.editor = None;
                        this.open_once_read = Some(count);
                    },
                    cx,
                )
            },
            window,
            cx,
        );
        page.child(div().flex().px(theme.spacing.small).child(add))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications_store::NotificationRule;

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn a_draft_writes_only_what_is_not_the_default() {
        let entry = table("app = \"Teams\"\nbanners = false\npriority = true");
        let draft = Draft::of(&entry);
        assert!(!draft.banners && draft.show && draft.priority && draft.badge);
        assert_eq!(draft.table().unwrap(), entry);
        let rule: NotificationRule = toml::Value::Table(draft.table().unwrap())
            .try_into()
            .unwrap();
        assert_eq!(rule.app, "Teams");
        let blank = Draft {
            app: "  ".to_owned(),
            ..Draft::default()
        };
        assert!(blank.table().is_err());
        assert_eq!(
            Draft::default().summary(Language::English),
            "Everything as usual"
        );
        assert_eq!(
            draft.summary(Language::English),
            "No banners \u{b7} Through do not disturb"
        );
    }

    #[test]
    fn the_applications_of_the_history_are_offered_once_each() {
        let history = [
            ("Mail", "mail"),
            ("", "com.example.chat"),
            ("mail", "mail"),
            ("  ", ""),
        ];
        assert_eq!(apps_seen(history.into_iter()), ["Mail", "com.example.chat"]);
        assert_eq!(
            entries_of(&table("[[notification_rule]]\napp = \"a\"")).len(),
            1
        );
    }
}
