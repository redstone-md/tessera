// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Workspaces: either the virtual desktops of Windows, or workspaces of
//! Tessera's own, which hide the windows of every workspace but the one
//! shown. The workspaces of Tessera's own are one row for all displays,
//! or a row for each display; they are made and removed while Tessera
//! runs, between `min` and `max` of them, and laid out in `rows` rows.
//!
//! A window belongs to the workspace it was first seen on, or to all of
//! them once it is made sticky. Windows that are hidden are written down
//! before they are hidden, so that a Tessera that was terminated shows
//! them again when it is next started; one that quits shows them itself.
//! Which window lives where is kept in `workspaces-state.toml` as well,
//! so that the windows go back to their workspaces when Tessera starts
//! again.
//!
//! Every workspace has a number that stays while it exists, whatever is
//! made or removed before it ([`Workspaces::id_of`]), for what keeps
//! something of each workspace; [`Workspaces::generation`] changes
//! whenever a workspace is made or removed.

mod grid;
mod homes;
mod rename;
mod state;
mod stills;

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AsyncApp, Global, RenderImage};
use serde::Deserialize;

pub use grid::{Toward, columns, neighbour};
use homes::{Change, Homes};
pub use rename::RenamePrompt;
use state::{Saved, SavedRow, SavedWindow};
use stills::Stills;

use crate::config::config_dir;
use crate::config::edit::ConfigFile;
use crate::platform::{
    AppWindow, AppWindowId, DesktopSurvey, DisplayKey, Hotkey, Hotkeys, Modifiers, Passage,
    VirtualDesktops, WindowEvents,
};
use crate::shell::Shell;
use crate::tiling::listed;

/// How often it is checked whether something happened to the windows.
const CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// How often the windows are looked at even though nothing was heard of
/// them.
const LOOK_INTERVAL: Duration = Duration::from_secs(1);

/// The fewest and the most workspaces of its own Tessera keeps in a row.
const FEWEST: usize = 1;
const MOST: usize = 16;

/// How many workspaces the keys with a digit reach.
const DIGITS: usize = 9;

/// The file of the configuration directory that lists the hidden windows.
const RECORD: &str = "hidden-windows";

/// What is added to the place of a workspace for the number of the key
/// combination that sends a window there.
const SENDING: u32 = 16;

/// How long after a change of where the windows live it is written down,
/// so that many changes in a row are written once.
const STATE_DELAY: Duration = Duration::from_secs(2);

/// How long after windows are shown the list of hidden windows is
/// written again without them: until then it names them too, so that a
/// Tessera terminated while they come back still shows them.
const RECORD_AGAIN: Duration = Duration::from_secs(2);

/// For how long the virtual desktops of Windows are looked at after
/// something asked of them.
const SURVEY_WANTED: Duration = Duration::from_secs(5);

/// How often the virtual desktops of Windows are looked at at most.
const SURVEY_INTERVAL: Duration = Duration::from_millis(500);

/// How often the windows of the virtual desktops of Windows are looked at
/// once something asked which window is on which, when nothing asks now
/// and no window changed.
const SURVEY_AGAIN: Duration = Duration::from_secs(2);

/// What the configuration says about workspaces.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacesConfig {
    /// How many workspaces of its own Tessera keeps at first. Without it,
    /// the workspaces are the virtual desktops of Windows.
    own: Option<usize>,
    /// Whether every display has workspaces of its own, so that going to
    /// another one changes the windows of one display only.
    #[serde(default)]
    per_display: bool,
    /// The programs whose windows belong to a workspace, by the file name
    /// of the executable; every program when empty.
    #[serde(default)]
    only: Vec<String>,
    /// The programs whose windows are seen on every workspace.
    #[serde(default)]
    except: Vec<String>,
    /// The keys held with a digit to go to the workspace of that number,
    /// and with Shift as well to send the window in front there.
    keys: Option<String>,
    /// What the buttons of the workspaces say instead of their numbers,
    /// the first for the first workspace; an empty one leaves the number.
    #[serde(default)]
    names: Vec<String>,
    /// A character of the icon font for each workspace, drawn on its
    /// button beside its name or instead of its number; an empty one
    /// draws none.
    #[serde(default)]
    icons: Vec<String>,
    /// The fewest workspaces a row keeps when they are removed, 1 unless
    /// said.
    min: Option<usize>,
    /// The most a row has when they are made, 16 unless said.
    max: Option<usize>,
    /// How many rows the workspaces are laid out in, for the steps up and
    /// down and for the widget.
    rows: Option<usize>,
    /// Whether a step past the last workspace of a row, a column or the
    /// list comes to the first one.
    #[serde(default)]
    wrap: bool,
    /// Whether the widget ends with a button that makes a workspace.
    #[serde(default)]
    add_button: bool,
}

impl WorkspacesConfig {
    /// The same with only what changes where the windows are: the names,
    /// icons, limits and the grid change nothing about that.
    fn placing(&self) -> Self {
        Self {
            own: self.own,
            per_display: self.per_display,
            only: self.only.clone(),
            except: self.except.clone(),
            keys: self.keys.clone(),
            ..Self::default()
        }
    }

    /// The same, with `count` workspaces of Tessera's own where it names
    /// no number: without Explorer, as when Tessera is the shell, Windows
    /// has no virtual desktops.
    pub fn owning(&self, count: usize) -> Self {
        Self {
            own: self.own.or(Some(count)),
            ..self.clone()
        }
    }

    /// The fewest and the most workspaces a row may have. Limits that are
    /// not usable are reported and count as 1 and 16.
    fn limits(&self) -> (usize, usize) {
        let (fewest, most) = (self.min.unwrap_or(FEWEST), self.max.unwrap_or(MOST));
        if FEWEST <= fewest && fewest <= most && most <= MOST {
            return (fewest, most);
        }
        report!(
            "`workspaces.min` and `workspaces.max` are not numbers from {FEWEST} to {MOST} with min no larger than max, so they are {FEWEST} and {MOST}"
        );
        (FEWEST, MOST)
    }

    /// How many workspaces of its own Tessera keeps at first, if it keeps
    /// any, within the limits. A number that is not usable is reported and
    /// counts as none.
    fn count(&self) -> Option<usize> {
        let count = self.own?;
        if !(FEWEST..=MOST).contains(&count) {
            report!(
                "`workspaces.own` is not a number from {FEWEST} to {MOST}, so the workspaces are the virtual desktops of Windows"
            );
            return None;
        }
        let (fewest, most) = self.limits();
        Some(count.clamp(fewest, most))
    }

    /// The key combinations for `count` workspaces, of which the keys
    /// reach the first nine: one for each to go there and, unless Shift is
    /// among the keys held already, one to send a window there.
    fn hotkeys(&self, count: usize) -> Vec<Hotkey> {
        let Some(keys) = self.keys.as_deref().filter(|_| count > 0) else {
            return Vec::new();
        };
        let Some(modifiers) = Modifiers::parse(keys) else {
            report!(
                "`workspaces.keys` names keys among alt, ctrl, shift and win joined by +, not `{keys}`"
            );
            return Vec::new();
        };
        let sending = Modifiers {
            shift: true,
            ..modifiers
        };
        (0..count.min(DIGITS) as u32)
            .flat_map(|place| {
                let key = b'1' + place as u8;
                let going = Hotkey {
                    id: place,
                    modifiers,
                    key,
                };
                let sent = Hotkey {
                    id: SENDING + place,
                    modifiers: sending,
                    key,
                };
                [Some(going), (!modifiers.shift).then_some(sent)]
            })
            .flatten()
            .collect()
    }
}

