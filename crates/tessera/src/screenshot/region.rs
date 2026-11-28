// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The region tool: a rectangle chosen on a frozen picture of the screen.
//!
//! Every display gets a window above all others that shows its part of
//! the picture, dimmed outside the rectangle. A drag draws the rectangle,
//! across displays too; its handles and its inside move its sides and
//! itself; a click without a drag takes the rectangle of the window under
//! the pointer as the picture shows it. The arrows move the rectangle by a
//! pixel and, with Shift, move its right and bottom sides. Enter or a
//! double click takes it, Esc ends the tool without a picture. Beside the
//! pointer a magnifier shows the pixels under it, the colour of the one in
//! the middle and where it is, and the rectangle says its size in pixels.

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, CursorStyle, Entity, FocusHandle, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, RenderImage, Styled, Window, WindowHandle,
    black, canvas, div, fill, img, point, px, rgba, size,
};
use image::RgbaImage;

use super::{Screenshots, Shot, Then, cut, gpui_display, render_image};
use crate::i18n::Language;
use crate::platform::{Display, NativeSurface, ScreenRect, SmallBirth, pointer_at};
use crate::popup::floating_window;
use crate::theme::Theme;

/// How dark the picture is outside the rectangle, from 0 to 1.
const DIM: f32 = 0.45;

/// How far from a side or a corner of the rectangle a press takes it, in
/// physical pixels.
const REACH: i32 = 10;

/// How far the pointer moves before a press is a drag, in physical pixels.
const DRAG: i32 = 4;

/// The pixels the magnifier shows across, and how large each is drawn.
const LOOKED: i32 = 15;
const ENLARGED: f32 = 8.;

/// The side of a handle, in logical pixels.
const HANDLE: f32 = 8.;

/// A side or a corner of the rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    const ALL: [Self; 8] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Right,
        Self::BottomRight,
        Self::Bottom,
        Self::BottomLeft,
        Self::Left,
    ];

    /// Which sides it moves: left, top, right, bottom.
    fn sides(self) -> (bool, bool, bool, bool) {
        match self {
            Self::TopLeft => (true, true, false, false),
            Self::Top => (false, true, false, false),
            Self::TopRight => (false, true, true, false),
            Self::Right => (false, false, true, false),
            Self::BottomRight => (false, false, true, true),
            Self::Bottom => (false, false, false, true),
            Self::BottomLeft => (true, false, false, true),
            Self::Left => (true, false, false, false),
        }
    }

    /// Where it is on `rect`.
    fn spot(self, rect: ScreenRect) -> (i32, i32) {
        let middle = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
        let (left, top, right, bottom) = self.sides();
        let x = match (left, right) {
            (true, _) => rect.left,
            (_, true) => rect.right,
            _ => middle.0,
        };
        let y = match (top, bottom) {
            (true, _) => rect.top,
            (_, true) => rect.bottom,
            _ => middle.1,
        };
        (x, y)
    }
}

/// The rectangle with the corners `one` and `other`.
fn spanned(one: (i32, i32), other: (i32, i32)) -> ScreenRect {
    ScreenRect {
        left: one.0.min(other.0),
        top: one.1.min(other.1),
        right: one.0.max(other.0),
        bottom: one.1.max(other.1),
    }
}

/// `start` with the sides `handle` names moved by `by`, its sides put in
/// order again where they crossed.
fn resized(start: ScreenRect, handle: Handle, by: (i32, i32)) -> ScreenRect {
    let (left, top, right, bottom) = handle.sides();
    let mut moved = start;
    if left {
        moved.left += by.0;
    }
    if right {
        moved.right += by.0;
    }
    if top {
        moved.top += by.1;
    }
    if bottom {
        moved.bottom += by.1;
    }
    spanned((moved.left, moved.top), (moved.right, moved.bottom))
}

