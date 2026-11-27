// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Shortcuts page of the settings (widget `shortcuts`, under Keys, and
//! the popup `shortcuts`): every action with its title and argument, the
//! keys that do it as chips, those the actions bring marked, and what
//! became of each: in use, given way to a key of the user, held by
//! another program, taken from Windows, or off. A search finds rows by
//! title, action and keys. A row is edited: a key changed or another one
//! added by pressing it, a key of an action turned off or given back,
//! and a key of the user changed whole. A new key can do any action,
//! with its argument checked as it is typed, start a program, open a
//! popup, change the tiling, or go to a side, a window, a button of the
//! dock or a folder. The keys of the user can be written to a scheme file
//! and read back, adding to them or in their place.
//!
//! The module also brings the action `activate-window`.

mod activate;
mod draft;
mod model;
mod recorder;
mod scheme;
mod titles;

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Context, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, PathPromptOptions, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, UniformListScrollHandle, Window, div, px,
    uniform_list,
};

use crate::actions::Actions;
use crate::config::{KeyConfig, PopupConfig, PopupPlace, WidgetSpec};
use crate::field::TextField;
use crate::i18n::Language;
use crate::keys::{self, Gesture, KeyState, NO_ACTION, Shortcuts, written_press};
use crate::module::{Module, WidgetRegistry};
use crate::motion::Interaction;
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, one_line, state_layer};

use self::draft::{Draft, Kind, joined_arguments};
use self::model::{Chip, ChipState, Line, Row};

pub use recorder::{record_with_tip, tell_written, write_keys};

/// The widget, and the popup that holds it.
const WIDGET: &str = "shortcuts";
const POPUP_WIDTH: f32 = 720.;
const POPUP_HEIGHT: f32 = 760.;

/// How high a line of the list is, and the list itself.
const LINE_HEIGHT: Pixels = px(52.);
const LIST_HEIGHT: Pixels = px(468.);

/// How high a chip is.
const CHIP_HEIGHT: Pixels = px(26.);

/// How many actions the finder of a new key shows at once.
const FOUND_AT_MOST: usize = 8;

/// The size of the notes under titles.
const NOTE_SIZE: Pixels = px(11.);

const ICON_SEARCH: &str = "\u{e721}";
const ICON_WARNING: &str = "\u{e7ba}";
const ICON_KEY: &str = "\u{e765}";

pub struct ShortcutsModule;

