// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The actions of the menus of files, as a page of the settings: widget
//! `file-actions`, drawn in the section Files. It lists the entries of
//! `[[file_action]]` with what each runs and what it is offered for, and
//! adds, edits, moves, removes and tries them: an action is tried on files
//! picked in the dialog of Windows, as the menu would run it.
//!
//! The entries are read from the file away from the thread of the
//! interface, as the file has them, and every change is checked as Tessera
//! reads an action before it is written, with the rest of the file and its
//! comments as they were. The shell takes the file over at once.

use gpui::{
    AppContext, AsyncApp, Context, Div, Entity, FontWeight, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, Render, SharedString, Stateful, StatefulInteractiveElement,
    Styled, WeakEntity, Window, div, px,
};

use super::quick::chip;
use super::settings_parts::{Feedback, chip_row, note_line};
use crate::config::edit::ConfigFile;
use crate::field::TextField;
use crate::file_actions::{self, Chosen, FileAction, MatchKind};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::runners::fill;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, list_title, one_line, state_layer};

/// The name of the page.
const NAME: &str = "file-actions";

const ICON_UP: &str = "\u{e70e}";
const ICON_DOWN: &str = "\u{e70d}";
const ICON_REMOVE: &str = "\u{e74d}";
const ICON_FOLDED: &str = "\u{e76c}";
const ICON_UNFOLDED: &str = "\u{e70d}";

/// The side of a round button with an icon.
const BUTTON: f32 = 28.;

pub struct FileActionsModule;

impl Module for FileActionsModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |_, cx| cx.new(FileActionsPage::new).into())
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
        let changed = "the actions of the file menus have changed in the meantime";
        match self {
            Self::Add(action) => file.add_file_action(&action).map(drop),
            Self::Set(index, action) => file.set_file_action(index, &action),
            Self::Move(from, to) => match file.move_file_action(from, to) {
                true => Ok(()),
                false => Err(changed.to_owned()),
            },
            Self::Remove(index) => match file.remove_file_action(index) {
                true => Ok(()),
                false => Err(changed.to_owned()),
            },
        }
    }
}

/// The entries of `[[file_action]]` in `file`, a configuration as a table.
fn entries_of(file: &toml::Table) -> Vec<toml::Table> {
    let list = file.get("file_action").and_then(toml::Value::as_array);
    { list.into_iter().flatten() }
        .filter_map(|entry| entry.as_table().cloned())
        .collect()
}

/// What the editor of an action holds, as text where it is typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Draft {
    name: String,
    command: String,
    submenu: String,
    icon: String,
    confirm: String,
    /// The patterns of names and the extensions, separated by commas.
    names: String,
    extensions: String,
    /// How many items at least and at most, as typed.
    min: String,
    max: String,
    kind: MatchKind,
    background: bool,
    terminal: bool,
    elevated: bool,
}

