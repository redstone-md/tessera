// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The editor of the window rules, as KWin's Window Rules and the
//! Specific apps page of Seelen UI: the page `rules-editor` of the
//! settings, in the section Window rules. It lists the `[[rule]]` entries
//! of the configuration in their order, each with what it names and what
//! it says; a rule is switched on and off, added, duplicated, taken away
//! and moved, by dragging it or with Move up and Move down. The sets of
//! rules that come with Tessera are shown as they are, and each of their
//! rules can be copied to the user's own.
//!
//! The editor of one rule offers every name a rule can give, with every
//! way of matching it and the tables of `not`, `any` and `all`, and every
//! key of what a rule says with a control of its own. It tells how many
//! windows the rule names now, and why the configuration would leave it
//! out, before anything is written. "Pick a window" hides the settings,
//! has a window clicked and opens them again with what was read of it, as
//! KWin's "Detect window properties"; a window can also be chosen from
//! the windows that are open. Rules are written one at a time with the
//! methods of config/edit.rs, which keep the comments of the others, and
//! they can be exported to a file of `[[rule]]` entries and imported from
//! one.
//!
//! The action `edit-window-rule <window>` opens the settings here with
//! that window read, the window as the window menu knows it (the number of
//! [`AppWindowId::raw`]); with `,temporarily` after it, what is set is
//! given to that window until it closes and never written.
//!
//! Reading and writing the file, reading windows and counting them are
//! all done off the thread of the interface.

mod detect;
mod draft;
mod editing;
mod parts;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, Render, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window, div, px,
};

use crate::actions::{Action, Actions};
use crate::banners::{Banners, Notice};
use crate::config::edit::ConfigFile;
use crate::field::TextField;
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::modules::Settings;
use crate::platform::{AppWindow, AppWindowId, Display, WindowPick};
use crate::popup::Popups;
use crate::rules::matching::Named;
use crate::rules::{Rules, templates};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::one_line;
use crate::workspaces::Workspaces;

use self::detect::{Detected, open_windows};
use self::draft::{Draft, summary};
use self::parts::{
    Carried, CarriedFace, LIFTED, card, chip, glyph_button, note, still_switch, text_button,
    wrapping,
};

/// The name of the page, and of the action that opens it at a window.
const NAME: &str = "rules-editor";
const ACTION: &str = "edit-window-rule";

/// The section of the settings the page stands in, as `open_at` takes it.
const SECTION: &str = "window rules";

/// What the action's argument ends in to give the rule to the window
/// until it closes.
const TEMPORARILY: &str = ",temporarily";

/// How long the settings take to go before the windows are covered to
/// pick one, or before a dialog of Windows is shown, which would otherwise
/// belong to the settings and close with them.
const AWAY_AFTER: Duration = Duration::from_millis(350);

/// How long typing rests before the windows a rule names are counted.
const COUNT_AFTER: Duration = Duration::from_millis(250);

/// How many of the windows a rule names are listed by their titles.
const LISTED: usize = 6;

/// The name the exported file is offered under.
const EXPORTED: &str = "window-rules.toml";

const ICON_ADD: &str = "\u{e710}";
const ICON_PICK: &str = "\u{e7c9}";
const ICON_WINDOWS: &str = "\u{e737}";
const ICON_IMPORT: &str = "\u{e8b5}";
const ICON_EXPORT: &str = "\u{ede1}";
const ICON_EDIT: &str = "\u{e70f}";
const ICON_COPY: &str = "\u{e8c8}";
const ICON_UP: &str = "\u{e70e}";
const ICON_DOWN: &str = "\u{e70d}";
const ICON_REMOVE: &str = "\u{e74d}";
const ICON_DRAG: &str = "\u{e76f}";
const ICON_MORE: &str = "\u{e70d}";
const ICON_LESS: &str = "\u{e70e}";

pub struct RulesEditorModule;