impl Module for ShortcutsModule {
    fn name(&self) -> &'static str {
        "shortcuts"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(WIDGET, |_, cx| cx.new(ShortcutsPage::new).into())?;
        widgets.register_popup(PopupConfig {
            name: WIDGET.to_owned(),
            place: PopupPlace::default(),
            width: POPUP_WIDTH,
            height: POPUP_HEIGHT,
            content: vec![WidgetSpec {
                widget: WIDGET.to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        actions.register(activate::NAME, activate::action())
    }
}

/// Records a combination, with a tip at the pointer, and writes a
/// `[[key]]` that brings forward the window `id`, or windows like it: of
/// its program, its packaged application or, for a program that hosts
/// many, its class. A notice tells how it went.
pub fn record_for_window(id: crate::platform::AppWindowId, cx: &mut App) {
    let Some(sought) = activate::sought_for_window(id) else {
        report!("the window has nothing to be found by again");
        return;
    };
    let title = id.title();
    record_with_tip(cx, move |press, cx| {
        let key = KeyConfig {
            press,
            action: Some(activate::NAME.to_owned()),
            arg: Some(sought),
            ..KeyConfig::default()
        };
        let written = key.clone();
        write_keys(
            move |file| file.add_key_entry(&written),
            cx,
            move |result, cx| tell_written(&key, &title, result, cx),
        );
    });
}

/// The title of what `what` does, in `language`, and what it is done
/// with, where it is done with something.
fn row_title(what: &KeyConfig, language: Language) -> (String, Option<String>) {
    let action = |name: &str| titles::title(name, language);
    if let Some(name) = &what.action {
        return (action(name), what.arg.clone());
    }
    if let Some(popup) = &what.popup {
        return (action("popup"), Some(popup.clone()));
    }
    if let Some(words) = &what.tiling {
        return (action("tiling"), Some(words.clone()));
    }
    if let Some(number) = what.dock {
        return (action("dock"), Some(number.to_string()));
    }
    if let Some(folder) = &what.files {
        return (action("files"), Some(folder.clone()));
    }
    if let Some(way) = &what.switch {
        return (action("switcher"), Some(way.clone()));
    }
    if let Some(run) = &what.run {
        let title = language.say("Start a program").to_owned();
        return (title, Some(joined_arguments(run)));
    }
    if let Some(side) = &what.focus {
        let title = language
            .say("Bring the window on a side forward")
            .to_owned();
        return (title, Some(side.clone()));
    }
    if let Some(side) = &what.carry {
        let title = language.say("Move the window to a side").to_owned();
        return (title, Some(side.clone()));
    }
    (language.say("Nothing").to_owned(), None)
}

/// What the page says became of a key.
fn state_text(state: &ChipState, language: Language) -> String {
    let said = match state {
        ChipState::InUse(KeyState::Held) => "In use",
        ChipState::InUse(KeyState::Taken) => "Taken from Windows",
        ChipState::InUse(KeyState::Refused) => "Held by another program",
        ChipState::InUse(KeyState::Off) => "Off",
        ChipState::InUse(KeyState::Button) => "Mouse buttons are not taken yet",
        ChipState::InUse(KeyState::Paused) => "Paused",
        ChipState::InUse(KeyState::Broken(why)) => {
            return format!("{}: {why}", language.say("Does nothing"));
        }
        ChipState::GivenWay(_) => "Gave way to your key",
        ChipState::DefaultsOff => "Off with the keys of Windows",
        ChipState::TurnedOff => "Turned off",
    };
    language.say(said).to_owned()
}

/// Whether a key of a row needs to be looked at.
fn troubled(chip: &Chip) -> bool {
    !chip.clashes.is_empty()
        || matches!(
            chip.state,
            ChipState::InUse(KeyState::Refused | KeyState::Broken(_))
        )
}

/// What a press is shown as: as Tessera shows keys, or as it is written
/// where it is none.
fn press_shown(press: &str, language: Language) -> String {
    Gesture::parse(press).map_or_else(|| press.to_owned(), |gesture| gesture.shown(language))
}

/// What a combination recorded is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordFor {
    /// The press of the draft.
    Draft,
    /// The key at `chip` of the row at `row`, changed.
    Change { row: usize, chip: usize },
    /// Another key of the row at `row`.
    Another { row: usize },
}

/// A combination being recorded.
struct Recording {
    target: RecordFor,
    held: gpui::SharedString,
    _listening: Task<()>,
}

/// The buttons of the page.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Control {
    Line(usize),
    NewKey,
    Import,
    Export,
    Pause,
    AddImported(bool),
    DropImported,
    Change(usize),
    TurnOff(usize),
    Reset(usize),
    Remove(usize),
    EditWhole(usize),
    Another,
    ResetRow,
    CloseRow,
    Kind(Kind),
    Found(String),
    Choice(String),
    Popup(String),
    Record,
    Browse,
    Save,
    Cancel,
}

/// What a button looks like.
enum Face {
    Chip { text: SharedString, selected: bool },
}

/// What the page keeps while the settings give way to a dialog of
/// Windows, so that it comes back as it was.
#[derive(Default)]
struct Kept {
    search: String,
    chosen: Option<KeyConfig>,
    imported: Option<Vec<KeyConfig>>,
}

impl gpui::Global for Kept {}

/// The Shortcuts page.
pub struct ShortcutsPage {
    search: Entity<TextField>,
    /// What the search holds, as the rows were found for it last.
    query: String,
    rows: Vec<Row>,
    /// The title and argument of each row, as shown.
    titles: Vec<(String, Option<String>)>,
    lines: Vec<Line>,
    scroll: UniformListScrollHandle,
    /// The row being looked at.
    chosen: Option<usize>,
    draft: Option<Draft>,
    recording: Option<Recording>,
    /// A scheme read from a file, waiting to be added or put in place.
    imported: Option<Vec<KeyConfig>>,
    interactions: HashMap<Control, Interaction>,
    /// What went wrong last, as it is told.
    failure: Option<SharedString>,
}

impl ShortcutsPage {
    fn new(cx: &mut Context<Self>) -> Self {
        let language = Language::of(cx);
        let placeholder = language.say("Search by name, action or keys");
        let search = cx.new(|cx| TextField::new(placeholder.into(), cx).with_glyph(ICON_SEARCH));
        // The field tells of every blink of its caret; only a change of
        // what is typed finds the rows again.
        cx.observe(&search, |this, field, cx| {
            let query = field.read(cx).text();
            if query != this.query {
                this.query = query;
                this.look_again(cx);
            }
        })
        .detach();
        cx.observe_global::<Shortcuts>(|this, cx| this.look_again(cx))
            .detach();
        // What was kept while a dialog of Windows was open is taken once.
        let kept = { cx.try_global::<Kept>() }.map(|kept| {
            (
                kept.search.clone(),
                kept.chosen.clone(),
                kept.imported.clone(),
            )
        });
        cx.set_global(Kept::default());
        let mut page = Self {
            search,
            query: String::new(),
            rows: Vec::new(),
            titles: Vec::new(),
            lines: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            chosen: None,
            draft: None,
            recording: None,
            imported: None,
            interactions: HashMap::new(),
            failure: None,
        };
        if let Some((search, chosen, imported)) = kept {
            page.search
                .update(cx, |field, cx| field.put_text(&search, cx));
            page.query = search;
            page.imported = imported;
            page.look_again(cx);
            page.chosen = chosen.and_then(|what| page.rows.iter().position(|row| row.what == what));
        } else {
            page.look_again(cx);
        }
        page
    }

    /// Works the rows out again from the entries in use and the search.
    fn look_again(&mut self, cx: &mut Context<Self>) {
        let chosen = self
            .chosen
            .and_then(|at| self.rows.get(at))
            .map(|row| row.what.clone());
        let (user, windows_keys) = Shortcuts::asked(cx);
        let user = user.to_vec();
        let defaults_on = windows_keys.unwrap_or_else(crate::platform::shell_mode);
        let defaults = { cx.try_global::<Actions>() }
            .map(Actions::default_keys)
            .unwrap_or_default();
        let in_use = Shortcuts::in_use_now(cx);
        self.rows = model::rows(&user, &defaults, defaults_on, &in_use);
        let language = Language::of(cx);
        self.titles = { self.rows.iter() }
            .map(|row| row_title(&row.what, language))
            .collect();
        let query = self.query.clone();
        let mut order: Vec<usize> =
            { (0..self.rows.len()).filter(|&at| self.found(at, &query, language)) }.collect();
        order.sort_by(|&one, &other| self.titles[one].cmp(&self.titles[other]));
        self.lines = model::lines(&self.rows, &order);
        self.chosen = chosen.and_then(|what| self.rows.iter().position(|row| row.what == what));
        // The buttons stand for other things now.
        self.interactions.clear();
        cx.notify();
    }

