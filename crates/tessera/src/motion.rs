// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Spring-driven motion: values that move toward a target and can be
//! redirected at any moment without a jump.

use std::time::{Duration, Instant};

use gpui::{
    Context, Div, Hsla, InteractiveElement, MouseButton, Rgba, Stateful,
    StatefulInteractiveElement, Window,
};

use crate::theme::Motion;

/// The longest step simulated at once; longer gaps are split up.
const MAX_STEP: Duration = Duration::from_millis(4);

/// The longest gap between frames that is simulated; after a longer pause
/// the spring continues from where it was instead of leaping ahead.
const MAX_GAP: Duration = Duration::from_millis(64);

/// How a spring moves.
#[derive(Clone, Copy, Debug)]
pub struct SpringSpec {
    /// Higher is faster.
    pub stiffness: f32,
    /// 1 settles without overshoot; lower values bounce.
    pub damping_ratio: f32,
}

/// A value that follows its target like a mass on a damped spring.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    spec: SpringSpec,
    value: f32,
    velocity: f32,
    target: f32,
    /// When the value was last advanced; `None` while at rest.
    advanced_at: Option<Instant>,
}

impl Spring {
    /// A spring at rest at `value`.
    pub fn new(spec: SpringSpec, value: f32) -> Self {
        Self {
            spec,
            value,
            velocity: 0.,
            target: value,
            advanced_at: None,
        }
    }

    /// Starts moving toward `target`, keeping the current velocity.
    pub fn animate_to(&mut self, target: f32) {
        self.target = target;
    }

    /// Pushes the value, as a change of `velocity` in units per second.
    pub fn kick(&mut self, velocity: f32) {
        self.velocity += velocity;
    }

    /// Whether the value has reached its target and stopped.
    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < 0.001 && self.velocity.abs() < 0.01
    }

    /// The value now. While it is moving, asks `window` for another frame,
    /// which repaints the view being rendered.
    pub fn value(&mut self, window: &Window) -> f32 {
        self.advance_to(Instant::now());
        if !self.is_settled() {
            window.request_animation_frame();
        }
        self.value
    }

    fn advance_to(&mut self, now: Instant) {
        if self.is_settled() {
            self.value = self.target;
            self.velocity = 0.;
            self.advanced_at = None;
            return;
        }
        let elapsed = match self.advanced_at.replace(now) {
            Some(before) => now.saturating_duration_since(before).min(MAX_GAP),
            None => Duration::ZERO,
        };
        let damping = 2. * self.spec.damping_ratio * self.spec.stiffness.sqrt();
        let mut remaining = elapsed;
        while !remaining.is_zero() {
            let step = remaining.min(MAX_STEP);
            remaining -= step;
            let seconds = step.as_secs_f32();
            let force = -self.spec.stiffness * (self.value - self.target) - damping * self.velocity;
            self.velocity += force * seconds;
            self.value += self.velocity * seconds;
        }
    }
}

/// How far the pointer's hover and press have progressed, each 0 to 1.
#[derive(Clone, Copy, Debug)]
pub struct InteractionState {
    pub hover: f32,
    /// May briefly leave the range: a press bounces.
    pub press: f32,
}

/// Animated pointer feedback for one control.
#[derive(Clone, Copy, Debug)]
pub struct Interaction {
    hover: Spring,
    press: Spring,
}

impl Interaction {
    /// The speed a press starts with, so that even a click shorter than a
    /// frame is seen.
    const PRESS_KICK: f32 = 12.;

    pub fn new(motion: &Motion) -> Self {
        Self {
            hover: Spring::new(motion.effects, 0.),
            press: Spring::new(motion.spatial_fast, 0.),
        }
    }

    fn set_hovered(&mut self, hovered: bool) {
        self.hover.animate_to(if hovered { 1. } else { 0. });
    }

    fn set_pressed(&mut self, pressed: bool) {
        if pressed {
            self.press.kick(Self::PRESS_KICK);
        }
        self.press.animate_to(if pressed { 1. } else { 0. });
    }