/// What the configuration says about how the workspaces look and how
/// many there may be, which changes nothing about where the windows are.
#[derive(Clone, Debug, PartialEq)]
struct Looks {
    names: Vec<String>,
    icons: Vec<String>,
    fewest: usize,
    most: usize,
    rows: usize,
    wrap: bool,
    add_button: bool,
}

impl Looks {
    fn of(config: &WorkspacesConfig) -> Self {
        let (fewest, most) = config.limits();
        Self {
            names: config.names.clone(),
            icons: config.icons.clone(),
            fewest,
            most,
            rows: config.rows.unwrap_or(1).clamp(1, MOST),
            wrap: config.wrap,
            add_button: config.add_button,
        }
    }
}

impl Default for Looks {
    fn default() -> Self {
        Self::of(&WorkspacesConfig::default())
    }
}

/// How the workspaces are laid out, for the widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub rows: usize,
    pub wrap: bool,
}

/// Something to do with the workspaces, as the action `workspace` names
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goal {
    /// Go to the workspace next to the one shown.
    Toward(Toward),
    /// Go to the workspace of that number, from 1.
    Number(usize),
    /// Make a workspace and go there.
    Add,
    /// Remove the workspace shown.
    Remove,
    /// Give the workspace shown a name.
    Rename,
}

impl Goal {
    /// What `arg` names: a direction, a number, or, unless `moving` (as
    /// for `send-window`), `add`, `remove` or `rename`.
    pub fn parse(arg: &str, moving: bool) -> Option<Self> {
        let arg = arg.trim();
        if let Some(toward) = Toward::parse(arg) {
            return Some(Self::Toward(toward));
        }
        if let Ok(number) = arg.parse::<usize>() {
            return (1..=MOST).contains(&number).then_some(Self::Number(number));
        }
        match arg.to_ascii_lowercase().as_str() {
            "add" if !moving => Some(Self::Add),
            "remove" if !moving => Some(Self::Remove),
            "rename" if !moving => Some(Self::Rename),
            _ => None,
        }
    }

    /// Why an `arg` is refused, for the action `workspace` or, if
    /// `moving`, `send-window`.
    pub fn check(arg: Option<&str>, moving: bool) -> Result<(), String> {
        match arg.and_then(|arg| Self::parse(arg, moving)) {
            Some(_) => Ok(()),
            None if moving => Err(format!(
                "`send-window` takes `next`, `previous`, `left`, `right`, `up`, `down` or a number from 1 to {MOST} as `arg`"
            )),
            None => Err(format!(
                "`workspace` takes `next`, `previous`, `left`, `right`, `up`, `down`, a number from 1 to {MOST}, `add`, `remove` or `rename` as `arg`"
            )),
        }
    }
}

/// A window as an overview of the workspaces shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resident {
    pub id: AppWindowId,
    /// What the icon of the window is taken from, where it is known.
    pub program: Option<PathBuf>,
}

/// What the button of the workspace at `index` says, given the `names`
/// of the workspaces: its name, or its number where it has none.
fn label(names: &[String], index: usize) -> String {
    named(names, index).unwrap_or_else(|| (index + 1).to_string())
}

/// The name in `names` of the workspace at `index`, if it has one.
fn named(names: &[String], index: usize) -> Option<String> {
    let name = names.get(index).map(|name| name.trim());
    name.filter(|name| !name.is_empty()).map(str::to_owned)
}

/// `list` with `value` at `index`, empty entries filling the places
/// before it and none left at its end.
fn with_entry(mut list: Vec<String>, index: usize, value: &str) -> Vec<String> {
    if list.len() <= index {
        list.resize(index + 1, String::new());
    }
    list[index] = value.trim().to_owned();
    while list.last().is_some_and(String::is_empty) {
        list.pop();
    }
    list
}

/// `list` without its entry at `index`, where it has one.
fn without_entry(mut list: Vec<String>, index: usize) -> Vec<String> {
    if index < list.len() {
        list.remove(index);
    }
    while list.last().is_some_and(String::is_empty) {
        list.pop();
    }
    list
}

/// The windows of every workspace of `survey`, the first list for the
/// first desktop.
fn survey_rows(survey: &DesktopSurvey) -> Vec<Vec<Resident>> {
    let mut rows = vec![Vec::new(); survey.count];
    for window in &survey.windows {
        if let Some(row) = rows.get_mut(window.desktop) {
            row.push(Resident {
                id: window.id,
                program: window.program.clone(),
            });
        }
    }
    for row in &mut rows {
        row.sort_by_key(|resident| resident.id.raw());
    }
    rows
}

/// What [`Workspaces`] keeps of the virtual desktops of Windows, read
/// away from the thread of GPUI while something wants to know.
#[derive(Default)]
struct Survey {
    /// What was found last.
    found: Option<DesktopSurvey>,
    /// When something last asked of the desktops, and when something last
    /// asked which window is on which.
    wanted: Cell<Option<Instant>>,
    windows_wanted: Cell<Option<Instant>>,
    /// Whether something ever asked which window is on which desktop, so
    /// that the windows are kept up to date for when it asks again.
    windows_ever: Cell<bool>,
    /// Whether the desktops are being looked at.
    asking: Cell<bool>,
}

impl Survey {
    /// Notes that something asked of the desktops now, such as their
    /// names.
    fn want(&self) {
        self.wanted.set(Some(Instant::now()));
    }

    /// Notes that something asked which window is on which desktop now,
    /// which costs a question to Explorer for every window.
    fn want_windows(&self) {
        self.want();
        self.windows_wanted.set(Some(Instant::now()));
        self.windows_ever.set(true);
    }

    /// Whether the desktops are to be looked at now, the last look being
    /// `looked` ago and a window having changed since where `changed`.
    /// While something asks they are looked at every
    /// [`SURVEY_INTERVAL`]; once something asked which window is on
    /// which, they are looked at when a window changes and every
    /// [`SURVEY_AGAIN`] after that, so that what asks again finds the
    /// windows as they are.
    fn due(&self, looked: Duration, changed: bool) -> bool {
        if self.asking.get() || looked < SURVEY_INTERVAL {
            return false;
        }
        if Self::recently(&self.wanted) {
            return changed || looked >= LOOK_INTERVAL;
        }
        self.windows_ever.get() && (changed || looked >= SURVEY_AGAIN)
    }

    /// Whether the next look is to say which window is on which desktop.
    fn with_windows(&self) -> bool {
        Self::recently(&self.windows_wanted) || self.windows_ever.get()
    }

    fn recently(asked: &Cell<Option<Instant>>) -> bool {
        { asked.get() }.is_some_and(|asked| asked.elapsed() < SURVEY_WANTED)
    }
}

/// What a state that was kept says of the windows that are still there.
struct Restored {
    next_id: u64,
    /// The rows, each with its display (`None` for the row of all
    /// displays), its workspaces and the one shown.
    rows: Vec<(Option<DisplayKey>, Vec<u64>, usize)>,
    /// The windows still there: each with the display of its row, its
    /// workspace (`None` where it is sticky) and its program.
    windows: Vec<(AppWindowId, Option<DisplayKey>, Option<u64>, PathBuf)>,
}