    /// Whether the row at `at` has every word of `query`, in its title,
    /// its English title, its action, its argument or its keys.
    fn found(&self, at: usize, query: &str, language: Language) -> bool {
        let row = &self.rows[at];
        let (title, argument) = &self.titles[at];
        let english = row_title(&row.what, Language::English).0;
        let mut texts: Vec<String> = vec![title.clone(), english];
        texts.extend(row.action().map(str::to_owned));
        texts.extend(argument.clone());
        for chip in &row.chips {
            texts.push(chip.press.clone());
            texts.push(press_shown(&chip.press, language));
        }
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        model::found(query, &texts)
    }

    /// The entries of the user as last configured.
    fn user(cx: &App) -> Vec<KeyConfig> {
        Shortcuts::asked(cx).0.to_vec()
    }

    /// Writes `change` of the entries, which are to be `expected` as the
    /// file holds them, so that a file changed meanwhile is not written
    /// over.
    fn write(
        &mut self,
        expected: Vec<KeyConfig>,
        change: impl FnOnce(&mut crate::config::edit::ConfigFile) -> bool + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.failure = None;
        let page = cx.entity().downgrade();
        write_keys(
            move |file| {
                let held = file.config().map(|config| config.keys).unwrap_or_default();
                held == expected && change(file)
            },
            cx,
            move |written, cx| {
                if let Err(error) = written {
                    page.update(cx, |this, cx| {
                        this.failure = Some(error.into());
                        cx.notify();
                    })
                    .ok();
                }
            },
        );
    }

    /// Adds `keys` to the entries of the user.
    fn add_entries(&mut self, keys: Vec<KeyConfig>, cx: &mut Context<Self>) {
        let expected = Self::user(cx);
        self.write(
            expected,
            move |file| keys.iter().all(|key| file.add_key_entry(key)),
            cx,
        );
    }

    /// Takes the entries of the user at `places` out.
    fn remove_entries(&mut self, mut places: Vec<usize>, cx: &mut Context<Self>) {
        places.sort_unstable();
        places.dedup();
        let expected = Self::user(cx);
        self.write(
            expected,
            move |file| places.iter().rev().all(|&at| file.remove_key_entry(at)),
            cx,
        );
    }

    /// Answers a click on `control`.
    fn act(&mut self, control: &Control, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.chosen.and_then(|at| self.rows.get(at)).cloned();
        let chip = |at: usize| row.as_ref().and_then(|row| row.chips.get(at)).cloned();
        match control {
            Control::Line(at) => {
                if let Some(Line::Row(row)) = self.lines.get(*at) {
                    self.chosen = (self.chosen != Some(*row)).then_some(*row);
                    self.draft = None;
                }
            }
            Control::NewKey => {
                self.chosen = None;
                let draft = Draft::new(None, None, cx);
                draft.press.read(cx).focus(window);
                self.draft = Some(draft);
            }
            Control::Import => self.import(cx),
            Control::Export => self.export(cx),
            Control::Pause => Shortcuts::pause(Some(!Shortcuts::paused(cx)), cx),
            Control::AddImported(replace) => {
                if let Some(scheme) = self.imported.take() {
                    let own = Self::user(cx);
                    let keys = scheme::merged(&own, &scheme, *replace);
                    self.write(own, move |file| file.set_key_entries(&keys), cx);
                }
            }
            Control::DropImported => self.imported = None,
            Control::Change(at) => {
                if chip(*at).is_some() {
                    let row = self.chosen.unwrap_or_default();
                    self.record(RecordFor::Change { row, chip: *at }, cx);
                }
            }
            Control::Another => {
                if let Some(row) = self.chosen {
                    self.record(RecordFor::Another { row }, cx);
                }
            }
            Control::TurnOff(at) => {
                if let Some(chip) = chip(*at) {
                    let off = KeyConfig {
                        press: chip.press,
                        action: Some(NO_ACTION.to_owned()),
                        ..KeyConfig::default()
                    };
                    self.add_entries(vec![off], cx);
                }
            }
            Control::Reset(at) => {
                let entry = chip(*at).and_then(|chip| match chip.state {
                    ChipState::GivenWay(entry) => Some(entry),
                    _ => chip.entry,
                });
                if let Some(entry) = entry {
                    self.remove_entries(vec![entry], cx);
                }
            }
            Control::Remove(at) => {
                if let Some(entry) = chip(*at).and_then(|chip| chip.entry) {
                    self.remove_entries(vec![entry], cx);
                }
            }
            Control::EditWhole(at) => {
                let user = Self::user(cx);
                let entry = chip(*at).and_then(|chip| chip.entry);
                if let Some((entry, key)) = entry.and_then(|at| Some((at, user.get(at)?.clone()))) {
                    self.draft = Some(Draft::new(Some((entry, key)), None, cx));
                }
            }
            Control::ResetRow => {
                if let Some(row) = &row {
                    let places = { row.chips.iter() }.filter_map(|chip| match chip.state {
                        ChipState::GivenWay(_) => None,
                        _ => chip.entry,
                    });
                    self.remove_entries(places.collect(), cx);
                }
            }
            Control::CloseRow => self.chosen = None,
            Control::Kind(kind) => {
                if let Some(draft) = &mut self.draft {
                    draft.set_kind(*kind);
                }
            }
            Control::Found(name) => {
                if let Some(draft) = &mut self.draft {
                    draft.action = Some(name.clone());
                    draft.value.read(cx).focus(window);
                }
            }
            Control::Choice(word) => {
                if let Some(draft) = &mut self.draft {
                    draft.choice = Some(word.clone());
                }
            }
            Control::Popup(name) => {
                if let Some(draft) = &self.draft {
                    let name = name.clone();
                    draft
                        .value
                        .update(cx, |field, cx| field.put_text(&name, cx));
                }
            }
            Control::Record => self.record(RecordFor::Draft, cx),
            Control::Browse => self.browse(cx),
            Control::Save => self.save(cx),
            Control::Cancel => self.draft = None,
        }
        cx.notify();
    }

