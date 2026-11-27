// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The overflow of the notification area: a grid of the icons that do not
//! stand in the bar, which opens from the button at the end of a strip,
//! beside the bar. Its icons pass clicks and descriptions on as the strip
//! does, are dragged to another place in it or out onto a strip, and the
//! keys move through them while it has the keyboard.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyWindowHandle, App, Bounds, Context, DisplayId, Entity, EntityId, FocusHandle, Global,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseUpEvent, ParentElement,
    Pixels, Render, Styled, Subscription, WeakEntity, Window, WindowHandle, div, point, px, size,
};

use super::model::TrayModel;
use super::{
    IconHost, KeyDeed, Layout, Side, Slot, Strips, TIP_KIND, TrayDrag, TrayStrip, button_side,
    contains, drop_at, icon_button, insertion, measure, own_menu_entries, press_with_keyboard,
    screen_point, tip_line,
};
use crate::menu::Menus;
use crate::motion::{Interaction, Spring};
use crate::platform::{
    NativeSurface, ScreenEdge, ScreenRect, Tray, TrayButton, pointer_at, pointer_held,
};
use crate::popup::{
    MARGIN, PopupAnchor, REOPEN_GUARD, floating_panel, open_floating, retire_floating,
};
use crate::theme::Theme;
use crate::tip::Tips;
use crate::tray_settings::{Place, TraySettings};
use crate::ui::release_hover;

/// The most icons in a row of the grid.
const COLUMNS: usize = 5;

/// Where an overflow opens and for which strip.
pub struct Opening {
    pub strip: WeakEntity<TrayStrip>,
    pub model: Entity<TrayModel>,
    /// The options of the strip, which say what its overflow holds.
    pub bluetooth: bool,
    pub limit: Option<usize>,
    /// The button that opens it, the surface of the strip and its display,
    /// on the screen.
    pub button: ScreenRect,
    pub surface: ScreenRect,
    pub screen: ScreenRect,
    /// The side of the display the strip stands at.
    pub from: Side,
    pub scale: f32,
    pub display_id: Option<DisplayId>,
    /// Whether it takes the keyboard on its first icon.
    pub keyboard: bool,
}

/// The open overflow, if one is.
#[derive(Default)]
pub struct TrayOverflow {
    open: Option<(WeakEntity<TrayStrip>, WindowHandle<OverflowView>)>,
    /// The strip whose overflow closed last, and when.
    closed: Option<(EntityId, Instant)>,
}

impl Global for TrayOverflow {}

impl TrayOverflow {
    /// Opens the overflow `opening` describes; another one closes.
    pub fn open(opening: Opening, cx: &mut App) {
        Self::close(cx);
        Tips::close(cx);
        let theme = *cx.global::<Theme>();
        let (_, overflow) = opening
            .model
            .read(cx)
            .arranged(opening.bluetooth, opening.limit);

        if overflow.is_empty() {
            return;
        }
        let (width, height) = extent(overflow.len(), &theme);
        let frame = frame_of(&opening, width, height);
        let rough = Bounds {
            origin: point(
                px(frame.left as f32 / opening.scale),
                px(frame.top as f32 / opening.scale),
            ),
            size: size(px(width), px(height)),
        };
        let strip = opening.strip.clone();
        let display_id = opening.display_id;
        let opened = open_floating(rough, display_id, true, cx, move |window, cx| {
            let adopted = NativeSurface::of(window).and_then(|native| {
                native.adopt_as_popup()?;
                native.place(frame)?;
                native.reveal(true);
                Ok(())
            });
            if let Err(error) = adopted {
                report!("the overflow of the notification area is not where it belongs: {error:#}");
            }
            OverflowView::new(opening, frame, window, cx)
        });
        match opened {
            Ok(window) => cx.default_global::<Self>().open = Some((strip, window)),
            Err(error) => report!("the overflow of the notification area did not open: {error:#}"),
        }
    }

