// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Window actions, as KWin has them, under `[window_actions]`: with the
//! key `modifier` names held (Alt or the Windows key), the left button of
//! the pointer moves the window under it from wherever it is pressed and
//! the right button resizes it from its nearest corner; while Tessera
//! moves a window so, its sides are drawn to the edges of the displays and
//! of the other windows within `snap_distance` pixels. A click with the
//! middle button on a title bar lowers, minimizes or closes the window
//! (`titlebar_middle`), Windows gives the keyboard to the window under the
//! pointer with `focus_follows_mouse`, and the action `kill-window` ends
//! the program of a window that is clicked, once that is confirmed.
//!
//! Where the configuration asks for it, a double click and a turn of the
//! wheel on a title bar, a turn of the wheel and the middle button with
//! the key of `modifier` held, and a click on a window that is not in
//! front do what it says (see [`mouse`]), and new windows are placed as
//! `placement` says (see [`placement`]).
//!
//! The windows that Windows itself moves, by their title bar or border,
//! are not drawn to edges: Windows moves them in a loop of its own that a
//! program outside them cannot adjust without being loaded into them.
//!
//! Everything here is off unless the configuration asks for it.

mod kill;
pub mod mouse;
pub mod placement;

use std::time::Duration;

use gpui::{App, AsyncApp, Global};
use serde::Deserialize;

use crate::actions::{Action, Actions};
use crate::platform::{
    ActiveTracking, AppWindow, AppWindowId, Display, GrabKey, MouseHook, PointerButton,
    PointerEvent, ScreenRect, pointer_at, pointer_held,
};
use crate::rules::Rules;
use crate::snap::unsnapped;
use crate::tiling::Tiler;

pub use kill::ask_to_kill;

use self::mouse::{
    InactiveClick, ModifierMiddle, ModifierWheel, MouseActions, TitleDouble, TitleWheel,
};
use self::placement::{Cascade, Placement};

/// The time between two moves of a window that is being dragged.
const FRAME: Duration = Duration::from_millis(16);

/// How near an edge a side of a window is drawn to it when the
/// configuration says nothing, and the most it may say, in logical
/// pixels.
const DEFAULT_SNAP_DISTANCE: u32 = 10;
const MOST_SNAP_DISTANCE: u32 = 100;

/// How long the pointer rests on a window before it gets the keyboard
/// when the configuration says nothing, and the most it may say, in
/// milliseconds.
const DEFAULT_FOCUS_DELAY: u32 = 300;
const MOST_FOCUS_DELAY: u32 = 5000;

/// How narrow and low a window may be resized with the pointer, in
/// logical pixels.
const LEAST_SIDE: f32 = 64.;

/// What a click with the middle button on a title bar does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleClick {
    #[default]
    None,
    /// Puts the window below the others.
    Lower,
    Minimize,
    Close,
}

/// `[window_actions]` of the configuration.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WindowActionsConfig {
    /// The key held to move windows with the left button and resize them
    /// with the right one; none by default.
    pub modifier: Option<GrabKey>,
    /// How near an edge of a display or of another window a side of a
    /// window that is moved so is drawn to it, in logical pixels; 0 draws
    /// it to none.
    pub snap_distance: u32,
    pub titlebar_middle: TitleClick,
    /// Whether the window under the pointer gets the keyboard once the
    /// pointer rests on it for `focus_delay` milliseconds.
    pub focus_follows_mouse: bool,
    pub focus_delay: u32,
    /// Whether that window is also brought above the others.
    pub focus_raises: bool,
    /// Where new windows go.
    pub placement: Placement,
    /// What a double click on a title bar does.
    pub titlebar_double: TitleDouble,
    /// What a turn of the wheel over a title bar does.
    pub titlebar_wheel: TitleWheel,
    /// What a turn of the wheel with `modifier` held does over a window.
    pub modifier_wheel: ModifierWheel,
    /// What the middle button with `modifier` held does to a window.
    pub modifier_middle: ModifierMiddle,
    /// What a click on a window that is not in front does.
    pub inactive_click: InactiveClick,
}