    /// Records a combination for `target`; a second click stops it.
    fn record(&mut self, target: RecordFor, cx: &mut Context<Self>) {
        if self.recording.take().is_some() {
            return;
        }
        let listening = recorder::record(
            cx,
            |this: &mut Self, held, cx| {
                if let Some(recording) = &mut this.recording {
                    recording.held = recorder::held_shown(held).into();
                    cx.notify();
                }
            },
            move |this: &mut Self, recorded, cx| {
                this.recording = None;
                if let Some(press) = recorded {
                    this.recorded(target, press, cx);
                }
                cx.notify();
            },
        );
        self.recording = Some(Recording {
            target,
            held: SharedString::default(),
            _listening: listening,
        });
    }

    /// Takes the combination `press` that was recorded for `target`.
    fn recorded(&mut self, target: RecordFor, press: String, cx: &mut Context<Self>) {
        match target {
            RecordFor::Draft => {
                if let Some(draft) = &self.draft {
                    draft
                        .press
                        .update(cx, |field, cx| field.put_text(&press, cx));
                }
            }
            RecordFor::Another { row } => {
                if let Some(row) = self.rows.get(row) {
                    let key = KeyConfig {
                        press,
                        ..row.what.clone()
                    };
                    self.add_entries(vec![key], cx);
                }
            }
            RecordFor::Change { row, chip } => {
                let Some(row) = self.rows.get(row) else {
                    return;
                };
                let Some(chip) = row.chips.get(chip).cloned() else {
                    return;
                };
                let key = KeyConfig {
                    press,
                    ..row.what.clone()
                };
                match (chip.default, chip.entry) {
                    // A key of the user gets the new keys.
                    (false, Some(entry)) => {
                        let expected = Self::user(cx);
                        self.write(
                            expected,
                            move |file| file.replace_key_entry(entry, &key),
                            cx,
                        );
                    }
                    // A key an action brings is turned off, and the user
                    // gets one of the new keys that does the same.
                    _ => {
                        let mut keys = vec![key];
                        if chip.state != ChipState::TurnedOff {
                            keys.push(KeyConfig {
                                press: chip.press,
                                action: Some(NO_ACTION.to_owned()),
                                ..KeyConfig::default()
                            });
                        }
                        self.add_entries(keys, cx);
                    }
                }
            }
        }
    }

    /// Writes the draft, once it can be used.
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = &self.draft else {
            return;
        };
        let key = draft.key(cx);
        let checked = match cx.try_global::<Actions>() {
            Some(actions) => keys::check(&key, actions),
            None => Ok(()),
        };
        if let Err(why) = checked {
            self.failure = Some(why.into());
            return;
        }
        let expected = Self::user(cx);
        match draft.changes.clone() {
            Some((entry, was)) if expected.get(entry) == Some(&was) => {
                self.write(
                    expected,
                    move |file| file.replace_key_entry(entry, &key),
                    cx,
                );
            }
            Some(_) => {
                let changed = Language::of(cx).say("The configuration has changed in the meantime");
                self.failure = Some(changed.into());
                return;
            }
            None => self.add_entries(vec![key], cx),
        }
        self.draft = None;
    }

    /// Keeps what the page shows while the settings give way to a dialog.
    fn keep(&self, cx: &mut Context<Self>) {
        let kept = Kept {
            search: self.search.read(cx).text(),
            chosen: self
                .chosen
                .and_then(|at| self.rows.get(at))
                .map(|row| row.what.clone()),
            imported: self.imported.clone(),
        };
        cx.set_global(kept);
    }

    /// Opens the page again where the settings or the popup were.
    fn come_back(in_popup: bool, cx: &mut App) {
        match in_popup {
            true => Shell::toggle_popup(WIDGET, cx),
            false => crate::modules::Settings::open_at("Keys", None, cx),
        }
    }

    /// Asks for a program in the dialog of Windows and puts it in the
    /// draft; the page gives way to the dialog and comes back with it.
    fn browse(&mut self, cx: &mut Context<Self>) {
        let prompt = Language::of(cx).say("Browse");
        let in_popup = Popups::open_name(cx) == Some(WIDGET);
        let page = cx.entity().downgrade();
        self.keep(cx);
        cx.spawn(async move |_, cx: &mut AsyncApp| {
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
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let put = |this: &mut Self, cx: &mut Context<Self>| {
                if let (Some(draft), Some(path)) = (&this.draft, &path) {
                    let path = path.to_string_lossy().into_owned();
                    draft
                        .program
                        .update(cx, |field, cx| field.put_text(&path, cx));
                }
            };
            if page.update(cx, put).is_err() {
                cx.update(|cx| Self::come_back(in_popup, cx)).ok();
            }
        })
        .detach();
    }

