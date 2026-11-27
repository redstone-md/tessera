// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Snapping: a window dragged to the left or right edge of a display takes
//! that half of it, a quarter where the edge meets a corner, and all of it
//! at the top edge; a translucent shape shows where it goes before it is
//! let go. With Shift held, the zones of the configuration are the places
//! instead. A window that was snapped takes its size from before again
//! when it is dragged away and let go elsewhere.
//!
//! The keys do the same: the action `snap` with `left`, `right`, `up` and
//! `down` goes from a half to a quarter, to the whole display and back,
//! as Win and the arrows do in Windows, and `snap-display` sends the
//! window to the next display. `snap-layouts` (Win+Z) opens small layouts
//! of the display, and a click on a zone of one puts the window there;
//! the same layouts open when the pointer rests on the maximize button of
//! a window. After a window took a half, dragged there or with the keys,
//! or was put into a zone of a layout, the other windows are offered for
//! the rest of the display.
//!
//! Snapping by dragging is on by default only when Tessera is the shell.
//! While it is on, the snapping of Windows itself is switched off until
//! Tessera ends, because the two would place the same window; whether
//! Windows still snaps without Explorer has not been seen. Windows that
//! are tiled are left to the tiling.

mod assist;
mod layouts;
mod preview;

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    App, AsyncApp, Bounds, Context, DisplayId, Global, Render, Window, WindowHandle, point, px,
    size,
};
use serde::Deserialize;

use crate::actions::{Action, Actions};
use crate::config::KeyConfig;
use crate::module::{Module, WidgetRegistry};
use crate::platform::{
    AppWindowId, Display, MaximizeHover, MoveEvent, MoveWatch, NativeSurface, ScreenEdge,
    ScreenRect, WindowArranging, on_resizing_border, pointer_at, shell_mode, shift_held,
};
use crate::popup::{PopupAnchor, open_floating};
use crate::tiling::{Tiler, Toward};

use self::assist::AssistView;
use self::layouts::LayoutsView;
use self::preview::PreviewView;

/// The time between two looks at a window that is being dragged.
const FRAME: Duration = Duration::from_millis(16);

/// How near an edge of a display the pointer is to be for a window to
/// snap there, when the configuration says nothing, and the most it may
/// say, in physical pixels.
const DEFAULT_EDGE: u32 = 8;
const WIDEST_EDGE: u32 = 64;

/// How far from a corner along an edge the pointer may be for a window
/// to take a quarter, when the configuration says nothing, and the most.
const DEFAULT_CORNER: u32 = 96;
const WIDEST_CORNER: u32 = 600;

/// The most room the configuration may leave around snapped windows.
const WIDEST_GAP: u32 = 64;

/// How long the pointer rests on a maximize button before the layouts
/// open, when the configuration says nothing, and the range it may say.
const DEFAULT_HOVER_DELAY: u32 = 450;
const HOVER_DELAYS: (u32, u32) = (100, 3000);

/// How far a window has to be dragged to count as moved, in physical
/// pixels: a click on the title bar moves it by nothing.
const MOVED: i32 = 4;

/// How much the size of a window may change while it is dragged before
/// the drag counts as resizing it, while the window has not said whether
/// the drag began on its border.
const RESIZED: i32 = 2;

/// How far from where it was placed a snapped window may be and still be
/// where it was snapped: some programs round their size.
const STILL_THERE: i32 = 24;

/// The six layouts of the snap layouts, as Windows 11 has them: halves,
/// thirds, two thirds and one, quarters, a half and two quarters, and a
/// wide middle between two narrow columns.
const LAYOUTS: [&[Zone]; 6] = [
    &[Zone::new(0., 0., 0.5, 1.), Zone::new(0.5, 0., 0.5, 1.)],
    &[
        Zone::new(0., 0., 1. / 3., 1.),
        Zone::new(1. / 3., 0., 1. / 3., 1.),
        Zone::new(2. / 3., 0., 1. / 3., 1.),
    ],
    &[
        Zone::new(0., 0., 2. / 3., 1.),
        Zone::new(2. / 3., 0., 1. / 3., 1.),
    ],
    &[
        Zone::new(0., 0., 0.5, 0.5),
        Zone::new(0.5, 0., 0.5, 0.5),
        Zone::new(0., 0.5, 0.5, 0.5),
        Zone::new(0.5, 0.5, 0.5, 0.5),
    ],
    &[
        Zone::new(0., 0., 0.5, 1.),
        Zone::new(0.5, 0., 0.5, 0.5),
        Zone::new(0.5, 0.5, 0.5, 0.5),
    ],
    &[
        Zone::new(0., 0., 0.25, 1.),
        Zone::new(0.25, 0., 0.5, 1.),
        Zone::new(0.75, 0., 0.25, 1.),
    ],
];

/// A part of the work area of a display, in shares of its width and
/// height from its top left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zone {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

/// What a share may be beyond 1 and still count as reaching the edge.
const SLACK: f32 = 0.001;

impl Zone {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The zone `[x, y, width, height]` names, if it lies within the work
    /// area and is not empty.
    pub fn parse([x, y, width, height]: [f32; 4]) -> Option<Self> {
        let share = |value: f32| value.is_finite() && (0. ..=1.).contains(&value);
        let fits = [x, y, width, height].into_iter().all(share)
            && width > 0.
            && height > 0.
            && x + width <= 1. + SLACK
            && y + height <= 1. + SLACK;
        fits.then_some(Self::new(x, y, width, height))
    }

    /// What `rect` is of `work`, in shares.
    pub fn of(rect: ScreenRect, work: ScreenRect) -> Self {
        let width = (work.right - work.left).max(1) as f32;
        let height = (work.bottom - work.top).max(1) as f32;
        Self::new(
            (rect.left - work.left) as f32 / width,
            (rect.top - work.top) as f32 / height,
            (rect.right - rect.left) as f32 / width,
            (rect.bottom - rect.top) as f32 / height,
        )
    }

    /// The zone as the configuration writes it, in thousandths.
    pub fn written(self) -> [f64; 4] {
        let round = |value: f32| (f64::from(value) * 1000.).round() / 1000.;
        [
            round(self.x),
            round(self.y),
            round(self.width),
            round(self.height),
        ]
    }

    /// The zone on the screen, within `work`, with `gap` between it and
    /// the zones beside it and between it and the edges of `work`.
    pub fn rect(self, work: ScreenRect, gap: i32) -> ScreenRect {
        let (width, height) = (
            (work.right - work.left) as f32,
            (work.bottom - work.top) as f32,
        );
        let at = |start: i32, length: f32, share: f32| start + (length * share).round() as i32;
        // Half of the gap on each side of an edge between two zones makes
        // the whole gap between them.
        let (half, rest) = (gap / 2, gap - gap / 2);
        let inset = |share: f32, edge: f32| match (share - edge).abs() < SLACK {
            true => gap,
            false => half,
        };
        let far = |share: f32| match (share - 1.).abs() < SLACK {
            true => gap,
            false => rest,
        };
        let left = at(work.left, width, self.x) + inset(self.x, 0.);
        let top = at(work.top, height, self.y) + inset(self.y, 0.);
        let right = at(work.left, width, self.x + self.width) - far(self.x + self.width);
        let bottom = at(work.top, height, self.y + self.height) - far(self.y + self.height);
        ScreenRect {
            left,
            top,
            right: right.max(left + 1),
            bottom: bottom.max(top + 1),
        }
    }

