// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A track with a handle that sets a share, from 0 to 1, by a drag, a
//! click or the wheel.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, Context, Div, InteractiveElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, ScrollDelta, ScrollWheelEvent, Stateful, Styled, Window, canvas, div,
    px,
};

use crate::motion::Spring;
use crate::theme::Theme;

/// The part of the range one notch of the mouse wheel moves the share.
const WHEEL_STEP: f32 = 0.02;

/// The share for a pointer at `x`, on a track that starts at `left` and is
/// `width` wide, with `inset` at each end taken by half the handle.
fn share_at(x: f32, left: f32, width: f32, inset: f32) -> f32 {
    let travel = width - inset * 2.;
    if travel <= 0. {
        return 0.;
    }
    ((x - left - inset) / travel).clamp(0., 1.)
}

/// What a view keeps for a slider it shows.
pub struct Slider {
    /// The share as drawn; it follows the real one.
    shown: Spring,
    dragging: bool,
    /// Drawn in the colors of what is switched off.
    dimmed: bool,
    /// Where the track was painted last, in window coordinates.
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl Slider {
    /// A slider that starts at `share`.
    pub fn new(theme: &Theme, share: f32) -> Self {
        Self {
            shown: Spring::new(theme.motion.effects, share),
            dragging: false,
            dimmed: false,
            bounds: Rc::default(),
        }
    }

    /// Draws the slider in the colors of what is switched off, or in its
    /// own.
    pub fn dim(&mut self, dimmed: bool) {
        self.dimmed = dimmed;
    }

    /// The share under a pointer at `x` of the window.
    fn share_under(&self, x: Pixels, theme: &Theme) -> f32 {
        let track = self.bounds.get();
        let slider = theme.slider;
        let inset = f32::from(slider.handle_width) / 2. + f32::from(slider.handle_gap);
        share_at(
            f32::from(x),
            f32::from(track.origin.x),
            f32::from(track.size.width),
            inset,
        )
    }

    /// The track, with the handle at `share`. `of` finds the slider in the
    /// view that shows it, and `set` is told every share the pointer asks
    /// for.
    pub fn track<V: 'static>(
        &mut self,
        id: &'static str,
        share: f32,
        window: &mut Window,
        cx: &mut Context<V>,
        of: fn(&mut V) -> &mut Self,
        set: fn(&mut V, f32, &mut Context<V>),
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let slider = theme.slider;
        self.shown.animate_to(share);
        let shown = self.shown.value(window).clamp(0., 1.);
        let width = self.bounds.get().size.width;
        let travel = (width - slider.handle_width - slider.handle_gap * 2.).max(px(0.));
        let (active, inactive) = if self.dimmed {
            (theme.colors.outline, theme.colors.surface_container_highest)
        } else {
            (theme.colors.primary, theme.colors.secondary_container)
        };
        let follow = move |view: &mut V, x: Pixels, cx: &mut Context<V>| {
            let theme = *cx.global::<Theme>();
            let share = of(view).share_under(x, &theme);
            set(view, share, cx);
        };
        let painted = self.bounds.clone();
        div()
            .id(id)
            .flex_1()
            .h(slider.handle_height)
            .relative()
            .flex()
            .items_center()
            .gap(slider.handle_gap)
            .child(
                canvas(move |bounds, _, _| painted.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .flex_none()
                    .w(travel * shown)
                    .h(slider.track_height)
                    .rounded_l(theme.shape.full)
                    .rounded_r(slider.inner_radius)
                    .bg(active),
            )
            .child(
                div()
                    .flex_none()
                    .w(slider.handle_width)
                    .h_full()
                    .rounded(theme.shape.full)
                    .bg(active),
            )
            .child(
                div()
                    .flex_none()
                    .w(travel * (1. - shown))
                    .h(slider.track_height)
                    .rounded_l(slider.inner_radius)
                    .rounded_r(theme.shape.full)
                    .bg(inactive),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, _, cx| {
                    of(view).dragging = true;
                    follow(view, event.position.x, cx);
                }),
            )
            .on_mouse_move(cx.listener(move |view, event: &MouseMoveEvent, _, cx| {
                if !of(view).dragging {
                    return;
                }
                if event.pressed_button == Some(MouseButton::Left) {
                    follow(view, event.position.x, cx);
                } else {
                    of(view).dragging = false;
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |view, _, _, _| of(view).dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |view, _, _, _| of(view).dragging = false),
            )
            .on_scroll_wheel(cx.listener(move |view, event: &ScrollWheelEvent, _, cx| {
                let up = match event.delta {
                    ScrollDelta::Lines(lines) => lines.y,
                    ScrollDelta::Pixels(pixels) => f32::from(pixels.y),
                };
                if up != 0. {
                    let share = (share + WHEEL_STEP * up.signum()).clamp(0., 1.);
                    set(view, share, cx);
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_track_maps_the_pointer_to_a_share() {
        assert_eq!(share_at(100., 100., 220., 10.), 0.);
        assert_eq!(share_at(110., 100., 220., 10.), 0.);
        assert_eq!(share_at(210., 100., 220., 10.), 0.5);
        assert_eq!(share_at(310., 100., 220., 10.), 1.);
        assert_eq!(share_at(900., 100., 220., 10.), 1.);
        assert_eq!(share_at(5., 100., 0., 10.), 0.);
    }
}