    /// Reads a scheme the user picks in the dialog of Windows, to be added
    /// or put in place once the user says which.
    fn import(&mut self, cx: &mut Context<Self>) {
        let prompt = Language::of(cx).say("Import…");
        let in_popup = Popups::open_name(cx) == Some(WIDGET);
        self.keep(cx);
        let page = cx.entity().downgrade();
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            let options = PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(SharedString::new_static(prompt)),
            };
            let Ok(chosen) = cx.update(|cx| cx.prompt_for_paths(options)) else {
                return;
            };
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let read = cx.background_executor().spawn(async move {
                let text = std::fs::read_to_string(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                scheme::read(&text).map_err(|error| format!("{}: {error}", path.display()))
            });
            let read = read.await;
            let shown = page.update(cx, |this, cx| this.take_scheme(read.clone(), cx));
            if shown.is_err() {
                cx.update(|cx| {
                    cx.default_global::<Kept>().imported = read.ok();
                    Self::come_back(in_popup, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn take_scheme(&mut self, read: Result<Vec<KeyConfig>, String>, cx: &mut Context<Self>) {
        match read {
            Ok(keys) => self.imported = Some(keys),
            Err(error) => self.failure = Some(error.into()),
        }
        cx.notify();
    }

    /// Writes the entries of the user to a scheme file the user names in
    /// the dialog of Windows.
    fn export(&mut self, cx: &mut Context<Self>) {
        let keys = Self::user(cx);
        let note = "Keys of Tessera: [[key]] entries, as config.toml has them.\nRead back with Import… on the Shortcuts page.".to_owned();
        let folder = dirs_documents();
        let in_popup = Popups::open_name(cx) == Some(WIDGET);
        self.keep(cx);
        let page = cx.entity().downgrade();
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            let asked = cx.update(|cx| cx.prompt_for_new_path(&folder, Some(scheme::FILE_NAME)));
            let Ok(asked) = asked else {
                return;
            };
            let Ok(Ok(Some(path))) = asked.await else {
                return;
            };
            let text = scheme::written(&keys, &note);
            let written = cx
                .background_executor()
                .spawn(async move {
                    std::fs::write(&path, text)
                        .map_err(|error| format!("{}: {error}", path.display()))
                })
                .await;
            let told = page.update(cx, |this, cx| {
                this.failure = written.clone().err().map(SharedString::from);
                cx.notify();
            });
            if told.is_err() {
                cx.update(|cx| Self::come_back(in_popup, cx)).ok();
            }
        })
        .detach();
    }
}

/// The folder a scheme is offered to be written to: the user's Documents,
/// or the configuration's own.
fn dirs_documents() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    let documents = home
        .map(|home| home.join("Documents"))
        .filter(|path| path.is_dir());
    documents
        .or_else(crate::config::config_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

// Drawing.
impl ShortcutsPage {
    fn button(
        &mut self,
        control: Control,
        face: Face,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let state = { self.interactions.entry(control.clone()) }
            .or_insert_with(|| Interaction::new(&theme.motion))
            .state(window);
        let button = div()
            .id(SharedString::from(format!("shortcuts-{control:?}")))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full);
        let button = match face {
            Face::Chip { text, selected } => {
                let (base, color) = match selected {
                    true => (theme.colors.secondary_container, theme.colors.primary),
                    false => (theme.colors.surface_container, theme.colors.on_surface),
                };
                button
                    .h(CHIP_HEIGHT)
                    .px(theme.spacing.small + theme.spacing.tiny)
                    .bg(state_layer(&theme, base, state))
                    .text_color(color)
                    .child(text)
            }
        };
        let action = control.clone();
        let button = button.on_click(cx.listener(move |this, _, window, cx| {
            this.act(&action, window, cx);
        }));
        Interaction::track(button, cx, move |this: &mut Self| {
            this.interactions.get_mut(&control)
        })
        .into_any_element()
    }

    fn chip_button(
        &mut self,
        control: Control,
        text: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = Language::of(cx).say(text).into();
        let face = Face::Chip {
            text,
            selected: false,
        };
        self.button(control, face, window, cx)
    }

    /// The card at the top: the search, and what is done to all keys.
    fn head(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let paused = Shortcuts::paused(cx);
        let pause = match paused {
            true => "Resume",
            false => "Pause",
        };
        let face = Face::Chip {
            text: language.say("Add a shortcut").into(),
            selected: true,
        };
        let buttons = vec![
            self.button(Control::NewKey, face, window, cx),
            self.chip_button(Control::Import, "Import…", window, cx),
            self.chip_button(Control::Export, "Export…", window, cx),
            self.chip_button(Control::Pause, pause, window, cx),
        ];
        let (_, windows_keys) = Shortcuts::asked(cx);
        let defaults_on = windows_keys.unwrap_or_else(crate::platform::shell_mode);
        let mut notes = Vec::new();
        if paused {
            notes.push("The shortcuts are paused: only the keys of Pause the shortcuts work.");
        }
        if !defaults_on {
            notes.push("The keys of Windows are off (windows_keys), so the keys the actions bring are not used.");
        }
        let mut card = card(&theme, language.say("Shortcuts"))
            .child(note(&theme, language.say("Every action Tessera has, the keys that do it, and what became of each. A click on a row edits its keys.")))
            .children(notes.into_iter().map(|text| note(&theme, language.say(text))))
            .child(
                div()
                    .h(px(36.))
                    .flex()
                    .child(self.search.clone()),
            )
            .child(wrap(&theme).children(buttons));
        if let Some(failure) = &self.failure {
            card = card.child(warning(&theme, failure.clone()));
        }
        if let Some(imported) = &self.imported {
            let count = imported.len();
            let text = format!("{} {count}", language.say("Keys in the scheme:"));
            let face = Face::Chip {
                text: language.say("Add to your keys").into(),
                selected: true,
            };
            let add = self.button(Control::AddImported(false), face, window, cx);
            let replace =
                self.chip_button(Control::AddImported(true), "Replace your keys", window, cx);
            let drop = self.chip_button(Control::DropImported, "Cancel", window, cx);
            card = card
                .child(note(&theme, text))
                .child(wrap(&theme).child(add).child(replace).child(drop));
        }
        card.into_any_element()
    }

    /// The card of the row looked at: each of its keys with what became of
    /// it and what can be done to it.
    fn row_card(
        &mut self,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let row = self.rows.get(at)?.clone();
        let (title, argument) = self.titles.get(at)?.clone();
        let mut card = card(&theme, title);
        if let Some(argument) = argument {
            card = card.child(note(&theme, argument));
        }
        if let Some(about) = row
            .action()
            .and_then(|name| titles::argument(name, language))
        {
            let about = format!("{}: {about}", language.say("Argument"));
            card = card.child(note(&theme, about));
        }
        for (place, chip) in row.chips.iter().enumerate() {
            let recording = self.recording.as_ref().filter(|recording| {
                recording.target
                    == RecordFor::Change {
                        row: at,
                        chip: place,
                    }
            });
            let shown = match recording {
                Some(recording) => {
                    format!("{} {}", language.say(recorder::WAITING), recording.held)
                }
                None => press_shown(&chip.press, language),
            };
            let mut said = vec![state_text(&chip.state, language)];
            if chip.default {
                said.insert(0, language.say("Brought by the action").to_owned());
            }
            for other in &chip.clashes {
                let other = self.titles.get(*other).map(|(title, _)| title.clone());
                said.push(format!(
                    "{} {}",
                    language.say("Clashes with"),
                    other.unwrap_or_default()
                ));
            }
            let mut buttons = vec![self.chip_button(Control::Change(place), "Change", window, cx)];
            match (&chip.state, chip.default, chip.entry) {
                (ChipState::TurnedOff | ChipState::GivenWay(_), true, _) => {
                    buttons.push(self.chip_button(Control::Reset(place), "Reset", window, cx));
                }
                (_, true, Some(_)) => {
                    buttons.push(self.chip_button(Control::Reset(place), "Reset", window, cx));
                }
                (ChipState::InUse(_) | ChipState::DefaultsOff, true, None) => {
                    buttons.push(self.chip_button(Control::TurnOff(place), "Turn off", window, cx));
                }
                (_, false, Some(_)) => {
                    buttons.push(self.chip_button(Control::EditWhole(place), "Edit", window, cx));
                    buttons.push(self.chip_button(Control::Remove(place), "Remove", window, cx));
                }
                _ => {}
            }
            let line = div()
                .flex()
                .items_center()
                .justify_between()
                .gap(theme.spacing.small)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().font_weight(FontWeight::MEDIUM).child(one_line(shown)))
                        .child(
                            div()
                                .text_size(NOTE_SIZE)
                                .text_color(theme.colors.on_surface_variant)
                                .child(said.join(" \u{b7} ")),
                        ),
                )
                .child(wrap(&theme).flex_none().children(buttons));
            card = card.child(line);
        }
        let another = match &self.recording {
            Some(recording) if recording.target == (RecordFor::Another { row: at }) => {
                let text = format!("{} {}", language.say(recorder::WAITING), recording.held);
                let face = Face::Chip {
                    text: text.into(),
                    selected: true,
                };
                self.button(Control::Another, face, window, cx)
            }
            _ => self.chip_button(Control::Another, "Add another key", window, cx),
        };
        let mut buttons = vec![another];
        if row.has_default() {
            buttons.push(self.chip_button(Control::ResetRow, "Reset to the defaults", window, cx));
        }
        buttons.push(self.chip_button(Control::CloseRow, "Done", window, cx));
        Some(
            card.child(wrap(&theme).children(buttons))
                .into_any_element(),
        )
    }

    /// The card of the key being made or changed.
    fn draft_card(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let draft = self.draft.as_ref()?;
        let (kind, chosen_action, choice) =
            (draft.kind, draft.action.clone(), draft.choice.clone());
        let (press, finder, value, program, arguments) = (
            draft.press.clone(),
            draft.finder.clone(),
            draft.value.clone(),
            draft.program.clone(),
            draft.arguments.clone(),
        );
        let key = draft.key(cx);
        let title = match draft.changes {
            Some(_) => language.say("Change the shortcut"),
            None => language.say("Add a shortcut"),
        };
        let recording =
            { self.recording.as_ref() }.filter(|recording| recording.target == RecordFor::Draft);
        let record = match recording {
            Some(recording) => {
                let text = format!("{} {}", language.say(recorder::WAITING), recording.held);
                let face = Face::Chip {
                    text: text.into(),
                    selected: true,
                };
                self.button(Control::Record, face, window, cx)
            }
            None => self.chip_button(Control::Record, "Record", window, cx),
        };
        let mut card = card(&theme, title)
            .child(line(&theme, language.say("Keys"), div().flex_1().flex().gap(theme.spacing.small).child(field_box(press)).child(record).into_any_element()))
            .child(note(&theme, language.say("Up to four combinations after one another, with commas between them (ctrl+alt+k, t), or keys to hold and a mouse button (ctrl+back, ctrl+forward, ctrl+middle).")));
        let mut kinds = Vec::new();
        for each in Kind::ALL {
            let face = Face::Chip {
                text: language.say(each.title()).into(),
                selected: each == kind,
            };
            kinds.push(self.button(Control::Kind(each), face, window, cx));
        }
        card = card.child(line(
            &theme,
            language.say("Does"),
            wrap(&theme)
                .justify_end()
                .children(kinds)
                .into_any_element(),
        ));
        match kind {
            Kind::Action => {
                card = card.child(line(
                    &theme,
                    language.say("Action"),
                    field_box(finder.clone()).into_any_element(),
                ));
                let query = finder.read(cx).text();
                let mut found = Vec::new();
                let names = cx
                    .try_global::<Actions>()
                    .map(Actions::names)
                    .unwrap_or_default();
                for name in names.into_iter().filter(|name| name != NO_ACTION) {
                    let title = titles::title(&name, language);
                    let english = titles::title(&name, Language::English);
                    if model::found(&query, &[&title, &english, &name]) {
                        found.push((title, name));
                    }
                }
                found.sort();
                let more = found.len().saturating_sub(FOUND_AT_MOST);
                let mut buttons = Vec::new();
                for (title, name) in found.into_iter().take(FOUND_AT_MOST) {
                    let face = Face::Chip {
                        text: title.into(),
                        selected: chosen_action.as_deref() == Some(name.as_str()),
                    };
                    buttons.push(self.button(Control::Found(name), face, window, cx));
                }
                card = card.child(wrap(&theme).children(buttons));
                if more > 0 {
                    let text = format!("{} {more}", language.say("More actions match:"));
                    card = card.child(note(&theme, text));
                }
                if let Some(name) = &chosen_action {
                    let about = titles::argument(name, language)
                        .unwrap_or_else(|| language.say("It takes no argument").to_owned());
                    card = card
                        .child(line(
                            &theme,
                            language.say("Argument"),
                            field_box(value.clone()).into_any_element(),
                        ))
                        .child(note(&theme, about));
                }
            }
            Kind::Program => {
                let browse = self.chip_button(Control::Browse, "Browse", window, cx);
                card = card
                    .child(line(
                        &theme,
                        language.say("Program"),
                        div()
                            .flex_1()
                            .flex()
                            .gap(theme.spacing.small)
                            .child(field_box(program))
                            .child(browse)
                            .into_any_element(),
                    ))
                    .child(line(
                        &theme,
                        language.say("Arguments"),
                        field_box(arguments).into_any_element(),
                    ));
            }
            Kind::Popup => {
                card = card.child(line(
                    &theme,
                    language.say("Popup"),
                    field_box(value.clone()).into_any_element(),
                ));
                let mut names: Vec<String> = { cx.try_global::<WidgetRegistry>() }
                    .map(WidgetRegistry::popup_names)
                    .unwrap_or_default();
                names.extend(
                    Shell::config(cx)
                        .popups
                        .iter()
                        .map(|popup| popup.name.clone()),
                );
                names.sort();
                names.dedup();
                let now = value.read(cx).text();
                let mut buttons = Vec::new();
                for name in names {
                    let face = Face::Chip {
                        text: name.clone().into(),
                        selected: now.trim() == name,
                    };
                    buttons.push(self.button(Control::Popup(name), face, window, cx));
                }
                card = card.child(wrap(&theme).children(buttons));
            }
            Kind::Tiling | Kind::Files => {
                let label = kind.field().unwrap_or_default();
                card = card.child(line(
                    &theme,
                    language.say(label),
                    field_box(value.clone()).into_any_element(),
                ));
            }
            Kind::Focus | Kind::Move | Kind::Switch | Kind::Dock => {
                let mut buttons = Vec::new();
                for word in kind.choices() {
                    let face = Face::Chip {
                        text: (*word).into(),
                        selected: choice.as_deref() == Some(*word),
                    };
                    buttons.push(self.button(
                        Control::Choice((*word).to_owned()),
                        face,
                        window,
                        cx,
                    ));
                }
                card = card.child(wrap(&theme).justify_end().children(buttons));
            }
        }
        let checked = match cx.try_global::<Actions>() {
            Some(actions) => keys::check(&key, actions),
            None => Ok(()),
        };
        if let Err(why) = &checked {
            card = card.child(warning(&theme, why.clone().into()));
        } else if let Some(press) = written_press(&key.press) {
            let taken = self.rows.iter().enumerate().find(|(_, row)| {
                row.what != model::what_of(&key)
                    && row
                        .chips
                        .iter()
                        .any(|chip| written_press(&chip.press).as_ref() == Some(&press))
            });
            if let Some((at, _)) = taken {
                let other = self
                    .titles
                    .get(at)
                    .map(|(title, _)| title.clone())
                    .unwrap_or_default();
                let text = format!("{} {other}", language.say("These keys are given to"));
                card = card.child(note(&theme, text));
            }
        }
        let face = Face::Chip {
            text: language.say("Save").into(),
            selected: checked.is_ok(),
        };
        let save = self.button(Control::Save, face, window, cx);
        let cancel = self.chip_button(Control::Cancel, "Cancel", window, cx);
        Some(
            card.child(wrap(&theme).justify_end().child(cancel).child(save))
                .into_any_element(),
        )
    }

    /// The lines of the list at `range`.
    fn lines_at(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut drawn = Vec::new();
        for at in range {
            let Some(line) = self.lines.get(at).copied() else {
                continue;
            };
            let element = match line {
                Line::Group(group) => div()
                    .h(LINE_HEIGHT)
                    .flex()
                    .items_end()
                    .pb(theme.spacing.tiny)
                    .px(theme.spacing.small)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.primary)
                    .child(one_line(language.say(group.title())))
                    .into_any_element(),
                Line::Row(row) => self.row_line(at, row, window, cx),
            };
            drawn.push(element);
        }
        drawn
    }

