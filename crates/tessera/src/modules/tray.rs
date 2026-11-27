// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The icons applications keep in the notification area. Widget: `tray`.
//! Action: `tray-focus` (Win+B where the keys of Windows are in use),
//! which puts the keyboard on the strip.
//!
//! The strip shows the icons that stand in the bar, in their order, and a
//! button at its end that opens the overflow: a grid of the other icons.
//! An icon is dragged to another place in the strip, onto the button or
//! into the overflow, and back. Shift with the right button, or the middle
//! button, opens Tessera's own menu of an icon, which moves it between the
//! bar and the overflow; the right button alone is the application's. On
//! a bar that is higher than wide the strip is a column, two side by side
//! where the bar is wide enough for them.

mod model;
mod overflow;

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, Bounds, Context, DispatchPhase, Entity, EntityId,
    FocusHandle, Global, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseUpEvent,
    ParentElement, Pixels, Point, Render, SharedString, Stateful, StatefulInteractiveElement,
    Styled, Subscription, WeakEntity, Window, canvas, div, img, px,
};

use crate::actions::{Action, Actions};
use crate::i18n::Language;
use crate::icons::Icon;
use crate::menu::{MenuEntry, Menus};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, InteractionState, Spring, mix};
use crate::platform::{AppWindowId, NativeSurface, ScreenRect, Tray, TrayButton, pointer_at};
use crate::popup::{MARGIN, PopupAnchor};
use crate::theme::Theme;
use crate::tip::Tips;
use crate::tray_settings::{Place, TraySettings};
use crate::ui::{Pick, icon, moved, state_layer};
use model::{KeyboardBack, TrayModel};
use overflow::{Opening, TrayOverflow};

/// The side of an icon, as a share of the height of a pill.
const ICON_SHARE: f32 = 0.5;

/// The arrows on the button of the overflow, pointing where it opens: up,
/// down, left and right.
const ICON_UP: &str = "\u{e70e}";
const ICON_DOWN: &str = "\u{e70d}";
const ICON_LEFT: &str = "\u{e76b}";
const ICON_RIGHT: &str = "\u{e76c}";

/// On the entries of an icon's own menu.
const ICON_TO_BAR: &str = "\u{e718}";
const ICON_TO_OVERFLOW: &str = "\u{e77a}";

/// How much a pressed icon shrinks, as a share of its size.
const PRESS_SHRINK: f32 = 0.16;

/// How much of an icon shows while it is dragged elsewhere.
const DRAGGED_OPACITY: f32 = 0.35;

/// The longest description shown, in characters.
const TIP_LENGTH: usize = 64;

/// About how wide a character of a description is, to open it beside a
/// bar that is higher than wide.
const TIP_CHARACTER: f32 = 7.;

/// How wide a menu is, to open it beside such a bar.
const MENU_WIDTH: f32 = 208.;

/// What the tips of the icons are told apart from other tips by.
const TIP_KIND: &str = "tray";

/// The tip of the button of the overflow.
const TIP_MORE: (&str, u64) = ("tray-more", 0);

/// The name of the action that puts the keyboard on the strip.
const FOCUS_ACTION: &str = "tray-focus";

pub struct TrayModule;

impl Module for TrayModule {
    fn name(&self) -> &'static str {
        "tray"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("tray", |context, cx| {
            // Earlier the option named the side the description unrolled
            // at; a layout that still does wants descriptions.
            let named_side = context.options.get("tips").is_some_and(toml::Value::is_str);
            let tips = named_side || context.flag("tips").unwrap_or(true);
            let limit = context.number("limit").map(|limit| limit.max(0.) as usize);
            let bluetooth = context.flag("bluetooth").unwrap_or(false);
            cx.new(|cx| TrayStrip::new(tips, limit, bluetooth, cx))
                .into()
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        // Explorer keeps Win+B to itself while it runs.
        let focus = Action::new(|_, cx| TrayStrip::focus_one(cx))
            .taking_reserved_keys()
            .by_default("win+b", None);
        actions.register(FOCUS_ACTION, focus)
    }
}

/// An icon being dragged: which, and from where.
#[derive(Clone, Copy, Debug)]
struct TrayDrag {
    id: u64,
}

/// What follows the pointer while an icon is dragged.
struct Ghost {
    image: Icon,
    side: Pixels,
}

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match self.image.clone() {
            Some(image) => img(image).size(self.side).into_any_element(),
            None => div().size(self.side).into_any_element(),
        }
    }
}

/// How the places a dragged icon can go to are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Row,
    Column,
    /// Rows of several, read row by row.
    Grid,
}

/// A rectangle in logical pixels of a window.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Slot {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Slot {
    fn of(bounds: Bounds<Pixels>) -> Self {
        let length = |length: Pixels| length / px(1.);
        Self {
            left: length(bounds.origin.x),
            top: length(bounds.origin.y),
            right: length(bounds.origin.x + bounds.size.width),
            bottom: length(bounds.origin.y + bounds.size.height),
        }
    }

    fn middle(self) -> (f32, f32) {
        ((self.left + self.right) / 2., (self.top + self.bottom) / 2.)
    }
}

/// Where among `slots`, laid out as `layout`, an icon dropped at `point`
/// goes: before how many of them.
fn insertion(slots: &[Slot], (x, y): (f32, f32), layout: Layout) -> usize {
    let before = |slot: &&Slot| {
        let (middle_x, middle_y) = slot.middle();
        match layout {
            Layout::Row => middle_x < x,
            Layout::Column => middle_y < y,
            Layout::Grid => slot.bottom <= y || (slot.top <= y && middle_x < x),
        }
    };
    slots.iter().filter(before).count()
}

