// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The window that draws the icons of one display: a tile per icon in
//! the cells of the grid, the band that is drawn around icons to choose
//! them, the icons being dragged, and what the pointer and the keys do
//! there.
//!
//! A drag of icons stays inside Tessera while the pointer is over the
//! desktop of any display: the press holds the pointer (GPUI captures
//! it), so the window goes on hearing where it is beyond its own display,
//! and the icons are put where it is let go. Once the pointer is over
//! anything else, the drag goes on as a drag of files out of Tessera
//! ([`DragOut`]), which is started from a task of GPUI with nothing of it
//! borrowed, and gives GPUI the release of the button that OLE takes.
//! What other programs drop onto the desktop is taken by Tessera's own
//! drop target ([`DropTarget`]), which follows Shift, Ctrl and Alt as
//! Explorer does.

use std::collections::HashSet;
use std::time::Duration;

use gpui::{
    AnyElement, AsyncApp, Bounds, Context, Div, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, Render, ScrollWheelEvent, Styled, Subscription, WeakEntity,
    Window, canvas, div, hsla, img, point, prelude::FluentBuilder, px, size,
};

use super::grid::{Spot, Toward};
use super::items::DesktopItem;
use super::{DesktopModel, POWER_DIALOG, Room, icon_of, menu};
use crate::actions::Actions;
use crate::platform::{
    DesktopLayer, DragOut, DropEvent, DropTarget, DropZone, DropZones, ScreenRect, layer_frame,
    pointer_at,
};
use crate::theme::Theme;
use crate::ui::release_hover;

/// How far the pointer moves with the button held before a press on an
/// icon becomes a drag, in logical pixels.
const DRAG_DISTANCE: f32 = 4.;

/// How often the window looks whether the room of its display changed.
const PLACE_CHECK: Duration = Duration::from_secs(1);

/// How far the wheel turns with Ctrl held for the icons to change size.
const WHEEL_STEP: f32 = 40.;

/// How opaque the icons being dragged are where they lie, and where they
/// are carried.
const LEFT_BEHIND: f32 = 0.45;
const CARRIED: f32 = 0.7;

/// How many lines of its name an icon shows, and a chosen one.
const NAME_LINES: usize = 2;
const CHOSEN_NAME_LINES: usize = 5;

/// A press of the left button on an icon, which may become a drag.
#[derive(Clone)]
struct Press {
    at: Point<Pixels>,
    key: String,
    /// Whether the icon was chosen among others, which a click without
    /// keys narrows the choice to once the button is let go, so that the
    /// others can be dragged with it.
    narrow: bool,
}

/// Icons being dragged, from where the press was to where the pointer is.
struct Carried {
    keys: Vec<String>,
    /// The icon that was pressed, which goes where the pointer is.
    lead: String,
    from: Point<Pixels>,
    to: Point<Pixels>,
}

/// The band drawn from where the press was to the pointer, which chooses
/// the icons it touches, with those chosen before where Ctrl was held.
struct Band {
    from: Point<Pixels>,
    to: Point<Pixels>,
    before: HashSet<String>,
}

impl Band {
    fn bounds(&self) -> Bounds<Pixels> {
        let origin = point(self.from.x.min(self.to.x), self.from.y.min(self.to.y));
        let far = point(self.from.x.max(self.to.x), self.from.y.max(self.to.y));
        Bounds::from_corners(origin, far)
    }
}

/// The root view of the window of the icons of one display.
pub struct DesktopIconsView {
    model: Entity<DesktopModel>,
    /// The number of the display among the displays in their order.
    display: usize,
    layer: Option<DesktopLayer>,
    /// Where the layer was put, in physical pixels.
    frame: Option<ScreenRect>,
    scale: f32,
    focus: FocusHandle,
    zones: DropZones,
    drops: Option<DropTarget>,
    press: Option<Press>,
    carried: Option<Carried>,
    band: Option<Band>,
    hovered: Option<String>,
    /// How far the wheel was turned with Ctrl held, toward a step.
    wheel: f32,
    _model: Subscription,
}

