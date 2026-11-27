// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Tiling: application windows arranged side by side, so that they fill
//! what the surfaces of the shell leave of each display.
//!
//! Only windows that can be resized by their border take part, and of
//! those only the ones that are neither minimized nor maximized. Each
//! workspace of each display has tiles of its own: a layout, which is a
//! tree of rooms divided among windows (see [`tree`]), and the sizes the
//! user gave them, which are kept over restarts. A window that is dragged
//! onto the tile of another trades places with it, or takes its place in
//! the order, and so does the window in front when a key combination
//! sends it toward a side; another combination brings the window on that
//! side in front. When tiling is switched off, and when the shell quits,
//! every window goes back to where it was before it was first tiled.
//!
//! The tiled window in front can be told apart by its border, which
//! Windows draws in a color of the theme while it is asked to, or which
//! Tessera draws itself.

mod command;
mod config;
mod declarative;
mod displays;
mod layouts;
mod state;
mod tiles;
mod tree;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{App, AsyncApp, Global, Hsla, SharedString};

pub use command::{TilingCommand, parse_command};
pub use config::TilingConfig;
pub use layouts::TileLayout;

use layouts::LayoutChoice;

use command::words;
use config::{DragMode, Knobs, Marking, argb};
use declarative::Library;
use displays::DisplayName;
use state::{Kept, PlaceKey};
use tiles::{Layout, Tiles, same_size};
use tree::Axis;

use crate::actions::{Action, Actions};
use crate::config::config_dir;
use crate::i18n::Language;
use crate::keys::Shortcuts;
use crate::motion::{Spring, SpringSpec, mix};
use crate::platform::{
    AppWindow, AppWindowId, Display, DisplayKey, MoveEvent, MoveWatch, ScreenRect, TileBorder,
    WindowEvents, pointer_held,
};
use crate::theme::Theme;
use crate::workspaces::Workspaces;

/// How often it is checked whether something happened to the windows.
const CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// How often the windows are looked at even though nothing was heard of
/// them: a window that stops being maximized, for one, goes unannounced.
const LOOK_INTERVAL: Duration = Duration::from_secs(2);

/// The time between two steps of windows that are moving.
const FRAME: Duration = Duration::from_millis(8);

/// How often the directory of layout files is looked at while windows
/// are tiled.
const LAYOUTS_LOOK: Duration = Duration::from_secs(2);

/// How long after the last change what is kept is written.
const WRITE_AFTER: Duration = Duration::from_secs(2);

/// How long a window may be held by the user before it is taken as let
/// go, in case Windows did not tell.
const LONGEST_HOLD: Duration = Duration::from_secs(60);

/// The directory of the configuration that holds the layout files.
const LAYOUTS_DIRECTORY: &str = "tiling-layouts";

/// The keys the action `tiling` has by default where the keys of Windows
/// are in use, while tiling is on: where the next window opens, and every
/// tile alike again.
const DEFAULT_KEYS: [(&str, &str); 5] = [
    ("win+shift+i", "reserve up"),
    ("win+shift+k", "reserve down"),
    ("win+shift+j", "reserve left"),
    ("win+shift+l", "reserve right"),
    ("win+alt+0", "reset"),
];

fn center(rect: ScreenRect) -> (i32, i32) {
    ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2)
}

fn holds(rect: ScreenRect, (x, y): (i32, i32)) -> bool {
    (rect.left..rect.right).contains(&x) && (rect.top..rect.bottom).contains(&y)
}

/// A side of a window, as a key combination names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toward {
    Left,
    Right,
    Up,
    Down,
}

impl Toward {
    /// The side `name` is: `left`, `right`, `up` or `down`.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }

    /// Of `others`, the one that lies on this side of `from`: the nearest
    /// one, where a step aside counts twice as far as one ahead.
    fn of<T: Copy>(self, from: ScreenRect, others: &[(T, ScreenRect)]) -> Option<T> {
        let (x, y) = center(from);
        let far = |&(_, other): &(T, ScreenRect)| {
            let (other_x, other_y) = center(other);
            let (ahead, aside) = match self {
                Self::Left => (x - other_x, other_y - y),
                Self::Right => (other_x - x, other_y - y),
                Self::Up => (y - other_y, other_x - x),
                Self::Down => (other_y - y, other_x - x),
            };
            (ahead > 0).then(|| ahead + 2 * aside.abs())
        };
        let nearest = others
            .iter()
            .filter_map(|other| Some((far(other)?, other.0)))
            .min_by_key(|&(far, _)| far)?;
        Some(nearest.1)
    }
}

/// The rectangle `amount` of the way from `from` to `to`; an amount above
/// 1 lies beyond `to`.
fn between(from: ScreenRect, to: ScreenRect, amount: f32) -> ScreenRect {
    let mix = |from: i32, to: i32| from + ((to - from) as f32 * amount).round() as i32;
    let (left, top) = (mix(from.left, to.left), mix(from.top, to.top));
    ScreenRect {
        left,
        top,
        // A window keeps a size it can be drawn at.
        right: mix(from.right, to.right).max(left + 1),
        bottom: mix(from.bottom, to.bottom).max(top + 1),
    }
}

/// The place after `at` among `count` in a ring, or before it with
/// `back`.
fn around(at: usize, count: usize, back: bool) -> usize {
    match back {
        true => (at + count - 1) % count,
        false => (at + 1) % count,
    }
}

/// Whether the program at `executable` is one of `only`, or `only` is
/// empty, and it is none of `except`; both name programs by the file name
/// of the executable. A program that Windows does not let be read is not.
pub fn listed(only: &[String], except: &[String], executable: Option<&Path>) -> bool {
    let Some(name) = executable.and_then(Path::file_name) else {
        return false;
    };
    let name = name.to_string_lossy();
    let among = |names: &[String]| names.iter().any(|one| one.eq_ignore_ascii_case(&name));
    (only.is_empty() || among(only)) && !among(except)
}

/// Whether `title`, or another name of a window, has one of `pieces` in
/// it, whatever the case of the letters. A piece without letters is in no
/// title.
fn titled(pieces: &[String], title: &str) -> bool {
    let title = title.to_lowercase();
    { pieces.iter() }
        .filter(|piece| !piece.trim().is_empty())
        .any(|piece| title.contains(&piece.to_lowercase()))
}

/// What a workspace of a display is kept under. Workspaces are counted
/// by their place for now, so a workspace that is taken away hands what
/// it kept to the one that comes into its place.
pub fn workspace_slot(_display: DisplayKey, index: usize, _cx: &App) -> String {
    index.to_string()
}

/// Whether windows are tiled right now, and the layout of the display in
/// front. It starts as the configuration says and changes with the
/// `tiling` widget and commands, until the configuration is read again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TilingMode {
    pub enabled: bool,
    pub layout: LayoutChoice,
}

impl Global for TilingMode {}

