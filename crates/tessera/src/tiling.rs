// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Tiling: application windows arranged side by side, so that they fill
//! what the surfaces of the shell leave of each display.
//!
//! Only windows that can be resized by their border take part, and of
//! those only the ones that are neither minimized nor maximized. A window
//! that is dragged onto the tile of another trades places with it, and so
//! does the window in front when a key combination sends it toward a
//! side; another combination brings the window on that side in front. When
//! tiling is switched off, and when the shell quits, every window goes
//! back to where it was before it was first tiled.
//!
//! The tiled window in front can be told apart by its border, which
//! Windows draws in a color of the theme while it is asked to.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{App, AsyncApp, Global, Hsla};
use serde::Deserialize;

use crate::motion::{Spring, SpringSpec, mix};
use crate::platform::{AppWindow, AppWindowId, DisplayKey, ScreenRect, WindowEvents, pointer_held};
use crate::theme::Theme;

/// How often it is checked whether something happened to the windows.
const CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// How often the windows are looked at even though nothing was heard of
/// them: a window that stops being maximized, for one, goes unannounced.
const LOOK_INTERVAL: Duration = Duration::from_secs(2);

/// The time between two steps of windows that are moving.
const FRAME: Duration = Duration::from_millis(8);

/// The distance between tiles when the configuration sets none, and the
/// widest it may set, in pixels of the screen.
const DEFAULT_GAP: f32 = 8.;
const WIDEST_GAP: f32 = 200.;

/// The share of the width the first window of `master` takes when nothing
/// changed it, how much a key combination changes it by, and the least
/// and the most it may be.
const EVEN_SHARE: f32 = 0.5;
const SHARE_STEP: f32 = 0.05;
const SHARE_RANGE: (f32, f32) = (0.2, 0.8);

/// How the windows of a display share it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileLayout {
    /// The first window takes the left half, the others share the right
    /// one, stacked.
    #[default]
    Master,
    /// Side by side.
    Columns,
    /// One above the other.
    Rows,
    /// Rows of equally many windows; the last row may hold fewer, wider
    /// ones.
    Grid,
    /// Every window takes the whole space, one over the other.
    Monocle,
}

impl TileLayout {
    pub const ALL: [Self; 5] = [
        Self::Master,
        Self::Columns,
        Self::Rows,
        Self::Grid,
        Self::Monocle,
    ];

    /// What the layout is called where it is shown.
    pub fn name(self) -> &'static str {
        match self {
            Self::Master => "Master",
            Self::Columns => "Columns",
            Self::Rows => "Rows",
            Self::Grid => "Grid",
            Self::Monocle => "Monocle",
        }
    }

    /// The places of `count` windows in `area`, `gap` apart from each
    /// other and from the border of the area, the first window of
    /// `master` taking half the width.
    #[cfg(test)]
    pub fn tiles(self, area: ScreenRect, count: usize, gap: i32) -> Vec<ScreenRect> {
        self.shared(area, count, gap, EVEN_SHARE)
    }

    /// The places of `count` windows in `area`, `gap` apart from each
    /// other and from the border of the area. In `master` the first
    /// window takes `share` of the width, from 0 to 1.
    pub fn shared(self, area: ScreenRect, count: usize, gap: i32, share: f32) -> Vec<ScreenRect> {
        if count == 0 {
            return Vec::new();
        }
        let area = ScreenRect {
            left: area.left + gap,
            top: area.top + gap,
            right: area.right - gap,
            bottom: area.bottom - gap,
        };
        let across = |parts| spans(area.left, area.right, parts, gap);
        let down = |parts| spans(area.top, area.bottom, parts, gap);
        let column = |(left, right): (i32, i32), (top, bottom): (i32, i32)| ScreenRect {
            left,
            top,
            right,
            bottom,
        };
        let whole_height = (area.top, area.bottom);
        let whole_width = (area.left, area.right);
        match self {
            Self::Columns => across(count)
                .into_iter()
                .map(|span| column(span, whole_height))
                .collect(),
            Self::Rows => down(count)
                .into_iter()
                .map(|span| column(whole_width, span))
                .collect(),
            Self::Monocle => vec![area; count],
            Self::Master if count == 1 => vec![area],
            Self::Master => {
                let room = (area.right - area.left - gap).max(2);
                let first = ((room as f32 * share) as i32).clamp(1, room - 1);
                let sides = [
                    (area.left, area.left + first),
                    (area.left + first + gap, area.right),
                ];
                let mut tiles = vec![column(sides[0], whole_height)];
                let stacked = down(count - 1);
                tiles.extend(stacked.into_iter().map(|span| column(sides[1], span)));
                tiles
            }
            Self::Grid => {
                let columns = (1..=count)
                    .find(|columns| columns * columns >= count)
                    .unwrap_or(count);
                let rows = count.div_ceil(columns);
                let mut tiles = Vec::with_capacity(count);
                for (row, span) in down(rows).into_iter().enumerate() {
                    let held = columns.min(count - row * columns);
                    let spans = spans(area.left, area.right, held, gap);
                    tiles.extend(spans.into_iter().map(|across| column(across, span)));
                }
                tiles
            }
        }
    }
}