    /// Closes the open overflow, if there is one.
    pub fn close(cx: &mut App) {
        if let Some((_, window)) = cx.default_global::<Self>().open.take() {
            // An overflow that has closed by itself is gone, which is fine.
            window.update(cx, |view, _, cx| view.close(cx)).ok();
        }
    }

    /// Whether the overflow of `strip` closed a moment ago: the click on
    /// its button that closed it by taking the keyboard from it is not to
    /// open it again.
    pub fn closed_lately(strip: &WeakEntity<TrayStrip>, cx: &App) -> bool {
        let closed = cx.try_global::<Self>().and_then(|this| this.closed);
        closed.is_some_and(|(of, at)| of == strip.entity_id() && at.elapsed() < REOPEN_GUARD)
    }

    /// A press of the pointer in the window of the strip `strip` ended;
    /// `dragged` says whether it dragged an icon. An overflow of that strip
    /// that stayed open through the press closes after a click, as it
    /// would have when the press took the keyboard from it. After a drag,
    /// which may have put an icon into it, it takes the keyboard back, so
    /// that the next click elsewhere closes it.
    pub fn press_ended(strip: EntityId, dragged: bool, cx: &mut App) {
        let open = cx.try_global::<Self>().and_then(|this| this.open.clone());
        let Some((_, window)) = open.filter(|(of, _)| of.entity_id() == strip) else {
            return;
        };
        window
            .update(cx, |view, window, cx| {
                if !std::mem::take(&mut view.held) {
                    return;
                }
                let back =
                    dragged && NativeSurface::of(window).is_ok_and(|native| native.take_keyboard());
                if !back {
                    view.close(cx);
                }
            })
            .ok();
    }

    /// Whether the overflow of `strip` is open.
    pub fn is_open_for(strip: &WeakEntity<TrayStrip>, cx: &App) -> bool {
        let open = cx.try_global::<Self>().and_then(|this| this.open.as_ref());
        open.is_some_and(|(of, _)| of.entity_id() == strip.entity_id())
    }

    /// Takes the icon `id` that was let go at `point` of the screen, if
    /// the open overflow is there; whether it did.
    pub fn take_drop(id: u64, point: (i32, i32), cx: &mut App) -> bool {
        let Some((_, window)) = cx.try_global::<Self>().and_then(|this| this.open.clone()) else {
            return false;
        };
        let taken = window.update(cx, |view, window, cx| {
            let local = view.local(point, window)?;
            view.put(id, local, window, cx);
            Some(())
        });
        matches!(taken, Ok(Some(())))
    }
}

/// The logical width and height of the panel of an overflow of `count`
/// icons.
fn extent(count: usize, theme: &Theme) -> (f32, f32) {
    let columns = count.clamp(1, COLUMNS);
    let rows = count.div_ceil(columns).max(1);
    let side = button_side(theme) / px(1.);
    let gap = theme.spacing.tiny / px(1.);
    let padding = theme.spacing.small / px(1.);
    let length = |cells: usize| cells as f32 * side + (cells as f32 - 1.) * gap + padding * 2.;
    (length(columns), length(rows))
}

/// The frame on the screen of the window of an overflow whose panel is
/// `width` by `height` logical pixels: beside the strip, at its button,
/// kept on the display.
fn frame_of(opening: &Opening, width: f32, height: f32) -> ScreenRect {
    let physical = |length: f32| ((length + MARGIN * 2.) * opening.scale).round() as i32;
    placed(
        opening.from,
        opening.button,
        opening.surface,
        opening.screen,
        (physical(width), physical(height)),
    )
}