impl Default for WindowActionsConfig {
    fn default() -> Self {
        Self {
            modifier: None,
            snap_distance: DEFAULT_SNAP_DISTANCE,
            titlebar_middle: TitleClick::None,
            focus_follows_mouse: false,
            focus_delay: DEFAULT_FOCUS_DELAY,
            focus_raises: false,
            placement: Placement::Windows,
            titlebar_double: TitleDouble::Windows,
            titlebar_wheel: TitleWheel::Nothing,
            modifier_wheel: ModifierWheel::Nothing,
            modifier_middle: ModifierMiddle::Nothing,
            inactive_click: InactiveClick::Windows,
        }
    }
}

impl WindowActionsConfig {
    /// What Windows is to do of the keyboard and the pointer, as this
    /// says, within the bounds it may say.
    fn tracking(&self) -> ActiveTracking {
        ActiveTracking {
            on: true,
            delay: self.focus_delay.min(MOST_FOCUS_DELAY),
            raises: self.focus_raises,
        }
    }

    /// The actions of the pointer this says.
    fn mouse(&self) -> MouseActions {
        MouseActions {
            titlebar_double: self.titlebar_double,
            titlebar_wheel: self.titlebar_wheel,
            modifier_wheel: self.modifier_wheel,
            modifier_middle: self.modifier_middle,
            inactive_click: self.inactive_click,
        }
    }
}

/// A line that the sides of a moved window are drawn to: `at` across,
/// reaching from `from` to `to` along it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    at: i32,
    from: i32,
    to: i32,
}

impl Edge {
    /// The nearest way from `side`, a side of a window reaching from
    /// `start` to `end` along the edge, to this edge, if it is within
    /// `distance` and the two lie beside each other.
    fn pull(self, side: i32, start: i32, end: i32, distance: i32) -> Option<i32> {
        let beside = self.from - distance < end && self.to + distance > start;
        let way = self.at - side;
        (beside && way.abs() <= distance).then_some(way)
    }
}

/// The edges a moved window is drawn to: the upright ones that its left
/// and right sides are drawn to, and the level ones for its top and
/// bottom.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    upright: Vec<Edge>,
    level: Vec<Edge>,
}

impl Edges {
    /// Adds the four sides of `rect`.
    pub fn add(&mut self, rect: ScreenRect) {
        for at in [rect.left, rect.right] {
            let (from, to) = (rect.top, rect.bottom);
            self.upright.push(Edge { at, from, to });
        }
        for at in [rect.top, rect.bottom] {
            let (from, to) = (rect.left, rect.right);
            self.level.push(Edge { at, from, to });
        }
    }

    /// The edges of the displays and of their work areas.
    fn of_displays(displays: &[Display]) -> Self {
        let mut edges = Self::default();
        for display in displays {
            edges.add(display.frame);
            if display.work != display.frame {
                edges.add(display.work);
            }
        }
        edges
    }
}

/// Which sides of a window follow the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sides {
    left: bool,
    top: bool,
    right: bool,
    bottom: bool,
}

impl Sides {
    /// All of them, as when the window is moved.
    const ALL: Self = Self {
        left: true,
        top: true,
        right: true,
        bottom: true,
    };
}

/// `frame` with its sides `moving` drawn to the nearest of `edges` within
/// `distance`, the nearest one on each axis: a window that is moved
/// moves as a whole, one that is resized only on the sides that follow
/// the pointer.
pub fn magnetic(frame: ScreenRect, moving: Sides, edges: &Edges, distance: i32) -> ScreenRect {
    if distance <= 0 {
        return frame;
    }
    let nearest = |sides: [(bool, i32); 2], lines: &[Edge], start: i32, end: i32| {
        { sides.into_iter() }
            .filter(|&(moves, _)| moves)
            .flat_map(|(_, side)| {
                { lines.iter() }.filter_map(move |edge| edge.pull(side, start, end, distance))
            })
            .min_by_key(|way| way.abs())
    };
    let across = nearest(
        [(moving.left, frame.left), (moving.right, frame.right)],
        &edges.upright,
        frame.top,
        frame.bottom,
    );
    let down = nearest(
        [(moving.top, frame.top), (moving.bottom, frame.bottom)],
        &edges.level,
        frame.left,
        frame.right,
    );
    let mut rect = frame;
    if let Some(way) = across {
        rect.left += if moving.left { way } else { 0 };
        rect.right += if moving.right { way } else { 0 };
    }
    if let Some(way) = down {
        rect.top += if moving.top { way } else { 0 };
        rect.bottom += if moving.bottom { way } else { 0 };
    }
    rect
}