impl Module for RulesEditorModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |context, cx| {
            let item = context.text("item").map(str::to_owned);
            cx.new(|cx| RulesEditor::new(item.as_deref(), cx)).into()
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let edit = Action::new(|arg, cx| {
            let item = arg.map(|arg| format!("window:{}", arg.trim()));
            Settings::open_at(SECTION, item.as_deref(), cx);
        })
        .checking(|arg| match arg.map(asked_window) {
            Some(Some(_)) => Ok(()),
            _ => Err(format!(
                "`{ACTION}` takes the number of a window, with `{TEMPORARILY}` after it to give the rule to it until it closes"
            )),
        });
        actions.register(ACTION, edit)
    }
}

/// The window an argument of the action names, and whether what is set is
/// to be given to it until it closes.
fn asked_window(arg: &str) -> Option<(AppWindowId, bool)> {
    let arg = arg.trim();
    let (number, temporarily) = match arg.strip_suffix(TEMPORARILY) {
        Some(number) => (number, true),
        None => (arg, false),
    };
    let raw: isize = number.trim().parse().ok()?;
    (raw != 0).then(|| (AppWindowId::from_raw(raw), temporarily))
}

/// The window the item of the page names: `window:<number>`, with
/// `,temporarily` after it.
fn item_window(item: &str) -> Option<(AppWindowId, bool)> {
    asked_window(item.strip_prefix("window:")?)
}

/// A change of the rules of the file.
enum Change {
    /// The rule at a place, which was the first table, becomes the second.
    Set(usize, toml::Table, toml::Table),
    /// A rule goes in before the one at a place.
    Insert(usize, toml::Table),
    /// Rules go after the last one.
    Append(Vec<toml::Table>),
    /// The rule at a place, which was the table, goes.
    Remove(usize, toml::Table),
    /// The rule at the first place goes to the second.
    Move(usize, usize),
}

impl Change {
    /// Makes the change in the file, if the rules it changes are still
    /// those that were shown.
    fn make(self) -> Result<(), String> {
        ConfigFile::change(|file| {
            let shown = |file: &ConfigFile, index: usize, table: &toml::Table| {
                let now = file.table();
                let rules = now.as_ref().and_then(|now| now.get("rule")?.as_array());
                let rule = rules.and_then(|rules| rules.get(index)?.as_table());
                match rule == Some(table) {
                    true => Ok(()),
                    false => {
                        Err("the rules were changed meanwhile; they are shown again".to_owned())
                    }
                }
            };
            let done = match self {
                Self::Set(index, before, after) => {
                    shown(file, index, &before)?;
                    file.set_window_rule(index, &after)
                }
                Self::Insert(index, table) => file.insert_window_rule(index, &table),
                Self::Append(tables) => {
                    let count = rules_of(file).len();
                    { tables.iter().enumerate() }
                        .all(|(at, table)| file.insert_window_rule(count + at, table))
                }
                Self::Remove(index, before) => {
                    shown(file, index, &before)?;
                    file.remove_window_rule(index)
                }
                Self::Move(from, to) => file.move_window_rule(from, to),
            };
            match done {
                true => Ok(()),
                false => Err("the configuration has a `rule` that is no list of tables".to_owned()),
            }
        })
    }
}

/// The `[[rule]]` tables of `file`.
fn rules_of(file: &ConfigFile) -> Vec<toml::Table> {
    let table = file.table().unwrap_or_default();
    {
        table
            .get("rule")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
    }
    .filter_map(|rule| rule.as_table().cloned())
    .collect()
}

/// What is read of the file and of the displays when the page opens.
struct Read {
    rules: Vec<toml::Table>,
    templates_on: Vec<String>,
    displays: usize,
}