/// `old`, the icons a strip showed, merged with `new`, those it shows now
/// in their order: each with whether it is leaving. One that leaves keeps
/// its place after the icon it followed, so that it shrinks where it was.
fn merged(old: &[(u64, bool)], new: &[u64]) -> Vec<(u64, bool)> {
    let mut merged: Vec<(u64, bool)> = new.iter().map(|&id| (id, false)).collect();
    let mut anchor = None;
    for &(id, _) in old {
        if new.contains(&id) {
            anchor = Some(id);
            continue;
        }
        let at = match anchor {
            Some(anchor) => merged
                .iter()
                .position(|&(id, _)| id == anchor)
                .map_or(0, |at| at + 1),
            None => 0,
        };
        // Several that leave after one icon keep their order.
        let at = merged[at..]
            .iter()
            .position(|&(_, leaving)| !leaving)
            .map_or(merged.len(), |offset| at + offset);
        merged.insert(at, (id, true));
        anchor = Some(id);
    }
    merged
}

/// The side of its display a strip stands at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

impl Side {
    /// The side of `screen` that `surface` stands at: a surface higher than
    /// wide stands at the left or right, others at the top or bottom.
    fn of(surface: ScreenRect, screen: ScreenRect) -> Self {
        let vertical = surface.bottom - surface.top > surface.right - surface.left;
        match vertical {
            true if surface.left + surface.right < screen.left + screen.right => Self::Left,
            true => Self::Right,
            false if surface.top + surface.bottom < screen.top + screen.bottom => Self::Top,
            false => Self::Bottom,
        }
    }

    fn is_vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    /// The arrow that points away from this side.
    fn arrow(self) -> &'static str {
        match self {
            Self::Top => ICON_DOWN,
            Self::Bottom => ICON_UP,
            Self::Left => ICON_RIGHT,
            Self::Right => ICON_LEFT,
        }
    }
}

/// Where the surface of a strip is on the screen, to tell where its icons
/// are there.
#[derive(Clone, Copy, Debug)]
struct SurfacePlace {
    surface: ScreenRect,
    screen: ScreenRect,
    side: Side,
    /// Physical pixels per logical one.
    scale: f32,
}

impl SurfacePlace {
    fn of_window(window: &Window) -> Option<Self> {
        let native = NativeSurface::of(window).ok()?;
        let (surface, screen) = (native.frame().ok()?, native.display_frame().ok()?);
        Some(Self {
            surface,
            screen,
            side: Side::of(surface, screen),
            scale: window.scale_factor(),
        })
    }

    /// The frame on the screen of what was drawn at `bounds` in the
    /// surface, and the point where a menu of it belongs: beside its
    /// middle, at the edge of the surface that faces the display.
    fn of(self, bounds: Bounds<Pixels>) -> (ScreenRect, (i32, i32)) {
        let frame = self.frame_of(bounds);
        let (x, y) = middle(frame);
        let menu = match self.side {
            Side::Top => (x, self.surface.bottom),
            Side::Bottom => (x, self.surface.top),
            Side::Left => (self.surface.right, y),
            Side::Right => (self.surface.left, y),
        };
        (frame, menu)
    }

    /// The frame on the screen of what was drawn at `bounds`.
    fn frame_of(self, bounds: Bounds<Pixels>) -> ScreenRect {
        let physical = |length: Pixels| (length / px(1.) * self.scale).round() as i32;
        ScreenRect {
            left: self.surface.left + physical(bounds.origin.x),
            top: self.surface.top + physical(bounds.origin.y),
            right: self.surface.left + physical(bounds.origin.x + bounds.size.width),
            bottom: self.surface.top + physical(bounds.origin.y + bounds.size.height),
        }
    }

    /// The point `point` of the screen in logical pixels of the surface,
    /// if it is on it.
    fn local(self, (x, y): (i32, i32)) -> Option<(f32, f32)> {
        contains(self.surface, (x, y)).then(|| {
            (
                (x - self.surface.left) as f32 / self.scale,
                (y - self.surface.top) as f32 / self.scale,
            )
        })
    }

    /// Where something `width` logical pixels wide opens beside the icon at
    /// `bounds` of a surface that is higher than wide: its panel starts at
    /// the edge of the surface that faces the display, level with the icon.
    fn beside(self, bounds: Bounds<Pixels>, width: f32, window: &Window, cx: &App) -> PopupAnchor {
        let frame = self.frame_of(bounds);
        let half = ((width / 2. + MARGIN) * self.scale).round() as i32;
        let (edge, x) = match self.side {
            Side::Right => (self.surface.left, self.surface.left - half),
            _ => (self.surface.right, self.surface.right + half),
        };
        PopupAnchor {
            surface: ScreenRect {
                left: edge,
                top: frame.top,
                right: edge,
                bottom: frame.top,
            },
            edge: crate::platform::ScreenEdge::Top,
            x,
            screen: self.screen,
            scale: self.scale,
            display_id: window.display(cx).map(|display| display.id()),
            display: NativeSurface::of(window).ok().map(NativeSurface::display),
        }
    }

    /// Where something `width` logical pixels wide opens for the icon at
    /// `bounds`: under or over it on a bar that is wider than high, beside
    /// it on one that is higher than wide.
    fn anchor(
        self,
        bounds: Bounds<Pixels>,
        width: f32,
        window: &Window,
        cx: &App,
    ) -> anyhow::Result<PopupAnchor> {
        match self.side.is_vertical() {
            true => Ok(self.beside(bounds, width, window, cx)),
            false => PopupAnchor::at(window, bounds.origin.x + bounds.size.width / 2., cx),
        }
    }
}

/// The middle of `frame`.
fn middle(frame: ScreenRect) -> (i32, i32) {
    (
        i32::midpoint(frame.left, frame.right),
        i32::midpoint(frame.top, frame.bottom),
    )
}

/// Whether `point` is on `frame`.
fn contains(frame: ScreenRect, (x, y): (i32, i32)) -> bool {
    (frame.left..frame.right).contains(&x) && (frame.top..frame.bottom).contains(&y)
}