    /// Whether `point` lies in the zone of `work`.
    fn holds(self, work: ScreenRect, (x, y): (i32, i32)) -> bool {
        let rect = self.rect(work, 0);
        (rect.left..rect.right).contains(&x) && (rect.top..rect.bottom).contains(&y)
    }
}

/// A place a window snaps to on a display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    LeftHalf,
    RightHalf,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Maximize,
}

impl Target {
    /// The target `name` is, as the action `snap` names it.
    fn parse(name: &str) -> Option<Self> {
        match name {
            "left-half" => Some(Self::LeftHalf),
            "right-half" => Some(Self::RightHalf),
            "top-left" => Some(Self::TopLeft),
            "top-right" => Some(Self::TopRight),
            "bottom-left" => Some(Self::BottomLeft),
            "bottom-right" => Some(Self::BottomRight),
            "maximize" => Some(Self::Maximize),
            _ => None,
        }
    }

    /// The part of the work area the target is.
    pub fn zone(self) -> Zone {
        match self {
            Self::LeftHalf => Zone::new(0., 0., 0.5, 1.),
            Self::RightHalf => Zone::new(0.5, 0., 0.5, 1.),
            Self::TopLeft => Zone::new(0., 0., 0.5, 0.5),
            Self::TopRight => Zone::new(0.5, 0., 0.5, 0.5),
            Self::BottomLeft => Zone::new(0., 0.5, 0.5, 0.5),
            Self::BottomRight => Zone::new(0.5, 0.5, 0.5, 0.5),
            Self::Maximize => Zone::new(0., 0., 1., 1.),
        }
    }

    /// The half that is left of the display once a window took this
    /// half; none for the other targets.
    fn partner(self) -> Option<Self> {
        match self {
            Self::LeftHalf => Some(Self::RightHalf),
            Self::RightHalf => Some(Self::LeftHalf),
            _ => None,
        }
    }

    /// The same target with left and right traded.
    fn mirrored(self) -> Self {
        match self {
            Self::LeftHalf => Self::RightHalf,
            Self::RightHalf => Self::LeftHalf,
            Self::TopLeft => Self::TopRight,
            Self::TopRight => Self::TopLeft,
            Self::BottomLeft => Self::BottomRight,
            Self::BottomRight => Self::BottomLeft,
            Self::Maximize => Self::Maximize,
        }
    }
}

/// Where a window dragged with the pointer at `pointer` snaps on the
/// display `frame`: at its left or right edge, within `edge` pixels, the
/// half on that side, or the quarter where the pointer is within `corner`
/// pixels of the top or the bottom; at the top edge the whole display, or
/// the quarter near a corner; at the bottom edge a quarter near a corner
/// only.
pub fn target_at((x, y): (i32, i32), frame: ScreenRect, edge: i32, corner: i32) -> Option<Target> {
    let inside = (frame.left..frame.right).contains(&x) && (frame.top..frame.bottom).contains(&y);
    if !inside {
        return None;
    }
    let (left, right) = (x < frame.left + edge, x >= frame.right - edge);
    let (top, bottom) = (y < frame.top + edge, y >= frame.bottom - edge);
    let (near_top, near_bottom) = (y < frame.top + corner, y >= frame.bottom - corner);
    let (near_left, near_right) = (x < frame.left + corner, x >= frame.right - corner);
    let side = |left_side: bool| match (near_top, near_bottom, left_side) {
        (true, _, true) => Target::TopLeft,
        (true, _, false) => Target::TopRight,
        (_, true, true) => Target::BottomLeft,
        (_, true, false) => Target::BottomRight,
        (false, false, true) => Target::LeftHalf,
        (false, false, false) => Target::RightHalf,
    };
    match () {
        () if left => Some(side(true)),
        () if right => Some(side(false)),
        () if top && near_left => Some(Target::TopLeft),
        () if top && near_right => Some(Target::TopRight),
        () if top => Some(Target::Maximize),
        () if bottom && near_left => Some(Target::BottomLeft),
        () if bottom && near_right => Some(Target::BottomRight),
        () => None,
    }
}

/// How a window is placed now, as far as the keys of snapping go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    /// Where the user put it.
    Restored,
    Snapped(Target),
    Maximized,
}

/// What a key of snapping does to a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    To(Target),
    Restore,
    Minimize,
    /// To `Target` on the next display toward the side.
    Across(Toward, Target),
    Stay,
}

/// What the key toward `toward` does to a window placed as `placed`, as
/// Win and the arrows do in Windows: left and right go to a half, from a
/// half back to where the window was, and from a half or a quarter at the
/// edge on to the next display; up goes from a half to its top quarter
/// and from there to the whole display; down takes these steps back, and
/// minimizes a window that is where the user put it.
pub fn quick(placed: Placed, toward: Toward) -> Step {
    use Target::*;
    match (toward, placed) {
        (Toward::Left | Toward::Right, Placed::Restored | Placed::Maximized) => {
            Step::To(match toward {
                Toward::Left => LeftHalf,
                _ => RightHalf,
            })
        }
        (Toward::Left, Placed::Snapped(target)) => match target {
            RightHalf => Step::Restore,
            TopRight | BottomRight => Step::To(target.mirrored()),
            LeftHalf | TopLeft | BottomLeft => Step::Across(Toward::Left, target.mirrored()),
            Maximize => Step::To(LeftHalf),
        },
        (Toward::Right, Placed::Snapped(target)) => match target {
            LeftHalf => Step::Restore,
            TopLeft | BottomLeft => Step::To(target.mirrored()),
            RightHalf | TopRight | BottomRight => Step::Across(Toward::Right, target.mirrored()),
            Maximize => Step::To(RightHalf),
        },
        (Toward::Up, Placed::Maximized) => Step::Stay,
        (Toward::Up, Placed::Restored) => Step::To(Maximize),
        (Toward::Up, Placed::Snapped(target)) => Step::To(match target {
            LeftHalf => TopLeft,
            RightHalf => TopRight,
            BottomLeft => LeftHalf,
            BottomRight => RightHalf,
            TopLeft | TopRight | Maximize => Maximize,
        }),
        (Toward::Down, Placed::Maximized) => Step::Restore,
        (Toward::Down, Placed::Restored) => Step::Minimize,
        (Toward::Down, Placed::Snapped(target)) => match target {
            LeftHalf => Step::To(BottomLeft),
            RightHalf => Step::To(BottomRight),
            TopLeft => Step::To(LeftHalf),
            TopRight => Step::To(RightHalf),
            BottomLeft | BottomRight => Step::Minimize,
            Maximize => Step::Restore,
        },
    }
}

