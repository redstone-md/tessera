// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The content of an edge surface: widgets arranged in three slots, and the
//! way to arrange them by hand: while that is on, a widget is dragged to
//! another place of its surface or off the surface, and a button in every
//! slot adds one.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AnyView, App, AppContext, AsyncApp, Axis, Bounds, Context, CursorStyle,
    DispatchPhase, Div, DragMoveEvent, Global, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, canvas, div, linear_color_stop,
    linear_gradient, px,
};

use crate::bar_menu::{BarMenu, BarTarget};
use crate::config::edit::{ConfigFile, Place, Slot};
use crate::config::{SurfaceConfig, WidgetSpec};
use crate::i18n::Language;
use crate::menu::Menus;
use crate::module::WidgetRegistry;
use crate::motion::{Interaction, Spring};
use crate::platform::{
    DisplayKey, NativeSurface, ScreenEdge, ScreenRect, desktop_look, desktop_pixels, pointer_at,
};
use crate::popup::{PopupAnchor, Popups};
use crate::shell::Shell;
use crate::surface::bar_theme;
use crate::theme::{Ground, Theme};
use crate::ui::{HOVER_LAYER, PRESS_LAYER, centered_row, icon};

/// How much the width of a slot changes before the row is laid out anew.
const FIT_SLACK: Pixels = px(0.5);

const ICON_ADD: &str = "\u{e710}";
const ICON_DONE: &str = "\u{e73e}";
const ICON_DISCARD: &str = "\u{e74d}";

/// The width the first of three slots, `wide` each, is given in `room`
/// with `gap` between them where the widest side leaves the middle one no
/// room in the middle; `None` where it does. The middle slot is moved off
/// the middle only as far as the last slot needs, and no further than the
/// first one lets it.
fn lead_of(wide: [Pixels; 3], room: Pixels, gap: Pixels) -> Option<Pixels> {
    let [start, center, end] = wide;
    if center + start.max(end) * 2. <= room {
        return None;
    }
    let middle = (room - center) / 2. - gap;
    let before_end = room - end - center - gap * 2.;
    Some(middle.min(before_end).max(start))
}

/// The smallest a widget is while the widgets are arranged, so that one
/// that shows nothing at the moment can still be taken.
const HANDLE_MIN: Pixels = px(20.);
/// The size of the button that adds a widget to a slot.
const ADD_SIZE: Pixels = px(22.);
/// How wide the line is that shows where a dragged widget would go, and
/// how much of the height of the surface it takes.
const MARKER_WIDTH: Pixels = px(4.);
const MARKER_HEIGHT: f32 = 0.6;
/// How far past the surface a widget is dragged to be taken out of it.
const AWAY: Pixels = px(40.);
/// How fast the outlines of the widgets breathe, in radians a second, how
/// far apart neighbors are in that, and how strong the outline is at its
/// weakest and beyond that.
pub const BREATH: f32 = 3.2;
const BREATH_STEP: f32 = 0.8;
pub const OUTLINE_REST: f32 = 0.35;
pub const OUTLINE_SWING: f32 = 0.3;
/// How much of the outline's color fills a widget.
pub const HANDLE_FILL: f32 = 0.25;
/// How much of a widget is left to see while it is dragged.
const LIFTED: f32 = 0.35;

/// The widgets that answer a click with the right button themselves: a
/// click with the right button on one opens the menu of the bar only with
/// Shift held, or while the widgets are arranged.
const OWN_MENUS: [&str; 5] = ["windows", "tray", "trash", "wallpaper", "workspaces"];

/// The widget that takes the free room of its slot with `flexible = true`.
pub const SPACER_WIDGET: &str = "spacer";

/// The widgets that draw no text and so need no field behind them, nor a
/// cell of their own on a bar at the left or the right.
const PLAIN: [&str; 2] = [SPACER_WIDGET, "separator"];

/// How wide the fade is at a side where a widget wider than a bar at the
/// left or the right is cut.
const FADE: Pixels = px(8.);

/// Whether `spec` is a spacer that takes the free room of its slot.
pub fn is_flexible(spec: &WidgetSpec) -> bool {
    spec.widget == SPACER_WIDGET
        && spec
            .options
            .get("flexible")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false)
}

/// Whether the widgets of the surfaces are being arranged by hand.
#[derive(Default)]
pub struct Arranging(bool);

impl Global for Arranging {}

impl Arranging {
    pub fn is_on(cx: &App) -> bool {
        cx.try_global::<Self>().is_some_and(|arranging| arranging.0)
    }

    pub fn set(on: bool, cx: &mut App) {
        cx.set_global(Self(on));
        Menus::close(cx);
        cx.refresh_windows();
    }
}

/// The panels there are while the widgets are arranged, so that a widget
/// dragged off one finds the one it is over.
#[derive(Default)]
struct Arena(Vec<WeakEntity<Panel>>);

impl Global for Arena {}

/// How often a bar that lets the desktop through asks whether the
/// wallpaper changed, which is cheap.
const GROUND_CHECK: Duration = Duration::from_secs(1);

/// How long a bar goes without reading the desktop under it while the
/// wallpaper is not seen to change: reading is not cheap, and a wallpaper
/// that moves is not seen to change.
const GROUND_KEPT: Duration = Duration::from_secs(60);

