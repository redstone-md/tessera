// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The configuration of the displays, as the display page of KDE Plasma
//! offers it. Widget `display-settings`, in the popup of the same name in
//! the middle of the display, which the action `display-settings` opens:
//! the displays drawn as they stand on the desktop, to be dragged into
//! place (they snap to each other's edges, never overlap and always touch
//! one another), and for the one chosen whether it is on, whether it is
//! the main display, and its resolution, refresh rate, scale and
//! orientation.
//!
//! Nothing changes before Apply. After it the displays show the new
//! configuration for 15 seconds, with a banner and a bar here asking
//! whether to keep it; without an answer the old one comes back, as in
//! Windows, so that a mode the display cannot show undoes itself. It is
//! saved for the next start only once kept, and the shell puts the old one
//! back when it quits during the trial. Win+P stays with the popup
//! `display-mode`.
//!
//! The displays are called by the number Windows gives their desktop,
//! "Display 1" for `\\.\DISPLAY1`, and a display that is off by the next
//! number free.

use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, AsyncApp, Context, Div, Entity, FontWeight, Global, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, Render,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
    div, px,
};

use super::quick::{SettingsPage, chip};
use super::settings_parts::{Feedback, chip_row, note_line};
use crate::actions::{Action, Actions};
use crate::banners::{Banners, Notice, NoticeAction};
use crate::config::{PopupConfig, PopupPlace, WidgetSpec};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::Spring;
use crate::notifications_store::NotificationStore;
use crate::platform::{DisplayOutput, DisplaySetup, Rotation, VideoMode, hertz_text};
use crate::runners::fill;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{choice_button, list_title, one_line, switch};

/// The name of the popup, of its widget and of the action that opens it.
const NAME: &str = "display-settings";

/// How large the popup is, in logical pixels.
const WIDTH: f32 = 600.;
const HEIGHT: f32 = 760.;

/// The size of the drawing of the arrangement, and the room kept around
/// the displays in it.
const CANVAS_WIDTH: f32 = 520.;
const CANVAS_HEIGHT: f32 = 200.;
const CANVAS_MARGIN: f32 = 16.;

/// How near, in pixels of the drawing, an edge has to come to another to
/// snap to it.
const SNAP_DISTANCE: f32 = 10.;

/// How long a new configuration is tried before the old one comes back.
const TRIAL: Duration = Duration::from_secs(15);

const ICON_DISPLAY: &str = "\u{e7f4}";

const SETTINGS: SettingsPage = SettingsPage {
    address: "ms-settings:display",
    text: "Display settings",
};

pub struct DisplaySettingsModule;