    /// The state now; keeps the view being rendered repainting while it moves.
    pub fn state(&mut self, window: &Window) -> InteractionState {
        InteractionState {
            hover: self.hover.value(window).clamp(0., 1.),
            press: self.press.value(window),
        }
    }

    /// Makes `element` drive the interaction that `find` locates in the
    /// view. `find` returns `None` once the control is gone.
    pub fn track<V: 'static>(
        element: Stateful<Div>,
        cx: &mut Context<V>,
        find: impl for<'a> Fn(&'a mut V) -> Option<&'a mut Self> + Clone + 'static,
    ) -> Stateful<Div> {
        let update = move |apply: fn(&mut Self, bool), value: bool| {
            let find = find.clone();
            move |view: &mut V, cx: &mut Context<V>| {
                if let Some(interaction) = find(view) {
                    apply(interaction, value);
                    cx.notify();
                }
            }
        };
        let (press, release, release_out) = (
            update(Self::set_pressed, true),
            update(Self::set_pressed, false),
            update(Self::set_pressed, false),
        );
        let (enter, leave) = (
            update(Self::set_hovered, true),
            update(Self::set_hovered, false),
        );
        element
            .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                if *hovered {
                    enter(view, cx)
                } else {
                    leave(view, cx)
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, _, cx| press(view, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |view, _, _, cx| release(view, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |view, _, _, cx| release_out(view, cx)),
            )
    }
}

/// The color `amount` of the way from `from` to `to`, with `amount` clamped
/// to 0..=1.
pub fn mix(from: Hsla, to: Hsla, amount: f32) -> Hsla {
    let (from, to, amount) = (Rgba::from(from), Rgba::from(to), amount.clamp(0., 1.));
    let channel = |from: f32, to: f32| from + (to - from) * amount;
    Rgba {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
        a: channel(from.a, to.a),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNCY: SpringSpec = SpringSpec {
        stiffness: 380.,
        damping_ratio: 0.6,
    };
    const SMOOTH: SpringSpec = SpringSpec {
        stiffness: 380.,
        damping_ratio: 1.,
    };

    /// Runs `spring` toward 1 at 60 frames per second for two seconds and
    /// returns the largest value it reached.
    fn run(spring: &mut Spring) -> f32 {
        let start = Instant::now();
        spring.animate_to(1.);
        (0..=120)
            .map(|frame| {
                spring.advance_to(start + Duration::from_millis(frame * 1000 / 60));
                spring.value
            })
            .fold(f32::MIN, f32::max)
    }

    #[test]
    fn a_spring_settles_exactly_on_its_target() {
        for spec in [BOUNCY, SMOOTH] {
            let mut spring = Spring::new(spec, 0.);
            run(&mut spring);
            assert!(spring.is_settled());
            assert_eq!(spring.value, 1.);
        }
    }

    #[test]
    fn only_an_underdamped_spring_overshoots() {
        assert!(run(&mut Spring::new(BOUNCY, 0.)) > 1.02);
        assert!(run(&mut Spring::new(SMOOTH, 0.)) <= 1.001);
    }

    #[test]
    fn a_spring_at_rest_does_not_move() {
        let mut spring = Spring::new(BOUNCY, 0.5);
        spring.advance_to(Instant::now() + Duration::from_secs(1));
        assert_eq!(spring.value, 0.5);
        assert!(spring.is_settled());
    }

    #[test]
    fn mixing_interpolates_and_clamps() {
        let (black, white) = (gpui::black(), gpui::white());
        assert_eq!(Rgba::from(mix(black, white, 0.)), Rgba::from(black));
        assert_eq!(Rgba::from(mix(black, white, 2.)), Rgba::from(white));
        let middle = Rgba::from(mix(black, white, 0.5));
        assert!((middle.r - 0.5).abs() < 0.01, "red is {}", middle.r);
    }

    #[test]
    fn a_long_pause_does_not_make_the_value_leap() {
        let mut spring = Spring::new(SMOOTH, 0.);
        let start = Instant::now();
        spring.animate_to(1.);
        spring.advance_to(start);
        spring.advance_to(start + Duration::from_secs(10));
        assert!(spring.value < 0.6, "moved to {}", spring.value);
    }
}
