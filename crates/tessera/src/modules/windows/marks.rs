// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What a button of the dock shows besides its icon: the dots of its
//! windows or their number, the progress its application reports, its
//! overlay icon, and the mark of a window that asks for attention.

use std::time::Duration;

use gpui::{
    AnyElement, Div, FontWeight, Hsla, IntoElement, ParentElement, Pixels, SharedString, Styled,
    div, img, px,
};

use super::options::{Layout, Side};
use crate::icons::Icon;
use crate::motion::mix;
use crate::platform::ProgressState;
use crate::theme::Theme;

/// How long the segment of a progress without a known end takes to
/// travel along its button.
pub const TRAVEL: Duration = Duration::from_millis(1400);

/// How long one pulse of a button that asks for attention takes.
pub const PULSE: Duration = Duration::from_millis(1200);

/// How long the segment of a progress without a known end is, as a share
/// of the button.
const SEGMENT: f32 = 0.3;

/// How much of the tint of attention the background takes at the height
/// of a pulse.
const ATTENTION_TINT: f32 = 0.55;

/// How strongly the track under a progress shows.
const TRACK: f32 = 0.3;

/// How large an overlay icon is, as a part of the button.
const OVERLAY_SIZE: f32 = 0.42;

/// How large the mark of attention is, as a part of the indicator.
const ATTENTION_DOT: f32 = 1.6;

/// The part of the hue circle that the tertiary color lies from the
/// primary one, as Material 3 places it.
const TERTIARY_TURN: f32 = 1. / 6.;

/// The hue of the color of failure.
const ERROR_HUE: f32 = 0.;

/// The theme has no tertiary role yet: it is the primary color turned
/// along the hue circle, as light and as colorful.
pub fn tertiary(theme: &Theme) -> Hsla {
    let primary = theme.colors.primary;
    Hsla {
        h: (primary.h + TERTIARY_TURN).fract(),
        ..primary
    }
}

/// The theme has no role of failure yet: a red as light as the primary
/// color, and at least fairly colorful.
pub fn error(theme: &Theme) -> Hsla {
    let primary = theme.colors.primary;
    Hsla {
        h: ERROR_HUE,
        s: primary.s.max(0.6),
        ..primary
    }
}

/// The color of progress in `state`.
pub fn progress_color(theme: &Theme, state: ProgressState) -> Hsla {
    match state {
        ProgressState::Normal | ProgressState::Indeterminate => theme.colors.primary,
        ProgressState::Paused => tertiary(theme),
        ProgressState::Error => error(theme),
    }
}

/// The share of a cycle of `period` that `elapsed` is into it.
pub fn phase(elapsed: Duration, period: Duration) -> f32 {
    let period = period.as_secs_f32();
    (elapsed.as_secs_f32() % period) / period
}

/// Where the segment of a progress without a known end is at `phase` of
/// its travel: from where to where along the button, each from 0 to 1.
/// It comes in at the start and goes out at the end.
pub fn travelling(phase: f32) -> (f32, f32) {
    let head = phase * (1. + SEGMENT);
    ((head - SEGMENT).max(0.), head.min(1.))
}

/// How strong a pulse is at `phase`: from none to full and back.
pub fn pulse(phase: f32) -> f32 {
    (1. - (phase * std::f32::consts::TAU).cos()) / 2.
}

/// The background of a button that asks for attention, `amount` of the
/// way there, at the strength `pulse`.
pub fn attention_background(theme: &Theme, base: Hsla, amount: f32, pulse: f32) -> Hsla {
    mix(
        base,
        tertiary(theme),
        amount.clamp(0., 1.) * ATTENTION_TINT * pulse,
    )
}

/// The side of the button the marks of `layout` lie on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Bottom,
    Left,
    Right,
}

impl Edge {
    pub fn of(layout: Layout) -> Self {
        match layout {
            Layout::Row => Self::Bottom,
            Layout::Column { toward: Side::Left } => Self::Left,
            Layout::Column {
                toward: Side::Right,
            } => Self::Right,
        }
    }

    fn across(self) -> bool {
        self == Self::Bottom
    }
}

/// A strip `thickness` deep, `inset` away from `edge`, that covers the
/// part `from` to `to` of `length` along it, starting `margin` from the
/// start of the button; the bottom of a column is its start, as a
/// progress fills upward.
fn strip(
    edge: Edge,
    inset: Pixels,
    thickness: Pixels,
    margin: Pixels,
    length: Pixels,
    from: f32,
    to: f32,
) -> Div {
    let strip = div().absolute();
    let span = length * (to - from).max(0.);
    let start = margin + length * from;
    match edge {
        Edge::Bottom => strip.bottom(inset).h(thickness).left(start).w(span),
        Edge::Left => strip.left(inset).w(thickness).bottom(start).h(span),
        Edge::Right => strip.right(inset).w(thickness).bottom(start).h(span),
    }
}

/// How far from its edge the indicator lies.
fn indicator_inset(theme: &Theme) -> Pixels {
    theme.spacing.tiny / 2.
}