impl Restored {
    /// What of `saved` holds now: the rows of the displays that are
    /// connected, and the windows that are the same windows.
    fn of(saved: Saved) -> Self {
        let display = |device: &str| match device.is_empty() {
            true => Some(None),
            false => DisplayKey::of_device(device).map(Some),
        };
        let rows: Vec<(Option<DisplayKey>, Vec<u64>, usize)> = { saved.rows.into_iter() }
            .filter(|row| !row.ids.is_empty())
            .filter_map(|row: SavedRow| {
                let current = row.current.min(row.ids.len() - 1);
                Some((display(&row.display)?, row.ids, current))
            })
            .collect();
        let windows = { saved.windows.into_iter() }
            .filter_map(|window: SavedWindow| {
                let id = AppWindowId::from_raw(window.window as isize);
                if !id.exists() || !id.mark().is_some_and(|mark| window.is(&mark)) {
                    return None;
                }
                let workspace = (!window.sticky).then_some(window.workspace);
                let program = PathBuf::from(&window.executable);
                Some((id, display(&window.display)?, workspace, program))
            })
            .collect();
        Self {
            next_id: saved.next_id,
            rows,
            windows,
        }
    }

    /// The windows that live on a workspace that is not shown.
    fn away(&self) -> HashSet<isize> {
        let mut away = HashSet::new();
        for (id, display, workspace, _) in &self.windows {
            let Some(workspace) = workspace else {
                continue;
            };
            let row = self.rows.iter().find(|(key, ..)| key == display);
            if row.is_some_and(|(_, ids, current)| ids.get(*current) != Some(workspace)) {
                away.insert(id.raw());
            }
        }
        away
    }
}

/// What is written of the state, before what can be found out only away
/// from the thread of GPUI is added.
struct Draft {
    /// Counts the drafts made, the first being 1.
    sequence: u64,
    own: usize,
    per_display: bool,
    next_id: u64,
    rows: Vec<(Option<DisplayKey>, Vec<u64>, usize)>,
    /// Each window with the display of its row and its workspace, `None`
    /// for a sticky one.
    windows: Vec<(AppWindowId, Option<DisplayKey>, Option<u64>)>,
}

impl Draft {
    /// The state to write, with the names of the displays and the marks
    /// of the windows, which are asked of Windows; a window whose process
    /// may not be asked is left out.
    fn complete(self) -> (Saved, u64) {
        let device =
            |display: Option<DisplayKey>| display.and_then(DisplayKey::device).unwrap_or_default();
        let rows = { self.rows.into_iter() }
            .map(|(display, ids, current)| SavedRow {
                display: device(display),
                ids,
                current,
            })
            .collect();
        let windows = { self.windows.into_iter() }
            .filter_map(|(id, display, workspace)| {
                let mark = id.mark()?;
                Some(SavedWindow {
                    window: id.raw() as i64,
                    executable: mark.executable,
                    started: mark.started,
                    display: device(display),
                    workspace: workspace.unwrap_or_default(),
                    sticky: workspace.is_none(),
                })
            })
            .collect();
        let saved = Saved {
            own: self.own,
            per_display: self.per_display,
            next_id: self.next_id,
            rows,
            windows,
        };
        (saved, self.sequence)
    }

    /// Completes the draft and writes it.
    fn write(self) {
        let (saved, sequence) = self.complete();
        saved.write(sequence);
    }
}

/// Keeps the workspaces.
pub struct Workspaces {
    /// What is configured about where the windows are.
    config: WorkspacesConfig,
    /// What is configured about the rest.
    looks: Looks,
    /// How many workspaces a row has at first.
    own: usize,
    /// The workspaces all displays have in common; with a row for each
    /// display it holds no window and says how many a row has at first.
    homes: Homes,
    /// The workspaces of each display that was seen, when each has a row
    /// of its own.
    local: Vec<(DisplayKey, Homes)>,
    /// What the icon of each window is taken from; a hidden window is
    /// not asked, so it is kept from when the window was seen.
    programs: HashMap<AppWindowId, PathBuf>,
    /// The windows seen on every workspace.
    sticky: HashSet<AppWindowId>,
    /// The number the next workspace that is made gets.
    next_id: u64,
    /// Raised whenever a workspace is made or removed.
    generation: u64,
    /// The pictures of hidden windows made into images.
    stills: Stills,
    /// What is known of the virtual desktops of Windows.
    survey: Survey,
    /// When which window lives where is to be written down, and how many
    /// times it was.
    state_due: Option<Instant>,
    drafts: u64,
    /// When the list of hidden windows is to be written again.
    record_due: Option<Instant>,
    /// What a state kept from before says, until the workspaces are made.
    restore: Option<Restored>,
    /// Counts the tasks that were started to watch the windows; only the
    /// latest goes on.
    task: u64,
}

impl Global for Workspaces {}

impl Workspaces {
    /// Shows the windows a Tessera before this one left hidden, but for
    /// those that go back to a workspace not shown, and keeps workspaces
    /// as `config` says.
    pub fn start(config: &WorkspacesConfig, cx: &mut App) {
        let restore = config.count().and_then(|own| {
            let saved = Saved::read()?;
            saved
                .fits(own, config.per_display)
                .then(|| Restored::of(saved))
        });
        let away = restore.as_ref().map(Restored::away).unwrap_or_default();
        Self::recover_except(&away);
        cx.set_global(Self {
            config: WorkspacesConfig::default(),
            looks: Looks::default(),
            own: 0,
            homes: Homes::default(),
            local: Vec::new(),
            programs: HashMap::new(),
            sticky: HashSet::new(),
            next_id: 1,
            generation: 0,
            stills: Stills::default(),
            survey: Survey::default(),
            state_due: None,
            drafts: 0,
            record_due: None,
            restore,
            task: 0,
        });
        cx.on_app_quit(|cx| {
            let this = cx.global_mut::<Self>();
            if this.owned() {
                this.draft().write();
            }
            this.show_all();
            async {}
        })
        .detach();
        Self::configure(config, cx);
    }

    /// Answers the key combination of the workspaces with the number
    /// `pressed`.
    pub fn pressed(pressed: u32, cx: &mut App) {
        let display = Self::worked_on();
        match pressed.checked_sub(SENDING) {
            Some(place) => Self::send_front(place as usize, display, cx),
            None => Self::show(pressed as usize, display, cx),
        }
    }

    /// The display that is worked on, which keys and actions mean: that
    /// of the window in front, or else the one the pointer is on.
    pub fn worked_on() -> Option<DisplayKey> {
        let front = AppWindowId::foreground();
        let windows = AppWindow::enumerate();
        let in_front = windows.iter().find(|window| Some(window.id) == front);
        in_front
            .map(|window| window.display)
            .or_else(DisplayKey::under_pointer)
    }

    /// Whether the workspaces are Tessera's own.
    fn owned(&self) -> bool {
        self.homes.count() > 0
    }

    /// Whether every display has a row of workspaces of its own.
    fn separate(&self) -> bool {
        self.config.per_display && self.owned()
    }

    /// The workspaces `display` shows, or those of the display the
    /// pointer is on; `None` where a display has shown none yet.
    fn peek(&self, display: Option<DisplayKey>) -> Option<&Homes> {
        if !self.separate() {
            return Some(&self.homes);
        }
        let display = display.or_else(DisplayKey::under_pointer)?;
        let local = self.local.iter().find(|(key, _)| *key == display);
        local.map(|(_, homes)| homes)
    }