impl DesktopIconsView {
    pub fn new(
        model: Entity<DesktopModel>,
        display: usize,
        layer: Option<DesktopLayer>,
        frame: Option<ScreenRect>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observed = cx.observe(&model, |this: &mut Self, _, cx| {
            this.tell_room(cx);
            cx.notify();
        });
        // Alt+F4 that reaches the window closes nothing: it asks how to
        // end the session, as on the desktop of Windows.
        window.on_window_should_close(cx, |_, cx| {
            cx.defer(|cx| Actions::run(POWER_DIALOG, None, cx));
            false
        });
        let zones = DropZones::new();
        let drops = Self::take_drops(&model, &zones, window, cx);
        let mut this = Self {
            model,
            display,
            layer,
            frame,
            scale: window.scale_factor(),
            focus: cx.focus_handle(),
            zones,
            drops,
            press: None,
            carried: None,
            band: None,
            hovered: None,
            wheel: 0.,
            _model: observed,
        };
        this.tell_room(cx);
        Self::keep_placed(cx);
        this
    }

    /// Closes the window, giving it back to the desktop first.
    pub fn close(&mut self, window: &mut Window) {
        self.drops = None;
        self.layer = None;
        window.remove_window();
    }

    /// Has the window take what other programs drop onto it, in place of
    /// GPUI: onto the desktop or into the folders among the icons.
    fn take_drops(
        model: &Entity<DesktopModel>,
        zones: &DropZones,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<DropTarget> {
        let (target, events) = match DropTarget::register(window, zones.clone()) {
            Ok(registered) => registered,
            Err(error) => {
                report!("the desktop takes no drops: {error:#}");
                return None;
            }
        };
        let model = model.downgrade();
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            while let Ok(event) = events.recv().await {
                let handled = model.update(cx, |model, cx| match event {
                    DropEvent::Dropped { folder, .. }
                        if Some(&folder) == model.folder().as_ref() =>
                    {
                        // What comes stands where it was dropped.
                        let at = pointer_at().and_then(|at| model.spot_at(at));
                        model.place_new_at(at);
                    }
                    DropEvent::Done { outcome, .. } => {
                        outcome.report();
                        model.reload(cx);
                    }
                    _ => {}
                });
                if handled.is_err() {
                    break;
                }
            }
        })
        .detach();
        Some(target)
    }

    /// Puts the window where the room of its display is, now and whenever
    /// it changes, until the view is gone.
    fn keep_placed(cx: &mut Context<Self>) {
        cx.spawn(async move |view: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(PLACE_CHECK).await;
                if view.update(cx, |view, cx| view.settle(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn settle(&mut self, cx: &mut Context<Self>) {
        let Some(layer) = &self.layer else {
            return;
        };
        let Some(native) = self.native() else {
            return;
        };
        let (Ok(whole), Some(room)) = (native.display_frame(), native.display().work_area()) else {
            return;
        };
        let frame = layer_frame(room, whole);
        if Some(frame) != self.frame {
            if let Err(error) = layer.place(frame) {
                report!("the icons of a display are not where they belong: {error:#}");
            }
            self.frame = Some(frame);
            self.tell_room(cx);
        }
    }

    fn native(&self) -> Option<crate::platform::NativeSurface> {
        // The window is the layer's; its display is asked through it.
        self.layer.as_ref().map(DesktopLayer::surface)
    }

    /// Tells the model where the room of the display is and how many
    /// icons it holds.
    fn tell_room(&mut self, cx: &mut Context<Self>) {
        let Some(frame) = self.frame else {
            return;
        };
        let (width, height) = self.model.read(cx).config().size.cell();
        let logical = |length: i32| length as f32 / self.scale;
        let room = Room {
            frame,
            scale: self.scale,
            grid: super::grid::Grid::fitting(
                logical(frame.right - frame.left),
                logical(frame.bottom - frame.top),
                width,
                height,
            ),
        };
        let display = self.display;
        self.model
            .update(cx, |model, cx| model.set_room(display, room, cx));
    }

    /// The cell of an icon, in logical pixels.
    fn cell(&self, cx: &Context<Self>) -> (f32, f32) {
        self.model.read(cx).config().size.cell()
    }

    /// Where the tile at `spot` is, in the coordinates of the window.
    fn tile_bounds(spot: Spot, cell: (f32, f32)) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(spot.column * cell.0), px(spot.row * cell.1)),
            size: size(px(cell.0), px(cell.1)),
        }
    }

    /// The icon of this display at `position`, in the coordinates of the
    /// window.
    fn key_at(&self, position: Point<Pixels>, cx: &Context<Self>) -> Option<String> {
        let model = self.model.read(cx);
        if !model.shows_icons() {
            return None;
        }
        let cell = self.cell(cx);
        { model.items().iter() }
            .filter_map(|item| Some((item, model.spot(&item.key)?)))
            .filter(|(_, spot)| spot.display == self.display)
            .find(|(_, spot)| Self::tile_bounds(*spot, cell).contains(&position))
            .map(|(item, _)| item.key.clone())
    }

    /// Takes the keys, as a click on the desktop gives them.
    fn take_focus(&mut self, window: &mut Window) {
        if let Some(layer) = &self.layer {
            layer.focus();
        }
        window.focus(&self.focus);
    }

    fn press_left(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.take_focus(window);
        let renaming = { self.model.read(cx).rename.as_ref() }.map(|rename| rename.key.clone());
        if let Some(key) = renaming {
            // A press in the field of the name is the field's.
            let model = self.model.read(cx);
            let cell = self.cell(cx);
            let spot = model.spot(&key).filter(|spot| spot.display == self.display);
            if spot.is_some_and(|spot| Self::tile_bounds(spot, cell).contains(&event.position)) {
                return;
            }
            self.model
                .update(cx, |model, cx| model.end_rename(false, cx));
        }
        let held = event.modifiers;
        let Some(key) = self.key_at(event.position, cx) else {
            let before = match held.control {
                true => self.model.read(cx).chosen().into_iter().collect(),
                false => HashSet::new(),
            };
            if !held.control {
                self.model.update(cx, |model, cx| model.clear_choice(cx));
            }
            self.band = Some(Band {
                from: event.position,
                to: event.position,
                before,
            });
            cx.notify();
            return;
        };
        if event.click_count >= 2 {
            self.model.update(cx, |model, cx| {
                if !model.is_chosen(&key) {
                    model.click(&key, false, false, cx);
                }
                model.open_chosen(cx);
            });
            return;
        }
        let chosen = self.model.read(cx).is_chosen(&key);
        let narrow = chosen && !held.control && !held.shift;
        if !narrow {
            self.model.update(cx, |model, cx| {
                model.click(&key, held.control, held.shift, cx)
            });
        }
        self.press = Some(Press {
            at: event.position,
            key,
            narrow,
        });
    }

    fn press_right(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.take_focus(window);
        let extended = event.modifiers.shift;
        match self.key_at(event.position, cx) {
            Some(key) => {
                self.model.update(cx, |model, cx| {
                    if !model.is_chosen(&key) {
                        model.click(&key, false, false, cx);
                    }
                });
                menu::open_item_menu(&self.model, event.position, extended, window, cx);
            }
            None => {
                self.model.update(cx, |model, cx| model.clear_choice(cx));
                let at = pointer_at().and_then(|at| self.model.read(cx).spot_at(at));
                menu::open_background_menu(&self.model, event.position, at, extended, window, cx);
            }
        }
    }

    /// The pointer moved, inside the window or, while a button is held,
    /// anywhere.
    fn moved(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let cell = self.cell(cx);
        if let Some(band) = &mut self.band {
            band.to = event.position;
            let bounds = band.bounds();
            let model = self.model.read(cx);
            let touched: Vec<String> = { model.items().iter() }
                .filter_map(|item| Some((item, model.spot(&item.key)?)))
                .filter(|(_, spot)| spot.display == self.display)
                .filter(|(_, spot)| Self::tile_bounds(*spot, cell).intersects(&bounds))
                .map(|(item, _)| item.key.clone())
                .collect();
            let before = band.before.clone();
            self.model.update(cx, |model, cx| {
                model.choose(before.into_iter().chain(touched), false, cx)
            });
            cx.notify();
            return;
        }
        if let Some(carried) = &mut self.carried {
            carried.to = event.position;
            let inside = Bounds {
                origin: Point::default(),
                size: window.viewport_size(),
            }
            .contains(&event.position);
            let over_desktop = inside || pointer_at().is_some_and(DesktopLayer::at_point);
            if !over_desktop {
                self.drag_out(window, cx);
            }
            cx.notify();
            return;
        }
        if let Some(press) = &self.press {
            if event.pressed_button != Some(MouseButton::Left) {
                self.press = None;
                return;
            }
            let moved = event.position - press.at;
            if (moved.x / px(1.)).hypot(moved.y / px(1.)) >= DRAG_DISTANCE {
                let press = self.press.take().expect("a press is held");
                let keys = self.model.read(cx).chosen();
                self.carried = Some(Carried {
                    keys,
                    lead: press.key,
                    from: press.at,
                    to: event.position,
                });
                cx.notify();
            }
            return;
        }
        let hovered = self.key_at(event.position, cx);
        if hovered != self.hovered {
            self.hovered = hovered;
            cx.notify();
        }
    }

    /// The icons dragged left the desktop: the drag goes on as a drag of
    /// their files out of Tessera, into any program.
    fn drag_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(carried) = self.carried.take() else {
            return;
        };
        let model = self.model.read(cx);
        let paths: Vec<_> = { carried.keys.iter() }
            .filter_map(|key| model.item(key)?.path().map(std::path::Path::to_owned))
            .collect();
        if paths.is_empty() {
            return;
        }
        let dragged = DragOut::new(paths).start(window, cx);
        let model = self.model.downgrade();
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            if let Err(error) = dragged.await {
                report!("the files were not dragged: {error}");
            }
            model.update(cx, |model, cx| model.reload(cx)).ok();
        })
        .detach();
    }

    /// The left button was let go, inside the window or anywhere while it
    /// was held from it.
    fn released(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.band.take().is_some() {
            cx.notify();
        }
        if let Some(press) = self.press.take()
            && press.narrow
        {
            self.model
                .update(cx, |model, cx| model.click(&press.key, false, false, cx));
        }
        let Some(carried) = self.carried.take() else {
            return;
        };
        cx.notify();
        let copy = event.modifiers.control;
        let cell = self.cell(cx);
        let display = self.display;
        self.model.update(cx, |model, cx| {
            let Some(lead) = model.spot(&carried.lead) else {
                return;
            };
            let moved = carried.to - carried.from;
            let here = Spot {
                column: lead.column + moved.x / px(1.) / cell.0,
                row: lead.row + moved.y / px(1.) / cell.1,
                display,
            };
            // Beyond the window the pointer is on the desktop of another
            // display, which says where.
            let target = pointer_at()
                .and_then(|at| model.spot_at(at))
                .filter(|spot| spot.display != display)
                .unwrap_or(here);
            if let Some(onto) = model.item_at(target, &carried.keys) {
                let onto = onto.key.clone();
                model.drop_onto(&onto, &carried.keys, copy, cx);
                return;
            }
            let by = (target.column - lead.column, target.row - lead.row);
            let to = (target.display != lead.display).then_some(target.display);
            model.shift(&carried.keys, by, to, cx);
        });
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let held = &event.keystroke.modifiers;
        if self.model.read(cx).rename.is_some() {
            match key {
                "enter" | "escape" => {
                    let keep = key == "escape";
                    self.model
                        .update(cx, |model, cx| model.end_rename(keep, cx));
                    window.focus(&self.focus);
                }
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        let window_handle = window.window_handle();
        let toward = match key {
            "left" => Some(Toward::Left),
            "right" => Some(Toward::Right),
            "up" => Some(Toward::Up),
            "down" => Some(Toward::Down),
            _ => None,
        };
        let model = self.model.clone();
        match (key, held.control, held.alt, held.shift) {
            (_, false, false, extend) if toward.is_some() => {
                let toward = toward.expect("an arrow was pressed");
                model.update(cx, |model, cx| model.step(toward, extend, cx));
            }
            ("enter", false, false, false) => model.update(cx, |model, cx| model.open_chosen(cx)),
            ("enter", false, true, false) => model.read(cx).properties(),
            ("f2", false, false, false) => {
                model.update(cx, |model, cx| model.rename_chosen(window, cx))
            }
            ("f5", false, false, false) | ("r", true, false, false) => {
                model.update(cx, |model, cx| model.reload(cx))
            }
            ("delete", false, false, erase) => {
                model.update(cx, |model, cx| model.delete(erase, cx))
            }
            ("a", true, false, false) => model.update(cx, |model, cx| model.choose_all(cx)),
            ("c", true, false, false) => model.read(cx).put_on_clipboard(false),
            ("x", true, false, false) => model.read(cx).put_on_clipboard(true),
            ("v", true, false, false) => model.update(cx, |model, cx| model.paste(false, cx)),
            ("z", true, false, false) => model.update(cx, |model, cx| model.undo(false, cx)),
            ("y", true, false, false) | ("z", true, false, true) => {
                model.update(cx, |model, cx| model.undo(true, cx))
            }
            ("n", true, false, true) => model.update(cx, |model, cx| {
                model.new_item(None, None, window_handle, cx)
            }),
            ("escape", false, false, false) => model.update(cx, |model, cx| model.clear_choice(cx)),
            ("=" | "+", true, false, _) => step_size(&model, true, cx),
            ("-", true, false, _) => step_size(&model, false, cx),
            // The keys of the sizes of Explorer's desktop.
            ("2", true, false, true) => DesktopModel::set_size(super::IconSize::Large, cx),
            ("3", true, false, true) => DesktopModel::set_size(super::IconSize::Medium, cx),
            ("4", true, false, true) => DesktopModel::set_size(super::IconSize::Small, cx),
            (_, false, false, _) => {
                let Some(letter) = event.keystroke.key_char.clone().filter(|letter| {
                    letter.chars().all(|c| !c.is_control()) && !letter.trim().is_empty()
                }) else {
                    return;
                };
                model.update(cx, |model, cx| model.type_letter(&letter, cx));
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The icons change size as the wheel turns with Ctrl held, a step for
    /// every notch.
    fn wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if !event.modifiers.control {
            return;
        }
        let turned = self.wheel + event.delta.pixel_delta(px(WHEEL_STEP)).y / px(1.);
        if turned.abs() < WHEEL_STEP {
            self.wheel = turned;
            return;
        }
        self.wheel = 0.;
        step_size(&self.model, turned > 0., cx);
    }

    /// The tile of `item` at `bounds`.
    fn tile(
        &self,
        item: &DesktopItem,
        bounds: Bounds<Pixels>,
        opacity: f32,
        lit: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let model = self.model.read(cx);
        let size = model.config().size;
        let chosen = lit && model.is_chosen(&item.key);
        let hovered = lit && self.hovered.as_ref() == Some(&item.key);
        let side = px(size.icon());
        // Fetched once for the display of the largest scale, and drawn
        // smaller on the others.
        let picture = match icon_of(item, model.icon_pixels(), cx) {
            Some(image) => img(image).size(side).into_any_element(),
            None => div()
                .size(side)
                .rounded(theme.shape.small)
                .bg(theme.colors.outline.opacity(0.3))
                .into_any_element(),
        };
        let arrow = item.is_link().then(|| DesktopModel::arrow(cx)).flatten();
        let picture = div()
            .relative()
            .size(side)
            .flex_none()
            .child(picture)
            .children(
                arrow.map(|arrow| img(arrow).absolute().left_0().bottom_0().size(side * 0.4)),
            );
        let renamed = { model.rename.as_ref() }
            .filter(|rename| rename.key == item.key)
            .map(|rename| rename.field.clone());
        let label = match renamed {
            Some(field) => div()
                .w(bounds.size.width + px(24.))
                .flex_none()
                .rounded(theme.shape.small)
                .bg(theme.colors.surface_container)
                .px(theme.spacing.tiny)
                .child(field)
                .into_any_element(),
            None => name_label(&item.name, bounds.size.width, chosen, theme),
        };
        let ground = match (chosen, hovered) {
            (true, _) => theme.colors.primary.opacity(0.32),
            (false, true) => hsla(0., 0., 1., 0.12),
            _ => gpui::transparent_black(),
        };
        div()
            .absolute()
            .left(bounds.origin.x)
            .top(bounds.origin.y)
            .w(bounds.size.width)
            .min_h(bounds.size.height)
            .p(px(2.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(2.))
            .rounded(theme.shape.small)
            .bg(ground)
            .border_1()
            .border_color(match chosen {
                true => theme.colors.primary.opacity(0.6),
                false => gpui::transparent_black(),
            })
            .opacity(opacity)
            .child(picture)
            .child(label)
    }
}

/// The name of an icon in white with a dark shadow, as on the desktop of
/// Windows, in two lines, or more where it is chosen.
fn name_label(name: &str, width: Pixels, chosen: bool, theme: &Theme) -> AnyElement {
    let lines = if chosen {
        CHOSEN_NAME_LINES
    } else {
        NAME_LINES
    };
    let text = |color| {
        div()
            .w(width - px(6.))
            .text_center()
            .line_clamp(lines)
            .text_ellipsis()
            .text_size(theme.typography.label_size)
            .font_family(theme.typography.family)
            .text_color(color)
            .child(name.to_owned())
    };
    div()
        .relative()
        .child(
            text(hsla(0., 0., 0., 0.6))
                .absolute()
                .left(px(1.))
                .top(px(1.)),
        )
        .child(text(hsla(0., 0., 1., 1.)))
        .into_any_element()
}

/// Makes the icons a step larger or smaller.
fn step_size(model: &Entity<DesktopModel>, larger: bool, cx: &mut gpui::App) {
    let size = model.read(cx).config().size;
    let next = size.step(larger);
    if next != size {
        DesktopModel::set_size(next, cx);
    }
}

impl Render for DesktopIconsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        self.scale = window.scale_factor();
        let cell = self.cell(cx);
        let model = self.model.read(cx);
        let shows = model.shows_icons();
        let dragged: HashSet<&String> = { self.carried.iter() }
            .flat_map(|carried| &carried.keys)
            .collect();
        let mut zones = Vec::new();
        if let Some(folder) = model.folder() {
            zones.push(DropZone {
                bounds: Bounds {
                    origin: Point::default(),
                    size: window.viewport_size(),
                },
                folder,
            });
        }
        let mut tiles = Vec::new();
        let mut carried = Vec::new();
        if shows {
            for item in model.items() {
                let Some(spot) = model
                    .spot(&item.key)
                    .filter(|spot| spot.display == self.display)
                else {
                    continue;
                };
                let bounds = Self::tile_bounds(spot, cell);
                if let (true, Some(path)) = (item.is_folder(), item.path()) {
                    zones.push(DropZone {
                        bounds,
                        folder: path.to_owned(),
                    });
                }
                let moving = dragged.contains(&item.key);
                let opacity = if moving { LEFT_BEHIND } else { 1. };
                tiles.push(self.tile(item, bounds, opacity, true, &theme, cx));
                if let (true, Some(drag)) = (moving, &self.carried) {
                    let moved = drag.to - drag.from;
                    let carried_bounds = Bounds {
                        origin: bounds.origin + moved,
                        size: bounds.size,
                    };
                    carried.push(self.tile(item, carried_bounds, CARRIED, false, &theme, cx));
                }
            }
        }
        self.zones.set(zones);
        let band = self.band.as_ref().map(|band| {
            let bounds = band.bounds();
            div()
                .absolute()
                .left(bounds.origin.x)
                .top(bounds.origin.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .bg(theme.colors.primary.opacity(0.18))
                .border_1()
                .border_color(theme.colors.primary.opacity(0.7))
        });
        let view = cx.entity().downgrade();
        // Where the pointer goes and where the button is let go is heard
        // from anywhere while a press of this window holds it.
        let listening = canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                let moving = view.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        moving
                            .update(cx, |this, cx| this.moved(event, window, cx))
                            .ok();
                    }
                });
                let releasing = view.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == gpui::DispatchPhase::Bubble && event.button == MouseButton::Left {
                        releasing
                            .update(cx, |this, cx| this.released(event, cx))
                            .ok();
                    }
                });
            },
        )
        .absolute()
        .size_full();
        div()
            .id("desktop-icons")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .font_family(theme.typography.family)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.press_left(event, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event, window, cx| this.press_right(event, window, cx)),
            )
            .on_key_down(cx.listener(|this, event, window, cx| this.key_down(event, window, cx)))
            .on_scroll_wheel(cx.listener(|this, event, _, cx| this.wheel(event, cx)))
            .child(listening)
            .children(tiles)
            .children(band)
            .when(!carried.is_empty(), |root| root.children(carried))
    }
}
