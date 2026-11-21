// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Building blocks shared by shell surfaces.

use std::f32::consts::TAU;

use gpui::{
    Bounds, Canvas, Div, Hsla, IntoElement, ParentElement, PathBuilder, Pixels, SharedString,
    Styled, Window, canvas, div, point, px,
};

use crate::motion::{InteractionState, mix};
use crate::platform::NativeSurface;
use crate::theme::Theme;

/// Ends the hover of what the pointer was on when it left `window`. GPUI
/// learns that the pointer left but goes on taking it to be where it was
/// last seen, so the root view of every window calls this as it renders,
/// which it does when the pointer leaves.
pub fn release_hover(window: &Window) {
    let position = window.mouse_position();
    let inside = position.x >= px(0.) && position.y >= px(0.);
    if !window.is_window_hovered()
        && inside
        && let Ok(native) = NativeSurface::of(window)
    {
        native.forget_pointer();
    }
}

/// A floating, fully rounded container that holds a group of bar content.
pub fn pill(theme: &Theme) -> Div {
    div()
        .h(theme.bar.pill_height)
        .px(theme.spacing.medium)
        .flex()
        .items_center()
        .gap(theme.spacing.small)
        .rounded(theme.shape.full)
        .bg(theme.colors.surface_container)
        .text_size(theme.typography.label_size)
        .text_color(theme.colors.on_surface)
}

/// An icon: `glyph` is a character of the theme's icon font.
pub fn icon(theme: &Theme, glyph: impl Into<SharedString>) -> Div {
    div()
        .font_family(theme.typography.icon_family)
        .text_size(theme.typography.icon_size)
        .child(glyph.into())
}

/// A line that is wavy up to `level`, from 0 to 1, and straight after it.
/// The wave travels as the level changes.
pub fn wave_meter(theme: &Theme, level: f32) -> impl IntoElement + use<> {
    wave_line(theme, level).w(theme.meter.width).flex_none()
}

/// The line of [`wave_meter`], as wide as the place it is put in.
pub fn wave_bar(theme: &Theme, level: f32) -> impl IntoElement + use<> {
    wave_line(theme, level).w_full()
}

fn wave_line(theme: &Theme, level: f32) -> Canvas<()> {
    let theme = *theme;
    let metrics = theme.meter;
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| paint_wave(bounds, level.clamp(0., 1.), &theme, window),
    )
    .h(metrics.amplitude * 2. + metrics.stroke)
}

fn paint_wave(bounds: Bounds<Pixels>, level: f32, theme: &Theme, window: &mut Window) {
    /// The distance between the points the wave is drawn through.
    const STEP: Pixels = px(1.);

    let metrics = theme.meter;
    let middle = bounds.origin.y + bounds.size.height / 2.;
    let left = bounds.origin.x + metrics.stroke / 2.;
    let right = bounds.origin.x + bounds.size.width - metrics.stroke / 2.;
    let split = left + (right - left) * level;

    if split - left >= STEP {
        // The phase follows the level, so a changing level moves the wave.
        let phase = level * TAU * 3.;
        let mut wave = PathBuilder::stroke(metrics.stroke);
        let mut x = left;
        let mut started = false;
        while x <= split {
            let turn = (x - left) / metrics.wavelength;
            let y = middle + metrics.amplitude * (turn * TAU - phase).sin();
            if started {
                wave.line_to(point(x, y));
            } else {
                wave.move_to(point(x, y));
                started = true;
            }
            x += STEP;
        }
        if let Ok(path) = wave.build() {
            window.paint_path(path, theme.colors.primary);
        }
    }
    let rest = if split - left >= STEP {
        split + metrics.gap
    } else {
        left
    };
    if right - rest >= STEP {
        let mut line = PathBuilder::stroke(metrics.stroke);
        line.move_to(point(rest, middle));
        line.line_to(point(right, middle));
        if let Ok(path) = line.build() {
            window.paint_path(path, theme.colors.secondary_container);
        }
    }
}

/// How much of the content color covers a hovered control.
pub const HOVER_LAYER: f32 = 0.08;

/// How much more of it covers a pressed one.
pub const PRESS_LAYER: f32 = 0.10;

/// `base` as it looks under the pointer: tinted toward the content color.
pub fn state_layer(theme: &Theme, base: Hsla, state: InteractionState) -> Hsla {
    let amount = HOVER_LAYER * state.hover + PRESS_LAYER * state.press;
    mix(base, theme.colors.on_surface, amount)
}

/// A row that keeps `center` in the middle regardless of the side widths.
pub fn centered_row(theme: &Theme, start: Div, center: Div, end: Div) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .px(theme.spacing.small)
        .child(div().flex_1().flex().justify_start().child(start))
        .child(center)
        .child(div().flex_1().flex().justify_end().child(end))
}