    /// The line of the row at `row`, the `at`th of the list: its title,
    /// its argument and its keys.
    fn row_line(
        &mut self,
        at: usize,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let control = Control::Line(at);
        let state = { self.interactions.entry(control.clone()) }
            .or_insert_with(|| Interaction::new(&theme.motion))
            .state(window);
        let Some(shown) = self.rows.get(row) else {
            return div().h(LINE_HEIGHT).into_any_element();
        };
        let (title, argument) = self.titles[row].clone();
        let chosen = self.chosen == Some(row);
        let trouble = shown.chips.iter().any(troubled);
        let mut chips = Vec::new();
        for chip in shown.chips.iter().take(3) {
            let off = !matches!(&chip.state, ChipState::InUse(state) if *state != KeyState::Off);
            let text = press_shown(&chip.press, language);
            let color = match (off, chip.default) {
                (true, _) => theme.colors.outline,
                (false, true) => theme.colors.on_surface_variant,
                (false, false) => theme.colors.primary,
            };
            chips.push(
                div()
                    .flex_none()
                    .h(px(22.))
                    .px(theme.spacing.small)
                    .flex()
                    .items_center()
                    .rounded(theme.shape.small)
                    .border_1()
                    .border_color(color)
                    .text_size(NOTE_SIZE)
                    .text_color(color)
                    .child(text),
            );
        }
        let more = shown.chips.len().saturating_sub(3);
        let more = (more > 0).then(|| div().text_size(NOTE_SIZE).child(format!("+{more}")));
        let ground = match chosen {
            true => theme.colors.secondary_container,
            false => theme.colors.surface_container,
        };
        let mark = trouble.then(|| icon(&theme, ICON_WARNING).text_color(theme.colors.primary));
        let element = div()
            .id(SharedString::from(format!("shortcuts-line-{at}")))
            .h(LINE_HEIGHT)
            .py(px(3.))
            .child(
                div()
                    .size_full()
                    .px(theme.spacing.medium)
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .rounded(theme.shape.medium)
                    .bg(state_layer(&theme, ground, state))
                    .child(
                        icon(&theme, ICON_KEY)
                            .flex_none()
                            .text_color(theme.colors.on_surface_variant),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().min_w_0().child(one_line(title)))
                            .children(argument.map(|argument| {
                                div()
                                    .min_w_0()
                                    .text_size(NOTE_SIZE)
                                    .text_color(theme.colors.on_surface_variant)
                                    .child(one_line(argument))
                            })),
                    )
                    .children(mark)
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .gap(theme.spacing.tiny)
                            .children(chips)
                            .children(more),
                    ),
            )
            .on_click(
                cx.listener(move |this, _, window, cx| this.act(&Control::Line(at), window, cx)),
            );
        Interaction::track(element, cx, move |this: &mut Self| {
            this.interactions.get_mut(&control)
        })
        .into_any_element()
    }
}