impl TilingMode {
    /// What follows when the modes are gone through one by one: every
    /// layout of `choices` in turn, then no tiling, then the first again.
    pub fn next(self, choices: &[LayoutChoice]) -> Self {
        let first = choices.first().cloned().unwrap_or_default();
        if !self.enabled {
            return Self {
                enabled: true,
                layout: first,
            };
        }
        let place = choices.iter().position(|choice| *choice == self.layout);
        match place.and_then(|place| choices.get(place + 1)) {
            Some(layout) => Self {
                enabled: true,
                layout: layout.clone(),
            },
            None => Self {
                enabled: false,
                layout: first,
            },
        }
    }
}

/// A window on its way to a place.
struct Move {
    from: ScreenRect,
    to: ScreenRect,
    /// From 0 at `from` to 1 at `to`.
    progress: Spring,
}

/// The window whose border is colored.
struct Marked {
    id: AppWindowId,
    /// From 0, the color borders have otherwise, to 1, the color that
    /// tells the window apart.
    shown: Spring,
    /// The color Windows was last asked for.
    painted: Option<u32>,
}

/// `color` as Windows takes the color of a border: 0x00BBGGRR.
fn border_color(color: Hsla) -> u32 {
    let color = color.to_rgb();
    let part = |part: f32| (part.clamp(0., 1.) * 255.).round() as u32;
    part(color.b) << 16 | part(color.g) << 8 | part(color.r)
}

/// A tile a window was sent to, and what was seen of the window once it
/// was looked at again without having got there: an application may keep
/// a size of its own, and is not asked again until something changes.
type Sent = (ScreenRect, Option<ScreenRect>);

/// A display as it was looked at, with the workspace it shows.
struct Seen {
    key: DisplayKey,
    name: DisplayName,
    /// What the bars leave of it.
    work: ScreenRect,
    /// The workspace it shows, counted from 0.
    workspace: usize,
    place: PlaceKey,
}

/// The displays, and the window in front.
struct Look {
    displays: Vec<Seen>,
    front: Option<AppWindowId>,
}

impl Look {
    fn now(cx: &App) -> Self {
        let displays = Display::all();
        let frames: Vec<ScreenRect> = displays.iter().map(|display| display.frame).collect();
        let names = displays::names(&frames);
        let workspaces = cx.try_global::<Workspaces>().is_some();
        let displays = { displays.into_iter().zip(names) }
            .map(|(display, name)| {
                let workspace = match workspaces {
                    true => Workspaces::now(Some(display.key), cx).current,
                    false => 0,
                };
                let place = PlaceKey {
                    display: name.key(),
                    workspace: workspace_slot(display.key, workspace, cx),
                };
                Seen {
                    key: display.key,
                    name,
                    work: display.work,
                    workspace,
                    place,
                }
            })
            .collect();
        Self {
            displays,
            front: AppWindowId::foreground(),
        }
    }

    fn of(&self, display: DisplayKey) -> Option<&Seen> {
        self.displays.iter().find(|seen| seen.key == display)
    }
}

/// A place as it was last arranged.
struct Shown {
    display: DisplayKey,
    place: PlaceKey,
    layout: LayoutChoice,
    area: ScreenRect,
    gap: i32,
}

/// Keeps the windows in their tiles.
pub struct Tiler {
    config: TilingConfig,
    knobs: Knobs,
    library: Library,
    /// The tiles of each workspace of each display that was seen.
    places: HashMap<PlaceKey, Tiles>,
    /// What was kept of the places not seen yet.
    kept: BTreeMap<PlaceKey, Kept>,
    /// The places of the displays at the last arrangement.
    shown: Vec<Shown>,
    /// The windows that are tiled.
    managed: HashSet<AppWindowId>,
    moves: HashMap<AppWindowId, Move>,
    /// Where each window was before it was first tiled.
    before: HashMap<AppWindowId, ScreenRect>,
    sent: HashMap<AppWindowId, Sent>,
    /// The windows taken out of the tiles, which stay where they are put.
    floating: HashSet<AppWindowId>,
    marked: Option<Marked>,
    /// The border Tessera draws, around which window and where.
    drawn: Option<(AppWindowId, ScreenRect, i32, i32, u32)>,
    /// Whether the order was changed by a key combination, so that the
    /// window in front lies on a tile that is no longer its own without
    /// having been dropped there.
    carried: bool,
    /// Counts the tasks that were started to keep the tiles; only the
    /// latest goes on.
    task: u64,
    /// What the window rules say: `true` for a window that is tiled
    /// whatever the configuration of tiling says of it, `false` for one
    /// that is left alone.
    ruled: HashMap<AppWindowId, bool>,
    /// The windows the rules have Windows draw no border around, which
    /// the tiling neither colors nor gives their border back.
    borderless: HashSet<AppWindowId>,
    /// The windows the user moves or resizes now, since when.
    held: HashMap<AppWindowId, Instant>,
    /// The tiled windows the user let go of since the last arrangement.
    ended: Vec<AppWindowId>,
    /// Windows that were dropped on another display and belong to a place
    /// there, until they have been sent to it.
    claimed: HashMap<AppWindowId, DisplayKey>,
    /// Whether the windows are to be arranged at the next check.
    nudged: bool,
    /// What is kept, as the file writes it, when it last changed, and as
    /// it was last written.
    pending: String,
    changed: Option<Instant>,
    written: String,
    /// What the layout files were when they were last read, when they
    /// were last looked at, and whether they are being read.
    layouts_stamp: Option<u64>,
    layouts_looked: Option<Instant>,
    scanning: bool,
}

impl Global for Tiler {}

impl Tiler {
    /// Starts tiling as `config` says, with what was kept of the tiles.
    /// The layout files and the kept tiles are read here, once, as the
    /// configuration itself is; later changes of the files are read away
    /// from the user interface.
    pub fn start(config: &TilingConfig, cx: &mut App) {
        let directory = layouts_directory();
        let scan = directory.as_deref().map(declarative::scan);
        let library = Library::with(
            scan.as_ref()
                .map(|scan| scan.layouts.clone())
                .unwrap_or_default(),
        );
        for problem in scan.iter().flat_map(|scan| &scan.problems) {
            report!("{problem}");
        }
        let kept = state::read();
        let mut this = Self {
            config: config.clone(),
            knobs: config.knobs(),
            library,
            places: HashMap::new(),
            kept,
            shown: Vec::new(),
            managed: HashSet::new(),
            moves: HashMap::new(),
            before: HashMap::new(),
            sent: HashMap::new(),
            floating: HashSet::new(),
            marked: None,
            drawn: None,
            carried: false,
            task: 0,
            ruled: HashMap::new(),
            borderless: HashSet::new(),
            held: HashMap::new(),
            ended: Vec::new(),
            claimed: HashMap::new(),
            nudged: false,
            pending: String::new(),
            changed: None,
            written: String::new(),
            layouts_stamp: scan.map(|scan| scan.stamp),
            layouts_looked: Some(Instant::now()),
            scanning: false,
        };
        this.pending = this.kept_text();
        this.written = this.pending.clone();
        config.check();
        this.check_layouts();
        cx.set_global(this);
        cx.set_global(TilingMode {
            enabled: false,
            layout: config.layout.clone(),
        });
        cx.on_app_quit(|cx| {
            let this = cx.global_mut::<Self>();
            this.put_back_at_once();
            let text = this.kept_text();
            if text != this.written {
                state::write(&text);
            }
            async {}
        })
        .detach();
        Self::add_action(cx);
        Self::watch_moves(cx);
        Self::set_enabled(config.enabled, cx);
    }