/// A widget in a slot, and the popup a click on it opens.
struct Placed {
    view: AnyView,
    /// What the layout calls it.
    name: SharedString,
    /// Where it stands in its slot in the configuration file, where the
    /// widgets no module provides are counted as well.
    index: usize,
    popup: Option<String>,
    /// Whether it is a spacer that takes the free room of its slot.
    flexible: bool,
    interaction: Interaction,
    /// Where the widget is in its surface, once it has been drawn.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// How far the field has come in that a widget of a flat bar gets
    /// where the desktop under it leaves its text unread.
    field: Spring,
}

/// A widget on its way to another place.
struct Dragged {
    place: Place,
    name: SharedString,
    /// Whether letting go now takes the widget out of the layout.
    discarding: Rc<Cell<bool>>,
}

/// What follows the pointer while a widget is dragged.
struct Ghost {
    name: SharedString,
    discarding: Rc<Cell<bool>>,
}

impl Ghost {
    /// The pill that stands for the widget `name` on its way.
    fn pill(theme: &Theme, name: SharedString, discarding: bool) -> Div {
        let color = if discarding {
            theme.colors.outline
        } else {
            theme.colors.primary
        };
        div()
            .h(theme.bar.pill_height)
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .rounded(theme.shape.full)
            .bg(color)
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.surface_container)
            .whitespace_nowrap()
            .children(
                discarding
                    .then(|| icon(theme, ICON_DISCARD).text_size(theme.typography.label_size)),
            )
            .child(name)
    }
}

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        Self::pill(&theme, self.name.clone(), self.discarding.get())
    }
}

/// Where a dragged widget would go if it were let go.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Aim {
    /// Before what stands at this place of the file in this slot.
    Before(usize, usize),
    /// The same on another panel, which shows the surface of this number.
    Onto(usize, usize, usize),
    /// Out of the layout.
    Away,
}

/// Hosts the widgets a layout assigns to one surface.
pub struct Panel {
    /// Counts the surfaces in the order of the configuration file.
    surface: usize,
    /// The start, center and end slots.
    slots: [Vec<Placed>; 3],
    /// How many widgets the file has in each slot.
    counts: [usize; 3],
    /// Where each slot is in the surface, once it has been drawn.
    places: [Rc<Cell<Option<Bounds<Pixels>>>>; 3],
    /// The edge the content slides in from when the surface opens.
    edge: ScreenEdge,
    /// How far the content has come in: 0 is hidden behind the edge.
    entrance: Spring,
    /// How far what belongs to arranging has come in.
    chrome: Spring,
    /// When this content was made; the outlines breathe from then on.
    began: Instant,
    dragged: Option<Place>,
    aim: Option<Aim>,
    /// How far the line at the aim has grown.
    marker: Spring,
    discarding: Rc<Cell<bool>>,
    /// Where the window is on the screen and how many physical pixels a
    /// logical one of it takes, while the widgets are arranged.
    frame: Option<(ScreenRect, f32)>,
    /// Whether the panel is among those a dragged widget looks for.
    listed: bool,
    /// The panel the widget dragged off this one is over.
    visited: Option<WeakEntity<Panel>>,
    /// A widget dragged off another panel that is over this one, and where
    /// the pointer is.
    visitor: Option<(SharedString, Pixels)>,
    /// What the desktop under the surface looks like, where the bars are
    /// flat and kept readable.
    ground: Option<Ground>,
    /// Whether the desktop under the surface is being looked at.
    watching: bool,
    /// Whether the widgets have fields of their own, where the surface
    /// says instead of the theme.
    floating: Option<bool>,
    /// For each slot, whether its widgets are those of the file's surface
    /// and so arranged by hand; an override for the display replaced the
    /// others.
    editable: [bool; 3],
    /// For each slot, whether a spacer in it takes its free room.
    flexible: [bool; 3],
    /// How many menus had opened when the right button last went down,
    /// so that a widget that opened its own for the click is left to it.
    menus_before: Rc<Cell<u64>>,
}

impl Panel {
    /// Instantiates the widgets `layout` names, which is the surface at
    /// `surface` of the configuration as it is on its display; `replaced`
    /// says which slots an override for the display gave. Unknown names
    /// are reported and left out.
    pub fn new(
        surface: usize,
        layout: &SurfaceConfig,
        replaced: [bool; 3],
        display: Option<DisplayKey>,
        cx: &mut App,
    ) -> Self {
        let motion = cx.global::<Theme>().motion;
        let mut build = |specs: &[WidgetSpec]| -> Vec<Placed> {
            specs
                .iter()
                .enumerate()
                .filter_map(|(index, spec)| {
                    Some(Placed {
                        view: WidgetRegistry::build(spec, display, cx)?,
                        name: spec.widget.clone().into(),
                        index,
                        popup: cx.global::<WidgetRegistry>().popup_of(spec),
                        flexible: is_flexible(spec),
                        interaction: Interaction::new(&motion),
                        bounds: Rc::default(),
                        field: Spring::new(motion.effects, 0.),
                    })
                })
                .collect()
        };
        let slots = [
            build(&layout.start),
            build(&layout.center),
            build(&layout.end),
        ];
        // Content that replaces other content while the widgets are
        // arranged is there at once: it does not come in after every move.
        let arranging = Arranging::is_on(cx);
        let shown = if arranging { 1. } else { 0. };
        let mut entrance = Spring::new(motion.spatial, shown);
        entrance.animate_to(1.);
        Self {
            surface,
            slots,
            counts: [layout.start.len(), layout.center.len(), layout.end.len()],
            places: Default::default(),
            edge: layout.edge,
            entrance,
            chrome: Spring::new(motion.spatial, shown),
            began: Instant::now(),
            dragged: None,
            aim: None,
            marker: Spring::new(motion.spatial, 0.),
            discarding: Rc::default(),
            frame: None,
            listed: false,
            visited: None,
            visitor: None,
            ground: None,
            watching: false,
            floating: layout.floating,
            editable: replaced.map(|replaced| !replaced),
            flexible: [&layout.start, &layout.center, &layout.end]
                .map(|slot| slot.iter().any(is_flexible)),
            menus_before: Rc::default(),
        }
    }