/// The frame `size` large that opens from the button `button` of a strip
/// on `surface` standing at `from` of `screen`.
fn placed(
    from: Side,
    button: ScreenRect,
    surface: ScreenRect,
    screen: ScreenRect,
    (width, height): (i32, i32),
) -> ScreenRect {
    let along = |middle: i32, low: i32, high: i32, length: i32| {
        (middle - length / 2).clamp(low, (high - length).max(low))
    };
    let (left, top) = match from {
        Side::Top | Side::Bottom => {
            let left = along(
                i32::midpoint(button.left, button.right),
                screen.left,
                screen.right,
                width,
            );
            let top = match from {
                Side::Top => surface.bottom,
                _ => surface.top - height,
            };
            (left, top)
        }
        Side::Left | Side::Right => {
            let top = along(
                i32::midpoint(button.top, button.bottom),
                screen.top,
                screen.bottom,
                height,
            );
            let left = match from {
                Side::Left => surface.right,
                _ => surface.left - width,
            };
            (left, top)
        }
    };
    ScreenRect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// One icon of the overflow and its pointer feedback.
struct OverflowCell {
    interaction: Interaction,
    drawn: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// The root view of the window of an overflow.
pub struct OverflowView {
    model: Entity<TrayModel>,
    strip: WeakEntity<TrayStrip>,
    bluetooth: bool,
    limit: Option<usize>,
    /// Where it opens from, for when the number of its icons changes.
    opening: Opening,
    /// The frame its window was placed at.
    frame: ScreenRect,
    /// The icons, in their order.
    ids: Vec<u64>,
    cells: HashMap<u64, OverflowCell>,
    /// 0 while hidden, 1 when fully open.
    visibility: Spring,
    closing: bool,
    /// Whether the window has had keyboard focus; before that, not having
    /// it does not mean the user turned away.
    was_active: bool,
    /// Whether the window lost the keyboard to a press of the pointer on
    /// the bar of its strip, which may start dragging an icon into it: it
    /// stays open until the press ends.
    held: bool,
    focus: FocusHandle,
    /// The icon the keyboard is on, by its place in the grid.
    keyboard: Option<usize>,
    /// Whether the keyboard came from the strip, where it goes back.
    keyboard_from_strip: bool,
    /// The icon being dragged in this window, and where it would go.
    dragged: Option<u64>,
    aim: Option<usize>,
    /// The window of the view.
    own: AnyWindowHandle,
    _subscriptions: Vec<Subscription>,
}

impl IconHost for OverflowView {
    fn model(&self) -> &Entity<TrayModel> {
        &self.model
    }

    fn open_own_menu(
        &mut self,
        id: u64,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = own_menu_entries(&self.model, id, false, cx);
        let middle = point(
            bounds.origin.x + bounds.size.width / 2.,
            bounds.origin.y + bounds.size.height,
        );
        match PopupAnchor::under(window, middle, cx) {
            Ok(anchor) => Menus::open(entries, anchor, cx),
            Err(error) => report!("the menu of a notification icon has nowhere to open: {error:#}"),
        }
    }

    fn drawn(&self, id: u64) -> Option<Bounds<Pixels>> {
        self.cells.get(&id)?.drawn.get()
    }
}

impl OverflowView {
    fn new(
        opening: Opening,
        frame: ScreenRect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let motion = cx.global::<Theme>().motion;
        let mut visibility = Spring::new(motion.spatial, 0.);
        visibility.animate_to(1.);
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.was_active = true;
                this.held = false;
            } else if this.was_active {
                let on_bar = pointer_at().is_some_and(|at| contains(this.opening.surface, at));
                match pointer_held() && on_bar {
                    true => this.held = true,
                    false => this.close(cx),
                }
            }
        });
        let changes = cx.observe(&opening.model, |this, _, cx| this.sync(cx));
        let focus = cx.focus_handle();
        window.focus(&focus);
        let keyboard = opening.keyboard;
        let mut view = Self {
            model: opening.model.clone(),
            strip: opening.strip.clone(),
            bluetooth: opening.bluetooth,
            limit: opening.limit,
            opening,
            frame,
            ids: Vec::new(),
            cells: HashMap::new(),
            visibility,
            closing: false,
            was_active: false,
            held: false,
            focus,
            keyboard: keyboard.then_some(0),
            keyboard_from_strip: keyboard,
            dragged: None,
            aim: None,
            own: window.window_handle(),
            _subscriptions: vec![activation, changes],
        };
        view.sync(cx);
        view
    }

    /// Takes over the icons of the overflow from the model, and closes
    /// once there are none.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let (_, ids) = self.model.read(cx).arranged(self.bluetooth, self.limit);
        if ids.is_empty() && !self.closing {
            self.close(cx);
        }
        let motion = cx.global::<Theme>().motion;
        self.cells.retain(|id, _| ids.contains(id));
        for &id in &ids {
            self.cells.entry(id).or_insert_with(|| OverflowCell {
                interaction: Interaction::new(&motion),
                drawn: Rc::default(),
            });
        }
        if let Some(chosen) = self.keyboard {
            self.keyboard = Some(chosen.min(ids.len().saturating_sub(1)));
        }
        if ids.len() != self.ids.len() {
            self.ids = ids;
            self.fit(cx);
        } else {
            self.ids = ids;
        }
        cx.notify();
    }

    /// Makes the window as large as its icons need, once this render is
    /// over.
    fn fit(&mut self, cx: &mut Context<Self>) {
        if self.ids.is_empty() {
            return;
        }
        let theme = *cx.global::<Theme>();
        let (width, height) = extent(self.ids.len(), &theme);
        let frame = frame_of(&self.opening, width, height);
        if frame == self.frame {
            return;
        }
        self.frame = frame;
        let window = self.own;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, _| {
                if let Err(error) = NativeSurface::of(window).and_then(|native| native.place(frame))
                {
                    report!("the overflow of the notification area keeps its size: {error:#}");
                }
            });
        });
    }

    /// Starts closing; the window goes away once the motion has finished.
    fn close(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.visibility.animate_to(0.);
        let ours = self.own;
        if let Some(overflow) = cx.try_global::<TrayOverflow>()
            && overflow
                .open
                .as_ref()
                .is_some_and(|(_, window)| AnyWindowHandle::from(*window) == ours)
        {
            let overflow = cx.global_mut::<TrayOverflow>();
            overflow.open = None;
            overflow.closed = Some((self.strip.entity_id(), Instant::now()));
        }
        cx.notify();
    }

    /// How many icons a row of the grid holds.
    fn columns(&self) -> usize {
        self.ids.len().clamp(1, COLUMNS)
    }

    /// The point `point` of the screen in logical pixels of the window, if
    /// it is on the window.
    fn local(&self, point: (i32, i32), window: &Window) -> Option<(f32, f32)> {
        let frame = NativeSurface::of(window).ok()?.frame().ok()?;
        let scale = window.scale_factor();
        contains(frame, point).then(|| {
            (
                (point.0 - frame.left) as f32 / scale,
                (point.1 - frame.top) as f32 / scale,
            )
        })
    }

    /// Where among the icons an icon dropped at `point` of the window goes.
    fn insertion_at(&self, point: (f32, f32)) -> usize {
        let slots: Vec<Slot> = self
            .ids
            .iter()
            .filter_map(|id| Some(Slot::of(self.cells.get(id)?.drawn.get()?)))
            .collect();
        insertion(&slots, point, Layout::Grid)
    }

    /// Puts the icon `id` where `point` of the window is among the icons.
    fn put(&mut self, id: u64, point: (f32, f32), _: &Window, cx: &mut Context<Self>) {
        self.aim = None;
        let mut index = self.insertion_at(point);
        if let Some(at) = self.ids.iter().position(|&other| other == id)
            && at < index
        {
            index -= 1;
        }
        let model = self.model.read(cx);
        let arranged = model.arranged(self.bluetooth, self.limit);
        if let Some(config) = model.put(id, Place::Overflow, index, arranged) {
            TraySettings::change(config, cx);
        }
    }

    /// The pointer came onto the icon `id`, or left it.
    fn tip(&self, id: u64, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        let key = (TIP_KIND, id);
        let icon = self.model.read(cx).icon(id);
        let target = icon
            .filter(|_| hovered)
            .and_then(|icon| Some((icon, self.drawn(id)?)));
        let Some((icon, drawn)) = target else {
            Tips::leave(key, cx);
            Tray::hover(id, false);
            return;
        };
        if let Some(frame) = self.screen_frame(drawn, window) {
            let middle = (
                i32::midpoint(frame.left, frame.right),
                i32::midpoint(frame.top, frame.bottom),
            );
            Tray::locate(id, frame, middle);
        }
        Tray::hover(id, true);
        if icon.icon.draws_own_tip {
            return;
        }
        let tip = tip_line(&icon.icon.tip);
        match PopupAnchor::at(window, drawn.origin.x + drawn.size.width / 2., cx) {
            Ok(anchor) => Tips::request(key, tip.into(), anchor, cx),
            Err(error) => report!("a description has nowhere to open: {error:#}"),
        }
    }

    /// Where `bounds` of the window are on the screen.
    fn screen_frame(&self, bounds: Bounds<Pixels>, window: &Window) -> Option<ScreenRect> {
        let frame = NativeSurface::of(window).ok()?.frame().ok()?;
        let scale = window.scale_factor();
        let physical = |length: Pixels| (length / px(1.) * scale).round() as i32;
        Some(ScreenRect {
            left: frame.left + physical(bounds.origin.x),
            top: frame.top + physical(bounds.origin.y),
            right: frame.left + physical(bounds.origin.x + bounds.size.width),
            bottom: frame.top + physical(bounds.origin.y + bounds.size.height),
        })
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(deed) = KeyDeed::of(event) else {
            return;
        };
        cx.stop_propagation();
        let count = self.ids.len();
        let Some(chosen) = self.keyboard else {
            // The first key that moves puts the keyboard on the first icon.
            if matches!(deed, KeyDeed::Leave) {
                self.close(cx);
            } else if count > 0 {
                self.keyboard = Some(0);
                cx.notify();
            }
            return;
        };
        match deed {
            KeyDeed::Leave => {
                self.close(cx);
                if self.keyboard_from_strip {
                    self.back_to_strip(cx);
                }
            }
            KeyDeed::Take | KeyDeed::Menu => {
                let Some(&id) = self.ids.get(chosen) else {
                    return;
                };
                let button = match deed {
                    KeyDeed::Take => TrayButton::Keyboard,
                    _ => TrayButton::KeyboardMenu,
                };
                // The location of the icon is told before the application
                // opens anything there.
                if let Some(frame) = self
                    .drawn(id)
                    .and_then(|drawn| self.screen_frame(drawn, window))
                {
                    let middle = (
                        i32::midpoint(frame.left, frame.right),
                        i32::midpoint(frame.top, frame.bottom),
                    );
                    Tray::locate(id, frame, middle);
                }
                press_with_keyboard(id, button, &self.model, self.strip.entity_id(), cx);
            }
            deed => {
                self.keyboard = Some(deed.moved(chosen, count, self.columns()));
                cx.notify();
            }
        }
    }

    /// Gives the keyboard back to the strip, on the button of the overflow.
    fn back_to_strip(&self, cx: &mut Context<Self>) {
        let Some(strip) = self.strip.upgrade() else {
            return;
        };
        let shown = cx.default_global::<Strips>().shown.clone();
        let Some((_, window)) = shown
            .into_iter()
            .find(|(of, _)| of.entity_id() == strip.entity_id())
        else {
            return;
        };
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                strip.update(cx, |strip, cx| {
                    let button = strip.places().saturating_sub(1);
                    strip.take_keyboard(button, window, cx);
                });
            });
        });
    }

    fn render_cell(
        &mut self,
        index: usize,
        id: u64,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let image = self
            .model
            .read(cx)
            .icon(id)
            .and_then(|icon| icon.image.clone());
        let Some(cell) = self.cells.get_mut(&id) else {
            return div().into_any_element();
        };
        let state = cell.interaction.state(window);
        let drawn = cell.drawn.clone();
        let focused = self.keyboard == Some(index);
        let dragged = self.dragged == Some(id) && cx.has_active_drag();
        let button =
            icon_button(id, image, state, focused, theme, cx).child(measure(drawn, |_| {}));
        let button = Interaction::track_hover(
            button,
            cx,
            move |this: &mut Self| Some(&mut this.cells.get_mut(&id)?.interaction),
            move |this, hovered, window, cx| this.tip(id, hovered, window, cx),
        );
        div()
            .flex_none()
            .opacity(if dragged { super::DRAGGED_OPACITY } else { 1. })
            .child(button)
            .into_any_element()
    }
}