impl Render for ShortcutsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let head = self.head(window, cx);
        let draft = self.draft_card(window, cx);
        let row = self.chosen.and_then(|at| self.row_card(at, window, cx));
        let list = match self.lines.is_empty() {
            true => note(&theme, language.say("No shortcut matches")).into_any_element(),
            false => uniform_list(
                "shortcuts-list",
                self.lines.len(),
                cx.processor(|this, range: Range<usize>, window, cx| {
                    this.lines_at(range, window, cx)
                }),
            )
            .track_scroll(self.scroll.clone())
            .h(LIST_HEIGHT)
            .into_any_element(),
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.medium)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            .child(head)
            .children(draft)
            .children(row)
            .child(
                div()
                    .rounded(theme.shape.large)
                    .bg(theme.colors.surface_container_highest)
                    .p(theme.spacing.small)
                    .child(list),
            )
    }
}

/// A card: a rounded field with the title of what it holds.
fn card(theme: &Theme, title: impl Into<SharedString>) -> gpui::Div {
    div()
        .p(theme.spacing.medium)
        .flex()
        .flex_col()
        .gap(theme.spacing.small + theme.spacing.tiny)
        .rounded(theme.shape.large)
        .bg(theme.colors.surface_container_highest)
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.primary)
                .child(title.into()),
        )
}