/// Where a window of `frame` is when the corner of it nearest to `from`,
/// the point where the pointer was pressed, follows the pointer to `to`,
/// keeping at least `least` pixels of width and height, and which of its
/// sides moved: the corner is that of the quarter of the window that
/// `from` lies in.
pub fn resized_from_corner(
    frame: ScreenRect,
    from: (i32, i32),
    to: (i32, i32),
    least: i32,
) -> (ScreenRect, Sides) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let left = from.0 < (frame.left + frame.right) / 2;
    let top = from.1 < (frame.top + frame.bottom) / 2;
    let mut rect = frame;
    match left {
        true => rect.left = (frame.left + dx).min(frame.right - least),
        false => rect.right = (frame.right + dx).max(frame.left + least),
    }
    match top {
        true => rect.top = (frame.top + dy).min(frame.bottom - least),
        false => rect.bottom = (frame.bottom + dy).max(frame.top + least),
    }
    let sides = Sides {
        left,
        top,
        right: !left,
        bottom: !top,
    };
    (rect, sides)
}

/// `rect` moved by `(x, y)`.
fn shifted(rect: ScreenRect, (x, y): (i32, i32)) -> ScreenRect {
    ScreenRect {
        left: rect.left + x,
        top: rect.top + y,
        right: rect.right + x,
        bottom: rect.bottom + y,
    }
}

/// Where a maximized window of `frame` goes when it is grabbed with the
/// pointer at `at` and takes the size of `restored` again: the pointer
/// stays at the same share of the way across it and down it, and it is
/// kept within `work`.
fn restored_around(
    restored: ScreenRect,
    frame: ScreenRect,
    at: (i32, i32),
    work: ScreenRect,
) -> ScreenRect {
    let across = unsnapped(restored, frame, at, work);
    let height = restored.bottom - restored.top;
    let down = (at.1 - frame.top) as f32 / (frame.bottom - frame.top).max(1) as f32;
    let top = at.1 - (height as f32 * down.clamp(0., 1.)).round() as i32;
    let top = top.clamp(work.top, (work.bottom - height).max(work.top));
    ScreenRect {
        top,
        bottom: top + height,
        ..across
    }
}

/// A window being moved or resized with the pointer.
struct Drag {
    window: AppWindowId,
    button: PointerButton,
    /// Where the pointer was pressed.
    from: (i32, i32),
    /// What was seen of the window then.
    frame: ScreenRect,
    edges: Edges,
    /// How near an edge a side is drawn to it, in physical pixels.
    distance: i32,
    /// The least width and height, in physical pixels.
    least: i32,
    /// Where the window was put last.
    placed: ScreenRect,
    /// Tells this drag from the ones before.
    serial: u64,
}

impl Drag {
    /// Where the window goes with the pointer at `pointer`.
    fn frame_at(&self, pointer: (i32, i32)) -> ScreenRect {
        let (frame, sides) = match self.button {
            PointerButton::Left => {
                let way = (pointer.0 - self.from.0, pointer.1 - self.from.1);
                (shifted(self.frame, way), Sides::ALL)
            }
            PointerButton::Right => resized_from_corner(self.frame, self.from, pointer, self.least),
        };
        magnetic(frame, sides, &self.edges, self.distance)
    }
}

/// Moves and resizes windows with the pointer, answers the middle button
/// on title bars and has focus follow the pointer.
#[derive(Default)]
pub struct WindowActions {
    config: WindowActionsConfig,
    drag: Option<Drag>,
    drags: u64,
    /// What the user chose of the keyboard following the pointer, taken
    /// while Tessera has Windows do otherwise.
    tracking: Option<ActiveTracking>,
    /// The window a cascade placed last.
    cascade: Cascade,
}