    /// The numbers for `count` new workspaces.
    fn fresh_ids(&mut self, count: usize) -> Vec<u64> {
        let first = self.next_id;
        self.next_id += count as u64;
        (first..self.next_id).collect()
    }

    /// The same to change, made where there are none yet.
    fn at(&mut self, display: Option<DisplayKey>) -> &mut Homes {
        let display = display.or_else(DisplayKey::under_pointer);
        let (true, Some(display)) = (self.separate(), display) else {
            return &mut self.homes;
        };
        let place = match self.local.iter().position(|(key, _)| *key == display) {
            Some(place) => place,
            None => {
                let ids = self.fresh_ids(self.homes.count());
                self.local.push((display, Homes::with_ids(ids)));
                self.local.len() - 1
            }
        };
        &mut self.local[place].1
    }

    /// The workspaces the window `id` belongs to.
    fn of_window(&mut self, id: AppWindowId) -> &mut Homes {
        let holds = |(_, homes): &(DisplayKey, Homes)| homes.home.contains_key(&id);
        match self.local.iter().position(holds) {
            Some(place) => &mut self.local[place].1,
            None => &mut self.homes,
        }
    }

    /// Every row of workspaces there is.
    fn rows_mut(&mut self) -> impl Iterator<Item = &mut Homes> {
        let local = self.local.iter_mut().map(|(_, homes)| homes);
        local.chain([&mut self.homes])
    }

    /// Takes over a configuration that was read again. Where it says
    /// something else about where the windows are than the one before,
    /// every window is shown and starts on the first workspace again.
    pub fn configure(config: &WorkspacesConfig, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        this.looks = Looks::of(config);
        let own = config.count().unwrap_or(0);
        let config = &config.placing();
        if this.task > 0 {
            if this.config == *config {
                return;
            }
            this.show_all();
        }
        this.config = config.clone();
        this.own = own;
        let ids = this.fresh_ids(own);
        this.homes = Homes::with_ids(ids);
        this.local.clear();
        this.sticky.clear();
        this.survey = Survey::default();
        this.generation += 1;
        this.task += 1;
        let task = this.task;
        if let Some(restored) = this.restore.take()
            && this.owned()
        {
            this.adopt(restored);
        }
        this.set_keys();
        // Beside the desktops of Windows the window changes tell when to
        // look again at which window is on which.
        let watched = WindowEvents::watch();
        if this.owned() {
            if !watched {
                report!("Windows does not report window changes, so workspaces follow them late");
            }
            this.save_soon();
        }
        Self::watch(task, cx);
    }

    /// Registers the key combinations for the workspaces there are.
    fn set_keys(&self) {
        let rows = self.local.iter().map(|(_, homes)| homes.count());
        let count = rows.chain([self.homes.count()]).max().unwrap_or(0);
        match Hotkeys::set("workspaces", self.config.hotkeys(count)) {
            Ok(refused) if refused.is_empty() => {}
            Ok(refused) => {
                let digits: Vec<String> = refused
                    .iter()
                    .map(|id| match id.checked_sub(SENDING) {
                        Some(place) => format!("Shift+{}", place + 1),
                        None => (id + 1).to_string(),
                    })
                    .collect();
                report!(
                    "Windows did not give the workspace keys with {}: another program holds them",
                    digits.join(", ")
                );
            }
            Err(error) => report!("the workspaces have no keys: {error:#}"),
        }
    }

    /// Takes up where the windows lived before Tessera started: the rows
    /// as they were, and the windows that are still there on their
    /// workspaces, those of the workspaces not shown hidden again.
    fn adopt(&mut self, restored: Restored) {
        let (fewest, most) = (self.looks.fewest, self.looks.most);
        self.next_id = self.next_id.max(restored.next_id);
        for (display, ids, current) in restored.rows {
            if !(fewest..=most).contains(&ids.len()) {
                continue;
            }
            let mut homes = Homes::with_ids(ids);
            homes.current = current;
            match (display, self.config.per_display) {
                (None, false) => self.homes = homes,
                (Some(display), true) => self.local.push((display, homes)),
                _ => {}
            }
        }
        let now = Instant::now();
        let mut hide = Vec::new();
        for (id, display, workspace, program) in restored.windows {
            self.programs.insert(id, program);
            let Some(workspace) = workspace else {
                self.sticky.insert(id);
                if !id.is_shown() {
                    id.unhide();
                }
                continue;
            };
            let homes = match display {
                None => Some(&mut self.homes),
                Some(display) => { self.local.iter_mut() }
                    .find(|(key, _)| *key == display)
                    .map(|(_, homes)| homes),
            };
            let placed = homes.and_then(|homes| {
                let index = homes.ids.iter().position(|&kept| kept == workspace)?;
                homes.home.insert(id, index);
                (index != homes.current).then(|| homes.hidden.insert(id, now))
            });
            match placed {
                Some(_) if id.is_shown() => hide.push(id),
                Some(_) => {}
                // A window that was left hidden for a workspace that is not
                // taken up after all is shown.
                None if !id.is_shown() => id.unhide(),
                None => {}
            }
        }
        // What is hidden is written down first.
        self.write_record();
        for id in hide {
            id.hide();
        }
    }