/// A remark in the quieter color.
fn note(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_color(theme.colors.on_surface_variant)
        .child(text.into())
}

/// Something that went wrong, marked.
fn warning(theme: &Theme, text: SharedString) -> gpui::Div {
    div()
        .flex()
        .gap(theme.spacing.small)
        .text_color(theme.colors.primary)
        .child(icon(theme, ICON_WARNING).flex_none())
        .child(div().flex_1().min_w_0().child(text))
}

/// A row of chips that continues on the next line.
fn wrap(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(theme.spacing.tiny + theme.spacing.tiny)
}

/// A line of a card: what it is about, and what is chosen there.
fn line(theme: &Theme, title: &'static str, chooser: AnyElement) -> gpui::Div {
    div()
        .min_h(CHIP_HEIGHT)
        .flex()
        .items_center()
        .justify_between()
        .gap(theme.spacing.small)
        .child(
            div()
                .flex_none()
                .w(px(96.))
                .text_color(theme.colors.on_surface_variant)
                .child(title),
        )
        .child(chooser)
}

/// A text field that takes the width it is given.
fn field_box(field: Entity<TextField>) -> gpui::Div {
    div().flex_1().min_w_0().h(px(32.)).flex().child(field)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_is_titled_by_what_it_does() {
        let key = |popup: Option<&str>, run: Option<Vec<&str>>| KeyConfig {
            popup: popup.map(str::to_owned),
            run: run.map(|run| run.into_iter().map(str::to_owned).collect()),
            ..KeyConfig::default()
        };
        let (title, argument) = row_title(
            &key(None, Some(vec!["wt.exe", "-d", "C:\\A B"])),
            Language::English,
        );
        assert_eq!(title, "Start a program");
        assert_eq!(argument.as_deref(), Some("wt.exe -d \"C:\\A B\""));
        let (title, argument) = row_title(&key(Some("launcher"), None), Language::English);
        assert_eq!(title, "Open a popup");
        assert_eq!(argument.as_deref(), Some("launcher"));
    }
}