impl Global for WindowActions {}

impl WindowActions {
    /// Does what `config` says from now on, and gives back what it changed
    /// of Windows and of other windows when Tessera quits.
    pub fn start(config: &WindowActionsConfig, cx: &mut App) {
        if cx.has_global::<Self>() {
            return Self::configure(config, cx);
        }
        cx.set_global(Self::default());
        give_back_on_quit(cx);
        Self::listen(cx);
        Self::apply(config, cx);
    }

    /// Takes over a configuration that was read again, if it says
    /// something else of the window actions.
    pub fn configure(config: &WindowActionsConfig, cx: &mut App) {
        match cx.try_global::<Self>() {
            None => Self::start(config, cx),
            Some(this) if this.config == *config => {}
            Some(_) => Self::apply(config, cx),
        }
    }

    fn apply(config: &WindowActionsConfig, cx: &mut App) {
        if config.snap_distance > MOST_SNAP_DISTANCE {
            report!("[window_actions] `snap_distance` is at most {MOST_SNAP_DISTANCE}");
        }
        if config.focus_delay > MOST_FOCUS_DELAY {
            report!("[window_actions] `focus_delay` is at most {MOST_FOCUS_DELAY} milliseconds");
        }
        let this = cx.global_mut::<Self>();
        this.config = config.clone();
        if config.modifier.is_none() {
            this.drag = None;
        }
        let middle = config.titlebar_middle != TitleClick::None;
        MouseHook::configure(config.modifier, middle);
        let mouse = config.mouse();
        if mouse.need_key() && config.modifier.is_none() {
            report!(
                "[window_actions] `modifier_wheel` and `modifier_middle` need a `modifier` to hold"
            );
        }
        MouseHook::ask(mouse.asks(config.modifier.is_some()));
        Rules::place_new_windows(config.placement != Placement::Windows, cx);
        match config.focus_follows_mouse {
            true => Self::take_tracking(config.tracking(), cx),
            false => Self::give_tracking_back(cx),
        }
    }

    /// Has Windows give the keyboard to the window under the pointer as
    /// `wanted` says, remembering first what the user chose.
    fn take_tracking(wanted: ActiveTracking, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if this.tracking.is_none() {
            this.tracking = ActiveTracking::chosen();
        }
        cx.background_executor()
            .spawn(async move {
                if ActiveTracking::now() != Some(wanted) && !wanted.set() {
                    report!("Windows does not have the keyboard follow the pointer");
                }
            })
            .detach();
    }

    /// Gives Windows back what the user chose of the keyboard following
    /// the pointer, also when a Tessera before this one ended without
    /// doing so.
    fn give_tracking_back(cx: &mut App) {
        let chosen = cx.global_mut::<Self>().tracking.take();
        cx.background_executor()
            .spawn(async move { restore_tracking(chosen) })
            .detach();
    }

