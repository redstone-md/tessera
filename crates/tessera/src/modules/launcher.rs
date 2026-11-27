// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Finds and starts applications from the Start menu, opens the files
//! Windows Search knows or those of the folders its `folders` option
//! names, and shows what the runners answer to what is typed (see
//! crate::runners): calculations, conversions, web searches, windows,
//! commands, pages of the settings and more. Until something is written
//! the applications stand as tiles in rows, the way the Start menu of
//! Windows lists them. Widget: `launcher`.
//!
//! The answers the runners are sure of stand above the applications, the
//! others after the files. Enter does what the chosen row does, Shift+
//! Enter runs a command in a terminal, Ctrl+Shift+Enter as an
//! administrator, and Ctrl+C copies the result of a calculation or a
//! conversion. Ending the session or a process asks for Enter once more.
//!
//! The `runner` widget of modules/runner.rs is the same search in a
//! compact form: a field, and the rows only once something is typed.

use std::cell::Cell;
use std::cmp::Reverse;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    AnyElement, AnyView, App, AppContext, AsyncApp, ClipboardItem, Context, Entity, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, Modifiers, ParentElement, Pixels, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Task, WeakEntity, Window, div,
    img, point, px, relative,
};

use crate::field::TextField;
use crate::files::Files;
use crate::i18n::Language;
use crate::icons::AppIcons;
use crate::module::{Module, WidgetContext, WidgetRegistry};
use crate::modules::start::Pins;
use crate::motion::Spring;
use crate::platform::{Launch, SearchIndex, Shortcut};
use crate::popup::Popups;
use crate::runners::{
    Answer, AnswerAction, History, Query, RunnerOptions, Runners, STRONG, app_key,
};
use crate::theme::Theme;
use crate::ui::{icon, one_line};

const ICON_SEARCH: &str = "\u{e721}";
const ICON_PIN: &str = "\u{e718}";
const ICON_PINNED: &str = "\u{e841}";

/// The side of the button that pins the application of a row, or does
/// the second thing of an answer.
const PIN_BUTTON: Pixels = px(28.);

/// How high the tile of an application is, the side of its icon, how
/// wide a tile is at the least, which says how many stand in a row, and
/// how many do at the most.
const TILE_HEIGHT: Pixels = px(92.);
const TILE_ICON: Pixels = px(36.);
const TILE_MIN_WIDTH: f32 = 100.;
const TILES_IN_A_ROW: usize = 8;

/// How wide what a row says beside its name may be, for a file and for
/// an answer.
const FOLDER_WIDTH: Pixels = px(160.);
const DETAIL_WIDTH: Pixels = px(260.);

const DEFAULT_PLACEHOLDER: &str = "Search applications";

/// Rows prepared before the height of the list is known.
const ROWS_BEFORE_LAYOUT: usize = 16;

/// How many letters the search needs before it looks among the files,
/// and how many files it shows at most.
const LETTERS_FOR_FILES: usize = 2;
const FILES_SHOWN: usize = 30;

/// How deep under a folder files are looked for, the folder itself being
/// the first level, and how many are remembered at most.
const FOLDER_DEPTH: usize = 4;
const FILES_KEPT: usize = 20_000;

/// The folders whose files the search finds where the layout names none
/// and Windows Search cannot be asked: those of the user that Windows
/// keeps documents in.
const DEFAULT_FOLDERS: [&str; 6] = [
    "~/Desktop",
    "~/Documents",
    "~/Downloads",
    "~/Pictures",
    "~/Music",
    "~/Videos",
];

/// How many icons are looked for while one frame is drawn. Finding one
/// takes long enough to hold back the letters typed, so the rest of the
/// rows wait for the next frames.
const ICONS_PER_FRAME: usize = 2;

/// The attributes of a file or folder that is hidden or belongs to the
/// system.
const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;

/// How long a runner that is still finding out waits before it is asked
/// again, and how many times it is.
const ASK_AGAIN: Duration = Duration::from_millis(800);
const ASKED_AGAIN_AT_MOST: usize = 20;

pub struct LauncherModule;

impl Module for LauncherModule {
    fn name(&self) -> &'static str {
        "launcher"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("launcher", |context, cx| {
            launcher_view(context, Form::Full, cx)
        })
    }
}

/// How a launcher looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// The search of the Start menu: tiles or a list of the applications
    /// until something is typed.
    Full,
    /// The runner: a field, and rows only for what is typed, or the
    /// documents opened lately.
    Compact,
}

/// A launcher of `form` as the options of `context` set it up.
pub fn launcher_view(context: &WidgetContext, form: Form, cx: &mut App) -> AnyView {
    let language = Language::of(cx);
    let placeholder = match (context.text("placeholder"), form) {
        (Some(placeholder), _) => placeholder.to_owned(),
        (None, Form::Full) => language.say(DEFAULT_PLACEHOLDER).to_owned(),
        (None, Form::Compact) => language.say("Search, calculate, run…").to_owned(),
    };
    let folders = context.texts("folders");
    let folders = folders.map(|folders| folders.iter().map(|f| folder_named(f)).collect());
    let tiles = form == Form::Full && context.text("view") != Some("list");
    let defaults = RunnerOptions {
        recent_when_empty: form == Form::Compact,
    };
    let runners = Runners::of_widget(context, defaults);
    let setup = Setup {
        placeholder: SharedString::new(placeholder),
        folders,
        tiles,
        form,
        runners,
    };
    cx.new(|cx| Launcher::new(setup, cx)).into()
}