impl Module for DisplaySettingsModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |_, cx| cx.new(DisplaySettings::new).into())?;
        widgets.register_popup(PopupConfig {
            name: NAME.to_owned(),
            place: PopupPlace::Center,
            width: WIDTH,
            height: HEIGHT,
            content: vec![WidgetSpec {
                widget: NAME.to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        actions.register(NAME, Action::new(|_, cx| Shell::toggle_popup(NAME, cx)))
    }
}

/// A rectangle of the desktop, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Area {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl Area {
    fn right(self) -> i32 {
        self.x + self.width
    }

    fn bottom(self) -> i32 {
        self.y + self.height
    }

    fn at(self, x: i32, y: i32) -> Self {
        Self { x, y, ..self }
    }

    fn overlaps(self, other: Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// Whether the two share a piece of an edge, which Windows needs of
    /// displays that make one desktop.
    fn touches(self, other: Self) -> bool {
        let across =
            |from: i32, to: i32, other_from: i32, other_to: i32| from < other_to && other_from < to;
        let side = (self.right() == other.x || other.right() == self.x)
            && across(self.y, self.bottom(), other.y, other.bottom());
        let end = (self.bottom() == other.y || other.bottom() == self.y)
            && across(self.x, self.right(), other.x, other.right());
        side || end
    }
}

/// The nearest of `candidates` to `now` within `reach`; `now` where none is.
fn nearest_within(now: i32, candidates: impl Iterator<Item = i32>, reach: i32) -> i32 {
    { candidates }
        .filter(|candidate| (candidate - now).abs() <= reach)
        .min_by_key(|candidate| (candidate - now).abs())
        .unwrap_or(now)
}

/// `moving` with each of its edges pulled onto an edge of `others` that is
/// within `reach`: beside one another or lined up.
fn snap(moving: Area, others: &[Area], reach: i32) -> Area {
    let xs = others.iter().flat_map(|other| {
        [
            other.x - moving.width,
            other.right(),
            other.x,
            other.right() - moving.width,
        ]
    });
    let ys = others.iter().flat_map(|other| {
        [
            other.y - moving.height,
            other.bottom(),
            other.y,
            other.bottom() - moving.height,
        ]
    });
    moving.at(
        nearest_within(moving.x, xs, reach),
        nearest_within(moving.y, ys, reach),
    )
}

/// `moving` taken out of every area of `others` it overlaps, each time
/// the shortest way.
fn push_out(mut moving: Area, others: &[Area]) -> Area {
    for _ in 0..=others.len() * 2 {
        let Some(&other) = others.iter().find(|other| moving.overlaps(**other)) else {
            break;
        };
        let ways = [
            moving.at(other.x - moving.width, moving.y),
            moving.at(other.right(), moving.y),
            moving.at(moving.x, other.y - moving.height),
            moving.at(moving.x, other.bottom()),
        ];
        let distance = |way: &Area| (way.x - moving.x).abs() + (way.y - moving.y).abs();
        moving = ways.into_iter().min_by_key(distance).unwrap_or(moving);
    }
    moving
}

/// `moving` brought beside or above or below the nearest of `others`
/// where it touches none, sharing at least a quarter of the shorter edge.
fn attach(moving: Area, others: &[Area]) -> Area {
    if others.is_empty() || others.iter().any(|other| moving.touches(*other)) {
        return moving;
    }
    let gap = |other: &Area| {
        let across = (other.x - moving.right())
            .max(moving.x - other.right())
            .max(0);
        let down = (other.y - moving.bottom())
            .max(moving.y - other.bottom())
            .max(0);
        (across, down)
    };
    let Some(&nearest) = others.iter().min_by_key(|other| {
        let (across, down) = gap(other);
        across.max(down)
    }) else {
        return moving;
    };
    let (across, down) = gap(&nearest);
    let shared = |own: i32, theirs: i32| (own.min(theirs) / 4).max(1);
    let attached = if across >= down {
        let x = match moving.x + moving.width / 2 < nearest.x + nearest.width / 2 {
            true => nearest.x - moving.width,
            false => nearest.right(),
        };
        let share = shared(moving.height, nearest.height);
        let y = moving
            .y
            .clamp(nearest.y - moving.height + share, nearest.bottom() - share);
        moving.at(x, y)
    } else {
        let y = match moving.y + moving.height / 2 < nearest.y + nearest.height / 2 {
            true => nearest.y - moving.height,
            false => nearest.bottom(),
        };
        let share = shared(moving.width, nearest.width);
        let x = moving
            .x
            .clamp(nearest.x - moving.width + share, nearest.right() - share);
        moving.at(x, y)
    };
    push_out(attached, others)
}

/// The areas but the one at `index`.
fn others_of(areas: &[Area], index: usize) -> Vec<Area> {
    { areas.iter().enumerate() }
        .filter(|&(other, _)| other != index)
        .map(|(_, area)| *area)
        .collect()
}

/// Puts the area at `moved` out of the others' way and against one of
/// them, where it was dropped, then shifts them all so that the one at
/// `primary` stands at the origin.
fn settle(areas: &mut [Area], moved: usize, primary: usize) {
    if let Some(&area) = areas.get(moved) {
        let others = others_of(areas, moved);
        areas[moved] = attach(push_out(area, &others), &others);
    }
    normalise(areas, primary);
}

/// Puts each area but the one at `kept`, which changed its size, out of
/// the others' way and against one of them, then shifts them all so that
/// the one at `primary` stands at the origin.
fn tidy(areas: &mut [Area], kept: usize, primary: usize) {
    for index in (0..areas.len()).filter(|&index| index != kept) {
        let others = others_of(areas, index);
        areas[index] = attach(push_out(areas[index], &others), &others);
    }
    normalise(areas, primary);
}

/// Shifts every area so that the one at `primary` stands at the origin.
fn normalise(areas: &mut [Area], primary: usize) {
    let Some(&origin) = areas.get(primary) else {
        return;
    };
    for area in areas {
        *area = area.at(area.x - origin.x, area.y - origin.y);
    }
}

/// The number each display is called by: the one of its desktop, or for
/// one that is off the next free one.
fn numbers(outputs: &[DisplayOutput]) -> Vec<u32> {
    let mut next = outputs
        .iter()
        .filter_map(|output| output.number)
        .max()
        .unwrap_or(0);
    { outputs.iter() }
        .map(|output| {
            output.number.unwrap_or_else(|| {
                next += 1;
                next
            })
        })
        .collect()
}

/// The resolutions among `modes`, each once, in their order.
fn resolutions(modes: &[VideoMode]) -> Vec<(u32, u32)> {
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    for mode in modes {
        if !sizes.contains(&(mode.width, mode.height)) {
            sizes.push((mode.width, mode.height));
        }
    }
    sizes
}

/// The refresh rates `modes` offer at `size`, quickest first.
fn rates_at(modes: &[VideoMode], size: (u32, u32)) -> Vec<u32> {
    let mut rates: Vec<u32> = { modes.iter() }
        .filter(|mode| (mode.width, mode.height) == size)
        .map(|mode| mode.hertz)
        .collect();
    rates.sort_unstable_by(|one, other| other.cmp(one));
    rates.dedup();
    rates
}

/// What an orientation is called.
fn rotation_label(rotation: Rotation) -> &'static str {
    match rotation {
        Rotation::Landscape => "Landscape",
        Rotation::Portrait => "Portrait",
        Rotation::LandscapeFlipped => "Landscape (flipped)",
        Rotation::PortraitFlipped => "Portrait (flipped)",
    }
}

/// The configuration being tried, and the one that comes back unless it is
/// kept.
pub struct Trial {
    previous: Option<DisplaySetup>,
    until: Option<Instant>,
    /// The banner that asks, in the store of notifications.
    notice: Option<u64>,
    /// Counts the trials, so that the end of one is not taken for another.
    round: u64,
    /// Counts the times the old configuration came back, for the views to
    /// read the displays again.
    restored: u64,
}

struct SharedTrial(Entity<Trial>);

impl Global for SharedTrial {}

impl Trial {
    fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<SharedTrial>() {
            return shared.0.clone();
        }
        let trial = cx.new(|_| Self {
            previous: None,
            until: None,
            notice: None,
            round: 0,
            restored: 0,
        });
        cx.set_global(SharedTrial(trial.clone()));
        let ending = trial.downgrade();
        // A configuration tried when the shell quits is not kept: the old
        // one is put back before it goes, as it would be without an answer.
        // The quit waits for nothing longer, so this is done here.
        cx.on_app_quit(move |cx| {
            let previous = ending
                .update(cx, |this, _| this.previous.take())
                .ok()
                .flatten();
            if let Some(previous) = previous
                && let Err(error) = previous.restore()
            {
                report!("the displays were not set back: {error:#}");
            }
            async {}
        })
        .detach();
        trial
    }

    /// The whole seconds left before the old configuration comes back;
    /// `None` while nothing is tried.
    fn seconds_left(&self) -> Option<u64> {
        let until = self.until?;
        Some(
            until
                .saturating_duration_since(Instant::now())
                .as_secs_f32()
                .ceil() as u64,
        )
    }

    /// Starts trying what was just applied over `previous`. Applied
    /// during a trial, it is tried anew, and what comes back is still what
    /// there was before the first.
    fn begin(&mut self, previous: DisplaySetup, cx: &mut Context<Self>) {
        self.end_notice(cx);
        self.round += 1;
        self.previous = self.previous.take().or(Some(previous));
        self.until = Some(Instant::now() + TRIAL);
        let language = Language::of(cx);
        let keep = NoticeAction::new(language.say("Keep changes"), |cx| {
            Trial::shared(cx).update(cx, Trial::keep);
        });
        let revert = NoticeAction::new(language.say("Revert"), |cx| {
            Trial::shared(cx).update(cx, Trial::revert);
        });
        let notice = Notice {
            app: language.say("Display settings").to_owned(),
            title: language.say("Keep these display settings?").to_owned(),
            body: fill(
                language.say("The old ones come back in {} seconds."),
                &[&TRIAL.as_secs().to_string()],
            ),
            glyph: Some(ICON_DISPLAY),
            actions: vec![keep, revert],
            urgent: true,
            ..Default::default()
        };
        self.notice = Some(Banners::post(notice, cx).key);
        let round = self.round;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let going = this.update(cx, |this, cx| {
                    if this.round != round || this.previous.is_none() {
                        return false;
                    }
                    if this.seconds_left() == Some(0) {
                        this.revert(cx);
                        return false;
                    }
                    cx.notify();
                    true
                });
                if !going.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Keeps what is tried, and saves it for the next start away from the
    /// thread that draws.
    fn keep(&mut self, cx: &mut Context<Self>) {
        let tried = self.previous.take().is_some();
        self.until = None;
        self.end_notice(cx);
        cx.notify();
        if tried {
            cx.background_executor()
                .spawn(async {
                    if let Err(error) = DisplaySetup::save() {
                        report!("the display settings were not saved: {error:#}");
                    }
                })
                .detach();
        }
    }

    /// Puts the old configuration back, away from the thread that draws.
    fn revert(&mut self, cx: &mut Context<Self>) {
        self.until = None;
        self.end_notice(cx);
        let Some(previous) = self.previous.take() else {
            return;
        };
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let restored = cx
                .background_executor()
                .spawn(async move { previous.restore() });
            if let Err(error) = restored.await {
                report!("the displays were not set back: {error:#}");
            }
            this.update(cx, |this, cx| {
                this.restored += 1;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Takes the banner that asks away, and its notice from the list.
    fn end_notice(&mut self, cx: &mut App) {
        if let Some(key) = self.notice.take() {
            Banners::withdraw(key, cx);
            NotificationStore::dismiss(&[key], cx);
        }
    }
}

/// A display being dragged in the drawing.
struct Drag {
    /// Its place among the displays.
    index: usize,
    /// Where the pointer went down, and where the display stood then.
    from: Point<Pixels>,
    start: Area,
    /// The drawing as it was when the drag began, kept until it ends so
    /// that the displays do not shift under the pointer.
    frame: Frame,
    /// The main display then, by its place in the draft, which stays the
    /// main one wherever it is dragged.
    primary: Option<usize>,
}

/// How the desktop is drawn: the pixels of the drawing per pixel of the
/// desktop, and where the desktop's origin falls in it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    factor: f32,
    left: f32,
    top: f32,
}

impl Frame {
    /// The frame that fits `areas` into the drawing.
    fn fitting(areas: &[Area]) -> Self {
        let left = areas.iter().map(|area| area.x).min().unwrap_or(0);
        let top = areas.iter().map(|area| area.y).min().unwrap_or(0);
        let right = areas.iter().map(|area| area.right()).max().unwrap_or(1);
        let bottom = areas.iter().map(|area| area.bottom()).max().unwrap_or(1);
        let (width, height) = ((right - left).max(1) as f32, (bottom - top).max(1) as f32);
        let room = |side: f32| side - CANVAS_MARGIN * 2.;
        let factor = (room(CANVAS_WIDTH) / width).min(room(CANVAS_HEIGHT) / height);
        Self {
            factor,
            left: (CANVAS_WIDTH - width * factor) / 2. - left as f32 * factor,
            top: (CANVAS_HEIGHT - height * factor) / 2. - top as f32 * factor,
        }
    }
}

/// The settings of the displays.
struct DisplaySettings {
    /// What was read: `None` until it is, an error where it failed.
    read: Option<Result<DisplaySetup, String>>,
    /// The displays as the user changes them.
    draft: Vec<DisplayOutput>,
    selected: usize,
    drag: Option<Drag>,
    /// Whether a configuration is being applied.
    applying: bool,
    /// Why the last one was refused.
    refusal: Option<String>,
    trial: Entity<Trial>,
    /// The restorations the view has read the displays after.
    restored: u64,
    /// How far the switch of the display chosen is on its way to on.
    travel: Spring,
    feedback: Feedback,
    _trial: Subscription,
}

impl DisplaySettings {
    fn new(cx: &mut Context<Self>) -> Self {
        let trial = Trial::shared(cx);
        let restored = trial.read(cx).restored;
        let motion = cx.global::<Theme>().motion;
        let follow = cx.observe(&trial, |this, trial, cx| {
            let restored = trial.read(cx).restored;
            if restored != this.restored {
                this.restored = restored;
                Self::read(cx);
            }
            cx.notify();
        });
        Self::read(cx);
        Self {
            read: None,
            draft: Vec::new(),
            selected: 0,
            drag: None,
            applying: false,
            refusal: None,
            trial,
            restored,
            travel: Spring::new(motion.spatial_fast, 1.),
            feedback: Feedback::new(motion),
            _trial: follow,
        }
    }

    /// Reads the displays away from the thread that draws.
    fn read(cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx
                .background_executor()
                .spawn(async { DisplaySetup::read() });
            let read = read.await.map_err(|error| format!("{error:#}"));
            this.update(cx, |this, cx| this.take(read, cx)).ok();
        })
        .detach();
    }

    fn take(&mut self, read: Result<DisplaySetup, String>, cx: &mut Context<Self>) {
        if let Ok(setup) = &read {
            let chosen = self.draft.get(self.selected).map(|output| output.id);
            self.draft = setup.outputs.clone();
            self.selected = { chosen }
                .and_then(|id| self.draft.iter().position(|output| output.id == id))
                .or_else(|| self.draft.iter().position(DisplayOutput::is_primary))
                .unwrap_or(0);
            let on = self
                .draft
                .get(self.selected)
                .is_some_and(|output| output.active);
            self.travel.jump_to(if on { 1. } else { 0. });
        }
        self.read = Some(read);
        cx.notify();
    }

    fn setup(&self) -> Option<&DisplaySetup> {
        self.read.as_ref()?.as_ref().ok()
    }

    /// Whether the draft differs from what was read.
    fn changed(&self) -> bool {
        self.setup()
            .is_some_and(|setup| setup.outputs != self.draft)
    }

    /// The displays the drawing shows: those that are on, have a size and
    /// show a desktop of their own, by their place in the draft.
    fn drawn(&self) -> Vec<usize> {
        { self.draft.iter().enumerate() }
            .filter(|(_, output)| output.active && output.mirror_of.is_none())
            .filter(|(_, output)| output.mode.width > 0 && output.mode.height > 0)
            .map(|(index, _)| index)
            .collect()
    }

    fn area_of(output: &DisplayOutput) -> Area {
        let (width, height) = output.size();
        Area {
            x: output.position.0,
            y: output.position.1,
            width: width as i32,
            height: height as i32,
        }
    }

    /// The index of the main display among `drawn`.
    fn primary_among(&self, drawn: &[usize]) -> usize {
        { drawn.iter() }
            .position(|&index| self.draft[index].is_primary())
            .unwrap_or(0)
    }

    /// Puts the displays drawn out of each other's way and against one
    /// another, with the main one at the origin, or the one at `primary` of
    /// the draft. The one at `kept` of the draft stays where it is, or with
    /// `dropped` it is the one moved into place.
    fn arrange(&mut self, kept: usize, primary: Option<usize>, dropped: bool) {
        let drawn = self.drawn();
        let mut areas: Vec<Area> = drawn
            .iter()
            .map(|&index| Self::area_of(&self.draft[index]))
            .collect();
        let kept = drawn.iter().position(|&index| index == kept).unwrap_or(0);
        let primary = { primary }
            .and_then(|primary| drawn.iter().position(|&index| index == primary))
            .unwrap_or_else(|| self.primary_among(&drawn));
        match dropped {
            true => settle(&mut areas, kept, primary),
            false => tidy(&mut areas, kept, primary),
        }
        for (&index, area) in drawn.iter().zip(areas) {
            self.draft[index].position = (area.x, area.y);
        }
        self.follow_mirrors();
    }

    /// Has each display that shows the same as another stand where it
    /// does and show what it shows, where it can.
    fn follow_mirrors(&mut self) {
        for index in 0..self.draft.len() {
            let Some(main) = self.draft[index].mirror_of else {
                continue;
            };
            let Some(main) = self.draft.iter().find(|output| output.id == main).cloned() else {
                continue;
            };
            let mirror = &mut self.draft[index];
            mirror.position = main.position;
            mirror.rotation = main.rotation;
            let size = (main.mode.width, main.mode.height);
            if let Some(&mode) = { mirror.modes.iter() }
                .find(|mode| (mode.width, mode.height) == size && mode.hertz == main.mode.hertz)
                .or_else(|| {
                    mirror
                        .modes
                        .iter()
                        .find(|mode| (mode.width, mode.height) == size)
                })
            {
                mirror.mode = mode;
            }
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if index != self.selected && index < self.draft.len() {
            self.selected = index;
            let on = self.draft[index].active;
            self.travel.jump_to(if on { 1. } else { 0. });
            cx.notify();
        }
    }

    fn start_drag(&mut self, index: usize, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.select(index, cx);
        let drawn = self.drawn();
        let areas: Vec<Area> = drawn
            .iter()
            .map(|&at| Self::area_of(&self.draft[at]))
            .collect();
        self.drag = Some(Drag {
            index,
            from: event.position,
            start: Self::area_of(&self.draft[index]),
            frame: Frame::fitting(&areas),
            primary: self.draft.iter().position(DisplayOutput::is_primary),
        });
    }

    fn drag_to(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = &self.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_drag(cx);
            return;
        }
        let factor = drag.frame.factor.max(f32::EPSILON);
        let delta = event.position - drag.from;
        let moved = drag.start.at(
            drag.start.x + (f32::from(delta.x) / factor).round() as i32,
            drag.start.y + (f32::from(delta.y) / factor).round() as i32,
        );
        let index = drag.index;
        let others: Vec<Area> = { self.drawn().into_iter() }
            .filter(|&at| at != index)
            .map(|at| Self::area_of(&self.draft[at]))
            .collect();
        let snapped = snap(moved, &others, (SNAP_DISTANCE / factor).round() as i32);
        if (snapped.x, snapped.y) != self.draft[index].position {
            self.draft[index].position = (snapped.x, snapped.y);
            cx.notify();
        }
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        self.arrange(drag.index, drag.primary, true);
        cx.notify();
    }

    /// Changes the display chosen with `change`, then arranges the
    /// displays around it.
    fn edit(&mut self, change: impl FnOnce(&mut DisplayOutput), cx: &mut Context<Self>) {
        let Some(output) = self.draft.get_mut(self.selected) else {
            return;
        };
        change(output);
        self.refusal = None;
        self.arrange(self.selected, None, false);
        cx.notify();
    }

    fn set_active(&mut self, on: bool, cx: &mut Context<Self>) {
        let index = self.selected;
        let others_on = { self.draft.iter().enumerate() }
            .any(|(other, output)| other != index && output.active);
        if !on && !others_on {
            self.refusal = Some(
                Language::of(cx)
                    .say("At least one display has to stay on.")
                    .to_owned(),
            );
            cx.notify();
            return;
        }
        let was_primary = self.draft[index].is_primary();
        self.draft[index].active = on;
        self.travel.animate_to(if on { 1. } else { 0. });
        self.refusal = None;
        // Another display becomes the main one when the main one goes off.
        let primary = match was_primary && !on {
            true => self.drawn().first().copied(),
            false => None,
        };
        self.arrange(primary.unwrap_or(index), primary, false);
        cx.notify();
    }

    fn make_primary(&mut self, cx: &mut Context<Self>) {
        let index = self.selected;
        self.arrange(index, Some(index), false);
        cx.notify();
    }

    fn set_size(&mut self, size: (u32, u32), cx: &mut Context<Self>) {
        self.edit(
            |output| {
                let rates = rates_at(&output.modes, size);
                let hertz = match rates.contains(&output.mode.hertz) {
                    true => output.mode.hertz,
                    false => rates.first().copied().unwrap_or(output.mode.hertz),
                };
                output.mode = VideoMode {
                    width: size.0,
                    height: size.1,
                    hertz,
                };
            },
            cx,
        );
    }

    /// Applies the draft away from the thread that draws, and tries it.
    fn apply(&mut self, cx: &mut Context<Self>) {
        if self.applying || !self.changed() {
            return;
        }
        let Some(previous) = self.setup().cloned() else {
            return;
        };
        self.applying = true;
        self.refusal = None;
        let wanted = self.draft.clone();
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let applied = cx
                .background_executor()
                .spawn(async move { previous.apply(&wanted).map(|problems| (previous, problems)) });
            let applied = applied.await;
            this.update(cx, |this, cx| {
                this.applying = false;
                match applied {
                    Ok((previous, problems)) => {
                        this.trial.update(cx, |trial, cx| trial.begin(previous, cx));
                        this.refusal = (!problems.is_empty()).then(|| problems.join("; "));
                    }
                    Err(error) => this.refusal = Some(format!("{error:#}")),
                }
                Self::read(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Forgets the changes not applied.
    fn reset(&mut self, cx: &mut Context<Self>) {
        if let Some(setup) = self.setup() {
            self.draft = setup.outputs.clone();
            self.refusal = None;
            let on = self
                .draft
                .get(self.selected)
                .is_some_and(|output| output.active);
            self.travel.jump_to(if on { 1. } else { 0. });
            cx.notify();
        }
    }

    /// The drawing of the arrangement.
    fn canvas(&mut self, theme: &Theme, numbers: &[u32], cx: &mut Context<Self>) -> Div {
        let drawn = self.drawn();
        let areas: Vec<Area> = drawn
            .iter()
            .map(|&index| Self::area_of(&self.draft[index]))
            .collect();
        let frame = match &self.drag {
            Some(drag) => drag.frame,
            None => Frame::fitting(&areas),
        };
        let mut canvas = div()
            .flex_none()
            .relative()
            .w(px(CANVAS_WIDTH))
            .h(px(CANVAS_HEIGHT))
            .mx_auto()
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container);
        for (&index, area) in drawn.iter().zip(&areas) {
            let output = &self.draft[index];
            let chosen = index == self.selected;
            let (ground, ink) = match chosen {
                true => (theme.colors.secondary_container, theme.colors.on_surface),
                false => (
                    theme.colors.surface_container_highest,
                    theme.colors.on_surface_variant,
                ),
            };
            let (width, height) = output.size();
            let block = div()
                .id(("display-block", index))
                .absolute()
                .left(px(frame.left + area.x as f32 * frame.factor))
                .top(px(frame.top + area.y as f32 * frame.factor))
                .w(px(area.width as f32 * frame.factor))
                .h(px(area.height as f32 * frame.factor))
                .rounded(theme.shape.small)
                .border_2()
                .border_color(match chosen {
                    true => theme.colors.primary,
                    false => theme.colors.outline,
                })
                .bg(ground)
                .text_color(ink)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_size(px(20.))
                        .child(SharedString::from(numbers[index].to_string())),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .child(SharedString::from(format!("{width} × {height}"))),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.start_drag(index, event, cx)
                    }),
                );
            canvas = canvas.child(block);
        }
        canvas
    }

    /// The chips of `choices`, the one equal to `current` chosen, each
    /// doing `take` with its choice.
    fn chips<T: Copy + PartialEq + 'static>(
        &mut self,
        key: &'static str,
        choices: Vec<(T, SharedString)>,
        current: Option<T>,
        take: fn(&mut Self, T, &mut Context<Self>),
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let mut row = chip_row(&theme);
        for (index, (choice, label)) in choices.into_iter().enumerate() {
            let state = self.feedback.state((key, index), window);
            let element = chip(&theme, (key, index), label, current == Some(choice), state)
                .on_click(cx.listener(move |this, _, _, cx| take(this, choice, cx)));
            row = row.child(Feedback::track(
                element,
                (key, index),
                cx,
                |this: &mut Self| &mut this.feedback,
            ));
        }
        row
    }

    /// The settings of the display chosen.
    fn details(
        &mut self,
        theme: &Theme,
        language: Language,
        numbers: &[u32],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let Some(output) = self.draft.get(self.selected).cloned() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        let name = fill(
            language.say("Display {}"),
            &[&numbers[self.selected].to_string()],
        );
        let mirrored = { output.mirror_of }
            .and_then(|main| self.draft.iter().position(|other| other.id == main));
        let note = match (output.active, mirrored) {
            (false, _) => language.say("Off").to_owned(),
            (true, Some(main)) => fill(
                language.say("Shows the same as display {}"),
                &[&numbers[main].to_string()],
            ),
            (true, None) if output.is_primary() => language.say("Main display").to_owned(),
            (true, None) => String::new(),
        };
        let key = ("display-on", 0);
        let state = self.feedback.state(key, window);
        let travel = self.travel.value(window);
        let header = div()
            .id("display-header")
            .flex_none()
            .px(theme.spacing.medium)
            .py(theme.spacing.small)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container_highest)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(one_line(name)))
                    .child(
                        div()
                            .text_color(theme.colors.on_surface_variant)
                            .child(one_line(note)),
                    ),
            )
            .child(switch(theme, "display-switch", travel, state))
            .on_click(cx.listener(move |this, _, _, cx| {
                let on = !this.draft[this.selected].active;
                this.set_active(on, cx);
            }));
        rows.push(
            Feedback::track(header, key, cx, |this: &mut Self| &mut this.feedback)
                .into_any_element(),
        );
        if !output.active {
            let text = "Windows chooses the mode and the place of a display that is switched on; they can be changed once it is on.";
            rows.push(note_line(theme, language.say(text)).into_any_element());
            return rows;
        }
        if mirrored.is_some() {
            return rows;
        }
        if !output.is_primary() {
            let key = ("make-primary", 0);
            let state = self.feedback.state(key, window);
            let label = language.say("Make this the main display");
            let element = chip(theme, key, label, false, state)
                .on_click(cx.listener(|this, _, _, cx| this.make_primary(cx)));
            rows.push(
                chip_row(theme)
                    .child(Feedback::track(element, key, cx, |this: &mut Self| {
                        &mut this.feedback
                    }))
                    .into_any_element(),
            );
        }
        let size = (output.mode.width, output.mode.height);
        let sizes: Vec<((u32, u32), SharedString)> = { resolutions(&output.modes).into_iter() }
            .map(|(width, height)| ((width, height), format!("{width} × {height}").into()))
            .collect();
        if !sizes.is_empty() {
            rows.push(list_title(theme, language.say("Resolution")).into_any_element());
            rows.push(
                self.chips("resolution", sizes, Some(size), Self::set_size, window, cx)
                    .into_any_element(),
            );
        }
        let rates: Vec<(u32, SharedString)> = { rates_at(&output.modes, size).into_iter() }
            .map(|hertz| (hertz, hertz_text(hertz).into()))
            .collect();
        if !rates.is_empty() {
            rows.push(list_title(theme, language.say("Refresh rate")).into_any_element());
            let take = |this: &mut Self, hertz: u32, cx: &mut Context<Self>| {
                this.edit(|output| output.mode.hertz = hertz, cx)
            };
            rows.push(
                self.chips("refresh", rates, Some(output.mode.hertz), take, window, cx)
                    .into_any_element(),
            );
        }
        if let Some(scale) = &output.scale {
            let scales: Vec<(u32, SharedString)> = { scale.choices.iter() }
                .map(|&percent| {
                    let text = match percent == scale.recommended {
                        true => fill(language.say("{}% (recommended)"), &[&percent.to_string()]),
                        false => format!("{percent}%"),
                    };
                    (percent, text.into())
                })
                .collect();
            rows.push(list_title(theme, language.say("Scale")).into_any_element());
            let take = |this: &mut Self, percent: u32, cx: &mut Context<Self>| {
                this.edit(
                    |output| {
                        if let Some(scale) = &mut output.scale {
                            scale.current = percent;
                        }
                    },
                    cx,
                )
            };
            rows.push(
                self.chips("scale", scales, Some(scale.current), take, window, cx)
                    .into_any_element(),
            );
        }
        let rotations: Vec<(Rotation, SharedString)> = { Rotation::ALL.into_iter() }
            .map(|rotation| (rotation, language.say(rotation_label(rotation)).into()))
            .collect();
        rows.push(list_title(theme, language.say("Orientation")).into_any_element());
        let take = |this: &mut Self, rotation: Rotation, cx: &mut Context<Self>| {
            this.edit(|output| output.rotation = rotation, cx)
        };
        rows.push(
            self.chips(
                "rotation",
                rotations,
                Some(output.rotation),
                take,
                window,
                cx,
            )
            .into_any_element(),
        );
        rows
    }

    /// A button under the settings.
    fn button(
        &mut self,
        key: &'static str,
        label: &'static str,
        Press { prominent, usable }: Press,
        take: fn(&mut Self, &mut Context<Self>),
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let state = self.feedback.state((key, 0), window);
        let element = choice_button(&theme, label, prominent, state)
            .opacity(if usable { 1. } else { 0.5 })
            .id(key);
        if !usable {
            return element;
        }
        let element = element.on_click(cx.listener(move |this, _, _, cx| take(this, cx)));
        Feedback::track(element, (key, 0), cx, |this: &mut Self| &mut this.feedback)
    }
}