/// Divides the stretch from `start` to `end` into `parts` that are `gap`
/// apart and differ in length by a pixel at most.
fn spans(start: i32, end: i32, parts: usize, gap: i32) -> Vec<(i32, i32)> {
    let parts = parts.max(1) as i32;
    let room = (end - start - gap * (parts - 1)).max(parts);
    let edge = |part: i32| start + room * part / parts + gap * part;
    (0..parts)
        .map(|part| (edge(part), edge(part + 1) - gap))
        .collect()
}

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

/// What the configuration says about tiling.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TilingConfig {
    /// Whether windows are tiled from the start.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub layout: TileLayout,
    /// The distance between tiles, in pixels of the screen.
    gap: Option<f32>,
    /// The programs whose windows are tiled, by the file name of the
    /// executable; every program when empty.
    #[serde(default)]
    only: Vec<String>,
    /// The programs whose windows are left alone.
    #[serde(default)]
    except: Vec<String>,
    /// Pieces of titles: a window whose title has one of them in it is
    /// left alone, whatever the case of the letters.
    #[serde(default)]
    except_titles: Vec<String>,
    /// Whether the border of the tiled window in front is colored.
    #[serde(default)]
    border: bool,
}

impl TilingConfig {
    /// The distance between tiles. One that is not usable is reported and
    /// replaced by the built-in one.
    fn gap(&self) -> i32 {
        let gap = self.gap.unwrap_or(DEFAULT_GAP);
        if gap.is_finite() && (0. ..=WIDEST_GAP).contains(&gap) {
            return gap.round() as i32;
        }
        report!("`tiling.gap` is not a number from 0 to {WIDEST_GAP}, so it is {DEFAULT_GAP}");
        DEFAULT_GAP as i32
    }

    /// Whether the windows of the program at `executable` are tiled. A
    /// program that Windows does not let be read is one whose windows it
    /// does not let be moved either.
    fn covers(&self, executable: Option<&Path>) -> bool {
        listed(&self.only, &self.except, executable)
    }

