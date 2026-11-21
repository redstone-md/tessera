// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The content of an edge surface: widgets arranged in three slots, and the
//! way to arrange them by hand: while that is on, a widget is dragged to
//! another place of its surface or off the surface, and a button in every
//! slot adds one.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, AnyView, App, AppContext, Bounds, Context, CursorStyle, DispatchPhase, Div,
    DragMoveEvent, Global, InteractiveElement, IntoElement, MouseUpEvent, ParentElement, Pixels,
    Point, Render, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, px,
};

use crate::config::edit::{ConfigFile, Place, Slot};
use crate::config::{SurfaceConfig, WidgetSpec};
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::module::WidgetRegistry;
use crate::motion::{Interaction, Spring};
use crate::platform::{DisplayKey, ScreenEdge};
use crate::popup::{PopupAnchor, Popups};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{HOVER_LAYER, PRESS_LAYER, centered_row, icon};

const ICON_ADD: &str = "\u{e710}";
const ICON_DONE: &str = "\u{e73e}";
const ICON_DISCARD: &str = "\u{e74d}";

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
const BREATH: f32 = 3.2;
const BREATH_STEP: f32 = 0.8;
const OUTLINE_REST: f32 = 0.35;
const OUTLINE_SWING: f32 = 0.3;
/// How much of the outline's color fills a widget.
const HANDLE_FILL: f32 = 0.25;
/// How much of a widget is left to see while it is dragged.
const LIFTED: f32 = 0.35;

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

/// A widget in a slot, and the popup a click on it opens.
struct Placed {
    view: AnyView,
    /// What the layout calls it.
    name: SharedString,
    /// Where it stands in its slot in the configuration file, where the
    /// widgets no module provides are counted as well.
    index: usize,
    popup: Option<String>,
    interaction: Interaction,
    /// Where the widget is in its surface, once it has been drawn.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
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

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let discarding = self.discarding.get();
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
                    .then(|| icon(&theme, ICON_DISCARD).text_size(theme.typography.label_size)),
            )
            .child(self.name.clone())
    }
}

/// Where a dragged widget would go if it were let go.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Aim {
    /// Before what stands at this place of the file in this slot.
    Before(usize, usize),
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
}