    /// Takes over a configuration that was read again, if it says
    /// something else about tiling than the one before. Where it says
    /// something else of the layouts, the layouts chosen while Tessera
    /// ran give way to it.
    pub fn configure(config: &TilingConfig, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        // The layout files are read again with the configuration, so that
        // keys may name a file that was just made.
        if let Some(directory) = layouts_directory() {
            let scan = declarative::scan(&directory);
            this.take_scan(scan);
        }
        if this.config == *config {
            return;
        }
        if this.config.layouts_differ(config) {
            for tiles in this.places.values_mut() {
                tiles.chosen = None;
            }
            for kept in this.kept.values_mut() {
                kept.chosen = None;
            }
        }
        if this.knobs.marking != config.knobs().marking {
            this.unmark();
        }
        this.config = config.clone();
        this.knobs = config.knobs();
        config.check();
        this.check_layouts();
        this.nudged = true;
        let running = cx.global::<TilingMode>().enabled;
        if running == config.enabled {
            Self::run(cx);
        } else {
            Self::set_enabled(config.enabled, cx);
        }
    }

    /// Switches tiling on or off.
    fn set_enabled(enabled: bool, cx: &mut App) {
        let mut mode = cx.global::<TilingMode>().clone();
        mode.enabled = enabled;
        let this = cx.global_mut::<Self>();
        if !enabled {
            this.put_back();
        } else if !WindowEvents::watch() {
            report!("Windows does not report window changes, so tiles follow them late");
        }
        this.nudged = true;
        cx.set_global(mode);
        // The keys of tiling by default are left to other programs while
        // tiling is off, when they would do nothing.
        if cx.default_global::<Actions>().rest("tiling", !enabled) {
            Shortcuts::again(cx);
        }
        Self::run(cx);
    }

    /// Registers the action `tiling`, which takes what a `tiling` key
    /// takes, with its keys by default.
    fn add_action(cx: &mut App) {
        let mut action = Action::new(|arg, cx| match parse_command(&words(arg.unwrap_or(""))) {
            Ok(command) => Self::command(command, cx),
            Err(why) => report!("the action `tiling` does nothing: {why}"),
        })
        .checking(|arg| parse_command(&words(arg.unwrap_or(""))).map(drop));
        for (press, arg) in DEFAULT_KEYS {
            action = action.by_default(press, Some(arg));
        }
        Actions::add("tiling", action, cx);
    }