    /// Whether `window` is one to tile: its program is covered and its
    /// title, read only when titles are named, has none of their pieces.
    fn takes(&self, window: &AppWindow) -> bool {
        self.covers(window.executable.as_deref())
            && (self.except_titles.is_empty() || !titled(&self.except_titles, &window.id.title()))
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

/// Whether `title` has one of `pieces` in it, whatever the case of the
/// letters. A piece without letters is in no title.
fn titled(pieces: &[String], title: &str) -> bool {
    let title = title.to_lowercase();
    { pieces.iter() }
        .filter(|piece| !piece.trim().is_empty())
        .any(|piece| title.contains(&piece.to_lowercase()))
}

/// Whether windows are tiled right now, and how. It starts as the
/// configuration says and changes with the `tiling` widget, until the
/// configuration is read again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilingMode {
    pub enabled: bool,
    pub layout: TileLayout,
}

impl Global for TilingMode {}

impl TilingMode {
    /// What follows when the modes are gone through one by one: every
    /// layout in turn, then no tiling.
    pub fn next(self) -> Self {
        let place = TileLayout::ALL
            .iter()
            .position(|&layout| layout == self.layout);
        let following = place.and_then(|place| TileLayout::ALL.get(place + 1));
        match (self.enabled, following) {
            (false, _) => Self {
                enabled: true,
                layout: TileLayout::ALL[0],
            },
            (true, Some(&layout)) => Self {
                enabled: true,
                layout,
            },
            (true, None) => Self {
                enabled: false,
                layout: TileLayout::ALL[0],
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

/// `share` a step larger, or smaller, within what it may be.
fn stepped(share: f32, wider: bool) -> f32 {
    let step = if wider { SHARE_STEP } else { -SHARE_STEP };
    let (least, most) = SHARE_RANGE;
    (share + step).clamp(least, most)
}

/// The share of the width the first window of `master` takes once the
/// border between it and the stacked windows was dragged: `frame` is the
/// window that was resized and `tile` its place, in `area` with tiles
/// `gap` apart; `first` says whether it is the first window. `None` unless
/// that border alone was moved.
fn share_dragged(
    area: ScreenRect,
    gap: i32,
    first: bool,
    frame: ScreenRect,
    tile: ScreenRect,
) -> Option<f32> {
    if frame.top != tile.top || frame.bottom != tile.bottom {
        return None;
    }
    let edge = match first {
        true if frame.left == tile.left && frame.right != tile.right => frame.right,
        false if frame.right == tile.right && frame.left != tile.left => frame.left - gap,
        _ => return None,
    };
    let room = (area.right - area.left - 3 * gap).max(2);
    let (least, most) = SHARE_RANGE;
    Some(((edge - area.left - gap) as f32 / room as f32).clamp(least, most))
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

/// Keeps the windows in their tiles.
pub struct Tiler {
    config: TilingConfig,
    /// The tiled windows, in the order they take their tiles in.
    order: Vec<AppWindowId>,
    moves: HashMap<AppWindowId, Move>,
    /// Where each window was before it was first tiled.
    before: HashMap<AppWindowId, ScreenRect>,
    sent: HashMap<AppWindowId, Sent>,
    /// The windows taken out of the tiles, which stay where they are put.
    floating: HashSet<AppWindowId>,
    /// The share of the width the first window of `master` takes.
    share: f32,
    marked: Option<Marked>,
    /// Whether the order was changed by a key combination, so that the
    /// window in front lies on a tile that is no longer its own without
    /// having been dropped there.
    carried: bool,
    /// Counts the tasks that were started to keep the tiles; only the
    /// latest goes on.
    task: u64,
}

impl Global for Tiler {}

impl Tiler {
    /// Starts tiling as `config` says.
    pub fn start(config: &TilingConfig, cx: &mut App) {
        cx.set_global(Self {
            config: config.clone(),
            order: Vec::new(),
            moves: HashMap::new(),
            before: HashMap::new(),
            sent: HashMap::new(),
            floating: HashSet::new(),
            share: EVEN_SHARE,
            marked: None,
            carried: false,
            task: 0,
        });
        cx.on_app_quit(|cx| {
            cx.global_mut::<Self>().put_back_at_once();
            async {}
        })
        .detach();
        Self::set_mode(
            TilingMode {
                enabled: config.enabled,
                layout: config.layout,
            },
            cx,
        );
    }

    /// Takes over a configuration that was read again, if it says
    /// something else about tiling than the one before.
    pub fn configure(config: &TilingConfig, cx: &mut App) {
        if cx.global::<Self>().config == *config {
            return;
        }
        cx.global_mut::<Self>().config = config.clone();
        Self::set_mode(
            TilingMode {
                enabled: config.enabled,
                layout: config.layout,
            },
            cx,
        );
    }

    /// Tiles, or stops tiling, as `mode` says.
    pub fn set_mode(mode: TilingMode, cx: &mut App) {
        cx.set_global(mode);
        let this = cx.global_mut::<Self>();
        this.task += 1;
        let task = this.task;
        if !mode.enabled {
            this.put_back();
        } else if !WindowEvents::watch() {
            report!("Windows does not report window changes, so tiles follow them late");
        }
        let spring = cx.global::<Theme>().motion.spatial;
        cx.spawn(async move |cx: &mut AsyncApp| {
            let mut seen = None;
            let mut looked = Instant::now();
            loop {
                let moving = cx.update(|cx| {
                    let mode = *cx.global::<TilingMode>();
                    let theme = *cx.global::<Theme>();
                    let fade = (theme.colors.outline, theme.colors.primary);
                    let this = cx.global_mut::<Self>();
                    if this.task != task {
                        return None;
                    }
                    let now = WindowEvents::generation();
                    let due = seen != Some(now) || looked.elapsed() >= LOOK_INTERVAL;
                    // A window that is being dragged is left to the hand.
                    if mode.enabled && due && !pointer_held() {
                        (seen, looked) = (Some(now), Instant::now());
                        this.arrange(mode.layout, spring);
                    }
                    this.step(Instant::now());
                    let fading = this.mark(mode.enabled, fade, theme.motion.effects);
                    let moving = fading || !this.moves.is_empty();
                    (mode.enabled || moving).then_some(moving)
                });
                let pause = match moving {
                    Ok(Some(true)) => FRAME,
                    Ok(Some(false)) => CHECK_INTERVAL,
                    _ => break,
                };
                cx.background_executor().timer(pause).await;
            }
        })
        .detach();
    }

    /// The window in front and the window on the side `toward` of it,
    /// among the windows tiling covers, and whether both are on one
    /// display.
    fn beside(&self, toward: Toward) -> Option<(AppWindowId, AppWindowId, bool)> {
        let front = AppWindowId::foreground()?;
        let windows: Vec<AppWindow> = AppWindow::enumerate()
            .into_iter()
            .filter(|window| self.config.takes(window))
            .filter(|window| window.id.is_tileable())
            .collect();
        let display = |id| Some(windows.iter().find(|window| window.id == id)?.display);
        let home = display(front)?;
        let others: Vec<(AppWindowId, ScreenRect)> = { windows.iter() }
            .filter(|window| window.id != front)
            .filter_map(|window| Some((window.id, window.id.frame()?)))
            .collect();
        let other = toward.of(front.frame()?, &others)?;
        Some((front, other, display(other) == Some(home)))
    }

    /// The tiled window after the one in front on its display, in the
    /// order of the tiles, or the one before it with `back`.
    fn following(&self, back: bool) -> Option<AppWindowId> {
        let front = AppWindowId::foreground()?;
        let windows = AppWindow::enumerate();
        let display = |id| Some(windows.iter().find(|window| window.id == id)?.display);
        let home = display(front)?;
        let there = |id: &AppWindowId| display(*id) == Some(home);
        let ids: Vec<AppWindowId> = self.order.iter().copied().filter(there).collect();
        let at = ids.iter().position(|&id| id == front)?;
        let other = ids[around(at, ids.len(), back)];
        (other != front).then_some(other)
    }

    /// Brings the window on the side `toward` of the window in front, in
    /// front itself. Where every window takes the whole space there are
    /// no sides: right and down are the next window then, left and up
    /// the one before.
    pub fn focus(toward: Toward, cx: &mut App) {
        let mode = *cx.global::<TilingMode>();
        let this = cx.global::<Self>();
        let stacked = mode.enabled && mode.layout == TileLayout::Monocle;
        let back = matches!(toward, Toward::Left | Toward::Up);
        let other = stacked.then(|| this.following(back)).flatten();
        let other = other.or_else(|| Some(this.beside(toward)?.1));
        if let Some(other) = other {
            other.activate();
        }
    }

    /// Takes the window in front out of the tiles, back to where it was
    /// before it was tiled, or puts it among them again.
    pub fn float(cx: &mut App) {
        let mode = *cx.global::<TilingMode>();
        let spring = cx.global::<Theme>().motion.spatial;
        let this = cx.global_mut::<Self>();
        let Some(front) = AppWindowId::foreground().filter(|_| mode.enabled) else {
            return;
        };
        if !this.floating.remove(&front) {
            if !this.order.contains(&front) {
                return;
            }
            this.floating.insert(front);
            this.order.retain(|&id| id != front);
            this.sent.remove(&front);
            if let (Some(to), Some(from)) = (this.before.remove(&front), front.frame()) {
                this.send(front, from, to, spring);
            }
        }
        this.arrange(mode.layout, spring);
    }

    /// Gives the first window of `master` more of the width, or less,
    /// while windows are tiled; the other layouts stay as they are.
    pub fn resize(wider: bool, cx: &mut App) {
        let mode = *cx.global::<TilingMode>();
        if !mode.enabled {
            return;
        }
        let spring = cx.global::<Theme>().motion.spatial;
        let this = cx.global_mut::<Self>();
        this.share = stepped(this.share, wider);
        // The windows are on their way to new tiles, not dropped on them.
        this.carried = true;
        this.arrange(mode.layout, spring);
    }

    /// Has the window in front trade tiles with the one on the side
    /// `toward` of it on the same display, while windows are tiled.
    pub fn carry(toward: Toward, cx: &mut App) {
        let mode = *cx.global::<TilingMode>();
        let spring = cx.global::<Theme>().motion.spatial;
        let this = cx.global_mut::<Self>();
        let Some((front, other, true)) = this.beside(toward) else {
            return;
        };
        let place = |id| this.order.iter().position(|&listed| listed == id);
        if let (true, Some(first), Some(second)) = (mode.enabled, place(front), place(other)) {
            this.order.swap(first, second);
            this.carried = true;
            this.arrange(mode.layout, spring);
        }
    }

    /// Colors the border of the tiled window in front, if the
    /// configuration asks for it and windows are tiled, fading from the
    /// first color of `fade` to the second, and gives the window that was
    /// in front before its own border back. Whether the color still
    /// changes is returned.
    fn mark(&mut self, enabled: bool, (from, to): (Hsla, Hsla), spring: SpringSpec) -> bool {
        let front = (enabled && self.config.border)
            .then(AppWindowId::foreground)
            .flatten()
            .filter(|id| self.order.contains(id));
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

    /// Gives the window whose border is colored its own border back.
    fn unmark(&mut self) {
        if let Some(marked) = self.marked.take() {
            marked.id.set_border(None);
        }
    }

    /// Sends every window that is not in its tile on its way there.
    fn arrange(&mut self, layout: TileLayout, spring: SpringSpec) {
        let windows: Vec<AppWindow> = AppWindow::enumerate()
            .into_iter()
            .filter(|window| self.config.takes(window))
            .filter(|window| window.id.is_tileable())
            .collect();
        let present = |id: &AppWindowId| windows.iter().any(|window| window.id == *id);
        self.order.retain(present);
        self.moves.retain(|id, _| present(id));
        self.sent.retain(|id, _| present(id));
        self.floating.retain(present);
        // Where a window was is remembered while the window exists, also
        // while it is minimized or maximized.
        self.before.retain(|id, _| id.frame().is_some());
        for window in &windows {
            if !self.order.contains(&window.id) && !self.floating.contains(&window.id) {
                self.order.push(window.id);
            }
        }

        let gap = self.config.gap();
        let mut places = Self::places(&self.order, &windows, layout, gap, self.share);
        // Windows a key combination had trade places are on their way to
        // their tiles, not dropped on them.
        let carried = std::mem::take(&mut self.carried);
        let dropped = match carried {
            true => None,
            false => self.traded(&places),
        };
        // A border dragged between the first window and the others gives
        // the first window another share.
        let resized = (!carried && dropped.is_none() && layout == TileLayout::Master)
            .then(|| self.resized(&places, &windows, gap))
            .flatten();
        if let Some(share) = resized {
            self.share = share;
            places = Self::places(&self.order, &windows, layout, gap, share);
        }
        if let Some((dragged, onto)) = dropped {
            let place = |id| self.order.iter().position(|&listed| listed == id);
            if let (Some(first), Some(second)) = (place(dragged), place(onto)) {
                self.order.swap(first, second);
                places = Self::places(&self.order, &windows, layout, gap, self.share);
            }
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
    }

    fn send(&mut self, id: AppWindowId, from: ScreenRect, to: ScreenRect, spring: SpringSpec) {
        let mut progress = Spring::new(spring, 0.);
        progress.animate_to(1.);
        self.moves.insert(id, Move { from, to, progress });
    }

    /// The tile of each window, display by display.
    fn places(
        order: &[AppWindowId],
        windows: &[AppWindow],
        layout: TileLayout,
        gap: i32,
        share: f32,
    ) -> Vec<(AppWindowId, ScreenRect)> {
        let mut displays: Vec<(DisplayKey, Vec<AppWindowId>)> = Vec::new();
        for &id in order {
            let Some(window) = windows.iter().find(|window| window.id == id) else {
                continue;
            };
            match displays.iter_mut().find(|(key, _)| *key == window.display) {
                Some((_, ids)) => ids.push(id),
                None => displays.push((window.display, vec![id])),
            }
        }
        let mut places = Vec::new();
        for (display, ids) in displays {
            let Some(area) = display.work_area() else {
                continue;
            };
            let tiles = layout.shared(area, ids.len(), gap, share);
            places.extend(ids.into_iter().zip(tiles));
        }
        places
    }

    /// The share the first window of `master` is to take, if the window
    /// in front was resized by the border between it and the others.
    fn resized(
        &self,
        places: &[(AppWindowId, ScreenRect)],
        windows: &[AppWindow],
        gap: i32,
    ) -> Option<f32> {
        let front = AppWindowId::foreground()?;
        if self.moves.contains_key(&front) {
            return None;
        }
        let frame = front.frame()?;
        // An application that keeps a size of its own was not resized.
        if matches!(self.sent.get(&front), Some(&(_, Some(kept))) if kept == frame) {
            return None;
        }
        let &(_, tile) = places.iter().find(|(id, _)| *id == front)?;
        let display = |id| Some(windows.iter().find(|window| window.id == id)?.display);
        let home = display(front)?;
        let mut there = { self.order.iter() }.filter(|&&id| display(id) == Some(home));
        let first = there.next()? == &front;
        there.next()?;
        share_dragged(home.work_area()?, gap, first, frame, tile)
    }

    /// The window in front, if it was dropped on the tile of another
    /// window, and that other window.
    fn traded(&self, places: &[(AppWindowId, ScreenRect)]) -> Option<(AppWindowId, AppWindowId)> {
        let front = AppWindowId::foreground()?;
        if self.moves.contains_key(&front) {
            return None;
        }
        let &(_, own) = places.iter().find(|(id, _)| *id == front)?;
        let middle = center(front.frame()?);
        if holds(own, middle) {
            return None;
        }
        let onto = places.iter().find(|(_, tile)| holds(*tile, middle))?;
        Some((front, onto.0))
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
        self.order.clear();
        self.sent.clear();
        self.floating.clear();
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
        for (id, frame) in std::mem::take(&mut self.before) {
            if id.is_tileable() {
                id.place(frame);
            }
        }
    }
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

    const AREA: ScreenRect = ScreenRect {
        left: 0,
        top: 40,
        right: 1001,
        bottom: 740,
    };

    fn overlap(first: ScreenRect, second: ScreenRect) -> bool {
        first.left < second.right
            && second.left < first.right
            && first.top < second.bottom
            && second.top < first.bottom
    }

    #[test]
    fn tiles_fill_the_area_without_overlapping() {
        let apart = |layout: &TileLayout| *layout != TileLayout::Monocle;
        for layout in TileLayout::ALL.into_iter().filter(apart) {
            for count in 1..=9 {
                let tiles = layout.tiles(AREA, count, 10);
                assert_eq!(tiles.len(), count, "{layout:?} of {count}");
                for (index, &tile) in tiles.iter().enumerate() {
                    assert!(tile.left >= AREA.left + 10 && tile.right <= AREA.right - 10);
                    assert!(tile.top >= AREA.top + 10 && tile.bottom <= AREA.bottom - 10);
                    assert!(tile.right - tile.left > 50 && tile.bottom - tile.top > 50);
                    let others = &tiles[index + 1..];
                    assert!(
                        others.iter().all(|&other| !overlap(tile, other)),
                        "{layout:?} of {count}"
                    );
                }
                let reaches = |edge: fn(&ScreenRect) -> i32, at: i32| {
                    tiles.iter().any(|tile| edge(tile) == at)
                };
                assert!(reaches(|tile| tile.left, 10) && reaches(|tile| tile.right, 991));
                assert!(reaches(|tile| tile.top, 50) && reaches(|tile| tile.bottom, 730));
            }
            assert!(layout.tiles(AREA, 0, 10).is_empty());
        }
    }

    #[test]
    fn each_layout_places_windows_its_own_way() {
        let tile = |left, top, right, bottom| ScreenRect {
            left,
            top,
            right,
            bottom,
        };
        let area = tile(0, 0, 1000, 600);
        assert_eq!(
            TileLayout::Master.tiles(area, 3, 0),
            [
                tile(0, 0, 500, 600),
                tile(500, 0, 1000, 300),
                tile(500, 300, 1000, 600)
            ]
        );
        assert_eq!(
            TileLayout::Columns.tiles(area, 2, 20),
            [tile(20, 20, 490, 580), tile(510, 20, 980, 580)]
        );
        assert_eq!(
            TileLayout::Rows.tiles(area, 2, 0),
            [tile(0, 0, 1000, 300), tile(0, 300, 1000, 600)]
        );
        // Three in a grid: two above, a wide one below.
        assert_eq!(
            TileLayout::Grid.tiles(area, 3, 0),
            [
                tile(0, 0, 500, 300),
                tile(500, 0, 1000, 300),
                tile(0, 300, 1000, 600)
            ]
        );
    }

    #[test]
    fn stacked_windows_each_take_the_whole_space() {
        let tiles = TileLayout::Monocle.tiles(AREA, 3, 10);
        assert_eq!(tiles.len(), 3);
        assert!(tiles.iter().all(|&tile| tile == tiles[0]));
        assert_eq!(tiles[0].left, AREA.left + 10);
        assert_eq!(tiles[0].bottom, AREA.bottom - 10);
    }

    #[test]
    fn a_ring_goes_on_past_its_ends() {
        assert_eq!(around(2, 3, false), 0);
        assert_eq!(around(0, 3, true), 2);
        assert_eq!(around(0, 1, false), 0);
    }

    #[test]
    fn the_modes_come_round() {
        let mut mode = TilingMode {
            enabled: false,
            layout: TileLayout::Grid,
        };
        let mut seen = Vec::new();
        for _ in 0..=TileLayout::ALL.len() {
            mode = mode.next();
            seen.push((mode.enabled, mode.layout));
        }
        let mut expected: Vec<_> = TileLayout::ALL
            .iter()
            .map(|&layout| (true, layout))
            .collect();
        expected.push((false, TileLayout::ALL[0]));
        assert_eq!(seen, expected);
    }

    #[test]
    fn a_dragged_border_gives_the_first_window_another_share() {
        let area = ScreenRect {
            left: 0,
            top: 0,
            right: 1024,
            bottom: 600,
        };
        let tiles = TileLayout::Master.shared(area, 3, 8, 0.5);
        let (first, second) = (tiles[0], tiles[1]);
        let wider = ScreenRect {
            right: first.right + 250,
            ..first
        };
        let share = share_dragged(area, 8, true, wider, first).unwrap();
        // The tiles of the new share put the border where it was dragged.
        let again = TileLayout::Master.shared(area, 3, 8, share);
        assert!((again[0].right - wider.right).abs() <= 1);
        // The border dragged from the other side gives the same share.
        let narrower = ScreenRect {
            left: second.left + 250,
            ..second
        };
        let other = share_dragged(area, 8, false, narrower, second).unwrap();
        assert!((other - share).abs() < 0.01);
        // A window in its tile, one resized at another border and one
        // that was moved were not resized at that border.
        assert_eq!(share_dragged(area, 8, true, first, first), None);
        let taller = ScreenRect {
            bottom: first.bottom - 40,
            ..wider
        };
        assert_eq!(share_dragged(area, 8, true, taller, first), None);
        let moved = ScreenRect {
            left: first.left + 30,
            right: first.right + 30,
            ..first
        };
        assert_eq!(share_dragged(area, 8, true, moved, first), None);
        // The share stays within what it may be.
        let most = ScreenRect {
            right: 1020,
            ..first
        };
        assert_eq!(
            share_dragged(area, 8, true, most, first),
            Some(SHARE_RANGE.1)
        );
    }

    #[test]
    fn the_first_window_of_master_takes_its_share() {
        let area = ScreenRect {
            left: 0,
            top: 0,
            right: 1010,
            bottom: 500,
        };
        let even = TileLayout::Master.shared(area, 2, 10, EVEN_SHARE);
        assert_eq!(even, TileLayout::Master.tiles(area, 2, 10));
        let wide = TileLayout::Master.shared(area, 2, 10, 0.75);
        assert_eq!((wide[0].left, wide[0].right), (10, 745));
        assert_eq!((wide[1].left, wide[1].right), (755, 1000));
        let columns = TileLayout::Columns;
        assert_eq!(columns.shared(area, 2, 10, 0.7), columns.tiles(area, 2, 10));

        assert!((stepped(0.5, true) - 0.55).abs() < 1e-6);
        assert!((stepped(0.78, true) - 0.8).abs() < 1e-6);
        assert!((stepped(0.2, false) - 0.2).abs() < 1e-6);
    }

    #[test]
    fn a_piece_of_a_title_leaves_a_window_alone() {
        let pieces = ["Picture-in-Picture".to_owned(), " ".to_owned()];
        assert!(titled(&pieces, "picture-in-picture"));
        assert!(titled(&pieces, "Video - Picture-in-Picture - Browser"));
        assert!(!titled(&pieces, "Two words"));
        assert!(!titled(&[], "Anything"));
        let config: TilingConfig = toml::from_str("except_titles = [\"Find\"]").unwrap();
        assert_eq!(config.except_titles, ["Find"]);
    }

    #[test]
    fn a_border_color_is_blue_green_red() {
        assert_eq!(border_color(gpui::rgb(0x112233).into()), 0x0033_2211);
        assert_eq!(border_color(gpui::rgb(0xffffff).into()), 0x00ff_ffff);
        let config: TilingConfig = toml::from_str("border = true").unwrap();
        assert!(config.border);
        assert!(!TilingConfig::default().border);
    }

    #[test]
    fn the_configuration_chooses_the_programs_and_the_gap() {
        let config = |text: &str| toml::from_str::<TilingConfig>(text).unwrap();
        let covers = |config: &TilingConfig, path: &str| config.covers(Some(Path::new(path)));

        let all = config("");
        assert!(!all.enabled);
        assert_eq!(all.layout, TileLayout::Master);
        assert_eq!(all.gap(), 8);
        assert!(covers(&all, r"C:\a\Editor.exe"));
        assert!(!all.covers(None));

        let some =
            config("only = [\"editor.exe\", \"term.exe\"]\nexcept = [\"TERM.exe\"]\ngap = 0");
        assert_eq!(some.gap(), 0);
        assert!(covers(&some, r"C:\a\Editor.exe"));
        assert!(!covers(&some, r"C:\a\term.exe"));
        assert!(!covers(&some, r"C:\a\other.exe"));

        assert_eq!(config("gap = -3").gap(), 8);
        assert_eq!(config("layout = \"grid\"").layout, TileLayout::Grid);
        assert!(toml::from_str::<TilingConfig>("layout = \"spiral\"").is_err());
        assert!(toml::from_str::<TilingConfig>("gaps = 1").is_err());
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