/// How a button looks: whether it is the one Enter would press, and
/// whether it can be pressed now.
#[derive(Clone, Copy)]
struct Press {
    prominent: bool,
    usable: bool,
}

impl Render for DisplaySettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut frame = div()
            .id(NAME)
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .overflow_y_scroll()
            .on_mouse_move(
                cx.listener(|this, event: &MouseMoveEvent, _, cx| this.drag_to(event, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.end_drag(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.end_drag(cx)),
            )
            .child(
                div()
                    .flex_none()
                    .px(theme.spacing.small)
                    .pb(theme.spacing.tiny)
                    .font_weight(FontWeight::MEDIUM)
                    .child(one_line(language.say("Display settings"))),
            );
        if let Some(left) = self.trial.read(cx).seconds_left() {
            let text = fill(
                language.say("Keep these display settings? The old ones come back in {} seconds."),
                &[&left.to_string()],
            );
            let keep = self.button(
                "keep",
                language.say("Keep changes"),
                Press {
                    prominent: true,
                    usable: true,
                },
                |this, cx| this.trial.update(cx, Trial::keep),
                window,
                cx,
            );
            let revert = self.button(
                "revert",
                language.say("Revert"),
                Press {
                    prominent: false,
                    usable: true,
                },
                |this, cx| this.trial.update(cx, Trial::revert),
                window,
                cx,
            );
            frame = frame.child(
                div()
                    .flex_none()
                    .p(theme.spacing.medium)
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.small)
                    .rounded(theme.shape.medium)
                    .bg(theme.colors.secondary_container)
                    .child(div().font_weight(FontWeight::MEDIUM).child(text))
                    .child(
                        div()
                            .flex()
                            .gap(theme.spacing.small)
                            .child(keep)
                            .child(revert),
                    ),
            );
        }
        match &self.read {
            None => return frame.child(note_line(&theme, language.say("Reading…"))),
            Some(Err(error)) => {
                let text = fill(language.say("The displays cannot be read: {}"), &[error]);
                return frame.child(note_line(&theme, text));
            }
            Some(Ok(_)) => {}
        }
        let numbers = numbers(&self.draft);
        frame = frame
            .child(self.canvas(&theme, &numbers, cx))
            .child(note_line(
                &theme,
                language.say("Drag the displays to arrange them; a click chooses one."),
            ));
        if self.draft.len() > 1 {
            let choices: Vec<(usize, SharedString)> = { (0..self.draft.len()).zip(&numbers) }
                .map(|(index, number)| {
                    let mut name = fill(language.say("Display {}"), &[&number.to_string()]);
                    if !self.draft[index].active {
                        name = format!("{name} · {}", language.say("Off"));
                    }
                    (index, name.into())
                })
                .collect();
            let selected = Some(self.selected);
            frame = frame.child(self.chips("display", choices, selected, Self::select, window, cx));
        }
        let details = self.details(&theme, language, &numbers, window, cx);
        frame = frame.children(details);
        if let Some(refusal) = &self.refusal {
            frame = frame.child(note_line(&theme, refusal.clone()));
        }
        let changed = self.changed() && !self.applying;
        let apply = self.button(
            "apply",
            language.say("Apply"),
            Press {
                prominent: true,
                usable: changed,
            },
            Self::apply,
            window,
            cx,
        );
        let reset = self.button(
            "reset",
            language.say("Cancel"),
            Press {
                prominent: false,
                usable: changed,
            },
            Self::reset,
            window,
            cx,
        );
        frame = frame.child(
            div()
                .flex_none()
                .pt(theme.spacing.small)
                .flex()
                .justify_end()
                .gap(theme.spacing.small)
                .child(reset)
                .child(apply),
        );
        let key = ("settings-link", 0);
        let state = self.feedback.state(key, window);
        let link = SETTINGS.link(&theme, language, state);
        frame.child(Feedback::track(link, key, cx, |this: &mut Self| {
            &mut this.feedback
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(x: i32, y: i32, width: i32, height: i32) -> Area {
        Area {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn an_edge_near_another_snaps_to_it() {
        let main = area(0, 0, 1920, 1080);
        let near = area(1935, 12, 1280, 1024);
        assert_eq!(snap(near, &[main], 40), area(1920, 0, 1280, 1024));
        let far = area(2100, 300, 1280, 1024);
        assert_eq!(snap(far, &[main], 40), far);
    }

    #[test]
    fn a_display_dropped_on_another_is_put_beside_it() {
        let main = area(0, 0, 1920, 1080);
        let dropped = area(1700, 100, 1280, 1024);
        let out = push_out(dropped, &[main]);
        assert!(!out.overlaps(main));
        assert_eq!(out, area(1920, 100, 1280, 1024));
    }

    #[test]
    fn a_display_left_apart_is_brought_against_the_nearest() {
        let main = area(0, 0, 1920, 1080);
        let apart = area(2500, 2000, 1280, 1024);
        let attached = attach(apart, &[main]);
        assert!(attached.touches(main), "{attached:?}");
        assert!(!attached.overlaps(main));
        let beside = area(1920, 0, 1280, 1024);
        assert_eq!(attach(beside, &[main]), beside);
    }

    #[test]
    fn corners_alone_do_not_touch() {
        let main = area(0, 0, 100, 100);
        assert!(!area(100, 100, 50, 50).touches(main));
        assert!(area(100, 99, 50, 50).touches(main));
        assert!(area(0, 100, 50, 50).touches(main));
    }

    #[test]
    fn the_main_display_stays_at_the_origin() {
        let mut areas = [area(-1280, 0, 1280, 1024), area(0, 0, 1920, 1080)];
        normalise(&mut areas, 0);
        assert_eq!(areas[0], area(0, 0, 1280, 1024));
        assert_eq!(areas[1], area(1280, 0, 1920, 1080));
    }

    #[test]
    fn a_larger_display_pushes_the_others_out_of_its_way() {
        // The first grew from 1920 wide and now covers the second.
        let mut areas = [area(0, 0, 2560, 1440), area(1920, 0, 1920, 1080)];
        tidy(&mut areas, 0, 0);
        assert!(!areas[0].overlaps(areas[1]));
        assert!(areas[0].touches(areas[1]));
        assert_eq!(areas[0], area(0, 0, 2560, 1440));
    }

    #[test]
    fn a_display_dropped_apart_is_brought_back_and_the_rest_stay() {
        let mut areas = [
            area(0, 0, 1920, 1080),
            area(1920, 0, 1920, 1080),
            area(500, 3000, 800, 600),
        ];
        settle(&mut areas, 2, 0);
        assert_eq!(areas[0], area(0, 0, 1920, 1080));
        assert_eq!(areas[1], area(1920, 0, 1920, 1080));
        assert!(areas[2].touches(areas[0]) || areas[2].touches(areas[1]));
    }

    #[test]
    fn a_display_switched_off_leaves_no_gap() {
        // The middle one of three went off.
        let mut areas = [area(0, 0, 1920, 1080), area(3840, 0, 1920, 1080)];
        tidy(&mut areas, 0, 0);
        assert_eq!(areas[1], area(1920, 0, 1920, 1080));
    }

    #[test]
    fn displays_that_are_off_get_the_next_numbers() {
        let output = |number| {
            let mut output = DisplayOutput::test_output();
            output.number = number;
            output
        };
        let outputs = [output(Some(2)), output(None), output(Some(1)), output(None)];
        assert_eq!(numbers(&outputs), vec![2, 3, 1, 4]);
    }

    #[test]
    fn rates_and_resolutions_come_from_the_modes() {
        let mode = |width, height, hertz| VideoMode {
            width,
            height,
            hertz,
        };
        let modes = [
            mode(2560, 1440, 144),
            mode(2560, 1440, 60),
            mode(1920, 1080, 60),
            mode(1920, 1080, 59),
        ];
        assert_eq!(resolutions(&modes), vec![(2560, 1440), (1920, 1080)]);
        assert_eq!(rates_at(&modes, (1920, 1080)), vec![60, 59]);
        assert!(rates_at(&modes, (800, 600)).is_empty());
    }

    #[test]
    fn the_drawing_fits_the_displays() {
        let areas = [area(0, 0, 1920, 1080), area(1920, 0, 1920, 1080)];
        let frame = Frame::fitting(&areas);
        let width = 3840. * frame.factor;
        assert!(width <= CANVAS_WIDTH - CANVAS_MARGIN * 2. + 0.5);
        let left = frame.left;
        let right = frame.left + width;
        assert!((left - (CANVAS_WIDTH - right)).abs() < 0.5);
    }
}
