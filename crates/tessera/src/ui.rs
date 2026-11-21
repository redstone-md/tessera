// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Building blocks shared by shell surfaces.

mod slider;

use std::f32::consts::TAU;

use std::panic::Location;

use gpui::{
    App, AvailableSpace, Bounds, Canvas, Div, Element, ElementId, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, ParentElement, PathBuilder, Pixels, ShapedLine,
    SharedString, Size, Style, Styled, TextRun, Window, canvas, div, font, point, px,
};

use crate::motion::{InteractionState, mix};
use crate::platform::NativeSurface;
use crate::theme::Theme;

pub use slider::Slider;

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
        .bg(theme.on_bar(theme.colors.surface_container))
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

/// What stands for the part of a text that was cut off.
const CUT: &str = "\u{2026}";

/// The `beginning` of a text with the mark that the rest was cut off, which
/// stands right after the last thing that is not a space.
fn cut_after(beginning: &str) -> String {
    format!("{}{CUT}", beginning.trim_end())
}

/// How far over its room a text may be before it is cut; shaping a text
/// twice does not give the same width to the last fraction.
const CUT_SLACK: f32 = 0.5;

/// How wide `text` is on one line in the font `family` at `size`.
pub fn text_width(
    text: &SharedString,
    family: &'static str,
    size: Pixels,
    window: &Window,
) -> Pixels {
    let run = TextRun {
        len: text.len(),
        font: font(family),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text.clone(), size, &[run], None);
    line.width
}

/// `text` on one line in the text style around it, as wide as the text
/// and no wider than the room it gets, its end cut off and marked where
/// it does not fit. A line break in it counts as a space.
///
/// GPUI can cut a text too, but measures a text that may not wrap only
/// once, without a limit, and so never cuts it, and loses count of the
/// bytes of a text that may wrap when it cuts it a second time. The
/// cut is found here by shaping the text with its mark, the way it is
/// drawn, instead of adding up the widths of single characters.
pub fn one_line(text: impl Into<SharedString>) -> OneLine {
    let text = text.into();
    let text = if text.contains(['\n', '\r']) {
        text.replace(['\n', '\r'], " ").into()
    } else {
        text
    };
    OneLine { text }
}

/// The element [`one_line`] makes.
pub struct OneLine {
    text: SharedString,
}

impl OneLine {
    /// The text shaped in the text style around it, cut to `room` if one
    /// is given and the text is `whole` wide and does not fit.
    fn shaped(&self, room: Option<(Pixels, Pixels)>, window: &Window) -> ShapedLine {
        let style = window.text_style();
        let size = style.font_size.to_pixels(window.rem_size());
        let shape = |text: SharedString| {
            let run = style.to_run(text.len());
            window.text_system().shape_line(text, size, &[run], None)
        };
        let room = match room {
            Some((room, whole)) if whole > room + px(CUT_SLACK) => room,
            _ => return shape(self.text.clone()),
        };
        // The longest beginning that fits with the mark after it, found by
        // shaping, which is what tells how wide a text is drawn.
        let ends: Vec<usize> = self.text.char_indices().map(|(end, _)| end).collect();
        let cut = |kept: usize| shape(cut_after(&self.text[..ends[kept]]).into());
        let (mut fits, mut over) = (0, ends.len());
        while over - fits > 1 {
            let kept = fits + (over - fits) / 2;
            if cut(kept).width <= room + px(CUT_SLACK) {
                fits = kept;
            } else {
                over = kept;
            }
        }
        cut(fits)
    }
}

impl IntoElement for OneLine {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for OneLine {
    /// How wide the whole text is.
    type RequestLayoutState = Pixels;
    type PrepaintState = ShapedLine;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, Pixels) {
        let whole = self.shaped(None, window).width.ceil();
        let height = window.line_height();
        let layout = window.request_measured_layout(Style::default(), move |known, room, _, _| {
            let room = known.width.or(match room.width {
                AvailableSpace::Definite(room) => Some(room),
                _ => None,
            });
            Size {
                width: room.map_or(whole, |room| room.min(whole)),
                height,
            }
        });
        (layout, whole)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        whole: &mut Pixels,
        window: &mut Window,
        _: &mut App,
    ) -> ShapedLine {
        self.shaped(Some((bounds.size.width, *whole)), window)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Pixels,
        line: &mut ShapedLine,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Err(error) = line.paint(bounds.origin, window.line_height(), window, cx) {
            report!("a line of text was not drawn: {error:#}");
        }
    }
}