    /// Whether the surface runs down its display, at the left or the right.
    fn vertical(&self) -> bool {
        self.edge.is_vertical()
    }

    /// Of a point in the surface, how far it is along the surface.
    fn along(&self, position: Point<Pixels>) -> Pixels {
        self.edge.along_of(position.x, position.y)
    }

    /// The theme as this surface has it.
    fn theme(&self, cx: &App) -> Theme {
        bar_theme(*cx.global::<Theme>(), self.floating)
    }

    /// Starts looking at the desktop under the surface, where the bars
    /// are flat and kept readable. The desktop is read off the thread of
    /// the interface: when the wallpaper is seen to change, once more a
    /// [`GROUND_CHECK`] later, since a wallpaper fades in, and every
    /// [`GROUND_KEPT`] otherwise.
    ///
    /// A bar at the left or the right is not looked at: what is read of
    /// the desktop is compared across a bar, not down it.
    fn watch_ground(&mut self, theme: &Theme, window: &Window, cx: &mut Context<Self>) {
        if self.watching || !theme.grounds_bars() || self.vertical() {
            return;
        }
        let Ok(frame) = NativeSurface::of(window).and_then(|native| native.frame()) else {
            return;
        };
        self.watching = true;
        let floating = self.floating;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut seen = None;
            let mut again = false;
            let mut read_at: Option<Instant> = None;
            let mut ground = None;
            loop {
                let wanted =
                    cx.update(|cx| bar_theme(*cx.global::<Theme>(), floating).grounds_bars());
                if matches!(wanted, Ok(true)) {
                    let look = cx.background_executor().spawn(async { desktop_look() });
                    let look = Some(look.await);
                    let changed = look != seen;
                    let kept = read_at.is_some_and(|at| at.elapsed() < GROUND_KEPT);
                    if changed || again || !kept {
                        (seen, again, read_at) = (look, changed, Some(Instant::now()));
                        let read = async move {
                            let (width, pixels) = desktop_pixels(frame)?;
                            Some(Ground::read(width, &pixels))
                        };
                        ground = cx.background_executor().spawn(read).await;
                    }
                } else {
                    (seen, read_at, ground) = (None, None, None);
                }
                let ground = ground.clone();
                let kept = this.update(cx, |this, cx| {
                    if this.ground != ground {
                        this.ground = ground;
                        cx.notify();
                    }
                });
                if kept.is_err() {
                    break;
                }
                cx.background_executor().timer(GROUND_CHECK).await;
            }
        })
        .detach();
    }

    /// The field behind a widget of a flat bar: as strong as the desktop
    /// under the widget takes from what is read on it. Nothing where the
    /// bars do not look at the desktop.
    fn field(
        placed: &mut Placed,
        ground: Option<&Ground>,
        theme: &Theme,
        window: &Window,
    ) -> Option<Div> {
        if !theme.grounds_bars() || window.viewport_size().height > window.viewport_size().width {
            return None;
        }
        let unread = ground
            .zip(placed.bounds.get())
            .filter(|(_, bounds)| bounds.size.width > px(0.))
            .is_some_and(|(ground, bounds)| {
                let scale = window.scale_factor();
                let (from, to) = (f32::from(bounds.left()), f32::from(bounds.right()));
                !theme.is_read_on(ground, from * scale, to * scale)
            });
        let wanted = if unread { 1. } else { 0. };
        if placed.field.target() != wanted {
            placed.field.animate_to(wanted);
        }
        let shown = placed.field.value(window).clamp(0., 1.);
        Some(
            div()
                .absolute()
                .size_full()
                .top_0()
                .left_0()
                .rounded(theme.shape.full)
                .bg(theme.colors.surface_container.opacity(shown)),
        )
    }

    /// The middle of the first widget that opens the popup `name`, if
    /// there is one and it has been drawn.
    pub fn popup_center(&self, name: &str) -> Option<Point<Pixels>> {
        let mut placed = self.slots.iter().flatten();
        let bounds = placed
            .find(|placed| placed.popup.as_deref() == Some(name))?
            .bounds
            .get()?;
        Some(bounds.center())
    }

    /// Opens the popup `name` at the pointer, which is on the widget that
    /// opens it.
    fn open_popup(&self, name: &str, window: &Window, cx: &mut App) {
        match PopupAnchor::at_point(window, window.mouse_position(), cx) {
            Ok(anchor) => Popups::toggle(name, anchor, cx),
            Err(error) => report!("the popup `{name}` has nowhere to open: {error:#}"),
        }
    }

    /// The widget at `index` of `slot`. One that opens a popup is wrapped in
    /// a button with a state layer over it; on a flat bar that is kept
    /// readable each has its field under it.
    fn widget(
        &mut self,
        slot: usize,
        index: usize,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let vertical = self.vertical();
        let placed = &mut self.slots[slot][index];
        let plain = PLAIN.contains(&placed.name.as_ref());
        let seen = placed.bounds.clone();
        let measure = move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
            seen.set(Some(bounds));
        };
        if plain && placed.popup.is_none() {
            return Self::plain(placed, measure, vertical);
        }
        let field = Self::field(placed, self.ground.as_ref(), theme, window);
        let element = Self::button(placed, field, measure, slot, index, theme, window, cx);
        match vertical && !plain {
            true => Self::cell(element, placed.bounds.get(), theme, window),
            false => element,
        }
    }

    /// The plain widget `placed`, with no field nor button: in a box that
    /// grows as the widget does, so that a spacer still takes the free
    /// room, and that `measure` learns where it is, for the menu of the
    /// widget and for widgets dropped next to it.
    fn plain(
        placed: &Placed,
        measure: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static,
        vertical: bool,
    ) -> AnyElement {
        let room = div().relative().flex().items_center().justify_center();
        let room = match vertical {
            true => room.flex_col().w_full(),
            false => room.h_full(),
        };
        let room = match placed.flexible {
            true => room.flex_1(),
            false => room.flex_none(),
        };
        room.child(placed.view.clone())
            .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
            .into_any_element()
    }

    /// The widget `placed`, at `index` of `slot`, over its `field` if it
    /// has one, and in a button with a state layer over it where it opens
    /// a popup; `measure` learns where it is.
    #[allow(clippy::too_many_arguments, reason = "one widget and where it is")]
    fn button(
        placed: &mut Placed,
        field: Option<Div>,
        measure: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static,
        slot: usize,
        index: usize,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(popup) = placed.popup.clone() else {
            return div()
                .relative()
                .flex_none()
                .children(field)
                .child(placed.view.clone())
                .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
                .into_any_element();
        };
        let state = placed.interaction.state(window);
        let layer = HOVER_LAYER * state.hover + PRESS_LAYER * state.press.clamp(0., 1.);
        let button = div()
            .id(("panel-widget", slot * 1000 + index))
            .relative()
            .on_click(cx.listener(move |this, _, window, cx| this.open_popup(&popup, window, cx)))
            .children(field)
            .child(placed.view.clone())
            .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
            .child(
                div()
                    .absolute()
                    .size_full()
                    .top_0()
                    .left_0()
                    .rounded(theme.shape.full)
                    .bg(theme.colors.on_surface.opacity(layer)),
            );
        Interaction::track(button, cx, move |this: &mut Self| {
            Some(&mut this.slots.get_mut(slot)?.get_mut(index)?.interaction)
        })
        .into_any_element()
    }

    /// `element`, a widget on a bar at the left or the right, in a cell as
    /// wide as the bar, in its middle. A widget wider than the bar, as it
    /// was `drawn` last, is cut at both sides behind a short fade into the
    /// color of the fields; on a flat bar, which has no fields, it is cut
    /// without one.
    fn cell(
        element: AnyElement,
        drawn: Option<Bounds<Pixels>>,
        theme: &Theme,
        window: &Window,
    ) -> AnyElement {
        let wide = drawn.is_some_and(|drawn| drawn.size.width > window.viewport_size().width);
        let color = theme.on_bar(theme.colors.surface_container);
        let fade = |angle: f32| {
            div()
                .absolute()
                .top_0()
                .h_full()
                .w(FADE)
                .bg(linear_gradient(
                    angle,
                    linear_color_stop(color, 0.),
                    linear_color_stop(color.opacity(0.), 1.),
                ))
        };
        let fades = wide.then(|| [fade(90.).left_0(), fade(270.).right_0()]);
        div()
            .relative()
            .w_full()
            .flex()
            .flex_none()
            .justify_center()
            .overflow_hidden()
            .child(element)
            .children(fades.into_iter().flatten())
            .into_any_element()
    }

    /// The widget at `index` of `slot` while the widgets are arranged: it
    /// does nothing by itself, and is taken and dragged by the outline
    /// over it, which has come in as far as `chrome`.
    fn handle(
        &mut self,
        slot: usize,
        index: usize,
        theme: &Theme,
        chrome: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let placed = &self.slots[slot][index];
        let place = Place {
            surface: self.surface,
            slot: Slot::ALL[slot],
            index: placed.index,
        };
        let lifted = self.dragged == Some(place);
        let breath =
            self.began.elapsed().as_secs_f32() * BREATH + (slot * 3 + index) as f32 * BREATH_STEP;
        let strength = (OUTLINE_REST + OUTLINE_SWING * (0.5 + 0.5 * breath.sin())) * chrome;
        let color = if lifted && self.aim == Some(Aim::Away) {
            theme.colors.outline
        } else {
            theme.colors.primary
        };
        let seen = placed.bounds.clone();
        let measure = move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
            seen.set(Some(bounds));
        };
        let dragged = Dragged {
            place,
            name: placed.name.clone(),
            discarding: self.discarding.clone(),
        };
        let panel = cx.entity().downgrade();
        // The widgets of a slot an override gave for the display are
        // arranged in the file: they stay where they are.
        let editable = self.editable[slot];
        let strength = if editable { strength } else { strength / 2. };
        let outline = div()
            .id(("panel-handle", slot * 1000 + index))
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .rounded(theme.shape.full)
            .border_1()
            .border_color(color.opacity(strength.clamp(0., 1.)))
            .bg(color.opacity((strength * HANDLE_FILL).clamp(0., 1.)))
            .occlude()
            // The outline keeps the pointer from the bar under it, so it
            // opens the menu of its widget itself.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.context_menu(event, window, cx);
                }),
            );
        if !editable {
            return div()
                .relative()
                .flex_none()
                .min_w(HANDLE_MIN)
                .min_h(HANDLE_MIN)
                .child(placed.view.clone())
                .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
                .child(outline)
                .into_any_element();
        }
        let outline = outline.cursor(CursorStyle::OpenHand).on_drag(
            dragged,
            move |dragged: &Dragged, _, _, cx| {
                let place = dragged.place;
                let taken = panel.update(cx, |this, cx| {
                    this.dragged = Some(place);
                    cx.notify();
                });
                taken.ok();
                let (name, discarding) = (dragged.name.clone(), dragged.discarding.clone());
                cx.new(|_| Ghost { name, discarding })
            },
        );
        div()
            .relative()
            .flex_none()
            .min_w(HANDLE_MIN)
            .min_h(HANDLE_MIN)
            .opacity(if lifted { LIFTED } else { 1. })
            .child(placed.view.clone())
            .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
            .child(outline)
            .into_any_element()
    }

    /// The line that shows where the dragged widget would go: across the
    /// surface, as it is between two widgets.
    fn marker(theme: &Theme, grown: f32, vertical: bool, window: &Window) -> AnyElement {
        let grown = grown.clamp(0., 1.);
        let viewport = window.viewport_size();
        let line = div()
            .flex_none()
            .rounded(theme.shape.full)
            .bg(theme.colors.primary.opacity(grown));
        match vertical {
            true => line
                .h(MARKER_WIDTH)
                .w(viewport.width * MARKER_HEIGHT * grown),
            false => line
                .w(MARKER_WIDTH)
                .h(viewport.height * MARKER_HEIGHT * grown),
        }
        .into_any_element()
    }

    /// The button that offers the widgets there are, to add one to `slot`.
    fn add_button(slot: usize, theme: &Theme, chrome: f32, cx: &mut Context<Self>) -> AnyElement {
        let hovered = theme.colors.primary.opacity(HANDLE_FILL * 2.);
        div()
            .id(("panel-add", slot))
            .flex_none()
            .size(ADD_SIZE * chrome.clamp(0., 1.))
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .rounded(theme.shape.full)
            .border_1()
            .border_color(theme.colors.primary.opacity(chrome.clamp(0., 1.)))
            .bg(theme.colors.primary.opacity(HANDLE_FILL))
            .hover(move |style| style.bg(hovered))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.offer(slot, window, cx)))
            .child(
                icon(theme, ICON_ADD)
                    .text_size(theme.typography.label_size)
                    .text_color(theme.colors.primary),
            )
            .into_any_element()
    }

    /// The button that ends arranging.
    /// On a bar at the left or the right it shows its mark alone, which
    /// fits the width of the bar.
    fn done_button(
        theme: &Theme,
        chrome: f32,
        vertical: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hovered = theme.colors.primary.opacity(0.85);
        let label = (!vertical).then(|| Language::of(cx).say("Done"));
        div()
            .id("panel-done")
            .flex_none()
            .h(ADD_SIZE)
            .px(theme.spacing.small + theme.spacing.tiny)
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .bg(theme.colors.primary)
            .hover(move |style| style.bg(hovered))
            .cursor_pointer()
            .opacity(chrome.clamp(0., 1.))
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.surface_container)
            .whitespace_nowrap()
            .on_click(|_, _, cx| Arranging::set(false, cx))
            .child(icon(theme, ICON_DONE).text_size(theme.typography.label_size))
            .children(label)
            .into_any_element()
    }

    /// Opens the list of the widgets there are; the one chosen is added to
    /// `slot` of this surface.
    fn offer(&self, slot: usize, window: &Window, cx: &mut App) {
        let entries = BarMenu::addable(self.surface, Slot::ALL[slot], cx);
        match PopupAnchor::at_point(window, window.mouse_position(), cx) {
            Ok(anchor) => Menus::open(entries, anchor, cx),
            Err(error) => report!("the list of widgets has nowhere to open: {error:#}"),
        }
    }

    /// The widget under `position` of the surface: its slot and its place
    /// among those drawn there.
    fn widget_at(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        self.slots.iter().enumerate().find_map(|(slot, placed)| {
            let index = placed.iter().position(|placed| {
                placed
                    .bounds
                    .get()
                    .is_some_and(|bounds| bounds.contains(&position))
            })?;
            Some((slot, index))
        })
    }

    /// Answers a click with the right button at `position`: the menu of
    /// the widget there, or of the bar where no widget is. A widget that
    /// answers the button itself is left to it unless Shift is held or the
    /// widgets are arranged, as is one that opened a menu for the click.
    fn context_menu(&self, event: &MouseDownEvent, window: &Window, cx: &mut App) {
        let arranging = Arranging::is_on(cx);
        let forced = event.modifiers.shift || arranging;
        if !forced && Menus::opened(cx) != self.menus_before.get() {
            return;
        }
        let target = match self.widget_at(event.position) {
            Some((slot, index)) => {
                let placed = &self.slots[slot][index];
                if !forced && OWN_MENUS.contains(&placed.name.as_ref()) {
                    return;
                }
                BarTarget::Widget {
                    place: Place {
                        surface: self.surface,
                        slot: Slot::ALL[slot],
                        index: placed.index,
                    },
                    name: placed.name.clone(),
                    editable: self.editable[slot],
                }
            }
            None => BarTarget::Bar {
                slot: Slot::ALL[self
                    .place_for(self.along(event.position))
                    .map_or(1, |(slot, _)| slot)],
            },
        };
        let edge = self.edge;
        match PopupAnchor::at_point(window, event.position, cx) {
            Ok(anchor) => BarMenu::open(self.surface, edge, target, anchor, window, cx),
            Err(error) => report!("the menu of the bar has nowhere to open: {error:#}"),
        }
    }

    /// Takes aim with the dragged widget, the pointer being at `position`:
    /// far past the surface is out of the layout; otherwise the slot is the
    /// nearest one, and the place in it is before the first widget whose
    /// middle is past the pointer.
    fn aim_at(&mut self, position: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        if self.dragged.is_none() {
            return;
        }
        // Across the surface is out of it, along it is a place in it.
        let viewport = window.viewport_size();
        let (across, depth) = match self.vertical() {
            true => (position.x, viewport.width),
            false => (position.y, viewport.height),
        };
        let aim = if across < -AWAY || across > depth + AWAY {
            let abroad = self.aim_abroad(cx);
            if abroad.is_none() {
                self.end_visit(cx);
            }
            abroad.or(Some(Aim::Away))
        } else {
            self.end_visit(cx);
            self.place_for(self.along(position))
                .map(|(slot, index)| Aim::Before(slot, index))
        };
        if aim != self.aim {
            self.aim = aim;
            self.discarding.set(aim == Some(Aim::Away));
            self.marker.jump_to(0.);
            self.marker.animate_to(1.);
            cx.notify();
        }
    }

    /// The slot nearest to `x`, a distance along this panel, and the place
    /// in it before the first widget whose middle is past `x`.
    fn place_for(&self, x: Pixels) -> Option<(usize, usize)> {
        let edge = self.edge;
        let start = |bounds: Bounds<Pixels>| edge.along_of(bounds.left(), bounds.top());
        let end = |bounds: Bounds<Pixels>| edge.along_of(bounds.right(), bounds.bottom());
        let distance = |bounds: Bounds<Pixels>| {
            let before = start(bounds) - x;
            let after = x - end(bounds);
            before.max(after).max(px(0.))
        };
        let (slot, _) = (0..self.slots.len())
            .filter(|&slot| self.editable[slot])
            .filter_map(|slot| Some((slot, distance(self.places[slot].get()?))))
            .min_by(|(_, one), (_, other)| {
                one.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
            })?;
        let next = self.slots[slot].iter().find(|placed| {
            let bounds = placed.bounds.get();
            bounds.is_some_and(|bounds| self.along(bounds.center()) > x)
        });
        Some((slot, next.map_or(self.counts[slot], |placed| placed.index)))
    }

    /// What the layout calls the widget being dragged.
    fn dragged_name(&self) -> Option<SharedString> {
        let place = self.dragged?;
        let slot = Slot::ALL.iter().position(|slot| *slot == place.slot)?;
        let mut widgets = self.slots[slot].iter();
        Some(
            widgets
                .find(|placed| placed.index == place.index)?
                .name
                .clone(),
        )
    }

    /// Takes aim on the panel of another window the pointer is over, which
    /// then shows the widget and where it would go. The pointer stays with
    /// the window the drag began in, so that panel learns of it from here.
    fn aim_abroad(&mut self, cx: &mut Context<Self>) -> Option<Aim> {
        let (x, y) = pointer_at()?;
        let name = self.dragged_name()?;
        let me = cx.entity_id();
        let panels = cx.try_global::<Arena>()?.0.clone();
        let over = panels.into_iter().find_map(|panel| {
            if panel.entity_id() == me {
                return None;
            }
            let (surface, edge, (frame, scale)) = panel
                .read_with(cx, |other, _| {
                    Some((other.surface, other.edge, other.frame?))
                })
                .ok()??;
            let inside =
                (frame.left..frame.right).contains(&x) && (frame.top..frame.bottom).contains(&y);
            // How far along the other surface the pointer is.
            let along = edge.along_of(x - frame.left, y - frame.top);
            inside.then(|| (panel, surface, px(along as f32 / scale)))
        });
        let (panel, surface, at) = over?;
        if self.visited.as_ref() != Some(&panel) {
            self.end_visit(cx);
            self.visited = Some(panel.clone());
        }
        let place = panel
            .update(cx, |other, cx| other.receive(name, at, cx))
            .ok()??;
        Some(Aim::Onto(surface, place.0, place.1))
    }

    /// Shows the widget `name`, dragged off another panel, at `at` of this
    /// one, and says where it would go.
    fn receive(
        &mut self,
        name: SharedString,
        at: Pixels,
        cx: &mut Context<Self>,
    ) -> Option<(usize, usize)> {
        let place = self.place_for(at);
        let aim = place.map(|(slot, index)| Aim::Before(slot, index));
        if aim != self.aim {
            self.aim = aim;
            self.marker.jump_to(0.);
            self.marker.animate_to(1.);
        }
        self.visitor = Some((name, at));
        cx.notify();
        place
    }

    /// The widget of another panel is over this one no longer.
    fn dismiss_visitor(&mut self, cx: &mut Context<Self>) {
        self.visitor = None;
        self.aim = None;
        cx.notify();
    }

    /// The dragged widget is no longer over the panel of another window.
    fn end_visit(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = self.visited.take() {
            panel.update(cx, |other, cx| other.dismiss_visitor(cx)).ok();
        }
    }

    /// Lets go of the dragged widget: it goes where the aim is.
    fn let_go(&mut self, cx: &mut Context<Self>) {
        let Some(from) = self.dragged.take() else {
            return;
        };
        let aim = self.aim.take();
        self.end_visit(cx);
        self.discarding.set(false);
        self.marker.jump_to(0.);
        cx.notify();
        let (surface, slot, index) = match aim {
            Some(Aim::Away) => {
                return cx.defer(move |cx| rewrite(cx, |file| file.remove(from)));
            }
            Some(Aim::Before(slot, index)) => (from.surface, slot, index),
            Some(Aim::Onto(surface, slot, index)) => (surface, slot, index),
            None => return,
        };
        let to = Place {
            surface,
            slot: Slot::ALL[slot],
            index,
        };
        let stays = to.surface == from.surface
            && to.slot == from.slot
            && (index == from.index || index == from.index + 1);
        if !stays {
            // This surface is still handling the pointer; it is given new
            // content after that.
            cx.defer(move |cx| rewrite(cx, |file| file.transfer(from, to)));
        }
    }

    fn slot(&mut self, slot: usize, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> Div {
        let widgets: Vec<AnyElement> = (0..self.slots[slot].len())
            .map(|index| self.widget(slot, index, theme, window, cx))
            .collect();
        // How long the slot is decides how the row is laid out, so a
        // length that changed has the row drawn again.
        let seen = self.places[slot].clone();
        let edge = self.edge;
        let measure = move |bounds: Bounds<Pixels>, window: &mut Window, _: &mut App| {
            let before = seen.replace(Some(bounds));
            let long =
                |bounds: Bounds<Pixels>| edge.along_of(bounds.size.width, bounds.size.height);
            let changed =
                before.is_none_or(|before| (long(before) - long(bounds)).abs() > FIT_SLACK);
            if changed {
                window.request_animation_frame();
            }
        };
        self.line_of(slot, theme)
            .children(widgets)
            .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
    }

    /// The container of the widgets of `slot`: a row on a bar at the top
    /// or the bottom, a column as wide as the bar on one at the left or
    /// the right. One with a spacer that takes the free room takes all the
    /// room its side of the bar has.
    fn line_of(&self, slot: usize, theme: &Theme) -> Div {
        let line = div()
            .relative()
            .flex()
            .items_center()
            .gap(theme.spacing.small);
        let line = match self.vertical() {
            true => line.flex_col().w_full(),
            false => line,
        };
        match self.flexible[slot] && slot != 1 {
            true => line.flex_1(),
            false => line.flex_none(),
        }
    }

    /// The length the first slot is given where the slots have no room to
    /// keep the middle one in the middle of a surface `length` long; `None`
    /// where they have.
    fn lead(&self, theme: &Theme, length: Pixels) -> Option<Pixels> {
        if self.flexible[0] || self.flexible[2] {
            // A side that takes its free room fills it; the middle stays.
            return None;
        }
        let edge = self.edge;
        let long = |slot: usize| {
            let place: Option<Bounds<Pixels>> = self.places[slot].get();
            place.map_or(px(0.), |place| {
                edge.along_of(place.size.width, place.size.height)
            })
        };
        let room = length - theme.spacing.small * 2.;
        lead_of([long(0), long(1), long(2)], room, theme.spacing.small)
    }

    /// The three slots laid out along the surface, the middle one in the
    /// middle where the sides leave it room.
    fn arranged_line(
        &self,
        theme: &Theme,
        [start, center, end]: [Div; 3],
        lead: Option<Pixels>,
    ) -> Div {
        if !self.vertical() {
            return centered_row(theme, start, center, end, lead);
        }
        centered_column(theme, start, center, end, lead)
    }

    /// A slot while the widgets are arranged: its widgets as handles, the
    /// line at the aim, and the button that adds a widget; the last slot
    /// ends with the button that ends arranging.
    fn arranged_slot(
        &mut self,
        slot: usize,
        theme: &Theme,
        chrome: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let aimed = match self.aim {
            Some(Aim::Before(at, index))
                if at == slot && (self.dragged.is_some() || self.visitor.is_some()) =>
            {
                Some(index)
            }
            _ => None,
        };
        let grown = self.marker.value(window);
        let vertical = self.vertical();
        let mut marked = false;
        let mut children = Vec::new();
        for index in 0..self.slots[slot].len() {
            if !marked && aimed.is_some_and(|aimed| self.slots[slot][index].index >= aimed) {
                children.push(Self::marker(theme, grown, vertical, window));
                marked = true;
            }
            let handle = self.handle(slot, index, theme, chrome, cx);
            children.push(match vertical {
                true => Self::cell(handle, None, theme, window),
                false => handle,
            });
        }
        if !marked && aimed.is_some() {
            children.push(Self::marker(theme, grown, vertical, window));
        }
        if self.editable[slot] {
            children.push(Self::add_button(slot, theme, chrome, cx));
        }
        if slot + 1 == self.slots.len() {
            children.push(Self::done_button(theme, chrome, vertical, cx));
        }
        let seen = self.places[slot].clone();
        let measure = move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
            seen.set(Some(bounds));
        };
        let line = div()
            .relative()
            .flex()
            .items_center()
            .gap(theme.spacing.small);
        match vertical {
            true => line.flex_col().w_full(),
            false => line,
        }
        .children(children)
        .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
    }
}