/// The progress of a button `size` long: a track along `edge`, just
/// inside the indicator, filled to `value`, or with a segment at `phase`
/// of its travel where the end is not known.
pub fn progress(
    theme: &Theme,
    edge: Edge,
    size: Pixels,
    state: ProgressState,
    value: f32,
    phase: f32,
) -> AnyElement {
    let thickness = theme.dock.indicator_height;
    let inset = indicator_inset(theme) + thickness + px(1.);
    let margin = theme.spacing.small;
    let length = size - margin * 2.;
    let color = progress_color(theme, state);
    let (from, to) = match state {
        ProgressState::Indeterminate => travelling(phase),
        _ => (0., value.clamp(0., 1.)),
    };
    let track = strip(edge, inset, thickness, margin, length, 0., 1.)
        .rounded(theme.shape.full)
        .bg(theme.colors.outline.opacity(TRACK));
    let fill = strip(edge, inset, thickness, margin, length, from, to)
        .rounded(theme.shape.full)
        .bg(color);
    div()
        .absolute()
        .size_full()
        .left_0()
        .top_0()
        .child(track)
        .child(fill)
        .into_any_element()
}

/// The dot under every application with a window, which grows into a
/// pill while one of its windows is in front, and beside it either a dot
/// for each of the further windows, `more` of them up to `dots`, or,
/// with `count`, the number of the windows where there are several.
/// `running` is 0 for an application without a window, which has none.
pub fn indicator(
    theme: &Theme,
    edge: Edge,
    active: f32,
    running: f32,
    more: f32,
    dots: u8,
    count: Option<usize>,
) -> AnyElement {
    let dot = theme.dock.indicator_height;
    let growth = theme.dock.indicator_active_width - dot;
    let color = mix(theme.colors.outline, theme.colors.primary, active);
    let long = dot + growth * active.max(0.);
    let further = (0..dots).map(|index| {
        let shown = (more - f32::from(index)).clamp(0., 1.);
        let further = div().rounded(theme.shape.full).opacity(shown).bg(color);
        match edge.across() {
            true => further.h(dot).w(dot * shown).ml(dot / 2. * shown),
            false => further.w(dot).h(dot * shown).mt(dot / 2. * shown),
        }
    });
    let number = count.filter(|&count| count > 1).map(|count| {
        let text = match count > 99 {
            true => "99+".to_owned(),
            false => count.to_string(),
        };
        div()
            .ml(dot / 2.)
            .text_size(dot * 3.)
            .line_height(dot * 3.)
            .font_weight(FontWeight::BOLD)
            .text_color(color)
            .child(SharedString::from(text))
    });
    let main = div().rounded(theme.shape.full).bg(color);
    let inset = indicator_inset(theme);
    let line = div()
        .absolute()
        .flex()
        .items_center()
        .justify_center()
        .opacity(running.clamp(0., 1.));
    let line = match edge {
        Edge::Bottom => line
            .left_0()
            .right_0()
            .bottom(inset)
            .child(main.h(dot).w(long)),
        Edge::Left => line.top_0().bottom_0().left(inset).flex_col(),
        Edge::Right => line.top_0().bottom_0().right(inset).flex_col(),
    };
    let line = match edge {
        Edge::Bottom => line,
        // In a column the pill stands along the side.
        Edge::Left | Edge::Right => {
            line.child(div().rounded(theme.shape.full).bg(color).w(dot).h(long))
        }
    };
    // A number reads across, whichever way the dock is laid out.
    match count {
        Some(_) => line.children(number),
        None => line.children(further),
    }
    .into_any_element()
}

/// The mark of a window that asks for attention, `amount` of the way
/// shown: a dot on the side away from the indicator in a row, and at the
/// top of the side of the marks in a column.
pub fn attention_dot(theme: &Theme, edge: Edge, amount: f32) -> AnyElement {
    let size = theme.dock.indicator_height * ATTENTION_DOT * amount.clamp(0., 1.2);
    let inset = indicator_inset(theme);
    let dot = div()
        .absolute()
        .size(size)
        .rounded(theme.shape.full)
        .bg(tertiary(theme))
        .opacity(amount.clamp(0., 1.));
    match edge {
        Edge::Bottom => dot.top(inset).left_0().right_0().mx_auto(),
        Edge::Left => dot.left(inset).top(inset * 2.),
        Edge::Right => dot.right(inset).top(inset * 2.),
    }
    .into_any_element()
}

/// The overlay icon an application gives its button, at its lower right,
/// where a badge would stand otherwise.
pub fn overlay(theme: &Theme, icon: Icon) -> Option<AnyElement> {
    let icon = icon?;
    let size = theme.dock.item_size * OVERLAY_SIZE;
    let inset = theme.spacing.tiny / 2.;
    Some(
        img(icon)
            .absolute()
            .right(inset)
            .bottom(inset)
            .size(size)
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_segment_comes_in_and_goes_out() {
        assert_eq!(travelling(0.), (0., 0.));
        let (from, to) = travelling(0.5);
        assert!(from > 0. && to < 1. && to - from > 0.29);
        assert_eq!(travelling(1.), (1., 1.));
    }

    #[test]
    fn a_pulse_rises_and_falls() {
        assert!(pulse(0.) < 0.001);
        assert!((pulse(0.5) - 1.).abs() < 0.001);
        assert!(pulse(0.99) < 0.01);
        let quarter = phase(Duration::from_millis(1500), PULSE);
        assert!((quarter - 0.25).abs() < 0.001);
    }

    #[test]
    fn the_derived_colors_keep_the_lightness_of_the_primary() {
        let theme = Theme::default();
        let primary = theme.colors.primary;
        assert_eq!(tertiary(&theme).l, primary.l);
        assert_ne!(tertiary(&theme).h, primary.h);
        assert_eq!(error(&theme).h, ERROR_HUE);
        assert!(error(&theme).s >= 0.6);
    }
}