/// `rect` moved by `by`, kept inside `bounds` where it fits.
fn shifted(rect: ScreenRect, by: (i32, i32), bounds: ScreenRect) -> ScreenRect {
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let left = (rect.left + by.0).clamp(bounds.left, (bounds.right - width).max(bounds.left));
    let top = (rect.top + by.1).clamp(bounds.top, (bounds.bottom - height).max(bounds.top));
    ScreenRect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// The handle of `rect` at `spot`, if one is within reach.
fn handle_at(rect: ScreenRect, spot: (i32, i32)) -> Option<Handle> {
    Handle::ALL.into_iter().find(|handle| {
        let (x, y) = handle.spot(rect);
        (spot.0 - x).abs() <= REACH && (spot.1 - y).abs() <= REACH
    })
}

fn inside(rect: ScreenRect, spot: (i32, i32)) -> bool {
    (rect.left..rect.right).contains(&spot.0) && (rect.top..rect.bottom).contains(&spot.1)
}

/// The rectangle of the window at `spot`, from the front, within
/// `bounds`.
fn window_at(windows: &[ScreenRect], spot: (i32, i32), bounds: ScreenRect) -> Option<ScreenRect> {
    let found = windows.iter().find(|window| inside(**window, spot))?;
    let kept = ScreenRect {
        left: found.left.max(bounds.left),
        top: found.top.max(bounds.top),
        right: found.right.min(bounds.right),
        bottom: found.bottom.min(bounds.bottom),
    };
    (kept.right > kept.left && kept.bottom > kept.top).then_some(kept)
}

/// The rectangle after the key `key` with Shift held if `shift`: an arrow
/// moves it a pixel, or with Shift moves its right or bottom side.
fn nudged(rect: ScreenRect, key: &str, shift: bool, bounds: ScreenRect) -> Option<ScreenRect> {
    let by = match key {
        "left" => (-1, 0),
        "right" => (1, 0),
        "up" => (0, -1),
        "down" => (0, 1),
        _ => return None,
    };
    Some(match shift {
        true => {
            let mut grown = rect;
            grown.right = (grown.right + by.0).max(grown.left + 1);
            grown.bottom = (grown.bottom + by.1).max(grown.top + 1);
            grown
        }
        false => shifted(rect, by, bounds),
    })
}

/// What a press with the left button holds.
#[derive(Clone, Copy, Debug)]
enum Grab {
    /// A new rectangle from `from`, a drag once the pointer moved.
    New { from: (i32, i32), dragged: bool },
    /// The rectangle, moved with the pointer from `from`.
    Move { from: (i32, i32), start: ScreenRect },
    /// A side or a corner.
    Side {
        handle: Handle,
        from: (i32, i32),
        start: ScreenRect,
    },
}

/// The tool, shared by the windows of every display.
struct Region {
    shot: Shot,
    windows: Vec<ScreenRect>,
    then: Then,
    chosen: Option<ScreenRect>,
    grab: Option<Grab>,
    pointer: Option<(i32, i32)>,
    views: Vec<WindowHandle<RegionView>>,
    ended: bool,
}

impl Region {
    fn bounds(&self) -> ScreenRect {
        self.shot.frame
    }

    fn press(&mut self, spot: (i32, i32), clicks: usize) {
        if clicks >= 2 && self.chosen.is_some_and(|chosen| inside(chosen, spot)) {
            self.grab = None;
            return;
        }
        self.grab = Some(match self.chosen {
            Some(chosen) => match handle_at(chosen, spot) {
                Some(handle) => Grab::Side {
                    handle,
                    from: spot,
                    start: chosen,
                },
                None if inside(chosen, spot) => Grab::Move {
                    from: spot,
                    start: chosen,
                },
                None => Grab::New {
                    from: spot,
                    dragged: false,
                },
            },
            None => Grab::New {
                from: spot,
                dragged: false,
            },
        });
    }

    fn moved(&mut self, spot: (i32, i32)) {
        self.pointer = Some(spot);
        let bounds = self.bounds();
        match &mut self.grab {
            Some(Grab::New { from, dragged }) => {
                let far = (spot.0 - from.0).abs().max((spot.1 - from.1).abs()) >= DRAG;
                *dragged |= far;
                if *dragged {
                    self.chosen = Some(spanned(*from, spot));
                }
            }
            Some(Grab::Move { from, start }) => {
                let by = (spot.0 - from.0, spot.1 - from.1);
                self.chosen = Some(shifted(*start, by, bounds));
            }
            Some(Grab::Side {
                handle,
                from,
                start,
            }) => {
                let by = (spot.0 - from.0, spot.1 - from.1);
                self.chosen = Some(resized(*start, *handle, by));
            }
            None => {}
        }
    }

    /// The left button was let go at `spot`: a press that did not drag
    /// takes the window there, or the display where no window is.
    fn released(&mut self, spot: (i32, i32)) {
        if let Some(Grab::New { dragged: false, .. }) = self.grab {
            let bounds = self.bounds();
            let display = Display::at(spot).map(|display| display.frame);
            self.chosen = window_at(&self.windows, spot, bounds).or(display);
        }
        self.grab = None;
        let tiny = self
            .chosen
            .is_some_and(|chosen| chosen.right - chosen.left < 1 || chosen.bottom - chosen.top < 1);
        if tiny {
            self.chosen = None;
        }
    }
}

/// Opens the region tool.
pub struct RegionTool;

impl RegionTool {
    /// Shows `shot`, a picture of the whole screen with `windows` on it,
    /// on every display to choose a rectangle of, which is then done with
    /// as `then` says. False where no window could open.
    pub fn open(shot: Shot, windows: Vec<ScreenRect>, then: Then, cx: &mut App) -> bool {
        let region = cx.new(|_| Region {
            shot,
            windows,
            then,
            chosen: None,
            grab: None,
            pointer: pointer_at(),
            views: Vec::new(),
            ended: false,
        });
        let under = pointer_at().and_then(Display::at);
        let mut views = Vec::new();
        for display in Display::all() {
            let focus = under.is_some_and(|under| under.key == display.key);
            let picture = {
                let region = region.read(cx);
                cut(&region.shot.pixels, region.shot.frame, display.frame)
            };
            let Some(picture) = picture else {
                continue;
            };
            match open_view(region.clone(), display, render_image(&picture), focus, cx) {
                Ok(view) => views.push(view),
                Err(error) => report!("a display shows no region tool: {error:#}"),
            }
        }
        let opened = !views.is_empty();
        region.update(cx, |region, _| region.views = views);
        opened
    }
}

/// Ends the tool: with `take` the rectangle chosen, if one is, is done
/// with as asked.
fn end(region: &Entity<Region>, take: bool, cx: &mut App) {
    let ended = region.update(cx, |region, _| {
        if region.ended {
            return None;
        }
        region.ended = true;
        let chosen = region.chosen.filter(|_| take);
        let shot = chosen.and_then(|chosen| {
            let pixels = cut(&region.shot.pixels, region.shot.frame, chosen)?;
            Some(Shot {
                pixels: Arc::new(pixels),
                frame: chosen,
                taken: region.shot.taken,
            })
        });
        Some((std::mem::take(&mut region.views), shot, region.then))
    });
    let Some((views, shot, then)) = ended else {
        return;
    };
    for view in views {
        view.update(cx, |_, window, _| window.remove_window()).ok();
    }
    Screenshots::done(cx);
    if let Some(shot) = shot {
        Screenshots::finish(shot, then, cx);
    }
}

fn open_view(
    region: Entity<Region>,
    display: Display,
    picture: Arc<RenderImage>,
    focus: bool,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<RegionView>> {
    let frame = display.frame;
    let scale = display.scale;
    let rough = Bounds {
        origin: point(px(frame.left as f32 / scale), px(frame.top as f32 / scale)),
        size: size(
            px((frame.right - frame.left) as f32 / scale),
            px((frame.bottom - frame.top) as f32 / scale),
        ),
    };
    let options = floating_window(rough, gpui_display(&display, cx), focus);
    let _small = SmallBirth::begin();
    cx.open_window(options, |window, cx| {
        let adopted = NativeSurface::of(window).and_then(|native| {
            native.adopt_as_popup()?;
            native.place(frame)?;
            native.reveal(focus);
            Ok(())
        });
        if let Err(error) = adopted {
            report!("the region tool is not where it belongs: {error:#}");
        }
        cx.new(|cx| {
            cx.observe(&region, |_, _, cx| cx.notify()).detach();
            RegionView {
                region,
                display,
                picture,
                focus: cx.focus_handle(),
                focused: !focus,
            }
        })
    })
}

/// The window of the tool on one display.
struct RegionView {
    region: Entity<Region>,
    display: Display,
    /// The part of the picture this display shows.
    picture: Arc<RenderImage>,
    focus: FocusHandle,
    /// Whether the keyboard was given to it yet, where it is to have it.
    focused: bool,
}

impl RegionView {
    /// Where on the screen `position` of this window is, in physical
    /// pixels.
    fn screen(&self, position: Point<Pixels>) -> (i32, i32) {
        let scale = self.display.scale;
        (
            self.display.frame.left + (position.x / px(1.) * scale).round() as i32,
            self.display.frame.top + (position.y / px(1.) * scale).round() as i32,
        )
    }

    /// Where on this window the screen's `spot` is, in logical pixels.
    fn local(&self, spot: (i32, i32)) -> Point<Pixels> {
        let scale = self.display.scale;
        point(
            px((spot.0 - self.display.frame.left) as f32 / scale),
            px((spot.1 - self.display.frame.top) as f32 / scale),
        )
    }

    fn rect_bounds(&self, rect: ScreenRect) -> Bounds<Pixels> {
        let origin = self.local((rect.left, rect.top));
        let corner = self.local((rect.right, rect.bottom));
        Bounds {
            origin,
            size: size(corner.x - origin.x, corner.y - origin.y),
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let spot = self.screen(event.position);
        let take = event.click_count >= 2;
        self.region.update(cx, |region, cx| {
            region.press(spot, event.click_count);
            cx.notify();
        });
        if take
            && self
                .region
                .read(cx)
                .chosen
                .is_some_and(|chosen| inside(chosen, spot))
        {
            let region = self.region.clone();
            cx.defer(move |cx| end(&region, true, cx));
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let spot = self.screen(event.position);
        self.region.update(cx, |region, cx| {
            region.moved(spot);
            cx.notify();
        });
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let spot = self.screen(event.position);
        self.region.update(cx, |region, cx| {
            region.released(spot);
            cx.notify();
        });
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let region = self.region.clone();
        match key {
            "escape" => cx.defer(move |cx| end(&region, false, cx)),
            "enter" => cx.defer(move |cx| end(&region, true, cx)),
            _ => {
                let shift = event.keystroke.modifiers.shift;
                self.region.update(cx, |region, cx| {
                    let bounds = region.bounds();
                    let nudged = region
                        .chosen
                        .and_then(|chosen| nudged(chosen, key, shift, bounds));
                    if let Some(nudged) = nudged {
                        region.chosen = Some(nudged);
                        cx.notify();
                    }
                });
            }
        }
    }

    /// The dimmed picture around `chosen`, or all of it without one.
    fn shade(
        &self,
        chosen: Option<ScreenRect>,
        window_size: gpui::Size<Pixels>,
    ) -> Vec<AnyElement> {
        let dim = black().opacity(DIM);
        let quad = |left: Pixels, top: Pixels, width: Pixels, height: Pixels| {
            div()
                .absolute()
                .left(left)
                .top(top)
                .w(width.max(px(0.)))
                .h(height.max(px(0.)))
                .bg(dim)
                .into_any_element()
        };
        let Some(chosen) = chosen else {
            return vec![quad(px(0.), px(0.), window_size.width, window_size.height)];
        };
        let hole = self.rect_bounds(chosen);
        let (top, bottom) = (
            hole.origin.y.max(px(0.)),
            (hole.origin.y + hole.size.height).min(window_size.height),
        );
        let (left, right) = (
            hole.origin.x.max(px(0.)),
            (hole.origin.x + hole.size.width).min(window_size.width),
        );
        if bottom <= top || right <= left {
            return vec![quad(px(0.), px(0.), window_size.width, window_size.height)];
        }
        vec![
            quad(px(0.), px(0.), window_size.width, top),
            quad(
                px(0.),
                bottom,
                window_size.width,
                window_size.height - bottom,
            ),
            quad(px(0.), top, left, bottom - top),
            quad(right, top, window_size.width - right, bottom - top),
        ]
    }

    /// The outline of `chosen`, its handles and its size.
    fn outline(&self, chosen: ScreenRect, theme: &Theme, language: Language) -> Vec<AnyElement> {
        let bounds = self.rect_bounds(chosen);
        let mut drawn = vec![
            div()
                .absolute()
                .left(bounds.origin.x)
                .top(bounds.origin.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .border_1()
                .border_color(theme.colors.primary)
                .into_any_element(),
        ];
        for handle in Handle::ALL {
            let at = self.local(handle.spot(chosen));
            drawn.push(
                div()
                    .absolute()
                    .left(at.x - px(HANDLE / 2.))
                    .top(at.y - px(HANDLE / 2.))
                    .size(px(HANDLE))
                    .rounded(px(2.))
                    .bg(theme.colors.primary)
                    .border_1()
                    .border_color(theme.colors.surface_container)
                    .into_any_element(),
            );
        }
        let label = format!(
            "{} × {} {}",
            chosen.right - chosen.left,
            chosen.bottom - chosen.top,
            language.say("pixels")
        );
        let above = bounds.origin.y - px(28.);
        let top = if above < px(0.) {
            bounds.origin.y + px(6.)
        } else {
            above
        };
        drawn.push(
            div()
                .absolute()
                .left(bounds.origin.x)
                .top(top)
                .px(theme.spacing.small)
                .py(px(2.))
                .rounded(theme.shape.small)
                .bg(theme.colors.surface_container)
                .text_color(theme.colors.on_surface)
                .text_size(theme.typography.label_size)
                .child(label)
                .into_any_element(),
        );
        drawn
    }

    /// The magnifier beside `spot`: the pixels around it enlarged, the
    /// colour of the one in the middle and where it is.
    fn magnifier(
        &self,
        spot: (i32, i32),
        pixels: &RgbaImage,
        frame: ScreenRect,
        theme: &Theme,
    ) -> AnyElement {
        let side = LOOKED as f32 * ENLARGED;
        let half = LOOKED / 2;
        let mut colors: Vec<Hsla> = Vec::with_capacity((LOOKED * LOOKED) as usize);
        for row in -half..=half {
            for column in -half..=half {
                let (x, y) = (spot.0 + column - frame.left, spot.1 + row - frame.top);
                let pixel = match x >= 0
                    && y >= 0
                    && (x as u32) < pixels.width()
                    && (y as u32) < pixels.height()
                {
                    true => pixels.get_pixel(x as u32, y as u32).0,
                    false => [0, 0, 0, 255],
                };
                let [red, green, blue, _] = pixel;
                colors.push(rgba(u32::from_be_bytes([red, green, blue, 255])).into());
            }
        }
        let middle = { (spot.0 - frame.left, spot.1 - frame.top) };
        let hex = match middle.0 >= 0
            && middle.1 >= 0
            && (middle.0 as u32) < pixels.width()
            && (middle.1 as u32) < pixels.height()
        {
            true => {
                let [red, green, blue, _] = pixels.get_pixel(middle.0 as u32, middle.1 as u32).0;
                format!("#{red:02X}{green:02X}{blue:02X}")
            }
            false => String::new(),
        };
        let outline = theme.colors.primary;
        let grid = canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let cell = px(ENLARGED);
                for (index, color) in colors.iter().enumerate() {
                    let (row, column) = (index as i32 / LOOKED, index as i32 % LOOKED);
                    let at = bounds.origin + point(cell * column as f32, cell * row as f32);
                    window.paint_quad(fill(Bounds::new(at, size(cell, cell)), *color));
                }
                // The pixel in the middle.
                let at = bounds.origin + point(cell * half as f32, cell * half as f32);
                let ring = gpui::outline(
                    Bounds::new(at, size(cell, cell)),
                    outline,
                    Default::default(),
                );
                window.paint_quad(ring);
            },
        )
        .size(px(side));
        let local = self.local(spot);
        div()
            .absolute()
            .left(local.x + px(24.))
            .top(local.y + px(24.))
            .flex()
            .flex_col()
            .rounded(theme.shape.small)
            .overflow_hidden()
            .border_1()
            .border_color(theme.colors.outline)
            .bg(theme.colors.surface_container)
            .child(grid)
            .child(
                div()
                    .px(theme.spacing.small)
                    .py(px(2.))
                    .text_size(theme.typography.label_size)
                    .text_color(theme.colors.on_surface)
                    .child(format!("{hex}  {}, {}", spot.0, spot.1)),
            )
            .into_any_element()
    }
}

impl Render for RegionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus);
        }
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let region = self.region.read(cx);
        let (chosen, pointer, grabbing) = (region.chosen, region.pointer, region.grab.is_some());
        let here = pointer.filter(|spot| inside(self.display.frame, *spot));
        let window_size = window.viewport_size();
        let mut layers: Vec<AnyElement> = Vec::new();
        layers.push(
            img(self.picture.clone())
                .absolute()
                .size_full()
                .into_any_element(),
        );
        // The window a click would take, while nothing is chosen.
        let hovered = match (chosen, grabbing, here) {
            (None, false, Some(spot)) => window_at(&region.windows, spot, region.bounds()),
            _ => None,
        };
        layers.extend(self.shade(chosen.or(hovered), window_size));
        if let Some(hovered) = hovered {
            let bounds = self.rect_bounds(hovered);
            layers.push(
                div()
                    .absolute()
                    .left(bounds.origin.x)
                    .top(bounds.origin.y)
                    .w(bounds.size.width)
                    .h(bounds.size.height)
                    .border_2()
                    .border_color(theme.colors.primary.opacity(0.6))
                    .into_any_element(),
            );
        }
        if let Some(chosen) = chosen {
            layers.extend(self.outline(chosen, &theme, language));
        }
        if let Some(spot) = here {
            layers.push(self.magnifier(spot, &region.shot.pixels, region.shot.frame, &theme));
        }
        let hint = language.say(
            "Drag to choose a rectangle, or click a window. Enter takes it, Esc cancels; the arrows move it, and with Shift they resize it.",
        );
        layers.push(
            div()
                .absolute()
                .top(px(16.))
                .left(px(0.))
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .px(theme.spacing.medium)
                        .py(theme.spacing.small)
                        .rounded(theme.shape.full)
                        .bg(theme.colors.surface_container.opacity(0.9))
                        .text_color(theme.colors.on_surface)
                        .text_size(theme.typography.label_size)
                        .child(hint),
                )
                .into_any_element(),
        );
        div()
            .id("region-tool")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .cursor(CursorStyle::Crosshair)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| {
                    // A display that was not in front takes the keys
                    // once it is clicked.
                    window.focus(&this.focus);
                    this.mouse_down(event, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event, _, cx| this.mouse_move(event, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event, _, cx| this.mouse_up(event, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    let region = this.region.clone();
                    cx.defer(move |cx| end(&region, false, cx));
                }),
            )
            .children(layers)
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
    fn a_drag_spans_a_rectangle_either_way() {
        assert_eq!(spanned((10, 20), (5, 40)), rect(5, 20, 10, 40));
    }

    #[test]
    fn a_handle_moves_its_sides_and_crossing_sides_swap() {
        let start = rect(10, 10, 50, 50);
        assert_eq!(
            resized(start, Handle::BottomRight, (5, -5)),
            rect(10, 10, 55, 45)
        );
        assert_eq!(resized(start, Handle::Left, (60, 0)), rect(50, 10, 70, 50));
        assert_eq!(handle_at(start, (49, 31)), Some(Handle::Right));
        assert_eq!(handle_at(start, (30, 30)), None);
    }

    #[test]
    fn a_moved_rectangle_stays_on_the_screen() {
        let bounds = rect(0, 0, 100, 100);
        assert_eq!(
            shifted(rect(10, 10, 30, 30), (100, -100), bounds),
            rect(80, 0, 100, 20)
        );
    }

    #[test]
    fn the_arrows_move_and_with_shift_resize() {
        let bounds = rect(0, 0, 100, 100);
        let start = rect(10, 10, 20, 20);
        assert_eq!(
            nudged(start, "left", false, bounds),
            Some(rect(9, 10, 19, 20))
        );
        assert_eq!(
            nudged(start, "down", true, bounds),
            Some(rect(10, 10, 20, 21))
        );
        assert_eq!(
            nudged(rect(10, 10, 11, 11), "up", true, bounds),
            Some(rect(10, 10, 11, 11))
        );
        assert_eq!(nudged(start, "a", false, bounds), None);
    }

    #[test]
    fn a_click_takes_the_window_in_front_there() {
        let windows = [rect(0, 0, 50, 50), rect(-20, 0, 200, 200)];
        let bounds = rect(0, 0, 100, 100);
        assert_eq!(
            window_at(&windows, (10, 10), bounds),
            Some(rect(0, 0, 50, 50))
        );
        assert_eq!(
            window_at(&windows, (60, 10), bounds),
            Some(rect(0, 0, 100, 100))
        );
        assert_eq!(window_at(&windows, (300, 300), bounds), None);
    }
}