/// Where `frame`, which lies in the work area `from`, goes in the work
/// area `to`: at the same share of the way across and down, and as large
/// a share of it, so that it fits.
pub fn carried(frame: ScreenRect, from: ScreenRect, to: ScreenRect) -> ScreenRect {
    let zone = Zone::of(frame, from);
    let fitted = Zone::new(
        zone.x.clamp(0., 1.),
        zone.y.clamp(0., 1.),
        zone.width.clamp(0.05, 1.),
        zone.height.clamp(0.05, 1.),
    );
    let fitted = Zone {
        x: fitted.x.min(1. - fitted.width),
        y: fitted.y.min(1. - fitted.height),
        ..fitted
    };
    fitted.rect(to, 0)
}

/// Where a snapped window of `frame` goes when it was dragged away by the
/// pointer, now at `pointer`, and takes the size of `restored` again: the
/// pointer stays at the same share of the way across it, its top stays,
/// and it is kept within `work`.
pub fn unsnapped(
    restored: ScreenRect,
    frame: ScreenRect,
    (x, _): (i32, i32),
    work: ScreenRect,
) -> ScreenRect {
    let (width, height) = (
        restored.right - restored.left,
        restored.bottom - restored.top,
    );
    let across = (x - frame.left) as f32 / (frame.right - frame.left).max(1) as f32;
    let left = x - (width as f32 * across.clamp(0., 1.)).round() as i32;
    let left = left.clamp(work.left, (work.right - width).max(work.left));
    let top = frame
        .top
        .clamp(work.top, (work.bottom - height).max(work.top));
    ScreenRect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// Whether `a` and `b` are the same within `slack` pixels on every side.
fn near(a: ScreenRect, b: ScreenRect, slack: i32) -> bool {
    (a.left - b.left).abs() <= slack
        && (a.top - b.top).abs() <= slack
        && (a.right - b.right).abs() <= slack
        && (a.bottom - b.bottom).abs() <= slack
}

/// The width and height of `rect`.
fn extent(rect: ScreenRect) -> (i32, i32) {
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// What `[snap]` of the configuration says.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapConfig {
    /// Whether windows dragged to an edge snap there; by default only when
    /// Tessera is the shell.
    enabled: Option<bool>,
    /// How near an edge the pointer is to be, in physical pixels.
    edge: Option<u32>,
    /// How near a corner along an edge the pointer is to be for a quarter.
    corner: Option<u32>,
    /// The room left around snapped windows, in physical pixels.
    gap: Option<u32>,
    /// Whether the other windows are offered for the rest of the display.
    assist: Option<bool>,
    /// Whether the snap layouts open when the pointer rests on the
    /// maximize button of a window; by default when snapping is on.
    hover: Option<bool>,
    /// How long it rests there first, in milliseconds.
    hover_delay: Option<u32>,
    /// The zones a window snaps to when it is dragged with Shift held.
    #[serde(default, rename = "zone")]
    zones: Vec<ZoneConfig>,
    /// The layouts of the snap layouts, in place of the six of Windows.
    #[serde(default, rename = "layout")]
    layouts: Vec<LayoutConfig>,
}

/// A `[[snap.zone]]`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneConfig {
    /// The display, counted from 1 from the left; every display without.
    display: Option<usize>,
    /// `[x, y, width, height]` in shares of the work area.
    rect: [f32; 4],
}

/// A `[[snap.layout]]`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutConfig {
    /// Its zones, each `[x, y, width, height]` in shares of the work area.
    zones: Vec<[f32; 4]>,
}

/// What the configuration comes to, checked.
#[derive(Clone, Debug, PartialEq)]
struct Settings {
    dragging: bool,
    edge: i32,
    corner: i32,
    gap: i32,
    assist: bool,
    hover: Option<Duration>,
    /// The zones of Shift and the display each is for, counted from 1.
    zones: Vec<(Option<usize>, Zone)>,
    layouts: Vec<Vec<Zone>>,
}

impl Default for Settings {
    fn default() -> Self {
        SnapConfig::default().settings(false)
    }
}

/// `value`, or `default` without one; one beyond `range` is reported as
/// `name` and replaced by `default`.
fn within(name: &str, value: Option<u32>, (least, most): (u32, u32), default: u32) -> u32 {
    match value {
        None => default,
        Some(value) if (least..=most).contains(&value) => value,
        Some(value) => {
            report!("`snap.{name}` is {value}, not from {least} to {most}, so it is {default}");
            default
        }
    }
}

impl SnapConfig {
    /// What the configuration comes to where Tessera is the shell or not,
    /// as `shell` says. Zones and layouts that cannot be used are
    /// reported and left out.
    fn settings(&self, shell: bool) -> Settings {
        let dragging = self.enabled.unwrap_or(shell);
        let edge = within("edge", self.edge, (1, WIDEST_EDGE), DEFAULT_EDGE);
        let corner = within(
            "corner",
            self.corner,
            (edge, WIDEST_CORNER),
            DEFAULT_CORNER.max(edge),
        );
        let gap = within("gap", self.gap, (0, WIDEST_GAP), 0);
        let delay = within(
            "hover_delay",
            self.hover_delay,
            HOVER_DELAYS,
            DEFAULT_HOVER_DELAY,
        );
        let zones = { self.zones.iter() }
            .filter_map(|zone| match Zone::parse(zone.rect) {
                Some(parsed) => Some((zone.display, parsed)),
                None => {
                    report!("a `[[snap.zone]]` is left out: its `rect` is not within the display");
                    None
                }
            })
            .collect();
        let layouts: Vec<Vec<Zone>> = { self.layouts.iter() }
            .filter_map(|layout| {
                let zones: Option<Vec<Zone>> = layout.zones.iter().copied().map(Zone::parse).collect();
                match zones {
                    Some(zones) if !zones.is_empty() => Some(zones),
                    _ => {
                        report!("a `[[snap.layout]]` is left out: it has no zones, or one is not within the display");
                        None
                    }
                }
            })
            .collect();
        let layouts = match layouts.is_empty() {
            true => LAYOUTS.iter().map(|zones| zones.to_vec()).collect(),
            false => layouts,
        };
        Settings {
            dragging,
            edge: edge as i32,
            corner: corner as i32,
            gap: gap as i32,
            assist: self.assist.unwrap_or(true),
            hover: self
                .hover
                .unwrap_or(dragging)
                .then(|| Duration::from_millis(u64::from(delay))),
            zones,
            layouts,
        }
    }
}

/// Where a window was snapped to.
#[derive(Clone, Copy, Debug)]
struct Snapped {
    /// The target it was snapped to, if it was one of them.
    target: Option<Target>,
    /// Where it was sent.
    placed: ScreenRect,
    /// Where it was before it was first snapped.
    restored: ScreenRect,
}

/// Where a window that is being dragged would snap.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Aim {
    display: Display,
    zone: Zone,
    target: Option<Target>,
}

impl Aim {
    fn rect(self, gap: i32) -> ScreenRect {
        self.zone.rect(self.display.work, gap)
    }
}

/// Whether keys of Tessera's own snap the window in front: those the
/// actions of snapping bring where `windows_keys` asks for the keys of
/// Windows, by default when Tessera is the shell, or a `[[key]]` with the
/// action `snap`.
fn keys_bound(windows_keys: Option<bool>, keys: &[KeyConfig]) -> bool {
    windows_keys.unwrap_or_else(shell_mode) || { keys.iter() }
        .any(|key| key.action.as_deref() == Some("snap"))
}