fn read() -> Result<Read, String> {
    let file = ConfigFile::open()?;
    let whole = file.table().unwrap_or_default();
    let templates_on = { whole.get("rule_templates") }
        .and_then(toml::Value::as_array)
        .map(|names| {
            { names.iter() }
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok(Read {
        rules: rules_of(&file),
        templates_on,
        displays: Display::all().len(),
    })
}

/// Where a text of the editor is typed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Slot {
    /// A name of the table of names at a path, by its key.
    Name(Vec<draft::Step>, &'static str),
    /// A text of what the rule says, by its key.
    Text(&'static str),
    /// One of the two numbers of a pair the rule says, by its key.
    Pair(&'static str, usize),
}

/// The editor of the window rules.
pub struct RulesEditor {
    /// The rules of the file as their tables, once they were read.
    rules: Option<Vec<toml::Table>>,
    /// The sets of rules that come with Tessera and are switched on.
    templates_on: Vec<String>,
    /// The set whose rules are shown.
    template_open: Option<&'static str>,
    failure: Option<SharedString>,
    displays: usize,
    workspaces: usize,
    /// The rule being changed, what its table was when it was taken up,
    /// and why it cannot be used, if it cannot.
    draft: Option<Draft>,
    original: Option<toml::Table>,
    draft_fault: Option<String>,
    fields: HashMap<Slot, Entity<TextField>>,
    /// The pairs whose numbers are not both numbers.
    pair_faults: HashSet<&'static str>,
    /// The titles of the windows the rule names now, once counted, and a
    /// count of the countings asked for, so that only the last one shows.
    named_now: Option<Vec<String>>,
    countings: u64,
    /// The window that was read, which of what was read are ticked, and
    /// whether what is set is to be given to it until it closes.
    detected: Option<Detected>,
    ticked: HashSet<usize>,
    temporarily: bool,
    /// The open windows to choose one of, once they were read.
    open: Option<Vec<(AppWindowId, String)>>,
    /// The rule carried to another place.
    lifted: Option<usize>,
}

impl RulesEditor {
    fn new(item: Option<&str>, cx: &mut Context<Self>) -> Self {
        let workspaces = Workspaces::residents(None, cx).map_or(0, |rows| rows.len());
        let mut editor = Self {
            rules: None,
            templates_on: Vec::new(),
            template_open: None,
            failure: None,
            displays: 1,
            workspaces,
            draft: None,
            original: None,
            draft_fault: None,
            fields: HashMap::new(),
            pair_faults: HashSet::new(),
            named_now: None,
            countings: 0,
            detected: None,
            ticked: HashSet::new(),
            temporarily: false,
            open: None,
            lifted: None,
        };
        editor.reload(cx);
        if let Some((window, temporarily)) = item.and_then(item_window) {
            editor.temporarily = temporarily;
            editor.detect(window, cx);
        }
        editor
    }

    /// Reads the rules of the file again, aside.
    fn reload(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx.background_executor().spawn(async { read() }).await;
            this.update(cx, |this, cx| {
                match read {
                    Ok(read) => {
                        this.rules = Some(read.rules);
                        this.templates_on = read.templates_on;
                        this.displays = read.displays.max(1);
                    }
                    Err(error) => this.failure = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Makes `change` in the file aside, has the shell take it over and
    /// shows the rules again; `then` is done once it is written.
    fn write(
        &mut self,
        change: Change,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let made = cx
                .background_executor()
                .spawn(async move { change.make() })
                .await;
            this.update(cx, |this, cx| {
                match made {
                    Ok(()) => {
                        this.failure = None;
                        then(this, cx);
                        cx.defer(Shell::config_changed);
                    }
                    Err(error) => {
                        report!("{error}");
                        this.failure = Some(error.into());
                    }
                }
                this.reload(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The rule at `index` as the file holds it.
    fn rule(&self, index: usize) -> Option<toml::Table> {
        self.rules.as_ref()?.get(index).cloned()
    }

    /// Begins changing `draft`, what was changed before being dropped.
    fn take_up(&mut self, draft: Draft, cx: &mut Context<Self>) {
        self.original = draft.index.and_then(|index| self.rule(index));
        self.draft = Some(draft);
        self.fields.clear();
        self.pair_faults.clear();
        self.changed(cx);
    }

    /// Stops changing the rule without writing it.
    fn drop_draft(&mut self, cx: &mut Context<Self>) {
        self.draft = None;
        self.original = None;
        self.draft_fault = None;
        self.named_now = None;
        self.fields.clear();
        self.temporarily = false;
        cx.notify();
    }

    /// The draft changed: it is checked, and the windows it names are
    /// counted once typing rests.
    fn changed(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = &self.draft else {
            return;
        };
        let checked = draft.check();
        let language = Language::of(cx);
        self.draft_fault = { checked.as_ref().err() }
            .map(|error| format!("{} {error}", language.say("The rule cannot be used:")));
        if let Some(key) = self.pair_faults.iter().next() {
            let told = language.say("Two numbers are needed:");
            self.draft_fault = Some(format!("{told} {key}"));
        }
        self.countings += 1;
        let counting = self.countings;
        cx.notify();
        let Ok(rule) = checked else {
            self.named_now = None;
            return;
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(COUNT_AFTER).await;
            let current = this.update(cx, |this, _| this.countings == counting);
            if !matches!(current, Ok(true)) {
                return;
            }
            let titles = cx
                .background_executor()
                .spawn(async move {
                    { AppWindow::enumerate().into_iter() }
                        .filter(|window| rule.matches(&Named::of(window)))
                        .map(|window| window.id.title())
                        .collect::<Vec<String>>()
                })
                .await;
            this.update(cx, |this, cx| {
                if this.countings == counting {
                    this.named_now = Some(titles);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Writes the rule being changed: in its place, or after the others.
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.clone() else {
            return;
        };
        if self.draft_fault.is_some() {
            return;
        }
        let change = match (draft.index, self.original.clone()) {
            (Some(index), Some(before)) => Change::Set(index, before, draft.table),
            _ => Change::Append(vec![draft.table]),
        };
        self.write(change, |this, cx| this.drop_draft(cx), cx);
    }

    /// Gives what the rule being changed says to the window that was read,
    /// until it closes, and writes nothing.
    fn give_until_closed(&mut self, cx: &mut Context<Self>) {
        let (Some(draft), Some(detected)) = (&self.draft, &self.detected) else {
            return;
        };
        let Ok(rule) = draft.check() else {
            return;
        };
        Rules::temporary(detected.id, rule.says().clone(), cx);
        self.drop_draft(cx);
    }

    /// Reads the window `id` aside, and takes up a new rule for it with
    /// what the rule of the window menu would name it by.
    fn detect(&mut self, id: AppWindowId, cx: &mut Context<Self>) {
        self.open = None;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx
                .background_executor()
                .spawn(async move { Detected::read(id) })
                .await;
            this.update(cx, |this, cx| {
                let Some(detected) = read else {
                    this.failure = Some(Language::of(cx).say("The window is gone.").into());
                    cx.notify();
                    return;
                };
                this.ticked = detected.ticked_at_first().into_iter().collect();
                this.detected = Some(detected);
                let mut draft = Draft::default();
                this.take_ticked(&mut draft);
                this.take_up(draft, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Writes what is ticked of the window that was read into `draft`.
    fn take_ticked(&self, draft: &mut Draft) {
        let Some(detected) = &self.detected else {
            return;
        };
        for (at, suggestion) in detected.suggestions().iter().enumerate() {
            if self.ticked.contains(&at) {
                suggestion.take_into(draft);
            }
        }
    }

    /// Hides the settings, has a window picked and opens the settings
    /// again with it read.
    fn pick(&mut self, cx: &mut Context<Self>) {
        let theme = *cx.global::<Theme>();
        let color = argb(theme.colors.primary);
        cx.defer(Popups::close);
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            cx.background_executor().timer(AWAY_AFTER).await;
            let picked = WindowPick::start(color).recv().await.ok().flatten();
            cx.update(|cx| {
                let item = picked.map(|id| format!("window:{}", id.raw()));
                Settings::open_at(SECTION, item.as_deref(), cx);
            })
            .ok();
        })
        .detach();
    }

    /// Reads the open windows aside, to choose one of.
    fn list_open(&mut self, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            cx.notify();
            return;
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let open = cx
                .background_executor()
                .spawn(async { open_windows() })
                .await;
            this.update(cx, |this, cx| {
                this.open = Some(open);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Hides the settings, has Windows choose where to write the rules of
    /// the file, writes them there and opens the settings again.
    fn export(&mut self, cx: &mut Context<Self>) {
        let rules = self.rules.clone().unwrap_or_default();
        cx.defer(Popups::close);
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            cx.background_executor().timer(AWAY_AFTER).await;
            let folder = crate::config::config_dir().unwrap_or_default();
            let Ok(chosen) = cx.update(|cx| cx.prompt_for_new_path(&folder, Some(EXPORTED))) else {
                return;
            };
            let path = match chosen.await {
                Ok(Ok(Some(path))) => Some(path),
                Ok(Err(error)) => {
                    report!("no file can be chosen: {error:#}");
                    None
                }
                _ => None,
            };
            if let Some(path) = path {
                let count = rules.len();
                let written = cx
                    .background_executor()
                    .spawn(async move { export_to(&path, rules).map(|()| path) })
                    .await;
                cx.update(|cx| tell_exported(written, count, cx)).ok();
            }
            cx.update(|cx| Settings::open_at(SECTION, None, cx)).ok();
        })
        .detach();
    }

    /// Hides the settings, has Windows choose a file of rules, adds its
    /// rules after the others if each can be used, and opens the settings
    /// again.
    fn import(&mut self, cx: &mut Context<Self>) {
        let prompt = Language::of(cx).say("Import…");
        cx.defer(Popups::close);
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            cx.background_executor().timer(AWAY_AFTER).await;
            let options = PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(SharedString::new_static(prompt)),
            };
            let Ok(chosen) = cx.update(|cx| cx.prompt_for_paths(options)) else {
                return;
            };
            let path = match chosen.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                Ok(Err(error)) => {
                    report!("no file can be chosen: {error:#}");
                    None
                }
                _ => None,
            };
            if let Some(path) = path {
                let imported = cx
                    .background_executor()
                    .spawn(async move { import_from(&path) })
                    .await;
                cx.update(|cx| {
                    tell_imported(&imported, cx);
                    if imported.is_ok() {
                        Shell::config_changed(cx);
                    }
                })
                .ok();
            }
            cx.update(|cx| Settings::open_at(SECTION, None, cx)).ok();
        })
        .detach();
    }
}

/// `color` as 0xAARRGGBB.
fn argb(color: gpui::Hsla) -> u32 {
    let rgba = gpui::Rgba::from(color);
    let byte = |part: f32| (part.clamp(0., 1.) * 255.).round() as u32;
    byte(rgba.a) << 24 | byte(rgba.r) << 16 | byte(rgba.g) << 8 | byte(rgba.b)
}

/// The text of a file of `[[rule]]` entries holding `rules`.
fn file_of(rules: Vec<toml::Table>) -> Result<String, String> {
    let mut file = toml::Table::new();
    let rules = rules.into_iter().map(toml::Value::Table).collect();
    file.insert("rule".to_owned(), toml::Value::Array(rules));
    toml::to_string(&file).map_err(|error| error.to_string())
}

/// Writes `rules` to `path` as `[[rule]]` entries.
fn export_to(path: &std::path::Path, rules: Vec<toml::Table>) -> Result<(), String> {
    let text = file_of(rules)?;
    std::fs::write(path, text)
        .map_err(|error| format!("{} cannot be written: {error}", path.display()))
}

/// The rules of the text of a file of `[[rule]]` entries, each checked as
/// the configuration reads it, or why they cannot all be used.
fn rules_in(text: &str) -> Result<Vec<toml::Table>, String> {
    let file: toml::Table = toml::from_str(text).map_err(|error| error.message().to_owned())?;
    let rules = { file.get("rule").and_then(toml::Value::as_array) }
        .ok_or("it holds no [[rule]] entries")?;
    let mut tables = Vec::new();
    for (at, rule) in rules.iter().enumerate() {
        let table = rule.as_table().ok_or("its `rule` is no list of tables")?;
        let checked = draft::Draft::new(table.clone(), None).check();
        if let Err(error) = checked {
            return Err(format!("rule {}: {error}", at + 1));
        }
        tables.push(table.clone());
    }
    Ok(tables)
}

/// Adds the rules of the file at `path` after the others; how many is
/// returned.
fn import_from(path: &std::path::Path) -> Result<usize, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    let rules = rules_in(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let count = rules.len();
    Change::Append(rules).make()?;
    Ok(count)
}

/// Tells in a notice how the export went.
fn tell_exported(written: Result<PathBuf, String>, count: usize, cx: &mut App) {
    let language = Language::of(cx);
    let (title, body) = match written {
        Ok(path) => (
            format!("{}: {count}", language.say("Window rules exported")),
            path.display().to_string(),
        ),
        Err(error) => (
            language.say("The window rules are not exported").to_owned(),
            error,
        ),
    };
    post(title, body, cx);
}

/// Tells in a notice how the import went.
fn tell_imported(imported: &Result<usize, String>, cx: &mut App) {
    let language = Language::of(cx);
    let (title, body) = match imported {
        Ok(count) => (
            format!("{}: {count}", language.say("Window rules imported")),
            String::new(),
        ),
        Err(error) => (
            language.say("The window rules are not imported").to_owned(),
            error.clone(),
        ),
    };
    post(title, body, cx);
}

fn post(title: String, body: String, cx: &mut App) {
    let notice = Notice {
        app: Language::of(cx).say("Window rules").to_owned(),
        title,
        body,
        glyph: Some(ICON_WINDOWS),
        ..Notice::default()
    };
    Banners::post(notice, cx);
}

impl RulesEditor {
    /// The card of the rules of the file.
    fn rules_card(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        if self.lifted.is_some() && !cx.has_active_drag() {
            self.lifted = None;
        }
        let tools = wrapping(&theme)
            .child(
                text_button(&theme, "rules-add", ICON_ADD, language.say("Add"), true).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.detected = None;
                        this.take_up(Draft::default(), cx);
                    }),
                ),
            )
            .child(
                text_button(
                    &theme,
                    "rules-pick",
                    ICON_PICK,
                    language.say("Pick a window"),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.pick(cx))),
            )
            .child(
                text_button(
                    &theme,
                    "rules-open",
                    ICON_WINDOWS,
                    language.say("Choose an open window"),
                    self.open.is_some(),
                )
                .on_click(cx.listener(|this, _, _, cx| this.list_open(cx))),
            )
            .child(
                text_button(
                    &theme,
                    "rules-import",
                    ICON_IMPORT,
                    language.say("Import…"),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.import(cx))),
            )
            .child(
                text_button(
                    &theme,
                    "rules-export",
                    ICON_EXPORT,
                    language.say("Export…"),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
            );
        let mut body = card(&theme, language.say("Window rules")).child(tools);
        if let Some(failure) = &self.failure {
            body = body.child(note(&theme, failure.clone()).text_color(theme.colors.primary));
        }
        if let Some(open) = &self.open {
            let mut list = wrapping(&theme);
            for (at, (id, title)) in open.iter().enumerate() {
                let id = *id;
                list = list.child(
                    chip(&theme, ("rules-open-window", at), title.clone(), false)
                        .on_click(cx.listener(move |this, _, _, cx| this.detect(id, cx))),
                );
            }
            if open.is_empty() {
                list = list.child(note(&theme, language.say("No window is open.")));
            }
            body = body.child(list);
        }
        let Some(rules) = self.rules.clone() else {
            return body
                .child(note(&theme, language.say("Loading…")))
                .into_any_element();
        };
        if rules.is_empty() {
            body = body.child(note(
                &theme,
                language.say("There are no rules of your own yet."),
            ));
        }
        let count = rules.len();
        let mark = theme.colors.primary;
        for (index, table) in rules.iter().enumerate() {
            let (names, says) = summary(table);
            let enabled = table.get("enabled").and_then(toml::Value::as_bool) != Some(false);
            let title: SharedString = match names.is_empty() {
                true => language.say("A rule that names no window").into(),
                false => names.clone().into(),
            };
            let carried = Carried {
                from: index,
                title: title.clone(),
            };
            let handle = glyph_button(&theme, ("rule-drag", index), ICON_DRAG)
                .cursor_grab()
                .on_drag(carried, |carried: &Carried, _, _, cx| {
                    let title = carried.title.clone();
                    cx.new(|_| CarriedFace { title })
                });
            let toggled = table.clone();
            let switch = still_switch(&theme, ("rule-enabled", index), enabled).on_click(
                cx.listener(move |this, _, _, cx| {
                    let mut draft = Draft::new(toggled.clone(), Some(index));
                    draft.set_enabled(!enabled);
                    this.write(
                        Change::Set(index, toggled.clone(), draft.table),
                        |_, _| {},
                        cx,
                    );
                }),
            );
            let text = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().font_weight(FontWeight::MEDIUM).child(one_line(title)))
                .child(note(&theme, one_line(says)));
            let edited = table.clone();
            let mut buttons = wrapping(&theme).flex_none().child(
                glyph_button(&theme, ("rule-edit", index), ICON_EDIT).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.detected = None;
                        this.take_up(Draft::new(edited.clone(), Some(index)), cx);
                    },
                )),
            );
            let copied = table.clone();
            buttons = buttons.child(
                glyph_button(&theme, ("rule-duplicate", index), ICON_COPY).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.write(Change::Insert(index + 1, copied.clone()), |_, _| {}, cx);
                    },
                )),
            );
            if index > 0 {
                buttons =
                    buttons.child(glyph_button(&theme, ("rule-up", index), ICON_UP).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.write(Change::Move(index, index - 1), |_, _| {}, cx)
                        }),
                    ));
            }
            if index + 1 < count {
                buttons = buttons.child(
                    glyph_button(&theme, ("rule-down", index), ICON_DOWN).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.write(Change::Move(index, index + 1), |_, _| {}, cx)
                        },
                    )),
                );
            }
            let removed = table.clone();
            buttons = buttons.child(
                glyph_button(&theme, ("rule-remove", index), ICON_REMOVE).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let editing =
                            this.draft.as_ref().and_then(|draft| draft.index) == Some(index);
                        this.write(
                            Change::Remove(index, removed.clone()),
                            move |this, cx| {
                                if editing {
                                    this.drop_draft(cx);
                                }
                            },
                            cx,
                        );
                    },
                )),
            );
            let lifted = self.lifted == Some(index);
            body = body.child(
                div()
                    .id(("rule-row", index))
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .py(theme.spacing.tiny)
                    .border_t_2()
                    .border_color(gpui::transparent_black())
                    .opacity(if lifted { LIFTED } else { 1. })
                    .drag_over::<Carried>(move |style, _, _, _| style.border_color(mark))
                    .on_drop(cx.listener(move |this, carried: &Carried, _, cx| {
                        this.lifted = None;
                        let to = if carried.from < index {
                            index - 1
                        } else {
                            index
                        };
                        if carried.from != to {
                            this.write(Change::Move(carried.from, to), |_, _| {}, cx);
                        }
                    }))
                    .child(handle)
                    .child(switch)
                    .child(text)
                    .child(buttons),
            );
        }
        body.into_any_element()
    }

    /// The card of the window that was read: what was read, to tick.
    fn detected_card(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let detected = self.detected.clone()?;
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut body = card(&theme, language.say("Detected window properties")).child(note(
            &theme,
            language.say("Tick what names the window, then take it into the rule."),
        ));
        for (at, suggestion) in detected.suggestions().into_iter().enumerate() {
            let ticked = self.ticked.contains(&at);
            let tick = still_switch(&theme, ("rule-tick", at), ticked).on_click(cx.listener(
                move |this, _, _, cx| {
                    if !this.ticked.remove(&at) {
                        this.ticked.insert(at);
                    }
                    cx.notify();
                },
            ));
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .child(tick)
                    .child(
                        div()
                            .flex_none()
                            .w(px(72.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(suggestion.key()),
                    )
                    .child(div().flex_1().min_w_0().child(one_line(suggestion.text()))),
            );
        }
        body = body.child(
            wrapping(&theme).child(
                text_button(
                    &theme,
                    "rule-take-ticked",
                    ICON_ADD,
                    language.say("Take into the rule"),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    let mut draft = this.draft.take().unwrap_or_default();
                    this.take_ticked(&mut draft);
                    let original = this.original.take();
                    this.take_up(draft, cx);
                    this.original = original;
                })),
            ),
        );
        Some(body.into_any_element())
    }

    /// The card of the sets of rules that come with Tessera.
    fn templates_card(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut body = card(&theme, language.say("Rule sets that come with Tessera")).child(note(
            &theme,
            language.say("They come before your own rules; rule_templates switches them on."),
        ));
        for name in templates::names() {
            let on = self.templates_on.iter().any(|known| known == name);
            let open = self.template_open == Some(name);
            let state = language.say(if on { "On" } else { "Off" });
            let glyph = if open { ICON_LESS } else { ICON_MORE };
            body = body.child(
                div()
                    .id(SharedString::from(format!("rule-set-{name}")))
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.template_open = (this.template_open != Some(name)).then_some(name);
                        cx.notify();
                    }))
                    .child(crate::ui::icon(&theme, glyph).text_size(theme.typography.label_size))
                    .child(div().flex_1().font_weight(FontWeight::MEDIUM).child(name))
                    .child(note(&theme, state)),
            );
            if !open {
                continue;
            }
            for (at, table) in templates::tables_of(name).into_iter().enumerate() {
                let (names, says) = summary(&table);
                let copied = table.clone();
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.spacing.small)
                        .pl(theme.spacing.medium)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(one_line(names))
                                .child(note(&theme, one_line(says))),
                        )
                        .child(
                            text_button(
                                &theme,
                                SharedString::from(format!("rule-set-copy-{name}-{at}")),
                                ICON_COPY,
                                language.say("Copy to my rules"),
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.write(Change::Append(vec![copied.clone()]), |_, _| {}, cx);
                                },
                            )),
                        ),
                );
            }
        }
        body.into_any_element()
    }
}