/// A list written with commas, without the empty items.
fn listed(text: &str) -> Vec<String> {
    { text.split([',', ';']) }
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

impl Draft {
    /// What `entry` says, as the editor shows it; what it says otherwise
    /// than the editor writes is kept by [`Self::table`].
    fn of(entry: &toml::Table) -> Self {
        let text = |table: &toml::Table, key: &str| {
            table
                .get(key)
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let flag = |key: &str| {
            entry
                .get(key)
                .and_then(toml::Value::as_bool)
                .unwrap_or(false)
        };
        let matching = entry
            .get("match")
            .and_then(toml::Value::as_table)
            .cloned()
            .unwrap_or_default();
        let joined = |key: &str| {
            let items = matching.get(key).and_then(toml::Value::as_array);
            let items: Vec<&str> = items
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .collect();
            items.join(", ")
        };
        let number = |key: &str| {
            { matching.get(key).and_then(toml::Value::as_integer) }
                .map(|number| number.to_string())
                .unwrap_or_default()
        };
        let kind = match text(&matching, "kind").as_str() {
            "file" => MatchKind::File,
            "folder" => MatchKind::Folder,
            _ => MatchKind::Any,
        };
        Self {
            name: text(entry, "name"),
            command: text(entry, "command"),
            submenu: text(entry, "submenu"),
            icon: text(entry, "icon"),
            confirm: text(entry, "confirm"),
            names: joined("names"),
            extensions: joined("extensions"),
            min: number("min"),
            max: number("max"),
            kind,
            background: matching
                .get("background")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false),
            terminal: flag("terminal"),
            elevated: flag("elevated"),
        }
    }

    /// The entry the draft writes; keys it leaves empty are taken out.
    fn table(&self) -> Result<toml::Table, String> {
        let mut entry = toml::Table::new();
        let put = |table: &mut toml::Table, key: &str, text: &str| {
            let text = text.trim();
            if !text.is_empty() {
                table.insert(key.to_owned(), toml::Value::String(text.to_owned()));
            }
        };
        put(&mut entry, "name", &self.name);
        put(&mut entry, "command", &self.command);
        put(&mut entry, "submenu", &self.submenu);
        put(&mut entry, "icon", &self.icon);
        put(&mut entry, "confirm", &self.confirm);
        let mut matching = toml::Table::new();
        for (key, text) in [("names", &self.names), ("extensions", &self.extensions)] {
            let items = listed(text);
            if !items.is_empty() {
                let items = items.into_iter().map(toml::Value::String).collect();
                matching.insert(key.to_owned(), toml::Value::Array(items));
            }
        }
        for (key, text) in [("min", &self.min), ("max", &self.max)] {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let number: i64 = { text.parse().ok() }
                .filter(|number| *number >= 0)
                .ok_or_else(|| format!("`{text}` is not a number of items"))?;
            matching.insert(key.to_owned(), toml::Value::Integer(number));
        }
        let kind = match self.kind {
            MatchKind::Any => None,
            MatchKind::File => Some("file"),
            MatchKind::Folder => Some("folder"),
        };
        if let Some(kind) = kind {
            matching.insert("kind".to_owned(), toml::Value::String(kind.to_owned()));
        }
        if self.background {
            matching.insert("background".to_owned(), toml::Value::Boolean(true));
        }
        if !matching.is_empty() {
            entry.insert("match".to_owned(), toml::Value::Table(matching));
        }
        for (key, on) in [("terminal", self.terminal), ("elevated", self.elevated)] {
            if on {
                entry.insert(key.to_owned(), toml::Value::Boolean(true));
            }
        }
        Ok(entry)
    }
}

/// A choice of the draft that is on or off: its English label, whether it
/// is on, and what switches it.
type Flag = (&'static str, bool, fn(&mut Draft));

/// The fields of the editor of the entry at `index`.
struct Editor {
    index: usize,
    draft: Draft,
    fields: Vec<(&'static str, Entity<TextField>)>,
}

/// The texts of the draft that are typed: the key of each, its English
/// label and what the empty field says.
const TYPED: [(&str, &str, &str); 9] = [
    ("name", "Name", "Optimize the picture"),
    ("command", "Command", "tool.exe %F"),
    ("names", "Names", "*.png, report*"),
    ("extensions", "Extensions", "png, jpg"),
    ("min", "At least", "1"),
    ("max", "At most", "1"),
    ("submenu", "Submenu", "Pictures"),
    ("icon", "Icon", "E91B or C:\\Tools\\tool.exe,0"),
    ("confirm", "Ask first", "Optimize the pictures?"),
];

impl Editor {
    fn new(index: usize, entry: &toml::Table, cx: &mut Context<FileActionsPage>) -> Self {
        let draft = Draft::of(entry);
        let fields = { TYPED.into_iter() }
            .map(|(key, _, placeholder)| {
                let text = draft.typed(key).to_owned();
                let field = cx.new(|cx| {
                    let mut field = TextField::new(placeholder.into(), cx);
                    field.set_text(&text, false, cx);
                    field
                });
                (key, field)
            })
            .collect();
        Self {
            index,
            draft,
            fields,
        }
    }

    /// The draft with what the fields hold now.
    fn read(&self, cx: &gpui::App) -> Draft {
        let mut draft = self.draft.clone();
        for (key, field) in &self.fields {
            *draft.typed_mut(key) = field.read(cx).text();
        }
        draft
    }
}

impl Draft {
    fn typed(&self, key: &str) -> &str {
        match key {
            "name" => &self.name,
            "command" => &self.command,
            "names" => &self.names,
            "extensions" => &self.extensions,
            "min" => &self.min,
            "max" => &self.max,
            "submenu" => &self.submenu,
            "icon" => &self.icon,
            _ => &self.confirm,
        }
    }

    fn typed_mut(&mut self, key: &str) -> &mut String {
        match key {
            "name" => &mut self.name,
            "command" => &mut self.command,
            "names" => &mut self.names,
            "extensions" => &mut self.extensions,
            "min" => &mut self.min,
            "max" => &mut self.max,
            "submenu" => &mut self.submenu,
            "icon" => &mut self.icon,
            _ => &mut self.confirm,
        }
    }
}

/// The page.
struct FileActionsPage {
    /// The entries, once they were read.
    entries: Option<Vec<toml::Table>>,
    /// The entry being edited.
    editor: Option<Editor>,
    /// The entry that waits for a second click to be removed.
    removing: Option<usize>,
    /// What is wrong with what was last asked, or why it was not written.
    failure: Option<SharedString>,
    /// The entry whose editor opens once the entries are read again, as
    /// one just added.
    open_once_read: Option<usize>,
    feedback: Feedback,
}

impl FileActionsPage {
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
            file.table()
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

    /// Writes `change` to the file away from the thread of the interface,
    /// has the shell take it over and reads the entries again; `then` is
    /// done once it was written.
    fn write(
        &mut self,
        change: Change,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.failure = None;
        self.removing = None;
        let written = cx
            .background_executor()
            .spawn(async move { ConfigFile::change(|file| change.apply(file)) });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let written = written.await;
            this.update(cx, |this, cx| {
                match written {
                    Ok(()) => {
                        cx.defer(Shell::config_changed);
                        then(this, cx);
                    }
                    Err(error) => {
                        report!("the actions of the file menus are not written: {error}");
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
        let open = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.index == index);
        self.editor = match (open, self.list().get(index).cloned()) {
            (false, Some(entry)) => Some(Editor::new(index, &entry, cx)),
            _ => None,
        };
        cx.notify();
    }

    /// Writes what the editor holds.
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let index = editor.index;
        match editor.read(cx).table() {
            Ok(table) => self.write(Change::Set(index, table), |this, _| this.editor = None, cx),
            Err(error) => {
                self.failure = Some(error.into());
                cx.notify();
            }
        }
    }

    /// Changes a choice of the draft of the editor.
    fn choose(&mut self, change: impl FnOnce(&mut Draft), cx: &mut Context<Self>) {
        if let Some(editor) = &mut self.editor {
            change(&mut editor.draft);
            cx.notify();
        }
    }

    /// Runs what the editor holds on files picked in the dialog of
    /// Windows, where they fit it, as the menu would.
    fn try_action(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let language = Language::of(cx);
        let action = editor.read(cx).table().and_then(|table| {
            toml::Value::Table(table)
                .try_into::<FileAction>()
                .map_err(|error| error.to_string())
        });
        let action = match action {
            Ok(action) => action,
            Err(error) => {
                self.failure = Some(error.into());
                cx.notify();
                return;
            }
        };
        let folders = action.matching.kind == MatchKind::Folder;
        let options = PathPromptOptions {
            files: !folders,
            directories: folders,
            multiple: true,
            prompt: Some(language.say("Try").into()),
        };
        let chosen = cx.prompt_for_paths(options);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let paths = match chosen.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Err(error)) => {
                    report!("no file can be chosen: {error:#}");
                    return;
                }
                _ => return,
            };
            let chosen: Vec<Chosen> = { paths.iter() }
                .map(|path| Chosen {
                    path: path.clone(),
                    folder: folders,
                })
                .collect();
            let fits =
                !file_actions::offered(std::slice::from_ref(&action), &chosen, false).is_empty();
            this.update(cx, |this, cx| {
                match fits {
                    true => action.run(&paths),
                    false => {
                        let said = language.say("The action is not offered for what was chosen.");
                        this.failure = Some(said.into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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

    /// A round button with the icon `glyph` that does `act`.
    fn glyph(
        &mut self,
        key: (&'static str, usize),
        glyph: &'static str,
        act: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let state = self.feedback.state(key, window);
        let element = div()
            .id(key)
            .flex_none()
            .size(px(BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(&theme, theme.colors.surface_container, state))
            .text_color(theme.colors.on_surface_variant)
            .child(icon(&theme, glyph).text_size(theme.typography.label_size))
            .on_click(cx.listener(move |this, _, _, cx| act(this, cx)));
        Feedback::track(element, key, cx, |this: &mut Self| &mut this.feedback)
    }

    /// What the row of an entry says of it, folded: its name, and what it
    /// runs and for what.
    fn summary(entry: &toml::Table, language: Language) -> (String, String) {
        let draft = Draft::of(entry);
        let title = match draft.name.trim() {
            "" => language.say("An action without a name").to_owned(),
            name => name.to_owned(),
        };
        let mut said = vec![draft.command.clone()];
        let what = [draft.names.as_str(), draft.extensions.as_str()]
            .into_iter()
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        if !what.is_empty() {
            said.push(what);
        }
        if !draft.submenu.trim().is_empty() {
            said.push(fill(
                language.say("in the submenu {}"),
                &[draft.submenu.trim()],
            ));
        }
        (title, said.join(" \u{b7} "))
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
        let unfolded = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.index == index);
        let (title, note) = Self::summary(entry, Language::of(cx));
        let key = ("entry", index);
        let state = self.feedback.state(key, window);
        let ground = match unfolded {
            true => theme.colors.secondary_container,
            false => theme.colors.surface_container,
        };
        let row = div()
            .id(key)
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .px(theme.spacing.small)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .bg(state_layer(&theme, ground, state))
            .child(
                icon(&theme, if unfolded { ICON_UNFOLDED } else { ICON_FOLDED })
                    .text_color(theme.colors.on_surface_variant),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().font_weight(FontWeight::MEDIUM).child(one_line(title)))
                    .child(
                        div()
                            .text_size(theme.typography.label_size)
                            .text_color(theme.colors.on_surface_variant)
                            .child(one_line(note)),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(index, cx)));
        Feedback::track(row, key, cx, |this: &mut Self| &mut this.feedback)
    }

    /// A field of the editor under its label.
    fn labelled(theme: &Theme, label: &str, control: impl IntoElement) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(note_line(theme, label.to_owned()))
            .child(div().px(theme.spacing.small).child(control))
    }

    /// The editor of the entry it is open for, under its row.
    fn editor(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let editor = self.editor.as_ref()?;
        let (index, draft) = (editor.index, editor.draft.clone());
        let fields = editor.fields.clone();
        let mut column = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .pl(theme.spacing.medium);
        for ((_, field), (_, label, _)) in fields.into_iter().zip(TYPED) {
            column = column.child(Self::labelled(&theme, language.say(label), field));
        }
        column = column.child(note_line(
            &theme,
            language.say(
                "In the command, %f is the first path chosen, %F all of them, %d the folder of the first, %n its name, %N its name without its extension and %u its file: address.",
            ),
        ));

        let mut kinds = chip_row(&theme);
        for (at, (kind, label)) in [
            (MatchKind::Any, "Files and folders"),
            (MatchKind::File, "Files"),
            (MatchKind::Folder, "Folders"),
        ]
        .into_iter()
        .enumerate()
        {
            let chip = self.chip(
                ("kind", at),
                language.say(label),
                draft.kind == kind,
                move |this, cx| this.choose(|draft| draft.kind = kind, cx),
                window,
                cx,
            );
            kinds = kinds.child(chip);
        }
        column = column.child(Self::labelled(&theme, language.say("Offered for"), kinds));

        let mut switches = chip_row(&theme);
        let flags: [Flag; 3] = [
            ("Also for the folder shown", draft.background, |draft| {
                draft.background = !draft.background
            }),
            ("Runs in a console", draft.terminal, |draft| {
                draft.terminal = !draft.terminal
            }),
            ("Runs as administrator", draft.elevated, |draft| {
                draft.elevated = !draft.elevated
            }),
        ];
        for (at, (label, on, flip)) in flags.into_iter().enumerate() {
            let chip = self.chip(
                ("flag", at),
                language.say(label),
                on,
                move |this, cx| this.choose(flip, cx),
                window,
                cx,
            );
            switches = switches.child(chip);
        }
        column = column.child(switches);

        let mut actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.spacing.small)
            .px(theme.spacing.small);
        let save = self.chip(
            ("save", index),
            language.say("Save"),
            true,
            |this, cx| this.save(cx),
            window,
            cx,
        );
        let try_it = self.chip(
            ("try", index),
            language.say("Try"),
            false,
            |this, cx| this.try_action(cx),
            window,
            cx,
        );
        actions = actions.child(save).child(try_it);
        let count = self.list().len();
        if index > 0 {
            let up = self.glyph(
                ("up", index),
                ICON_UP,
                move |this, cx| {
                    this.write(
                        Change::Move(index, index - 1),
                        |this, _| this.editor = None,
                        cx,
                    )
                },
                window,
                cx,
            );
            actions = actions.child(up);
        }
        if index + 1 < count {
            let down = self.glyph(
                ("down", index),
                ICON_DOWN,
                move |this, cx| {
                    this.write(
                        Change::Move(index, index + 1),
                        |this, _| this.editor = None,
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
                move |this, cx| this.write(Change::Remove(index), |this, _| this.editor = None, cx),
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
                .child(one_line(language.say("Remove this action?")))
                .child(remove)
                .child(cancel);
        } else {
            let remove = self.glyph(
                ("remove", 0),
                ICON_REMOVE,
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

/// What a new action says until it is edited.
fn new_action() -> toml::Table {
    let mut action = toml::Table::new();
    action.insert(
        "name".to_owned(),
        toml::Value::String("New action".to_owned()),
    );
    action.insert(
        "command".to_owned(),
        toml::Value::String("notepad.exe %F".to_owned()),
    );
    let mut matching = toml::Table::new();
    matching.insert("kind".to_owned(), toml::Value::String("file".to_owned()));
    action.insert("match".to_owned(), toml::Value::Table(matching));
    action
}

impl Render for FileActionsPage {
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
            .child(list_title(&theme, language.say("Actions of the file menus")))
            .child(note_line(
                &theme,
                language.say(
                    "Each action runs a program on the files and folders it is offered for, in the menus of Tessera Files and of the desktop, before the verbs of Windows.",
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
            page = page.child(note_line(&theme, language.say("There are no actions yet.")));
        }
        for (index, entry) in entries.iter().enumerate() {
            page = page.child(self.header(index, entry, window, cx));
            if self
                .editor
                .as_ref()
                .is_some_and(|editor| editor.index == index)
                && let Some(editor) = self.editor(window, cx)
            {
                page = page.child(editor);
            }
        }
        let count = entries.len();
        let add = self.chip(
            ("add", 0),
            language.say("Add an action"),
            false,
            move |this, cx| {
                this.write(
                    Change::Add(new_action()),
                    // The new action is opened once it is read.
                    move |this, _| {
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

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn a_draft_writes_what_it_read() {
        let entry = table(
            "name = \"Optimize\"\ncommand = \"oxipng %F\"\nsubmenu = \"Pictures\"\nterminal = true\nmatch = { names = [\"*.png\", \"a*\"], kind = \"file\", min = 1, max = 4, background = true }\n",
        );
        let draft = Draft::of(&entry);
        assert_eq!(draft.names, "*.png, a*");
        assert_eq!(draft.max, "4");
        assert_eq!(draft.kind, MatchKind::File);
        assert!(draft.terminal && draft.background && !draft.elevated);
        assert_eq!(draft.table().unwrap(), entry);
        let action: FileAction = toml::Value::Table(draft.table().unwrap())
            .try_into()
            .unwrap();
        assert_eq!(action.matching.names, ["*.png", "a*"]);
    }

    #[test]
    fn empty_fields_are_left_out_and_numbers_are_checked() {
        let draft = Draft {
            name: " Open ".to_owned(),
            command: "notepad %f".to_owned(),
            extensions: " txt ,, md ".to_owned(),
            ..Draft::default()
        };
        assert_eq!(
            draft.table().unwrap(),
            table(
                "name = \"Open\"\ncommand = \"notepad %f\"\nmatch = { extensions = [\"txt\", \"md\"] }"
            )
        );
        let wrong = Draft {
            min: "two".to_owned(),
            ..draft
        };
        assert!(wrong.table().is_err());
        assert_eq!(
            entries_of(&table("[[file_action]]\nname = \"a\"\ncommand = \"b\"")).len(),
            1
        );
        let new: FileAction = toml::Value::Table(new_action()).try_into().unwrap();
        assert_eq!(new.matching.kind, MatchKind::File);
    }
}