/// A window that is being dragged.
struct Drag {
    id: AppWindowId,
    /// What was seen of the window when the drag began.
    start: ScreenRect,
    /// Where the pointer was then.
    pointer: (i32, i32),
    /// Where the window goes back to when it is restored, as it was
    /// before the drag.
    restored: ScreenRect,
    /// Whether the window was snapped when the drag began.
    snapped: bool,
    /// Whether the drag began on the border that resizes the window, as
    /// the window says once it answered.
    resizing: Option<bool>,
    /// Whether the size changed, which counts as resizing while the window
    /// has not answered. A window also changes its size as it is dragged
    /// onto a display of another scale, or out of being maximized.
    resized: bool,
    /// Whether it was moved by more than a click does.
    moved: bool,
    aim: Option<Aim>,
    /// Counts the drags; only the loop of the latest goes on.
    task: u64,
}

/// Snaps windows.
#[derive(Default)]
pub struct Snapper {
    config: SnapConfig,
    /// Whether keys of Tessera's own snap the window in front, so that
    /// Win with the arrows still snaps once Windows no longer does.
    keys_bound: bool,
    settings: Settings,
    snapped: HashMap<AppWindowId, Snapped>,
    drag: Option<Drag>,
    drags: u64,
    preview: Option<WindowHandle<PreviewView>>,
    /// What the user chose for the snapping of Windows, while Tessera has
    /// it switched off.
    arranging: Option<bool>,
    layouts: Option<WindowHandle<LayoutsView>>,
    assist: Option<WindowHandle<AssistView>>,
    /// The window the assistant was opened beside, which the keys of
    /// snapping are for while the assistant is in front.
    assisted: Option<AppWindowId>,
}

impl Global for Snapper {}

impl Snapper {
    /// Snaps as `config` says from now on, and gives Windows back its own
    /// snapping when the shell quits. `windows_keys` and `keys` are what
    /// the configuration says of the keys of Windows and of `[[key]]`
    /// entries.
    pub fn start(
        config: &SnapConfig,
        windows_keys: Option<bool>,
        keys: &[KeyConfig],
        cx: &mut App,
    ) {
        cx.set_global(Self::default());
        cx.on_app_quit(|cx| {
            cx.global_mut::<Self>().give_arranging_back();
            async {}
        })
        .detach();
        Self::watch_moves(cx);
        Self::watch_hover(cx);
        Self::apply(config, keys_bound(windows_keys, keys), cx);
    }

    /// Takes over a configuration that was read again, if it says
    /// something else about snapping or the keys that snap.
    pub fn configure(
        config: &SnapConfig,
        windows_keys: Option<bool>,
        keys: &[KeyConfig],
        cx: &mut App,
    ) {
        let bound = keys_bound(windows_keys, keys);
        let this = cx.global::<Self>();
        if this.config != *config || this.keys_bound != bound {
            Self::apply(config, bound, cx);
        }
    }

    fn apply(config: &SnapConfig, keys_bound: bool, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        this.config = config.clone();
        this.keys_bound = keys_bound;
        this.settings = config.settings(shell_mode());
        if this.settings.dragging {
            this.take_arranging();
            // Switching the snapping of Windows off also switches off its
            // keys, which only Tessera's own then stand in for.
            if !keys_bound && !WindowArranging::is_on() {
                report!(
                    "Win with the arrows and Win+Z do nothing while Tessera snaps: `windows_keys = true` at the top of config.toml binds Tessera's, as does a `[[key]]` with `action = \"snap\"`"
                );
            }
        } else {
            this.give_arranging_back();
        }
        MaximizeHover::watch(this.settings.hover);
    }

    /// Switches the snapping of Windows off while Tessera snaps, and
    /// remembers what the user chose for it.
    fn take_arranging(&mut self) {
        if self.arranging.is_some() {
            return;
        }
        let chosen = WindowArranging::chosen();
        if WindowArranging::is_on() && !WindowArranging::set(false) {
            report!("Windows keeps snapping windows itself, so a window may be snapped twice");
        }
        self.arranging = Some(chosen);
    }

    /// Gives Windows its own snapping back as the user chose it, also when
    /// a Tessera before this one ended without doing so.
    fn give_arranging_back(&mut self) {
        let chosen = self
            .arranging
            .take()
            .unwrap_or_else(WindowArranging::chosen);
        if chosen && !WindowArranging::is_on() && !WindowArranging::set(true) {
            report!("Windows does not snap windows again until the next sign in");
        }
    }

