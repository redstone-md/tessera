// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Tiling: application windows arranged side by side, so that they fill
//! what the surfaces of the shell leave of each display.
//!
//! Only windows that can be resized by their border take part, and of
//! those only the ones that are neither minimized nor maximized. A window
//! that is dragged onto the tile of another trades places with it. When
//! tiling is switched off, and when the shell quits, every window goes
//! back to where it was before it was first tiled.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{App, AsyncApp, Global};
use serde::Deserialize;

use crate::motion::{Spring, SpringSpec};
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
}

impl TileLayout {
    pub const ALL: [Self; 4] = [Self::Master, Self::Columns, Self::Rows, Self::Grid];

    /// What the layout is called where it is shown.
    pub fn name(self) -> &'static str {
        match self {
            Self::Master => "Master",
            Self::Columns => "Columns",
            Self::Rows => "Rows",
            Self::Grid => "Grid",
        }
    }

    /// The places of `count` windows in `area`, `gap` apart from each
    /// other and from the border of the area.
    pub fn tiles(self, area: ScreenRect, count: usize, gap: i32) -> Vec<ScreenRect> {
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
            Self::Master if count == 1 => vec![area],
            Self::Master => {
                let halves = across(2);
                let mut tiles = vec![column(halves[0], whole_height)];
                let stacked = down(count - 1);
                tiles.extend(stacked.into_iter().map(|span| column(halves[1], span)));
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
        let Some(name) = executable.and_then(Path::file_name) else {
            return false;
        };
        let name = name.to_string_lossy();
        let listed = |names: &[String]| {
            names
                .iter()
                .any(|listed| listed.eq_ignore_ascii_case(&name))
        };
        (self.only.is_empty() || listed(&self.only)) && !listed(&self.except)
    }
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
                    let moving = !this.moves.is_empty();
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

    /// Sends every window that is not in its tile on its way there.
    fn arrange(&mut self, layout: TileLayout, spring: SpringSpec) {
        let windows: Vec<AppWindow> = AppWindow::enumerate()
            .into_iter()
            .filter(|window| self.config.covers(window.executable.as_deref()))
            .filter(|window| window.id.is_tileable())
            .collect();
        let present = |id: &AppWindowId| windows.iter().any(|window| window.id == *id);
        self.order.retain(present);
        self.moves.retain(|id, _| present(id));
        self.sent.retain(|id, _| present(id));
        // Where a window was is remembered while the window exists, also
        // while it is minimized or maximized.
        self.before.retain(|id, _| id.frame().is_some());
        for window in &windows {
            if !self.order.contains(&window.id) {
                self.order.push(window.id);
            }
        }

        let gap = self.config.gap();
        let mut places = Self::places(&self.order, &windows, layout, gap);
        if let Some((dragged, onto)) = self.traded(&places) {
            let place = |id| self.order.iter().position(|&listed| listed == id);
            if let (Some(first), Some(second)) = (place(dragged), place(onto)) {
                self.order.swap(first, second);
                places = Self::places(&self.order, &windows, layout, gap);
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
            let tiles = layout.tiles(area, ids.len(), gap);
            places.extend(ids.into_iter().zip(tiles));
        }
        places
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
        let spring = self.moves.values().next().map(|moving| moving.progress);
        self.order.clear();
        self.sent.clear();
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
        for layout in TileLayout::ALL {
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