/// The positions in `names` of those containing `query`, ignoring case:
/// names that start with it first, then names with a word that does, then
/// the rest; within each group those `boost` ranks higher first, and
/// otherwise in their given order.
fn search<'a>(
    names: impl Iterator<Item = &'a str>,
    query: &str,
    boost: impl Fn(&str) -> u16,
) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut found: Vec<(u8, u16, usize)> = names
        .enumerate()
        .filter_map(|(index, name)| {
            let rank = crate::runners::rank(name, &query)?;
            Some((rank, boost(name), index))
        })
        .collect();
    found.sort_by_key(|&(rank, boost, _)| (rank, Reverse(boost)));
    found.into_iter().map(|(.., index)| index).collect()
}

/// `answers` in the order they are shown: the higher their score with
/// what `boost` adds for their key, the earlier; equals keep their order.
fn ranked(mut answers: Vec<Answer>, boost: impl Fn(&str) -> u16) -> Vec<Answer> {
    for answer in &mut answers {
        answer.score = answer.score.saturating_add(boost(&answer.key));
    }
    answers.sort_by_key(|answer| Reverse(answer.score));
    answers
}

/// What a row of the list opens: a shortcut of the applications or the
/// files, or an answer of a runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Shortcut(usize),
    Answer(usize),
}

/// The entry a row showed, which the rows of other answers do not move
/// the selection off: a shortcut by its place, an answer by its key, or
/// by its title where it has none.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Picked {
    Shortcut(usize),
    Answer { key: String, title: String },
}

/// What a shortcut is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    App,
    File,
}

/// The folder `name` stands for: a `~` it begins with is the folder of
/// the user.
fn folder_named(name: &str) -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    match (name.strip_prefix('~'), home) {
        (Some(rest), Some(home)) => home.join(rest.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(name),
    }
}

/// The files and folders under `folders`, each with its name, down to
/// [`FOLDER_DEPTH`] and at most [`FILES_KEPT`] of them; hidden ones, those
/// of the system and those whose names begin with a dot are left out with
/// all that is in them. The nearest to a folder come first.
fn files_under(folders: &[PathBuf]) -> Vec<Shortcut> {
    let mut found = Vec::new();
    let mut level: Vec<PathBuf> = folders.to_vec();
    for _ in 0..FOLDER_DEPTH {
        let mut deeper = Vec::new();
        for folder in &level {
            let Ok(entries) = fs::read_dir(folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || metadata.file_attributes() & HIDDEN_OR_SYSTEM != 0 {
                    continue;
                }
                if found.len() == FILES_KEPT {
                    return found;
                }
                let path = entry.path();
                if metadata.is_dir() {
                    deeper.push(path.clone());
                }
                found.push(Shortcut { name, path });
            }
        }
        level = deeper;
    }
    found
}

/// What the row of a file says beside its name: the folder it lies in.
fn folder_of(path: &Path) -> Option<String> {
    let folder = path.parent()?.file_name()?;
    Some(folder.to_string_lossy().into_owned())
}

/// The first of `count` rows of height `row` to draw and how many, for a
/// list scrolled down by `scrolled` that is `viewport` high.
fn visible_rows(count: usize, row: f32, scrolled: f32, viewport: f32) -> (usize, usize) {
    if viewport <= 0. || row <= 0. {
        return (0, count.min(ROWS_BEFORE_LAYOUT));
    }
    let first = ((scrolled.max(0.) / row) as usize).min(count);
    let shown = (viewport / row).ceil() as usize + 1;
    (first, shown.min(count - first))
}

/// How many tiles stand in a row of a list `width` wide.
fn tiles_across(width: f32) -> usize {
    ((width / TILE_MIN_WIDTH) as usize).clamp(1, TILES_IN_A_ROW)
}

/// What the keys held with Enter ask for: to run as an administrator,
/// and to run in a terminal.
fn modified(keys: &Modifiers) -> (bool, bool) {
    let admin = keys.control && keys.shift;
    (admin, keys.shift && !admin)
}

/// What a launcher is made with.
struct Setup {
    placeholder: SharedString,
    folders: Option<Vec<PathBuf>>,
    tiles: bool,
    form: Form,
    runners: Runners,
}