    /// Watches the windows for the task `task`, until another one starts.
    fn watch(task: u64, cx: &mut App) {
        cx.spawn(async move |cx: &mut AsyncApp| {
            let mut seen = None;
            let mut looked = Instant::now();
            loop {
                let going = cx.update(|cx| Self::tick(task, &mut seen, &mut looked, cx));
                if !matches!(going, Ok(true)) {
                    break;
                }
                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        })
        .detach();
    }

    /// Looks at the windows if something happened to them or a while
    /// passed, writes down what is due, and asks of the virtual desktops of
    /// Windows where something wants to know. Whether the task `task` goes
    /// on is returned.
    fn tick(task: u64, seen: &mut Option<u64>, looked: &mut Instant, cx: &mut App) -> bool {
        // Read first: the global is taken to change only where something
        // is to change, since taking it tells everything that observes the
        // workspaces.
        let this = cx.global::<Self>();
        if this.task != task {
            return false;
        }
        let now = WindowEvents::generation();
        let changed = *seen != Some(now);
        if !this.owned() {
            let survey = &this.survey;
            if survey.due(looked.elapsed(), changed) {
                (*seen, *looked) = (Some(now), Instant::now());
                let windows = survey.with_windows();
                survey.asking.set(true);
                Self::ask_survey(task, windows, cx);
            }
            return true;
        }
        let due = changed || looked.elapsed() >= LOOK_INTERVAL;
        let elapsed = |when: Option<Instant>| when.is_some_and(|when| when <= Instant::now());
        let record = elapsed(this.record_due);
        let state = elapsed(this.state_due);
        if !(due || record || state || this.stills.dropped()) {
            return true;
        }
        let this = cx.global_mut::<Self>();
        if due {
            (*seen, *looked) = (Some(now), Instant::now());
            this.look();
        }
        if record {
            this.record_due = None;
            this.write_record();
        }
        let draft = state.then(|| {
            this.state_due = None;
            this.draft()
        });
        // GPUI keeps what it drew of an image until it is told to let go.
        for image in this.stills.take_dropped() {
            cx.drop_image(image, None);
        }
        if let Some(draft) = draft {
            cx.background_executor()
                .spawn(async move { draft.write() })
                .detach();
        }
        true
    }

    /// Asks of the virtual desktops of Windows, and with `windows` which
    /// window is on which, and keeps the answer for the task `task`. Where
    /// desktops were made or removed, [`Self::generation`] is raised.
    fn ask_survey(task: u64, windows: bool, cx: &mut App) {
        let answer = VirtualDesktops::survey(windows);
        cx.spawn(async move |cx: &mut AsyncApp| {
            let found = answer.recv().await.ok();
            cx.update(|cx| {
                let this = cx.global::<Self>();
                if this.task != task {
                    return;
                }
                this.survey.asking.set(false);
                let Some(found) = found else {
                    return;
                };
                if this.survey.found.as_ref() == Some(&found) {
                    return;
                }
                let before = this.survey.found.as_ref().map(|found| &found.ids);
                let remade = before != Some(&found.ids);
                let this = cx.global_mut::<Self>();
                if remade {
                    this.generation += 1;
                }
                this.survey.found = Some(found);
            })
            .ok();
        })
        .detach();
    }

    /// Has which window lives where written down soon.
    fn save_soon(&mut self) {
        if self.state_due.is_none() {
            self.state_due = Some(Instant::now() + STATE_DELAY);
        }
    }

    /// What is written down of the workspaces and their windows.
    fn draft(&mut self) -> Draft {
        self.drafts += 1;
        let own = self.own;
        let mut rows = Vec::new();
        let mut windows = Vec::new();
        let mut add = |display: Option<DisplayKey>, homes: &Homes| {
            rows.push((display, homes.ids.clone(), homes.current));
            for (&id, &home) in &homes.home {
                windows.push((id, display, homes.ids.get(home).copied()));
            }
        };
        match self.separate() {
            true => {
                for (display, homes) in &self.local {
                    add(Some(*display), homes);
                }
            }
            false => add(None, &self.homes),
        }
        let sticky = self.sticky.iter().filter(|id| id.exists());
        windows.extend(sticky.map(|&id| (id, None, None)));
        Draft {
            sequence: self.drafts,
            own,
            per_display: self.config.per_display,
            next_id: self.next_id,
            rows,
            windows,
        }
    }

    /// How many workspaces there are and which one `display` shows:
    /// Tessera's own, or else the virtual desktops of Windows.
    pub fn now(display: Option<DisplayKey>, cx: &App) -> VirtualDesktops {
        let this = cx.global::<Self>();
        if !this.owned() {
            return VirtualDesktops::now();
        }
        let homes = this.peek(display);
        VirtualDesktops {
            count: homes.map_or(this.homes.count(), Homes::count),
            current: homes.map_or(0, |homes| homes.current),
        }
    }

    /// Whether the workspaces are Tessera's own, which take windows of
    /// other programs; the virtual desktops of Windows do not.
    pub fn own(cx: &App) -> bool {
        cx.global::<Self>().owned()
    }

    /// What the button of the workspace at `index` says: its name in the
    /// configuration, or the name Windows gives its virtual desktop, or
    /// its number where it has none.
    pub fn label(index: usize, cx: &App) -> String {
        let this = cx.global::<Self>();
        if let Some(name) = named(&this.looks.names, index) {
            return name;
        }
        if !this.owned() {
            this.survey.want();
            let found = this.survey.found.as_ref();
            let name = found.and_then(|found| found.names.get(index)?.clone());
            if let Some(name) = name {
                return name;
            }
        }
        label(&[], index)
    }

    /// The character of the icon font the button of the workspace at
    /// `index` shows, if it has one.
    pub fn icon(index: usize, cx: &App) -> Option<String> {
        named(&cx.global::<Self>().looks.icons, index)
    }

    /// Whether the workspace at `index` has a name of its own, so that its
    /// button says that and not its number.
    pub fn is_named(index: usize, cx: &App) -> bool {
        Self::label(index, cx) != label(&[], index)
    }

    /// How the workspaces are laid out.
    pub fn grid(cx: &App) -> Grid {
        let looks = &cx.global::<Self>().looks;
        Grid {
            rows: looks.rows,
            wrap: looks.wrap,
        }
    }

    /// Whether the widget offers a button that makes a workspace.
    pub fn offers_add(cx: &App) -> bool {
        cx.global::<Self>().looks.add_button
    }

    /// Whether a workspace can be made on `display` now: below the most
    /// there may be, or always for the virtual desktops of Windows.
    pub fn can_add(display: Option<DisplayKey>, cx: &App) -> bool {
        let this = cx.global::<Self>();
        !this.owned() || Self::now(display, cx).count < this.looks.most
    }

    /// Whether the workspace at `index` of `display` can be removed now:
    /// above the fewest there may be, and for the virtual desktops of
    /// Windows the one shown alone, where there is another.
    pub fn can_remove(index: usize, display: Option<DisplayKey>, cx: &App) -> bool {
        let this = cx.global::<Self>();
        let desktops = Self::now(display, cx);
        match this.owned() {
            true => index < desktops.count && desktops.count > this.looks.fewest.max(1),
            false => index == desktops.current && desktops.count > 1,
        }
    }

    /// The number that stays of the workspace at `index` of `display`,
    /// whatever is made or removed before it; `None` where there is no
    /// such workspace, or where every display has its own and `display`
    /// has shown none yet. For the virtual desktops of Windows it is made
    /// of the identifier of the desktop, once they were looked at.
    #[allow(
        dead_code,
        reason = "for the tiling, which keeps a layout for each workspace"
    )]
    pub fn id_of(display: Option<DisplayKey>, index: usize, cx: &App) -> Option<u64> {
        let this = cx.global::<Self>();
        if !this.owned() {
            this.survey.want();
            return this.survey.found.as_ref()?.ids.get(index).copied();
        }
        this.peek(display)?.ids.get(index).copied()
    }

    /// Where the workspace numbered `id` is on `display` now.
    #[allow(
        dead_code,
        reason = "for the tiling, which keeps a layout for each workspace"
    )]
    pub fn index_of(display: Option<DisplayKey>, id: u64, cx: &App) -> Option<usize> {
        let this = cx.global::<Self>();
        let ids = match this.owned() {
            true => &this.peek(display)?.ids,
            false => {
                this.survey.want();
                &this.survey.found.as_ref()?.ids
            }
        };
        ids.iter().position(|&kept| kept == id)
    }

    /// A number that changes whenever a workspace is made or removed, or
    /// the workspaces start over.
    pub fn generation(cx: &App) -> u64 {
        cx.global::<Self>().generation
    }

    /// The still picture of the window `id` while it is hidden on another
    /// workspace, taken as it was seen leaving; `None` for a window that is
    /// shown, and for one hidden without a picture, as a minimized window
    /// is, which the caller shows by its icon.
    #[allow(
        dead_code,
        reason = "for the task view, the overview and the switcher, which show hidden windows"
    )]
    pub fn still(id: AppWindowId, cx: &mut App) -> Option<Arc<RenderImage>> {
        // The images let go of here are dropped from what GPUI drew by the
        // next look at the windows, when no window of GPUI is being drawn.
        cx.global_mut::<Self>().stills.of(id)
    }

    /// Goes to the workspace at `index`, on `display` where every display
    /// has its own.
    pub fn show(index: usize, display: Option<DisplayKey>, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if !this.owned() {
            if !VirtualDesktops::now().show(index) {
                report!("Windows did not take the keys that go to another desktop");
            }
            return;
        }
        this.look();
        let change = this.at(display).go(index, Instant::now());
        this.apply(change);
    }

    /// Goes to the workspace next to the one `display` shows in the
    /// direction `toward`, if there is one.
    pub fn step(toward: Toward, display: Option<DisplayKey>, cx: &mut App) {
        if let Some(index) = Self::neighbour_of(toward, display, cx) {
            Self::show(index, display, cx);
        }
    }

    /// The workspace next to the one `display` shows in the direction
    /// `toward`.
    fn neighbour_of(toward: Toward, display: Option<DisplayKey>, cx: &App) -> Option<usize> {
        let desktops = Self::now(display, cx);
        let looks = &cx.global::<Self>().looks;
        neighbour(
            desktops.current,
            desktops.count,
            looks.rows,
            toward,
            looks.wrap,
        )
    }

    /// Sends the window that was last in front, on `display` where every
    /// display has its own workspaces, to the workspace at `index`. The
    /// virtual desktops of Windows offer no way to do that.
    pub fn send_front(index: usize, display: Option<DisplayKey>, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if !this.owned() {
            return;
        }
        this.look();
        let change = this.at(display).send(index, Instant::now());
        this.apply(change);
    }

    /// Does what `goal` names on the display that is worked on.
    pub fn pursue(goal: Goal, cx: &mut App) {
        let display = Self::worked_on();
        let current = Self::now(display, cx).current;
        match goal {
            Goal::Toward(toward) => Self::step(toward, display, cx),
            Goal::Number(number) => Self::show(number - 1, display, cx),
            Goal::Add => {
                if let Some(index) = Self::add(display, cx) {
                    Self::show(index, display, cx);
                }
            }
            Goal::Remove => {
                Self::remove(current, display, cx);
            }
            Goal::Rename if Self::own(cx) => RenamePrompt::open_near(current, display, cx),
            Goal::Rename => {
                report!(
                    "the virtual desktops of Windows are renamed in its task view: Windows offers other programs no way to rename them"
                );
            }
        }
    }

    /// Sends the window that was last in front on the display that is
    /// worked on to the workspace `goal` names.
    pub fn send_window(goal: Goal, cx: &mut App) {
        let display = Self::worked_on();
        let index = match goal {
            Goal::Toward(toward) => Self::neighbour_of(toward, display, cx),
            Goal::Number(number) => Some(number - 1),
            _ => None,
        };
        if let Some(index) = index {
            Self::send_front(index, display, cx);
        }
    }

    /// Makes a workspace at the end of the row of `display`, below the
    /// most there may be, without going there; its place is returned. For
    /// the virtual desktops of Windows, Explorer is asked to make one with
    /// its shortcut, and goes there itself.
    pub fn add(display: Option<DisplayKey>, cx: &mut App) -> Option<usize> {
        let this = cx.global_mut::<Self>();
        if !this.owned() {
            if !VirtualDesktops::add() {
                report!("Windows did not take the keys that make a desktop");
            }
            return None;
        }
        let most = this.looks.most;
        let ids = this.fresh_ids(1);
        let homes = this.at(display);
        if homes.count() >= most {
            return None;
        }
        let index = homes.add(ids[0]);
        this.generation += 1;
        this.set_keys();
        this.save_soon();
        Some(index)
    }

    /// Removes the workspace at `index` of `display`, above the fewest
    /// there may be. Its windows go to the workspace before it (after it,
    /// for the first); what is hidden after that is written down before
    /// any window is shown. Where the workspaces are one row for all
    /// displays, the name and icon of the workspace go with it from the
    /// configuration. For the virtual desktops of Windows only the one
    /// shown is removed, by Explorer through its shortcut. Whether a
    /// workspace was removed is returned.
    pub fn remove(index: usize, display: Option<DisplayKey>, cx: &mut App) -> bool {
        if !Self::can_remove(index, display, cx) {
            return false;
        }
        let this = cx.global_mut::<Self>();
        if !this.owned() {
            let removed = VirtualDesktops::remove_current();
            if !removed {
                report!("Windows did not take the keys that remove a desktop");
            }
            return removed;
        }
        this.look();
        let change = this.at(display).remove(index);
        this.generation += 1;
        this.apply(change);
        this.set_keys();
        if !this.separate() {
            let looks = this.looks.clone();
            let names = without_entry(looks.names.clone(), index);
            let icons = without_entry(looks.icons.clone(), index);
            Self::write_lists(names != looks.names, names, icons != looks.icons, icons, cx);
        }
        true
    }

    /// Names the workspace at `index` `name` in the configuration, as
    /// `names` of `[workspaces]`; an empty name gives it its number back.
    /// The names follow the places of the workspaces, as on every display.
    pub fn rename(index: usize, name: &str, cx: &mut App) {
        let names = with_entry(cx.global::<Self>().looks.names.clone(), index, name);
        Self::write_lists(true, names, false, Vec::new(), cx);
    }

    /// Gives the workspace at `index` the icon `glyph`, a character of the
    /// icon font, in the configuration, as `icons` of `[workspaces]`;
    /// `None` takes it away.
    pub fn set_icon(index: usize, glyph: Option<&str>, cx: &mut App) {
        let icons = cx.global::<Self>().looks.icons.clone();
        let icons = with_entry(icons, index, glyph.unwrap_or_default());
        Self::write_lists(false, Vec::new(), true, icons, cx);
    }

    /// Writes `names` and `icons` into `[workspaces]` of the configuration
    /// where `naming` and `iconing` say, and has them taken up at once.
    fn write_lists(
        naming: bool,
        names: Vec<String>,
        iconing: bool,
        icons: Vec<String>,
        cx: &mut App,
    ) {
        if !naming && !iconing {
            return;
        }
        let list = |list: &[String]| {
            (!list.is_empty()).then(|| toml_edit::Value::Array(list.iter().collect()))
        };
        let written = ConfigFile::open().and_then(|mut file| {
            let named = !naming || file.set_in("workspaces", "names", list(&names));
            let iconed = !iconing || file.set_in("workspaces", "icons", list(&icons));
            if !named || !iconed {
                return Err("`workspaces` in the configuration is no table".to_owned());
            }
            file.save()
        });
        match written {
            Ok(()) => {
                let looks = &mut cx.global_mut::<Self>().looks;
                if naming {
                    looks.names = names;
                }
                if iconing {
                    looks.icons = icons;
                }
                cx.defer(Shell::config_changed);
            }
            Err(error) => report!("{error}"),
        }
    }

    /// The windows of every workspace of Tessera's own, the first list for
    /// the first workspace, a sticky window on each; `None` when Tessera
    /// keeps none, so that what offers to move a window to a workspace
    /// offers nothing beside the virtual desktops of Windows, which take no
    /// window of another program. Where every display has its own
    /// workspaces they are those of `display`. What only shows the windows
    /// asks [`Self::windows_by_workspace`], which answers for the desktops
    /// of Windows too.
    pub fn residents(display: Option<DisplayKey>, cx: &App) -> Option<Vec<Vec<Resident>>> {
        let this = cx.global::<Self>();
        if !this.owned() {
            return None;
        }
        let resident = |id: AppWindowId| Resident {
            id,
            program: this.programs.get(&id).cloned(),
        };
        let residents = match this.peek(display) {
            Some(homes) => homes.residents(),
            None => vec![Vec::new(); this.homes.count()],
        };
        let mut sticky: Vec<AppWindowId> = { this.sticky.iter().copied() }
            .filter(|id| id.exists())
            .collect();
        sticky.sort_by_key(|id| id.raw());
        let residents = residents.into_iter();
        Some(
            residents
                .map(|mut windows| {
                    windows.extend(&sticky);
                    windows.into_iter().map(resident).collect()
                })
                .collect(),
        )
    }

    /// The windows of every workspace, the first list for the first
    /// workspace: those of [`Self::residents`] for the workspaces of
    /// Tessera's own, and for the virtual desktops of Windows the windows
    /// as they were last looked at. They are looked at while something
    /// asks, and afterwards every few seconds and when a window changes;
    /// the first time, and where Windows does not tell, the lists are
    /// empty. Moving a window between the desktops of Windows is not
    /// possible ([`Self::own`] says whether it is).
    #[allow(
        dead_code,
        reason = "for the task view, the overview, the switcher and the dock, which list the windows of every desktop"
    )]
    pub fn windows_by_workspace(display: Option<DisplayKey>, cx: &App) -> Vec<Vec<Resident>> {
        let this = cx.global::<Self>();
        if this.owned() {
            return Self::residents(display, cx).unwrap_or_default();
        }
        this.survey.want_windows();
        match &this.survey.found {
            Some(found) => survey_rows(found),
            None => vec![Vec::new(); VirtualDesktops::now().count],
        }
    }

    /// Goes to the workspace of the window `id` and brings it forward. A
    /// window on another virtual desktop of Windows is brought forward,
    /// and Windows goes to its desktop.
    pub fn show_window(id: AppWindowId, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if !this.owned() || this.sticky.contains(&id) {
            id.activate();
            return;
        }
        this.look();
        let homes = this.of_window(id);
        let Some(home) = homes.bring(id) else {
            return;
        };
        if home == homes.current {
            id.activate();
            return;
        }
        let change = homes.go(home, Instant::now());
        this.apply(change);
    }

    /// Has the window `id` live on the workspace at `index`, without going
    /// there; a sticky window is no longer sticky. The virtual desktops of
    /// Windows take no window of another program, so there it does
    /// nothing.
    pub fn move_window(id: AppWindowId, index: usize, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if !this.owned() {
            return;
        }
        if this.sticky.remove(&id) {
            this.save_soon();
        }
        this.look();
        let change = this.of_window(id).settle(id, index, Instant::now());
        this.apply(change);
    }

    /// Whether the window `id` is seen on every workspace.
    #[allow(
        dead_code,
        reason = "for the window menu and the rules, which make windows sticky"
    )]
    pub fn is_sticky(id: AppWindowId, cx: &App) -> bool {
        cx.global::<Self>().sticky.contains(&id)
    }

    /// Has the window `id` be seen on every workspace from now on, or
    /// live on the workspace shown again where it was. The virtual
    /// desktops of Windows show no window of another program on every
    /// desktop at a program's asking, so there it does nothing.
    pub fn toggle_sticky(id: AppWindowId, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if !this.owned() {
            report!(
                "Windows shows a window on all of its desktops only when asked in its task view"
            );
            return;
        }
        this.save_soon();
        if this.sticky.remove(&id) {
            // Seen again, it lives on the workspace shown.
            this.look();
            return;
        }
        this.sticky.insert(id);
        let mut hidden = false;
        for homes in this.rows_mut() {
            hidden |= homes.evict(id);
        }
        if hidden {
            this.apply(Change {
                show: vec![id],
                ..Change::default()
            });
        }
    }

    /// Takes in what windows there are.
    fn look(&mut self) {
        let windows: Vec<AppWindow> = AppWindow::enumerate()
            .into_iter()
            .filter(|window| {
                listed(
                    &self.config.only,
                    &self.config.except,
                    window.executable.as_deref(),
                )
            })
            .collect();
        self.programs.retain(|&id, _| id.exists());
        self.sticky.retain(|&id| id.exists());
        for window in &windows {
            if let Some(program) = window.program() {
                self.programs.insert(window.id, program);
            }
        }
        // A window that is shown has no still picture to give.
        let shown: HashSet<AppWindowId> = windows.iter().map(|window| window.id).collect();
        self.stills.retain(|id| !shown.contains(&id) && id.exists());
        let windows: Vec<&AppWindow> = { windows.iter() }
            .filter(|window| !self.sticky.contains(&window.id))
            .collect();
        let (front, now) = (AppWindowId::foreground(), Instant::now());
        let mut changed = false;
        for homes in self.rows_mut() {
            changed |= homes.forget(AppWindowId::exists);
        }
        if !self.separate() {
            let seen: Vec<AppWindowId> = windows.iter().map(|window| window.id).collect();
            changed |= self.homes.see(&seen, front, now);
        } else {
            // A window belongs to the display it is seen on. One that was
            // just asked to hide may pass over another display on its way
            // and stays with its own.
            let mut seen: Vec<(DisplayKey, Vec<AppWindowId>)> = Vec::new();
            for window in &windows {
                let id = window.id;
                let mut display = window.display;
                let holds = |(_, homes): &(DisplayKey, Homes)| homes.home.contains_key(&id);
                if let Some(owner) = self.local.iter().position(holds) {
                    let (key, homes) = &mut self.local[owner];
                    if *key != display && homes.hiding(id, now) {
                        display = *key;
                    } else if *key != display {
                        changed |= homes.evict(id);
                    }
                }
                match seen.iter_mut().find(|(key, _)| *key == display) {
                    Some((_, ids)) => ids.push(id),
                    None => seen.push((display, vec![id])),
                }
            }
            for (display, ids) in seen {
                changed |= self.at(Some(display)).see(&ids, front, now);
            }
        }
        if changed {
            self.write_record();
        }
        let mut moved = false;
        for homes in self.rows_mut() {
            moved |= std::mem::take(&mut homes.moved);
        }
        if moved {
            self.save_soon();
        }
    }

    /// Every window that is hidden.
    fn hidden(&self) -> Vec<AppWindowId> {
        let rows = self.local.iter().map(|(_, homes)| homes);
        let rows = rows.chain([&self.homes]);
        rows.flat_map(|homes| homes.hidden.keys().copied())
            .collect()
    }

    /// Hides and shows windows as `change` says. What is hidden is
    /// written down first, with the windows that are shown, which come
    /// off the list a moment later.
    fn apply(&mut self, change: Change) {
        self.write_record_with(&change.show);
        if !change.show.is_empty() {
            self.record_due = Some(Instant::now() + RECORD_AGAIN);
            self.stills.retain(|id| !change.show.contains(&id));
        }
        for homes in self.rows_mut() {
            homes.moved = false;
        }
        self.save_soon();
        Passage::leave(change.hide, !change.onward);
        Passage::enter(change.show, change.front, change.onward);
    }

    /// Shows every window that is hidden.
    fn show_all(&mut self) {
        for id in self.hidden() {
            id.unhide();
        }
        for homes in self.rows_mut() {
            homes.hidden.clear();
        }
        self.write_record();
    }

    fn record() -> Option<PathBuf> {
        Some(config_dir()?.join(RECORD))
    }

    /// Writes down which windows are hidden: the window and its process,
    /// line by line. No file is kept while none is.
    fn write_record(&self) {
        self.write_record_with(&[]);
    }

    /// Writes down which windows are hidden, and `showing` as well.
    fn write_record_with(&self, showing: &[AppWindowId]) {
        let Some(path) = Self::record() else {
            return;
        };
        let mut hidden = self.hidden();
        for &id in showing {
            if !hidden.contains(&id) {
                hidden.push(id);
            }
        }
        if hidden.is_empty() {
            let _ = fs::remove_file(&path);
            return;
        }
        let lines: String = hidden
            .iter()
            .map(|id| format!("{} {}\n", id.raw(), id.process()))
            .collect();
        if let Err(error) = fs::write(&path, lines) {
            report!(
                "{} was not written, so windows hidden now stay hidden if Tessera is terminated: {error}",
                path.display()
            );
        }
    }

    /// Shows the windows that are written down as hidden, if they are
    /// still the same windows and still hidden.
    pub(crate) fn recover() {
        Self::recover_except(&HashSet::new());
    }

    /// The same, but for the windows numbered `staying`, which stay
    /// hidden and written down.
    fn recover_except(staying: &HashSet<isize>) {
        let Some(path) = Self::record() else {
            return;
        };
        let Ok(lines) = fs::read_to_string(&path) else {
            return;
        };
        let mut kept = String::new();
        for (raw, process) in lines.lines().filter_map(parse) {
            let id = AppWindowId::from_raw(raw);
            if !id.exists() || id.process() != process || id.is_shown() {
                continue;
            }
            match staying.contains(&raw) {
                true => kept.push_str(&format!("{raw} {process}\n")),
                false => id.unhide(),
            }
        }
        let _ = match kept.is_empty() {
            true => fs::remove_file(&path),
            false => fs::write(&path, kept),
        };
    }
}

