// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The translucent shape that shows where a dragged window snaps once it
//! is let go. It lies over the work area of the display the pointer is
//! on, lets the pointer through and never takes the keyboard; the shape
//! moves from one place to the next instead of jumping.

use gpui::{
    App, Context, IntoElement, ParentElement, Render, Styled, Window, WindowHandle, div, px,
};

use super::{Kind, move_floating, open_at};
use crate::motion::Spring;
use crate::platform::{Display, ScreenRect};
use crate::popup::retire_floating;
use crate::theme::Theme;
use crate::ui::release_hover;

/// How much of its place the shape takes when it first appears, growing
/// from there to all of it.
const GROWN_FROM: f32 = 0.85;

/// How opaque the inside and the border of the shape are.
const FILL: f32 = 0.22;
const BORDER: f32 = 0.7;

/// The root view of the window of the shape.
pub struct PreviewView {
    /// The work area the window lies over, on the screen.
    over: ScreenRect,
    /// Physical pixels per logical one there.
    scale: f32,
    /// The left, top, right and bottom of the shape, in logical pixels of
    /// the window.
    sides: [Spring; 4],
    /// 0 while hidden, 1 when fully shown.
    shown: Spring,
    closing: bool,
}

impl PreviewView {
    /// Opens the window over the work area of `display`, the shape growing
    /// into `rect`.
    pub fn open(
        display: Display,
        rect: ScreenRect,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<Self>> {
        open_at(display.work, &display, Kind::Overlay, cx, |_, cx| {
            let motion = cx.global::<Theme>().motion;
            let mut view = Self {
                over: display.work,
                scale: display.scale,
                sides: [Spring::new(motion.spatial, 0.); 4],
                shown: Spring::new(motion.effects, 0.),
                closing: false,
            };
            view.appear(rect);
            view
        })
    }

    /// Shows the shape on its way to `aim`, the rect of a display, or has
    /// it fade where there is none.
    pub fn aim(
        &mut self,
        aim: Option<(Display, ScreenRect)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((display, rect)) = aim else {
            self.shown.animate_to(0.);
            cx.notify();
            return;
        };
        let hidden = self.shown.target() == 0. || self.closing;
        if display.work != self.over {
            self.over = display.work;
            self.scale = display.scale;
            move_floating(window, display.work, cx);
            self.appear(rect);
        } else if hidden {
            self.appear(rect);
        } else {
            let logical = self.logical(rect);
            for (side, to) in self.sides.iter_mut().zip(logical) {
                side.animate_to(to);
            }
        }
        self.closing = false;
        self.shown.animate_to(1.);
        cx.notify();
    }

    /// Fades the shape; the window is put away once it is gone.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.shown.animate_to(0.);
        cx.notify();
    }

    /// Has the shape grow into `rect` from a little inside it.
    fn appear(&mut self, rect: ScreenRect) {
        let [left, top, right, bottom] = self.logical(rect);
        let (middle_x, middle_y) = ((left + right) / 2., (top + bottom) / 2.);
        let grown = |from: f32, middle: f32| middle + (from - middle) * GROWN_FROM;
        let starts = [
            grown(left, middle_x),
            grown(top, middle_y),
            grown(right, middle_x),
            grown(bottom, middle_y),
        ];
        for ((side, start), to) in self
            .sides
            .iter_mut()
            .zip(starts)
            .zip([left, top, right, bottom])
        {
            side.jump_to(start);
            side.animate_to(to);
        }
        self.shown.jump_to(0.);
    }

    /// `rect` in logical pixels of the window.
    fn logical(&self, rect: ScreenRect) -> [f32; 4] {
        let scale = self.scale.max(0.1);
        [
            (rect.left - self.over.left) as f32 / scale,
            (rect.top - self.over.top) as f32 / scale,
            (rect.right - self.over.left) as f32 / scale,
            (rect.bottom - self.over.top) as f32 / scale,
        ]
    }
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let shown = self.shown.value(window).clamp(0., 1.);
        if self.closing && self.shown.is_settled() {
            retire_floating(window, cx);
        }
        let [left, top, right, bottom] = self.sides.each_mut().map(|side| side.value(window));
        let inset = theme.spacing.small / px(1.);
        let shape = div()
            .absolute()
            .left(px(left + inset))
            .top(px(top + inset))
            .w(px((right - left - inset * 2.).max(0.)))
            .h(px((bottom - top - inset * 2.).max(0.)))
            .rounded(theme.shape.large)
            .bg(theme.colors.primary.opacity(FILL))
            .border_2()
            .border_color(theme.colors.primary.opacity(BORDER))
            .opacity(shown);
        div().size_full().relative().child(shape)
    }
}