/// A search field over the list of Start menu shortcuts, the files of
/// the folders that are searched and the answers of the runners.
struct Launcher {
    /// The applications, and after them the files.
    shortcuts: Vec<Shortcut>,
    /// How many of `shortcuts` are applications.
    apps: usize,
    /// What the runners answered for what is written, the best first.
    answers: Vec<Answer>,
    /// What each runner answered last for what is written, by its place
    /// among `runners`: a runner asked again keeps its answers until it
    /// answers anew.
    told: Vec<Vec<Answer>>,
    runners: Runners,
    /// The runners being asked, while they are; asking again gives the
    /// question before up.
    asking: Option<Task<()>>,
    /// The action of an answer that waits for Enter once more, as ending
    /// the session does.
    armed: Option<AnswerAction>,
    form: Form,
    /// The field the search is typed into.
    field: Entity<TextField>,
    /// The rows that match the query, best first.
    matches: Vec<Row>,
    /// The rows of the applications and files among them, which the
    /// answers of the runners are put around.
    found: Vec<Row>,
    /// Whether the user moved the selection since the text changed: it
    /// then stays on its entry while the answers change around it.
    moved: bool,
    /// The position in `matches` that Enter starts.
    selected: usize,
    /// The row the selection highlight is drawn at, and the tile of that
    /// row; they follow `selected`.
    highlight: Spring,
    across: Spring,
    /// What the field said when the list was made, so that only another
    /// text starts it over: the field also tells of its caret.
    written: String,
    /// Whether the applications stand as tiles until something is
    /// written, and how many entries a row of the list holds now.
    tiles: bool,
    columns: usize,
    scroll: ScrollHandle,
    /// Whether the keys were given to the field yet.
    focused: bool,
    /// What is pinned to the Start menu, which a row can change.
    pins: Entity<Pins>,
    /// How many icons may still be looked for in the frame being drawn,
    /// and whether a row went without its icon for that.
    icons_left: Cell<usize>,
    icons_owed: Cell<bool>,
    /// Whether the files are those Windows Search finds for what is
    /// written, and not those read from folders.
    indexed: bool,
    /// The question Windows Search was asked last, while it is awaited;
    /// one asked before it is given up.
    asked: Option<Task<()>>,
    /// What was written when Windows Search gave the files held now.
    answered: Option<String>,
}

impl Launcher {
    fn new(setup: Setup, cx: &mut Context<Self>) -> Self {
        let shortcuts = Shortcut::start_menu();
        let apps = shortcuts.len();
        let field = cx.new(|cx| TextField::new(setup.placeholder, cx).with_glyph(ICON_SEARCH));
        cx.observe(&field, |this, _, cx| this.typed(cx)).detach();
        let pins = Pins::shared(cx);
        cx.observe(&pins, |_, _, cx| cx.notify()).detach();
        History::read(cx);
        let indexed = setup.folders.is_none();
        Self::read(setup.folders.unwrap_or_default(), cx);
        let mut launcher = Self {
            pins,
            matches: Vec::new(),
            found: Vec::new(),
            moved: false,
            apps,
            shortcuts,
            answers: Vec::new(),
            told: Vec::new(),
            runners: setup.runners,
            asking: None,
            armed: None,
            form: setup.form,
            field,
            selected: 0,
            highlight: Spring::new(cx.global::<Theme>().motion.spatial_fast, 0.),
            across: Spring::new(cx.global::<Theme>().motion.spatial_fast, 0.),
            written: String::new(),
            tiles: setup.tiles,
            columns: 1,
            scroll: ScrollHandle::new(),
            focused: false,
            icons_left: Cell::new(ICONS_PER_FRAME),
            icons_owed: Cell::new(false),
            indexed,
            asked: None,
            answered: None,
        };
        launcher.find(cx);
        // What the runners say before anything is typed, such as the
        // documents opened lately.
        launcher.consult(cx);
        launcher
    }