/// The window and the process a line of the record names.
fn parse(line: &str) -> Option<(isize, u32)> {
    let (window, process) = line.split_once(' ')?;
    Some((window.parse().ok()?, process.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: isize) -> AppWindowId {
        AppWindowId::from_raw(raw)
    }

    #[test]
    fn a_workspace_without_a_name_says_its_number() {
        let names = ["web".to_owned(), " ".to_owned()];
        assert_eq!(label(&names, 0), "web");
        assert_eq!(label(&names, 1), "2");
        assert_eq!(label(&names, 2), "3");
    }

    #[test]
    fn names_and_looks_change_nothing_about_the_windows() {
        let named: WorkspacesConfig =
            toml::from_str("own = 3\nnames = ['a']\nicons = ['\u{e774}']\nrows = 2\nwrap = true\nmin = 2\nmax = 5\nadd_button = true")
                .unwrap();
        let plain: WorkspacesConfig = toml::from_str("own = 3").unwrap();
        assert_ne!(named, plain);
        assert_eq!(named.placing(), plain);
    }

    #[test]
    fn keys_go_to_each_workspace_and_send_there_with_shift() {
        let config = |keys: &str| WorkspacesConfig {
            keys: Some(keys.to_owned()),
            ..WorkspacesConfig::default()
        };
        let keys = config("alt").hotkeys(2);
        let seen: Vec<_> = keys
            .iter()
            .map(|key| (key.id, key.key, key.modifiers.alt, key.modifiers.shift))
            .collect();
        assert_eq!(
            seen,
            [
                (0, b'1', true, false),
                (SENDING, b'1', true, true),
                (1, b'2', true, false),
                (SENDING + 1, b'2', true, true),
            ]
        );
        assert_eq!(config("alt+shift").hotkeys(3).len(), 3);
        assert_eq!(config("alt+shift").hotkeys(14).len(), DIGITS);
        assert!(config("alt").hotkeys(0).is_empty());
        assert!(config("alt+2").hotkeys(3).is_empty());
        assert!(WorkspacesConfig::default().hotkeys(3).is_empty());
    }

    #[test]
    fn the_count_of_workspaces_is_checked() {
        let config = |text: &str| toml::from_str::<WorkspacesConfig>(text).unwrap();
        assert_eq!(config("").count(), None);
        assert_eq!(config("own = 4").count(), Some(4));
        assert_eq!(config("own = 1").count(), Some(1));
        assert_eq!(config("own = 16").count(), Some(16));
        assert_eq!(config("own = 0").count(), None);
        assert_eq!(config("own = 17").count(), None);
        // The count at first is within the limits.
        assert_eq!(config("own = 2\nmin = 3").count(), Some(3));
        assert_eq!(config("own = 8\nmax = 5").count(), Some(5));
        // Limits that are not usable are the widest.
        assert_eq!(config("min = 4\nmax = 2").limits(), (FEWEST, MOST));
        assert_eq!(config("max = 40").limits(), (FEWEST, MOST));
        assert_eq!(config("min = 2").limits(), (2, MOST));
    }

    #[test]
    fn a_line_of_the_record_names_a_window_and_its_process() {
        assert_eq!(parse("1234 56"), Some((1234, 56)));
        assert_eq!(parse("1234"), None);
        assert_eq!(parse("window 56"), None);
    }

    #[test]
    fn a_list_of_the_configuration_gets_an_entry_and_loses_one() {
        let list = |items: &[&str]| {
            items
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(with_entry(Vec::new(), 2, " web "), list(&["", "", "web"]));
        assert_eq!(with_entry(list(&["a", "b"]), 1, ""), list(&["a"]));
        assert_eq!(with_entry(list(&["a"]), 3, ""), list(&["a"]));
        assert_eq!(without_entry(list(&["a", "b", "c"]), 1), list(&["a", "c"]));
        assert_eq!(without_entry(list(&["a", "", "c"]), 2), list(&["a"]));
        assert_eq!(without_entry(list(&["a"]), 4), list(&["a"]));
    }

    #[test]
    fn an_action_names_a_direction_a_number_or_a_change() {
        assert_eq!(Goal::parse("next", false), Some(Goal::Toward(Toward::Next)));
        assert_eq!(Goal::parse("3", true), Some(Goal::Number(3)));
        assert_eq!(Goal::parse("0", false), None);
        assert_eq!(Goal::parse("17", false), None);
        assert_eq!(Goal::parse("add", false), Some(Goal::Add));
        assert_eq!(Goal::parse("add", true), None);
        assert_eq!(Goal::parse("Rename", false), Some(Goal::Rename));
        assert!(Goal::check(None, false).is_err());
        assert!(Goal::check(Some("down"), true).is_ok());
        assert!(Goal::check(Some("remove"), true).is_err());
    }

    #[test]
    fn the_windows_of_the_desktops_of_windows_are_listed_desktop_by_desktop() {
        let window = |raw, desktop| crate::platform::DesktopWindow {
            id: id(raw),
            desktop,
            program: None,
        };
        let survey = DesktopSurvey {
            count: 3,
            current: 0,
            ids: vec![5, 6, 7],
            names: vec![None, Some("work".to_owned()), None],
            windows: vec![window(9, 2), window(4, 0), window(2, 2), window(8, 5)],
        };
        let rows: Vec<Vec<isize>> = { survey_rows(&survey).into_iter() }
            .map(|row| row.into_iter().map(|resident| resident.id.raw()).collect())
            .collect();
        assert_eq!(rows, [vec![4], vec![], vec![2, 9]]);
    }
}