    /// Hears of windows the user begins and ends moving or resizing: one
    /// held is not moved, and one let go is looked at once.
    fn watch_moves(cx: &mut App) {
        let events = MoveWatch::subscribe();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(event) = events.recv().await {
                let heard = cx.update(|cx| {
                    let this = cx.global_mut::<Self>();
                    match event {
                        MoveEvent::Start(id) if this.managed.contains(&id) => {
                            this.held.insert(id, Instant::now());
                        }
                        MoveEvent::Start(_) => {}
                        MoveEvent::End(id) => {
                            this.held.remove(&id);
                            if this.managed.contains(&id) {
                                this.ended.push(id);
                                this.nudged = true;
                            }
                        }
                    }
                });
                if heard.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Starts the task that keeps the tiles, in place of the one before.
    fn run(cx: &mut App) {
        let this = cx.global_mut::<Self>();
        this.task += 1;
        let task = this.task;
        let spring = cx.global::<Theme>().motion.spatial;
        cx.spawn(async move |cx: &mut AsyncApp| {
            let mut seen = None;
            let mut looked = Instant::now();
            loop {
                let ticked = cx.update(|cx| Self::tick(task, &mut seen, &mut looked, spring, cx));
                let Ok(Some(tick)) = ticked else {
                    break;
                };
                if let Some(text) = tick.write {
                    cx.background_executor()
                        .spawn(async move { state::write(&text) })
                        .detach();
                }
                if let (Some(known), Some(directory)) = (tick.scan, layouts_directory()) {
                    let scanned = cx
                        .background_executor()
                        .spawn(async move {
                            let stamp = declarative::stamp(&directory);
                            (Some(stamp) != known).then(|| declarative::scan(&directory))
                        })
                        .await;
                    let taken = cx.update(|cx| {
                        let this = cx.global_mut::<Self>();
                        this.scanning = false;
                        if let Some(scan) = scanned {
                            this.take_scan(scan);
                            this.check_layouts();
                        }
                    });
                    if taken.is_err() {
                        break;
                    }
                }
                cx.background_executor().timer(tick.pause).await;
            }
        })
        .detach();
    }

    /// One look at the windows: arranges them when something happened,
    /// moves those on their way a step and colors the border of the one
    /// in front. `None` once the task is to end.
    fn tick(
        task: u64,
        seen: &mut Option<u64>,
        looked: &mut Instant,
        spring: SpringSpec,
        cx: &mut App,
    ) -> Option<Tick> {
        let mode = cx.global::<TilingMode>().clone();
        let theme = *cx.global::<Theme>();
        let fade = (theme.colors.outline, theme.colors.primary);
        let this = cx.global::<Self>();
        if this.task != task {
            return None;
        }
        let now = WindowEvents::generation();
        let due = *seen != Some(now) || looked.elapsed() >= LOOK_INTERVAL || this.nudged;
        // A window that is being dragged is left to the hand.
        let held = this
            .held
            .values()
            .any(|since| since.elapsed() < LONGEST_HOLD);
        let arranging = mode.enabled && due && !pointer_held() && !held;
        let look = arranging.then(|| Look::now(cx));
        let this = cx.global_mut::<Self>();
        let mut front_layout = None;
        if let Some(look) = &look {
            (*seen, *looked) = (Some(now), Instant::now());
            this.nudged = false;
            this.held.clear();
            this.arrange(look, spring);
            front_layout = this.front_layout(look);
        }
        this.step(Instant::now());
        let fading = this.mark(mode.enabled, fade, theme.motion.effects);
        let write = this.due_write();
        let scan = (mode.enabled && this.layouts_due()).then_some(this.layouts_stamp);
        let moving = fading || !this.moves.is_empty();
        let waiting = this.changed.is_some();
        if let Some(layout) = front_layout.filter(|layout| *layout != mode.layout) {
            cx.set_global(TilingMode {
                enabled: mode.enabled,
                layout,
            });
        }
        if !(mode.enabled || moving || waiting || write.is_some()) {
            return None;
        }
        let pause = if moving { FRAME } else { CHECK_INTERVAL };
        Some(Tick { pause, write, scan })
    }

    /// Takes over what the window rules say of tiling windows, by window,
    /// and which windows they have drawn without a border.
    pub fn rule(ruled: HashMap<AppWindowId, bool>, borderless: HashSet<AppWindowId>, cx: &mut App) {
        if let Some(this) = cx.try_global::<Self>()
            && (this.ruled != ruled || this.borderless != borderless)
        {
            let this = cx.global_mut::<Self>();
            this.ruled = ruled;
            this.borderless = borderless;
        }
    }

    /// Whether `window` is one to tile, as the rules say or else the
    /// configuration of tiling.
    fn takes(&self, window: &AppWindow) -> bool {
        if let Some(&tiled) = self.ruled.get(&window.id) {
            return tiled;
        }
        let (titles, classes) = self.config.exceptions();
        self.config.covers(window.executable.as_deref())
            && (titles.is_empty() || !titled(titles, &window.id.title()))
            && (classes.is_empty() || !titled(classes, &window.id.class()))
    }

    /// Whether the window `id` is tiled now, so that it is left to the
    /// tiling.
    pub fn manages(id: AppWindowId, cx: &App) -> bool {
        let enabled = cx
            .try_global::<TilingMode>()
            .is_some_and(|mode| mode.enabled);
        enabled
            && cx
                .try_global::<Self>()
                .is_some_and(|this| this.managed.contains(&id))
    }

    /// Whether `window` is one the tiling places while it is on, or will
    /// as soon as it can.
    pub fn tiles(window: &AppWindow, cx: &App) -> bool {
        let enabled = cx
            .try_global::<TilingMode>()
            .is_some_and(|mode| mode.enabled);
        let taken = cx.try_global::<Self>().is_some_and(|this| {
            this.managed.contains(&window.id)
                || (this.takes(window) && !this.floating.contains(&window.id))
        });
        enabled && taken
    }

    /// Does what `command` asks.
    pub fn command(command: TilingCommand, cx: &mut App) {
        let enabled = cx.global::<TilingMode>().enabled;
        match command {
            TilingCommand::Next => Self::next_on(None, cx),
            TilingCommand::Off => Self::set_enabled(false, cx),
            TilingCommand::Layout(choice) => {
                Self::choose(None, choice, cx);
                if !enabled {
                    Self::set_enabled(true, cx);
                }
            }
            TilingCommand::Float if enabled => Self::float(cx),
            _ if !enabled => {}
            TilingCommand::Float => {}
            TilingCommand::Wider(wider) => {
                Self::change(cx, |tiles, layout, front, knobs, _| {
                    let step = if wider { knobs.step } else { -knobs.step };
                    tiles.wider(layout, front, step);
                });
            }
            TilingCommand::Width(more) | TilingCommand::Height(more) => {
                let axis = match command {
                    TilingCommand::Width(_) => Axis::Horizontal,
                    _ => Axis::Vertical,
                };
                Self::change(cx, |tiles, _, front, knobs, _| {
                    let step = if more { knobs.step } else { -knobs.step };
                    if let Some(front) = front {
                        tiles.grow(front, axis, step);
                    }
                });
            }
            TilingCommand::Reset => {
                Self::change(cx, |tiles, layout, _, _, area| tiles.reset(layout, area));
            }
            TilingCommand::Reserve(reserve) => {
                Self::change(cx, |tiles, _, _, _, _| tiles.reserve = Some(reserve));
            }
        }
    }

    /// Changes the tiles of the workspace of the display in front with
    /// `change`, which is given them, their layout, the window in front,
    /// the knobs and the area of the tiles, and arranges the windows
    /// again.
    fn change(
        cx: &mut App,
        change: impl FnOnce(&mut Tiles, Layout, Option<AppWindowId>, &Knobs, ScreenRect),
    ) {
        let look = Look::now(cx);
        let spring = cx.global::<Theme>().motion.spatial;
        let this = cx.global_mut::<Self>();
        let Some(seen) = this
            .front_display(&look)
            .and_then(|display| look.of(display))
        else {
            return;
        };
        let place = this.config.place(&seen.name, seen.workspace);
        let knobs = this.knobs;
        let library = &this.library;
        let tiles = tiles_of(&mut this.places, &mut this.kept, &seen.place);
        let choice = tiles.chosen.clone().unwrap_or(place.layout.clone());
        let layout = layout_of(library, &choice);
        let front = look.front.filter(|&id| tiles.holds(id));
        change(tiles, layout, front, &knobs, place.area(seen.work));
        // The windows are on their way to new tiles, not dropped on them.
        this.carried = true;
        this.arrange(&look, spring);
        this.note();
    }

    /// Has the workspace of `display`, or of the display in front, take
    /// the layout `choice`.
    fn choose(display: Option<DisplayKey>, choice: LayoutChoice, cx: &mut App) {
        if let LayoutChoice::File(name) = &choice
            && cx.global::<Self>().library.get(name).is_none()
        {
            report!("there is no tiling layout named `{name}`");
            return;
        }
        let look = Look::now(cx);
        let this = cx.global_mut::<Self>();
        let display = display.or_else(|| this.front_display(&look));
        let Some(seen) = display.and_then(|display| look.of(display)) else {
            return;
        };
        let configured = this.config.place(&seen.name, seen.workspace).layout;
        let tiles = tiles_of(&mut this.places, &mut this.kept, &seen.place);
        tiles.chosen = (choice != configured).then_some(choice.clone());
        this.nudged = true;
        this.note();
        let enabled = cx.global::<TilingMode>().enabled;
        // The widgets of every display look again.
        cx.set_global(TilingMode {
            enabled,
            layout: choice,
        });
    }

    /// Goes on to the next layout on the workspace of `display`, or of the
    /// display in front; after the last there is no tiling, and after none
    /// the first layout.
    pub fn next_on(display: Option<DisplayKey>, cx: &mut App) {
        let enabled = cx.global::<TilingMode>().enabled;
        let current = Self::layout_on(display, cx);
        let this = cx.global::<Self>();
        let choices = this.choices();
        let mode = TilingMode {
            enabled,
            layout: current,
        };
        let next = mode.next(&choices);
        if next.enabled {
            Self::choose(display, next.layout, cx);
        }
        if next.enabled != enabled {
            Self::set_enabled(next.enabled, cx);
        }
    }

    /// Every layout there is: Tessera's own, then those of files.
    fn choices(&self) -> Vec<LayoutChoice> {
        let builtin = TileLayout::ALL.map(LayoutChoice::Builtin);
        let files = self.library.names().into_iter().map(LayoutChoice::File);
        builtin.into_iter().chain(files).collect()
    }

    /// The layout of the workspace `display` shows, or of the display in
    /// front.
    fn layout_on(display: Option<DisplayKey>, cx: &App) -> LayoutChoice {
        let this = cx.global::<Self>();
        let shown = match display {
            Some(display) => this.shown.iter().find(|shown| shown.display == display),
            None => {
                let front = AppWindowId::foreground().filter(|id| this.managed.contains(id));
                let display = front
                    .and_then(|id| this.shown_of(id))
                    .map(|shown| shown.display)
                    .or_else(DisplayKey::under_pointer);
                display.and_then(|display| this.shown.iter().find(|shown| shown.display == display))
            }
        };
        match shown {
            Some(shown) => shown.layout.clone(),
            None => cx.global::<TilingMode>().layout.clone(),
        }
    }

    /// What the `tiling` widget on `display` says: the layout of the
    /// workspace there, or that windows float.
    pub fn label_on(display: Option<DisplayKey>, cx: &App) -> SharedString {
        let language = Language::of(cx);
        if !cx.global::<TilingMode>().enabled {
            return language.say("Floating").into();
        }
        let choice = Self::layout_on(display, cx);
        cx.global::<Self>().title(&choice, language).into()
    }

    /// What the layout `choice` is called where it is shown.
    fn title(&self, choice: &LayoutChoice, language: Language) -> String {
        match choice {
            LayoutChoice::Builtin(layout) => language.say(layout.name()).to_owned(),
            LayoutChoice::File(name) => {
                let Some(layout) = self.library.get(name) else {
                    return name.clone();
                };
                match (&layout.title, layout.bundled) {
                    (Some(title), true) => declarative::bundled_title(title)
                        .map_or_else(|| title.clone(), |title| language.say(title).to_owned()),
                    (Some(title), false) => title.clone(),
                    (None, _) => name.clone(),
                }
            }
        }
    }

    /// The place the window `id` is tiled in.
    fn shown_of(&self, id: AppWindowId) -> Option<&Shown> {
        self.shown.iter().find(|shown| {
            self.places
                .get(&shown.place)
                .is_some_and(|tiles| tiles.holds(id))
        })
    }

    /// The display in front: that of the tiled window in front, or else
    /// the one the pointer is on.
    fn front_display(&self, look: &Look) -> Option<DisplayKey> {
        let front = look.front.filter(|id| self.managed.contains(id));
        let tiled = front
            .and_then(|id| self.shown_of(id))
            .map(|shown| shown.display);
        tiled.or_else(DisplayKey::under_pointer)
    }

    /// The layout of the workspace of the display in front.
    fn front_layout(&self, look: &Look) -> Option<LayoutChoice> {
        let display = self.front_display(look)?;
        let shown = self.shown.iter().find(|shown| shown.display == display)?;
        Some(shown.layout.clone())
    }

    /// The tiled window in front and the window on the side `toward` of it,
    /// among the windows tiling covers, and whether both are in one
    /// place.
    fn beside(&self, toward: Toward) -> Option<(AppWindowId, AppWindowId, bool)> {
        let front = AppWindowId::foreground()?;
        let windows: Vec<AppWindow> = AppWindow::enumerate()
            .into_iter()
            .filter(|window| self.takes(window))
            .filter(|window| window.id.is_tileable())
            .collect();
        if !windows.iter().any(|window| window.id == front) {
            return None;
        }
        let others: Vec<(AppWindowId, ScreenRect)> = { windows.iter() }
            .filter(|window| window.id != front)
            .filter_map(|window| Some((window.id, window.id.frame()?)))
            .collect();
        let other = toward.of(front.frame()?, &others)?;
        let together =
            { self.places.values() }.any(|tiles| tiles.holds(front) && tiles.holds(other));
        Some((front, other, together))
    }

    /// The tiled window after the one in front in its place, in the order
    /// of the tiles, or the one before it with `back`.
    fn following(&self, back: bool) -> Option<AppWindowId> {
        let front = AppWindowId::foreground()?;
        let shown = self.shown_of(front)?;
        let ids = self.places.get(&shown.place)?.windows();
        let at = ids.iter().position(|&id| id == front)?;
        let other = ids[around(at, ids.len(), back)];
        (other != front).then_some(other)
    }

    /// Brings the window on the side `toward` of the window in front, in
    /// front itself. Where every window takes the whole space there are
    /// no sides: right and down are the next window then, left and up
    /// the one before.
    pub fn focus(toward: Toward, cx: &mut App) {
        let enabled = cx.global::<TilingMode>().enabled;
        let this = cx.global::<Self>();
        let stacked = { AppWindowId::foreground() }
            .and_then(|front| this.shown_of(front))
            .is_some_and(|shown| shown.layout == LayoutChoice::Builtin(TileLayout::Monocle));
        let back = matches!(toward, Toward::Left | Toward::Up);
        let other = (enabled && stacked).then(|| this.following(back)).flatten();
        let other = other.or_else(|| Some(this.beside(toward)?.1));
        if let Some(other) = other {
            other.activate();
        }
    }

    /// Takes the window in front out of the tiles, to where it was before
    /// it was tiled or to the middle of its display at the size
    /// `tiling.float` gives, or puts it among them again.
    pub fn float(cx: &mut App) {
        let enabled = cx.global::<TilingMode>().enabled;
        let look = Look::now(cx);
        let spring = cx.global::<Theme>().motion.spatial;
        let this = cx.global_mut::<Self>();
        let Some(front) = AppWindowId::foreground().filter(|_| enabled) else {
            return;
        };
        if !this.floating.remove(&front) {
            let Some(shown) = this.shown_of(front) else {
                return;
            };
            let (place, display) = (shown.place.clone(), shown.display);
            let layout = layout_of(&this.library, &shown.layout);
            if let Some(tiles) = this.places.get_mut(&place) {
                tiles.take(front, layout);
            }
            this.floating.insert(front);
            this.managed.remove(&front);
            this.sent.remove(&front);
            let work = look.of(display).map(|seen| seen.work);
            this.float_out(front, work, true, spring);
        }
        this.carried = true;
        this.arrange(&look, spring);
        this.note();
    }

    /// Sends the window `id`, taken out of the tiles, to the middle of
    /// `work` at the size `tiling.float` gives, or else, where `back`, to
    /// where it was before it was tiled.
    fn float_out(
        &mut self,
        id: AppWindowId,
        work: Option<ScreenRect>,
        back: bool,
        spring: SpringSpec,
    ) {
        let Some(from) = id.frame() else {
            return;
        };
        let to = match (self.knobs.float, work) {
            (Some((width, height)), Some(work)) => {
                let (room_x, room_y) = (work.right - work.left, work.bottom - work.top);
                let (width, height) = (
                    (room_x as f32 * width) as i32,
                    (room_y as f32 * height) as i32,
                );
                let (x, y) = center(work);
                Some(ScreenRect {
                    left: x - width / 2,
                    top: y - height / 2,
                    right: x - width / 2 + width,
                    bottom: y - height / 2 + height,
                })
            }
            _ if back => self.before.remove(&id),
            _ => None,
        };
        if let Some(to) = to {
            self.send(id, from, to, spring);
        }
    }

    /// Has the window in front trade tiles with the one on the side
    /// `toward` of it in the same place, while windows are tiled.
    pub fn carry(toward: Toward, cx: &mut App) {
        let enabled = cx.global::<TilingMode>().enabled;
        let look = Look::now(cx);
        let spring = cx.global::<Theme>().motion.spatial;
        let this = cx.global_mut::<Self>();
        let Some((front, other, true)) = this.beside(toward).filter(|_| enabled) else {
            return;
        };
        let swapped = { this.places.values_mut() }.any(|tiles| tiles.swap(front, other));
        if swapped {
            this.carried = true;
            this.arrange(&look, spring);
        }
    }

    /// Colors the border of the tiled window in front, if the
    /// configuration asks for it and windows are tiled, fading from the
    /// first color of `fade` to the second, and gives the window that was
    /// in front before its own border back; or draws a border of
    /// Tessera's own around it. Whether the color still changes is
    /// returned.
    fn mark(&mut self, enabled: bool, (from, to): (Hsla, Hsla), spring: SpringSpec) -> bool {
        let front = enabled
            .then(AppWindowId::foreground)
            .flatten()
            .filter(|id| self.managed.contains(id));
        match self.knobs.marking {
            Marking::None => {
                self.unmark();
                false
            }
            Marking::Drawn(look) => {
                // A window on its way gets its border once it is there.
                let still = front.filter(|id| !self.moves.contains_key(id));
                let wanted = still.and_then(|id| {
                    let color = look.color.unwrap_or_else(|| argb(to));
                    Some((id, id.frame()?, look.width, look.offset, color))
                });
                if wanted != self.drawn {
                    match wanted {
                        Some((id, frame, width, offset, color)) => {
                            TileBorder::show(id, frame, width, offset, color);
                        }
                        None => TileBorder::hide(),
                    }
                    self.drawn = wanted;
                }
                false
            }
            Marking::Windows => {
                let front = front.filter(|id| !self.borderless.contains(id));
                if self.marked.as_ref().map(|marked| marked.id) != front {
                    self.unmark();
                    self.marked = front.map(|id| {
                        let mut shown = Spring::new(spring, 0.);
                        shown.animate_to(1.);
                        Marked {
                            id,
                            shown,
                            painted: None,
                        }
                    });
                }
                let Some(marked) = &mut self.marked else {
                    return false;
                };
                let shown = marked.shown.value_at(Instant::now()).clamp(0., 1.);
                let color = border_color(mix(from, to, shown));
                if marked.painted != Some(color) {
                    marked.id.set_border(Some(color));
                    marked.painted = Some(color);
                }
                !marked.shown.is_settled()
            }
        }
    }

    /// Gives the window whose border is colored its own border back, or
    /// none, where the rules say it has none, and takes the border
    /// Tessera draws away.
    fn unmark(&mut self) {
        if let Some(marked) = self.marked.take() {
            match self.borderless.contains(&marked.id) {
                true => marked.id.hide_border(),
                false => marked.id.set_border(None),
            };
        }
        if self.drawn.take().is_some() {
            TileBorder::hide();
        }
    }

    /// Sends every window that is not in its tile on its way there.
    fn arrange(&mut self, look: &Look, spring: SpringSpec) {
        let windows: Vec<AppWindow> = AppWindow::enumerate()
            .into_iter()
            .filter(|window| self.takes(window))
            .filter(|window| window.id.is_tileable())
            .collect();
        let present = |id: &AppWindowId| windows.iter().any(|window| window.id == *id);
        self.moves.retain(|id, _| present(id));
        self.sent.retain(|id, _| present(id));
        self.floating.retain(present);
        // Where a window was is remembered while the window exists, also
        // while it is minimized or maximized.
        self.before.retain(|id, _| id.frame().is_some());

        // Windows let go of, and the window in front, which a program may
        // have resized: one dropped on another tile trades places with
        // it, and one resized by a border moves that border.
        let mut settling = std::mem::take(&mut self.ended);
        if !std::mem::take(&mut self.carried)
            && let Some(front) = look.front
            && !settling.contains(&front)
        {
            settling.push(front);
        }
        for id in settling {
            self.settle(id);
        }

        // A window on its way stays in the place that sent it, until it is
        // there.
        let mut belongs: HashMap<AppWindowId, DisplayKey> = std::mem::take(&mut self.claimed);
        for shown in &self.shown {
            let Some(tiles) = self.places.get(&shown.place) else {
                continue;
            };
            for &id in tiles.windows() {
                if self.moves.contains_key(&id) {
                    belongs.entry(id).or_insert(shown.display);
                }
            }
        }
        let display_of =
            |window: &AppWindow| belongs.get(&window.id).copied().unwrap_or(window.display);

        let mut places = Vec::new();
        let mut floated = Vec::new();
        self.shown.clear();
        self.managed.clear();
        for seen in &look.displays {
            let present: Vec<AppWindowId> = { windows.iter() }
                .filter(|window| display_of(window) == seen.key)
                .filter(|window| !self.floating.contains(&window.id))
                .map(|window| window.id)
                .collect();
            let place = self.config.place(&seen.name, seen.workspace);
            let tiles = tiles_of(&mut self.places, &mut self.kept, &seen.place);
            let choice = tiles.chosen.clone().unwrap_or(place.layout.clone());
            let layout = layout_of(&self.library, &choice);
            let area = place.area(seen.work);
            for id in tiles.fit(&present, layout, look.front, area, place.gap) {
                floated.push((id, seen.work));
            }
            places.extend(tiles.places(area, place.gap));
            self.managed.extend(tiles.windows().iter().copied());
            self.shown.push(Shown {
                display: seen.key,
                place: seen.place.clone(),
                layout: choice,
                area,
                gap: place.gap,
            });
        }
        for (id, work) in floated {
            self.floating.insert(id);
            self.float_out(id, Some(work), false, spring);
        }

        for (id, tile) in places {
            let Some(frame) = id.frame() else {
                continue;
            };
            if frame == tile {
                self.sent.remove(&id);
                continue;
            }
            match self.sent.get(&id) {
                Some(&(sent, None)) if sent == tile => {
                    self.sent.insert(id, (tile, Some(frame)));
                    continue;
                }
                Some(&(sent, Some(kept))) if sent == tile && kept == frame => continue,
                _ => {}
            }
            if self.moves.get(&id).is_some_and(|moving| moving.to == tile) {
                continue;
            }
            self.before.entry(id).or_insert(frame);
            self.send(id, frame, tile, spring);
        }
        self.note();
    }

    /// Looks at the window `id`, which may have been let go of off its
    /// tile: dropped with its middle on another tile it trades places
    /// with that window, or takes its place in the order, as `drag` says;
    /// dropped on a display without a tile there it goes to that display;
    /// resized by a border it moves the borders it was resized by.
    /// Otherwise it goes back to its tile.
    fn settle(&mut self, id: AppWindowId) {
        if self.moves.contains_key(&id) {
            return;
        }
        let Some(frame) = id.frame() else {
            return;
        };
        let Some(home) = self.shown.iter().position(|shown| {
            self.places
                .get(&shown.place)
                .is_some_and(|tiles| tiles.holds(id))
        }) else {
            return;
        };
        let (area, gap) = (self.shown[home].area, self.shown[home].gap);
        let Some(tile) = self
            .places
            .get(&self.shown[home].place)
            .and_then(|tiles| tiles.tile_of(id, area, gap))
        else {
            return;
        };
        if frame == tile {
            return;
        }
        // An application that keeps a size of its own was not resized.
        if matches!(self.sent.get(&id), Some(&(_, Some(kept))) if kept == frame) {
            return;
        }
        if !same_size(tile, frame) {
            let place = self.shown[home].place.clone();
            if let Some(tiles) = self.places.get_mut(&place) {
                tiles.resized(id, frame, area, gap);
            }
            return;
        }
        let middle = center(frame);
        if holds(tile, middle) {
            return;
        }
        // The tile the middle of the window is on, of any place shown.
        let mut onto = None;
        for (index, shown) in self.shown.iter().enumerate() {
            let Some(tiles) = self.places.get(&shown.place) else {
                continue;
            };
            let found = { tiles.places(shown.area, shown.gap).into_iter() }
                .find(|&(other, place)| other != id && holds(place, middle));
            if let Some((other, _)) = found {
                onto = Some((index, Some(other)));
                break;
            }
        }
        // Or a display without a tile there.
        let onto = onto.or_else(|| {
            let index = self
                .shown
                .iter()
                .position(|shown| holds(shown.area, middle))?;
            (index != home).then_some((index, None))
        });
        let Some((target, other)) = onto else {
            return;
        };
        let source_place = self.shown[home].place.clone();
        let target_place = self.shown[target].place.clone();
        if target == home {
            let Some(other) = other else {
                return;
            };
            if let Some(tiles) = self.places.get_mut(&source_place) {
                match self.knobs.drag {
                    DragMode::Swap => tiles.swap(id, other),
                    DragMode::Sort => tiles.sort(id, other),
                };
            }
            return;
        }
        // From one display to another.
        let source_layout = layout_of(&self.library, &self.shown[home].layout);
        let target_layout = layout_of(&self.library, &self.shown[target].layout);
        let (target_area, target_gap) = (self.shown[target].area, self.shown[target].gap);
        let (source_display, target_display) =
            (self.shown[home].display, self.shown[target].display);
        let Some(from) = self
            .places
            .get_mut(&source_place)
            .and_then(|tiles| tiles.take(id, source_layout))
        else {
            return;
        };
        let Some(tiles) = self.places.get_mut(&target_place) else {
            return;
        };
        let to = other.and_then(|other| tiles.windows().iter().position(|&listed| listed == other));
        let swapping = self.knobs.drag == DragMode::Swap && other.is_some();
        if let (true, Some(other)) = (swapping, other) {
            tiles.take(other, target_layout);
            if let Some(source) = self.places.get_mut(&source_place) {
                source.put(other, from, source_layout, area, gap);
            }
            self.claimed.insert(other, source_display);
        }
        if let Some(tiles) = self.places.get_mut(&target_place) {
            let at = to.unwrap_or(tiles.windows().len());
            tiles.put(id, at, target_layout, target_area, target_gap);
        }
        self.claimed.insert(id, target_display);
    }

    fn send(&mut self, id: AppWindowId, from: ScreenRect, to: ScreenRect, spring: SpringSpec) {
        let mut progress = Spring::new(spring, 0.);
        progress.animate_to(1.);
        self.moves.insert(id, Move { from, to, progress });
    }

    /// Moves the windows that are on their way a step further.
    fn step(&mut self, now: Instant) {
        let sent = &mut self.sent;
        self.moves.retain(|&id, moving| {
            let progress = moving.progress.value_at(now);
            if moving.progress.is_settled() {
                id.place(moving.to);
                sent.insert(id, (moving.to, None));
                return false;
            }
            // A window that is gone, or one Windows does not let be
            // moved, is given up.
            id.place(between(moving.from, moving.to, progress))
        });
    }

    /// Sends every window back to where it was before it was tiled.
    fn put_back(&mut self) {
        self.unmark();
        let spring = self.moves.values().next().map(|moving| moving.progress);
        self.let_go();
        for (id, frame) in std::mem::take(&mut self.before) {
            if !id.is_tileable() {
                continue;
            }
            let Some(from) = id.frame() else {
                continue;
            };
            let mut progress = spring.unwrap_or_else(|| Spring::new(BACK, 0.));
            progress.jump_to(0.);
            progress.animate_to(1.);
            let to = frame;
            self.moves.insert(id, Move { from, to, progress });
        }
    }

    /// Puts every window back where it was before it was tiled, without
    /// taking time over it.
    fn put_back_at_once(&mut self) {
        self.unmark();
        self.moves.clear();
        self.let_go();
        for (id, frame) in std::mem::take(&mut self.before) {
            if id.is_tileable() {
                id.place(frame);
            }
        }
    }

    /// Lets go of every tiled window; the places keep their trees for
    /// when tiling comes back.
    fn let_go(&mut self) {
        for tiles in self.places.values_mut() {
            tiles.clear();
        }
        self.managed.clear();
        self.sent.clear();
        self.floating.clear();
        self.claimed.clear();
        self.note();
    }

    /// Everything that is kept, as the file writes it.
    fn kept_text(&self) -> String {
        let mut kept = self.kept.clone();
        for (key, tiles) in &self.places {
            kept.insert(key.clone(), tiles.kept());
        }
        state::to_text(&kept)
    }

    /// Takes in that what is kept may have changed.
    fn note(&mut self) {
        let text = self.kept_text();
        if text != self.pending {
            self.pending = text;
            self.changed = Some(Instant::now());
        }
    }

    /// What is to be written now, once nothing changed for a while.
    fn due_write(&mut self) -> Option<String> {
        let changed = self.changed?;
        if changed.elapsed() < WRITE_AFTER {
            return None;
        }
        self.changed = None;
        if self.pending == self.written {
            return None;
        }
        self.written = self.pending.clone();
        Some(self.written.clone())
    }

    /// Whether the directory of layout files is to be looked at now.
    fn layouts_due(&mut self) -> bool {
        let due = self
            .layouts_looked
            .is_none_or(|looked| looked.elapsed() >= LAYOUTS_LOOK);
        if !due || self.scanning {
            return false;
        }
        self.scanning = true;
        self.layouts_looked = Some(Instant::now());
        true
    }

    /// Takes over the layout files as `scan` found them, if they changed.
    fn take_scan(&mut self, scan: declarative::Scan) {
        if self.layouts_stamp == Some(scan.stamp) {
            return;
        }
        for problem in &scan.problems {
            report!("{problem}");
        }
        self.layouts_stamp = Some(scan.stamp);
        self.library = Library::with(scan.layouts);
        self.nudged = true;
    }

    /// Reports the layouts the configuration names that no file has.
    fn check_layouts(&self) {
        for choice in self.config.layouts() {
            if let LayoutChoice::File(name) = choice
                && self.library.get(name).is_none()
            {
                report!(
                    "there is no tiling layout named `{name}`, so `master` is used where it is named"
                );
            }
        }
    }
}

/// What a look at the windows came to.
struct Tick {
    /// How long until the next look.
    pause: Duration,
    /// What is kept, to be written.
    write: Option<String>,
    /// The layout files are to be read where they changed since they were
    /// as this stamp says.
    scan: Option<Option<u64>>,
}

/// The tiles of the place `key`, made from what was kept of it if they
/// are not there yet.
fn tiles_of<'a>(
    places: &'a mut HashMap<PlaceKey, Tiles>,
    kept: &mut BTreeMap<PlaceKey, Kept>,
    key: &PlaceKey,
) -> &'a mut Tiles {
    places
        .entry(key.clone())
        .or_insert_with(|| Tiles::from_kept(kept.remove(key).unwrap_or_default()))
}