/// The point of the screen where the event `event` happened in `window`.
fn screen_point(window: &Window, position: Point<Pixels>) -> Option<(i32, i32)> {
    let frame = NativeSurface::of(window).ok()?.frame().ok()?;
    let scale = window.scale_factor();
    let physical = |length: Pixels| (length / px(1.) * scale).round() as i32;
    Some((
        frame.left + physical(position.x),
        frame.top + physical(position.y),
    ))
}

/// The first line of an icon's description, shortened to fit a bar.
fn tip_line(tip: &str) -> String {
    let line = tip.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= TIP_LENGTH {
        return line.to_owned();
    }
    let kept: String = line.chars().take(TIP_LENGTH - 1).collect();
    format!("{}…", kept.trim_end())
}

/// The side of an icon's button and of the button of the overflow.
fn button_side(theme: &Theme) -> Pixels {
    theme.bar.pill_height * ICON_SHARE + theme.spacing.small
}

/// What the views that show icons do in common: a strip and the overflow.
trait IconHost: Render + Sized {
    fn model(&self) -> &Entity<TrayModel>;

    /// Opens Tessera's own menu of the icon `id`, drawn at `bounds`.
    fn open_own_menu(
        &mut self,
        id: u64,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// Where the icon `id` was drawn.
    fn drawn(&self, id: u64) -> Option<Bounds<Pixels>>;
}

/// The entries of Tessera's own menu of the icon `id`: to the bar if it is
/// in the overflow, and to the overflow if it is in the bar.
fn own_menu_entries(model: &Entity<TrayModel>, id: u64, shown: bool, cx: &App) -> Vec<MenuEntry> {
    let language = Language::of(cx);
    let model = model.downgrade();
    let (glyph, label, to) = match shown {
        true => (ICON_TO_OVERFLOW, "Move to overflow", Place::Overflow),
        false => (ICON_TO_BAR, "Show in bar", Place::Bar),
    };
    vec![MenuEntry::new(glyph, language.say(label), move |cx| {
        let moved = model
            .upgrade()
            .and_then(|model| model.read(cx).moved(id, to));
        if let Some(config) = moved {
            TraySettings::change(config, cx);
        }
    })]
}

/// The button of the icon `id` with `image`: a click and the right button
/// go to the application, Shift with the right button and the middle
/// button open Tessera's own menu, and it can be dragged. `focused` rings
/// it while the keyboard is on it.
fn icon_button<V: IconHost>(
    id: u64,
    image: Icon,
    state: InteractionState,
    focused: bool,
    theme: &Theme,
    cx: &mut Context<V>,
) -> Stateful<gpui::Div> {
    let side = theme.bar.pill_height * ICON_SHARE;
    let icon_side = side * (1. - PRESS_SHRINK * state.press);
    let ghost = image.clone();
    div()
        .id(("tray-icon", id as usize))
        .size(button_side(theme))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.shape.full)
        .border_2()
        .border_color(match focused {
            true => theme.colors.primary,
            false => gpui::transparent_black(),
        })
        .bg(state_layer(
            theme,
            theme.on_bar(theme.colors.surface_container),
            state,
        ))
        .on_click(move |event, _, cx| {
            Tips::close(cx);
            let button = match event.click_count() {
                2 => TrayButton::Double,
                _ => TrayButton::Left,
            };
            Tray::press(id, button);
        })
        .on_mouse_up(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                Tips::close(cx);
                match (event.modifiers.shift, this.drawn(id)) {
                    (true, Some(bounds)) => this.open_own_menu(id, bounds, window, cx),
                    _ => Tray::press(id, TrayButton::Right),
                }
            }),
        )
        .on_mouse_up(
            MouseButton::Middle,
            cx.listener(move |this, _, window, cx| {
                Tips::close(cx);
                if let Some(bounds) = this.drawn(id) {
                    this.open_own_menu(id, bounds, window, cx);
                }
            }),
        )
        .on_drag(TrayDrag { id }, move |_, _, _, cx| {
            Tips::close(cx);
            let image = ghost.clone();
            cx.new(|_| Ghost { image, side })
        })
        .child(match image {
            Some(image) => img(image).size(icon_side).into_any_element(),
            None => div()
                .size(icon_side)
                .rounded(theme.shape.small)
                .bg(theme.colors.outline)
                .into_any_element(),
        })
}

/// A canvas the size of what holds it that tells `drawn` where that is,
/// and runs `moved` when it moved.
fn measure(
    drawn: Rc<Cell<Option<Bounds<Pixels>>>>,
    moved: impl Fn(Bounds<Pixels>) + 'static,
) -> impl IntoElement {
    let measure = move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
        if drawn.replace(Some(bounds)) != Some(bounds) {
            moved(bounds);
        }
    };
    canvas(measure, |_, (), _, _| {}).absolute().size_full()
}

/// A canvas that tells the overflow of the strip `strip` when a press of
/// the pointer anywhere in the window ends, and whether it dragged an icon.
/// The window holds the pointer from the press on, so it hears of the end
/// also past its edges.
fn press_watch(strip: EntityId) -> impl IntoElement {
    let listen = move |_: Bounds<Pixels>, (), window: &mut Window, _: &mut App| {
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            let button = matches!(event.button, MouseButton::Left | MouseButton::Right);
            if phase == DispatchPhase::Capture && button {
                // Heard before the drop is, and acted on after it.
                let dragged = cx.has_active_drag();
                cx.defer(move |cx| TrayOverflow::press_ended(strip, dragged, cx));
            }
        });
    };
    canvas(|_, _, _| {}, listen).absolute().size_0()
}

/// Tells the application of the icon `id` that the keyboard chose it, or
/// asked for its menu, as `button` says.
fn press_with_keyboard(
    id: u64,
    button: TrayButton,
    model: &Entity<TrayModel>,
    strip: EntityId,
    cx: &mut App,
) {
    model.update(cx, |model, _| model.chosen_with_keyboard(id, strip));
    Tray::press(id, button);
}

/// What a key does to a strip or to the overflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyDeed {
    /// The keyboard moves by this many places.
    Move(isize),
    /// It moves by a row, up with `-1` and down with `1`.
    Row(isize),
    First,
    Last,
    /// The chosen place is taken.
    Take,
    /// The menu of the chosen icon is asked for.
    Menu,
    Leave,
}