    fn watch_moves(cx: &mut App) {
        let events = MoveWatch::events();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(event) = events.recv().await {
                let handled = cx.update(|cx| match event {
                    MoveEvent::Start(id) => Self::begin(id, cx),
                    MoveEvent::End(id) => Self::finish(id, cx),
                });
                if handled.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn watch_hover(cx: &mut App) {
        let hovers = MaximizeHover::watch(None);
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok((id, pointer)) = hovers.recv().await {
                let opened = cx.update(|cx| {
                    let this = cx.global::<Self>();
                    let (layouts, hovering) = (this.layouts, this.settings.hover.is_some());
                    let hovering = hovering && !Tiler::manages(id, cx);
                    let open = layouts.is_some_and(|layouts| {
                        let open = layouts.update(cx, |view, _, _| !view.is_closing());
                        open.unwrap_or(false)
                    });
                    if hovering && !open {
                        Self::open_layouts(id, pointer, true, cx);
                    }
                });
                if opened.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Begins to follow the window `id`, which the user begins to drag.
    fn begin(id: AppWindowId, cx: &mut App) {
        if Tiler::manages(id, cx) {
            return;
        }
        let (Some(start), Some(pointer)) = (id.frame(), pointer_at()) else {
            return;
        };
        let this = cx.global_mut::<Self>();
        this.drags += 1;
        let task = this.drags;
        let snapped = { this.snapped.get(&id) }
            .is_some_and(|snapped| near(start, snapped.placed, STILL_THERE));
        let restored = this.restored_of(id).unwrap_or(start);
        this.drag = Some(Drag {
            id,
            start,
            pointer,
            restored,
            snapped,
            resizing: None,
            resized: false,
            moved: false,
            aim: None,
            task,
        });
        // The window is asked what the pointer is on away from here, as it
        // may take a moment to answer.
        cx.spawn(async move |cx: &mut AsyncApp| {
            let asked = cx
                .background_executor()
                .spawn(async move { on_resizing_border(id, pointer) });
            let resizing = asked.await;
            cx.update(|cx| {
                let drag = cx.global_mut::<Self>().drag.as_mut();
                if let Some(drag) = drag.filter(|drag| drag.task == task) {
                    drag.resizing = resizing;
                }
            })
            .ok();
        })
        .detach();
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(FRAME).await;
                if !matches!(cx.update(|cx| Self::follow(task, cx)), Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Looks at the window that is being dragged once more, and shows
    /// where it would snap. Whether the drag goes on is returned.
    fn follow(task: u64, cx: &mut App) -> bool {
        let this = cx.global_mut::<Self>();
        let Some(drag) = this.drag.as_mut().filter(|drag| drag.task == task) else {
            return false;
        };
        let (Some(frame), Some(pointer)) = (drag.id.frame(), pointer_at()) else {
            let id = drag.id;
            Self::finish(id, cx);
            return false;
        };
        let (width, height) = extent(frame);
        let (started_width, started_height) = extent(drag.start);
        drag.resized |=
            (width - started_width).abs() > RESIZED || (height - started_height).abs() > RESIZED;
        drag.moved |= (pointer.0 - drag.pointer.0).abs() > MOVED
            || (pointer.1 - drag.pointer.1).abs() > MOVED;
        let settings = &this.settings;
        let resizing = drag.resizing.unwrap_or(drag.resized);
        let aim = (settings.dragging && drag.moved && !resizing)
            .then(|| aim_at(pointer, settings))
            .flatten();
        if aim == drag.aim {
            return true;
        }
        drag.aim = aim;
        let gap = settings.gap;
        Self::show_preview(aim.map(|aim| (aim.display, aim.rect(gap))), cx);
        true
    }

    /// Shows where a window would snap, or that it would not.
    fn show_preview(aim: Option<(Display, ScreenRect)>, cx: &mut App) {
        let preview = cx.global::<Self>().preview;
        let shown =
            preview.map(|preview| preview.update(cx, |view, window, cx| view.aim(aim, window, cx)));
        if matches!(shown, Some(Ok(()))) || aim.is_none() {
            return;
        }
        let Some((display, rect)) = aim else {
            return;
        };
        match PreviewView::open(display, rect, cx) {
            Ok(preview) => cx.global_mut::<Self>().preview = Some(preview),
            Err(error) => report!("where a window snaps is not shown: {error:#}"),
        }
    }

    /// Snaps the window `id`, which the user let go, where it was aimed,
    /// or gives it its size from before it was snapped when it was dragged
    /// away from where it was snapped.
    fn finish(id: AppWindowId, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        let Some(drag) = this.drag.take_if(|drag| drag.id == id) else {
            return;
        };
        if let Some(preview) = this.preview.take() {
            preview.update(cx, |view, _, cx| view.close(cx)).ok();
        }
        let this = cx.global_mut::<Self>();
        if drag.resizing.unwrap_or(drag.resized) {
            // A window resized by hand is where the user wants it.
            this.snapped.remove(&id);
            return;
        }
        if let Some(aim) = drag.aim {
            Self::snap_to(id, aim.display, aim.zone, aim.target, drag.restored, cx);
            if let Some(partner) = aim.target.and_then(Target::partner) {
                Self::assist(id, aim.display, vec![partner.zone()], cx);
            }
            return;
        }
        if !drag.snapped || !drag.moved {
            return;
        }
        this.snapped.remove(&id);
        let (Some(frame), Some(pointer)) = (id.frame(), pointer_at()) else {
            return;
        };
        let Some(display) = Display::at(pointer) else {
            return;
        };
        id.place(unsnapped(drag.restored, frame, pointer, display.work));
    }

    /// Where the window `id` was before it was snapped, if it is snapped;
    /// where it goes back to when it is maximized.
    fn restored_of(&self, id: AppWindowId) -> Option<ScreenRect> {
        match self.placed(id) {
            Placed::Snapped(_) => self.snapped.get(&id).map(|snapped| snapped.restored),
            Placed::Maximized => self
                .snapped
                .get(&id)
                .map(|snapped| snapped.restored)
                .or_else(|| id.restored_frame()),
            Placed::Restored => {
                let snapped = self.snapped.get(&id);
                let there = snapped.zip(id.frame());
                match there {
                    Some((snapped, frame)) if near(frame, snapped.placed, STILL_THERE) => {
                        Some(snapped.restored)
                    }
                    _ => id.frame(),
                }
            }
        }
    }

    /// How the window `id` is placed now.
    fn placed(&self, id: AppWindowId) -> Placed {
        if id.is_maximized() {
            return Placed::Maximized;
        }
        let snapped = self.snapped.get(&id);
        match snapped.zip(id.frame()) {
            Some((snapped, frame)) if near(frame, snapped.placed, STILL_THERE) => {
                snapped.target.map_or(Placed::Restored, Placed::Snapped)
            }
            _ => Placed::Restored,
        }
    }

    /// Puts the window `id` into `zone` of `display`, or maximizes it
    /// there for [`Target::Maximize`], and remembers that it goes back to
    /// `restored`.
    fn snap_to(
        id: AppWindowId,
        display: Display,
        zone: Zone,
        target: Option<Target>,
        restored: ScreenRect,
        cx: &mut App,
    ) {
        let this = cx.global_mut::<Self>();
        if target == Some(Target::Maximize) {
            this.snapped.remove(&id);
            // Restoring it later brings back the size from before, on the
            // display it is maximized on.
            let center = (
                (restored.left + restored.right) / 2,
                (restored.top + restored.bottom) / 2,
            );
            let normal = match Display::at(center) {
                Some(there) if there.key != display.key => {
                    carried(restored, there.work, display.work)
                }
                _ => restored,
            };
            id.place_restored(normal, true);
            return;
        }
        let rect = zone.rect(display.work, this.settings.gap);
        Self::put(id, rect);
        this.snapped.insert(
            id,
            Snapped {
                target,
                placed: rect,
                restored,
            },
        );
    }

    /// Has what is seen of the window `id` cover `rect`. A maximized or
    /// minimized window is restored there in one step: until it is
    /// restored, its border is not as wide as it will be.
    fn put(id: AppWindowId, rect: ScreenRect) {
        if id.is_maximized() || id.is_minimized() {
            id.place_restored(rect, false);
        } else {
            id.place(rect);
        }
    }

    /// Offers the other windows for `zones` of `display`, once `placed`
    /// took its part of it, where the configuration asks for that.
    fn assist(placed: AppWindowId, display: Display, zones: Vec<Zone>, cx: &mut App) {
        let this = cx.global::<Self>();
        if !this.settings.assist || zones.is_empty() {
            return;
        }
        let gap = this.settings.gap;
        let rects = zones
            .into_iter()
            .map(|zone| zone.rect(display.work, gap))
            .collect();
        if let Some(open) = cx.global_mut::<Self>().assist.take() {
            open.update(cx, |view, _, cx| view.close(cx)).ok();
        }
        match AssistView::offer(placed, display, rects, cx) {
            Ok(assist) => {
                let this = cx.global_mut::<Self>();
                this.assisted = assist.is_some().then_some(placed);
                this.assist = assist;
            }
            Err(error) => report!("the other windows are not offered: {error:#}"),
        }
    }

    /// Puts the window `id` where `rect` is, as the window the assistant
    /// offered was chosen for it.
    fn place_chosen(id: AppWindowId, rect: ScreenRect, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        let Some(restored) = this.restored_of(id) else {
            return;
        };
        Self::put(id, rect);
        id.raise();
        this.snapped.insert(
            id,
            Snapped {
                target: None,
                placed: rect,
                restored,
            },
        );
    }

    /// The window the keys of snapping are for: the one in front, unless
    /// it is one of Tessera or the tiling has it. While the assistant is
    /// in front, as it is right after a window was snapped with the keys,
    /// they are for the window it was opened beside, and it closes.
    fn front(cx: &mut App) -> Option<AppWindowId> {
        let mut front = AppWindowId::foreground()?;
        if front.process() == std::process::id() {
            let this = cx.global_mut::<Self>();
            let (open, assisted) = (this.assist.take(), this.assisted.take());
            let showing = open.is_some_and(|open| {
                let closed = open.update(cx, |view, _, cx| {
                    let showing = !view.is_closing();
                    view.close(cx);
                    showing
                });
                closed.unwrap_or(false)
            });
            front = assisted.filter(|_| showing)?;
        }
        (front.exists() && !Tiler::manages(front, cx)).then_some(front)
    }

    /// Does the action `snap` with `arg` to the window in front.
    fn snap_front(arg: &str, cx: &mut App) {
        if let Some(id) = Self::front(cx) {
            Self::snap_window(id, arg, cx);
        }
    }

    /// Snaps the window `id` as `arg`, one the action `snap` takes, says.
    pub fn snap_window(id: AppWindowId, arg: &str, cx: &mut App) {
        let Some(display) = Display::of_window(id) else {
            return;
        };
        let this = cx.global::<Self>();
        let placed = this.placed(id);
        let step = match (Toward::parse(arg), Target::parse(arg), arg) {
            (Some(toward), _, _) => quick(placed, toward),
            (_, Some(target), _) => Step::To(target),
            (_, _, "restore") => Step::Restore,
            (_, _, "minimize") => Step::Minimize,
            _ => return,
        };
        let restored = this.restored_of(id);
        match step {
            Step::Stay => {}
            Step::Minimize => id.minimize(),
            Step::Restore => {
                let snapped = cx.global_mut::<Self>().snapped.remove(&id);
                match (placed, snapped) {
                    (Placed::Maximized, _) => id.restore(),
                    (_, Some(snapped)) => {
                        id.place(snapped.restored);
                    }
                    _ => {}
                }
            }
            Step::To(target) => {
                let restored = restored.or_else(|| id.frame());
                if let Some(restored) = restored {
                    Self::snap_to(id, display, target.zone(), Some(target), restored, cx);
                    if let Some(partner) = target.partner() {
                        Self::assist(id, display, vec![partner.zone()], cx);
                    }
                }
            }
            Step::Across(toward, target) => {
                let displays = Display::all();
                let at = displays.iter().position(|there| there.key == display.key);
                let beside = at.and_then(|at| match toward {
                    Toward::Left => at.checked_sub(1),
                    _ => Some(at + 1),
                });
                let next = beside.and_then(|beside| displays.get(beside)).copied();
                if let (Some(next), Some(restored)) = (next, restored) {
                    let restored = carried(restored, display.work, next.work);
                    Self::snap_to(id, next, target.zone(), Some(target), restored, cx);
                }
            }
        }
    }

    /// Sends the window in front to the next display, or the one before
    /// it with `back`.
    fn front_to_display(back: bool, cx: &mut App) {
        if let Some(id) = Self::front(cx) {
            Self::move_to_display(id, back, cx);
        }
    }

    /// Sends the window `id` to the next display, from the left to the
    /// right and round, or the one before with `back`: at the same share
    /// of the way across and down, snapped to the same place, or maximized
    /// there.
    pub fn move_to_display(id: AppWindowId, back: bool, cx: &mut App) {
        let displays = Display::all();
        let Some(from) = Display::of_window(id) else {
            return;
        };
        let Some(at) = displays.iter().position(|display| display.key == from.key) else {
            return;
        };
        if displays.len() < 2 {
            return;
        }
        let count = displays.len();
        let to = displays[if back {
            (at + count - 1) % count
        } else {
            (at + 1) % count
        }];
        let this = cx.global::<Self>();
        let placed = this.placed(id);
        let restored = this.restored_of(id);
        let snapped = this.snapped.get(&id).copied();
        match placed {
            Placed::Maximized => {
                if let Some(restored) = restored {
                    id.place_restored(carried(restored, from.work, to.work), true);
                }
            }
            Placed::Snapped(target) => {
                if let Some(restored) = restored {
                    let restored = carried(restored, from.work, to.work);
                    Self::snap_to(id, to, target.zone(), Some(target), restored, cx);
                }
            }
            Placed::Restored => {
                let Some(frame) = id.frame() else {
                    return;
                };
                let still = snapped.filter(|snapped| near(frame, snapped.placed, STILL_THERE));
                match still {
                    // A window put into a zone of a layout takes the same
                    // zone there.
                    Some(snapped) => {
                        let zone = Zone::of(snapped.placed, from.work);
                        let restored = carried(snapped.restored, from.work, to.work);
                        Self::snap_to(id, to, zone, None, restored, cx);
                    }
                    None => {
                        id.place(carried(frame, from.work, to.work));
                    }
                }
            }
        }
    }

    /// Opens the snap layouts for the window in front.
    fn layouts_for_front(cx: &mut App) {
        let Some(id) = Self::front(cx) else {
            return;
        };
        let (Some(frame), Some(display)) = (id.frame(), Display::of_window(id)) else {
            return;
        };
        // Under the right end of the title bar, where its buttons are.
        let reach = (layouts::REACH * display.scale) as i32;
        let x = (frame.right - reach).max(frame.left);
        let y = frame.top.max(display.work.top) + (layouts::BELOW * display.scale) as i32;
        Self::open_layouts(id, (x, y), false, cx);
    }

    /// Opens the snap layouts for the window `id` at `point`; `hovered`
    /// says that the pointer resting on its maximize button opened them,
    /// so that they close once it moves away.
    fn open_layouts(id: AppWindowId, point: (i32, i32), hovered: bool, cx: &mut App) {
        if let Some(open) = cx.global_mut::<Self>().layouts.take() {
            open.update(cx, |view, _, cx| view.close(cx)).ok();
        }
        let Some(display) = Display::at(point) else {
            return;
        };
        let layouts = cx.global::<Self>().settings.layouts.clone();
        match LayoutsView::open(id, display, point, layouts, hovered, cx) {
            Ok(view) => cx.global_mut::<Self>().layouts = Some(view),
            Err(error) => report!("the snap layouts did not open: {error:#}"),
        }
    }

    /// Puts the window `id` into the zone at `chosen` of `zones` on
    /// `display`, and offers the other windows for the rest.
    fn place_in_layout(
        id: AppWindowId,
        display: Display,
        zones: Vec<Zone>,
        chosen: usize,
        cx: &mut App,
    ) {
        let Some(&zone) = zones.get(chosen) else {
            return;
        };
        let restored = cx.global::<Self>().restored_of(id).or_else(|| id.frame());
        let Some(restored) = restored else {
            return;
        };
        Self::snap_to(id, display, zone, None, restored, cx);
        id.activate();
        let rest = { zones.iter().enumerate() }
            .filter(|&(index, _)| index != chosen)
            .map(|(_, &zone)| zone)
            .collect();
        Self::assist(id, display, rest, cx);
    }
}

/// Where a window dragged with the pointer at `pointer` would snap: in
/// the zone of the display under the pointer with Shift held, where the
/// configuration has zones for it, and at an edge otherwise.
fn aim_at(pointer: (i32, i32), settings: &Settings) -> Option<Aim> {
    let display = Display::at(pointer)?;
    if shift_held() && !settings.zones.is_empty() {
        let number = Display::all()
            .iter()
            .position(|there| there.key == display.key)
            .map(|index| index + 1);
        let zones: Vec<Zone> = { settings.zones.iter() }
            .filter(|(only, _)| only.is_none() || *only == number)
            .map(|&(_, zone)| zone)
            .collect();
        if !zones.is_empty() {
            let zone = zones
                .into_iter()
                .find(|zone| zone.holds(display.work, pointer))?;
            return Some(Aim {
                display,
                zone,
                target: None,
            });
        }
    }
    let target = target_at(pointer, display.frame, settings.edge, settings.corner)?;
    Some(Aim {
        display,
        zone: target.zone(),
        target: Some(target),
    })
}

/// The display of GPUI that is `display`.
fn gpui_display(display: &Display, cx: &App) -> Option<DisplayId> {
    let (x, y) = (
        display.frame.left as f32 / display.scale,
        display.frame.top as f32 / display.scale,
    );
    let off = |bounds: Bounds<gpui::Pixels>| {
        (bounds.origin.x / px(1.) - x).abs() + (bounds.origin.y / px(1.) - y).abs()
    };
    let displays = cx.displays();
    let nearest = displays
        .iter()
        .min_by(|first, second| off(first.bounds()).total_cmp(&off(second.bounds())));
    nearest.map(|display| display.id())
}

/// Where a floating window hangs from `point` of `display`.
pub fn anchor_at(display: &Display, (x, y): (i32, i32), cx: &App) -> PopupAnchor {
    PopupAnchor {
        surface: ScreenRect {
            left: x,
            top: y,
            right: x,
            bottom: y,
        },
        edge: ScreenEdge::Top,
        x,
        screen: display.work,
        scale: display.scale,
        display_id: gpui_display(display, cx),
        display: Some(display.key),
    }
}

/// What a window of snapping is to the other windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Drawn over everything, never in front, the pointer going through.
    Overlay,
    /// Above the windows, taking the keyboard.
    Popup,
    /// Above the windows, leaving the keyboard where it is.
    Surface,
}

/// Opens a floating window of `kind` over `frame` of `display`, whose root
/// view `build` makes.
fn open_at<V: Render + 'static>(
    frame: ScreenRect,
    display: &Display,
    kind: Kind,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V,
) -> anyhow::Result<WindowHandle<V>> {
    let scale = display.scale;
    let (width, height) = extent(frame);
    let rough = Bounds {
        origin: point(px(frame.left as f32 / scale), px(frame.top as f32 / scale)),
        size: size(px(width as f32 / scale), px(height as f32 / scale)),
    };
    let display_id = gpui_display(display, cx);
    let focus = kind == Kind::Popup;
    open_floating(rough, display_id, focus, cx, move |window, cx| {
        match NativeSurface::of(window) {
            // The window learns its size only if it is given while nothing
            // of the user interface is being worked on, as it is right
            // after this.
            Ok(native) => cx
                .spawn(async move |_, _| {
                    if let Err(error) = settle(native, frame, kind) {
                        report!("a window of snapping is not where it belongs: {error:#}");
                    }
                })
                .detach(),
            Err(error) => report!("a window of snapping is not where it belongs: {error:#}"),
        }
        build(window, cx)
    })
}

/// Gives the window `native` of `kind` its styles, places it over `frame`
/// and shows it.
fn settle(native: NativeSurface, frame: ScreenRect, kind: Kind) -> anyhow::Result<()> {
    match kind {
        Kind::Overlay => {
            native.adopt_as_shell_surface()?;
            native.place(frame)?;
            native.reveal(false);
            native.adopt_as_overlay()
        }
        Kind::Popup => {
            native.adopt_as_popup()?;
            native.place(frame)?;
            native.reveal(false);
            native.take_keyboard();
            Ok(())
        }
        Kind::Surface => {
            native.adopt_as_shell_surface()?;
            native.place(frame)?;
            native.reveal(false);
            Ok(())
        }
    }
}

/// Moves the floating window `window` over `frame`, once nothing of the
/// user interface is being worked on.
fn move_floating<V: 'static>(window: &Window, frame: ScreenRect, cx: &mut Context<V>) {
    match NativeSurface::of(window) {
        Ok(native) => cx
            .spawn(async move |_, _| {
                if let Err(error) = native.place(frame) {
                    report!("a window of snapping is not where it belongs: {error:#}");
                }
            })
            .detach(),
        Err(error) => report!("a window of snapping is not where it belongs: {error:#}"),
    }
}

/// The module that brings the actions of snapping and of the window menu.
pub struct SnapModule;

/// What the action `snap` takes.
const SNAP_ARGS: [&str; 13] = [
    "left",
    "right",
    "up",
    "down",
    "left-half",
    "right-half",
    "top-left",
    "top-right",
    "bottom-left",
    "bottom-right",
    "maximize",
    "restore",
    "minimize",
];

impl Module for SnapModule {
    fn name(&self) -> &'static str {
        "snap"
    }

    fn register(&self, _widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        Ok(())
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        // Windows keeps Win with the arrows and Win+Z to itself while
        // Explorer runs.
        let snap = Action::new(|arg, cx| Snapper::snap_front(arg.unwrap_or_default(), cx))
            .taking_reserved_keys()
            .checking(|arg| match arg {
                Some(arg) if SNAP_ARGS.contains(&arg) => Ok(()),
                _ => Err(format!("`snap` takes one of `{}`", SNAP_ARGS.join("`, `"))),
            })
            .by_default("win+left", Some("left"))
            .by_default("win+right", Some("right"))
            .by_default("win+up", Some("up"))
            .by_default("win+down", Some("down"));
        actions.register("snap", snap)?;
        let display = Action::new(|arg, cx| Snapper::front_to_display(arg == Some("previous"), cx))
            .taking_reserved_keys()
            .checking(|arg| match arg {
                None | Some("next" | "previous") => Ok(()),
                Some(_) => Err("`snap-display` takes `next`, `previous` or no `arg`".to_owned()),
            })
            .by_default("win+shift+right", Some("next"))
            .by_default("win+shift+left", Some("previous"));
        actions.register("snap-display", display)?;
        let layouts = Action::new(|_, cx| Snapper::layouts_for_front(cx))
            .taking_reserved_keys()
            .by_default("win+z", None);
        actions.register("snap-layouts", layouts)?;
        crate::window_menu::register(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    const SCREEN: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    #[test]
    fn the_edges_of_a_display_snap_to_halves_quarters_and_the_whole() {
        let at = |x, y| target_at((x, y), SCREEN, 8, 96);
        assert_eq!(at(0, 540), Some(Target::LeftHalf));
        assert_eq!(at(1919, 540), Some(Target::RightHalf));
        assert_eq!(at(960, 0), Some(Target::Maximize));
        assert_eq!(at(3, 20), Some(Target::TopLeft));
        assert_eq!(at(20, 3), Some(Target::TopLeft));
        assert_eq!(at(1915, 1070), Some(Target::BottomRight));
        assert_eq!(at(1900, 1079), Some(Target::BottomRight));
        assert_eq!(at(960, 1079), None);
        assert_eq!(at(960, 540), None);
        assert_eq!(at(-5, 540), None);
        // A display to the right of another one has edges of its own.
        let right = rect(1920, 0, 3840, 1080);
        assert_eq!(target_at((1921, 500), right, 8, 96), Some(Target::LeftHalf));
    }

    #[test]
    fn a_zone_is_a_share_of_the_work_area_with_the_gaps_between() {
        let work = rect(0, 40, 1920, 1080);
        assert_eq!(
            Target::LeftHalf.zone().rect(work, 0),
            rect(0, 40, 960, 1080)
        );
        assert_eq!(
            Target::BottomRight.zone().rect(work, 0),
            rect(960, 560, 1920, 1080)
        );
        let left = Target::LeftHalf.zone().rect(work, 10);
        let right = Target::RightHalf.zone().rect(work, 10);
        assert_eq!(left, rect(10, 50, 955, 1070));
        assert_eq!(right, rect(965, 50, 1910, 1070));
        assert_eq!(right.left - left.right, 10);
        let third = Zone::parse([1. / 3., 0., 1. / 3., 1.]).unwrap();
        assert_eq!(third.rect(work, 0), rect(640, 40, 1280, 1080));
        assert!(third.holds(work, (700, 500)));
        assert!(!third.holds(work, (100, 500)));
        assert_eq!(
            Zone::of(rect(960, 40, 1920, 560), work),
            Target::TopRight.zone()
        );
        assert_eq!(Target::TopRight.zone().written(), [0.5, 0., 0.5, 0.5]);
    }

    #[test]
    fn a_zone_lies_within_the_display() {
        assert!(Zone::parse([0., 0., 0.5, 1.]).is_some());
        assert!(Zone::parse([0.5, 0., 0.6, 1.]).is_none());
        assert!(Zone::parse([0., 0., 0., 1.]).is_none());
        assert!(Zone::parse([-0.1, 0., 0.5, 1.]).is_none());
        assert!(Zone::parse([0., 0., f32::NAN, 1.]).is_none());
    }

    #[test]
    fn the_arrow_keys_go_from_a_half_to_a_quarter_and_back() {
        use Target::*;
        let snapped = Placed::Snapped;
        assert_eq!(quick(Placed::Restored, Toward::Left), Step::To(LeftHalf));
        assert_eq!(quick(Placed::Maximized, Toward::Right), Step::To(RightHalf));
        assert_eq!(quick(snapped(RightHalf), Toward::Left), Step::Restore);
        assert_eq!(quick(snapped(LeftHalf), Toward::Right), Step::Restore);
        assert_eq!(
            quick(snapped(LeftHalf), Toward::Left),
            Step::Across(Toward::Left, RightHalf)
        );
        assert_eq!(quick(snapped(TopRight), Toward::Left), Step::To(TopLeft));
        assert_eq!(quick(snapped(LeftHalf), Toward::Up), Step::To(TopLeft));
        assert_eq!(quick(snapped(TopLeft), Toward::Up), Step::To(Maximize));
        assert_eq!(quick(snapped(BottomRight), Toward::Up), Step::To(RightHalf));
        assert_eq!(quick(Placed::Restored, Toward::Up), Step::To(Maximize));
        assert_eq!(quick(Placed::Maximized, Toward::Up), Step::Stay);
        assert_eq!(quick(Placed::Maximized, Toward::Down), Step::Restore);
        assert_eq!(
            quick(snapped(RightHalf), Toward::Down),
            Step::To(BottomRight)
        );
        assert_eq!(quick(snapped(TopLeft), Toward::Down), Step::To(LeftHalf));
        assert_eq!(quick(snapped(BottomLeft), Toward::Down), Step::Minimize);
        assert_eq!(quick(Placed::Restored, Toward::Down), Step::Minimize);
    }

    #[test]
    fn a_window_keeps_its_share_on_another_display() {
        let from = rect(0, 0, 1920, 1040);
        let to = rect(1920, 0, 4480, 1400);
        let moved = carried(rect(480, 260, 1440, 780), from, to);
        assert_eq!(moved, rect(2560, 350, 3840, 1050));
        // One larger than the work area is made to fit it.
        let big = carried(rect(-50, -10, 2000, 1100), from, to);
        assert!(to.covers(big));
    }

    #[test]
    fn a_window_dragged_away_takes_its_size_back_under_the_pointer() {
        let work = rect(0, 0, 1920, 1040);
        let restored = rect(300, 200, 1100, 800);
        // The pointer was a quarter of the way across the snapped window.
        let frame = rect(0, 0, 960, 1040);
        let placed = unsnapped(restored, frame, (240, 10), work);
        assert_eq!(placed, rect(40, 0, 840, 600));
        // It stays on the display.
        let placed = unsnapped(restored, rect(1700, 0, 1920, 1040), (1915, 10), work);
        assert_eq!(placed, rect(1120, 0, 1920, 600));
    }

    #[test]
    fn the_configuration_is_checked() {
        let config: SnapConfig = toml::from_str(
            "enabled = true\nedge = 500\ngap = 6\n[[zone]]\nrect = [0, 0, 0.3, 1]\n[[zone]]\ndisplay = 2\nrect = [0.5, 0, 0.8, 1]\n[[layout]]\nzones = [[0, 0, 0.7, 1], [0.7, 0, 0.3, 1]]\n",
        )
        .unwrap();
        let settings = config.settings(false);
        assert!(settings.dragging);
        assert_eq!(settings.edge, DEFAULT_EDGE as i32);
        assert_eq!(settings.gap, 6);
        assert_eq!(settings.zones.len(), 1);
        assert_eq!(settings.layouts.len(), 1);
        assert_eq!(settings.hover, Some(Duration::from_millis(450)));
        let plain = SnapConfig::default();
        assert!(!plain.settings(false).dragging);
        assert!(plain.settings(true).dragging);
        assert_eq!(plain.settings(true).layouts.len(), LAYOUTS.len());
        assert!(plain.settings(false).hover.is_none());
        assert!(toml::from_str::<SnapConfig>("edges = 3").is_err());
    }

    #[test]
    fn the_layouts_of_windows_fill_the_display() {
        for zones in LAYOUTS {
            let area: f32 = zones.iter().map(|zone| zone.width * zone.height).sum();
            assert!((area - 1.).abs() < 0.001, "{zones:?}");
            assert!(
                zones
                    .iter()
                    .all(|zone| Zone::parse([zone.x, zone.y, zone.width, zone.height]).is_some())
            );
        }
        assert_eq!(Target::LeftHalf.partner(), Some(Target::RightHalf));
        assert_eq!(Target::TopLeft.partner(), None);
        for arg in SNAP_ARGS {
            let known = Toward::parse(arg).is_some()
                || Target::parse(arg).is_some()
                || matches!(arg, "restore" | "minimize");
            assert!(known, "{arg}");
        }
    }
}