    /// Reads the files of `folders` aside, there may be many of them, and
    /// has them searched from then on.
    fn read(folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        if folders.is_empty() {
            return;
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let files = cx
                .background_executor()
                .spawn(async move { files_under(&folders) });
            let files = files.await;
            this.update(cx, |this, cx| this.take(files, cx)).ok();
        })
        .detach();
    }

    /// Has `files` be the files that are searched.
    fn take(&mut self, files: Vec<Shortcut>, cx: &mut Context<Self>) {
        self.shortcuts.truncate(self.apps);
        self.shortcuts.extend(files);
        self.find(cx);
        cx.notify();
    }

    /// Asks Windows Search for the files named like what is written, once
    /// enough letters are. Until it answers the files of the answer before
    /// stay, those of them that still match. Where it cannot be asked the
    /// folders of the user are read instead, from then on.
    fn ask(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx).trim().to_owned();
        if !self.indexed || text.chars().count() < LETTERS_FOR_FILES {
            self.asked = None;
            return;
        }
        let asked = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let found = cx.background_executor().spawn({
                let text = text.clone();
                async move { SearchIndex::files_named(&text, FILES_SHOWN) }
            });
            let found = found.await;
            this.update(cx, |this, cx| match found {
                Ok(files) => {
                    let named = |path: PathBuf| {
                        let name = path.file_name()?.to_string_lossy().into_owned();
                        Some(Shortcut { name, path })
                    };
                    let files = files.into_iter().filter_map(named).collect();
                    this.answered = Some(text);
                    this.take(files, cx);
                }
                Err(error) => {
                    report!("Windows Search finds no files, folders are read: {error:#}");
                    this.indexed = false;
                    let folders = DEFAULT_FOLDERS.map(folder_named).to_vec();
                    Self::read(folders, cx);
                }
            })
            .ok();
        });
        self.asked = Some(asked);
    }

    /// Asks the runners what they make of what is written; each answer
    /// joins the list as its runner gives it. A runner that is still
    /// finding out is asked again in a moment, while the text stays, and
    /// its answers stay until it answers again.
    fn consult(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        let language = Language::of(cx);
        let runners = self.runners.clone();
        let asking = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            for _ in 0..ASKED_AGAIN_AT_MOST {
                let query = Query::new(&text, language);
                let Ok(answers) = cx.update(|cx| runners.ask(query, cx.background_executor()))
                else {
                    return;
                };
                let mut waiting = false;
                while let Ok((runner, batch)) = answers.recv().await {
                    waiting |= batch.iter().any(|answer| answer.waiting);
                    let updated = this.update(cx, |this, cx| {
                        this.answered_by_runners(&text, runner, batch, cx);
                    });
                    if updated.is_err() {
                        return;
                    }
                }
                if !waiting {
                    return;
                }
                cx.background_executor().timer(ASK_AGAIN).await;
            }
        });
        self.asking = Some(asking);
    }

    /// Has `answers` be what the runner at `runner` answered to `text`,
    /// unless the field says something else by now or it answered the
    /// same before. Only the rows of the answers are put anew, and the
    /// selection stays on the entry the user moved it to.
    fn answered_by_runners(
        &mut self,
        text: &str,
        runner: usize,
        answers: Vec<Answer>,
        cx: &mut Context<Self>,
    ) {
        if text != self.written {
            return;
        }
        if self.told.len() <= runner {
            self.told.resize_with(runner + 1, Vec::new);
        }
        if self.told[runner] == answers {
            return;
        }
        self.told[runner] = answers;
        let picked = self.moved.then(|| self.picked()).flatten();
        let history = cx.default_global::<History>();
        self.answers = ranked(self.told.concat(), |key| history.boost(key));
        self.arrange();
        let again = |picked: Picked| self.matches.iter().position(|row| self.is(row, &picked));
        let row = picked.and_then(again).unwrap_or(self.selected);
        self.select(row, cx);
        cx.notify();
    }

    /// What the selected row is, to find it again among other rows.
    fn picked(&self) -> Option<Picked> {
        Some(match *self.matches.get(self.selected)? {
            Row::Shortcut(index) => Picked::Shortcut(index),
            Row::Answer(index) => {
                let answer = &self.answers[index];
                Picked::Answer {
                    key: answer.key.clone(),
                    title: answer.title.clone(),
                }
            }
        })
    }

    /// Whether `row` is the entry `picked` was.
    fn is(&self, row: &Row, picked: &Picked) -> bool {
        match (*row, picked) {
            (Row::Shortcut(index), Picked::Shortcut(known)) => index == *known,
            (Row::Answer(index), Picked::Answer { key, title }) => {
                let answer = &self.answers[index];
                match key.is_empty() {
                    true => answer.key.is_empty() && answer.title == *title,
                    false => answer.key == *key,
                }
            }
            _ => false,
        }
    }

    /// What the field shows and the list is searched for.
    fn text(&self, cx: &App) -> String {
        self.field.read(cx).text()
    }

    /// The field says something else: the list starts over.
    fn typed(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        if text == self.written {
            return;
        }
        self.written = text;
        self.armed = None;
        // The answers to the text before are not shown, nor done by an
        // Enter that comes before the runners answer this one.
        self.told.clear();
        self.answers.clear();
        self.moved = false;
        if !self.tiled() {
            self.columns = 1;
        }
        self.find(cx);
        self.ask(cx);
        self.consult(cx);
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.select(0, cx);
        cx.notify();
    }

    /// Whether the entries stand as tiles: nothing is looked for.
    fn tiled(&self) -> bool {
        self.tiles && self.written.trim().is_empty()
    }

    /// Has the highlight go to the selected entry.
    fn point(&mut self) {
        self.highlight
            .animate_to((self.selected / self.columns) as f32);
        self.across
            .animate_to((self.selected % self.columns) as f32);
    }

    /// What the shortcut at `index` of `shortcuts` is.
    fn kind(&self, index: usize) -> Kind {
        match index < self.apps {
            true => Kind::App,
            false => Kind::File,
        }
    }

    /// Looks for what the field asks for: the answers the runners are sure
    /// of first, then the applications, the files once enough letters
    /// are written, and the other answers last. While nothing is written
    /// the tiles show the applications alone, and the compact form only
    /// what the runners answer.
    fn find(&mut self, cx: &App) {
        self.found = self.shortcuts_found(cx);
        self.arrange();
    }

    /// The rows of the applications and files that match what is written,
    /// best first.
    fn shortcuts_found(&self, cx: &App) -> Vec<Row> {
        let text = self.text(cx);
        let typed = !text.trim().is_empty();
        let (apps, files) = self.shortcuts.split_at(self.apps);
        fn names(shortcuts: &[Shortcut]) -> impl Iterator<Item = &str> {
            shortcuts.iter().map(|shortcut| shortcut.name.as_str())
        }
        let mut matches: Vec<Row> = Vec::new();
        if typed || self.form == Form::Full {
            let history = cx.try_global::<History>();
            let boost = |name: &str| match (typed, history) {
                (true, Some(history)) => history.boost(&app_key(name)),
                _ => 0,
            };
            let found = search(names(apps), &text, boost);
            matches.extend(found.into_iter().map(Row::Shortcut));
        }
        if text.trim().chars().count() >= LETTERS_FOR_FILES {
            // What Windows Search found for these very letters is
            // shown as it is: it matches words, each on its own.
            let found = match self.answered.as_deref() == Some(text.trim()) {
                true => (0..files.len()).collect(),
                false => search(names(files), &text, |_| 0),
            };
            let found = found.into_iter().take(FILES_SHOWN);
            matches.extend(found.map(|index| Row::Shortcut(index + self.apps)));
        }
        matches
    }

    /// Puts the rows together: the answers the runners are sure of, the
    /// applications and files found, and the other answers.
    fn arrange(&mut self) {
        let mut matches: Vec<Row> = Vec::new();
        let answers = !self.tiled();
        let strong = self
            .answers
            .iter()
            .take_while(|answer| answer.score >= STRONG);
        let strong = strong.count();
        if answers {
            matches.extend((0..strong).map(Row::Answer));
        }
        matches.extend_from_slice(&self.found);
        if answers {
            matches.extend((strong..self.answers.len()).map(Row::Answer));
        }
        self.matches = matches;
        if self.selected >= self.matches.len() {
            self.selected = self.matches.len().saturating_sub(1);
        }
    }

    fn select(&mut self, row: usize, cx: &mut Context<Self>) {
        let row = row.min(self.matches.len().saturating_sub(1));
        if row != self.selected {
            self.selected = row;
            cx.notify();
        }
        self.point();
    }

    /// Selects the entry `steps` away, if there is one that far before,
    /// and scrolls it into view.
    fn step(&mut self, steps: isize, cx: &mut Context<Self>) {
        let Some(wanted) = self.selected.checked_add_signed(steps) else {
            return;
        };
        self.moved = true;
        self.select(wanted, cx);
        let row = match self.tiled() {
            true => TILE_HEIGHT,
            false => cx.global::<Theme>().launcher.row_height,
        };
        let viewport = self.scroll.bounds().size.height;
        let top = row * (self.selected / self.columns) as f32;
        let scrolled = -self.scroll.offset().y;
        let wanted = scrolled.clamp(top + row - viewport, top).max(px(0.));
        self.scroll.set_offset(point(px(0.), -wanted));
        cx.notify();
    }

    /// Does what the entry in `row` does, as Enter or a click asks with
    /// `keys` held, and closes the popup this is shown in. An answer that
    /// loses something waits for a second time first.
    fn launch(&mut self, row: usize, keys: &Modifiers, cx: &mut Context<Self>) {
        let (admin, terminal) = modified(keys);
        match self.matches.get(row).copied() {
            Some(Row::Shortcut(index)) => {
                let shortcut = &self.shortcuts[index];
                if self.kind(index) == Kind::App {
                    History::chose(&app_key(&shortcut.name), cx);
                }
                if admin {
                    Launch::new(&shortcut.path)
                        .verb("runas")
                        .named(format!("`{}`", shortcut.name))
                        .start();
                } else if !Files::open_folder(&shortcut.path, cx) {
                    shortcut.launch();
                }
            }
            Some(Row::Answer(index)) => {
                let answer = &self.answers[index];
                let action = answer.action.clone();
                if action.needs_confirmation() && self.armed.as_ref() != Some(&action) {
                    self.armed = Some(action);
                    cx.notify();
                    return;
                }
                self.armed = None;
                if action == AnswerAction::Nothing {
                    return;
                }
                History::chose(&answer.key, cx);
                action.perform(admin, terminal, cx);
            }
            None => return,
        }
        cx.defer(Popups::close);
    }

    /// Copies what the selected answer gives to copy, where the field has
    /// nothing selected to copy itself.
    fn copy(&mut self, cx: &mut Context<Self>) {
        if self.field.read(cx).has_selection() {
            return;
        }
        let Some(Row::Answer(index)) = self.matches.get(self.selected).copied() else {
            return;
        };
        if let Some(copy) = self.answers[index].copy.clone() {
            cx.write_to_clipboard(ClipboardItem::new_string(copy));
        }
    }

    /// Handles the keys that are not text; text goes to the field.
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keys = &event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "escape" => cx.defer(Popups::close),
            "enter" => self.launch(self.selected, keys, cx),
            "c" if keys.control && !keys.shift => self.copy(cx),
            "down" => self.step(self.columns as isize, cx),
            "up" => self.step(-(self.columns as isize), cx),
            // The field is empty while there are tiles, so these keys
            // have no caret to move.
            "right" if self.tiled() => self.step(1, cx),
            "left" if self.tiled() => self.step(-1, cx),
            _ => {}
        }
    }

    /// Whether the icon of the shortcut or file at `path` may be looked
    /// for in this frame; a row that may not waits for the next one.
    fn may_look_for_icon(&self, known: bool) -> bool {
        if known {
            return true;
        }
        if self.icons_left.get() == 0 {
            self.icons_owed.set(true);
            return false;
        }
        self.icons_left.set(self.icons_left.get() - 1);
        true
    }

    /// The icon of the entry in `row`, `size` wide and high: a blank
    /// square where there is none, or none was looked for yet.
    fn render_icon(
        &self,
        row: usize,
        size: Pixels,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let icons = cx.global::<AppIcons>();
        let image = match self.matches[row] {
            Row::Shortcut(index) => {
                let shortcut = &self.shortcuts[index];
                match self.kind(index) {
                    Kind::App => {
                        let known = icons.has(&shortcut.path);
                        self.may_look_for_icon(known)
                            .then(|| icons.of_shortcut(shortcut))
                            .flatten()
                    }
                    Kind::File => {
                        let known = icons.has_of_file(&shortcut.path);
                        self.may_look_for_icon(known)
                            .then(|| icons.of_file(&shortcut.path))
                            .flatten()
                    }
                }
            }
            Row::Answer(index) => {
                let answer = &self.answers[index];
                let file = answer.icon.as_deref();
                let program = file.is_some_and(|file| {
                    { file.extension() }.is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
                });
                let image = file.and_then(|file| match program {
                    true => {
                        let known = icons.has(file);
                        self.may_look_for_icon(known)
                            .then(|| icons.of_executable(file))
                            .flatten()
                    }
                    false => {
                        let known = icons.has_of_file(file);
                        self.may_look_for_icon(known)
                            .then(|| icons.of_file(file))
                            .flatten()
                    }
                });
                if image.is_none() {
                    return glyph_box(size, theme, answer.glyph, theme.colors.primary);
                }
                image
            }
        };
        match image {
            Some(image) => img(image).size(size).flex_none().into_any_element(),
            None => div()
                .size(size)
                .flex_none()
                .rounded(theme.shape.small)
                .bg(theme.colors.outline)
                .into_any_element(),
        }
    }

    /// The application in `row` as a tile: its icon over its name.
    fn render_tile(
        &self,
        row: usize,
        theme: &Theme,
        lit: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Row::Shortcut(index) = self.matches[row] else {
            return div().into_any_element();
        };
        let columns = self.columns;
        let part = 1. / columns as f32;
        let name = self.shortcuts[index].name.clone();
        let pin = self.render_pin(row, row == lit, theme, cx).map(|pin| {
            div()
                .absolute()
                .top(theme.spacing.tiny)
                .right(theme.spacing.tiny)
                .child(pin)
        });
        div()
            .id(("launcher-tile", row))
            .absolute()
            .top(TILE_HEIGHT * (row / columns) as f32)
            .left(relative(part * (row % columns) as f32))
            .w(relative(part))
            .h(TILE_HEIGHT)
            .px(theme.spacing.small)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(theme.spacing.small)
            .child(self.render_icon(row, TILE_ICON, theme, cx))
            .child(
                div()
                    .max_w_full()
                    .text_size(theme.typography.label_size)
                    .child(one_line(SharedString::new(name))),
            )
            .children(pin)
            .on_mouse_move(cx.listener(move |this, _, _, cx| this.select(row, cx)))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                this.launch(row, &event.modifiers(), cx);
            }))
            .into_any_element()
    }

    /// What the row in `row` is called and what it says beside that, in
    /// which color.
    fn describe(&self, row: usize, cx: &App) -> (String, Option<(String, Hsla, Pixels)>) {
        let theme = cx.global::<Theme>();
        let language = Language::of(cx);
        match self.matches[row] {
            Row::Shortcut(index) => {
                let shortcut = &self.shortcuts[index];
                // A file says where it lies.
                let folder = match self.kind(index) {
                    Kind::App => None,
                    Kind::File => folder_of(&shortcut.path),
                };
                let side =
                    folder.map(|folder| (folder, theme.colors.on_surface_variant, FOLDER_WIDTH));
                (shortcut.name.clone(), side)
            }
            Row::Answer(index) => {
                let answer = &self.answers[index];
                let armed = self.armed.as_ref() == Some(&answer.action);
                let side = match armed {
                    true => (
                        language.say("Press Enter again to confirm").to_owned(),
                        theme.colors.primary,
                        DETAIL_WIDTH,
                    ),
                    false => (
                        answer.detail.clone(),
                        theme.colors.on_surface_variant,
                        DETAIL_WIDTH,
                    ),
                };
                (answer.title.clone(), Some(side))
            }
        }
    }

    fn render_row(
        &self,
        row: usize,
        theme: &Theme,
        top: Pixels,
        lit: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (name, side) = self.describe(row, cx);
        let side = side.map(|(text, color, width)| {
            div()
                .flex_none()
                .max_w(width)
                .text_size(theme.typography.label_size)
                .text_color(color)
                .child(one_line(SharedString::new(text)))
        });
        let image = self.render_icon(row, theme.launcher.icon_size, theme, cx);
        let button = match self.matches[row] {
            Row::Shortcut(_) => self.render_pin(row, row == lit, theme, cx),
            Row::Answer(_) => self.render_answer_button(row, row == lit, theme, cx),
        };
        div()
            .id(("launcher-row", row))
            .absolute()
            .top(top)
            .w_full()
            .h(theme.launcher.row_height)
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .child(image)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(one_line(SharedString::new(name))),
            )
            .children(side)
            .children(button)
            .on_mouse_move(cx.listener(move |this, _, _, cx| this.select(row, cx)))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                this.launch(row, &event.modifiers(), cx);
            }))
            .into_any_element()
    }

    /// The button that pins the application of `row` to the Start menu or
    /// unpins it. It is there on the row the highlight is on (`lit`) and on
    /// pinned ones, and only if something shows the pins.
    fn render_pin(
        &self,
        row: usize,
        lit: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // Only applications are pinned.
        let Row::Shortcut(index) = self.matches[row] else {
            return None;
        };
        if self.kind(index) != Kind::App {
            return None;
        }
        let name = self.shortcuts[index].name.clone();
        let pins = self.pins.read(cx);
        let pinned = pins.holds(&name);
        if !pins.is_shown() || !(pinned || lit) {
            return None;
        }
        let (glyph, color) = if pinned {
            (ICON_PINNED, theme.colors.primary)
        } else {
            (ICON_PIN, theme.colors.on_surface_variant)
        };
        let button = round_button(("launcher-pin", row), theme, glyph, color).on_click(
            cx.listener(move |this, _, _, cx| {
                // The row under the button would start the application.
                cx.stop_propagation();
                this.pins.update(cx, |pins, cx| pins.toggle(&name, cx));
            }),
        );
        Some(button.into_any_element())
    }

    /// The button of the second thing the answer in `row` does, such as
    /// copying its result or closing its window, on the row the highlight
    /// is on.
    fn render_answer_button(
        &self,
        row: usize,
        lit: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Row::Answer(index) = self.matches[row] else {
            return None;
        };
        let (glyph, action) = self.answers[index].button.clone()?;
        if !lit {
            return None;
        }
        let color = theme.colors.on_surface_variant;
        let button = round_button(("launcher-answer", row), theme, glyph, color).on_click(
            cx.listener(move |this, _, _, cx| {
                // The row under the button would do its own action.
                cx.stop_propagation();
                action.perform(false, false, cx);
                // A closed window leaves the answers.
                if let AnswerAction::Close(_) = action {
                    this.consult(cx);
                }
            }),
        );
        Some(button.into_any_element())
    }

    /// What the list shows while it has no rows.
    fn render_empty(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let language = Language::of(cx);
        let said = match (self.form, self.written.trim().is_empty()) {
            (Form::Compact, true) => {
                language.say("Applications, windows, commands, calculations and more")
            }
            _ => language.say("Nothing matches"),
        };
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.colors.on_surface_variant)
            .child(said)
            .into_any_element()
    }

    fn render_list(
        &mut self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.matches.is_empty() {
            return self.render_empty(theme, cx);
        }
        // Tiles stand as many in a row as the list is wide for, which
        // is known once it was drawn.
        let tiled = self.tiled();
        let columns = match tiled {
            true => {
                let width = self.scroll.bounds().size.width;
                let width = match width > px(0.) {
                    true => width,
                    false => window.viewport_size().width,
                };
                tiles_across(f32::from(width))
            }
            false => 1,
        };
        if columns != self.columns {
            self.columns = columns;
            self.point();
        }
        let row = match tiled {
            true => TILE_HEIGHT,
            false => theme.launcher.row_height,
        };
        let count = self.matches.len();
        let lines = count.div_ceil(columns);
        let (first, shown) = visible_rows(
            lines,
            f32::from(row),
            -f32::from(self.scroll.offset().y),
            f32::from(self.scroll.bounds().size.height),
        );
        // The highlight moves to the selected entry; what goes with it
        // is drawn on the entry it has reached.
        let at = self.highlight.value(window).max(0.);
        let across = self.across.value(window).max(0.);
        let lit = at.round() as usize * columns + across.round() as usize;
        let part = 1. / columns as f32;
        let highlight = div()
            .absolute()
            .top(row * at)
            .left(relative(part * across))
            .w(relative(part))
            .h(row)
            .rounded(theme.shape.medium)
            .bg(theme.colors.secondary_container);
        self.icons_left.set(ICONS_PER_FRAME);
        let entries = first * columns..((first + shown) * columns).min(count);
        let rows: Vec<AnyElement> = entries
            .map(|index| match tiled {
                true => self.render_tile(index, theme, lit, cx),
                false => self.render_row(index, theme, row * index as f32, lit, cx),
            })
            .collect();
        // The rows that went without an icon get it in the next frames.
        if self.icons_owed.replace(false) {
            window.request_animation_frame();
        }
        div()
            .id("launcher-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(row * lines as f32)
                    .child(highlight)
                    .children(rows),
            )
            .into_any_element()
    }
}