impl KeyDeed {
    fn of(event: &KeyDownEvent) -> Option<Self> {
        let held = &event.keystroke.modifiers;
        if held.control || held.alt || held.platform {
            return None;
        }
        Some(match (event.keystroke.key.as_str(), held.shift) {
            ("left", _) => Self::Move(-1),
            ("right", _) => Self::Move(1),
            ("up", _) => Self::Row(-1),
            ("down", _) => Self::Row(1),
            ("tab", true) => Self::Move(-1),
            ("tab", false) => Self::Move(1),
            ("home", _) => Self::First,
            ("end", _) => Self::Last,
            ("enter" | "space", _) => Self::Take,
            ("f10", true) | ("menu", _) => Self::Menu,
            ("escape", _) => Self::Leave,
            _ => return None,
        })
    }

    /// The place chosen after this among `count`, `columns` to a row,
    /// coming from `chosen`.
    fn moved(self, chosen: usize, count: usize, columns: usize) -> usize {
        match self {
            Self::Move(by) => moved(chosen, count, Pick::Move(by)),
            Self::Row(by) => moved(chosen, count, Pick::Move(by * columns.max(1) as isize)),
            Self::First => moved(chosen, count, Pick::First),
            Self::Last => moved(chosen, count, Pick::Last),
            Self::Take | Self::Menu | Self::Leave => chosen,
        }
    }
}

/// The strips there are, for the action that puts the keyboard on one and
/// for drops from other windows.
#[derive(Default)]
struct Strips {
    shown: Vec<(WeakEntity<TrayStrip>, AnyWindowHandle)>,
}

impl Global for Strips {}

/// One icon of a strip and the motion it is in.
struct TrayItem {
    id: u64,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    interaction: Interaction,
    /// The icon went from the strip and is shrinking away.
    leaving: bool,
    /// Where the icon was drawn in its surface, once it has been.
    drawn: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// Shows the notification area and passes clicks on to the applications.
pub struct TrayStrip {
    model: Option<Entity<TrayModel>>,
    /// In the order shown, including icons that are still leaving.
    items: Vec<TrayItem>,
    /// The icons of the overflow, in their order.
    overflow: Vec<u64>,
    /// Whether the description of the icon under the pointer is shown.
    tips: bool,
    /// How many icons stand in the bar at most; the others are in the
    /// overflow.
    limit: Option<usize>,
    /// Whether the Bluetooth icon of Windows is shown among the others.
    bluetooth: bool,
    /// 0 while the overflow is empty, 1 while its button is shown.
    crowded: Spring,
    more: Interaction,
    /// Where the button of the overflow was drawn.
    more_drawn: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Where the strip itself was drawn in its window.
    strip_drawn: Rc<Cell<Option<Bounds<Pixels>>>>,
    place: Option<SurfacePlace>,
    focus: FocusHandle,
    /// The place the keyboard is on while the strip has it: an icon of the
    /// strip, or after them the button of the overflow.
    keyboard: Option<usize>,
    /// The window that was in front before the strip took the keyboard.
    previous: Option<AppWindowId>,
    /// The icon being dragged over the strip, and before which of its
    /// icons it would go.
    aim: Option<(u64, usize)>,
    /// The icon being dragged in the window of the strip, while one is.
    dragged: Option<u64>,
    /// The side of the button of an icon, as the theme says.
    side: Pixels,
    /// What the strip follows once it has a window.
    subscriptions: Vec<Subscription>,
}

impl IconHost for TrayStrip {
    fn model(&self) -> &Entity<TrayModel> {
        self.model.as_ref().expect("a strip with icons has a model")
    }

    fn open_own_menu(
        &mut self,
        id: u64,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(place) = self.place else {
            return;
        };
        let shown = self.items.iter().any(|item| item.id == id && !item.leaving);
        let entries = own_menu_entries(self.model(), id, shown, cx);
        match place.anchor(bounds, MENU_WIDTH, window, cx) {
            Ok(anchor) => Menus::open(entries, anchor, cx),
            Err(error) => report!("the menu of a notification icon has nowhere to open: {error:#}"),
        }
    }

    fn drawn(&self, id: u64) -> Option<Bounds<Pixels>> {
        self.items.iter().find(|item| item.id == id)?.drawn.get()
    }
}

impl TrayStrip {
    fn new(tips: bool, limit: Option<usize>, bluetooth: bool, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let model = match TrayModel::shared(cx) {
            Ok(model) => {
                cx.observe(&model, |this, _, cx| this.sync(cx)).detach();
                Some(model)
            }
            Err(error) => {
                report!("the notification area stays empty: {error}");
                None
            }
        };
        let mut strip = Self {
            model,
            items: Vec::new(),
            overflow: Vec::new(),
            tips,
            limit,
            bluetooth,
            crowded: Spring::new(motion.spatial, 0.),
            more: Interaction::new(&motion),
            more_drawn: Rc::default(),
            strip_drawn: Rc::default(),
            place: None,
            focus: cx.focus_handle(),
            keyboard: None,
            previous: None,
            aim: None,
            dragged: None,
            side: button_side(cx.global::<Theme>()),
            subscriptions: Vec::new(),
        };
        strip.sync(cx);
        strip
    }