impl Render for RulesEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let rules = self.rules_card(cx);
        let detected = self.detected_card(cx);
        let editing = self.editor_card(window, cx);
        let templates = self.templates_card(cx);
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.medium)
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .child(rules)
            .children(detected)
            .children(editing)
            .child(templates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_action_and_the_page_name_a_window() {
        assert_eq!(
            asked_window("1234"),
            Some((AppWindowId::from_raw(1234), false))
        );
        assert_eq!(
            asked_window(" 1234,temporarily"),
            Some((AppWindowId::from_raw(1234), true))
        );
        assert_eq!(asked_window("0"), None);
        assert_eq!(asked_window("notepad"), None);
        assert_eq!(
            item_window("window:77"),
            Some((AppWindowId::from_raw(77), false))
        );
        assert_eq!(item_window("77"), None);
    }

    #[test]
    fn rules_are_exported_and_read_back() {
        let rules: Vec<toml::Table> = [
            "exe = \"a.exe\"\nfloat = true\nnot = { title = \"x\" }",
            "app = \"B\"\nopacity = 0.5\nenabled = false",
        ]
        .into_iter()
        .map(|text| toml::from_str(text).unwrap())
        .collect();
        let text = file_of(rules.clone()).unwrap();
        assert!(text.contains("[[rule]]"));
        assert_eq!(rules_in(&text).unwrap(), rules);
        assert!(
            rules_in("[[rule]]\nexe = \"a\"\nfloats = true")
                .unwrap_err()
                .starts_with("rule 1")
        );
        assert!(rules_in("title = \"x\"").is_err());
        assert!(
            rules_in("[[rule]]\nopacity = 0.5")
                .unwrap_err()
                .contains("names no windows")
        );
    }

    #[test]
    fn a_color_is_given_as_its_bytes() {
        let red = gpui::Hsla::from(gpui::Rgba {
            r: 1.,
            g: 0.,
            b: 0.,
            a: 1.,
        });
        assert_eq!(argb(red), 0xffff_0000);
    }
}