/// The layout `choice` names; a file that is not there gives `master`.
fn layout_of<'a>(library: &'a Library, choice: &LayoutChoice) -> Layout<'a> {
    match choice {
        LayoutChoice::Builtin(layout) => Layout::Builtin(*layout),
        LayoutChoice::File(name) => library
            .get(name)
            .map_or(Layout::Builtin(TileLayout::Master), Layout::Declared),
    }
}

/// The directory of the layout files of the user.
fn layouts_directory() -> Option<PathBuf> {
    Some(config_dir()?.join(LAYOUTS_DIRECTORY))
}

/// How windows move back when tiling is switched off: without overshoot.
const BACK: SpringSpec = SpringSpec {
    stiffness: 380.,
    damping_ratio: 1.,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_on_a_side_is_the_nearest_one_that_way() {
        let rect = |left, top, right, bottom| ScreenRect {
            left,
            top,
            right,
            bottom,
        };
        // A wide window on the left, three in a column on the right.
        let master = rect(0, 0, 600, 900);
        let stack = [
            ('a', rect(600, 0, 1000, 300)),
            ('b', rect(600, 300, 1000, 600)),
            ('c', rect(600, 600, 1000, 900)),
        ];
        assert_eq!(Toward::Right.of(master, &stack), Some('b'));
        assert_eq!(Toward::Left.of(master, &stack), None);
        let others = [('m', master), stack[1], stack[2]];
        assert_eq!(Toward::Down.of(stack[0].1, &others), Some('b'));
        assert_eq!(Toward::Left.of(stack[0].1, &others), Some('m'));
        assert_eq!(Toward::Up.of(stack[0].1, &others), None);
        assert_eq!(Toward::Right.of(stack[0].1, &others), None);
    }

    #[test]
    fn a_side_has_one_of_four_names() {
        assert_eq!(Toward::parse("left"), Some(Toward::Left));
        assert_eq!(Toward::parse("down"), Some(Toward::Down));
        assert_eq!(Toward::parse("back"), None);
    }

    #[test]
    fn a_ring_goes_on_past_its_ends() {
        assert_eq!(around(0, 3, false), 1);
        assert_eq!(around(2, 3, false), 0);
        assert_eq!(around(0, 3, true), 2);
    }

    #[test]
    fn the_modes_come_round() {
        let mode = |enabled, layout| TilingMode {
            enabled,
            layout: LayoutChoice::Builtin(layout),
        };
        let builtin = TileLayout::ALL.map(LayoutChoice::Builtin);
        let off = mode(false, TileLayout::Monocle);
        assert_eq!(off.next(&builtin), mode(true, TileLayout::Master));
        assert_eq!(
            mode(true, TileLayout::Master).next(&builtin),
            mode(true, TileLayout::Columns)
        );
        assert_eq!(
            mode(true, TileLayout::Centered).next(&builtin),
            mode(false, TileLayout::Master)
        );
        let choices = [
            LayoutChoice::Builtin(TileLayout::Rows),
            LayoutChoice::File("tall".to_owned()),
        ];
        let rows = mode(true, TileLayout::Rows).next(&choices);
        assert_eq!(rows.layout, choices[1]);
        let last = rows.next(&choices);
        assert!(!last.enabled);
        // A layout that is no longer among them ends the tiling.
        let gone = TilingMode {
            enabled: true,
            layout: LayoutChoice::File("gone".to_owned()),
        };
        assert!(!gone.next(&choices).enabled);
    }

    #[test]
    fn a_piece_of_a_title_leaves_a_window_alone() {
        let pieces = ["Picture-in-Picture".to_owned(), " ".to_owned()];
        assert!(titled(&pieces, "picture-in-picture"));
        assert!(!titled(&pieces, "Editor"));
        assert!(!titled(&[" ".to_owned()], "a b"));
    }

    #[test]
    fn a_border_color_is_blue_green_red() {
        let red = Hsla::from(gpui::Rgba {
            r: 1.,
            g: 0.,
            b: 0.,
            a: 1.,
        });
        assert_eq!(border_color(red), 0x0000_00ff);
        assert_eq!(argb(red), 0xffff_0000);
    }

    #[test]
    fn the_configuration_chooses_the_programs() {
        let config = |text: &str| toml::from_str::<TilingConfig>(text).unwrap();
        let covers = |config: &TilingConfig, path: &str| config.covers(Some(Path::new(path)));
        let all = config("");
        assert!(covers(&all, r"C:\a\Editor.exe"));
        assert!(!all.covers(None));
        let some = config("only = [\"editor.exe\", \"term.exe\"]\nexcept = [\"TERM.exe\"]");
        assert!(covers(&some, r"C:\a\Editor.exe"));
        assert!(!covers(&some, r"C:\a\term.exe"));
        assert!(!covers(&some, r"C:\a\other.exe"));
        // A layout that is no layout of Tessera's own names a file.
        assert_eq!(
            config("layout = \"spiral\"").layout,
            LayoutChoice::File("spiral".to_owned())
        );
    }

    #[test]
    fn a_window_on_its_way_is_between_where_it_was_and_its_tile() {
        let from = ScreenRect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        let to = ScreenRect {
            left: 200,
            top: 100,
            right: 600,
            bottom: 300,
        };
        assert_eq!(between(from, to, 0.), from);
        assert_eq!(between(from, to, 1.), to);
        let half = between(from, to, 0.5);
        assert_eq!(
            (half.left, half.top, half.right, half.bottom),
            (100, 50, 350, 200)
        );
        assert!(between(from, to, 1.1).left > to.left);
    }
}