    /// Takes over the icons from the model: those that stay keep their
    /// motion, new ones come, removed ones and those moved to the overflow
    /// start leaving.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(model) = &self.model else {
            return;
        };
        let (bar, overflow) = model.read(cx).arranged(self.bluetooth, self.limit);
        let motion = cx.global::<Theme>().motion;
        let old: Vec<(u64, bool)> = self
            .items
            .iter()
            .map(|item| (item.id, item.leaving))
            .collect();
        let mut items = std::mem::take(&mut self.items);
        for (id, leaving) in merged(&old, &bar) {
            let item = match items.iter().position(|item| item.id == id) {
                Some(index) => {
                    let mut item = items.swap_remove(index);
                    if item.leaving != leaving {
                        item.leaving = leaving;
                        item.presence.animate_to(if leaving { 0. } else { 1. });
                    }
                    item
                }
                None => {
                    let mut presence = Spring::new(motion.spatial, 0.);
                    presence.animate_to(1.);
                    TrayItem {
                        id,
                        presence,
                        interaction: Interaction::new(&motion),
                        leaving: false,
                        drawn: Rc::default(),
                    }
                }
            };
            self.items.push(item);
        }
        self.overflow = overflow;
        self.crowded
            .animate_to(if self.overflow.is_empty() { 0. } else { 1. });
        if let Some(chosen) = self.keyboard {
            self.keyboard = Some(chosen.min(self.places().saturating_sub(1)));
        }
        cx.notify();
    }

    /// The icons the strip shows, not those leaving.
    fn staying(&self) -> impl Iterator<Item = &TrayItem> {
        self.items.iter().filter(|item| !item.leaving)
    }

    /// How many places the keyboard can be on: the icons, and the button
    /// of the overflow if it is there.
    fn places(&self) -> usize {
        self.staying().count() + usize::from(!self.overflow.is_empty())
    }

    /// Does what the first render of the strip, which knows its window,
    /// has to: follow keyboard requests, the activation of the window, and
    /// list the strip among the others.
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.subscriptions.is_empty() {
            return;
        }
        let this = cx.entity().downgrade();
        let strips = &mut cx.default_global::<Strips>().shown;
        strips.retain(|(strip, _)| strip.upgrade().is_some());
        strips.push((this, window.window_handle()));

        self.subscriptions
            .push(cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() && this.keyboard.take().is_some() {
                    this.previous = None;
                    cx.notify();
                }
            }));
        if let Some(model) = self.model.clone() {
            let back = cx.subscribe_in(
                &model,
                window,
                |this, _, event: &KeyboardBack, window, cx| {
                    if event.strip == cx.entity_id() {
                        let index = this.staying().position(|item| item.id == event.id);
                        this.take_keyboard(index.unwrap_or(0), None, window, cx);
                    }
                },
            );
            self.subscriptions.push(back);
        }
    }

    /// Puts the keyboard on one strip: the one on the display the pointer
    /// is on, or else the first.
    fn focus_one(cx: &mut App) {
        let shown = cx.default_global::<Strips>().shown.clone();
        let live: Vec<_> = shown
            .into_iter()
            .filter_map(|(strip, window)| Some((strip.upgrade()?, window)))
            .collect();
        let pointer = pointer_at();
        let on_display = live.iter().find(|(_, window)| {
            let display = window.update(cx, |_, window, _| {
                NativeSurface::of(window).ok()?.display_frame().ok()
            });
            match (display.ok().flatten(), pointer) {
                (Some(display), Some(pointer)) => contains(display, pointer),
                _ => false,
            }
        });
        let Some((strip, window)) = on_display.or(live.first()).cloned() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            strip.update(cx, |strip, cx| strip.take_keyboard(0, None, window, cx));
        });
    }

    /// Takes the keyboard, on the place `index`. `previous` is the window
    /// that had the keyboard before the strip, where that is known, as when
    /// it comes back from the overflow; otherwise it is the window in front
    /// now. Windows may keep the keyboard with the window in front, and
    /// then the strip does not take it.
    fn take_keyboard(
        &mut self,
        index: usize,
        previous: Option<AppWindowId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.places() == 0 {
            return;
        }
        if !window.is_window_active() {
            let front = AppWindowId::foreground();
            if !NativeSurface::of(window).is_ok_and(|native| native.take_keyboard()) {
                return;
            }
            self.previous = previous.or(front);
        } else if previous.is_some() {
            self.previous = previous;
        }
        window.focus(&self.focus);
        self.keyboard = Some(index.min(self.places() - 1));
        cx.notify();
    }

    /// Lets the keyboard go; with `back` it goes to the window that was in
    /// front before.
    fn leave_keyboard(&mut self, back: bool, cx: &mut Context<Self>) {
        self.keyboard = None;
        if let Some(previous) = self.previous.take().filter(|_| back) {
            previous.activate();
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(chosen), Some(deed)) = (self.keyboard, KeyDeed::of(event)) else {
            return;
        };
        cx.stop_propagation();
        let count = self.places();
        let icon = self.staying().nth(chosen).map(|item| item.id);
        let columns = self.columns(window);
        match deed {
            KeyDeed::Leave => self.leave_keyboard(true, cx),
            KeyDeed::Take | KeyDeed::Menu => {
                let strip = cx.entity_id();
                let button = match deed {
                    KeyDeed::Take => TrayButton::Keyboard,
                    _ => TrayButton::KeyboardMenu,
                };
                match (icon, self.model.clone()) {
                    (Some(id), Some(model)) => {
                        self.previous = None;
                        press_with_keyboard(id, button, &model, strip, cx);
                    }
                    (None, _) if deed == KeyDeed::Take => self.open_overflow(true, window, cx),
                    _ => {}
                }
            }
            _ => {
                // Along a single row or column the arrows up and down move
                // by one.
                let vertical = window.viewport_size().height > window.viewport_size().width;
                let deed = match deed {
                    KeyDeed::Row(by) if !vertical || columns == 1 => KeyDeed::Move(by),
                    deed => deed,
                };
                self.keyboard = Some(deed.moved(chosen, count, columns));
                cx.notify();
            }
        }
    }

    /// How many icons stand side by side on a bar that is higher than
    /// wide; on one that is wider than high, as many as there are.
    fn columns(&self, window: &Window) -> usize {
        let side = self.side;
        let viewport = window.viewport_size();
        match viewport.height > viewport.width {
            true if viewport.width >= side * 2. + side / 2. => 2,
            true => 1,
            false => self.places().max(1),
        }
    }

    /// The layout of the strip in `window`.
    fn layout(&self, window: &Window) -> Layout {
        let viewport = window.viewport_size();
        match (viewport.height > viewport.width, self.columns(window)) {
            (false, _) => Layout::Row,
            (true, 1) => Layout::Column,
            (true, _) => Layout::Grid,
        }
    }

    /// Opens the overflow at its button, or closes it if it is open; with
    /// `keyboard` the keyboard goes into it.
    fn open_overflow(&mut self, keyboard: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(model), Some(place), Some(button)) =
            (self.model.clone(), self.place, self.more_drawn.get())
        else {
            return;
        };
        let strip = cx.entity().downgrade();
        if TrayOverflow::is_open_for(&strip, cx) {
            TrayOverflow::close(cx);
            return;
        }
        if TrayOverflow::closed_lately(&strip, cx) {
            return;
        }
        let opening = Opening {
            strip,
            model,
            bluetooth: self.bluetooth,
            limit: self.limit,
            button: place.frame_of(button),
            surface: place.surface,
            screen: place.screen,
            from: place.side,
            scale: place.scale,
            display_id: window.display(cx).map(|display| display.id()),
            keyboard,
            previous: self.previous.filter(|_| keyboard),
        };
        TrayOverflow::open(opening, cx);
        cx.notify();
    }

    /// The pointer came onto the icon `id`, or left it: its description
    /// comes out of the bar beside it after a moment, or goes. The
    /// application hears of it, and one that draws its own description
    /// shows that instead.
    fn tip(&self, id: u64, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        let key = (TIP_KIND, id);
        let item = self.items.iter().find(|item| item.id == id);
        let model = self.model.as_ref().map(|model| model.read(cx));
        let icon = model.and_then(|model| model.icon(id));
        let target = item
            .filter(|item| hovered && !item.leaving)
            .and_then(|item| Some((icon?, item.drawn.get()?)));
        let Some((icon, drawn)) = target else {
            Tips::leave(key, cx);
            Tray::hover(id, false);
            return;
        };
        // The strip of every display shows the icon; the application is
        // told the place in the one the pointer is on.
        let place = SurfacePlace::of_window(window);
        if let Some(place) = place {
            let (frame, menu) = place.of(drawn);
            Tray::locate(id, frame, menu);
        }
        Tray::hover(id, true);
        if icon.icon.draws_own_tip || !self.tips {
            return;
        }
        let tip = tip_line(&icon.icon.tip);
        let width = tip.chars().count() as f32 * TIP_CHARACTER;
        let anchor = match place {
            Some(place) => place.anchor(drawn, width, window, cx),
            None => PopupAnchor::at(window, drawn.origin.x + drawn.size.width / 2., cx),
        };
        match anchor {
            Ok(anchor) => Tips::request(key, tip.into(), anchor, cx),
            Err(error) => report!("a description has nowhere to open: {error:#}"),
        }
    }

    /// Where among the icons of the strip an icon dropped at `point`, in
    /// logical pixels of its window, goes.
    fn insertion_at(&self, point: (f32, f32), window: &Window) -> usize {
        let slots: Vec<Slot> = self
            .staying()
            .filter_map(|item| Some(Slot::of(item.drawn.get()?)))
            .collect();
        insertion(&slots, point, self.layout(window))
    }

    /// An icon dragged over the strip is at `position`; `None` once it is
    /// off it.
    fn aim_at(
        &mut self,
        id: u64,
        position: Option<Point<Pixels>>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let aim = position.map(|position| {
            (
                id,
                self.insertion_at((position.x / px(1.), position.y / px(1.)), window),
            )
        });
        if self.aim != aim {
            self.aim = aim;
            cx.notify();
        }
    }

    /// Puts the icon `id` at `index` of the strip.
    fn put(&mut self, id: u64, index: usize, cx: &mut Context<Self>) {
        self.aim = None;
        let Some(model) = self.model.clone() else {
            return;
        };
        // The index counts the icons of the strip without the dragged one.
        let bar: Vec<u64> = self.staying().map(|item| item.id).collect();
        let index = match bar.iter().position(|&other| other == id) {
            Some(at) if at < index => index - 1,
            _ => index,
        };
        let arranged = (bar, self.overflow.clone());
        if let Some(config) = model.read(cx).put(id, Place::Bar, index, arranged) {
            TraySettings::change(config, cx);
        }
    }

    /// An icon dragged from another window was let go at `point` of the
    /// screen, which is on this strip.
    fn dropped_from_outside(
        &mut self,
        id: u64,
        point: (i32, i32),
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(local) = self.place.and_then(|place| place.local(point)) else {
            return;
        };
        let index = self.insertion_at(local, window);
        self.put(id, index, cx);
    }

    /// Whether `point` of the screen is on the strip, not only on its bar.
    fn covers(&self, point: (i32, i32)) -> bool {
        match (self.place, self.strip_drawn.get()) {
            (Some(place), Some(drawn)) => contains(place.frame_of(drawn), point),
            _ => false,
        }
    }

    /// An icon dragged from this strip was let go off it, at `position` of
    /// its window: where that is on another strip or in the overflow, it
    /// goes there.
    fn released_outside(
        &mut self,
        id: u64,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.aim = None;
        cx.notify();
        if let Some(point) = screen_point(window, position) {
            cx.defer(move |cx| drop_at(id, point, cx));
        }
    }

    fn render_item(
        &mut self,
        index: usize,
        shown_index: Option<usize>,
        vertical: bool,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(model) = self.model.clone() else {
            return div().into_any_element();
        };
        let item = &mut self.items[index];
        let id = item.id;
        let presence = item.presence.value(window);
        let state = item.interaction.state(window);
        let image = model.read(cx).icon(id).and_then(|icon| icon.image.clone());
        let focused = shown_index.is_some() && self.keyboard == shown_index;
        let dragged = cx.has_active_drag() && self.dragged == Some(id);

        let drawn = item.drawn.clone();
        let place = self.place;
        let button = icon_button(id, image, state, focused, theme, cx).child(measure(
            drawn,
            move |bounds| {
                if let Some(place) = place {
                    let (frame, menu) = place.of(bounds);
                    Tray::locate(id, frame, menu);
                }
            },
        ));
        let button = Interaction::track_hover(
            button,
            cx,
            move |this: &mut Self| {
                let item = this.items.iter_mut().find(|item| item.id == id)?;
                Some(&mut item.interaction)
            },
            move |this, hovered, window, cx| this.tip(id, hovered, window, cx),
        );

        // The slot includes the spacing between icons, so that an icon that
        // comes or goes moves its neighbors smoothly.
        let side = button_side(theme);
        let slot = (side + theme.spacing.tiny) * presence.max(0.);
        let frame = div();
        let frame = match vertical {
            true => frame.h(slot).w(side),
            false => frame.w(slot).h(side),
        };
        frame
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .opacity(presence.clamp(0., 1.) * if dragged { DRAGGED_OPACITY } else { 1. })
            .child(button)
            .into_any_element()
    }

    /// The button that opens the overflow, there while the overflow holds
    /// icons.
    fn render_more(
        &mut self,
        side_of: Side,
        focused: bool,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let crowded = self.crowded.value(window);
        let open = TrayOverflow::is_open_for(&cx.entity().downgrade(), cx);
        let state = self.more.state(window);
        let colors = theme.colors;
        let side = button_side(theme);
        let open = if open { 1. } else { 0. };
        let ground = mix(
            theme.on_bar(colors.surface_container),
            colors.secondary_container,
            open,
        );
        let button = div()
            .id("tray-more")
            .size(side)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .border_2()
            .border_color(match focused {
                true => colors.primary,
                false => gpui::transparent_black(),
            })
            .bg(state_layer(theme, ground, state))
            .text_color(mix(colors.on_surface_variant, colors.on_surface, open))
            .on_click(cx.listener(|this, _, window, cx| {
                Tips::close(cx);
                this.open_overflow(false, window, cx);
            }))
            .on_drop(cx.listener(|this, drag: &TrayDrag, _, cx| {
                // The drop goes no further, to the strip that would forget
                // the drag otherwise.
                this.dragged = None;
                this.aim = None;
                if let Some(model) = this.model.clone() {
                    let arranged = (
                        this.staying().map(|item| item.id).collect(),
                        this.overflow.clone(),
                    );
                    if let Some(config) =
                        model
                            .read(cx)
                            .put(drag.id, Place::Overflow, usize::MAX, arranged)
                    {
                        TraySettings::change(config, cx);
                    }
                }
            }))
            .child(icon(theme, side_of.arrow()))
            .child(measure(self.more_drawn.clone(), |_| {}));
        let button = Interaction::track_hover(
            button,
            cx,
            |this: &mut Self| Some(&mut this.more),
            |this, hovered, window, cx| this.more_tip(hovered, window, cx),
        );
        let slot = (side + theme.spacing.tiny) * crowded.max(0.);
        let frame = match side_of.is_vertical() {
            true => div().h(slot).w(side),
            false => div().w(slot).h(side),
        };
        frame
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .opacity(crowded.clamp(0., 1.))
            .child(button)
            .into_any_element()
    }

    /// The pointer came onto the button of the overflow, or left it.
    fn more_tip(&self, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        let (Some(place), Some(drawn), true) =
            (self.place, self.more_drawn.get(), hovered && self.tips)
        else {
            Tips::leave(TIP_MORE, cx);
            return;
        };
        let text = Language::of(cx).say("Show hidden icons");
        let width = text.chars().count() as f32 * TIP_CHARACTER;
        if let Ok(anchor) = place.anchor(drawn, width, window, cx) {
            Tips::request(TIP_MORE, SharedString::new(text), anchor, cx);
        }
    }

    /// A line that marks where a dragged icon would go.
    fn marker(vertical: bool, theme: &Theme) -> AnyElement {
        let side = button_side(theme);
        let marker = div()
            .flex_none()
            .rounded(theme.shape.full)
            .bg(theme.colors.primary);
        match vertical {
            true => marker.h(px(2.)).w(side),
            false => marker.w(px(2.)).h(side),
        }
        .into_any_element()
    }
}