impl Render for OverflowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window);
        if self.closing && self.visibility.is_settled() {
            retire_floating(window, cx);
        }
        let aim = self.aim.filter(|_| cx.has_active_drag());
        let ids = self.ids.clone();
        let mut cells = Vec::with_capacity(ids.len() + 1);
        for (index, &id) in ids.iter().enumerate() {
            if aim == Some(index) {
                cells.push(super::TrayStrip::marker(true, &theme));
            }
            cells.push(self.render_cell(index, id, &theme, window, cx));
        }
        if aim.is_some_and(|aim| aim >= ids.len()) {
            cells.push(super::TrayStrip::marker(true, &theme));
        }
        let travel_from = match self.opening.from {
            Side::Top => ScreenEdge::Top,
            _ => ScreenEdge::Bottom,
        };
        let grid = div()
            .id("tray-overflow")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .flex()
            .flex_wrap()
            .content_start()
            .gap(theme.spacing.tiny)
            .on_drag_move(
                cx.listener(|this, event: &gpui::DragMoveEvent<TrayDrag>, _, cx| {
                    this.dragged = Some(event.drag(cx).id);
                    let position = event.event.position;
                    let aim = event
                        .bounds
                        .contains(&position)
                        .then(|| this.insertion_at((position.x / px(1.), position.y / px(1.))));
                    if this.aim != aim {
                        this.aim = aim;
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(|this, drag: &TrayDrag, window, cx| {
                this.dragged = None;
                let position = window.mouse_position();
                this.put(
                    drag.id,
                    (position.x / px(1.), position.y / px(1.)),
                    window,
                    cx,
                );
            }))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.aim = None;
                    if let Some(id) = this.dragged.take().filter(|_| cx.has_active_drag())
                        && let Some(point) = screen_point(window, event.position)
                    {
                        cx.defer(move |cx| drop_at(id, point, cx));
                    }
                }),
            )
            .children(cells);
        floating_panel(&theme, travel_from, visibility, |panel| {
            panel.p(theme.spacing.small).child(grid)
        })
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

    #[test]
    fn the_overflow_opens_beside_the_bar_at_its_button() {
        let screen = rect(0, 0, 1920, 1080);
        let bottom = rect(0, 1032, 1920, 1080);
        let button = rect(1800, 1040, 1830, 1070);
        assert_eq!(
            placed(Side::Bottom, button, bottom, screen, (200, 100)),
            rect(1715, 932, 1915, 1032)
        );
        // Kept on the display.
        let corner = rect(1890, 1040, 1920, 1070);
        assert_eq!(
            placed(Side::Bottom, corner, bottom, screen, (200, 100)).right,
            1920
        );
        let top = rect(0, 0, 1920, 48);
        assert_eq!(placed(Side::Top, button, top, screen, (200, 100)).top, 48);
        let left = rect(0, 0, 48, 1080);
        let button = rect(8, 500, 40, 532);
        assert_eq!(
            placed(Side::Left, button, left, screen, (200, 100)),
            rect(48, 466, 248, 566)
        );
        let right = rect(1872, 0, 1920, 1080);
        assert_eq!(
            placed(Side::Right, button, right, screen, (200, 100)).right,
            1872
        );
    }

    #[test]
    fn the_grid_has_rows_of_at_most_five() {
        let theme = Theme::default();
        let (one_wide, one_high) = extent(1, &theme);
        let (five_wide, five_high) = extent(5, &theme);
        let (six_wide, six_high) = extent(6, &theme);
        assert!(five_wide > one_wide);
        assert_eq!(one_high, five_high);
        assert_eq!(six_wide, five_wide);
        assert!(six_high > five_high);
    }
}