    /// Hands what the hook of the pointer sees to [`Self::handle`], for as
    /// long as Tessera runs.
    fn listen(cx: &mut App) {
        let events = MouseHook::events();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(event) = events.recv().await {
                if cx.update(|cx| Self::handle(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn handle(event: PointerEvent, cx: &mut App) {
        match event {
            PointerEvent::Grab { button, window, at } => {
                // The key held is no key pressed by itself once let go.
                MouseHook::break_lone_key();
                Self::grab(window, button, at, cx);
            }
            PointerEvent::Release { at } => Self::release(at, cx),
            PointerEvent::Middle { window, at } => Self::middle(window, at, cx),
            PointerEvent::TitleDouble { window } => {
                mouse::title_double(cx.global::<Self>().config.titlebar_double, window);
            }
            PointerEvent::TitleWheel { window, up } => {
                let wheel = cx.global::<Self>().config.titlebar_wheel;
                mouse::title_wheel(wheel, window, up, cx);
            }
            PointerEvent::ModifierWheel { window, up } => {
                let wheel = cx.global::<Self>().config.modifier_wheel;
                mouse::modifier_wheel(wheel, window, up, cx);
            }
            PointerEvent::ModifierMiddle { window, .. } => {
                let middle = cx.global::<Self>().config.modifier_middle;
                mouse::modifier_middle(middle, window, cx);
            }
            PointerEvent::Activate { window } => window.activate(),
        }
    }

    /// Puts the new window `window` where `placement` says, as the rules
    /// ask when they first see a window that no rule places.
    pub fn place_new(window: &AppWindow, cx: &mut App) {
        let Some(this) = cx.try_global::<Self>() else {
            return;
        };
        let wanted = this.config.placement;
        let mut cascade = std::mem::take(&mut cx.global_mut::<Self>().cascade);
        placement::place(wanted, window, &mut cascade, cx);
        cx.global_mut::<Self>().cascade = cascade;
    }

    /// Begins moving or resizing `window`, pressed with `button` at `at`.
    /// A maximized window is restored first, around the pointer; a window
    /// the tiling places, a minimized one and one that fills its display,
    /// as a game does, are left alone.
    fn grab(window: AppWindowId, button: PointerButton, at: (i32, i32), cx: &mut App) {
        let this = cx.global::<Self>();
        if this.config.modifier.is_none() || Tiler::manages(window, cx) || window.is_minimized() {
            return;
        }
        let (Some(display), Some(mut frame)) = (Display::of_window(window), window.frame()) else {
            return;
        };
        let maximized = window.is_maximized();
        if !maximized && (window.fills_display() || frame.covers(display.frame)) {
            return;
        }
        if maximized {
            let restored = window.restored_frame().unwrap_or(frame);
            frame = restored_around(restored, frame, at, display.work);
            window.place_restored(frame, false);
        }
        let scale = display.scale.max(0.1);
        let distance = this.config.snap_distance.min(MOST_SNAP_DISTANCE) as f32 * scale;
        let this = cx.global_mut::<Self>();
        this.drags += 1;
        let serial = this.drags;
        this.drag = Some(Drag {
            window,
            button,
            from: at,
            frame,
            edges: Edges::of_displays(&Display::all()),
            distance: distance.round() as i32,
            least: (LEAST_SIDE * scale).round() as i32,
            placed: frame,
            serial,
        });
        Self::add_window_edges(window, serial, cx);
        Self::follow(serial, cx);
    }

    /// Adds the edges of the other windows that are shown to the drag
    /// `serial`, once they are read aside.
    fn add_window_edges(dragged: AppWindowId, serial: u64, cx: &mut App) {
        let read = cx.background_executor().spawn(async move {
            { AppWindow::enumerate().into_iter() }
                .map(|window| window.id)
                .filter(|&id| id != dragged && !id.is_minimized())
                .filter_map(AppWindowId::frame)
                .collect::<Vec<ScreenRect>>()
        });
        cx.spawn(async move |cx: &mut AsyncApp| {
            let frames = read.await;
            cx.update(|cx| {
                let drag = cx.global_mut::<Self>().drag.as_mut();
                if let Some(drag) = drag.filter(|drag| drag.serial == serial) {
                    for frame in frames {
                        drag.edges.add(frame);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Moves the window of the drag `serial` with the pointer sixty times
    /// a second, until the drag ends.
    fn follow(serial: u64, cx: &mut App) {
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(FRAME).await;
                let going = cx.update(|cx| Self::step(serial, None, cx));
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Puts the window of the drag `serial` where the pointer, at
    /// `pointer` or where it is now, has it. Whether the drag goes on is
    /// returned. A drag whose button is no longer held ends as if it was
    /// let go, as when it was let go over a window of a program that runs
    /// with more rights than Tessera, which Windows does not show the
    /// hook.
    fn step(serial: u64, pointer: Option<(i32, i32)>, cx: &mut App) -> bool {
        let this = cx.global_mut::<Self>();
        let Some(drag) = this.drag.as_mut().filter(|drag| drag.serial == serial) else {
            return false;
        };
        if !drag.window.exists() {
            this.drag = None;
            return false;
        }
        // Either button counts, as the hook and the state of the buttons
        // may tell them apart differently where they are swapped.
        if pointer.is_none() && !pointer_held() {
            MouseHook::end_grab(drag.button);
            if let Some(drag) = this.drag.take() {
                Rules::move_ended(drag.window, cx);
            }
            return false;
        }
        let Some(pointer) = pointer.or_else(pointer_at) else {
            return true;
        };
        let frame = drag.frame_at(pointer);
        if frame != drag.placed {
            drag.placed = frame;
            drag.window.place(frame);
        }
        true
    }

    /// Ends the drag with the pointer let go at `at`, and has the rules
    /// look at the window again.
    fn release(at: (i32, i32), cx: &mut App) {
        let Some(serial) = cx.global::<Self>().drag.as_ref().map(|drag| drag.serial) else {
            return;
        };
        Self::step(serial, Some(at), cx);
        if let Some(drag) = cx.global_mut::<Self>().drag.take() {
            Rules::move_ended(drag.window, cx);
        }
    }

    /// Does what `titlebar_middle` says to `window` if the middle button
    /// was pressed at `at` on its title bar, which the window is asked
    /// aside.
    fn middle(window: AppWindowId, at: (i32, i32), cx: &mut App) {
        let click = cx.global::<Self>().config.titlebar_middle;
        if click == TitleClick::None {
            return;
        }
        let asked = { cx.background_executor() }.spawn(async move { window.on_title_bar(at) });
        cx.spawn(async move |_: &mut AsyncApp| {
            if asked.await != Some(true) {
                return;
            }
            match click {
                TitleClick::None => {}
                TitleClick::Lower => {
                    window.lower();
                }
                TitleClick::Minimize => window.minimize(),
                TitleClick::Close => window.close(),
            }
        })
        .detach();
    }
}

/// Gives Windows back `chosen`, what the user chose of the keyboard
/// following the pointer, or else what the settings of the user say, if
/// Windows does otherwise now.
fn restore_tracking(chosen: Option<ActiveTracking>) {
    let Some(chosen) = chosen.or_else(ActiveTracking::chosen) else {
        return;
    };
    if ActiveTracking::now() != Some(chosen) && !chosen.set() {
        report!("Windows keeps the keyboard following the pointer until the next sign in");
    }
}

/// Marks that the windows are given back when Tessera quits.
#[derive(Default)]
struct GivingBack;

impl Global for GivingBack {}

/// Has Tessera give back, when it quits, what the window actions and the
/// rules changed: the styles of windows whose title bar was taken or that
/// were made to fill their display, the windows kept below, and the
/// keyboard following the pointer. Asked again, it does nothing more.
pub fn give_back_on_quit(cx: &mut App) {
    if cx.has_global::<GivingBack>() {
        return;
    }
    cx.set_global(GivingBack);
    cx.on_app_quit(|cx| {
        AppWindowId::give_styles_back();
        AppWindowId::give_focus_and_close_back();
        if let Some(this) = cx.try_global::<WindowActions>()
            && this.config.focus_follows_mouse
        {
            restore_tracking(this.tracking);
        }
        async {}
    })
    .detach();
}

/// Registers the action `kill-window`.
pub fn register(actions: &mut Actions) -> anyhow::Result<()> {
    let kill = Action::new(|_, cx| kill::pick(cx)).by_default("ctrl+alt+escape", None);
    actions.register("kill-window", kill)
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

    #[test]
    fn a_moved_window_is_drawn_to_the_nearest_edge() {
        let mut edges = Edges::default();
        edges.add(rect(0, 0, 1920, 1080));
        edges.add(rect(1000, 100, 1400, 500));
        // Its left side is 6 pixels from the display, its top 8: both pull.
        let moved = magnetic(rect(6, 8, 406, 308), Sides::ALL, &edges, 10);
        assert_eq!(moved, rect(0, 0, 400, 300));
        // Its right side is 4 pixels from the left of the other window.
        let beside = magnetic(rect(596, 200, 996, 400), Sides::ALL, &edges, 10);
        assert_eq!(beside, rect(600, 200, 1000, 400));
        // Far from everything it stays.
        let free = rect(300, 300, 700, 600);
        assert_eq!(magnetic(free, Sides::ALL, &edges, 10), free);
        assert_eq!(
            magnetic(rect(6, 8, 406, 308), Sides::ALL, &edges, 0),
            rect(6, 8, 406, 308)
        );
    }

    #[test]
    fn an_edge_draws_only_what_lies_beside_it() {
        let mut edges = Edges::default();
        edges.add(rect(1000, 100, 1400, 500));
        // Level with the window's left side but far below it: no pull.
        let below = rect(596, 900, 996, 1000);
        assert_eq!(magnetic(below, Sides::ALL, &edges, 10), below);
    }

    #[test]
    fn a_resized_window_moves_only_the_sides_that_follow() {
        let mut edges = Edges::default();
        edges.add(rect(0, 0, 1920, 1080));
        let frame = rect(100, 100, 500, 400);
        let (grown, sides) = resized_from_corner(frame, (450, 350), (1915, 1075), 50);
        assert_eq!(grown, rect(100, 100, 1965, 1125));
        assert!(sides.right && sides.bottom && !sides.left && !sides.top);
        let drawn = magnetic(rect(100, 100, 1915, 1075), sides, &edges, 10);
        assert_eq!(drawn, rect(100, 100, 1920, 1080));
    }

    #[test]
    fn the_nearest_corner_follows_the_pointer() {
        let frame = rect(100, 100, 500, 400);
        let (grown, _) = resized_from_corner(frame, (450, 350), (500, 380), 50);
        assert_eq!(grown, rect(100, 100, 550, 430));
        let (moved, sides) = resized_from_corner(frame, (120, 130), (90, 150), 50);
        assert_eq!(moved, rect(70, 120, 500, 400));
        assert!(sides.left && sides.top);
        // The window keeps its least size.
        let (small, _) = resized_from_corner(frame, (450, 350), (0, 0), 50);
        assert_eq!(small, rect(100, 100, 150, 150));
    }

    #[test]
    fn a_restored_window_stays_around_the_pointer() {
        let work = rect(0, 0, 1920, 1040);
        let restored = rect(400, 300, 1200, 900);
        // Grabbed three quarters across and down the maximized window.
        let placed = restored_around(restored, work, (1440, 780), work);
        assert_eq!(placed, rect(840, 330, 1640, 930));
        // Near the bottom it is kept within the work area.
        let low = restored_around(restored, work, (960, 1039), work);
        assert_eq!(low.bottom, 1040);
        assert_eq!(low.bottom - low.top, 600);
    }

    #[test]
    fn the_configuration_is_off_by_default() {
        let config: WindowActionsConfig = toml::from_str("").unwrap();
        assert_eq!(config, WindowActionsConfig::default());
        assert_eq!(config.modifier, None);
        assert!(!config.focus_follows_mouse);
        let config: WindowActionsConfig =
            toml::from_str("modifier = \"win\"\ntitlebar_middle = \"lower\"").unwrap();
        assert_eq!(config.modifier, Some(GrabKey::Win));
        assert_eq!(config.titlebar_middle, TitleClick::Lower);
        assert!(toml::from_str::<WindowActionsConfig>("modifier = \"ctrl\"").is_err());
        assert!(toml::from_str::<WindowActionsConfig>("snap = 3").is_err());
        let config: WindowActionsConfig = toml::from_str(
            "placement = \"smart\"\ntitlebar_double = \"above\"\ntitlebar_wheel = \"opacity\"\n\
             modifier_wheel = \"volume\"\nmodifier_middle = \"close\"\ninactive_click = \"activate-only\"",
        )
        .unwrap();
        assert_eq!(config.placement, Placement::Smart);
        assert_eq!(config.titlebar_double, TitleDouble::Above);
        assert_eq!(config.titlebar_wheel, TitleWheel::Opacity);
        assert_eq!(config.modifier_wheel, ModifierWheel::Volume);
        assert_eq!(config.modifier_middle, ModifierMiddle::Close);
        assert_eq!(config.inactive_click, InactiveClick::ActivateOnly);
        assert!(toml::from_str::<WindowActionsConfig>("placement = \"random\"").is_err());
    }
}