/// An icon dragged from a strip or the overflow was let go at `point` of
/// the screen, off the strip or the overflow it came from: it goes to the
/// strip or the overflow that is there, and stays where it was when the
/// point is on neither, as on the rest of a bar.
fn drop_at(id: u64, point: (i32, i32), cx: &mut App) {
    if TrayOverflow::take_drop(id, point, cx) {
        return;
    }
    let shown = cx.default_global::<Strips>().shown.clone();
    for (strip, window) in shown {
        let Some(strip) = strip.upgrade() else {
            continue;
        };
        if strip.read(cx).covers(point) {
            let _ = window.update(cx, |_, window, cx| {
                strip.update(cx, |strip, cx| {
                    strip.dropped_from_outside(id, point, window, cx)
                });
            });
            return;
        }
    }
}

impl Render for TrayStrip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.attach(window, cx);
        let theme = *cx.global::<Theme>();
        self.side = button_side(&theme);
        self.place = SurfacePlace::of_window(window);

        let viewport = window.viewport_size();
        let side_of = match self.place {
            Some(place) => place.side,
            None if viewport.height > viewport.width => Side::Left,
            None => Side::Bottom,
        };
        let vertical = viewport.height > viewport.width;
        let columns = self.columns(window);
        let aim = self
            .aim
            .filter(|_| cx.has_active_drag())
            .map(|(_, index)| index);
        let mut elements: Vec<AnyElement> = Vec::with_capacity(self.items.len() + 2);
        let mut shown_index = 0;
        for index in 0..self.items.len() {
            let staying = !self.items[index].leaving;
            if staying && aim == Some(shown_index) {
                elements.push(Self::marker(vertical, &theme));
            }
            let shown = staying.then_some(shown_index);
            elements.push(self.render_item(index, shown, vertical, &theme, window, cx));
            shown_index += usize::from(staying);
        }
        if aim.is_some_and(|aim| aim >= shown_index) {
            elements.push(Self::marker(vertical, &theme));
        }
        if !self.overflow.is_empty() || !self.crowded.is_settled() {
            let focused = self.keyboard == Some(shown_index);
            elements.push(self.render_more(side_of, focused, &theme, window, cx));
        }
        // An icon that has finished leaving was just drawn at no size.
        self.items
            .retain(|item| !(item.leaving && item.presence.is_settled()));

        if elements.is_empty() {
            return div().id("tray");
        }
        let padding = theme.spacing.small - theme.spacing.tiny / 2.;
        let root = div()
            .id("tray")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(theme.on_bar(theme.colors.surface_container))
            .on_drag_move(
                cx.listener(|this, event: &gpui::DragMoveEvent<TrayDrag>, window, cx| {
                    let id = event.drag(cx).id;
                    this.dragged = Some(id);
                    let over = event.bounds.contains(&event.event.position);
                    this.aim_at(id, over.then_some(event.event.position), window, cx);
                }),
            )
            .on_drop(cx.listener(|this, drag: &TrayDrag, window, cx| {
                this.dragged = None;
                let position = window.mouse_position();
                let index = this.insertion_at((position.x / px(1.), position.y / px(1.)), window);
                this.put(drag.id, index, cx);
            }))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    // Only a drag that is still going is let go here.
                    let dragged = this.dragged.take().filter(|_| cx.has_active_drag());
                    if let Some(id) = dragged {
                        this.released_outside(id, event.position, window, cx);
                    }
                }),
            );
        let root = match (vertical, columns) {
            (false, _) => root.h(theme.bar.pill_height).px(padding),
            (true, 1) => root.w(theme.bar.pill_height).py(padding).flex_col(),
            (true, _) => root
                .w((button_side(&theme) + theme.spacing.tiny) * columns as f32 + padding * 2.)
                .p(padding)
                .flex_wrap(),
        };
        root.child(press_watch(cx.entity_id()))
            .child(measure(self.strip_drawn.clone(), |_| {}))
            .children(elements)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(left: f32, top: f32) -> Slot {
        Slot {
            left,
            top,
            right: left + 10.,
            bottom: top + 10.,
        }
    }

    #[test]
    fn a_description_is_cut_to_one_short_line() {
        assert_eq!(tip_line(""), "");
        assert_eq!(
            tip_line(
                "Volume: 40%
Speakers"
            ),
            "Volume: 40%"
        );
        let long = "word ".repeat(20);
        let shown = tip_line(&long);
        assert_eq!(shown.chars().count(), TIP_LENGTH);
        assert!(shown.ends_with("wor…"), "got {shown:?}");
    }

    #[test]
    fn a_dropped_icon_goes_between_the_icons_it_is_dropped_between() {
        let row = [slot(0., 0.), slot(10., 0.), slot(20., 0.)];
        assert_eq!(insertion(&row, (-5., 5.), Layout::Row), 0);
        assert_eq!(insertion(&row, (12., 5.), Layout::Row), 1);
        assert_eq!(insertion(&row, (16., 5.), Layout::Row), 2);
        assert_eq!(insertion(&row, (99., 5.), Layout::Row), 3);
        let column = [slot(0., 0.), slot(0., 10.), slot(0., 20.)];
        assert_eq!(insertion(&column, (5., 16.), Layout::Column), 2);
        let grid = [slot(0., 0.), slot(10., 0.), slot(0., 10.), slot(10., 10.)];
        assert_eq!(insertion(&grid, (2., 15.), Layout::Grid), 2);
        assert_eq!(insertion(&grid, (17., 15.), Layout::Grid), 4);
        assert_eq!(insertion(&grid, (17., 2.), Layout::Grid), 2);
        assert_eq!(insertion(&grid, (0., -3.), Layout::Grid), 0);
    }

    #[test]
    fn icons_that_leave_keep_their_place() {
        assert_eq!(
            merged(&[(1, false), (2, false), (3, false)], &[1, 3]),
            vec![(1, false), (2, true), (3, false)]
        );
        assert_eq!(
            merged(&[(2, false), (1, false)], &[1, 4]),
            vec![(2, true), (1, false), (4, false)]
        );
        assert_eq!(
            merged(&[(1, false), (2, true), (5, false)], &[1]),
            vec![(1, false), (2, true), (5, true)]
        );
        assert_eq!(merged(&[], &[7]), vec![(7, false)]);
    }

    #[test]
    fn a_strip_knows_the_side_it_stands_at() {
        let screen = ScreenRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let strip = |left, top, right, bottom| {
            Side::of(
                ScreenRect {
                    left,
                    top,
                    right,
                    bottom,
                },
                screen,
            )
        };
        assert_eq!(strip(0, 0, 1920, 48), Side::Top);
        assert_eq!(strip(0, 1032, 1920, 1080), Side::Bottom);
        assert_eq!(strip(0, 0, 48, 1080), Side::Left);
        assert_eq!(strip(1872, 0, 1920, 1080), Side::Right);
    }

    #[test]
    fn keys_move_through_the_icons() {
        assert_eq!(KeyDeed::Move(1).moved(2, 3, 3), 0);
        assert_eq!(KeyDeed::Move(-1).moved(0, 3, 3), 2);
        assert_eq!(KeyDeed::Row(1).moved(1, 10, 5), 6);
        assert_eq!(KeyDeed::Last.moved(1, 4, 2), 3);
        assert_eq!(KeyDeed::Take.moved(1, 4, 2), 1);
    }

    #[test]
    fn win_b_is_taken_from_explorer() {
        let mut actions = Actions::default();
        TrayModule.actions(&mut actions).unwrap();
        assert!(actions.takes_reserved_keys(FOCUS_ACTION));
        assert_eq!(actions.default_keys()[0].press, "win+b");
    }
}