impl Panel {
    /// Instantiates the widgets `layout` names, which is the surface at
    /// `surface` of the configuration. Unknown names are reported and left
    /// out.
    pub fn new(
        surface: usize,
        layout: &SurfaceConfig,
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
                        interaction: Interaction::new(&motion),
                        bounds: Rc::default(),
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
        }
    }

    /// The middle of the first widget that opens the popup `name`, if
    /// there is one and it has been drawn.
    pub fn popup_middle(&self, name: &str) -> Option<Pixels> {
        let mut placed = self.slots.iter().flatten();
        let bounds = placed
            .find(|placed| placed.popup.as_deref() == Some(name))?
            .bounds
            .get()?;
        Some(bounds.center().x)
    }

    fn open_popup(&self, name: &str, window: &Window, cx: &mut App) {
        match PopupAnchor::at(window, window.mouse_position().x, cx) {
            Ok(anchor) => Popups::toggle(name, anchor, cx),
            Err(error) => report!("the popup `{name}` has nowhere to open: {error:#}"),
        }
    }

    /// The widget at `index` of `slot`. One that opens a popup is wrapped in
    /// a button with a state layer over it.
    fn widget(
        &mut self,
        slot: usize,
        index: usize,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let placed = &mut self.slots[slot][index];
        let Some(popup) = placed.popup.clone() else {
            return placed.view.clone().into_any_element();
        };
        let state = placed.interaction.state(window);
        let layer = HOVER_LAYER * state.hover + PRESS_LAYER * state.press.clamp(0., 1.);
        let seen = placed.bounds.clone();
        let measure = move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
            seen.set(Some(bounds));
        };
        let button = div()
            .id(("panel-widget", slot * 1000 + index))
            .relative()
            .on_click(cx.listener(move |this, _, window, cx| this.open_popup(&popup, window, cx)))
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
            .cursor(CursorStyle::OpenHand)
            .occlude()
            .on_drag(dragged, move |dragged: &Dragged, _, _, cx| {
                let place = dragged.place;
                let taken = panel.update(cx, |this, cx| {
                    this.dragged = Some(place);
                    cx.notify();
                });
                taken.ok();
                let (name, discarding) = (dragged.name.clone(), dragged.discarding.clone());
                cx.new(|_| Ghost { name, discarding })
            });
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

    /// The line that shows where the dragged widget would go.
    fn marker(theme: &Theme, grown: f32, window: &Window) -> AnyElement {
        let grown = grown.clamp(0., 1.);
        div()
            .flex_none()
            .w(MARKER_WIDTH)
            .h(window.viewport_size().height * MARKER_HEIGHT * grown)
            .rounded(theme.shape.full)
            .bg(theme.colors.primary.opacity(grown))
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
    fn done_button(theme: &Theme, chrome: f32, cx: &mut Context<Self>) -> AnyElement {
        let hovered = theme.colors.primary.opacity(0.85);
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
            .child(Language::of(cx).say("Done"))
            .into_any_element()
    }

    /// Opens the list of the widgets there are; the one chosen is added to
    /// `slot` of this surface.
    fn offer(&self, slot: usize, window: &Window, cx: &mut App) {
        let surface = self.surface;
        let slot = Slot::ALL[slot];
        let names = cx.global::<WidgetRegistry>().names();
        let entries = names
            .into_iter()
            .map(|name| {
                let widget = name.clone();
                MenuEntry::new(ICON_ADD, name, move |cx| {
                    let widget = widget.clone();
                    // The menu this is chosen in is still handling the
                    // click; the surfaces are made again after it.
                    cx.defer(move |cx| rewrite(cx, |file| file.add(surface, slot, &widget)));
                })
            })
            .collect();
        match PopupAnchor::at(window, window.mouse_position().x, cx) {
            Ok(anchor) => Menus::open(entries, anchor, cx),
            Err(error) => report!("the list of widgets has nowhere to open: {error:#}"),
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
        let height = window.viewport_size().height;
        let aim = if position.y < -AWAY || position.y > height + AWAY {
            Some(Aim::Away)
        } else {
            let distance = |bounds: Bounds<Pixels>| {
                let before = bounds.left() - position.x;
                let after = position.x - bounds.right();
                before.max(after).max(px(0.))
            };
            let nearest = (0..self.slots.len())
                .filter_map(|slot| Some((slot, distance(self.places[slot].get()?))))
                .min_by(|(_, one), (_, other)| {
                    one.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
                });
            nearest.map(|(slot, _)| {
                let next = self.slots[slot].iter().find(|placed| {
                    let bounds = placed.bounds.get();
                    bounds.is_some_and(|bounds| bounds.center().x > position.x)
                });
                let index = next.map_or(self.counts[slot], |placed| placed.index);
                Aim::Before(slot, index)
            })
        };
        if aim != self.aim {
            self.aim = aim;
            self.discarding.set(aim == Some(Aim::Away));
            self.marker.jump_to(0.);
            self.marker.animate_to(1.);
            cx.notify();
        }
    }

    /// Lets go of the dragged widget: it goes where the aim is.
    fn let_go(&mut self, cx: &mut Context<Self>) {
        let Some(from) = self.dragged.take() else {
            return;
        };
        let aim = self.aim.take();
        self.discarding.set(false);
        self.marker.jump_to(0.);
        cx.notify();
        match aim {
            Some(Aim::Away) => cx.defer(move |cx| rewrite(cx, |file| file.remove(from))),
            Some(Aim::Before(slot, index)) => {
                let slot = Slot::ALL[slot];
                let stays = slot == from.slot && (index == from.index || index == from.index + 1);
                if !stays {
                    // This surface is still handling the pointer; it is
                    // given new content after that.
                    cx.defer(move |cx| rewrite(cx, |file| file.relocate(from, slot, index)));
                }
            }
            None => {}
        }
    }

    fn slot(&mut self, slot: usize, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> Div {
        let widgets: Vec<AnyElement> = (0..self.slots[slot].len())
            .map(|index| self.widget(slot, index, theme, window, cx))
            .collect();
        div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .children(widgets)
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
            Some(Aim::Before(at, index)) if at == slot && self.dragged.is_some() => Some(index),
            _ => None,
        };
        let grown = self.marker.value(window);
        let mut marked = false;
        let mut children = Vec::new();
        for index in 0..self.slots[slot].len() {
            if !marked && aimed.is_some_and(|aimed| self.slots[slot][index].index >= aimed) {
                children.push(Self::marker(theme, grown, window));
                marked = true;
            }
            children.push(self.handle(slot, index, theme, chrome, cx));
        }
        if !marked && aimed.is_some() {
            children.push(Self::marker(theme, grown, window));
        }
        children.push(Self::add_button(slot, theme, chrome, cx));
        if slot + 1 == self.slots.len() {
            children.push(Self::done_button(theme, chrome, cx));
        }
        let seen = self.places[slot].clone();
        let measure = move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
            seen.set(Some(bounds));
        };
        div()
            .relative()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .children(children)
            .child(canvas(measure, |_, (), _, _| {}).absolute().size_full())
    }
}

/// Changes the configuration file with `change` and takes the file over at
/// once. A change that cannot be made or written is reported.
fn rewrite(cx: &mut App, change: impl FnOnce(&mut ConfigFile) -> bool) {
    let written = ConfigFile::open().and_then(|mut file| {
        if !change(&mut file) {
            return Err("the configuration has changed in the meantime".to_owned());
        }
        file.save()
    });
    match written {
        Ok(()) => Shell::config_changed(cx),
        Err(error) => report!("the layout stays as it is: {error}"),
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let entrance = self.entrance.value(window);
        let hidden = window.viewport_size().height * (1. - entrance);
        let offset = match self.edge {
            ScreenEdge::Top => -hidden,
            ScreenEdge::Bottom => hidden,
        };
        let root = div()
            .size_full()
            .relative()
            .top(offset)
            .opacity(entrance.clamp(0., 1.));
        let arranging = Arranging::is_on(cx);
        if (self.chrome.target() > 0.5) != arranging {
            self.chrome.animate_to(if arranging { 1. } else { 0. });
        }
        if !arranging {
            self.dragged = None;
            self.aim = None;
            let [start, center, end] = [0, 1, 2].map(|slot| self.slot(slot, &theme, window, cx));
            return root.child(centered_row(&theme, start, center, end));
        }
        // A drag that ended anywhere but over this surface is over too.
        if self.dragged.is_some() && !cx.has_active_drag() {
            self.dragged = None;
            self.aim = None;
            self.discarding.set(false);
        }
        // The outlines keep breathing.
        window.request_animation_frame();
        let chrome = self.chrome.value(window);
        let [start, center, end] =
            [0, 1, 2].map(|slot| self.arranged_slot(slot, &theme, chrome, window, cx));
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
        .child(centered_row(&theme, start, center, end))
        .child(canvas(|_, _, _| {}, listen).absolute().size_0())
    }
}