/// A column that keeps `center` in the middle of a bar at the left or the
/// right, as [`centered_row`] keeps it in the middle of one at the top or
/// the bottom: where the sides have no room for that, `lead` is the
/// height `start` is given instead.
fn centered_column(theme: &Theme, start: Div, center: Div, end: Div, lead: Option<Pixels>) -> Div {
    let column = div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .py(theme.spacing.small);
    let side = || {
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .min_h_0()
            .overflow_hidden()
    };
    if let Some(lead) = lead {
        return column
            .gap(theme.spacing.small)
            .child(side().flex_none().h(lead).justify_start().child(start))
            .child(center.flex_none())
            .child(side().flex_1().justify_end().child(end));
    }
    column
        .child(side().flex_1().justify_start().child(start))
        .child(center.flex_none())
        .child(side().flex_1().justify_end().child(end))
}

/// Changes the configuration file with `change` and takes the file over at
/// once. A change that cannot be made or written is reported.
pub fn rewrite(cx: &mut App, change: impl FnOnce(&mut ConfigFile) -> bool) {
    let written = ConfigFile::change(|file| {
        if !change(file) {
            return Err("the configuration has changed in the meantime".to_owned());
        }
        Ok(())
    });
    match written {
        Ok(()) => Shell::config_changed(cx),
        Err(error) => report!("the layout stays as it is: {error}"),
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme(cx);
        let entrance = self.entrance.value(window);
        let viewport = window.viewport_size();
        let depth = match self.edge.across() {
            Axis::Vertical => viewport.height,
            Axis::Horizontal => viewport.width,
        };
        let offset = self.edge.travel(depth * (1. - entrance));
        // How many menus had opened before a widget saw the right button
        // go down, to tell whether it opened one for the click.
        let menus_before = self.menus_before.clone();
        let before_widgets = move |_: Bounds<Pixels>, (), window: &mut Window, _: &mut App| {
            let menus_before = menus_before.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture && event.button == MouseButton::Right {
                    menus_before.set(Menus::opened(cx));
                }
            });
        };
        let root = div()
            .size_full()
            .relative()
            .left(offset.x)
            .top(offset.y)
            .opacity(entrance.clamp(0., 1.))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.context_menu(event, window, cx);
                }),
            )
            .child(canvas(|_, _, _| {}, before_widgets).absolute().size_0());
        let arranging = Arranging::is_on(cx);
        if (self.chrome.target() > 0.5) != arranging {
            self.chrome.animate_to(if arranging { 1. } else { 0. });
        }
        let length = self.edge.along_of(viewport.width, viewport.height);
        if !arranging {
            self.dragged = None;
            self.aim = None;
            (self.frame, self.visited, self.visitor) = (None, None, None);
            self.watch_ground(&theme, window, cx);
            let slots = [0, 1, 2].map(|slot| self.slot(slot, &theme, window, cx));
            let lead = self.lead(&theme, length);
            return root.child(self.arranged_line(&theme, slots, lead));
        }
        // A drag that ended anywhere but over this surface is over too.
        if self.dragged.is_some() && !cx.has_active_drag() {
            self.dragged = None;
            self.aim = None;
            self.discarding.set(false);
        }
        if self.visitor.is_some() && !cx.has_active_drag() {
            self.visitor = None;
            self.aim = None;
        }
        if !self.listed {
            self.listed = true;
            let panel = cx.entity().downgrade();
            let arena = cx.default_global::<Arena>();
            arena.0.retain(|panel| panel.upgrade().is_some());
            arena.0.push(panel);
        }
        let native = NativeSurface::of(window).and_then(|native| native.frame());
        self.frame = native.ok().map(|frame| (frame, window.scale_factor()));
        let vertical = self.vertical();
        let visitor = self.visitor.clone().map(|(name, at)| {
            let ghost = div()
                .absolute()
                .flex()
                .items_center()
                .child(Ghost::pill(&theme, name, false));
            match vertical {
                true => ghost.left_0().top(at).w_full().justify_center(),
                false => ghost.top_0().left(at + theme.spacing.small).h_full(),
            }
        });
        // The outlines keep breathing.
        crate::platform::Pace::quicken();
        window.request_animation_frame();
        let chrome = self.chrome.value(window);
        let slots = [0, 1, 2].map(|slot| self.arranged_slot(slot, &theme, chrome, window, cx));
        let panel = cx.entity().downgrade();
        // The button is let go of anywhere, also past the surface, where
        // nothing of it is under the pointer.
        let listen = move |_: Bounds<Pixels>, (), window: &mut Window, _: &mut App| {
            let panel = panel.clone();
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture {
                    panel.update(cx, |this, cx| this.let_go(cx)).ok();
                }
            });
        };
        root.on_drag_move(
            cx.listener(|this, event: &DragMoveEvent<Dragged>, window, cx| {
                this.aim_at(event.event.position, window, cx);
            }),
        )
        .child(self.arranged_line(&theme, slots, None))
        .children(visitor)
        .child(canvas(|_, _, _| {}, listen).absolute().size_0())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_middle_slot_leaves_the_middle_only_as_far_as_a_side_needs() {
        let gap = px(8.);
        // Both sides fit beside a middle slot in the middle.
        assert_eq!(
            lead_of([px(100.), px(200.), px(300.)], px(1000.), gap),
            None
        );
        // The last slot reaches 50 into it: it stands that much earlier.
        let lead = lead_of([px(100.), px(200.), px(450.)], px(1000.), gap);
        assert_eq!(lead, Some(px(334.)));
        // The first slot is not given less than it takes.
        let lead = lead_of([px(400.), px(200.), px(450.)], px(1000.), gap);
        assert_eq!(lead, Some(px(400.)));
        // A wide first slot moves the middle one on by what it needs.
        let lead = lead_of([px(450.), px(200.), px(100.)], px(1000.), gap);
        assert_eq!(lead, Some(px(450.)));
    }
}