/// A square of `size` with `glyph` in its middle, in `color`.
fn glyph_box(size: Pixels, theme: &Theme, glyph: &'static str, color: Hsla) -> AnyElement {
    div()
        .size(size)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color)
        .child(icon(theme, glyph))
        .into_any_element()
}

/// A round button of a row, showing `glyph` in `color`.
fn round_button(
    id: (&'static str, usize),
    theme: &Theme,
    glyph: &'static str,
    color: Hsla,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(PIN_BUTTON)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.shape.full)
        .text_color(color)
        .hover(|style| style.bg(theme.colors.surface_container_highest))
        .child(icon(theme, glyph))
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        if !self.focused {
            self.focused = true;
            self.field.read(cx).focus(window);
        }
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .child(div().flex_none().flex().child(self.field.clone()))
            .child(self.render_list(&theme, window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(title: &str, score: u16, key: &str) -> Answer {
        Answer::new(title, "", "", score, AnswerAction::Nothing).keyed(key)
    }

    #[test]
    fn better_matches_come_first() {
        let names = ["Paint", "Notepad", "Windows Terminal", "Terminal Preview"];
        let found = search(names.into_iter(), " TE ", |_| 0);
        // Starts with it, then a word starts with it, then merely contains.
        assert_eq!(found, [3, 2, 1]);
        assert_eq!(search(names.into_iter(), "", |_| 0).len(), 4);
        assert!(search(names.into_iter(), "xyz", |_| 0).is_empty());
    }

    #[test]
    fn what_was_chosen_before_comes_first_among_equals() {
        let names = ["Terminal Preview", "Terminal"];
        let boost = |name: &str| if name == "Terminal" { 40 } else { 0 };
        assert_eq!(search(names.into_iter(), "term", boost), [1, 0]);
        // A better match stays before a name chosen often.
        let names = ["Windows Terminal", "Terminal"];
        let boost = |name: &str| if name == "Windows Terminal" { 150 } else { 0 };
        assert_eq!(search(names.into_iter(), "term", boost), [1, 0]);
    }

    #[test]
    fn answers_are_ordered_by_their_score_and_their_history() {
        let answers = vec![
            answer("page", 400, "settings:Sound"),
            answer("result", 900, ""),
            answer("window", 700, "window"),
        ];
        let boost = |key: &str| if key == "settings:Sound" { 150 } else { 0 };
        let titles: Vec<String> = { ranked(answers, boost).into_iter() }
            .map(|answer| answer.title)
            .collect();
        assert_eq!(titles, ["result", "window", "page"]);
    }

    #[test]
    fn the_keys_held_with_enter_choose_how_a_command_runs() {
        let keys = |control: bool, shift: bool| Modifiers {
            control,
            shift,
            ..Modifiers::default()
        };
        assert_eq!(modified(&keys(false, false)), (false, false));
        assert_eq!(modified(&keys(false, true)), (false, true));
        assert_eq!(modified(&keys(true, true)), (true, false));
        assert_eq!(modified(&keys(true, false)), (false, false));
    }

    #[test]
    fn files_are_found_under_a_folder_without_the_hidden_ones() {
        let folder = std::env::temp_dir().join(format!("tessera-files-{}", std::process::id()));
        let inner = folder.join("one").join("two").join("three").join("four");
        fs::create_dir_all(&inner).unwrap();
        fs::create_dir_all(folder.join(".git")).unwrap();
        fs::write(folder.join("Report.txt"), "").unwrap();
        fs::write(folder.join(".git").join("config"), "").unwrap();
        fs::write(folder.join("one").join("notes.md"), "").unwrap();
        fs::write(inner.join("deep.txt"), "").unwrap();
        let found = files_under(std::slice::from_ref(&folder));
        fs::remove_dir_all(&folder).unwrap();
        let mut names: Vec<&str> = found.iter().map(|file| file.name.as_str()).collect();
        // The first level comes before what lies deeper.
        assert!(names[..2].contains(&"Report.txt") && names[..2].contains(&"one"));
        names.sort_unstable();
        // What is in `four` lies too deep, and `.git` is left out.
        assert_eq!(
            names,
            ["Report.txt", "four", "notes.md", "one", "three", "two"]
        );
        let notes = found.iter().find(|file| file.name == "notes.md").unwrap();
        assert_eq!(folder_of(&notes.path).as_deref(), Some("one"));
    }

    #[test]
    fn a_folder_may_be_named_from_the_folder_of_the_user() {
        assert_eq!(folder_named("D:/music"), PathBuf::from("D:/music"));
        if let Some(home) = std::env::var_os("USERPROFILE") {
            assert_eq!(
                folder_named("~/Documents"),
                PathBuf::from(home).join("Documents")
            );
        }
    }

    #[test]
    fn a_wider_list_holds_more_tiles_in_a_row() {
        assert_eq!(tiles_across(0.), 1);
        assert_eq!(tiles_across(390.), 3);
        assert_eq!(tiles_across(608.), 6);
        assert_eq!(tiles_across(4000.), TILES_IN_A_ROW);
    }

    #[test]
    fn only_rows_in_view_are_drawn() {
        assert_eq!(visible_rows(100, 40., 0., 0.), (0, ROWS_BEFORE_LAYOUT));
        assert_eq!(visible_rows(100, 40., 90., 200.), (2, 6));
        assert_eq!(visible_rows(100, 40., 3900., 200.), (97, 3));
        assert_eq!(visible_rows(3, 40., 0., 200.), (0, 3));
    }
}
