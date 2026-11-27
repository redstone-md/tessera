// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Room and lines between the widgets of a bar, on a bar at any edge.
//! Widgets: `spacer`, empty room `size` logical pixels long, or all the
//! room its slot has left with `flexible = true`; `separator`, a thin line
//! across the bar.

use gpui::{AppContext, Context, IntoElement, Pixels, Render, Styled, Window, div, px};

use crate::module::{Module, WidgetRegistry};
use crate::surface::{Axis, axis_of};
use crate::theme::Theme;

/// How long a spacer is where the layout does not say.
const SPACER_SIZE: f32 = 12.;

/// The most a spacer of a fixed size can be.
const LONGEST_SPACER: f32 = 2000.;

/// How thick the line of a separator is, and how much of the bar's
/// thickness it crosses.
const LINE: Pixels = px(1.);
const LINE_SHARE: f32 = 0.5;
const LONGEST_LINE: Pixels = px(40.);

/// How strong the line is, of the outline color of the theme.
const LINE_STRENGTH: f32 = 0.6;

pub struct SpacerModule;

impl Module for SpacerModule {
    fn name(&self) -> &'static str {
        "spacer"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("spacer", |context, cx| {
            let flexible = context.flag("flexible").unwrap_or(false);
            let size = context
                .number("size")
                .map_or(SPACER_SIZE, |size| size as f32);
            let spacer = Spacer {
                size: (!flexible).then(|| px(spacer_size(size))),
            };
            cx.new(|_| spacer).into()
        })?;
        widgets.register("separator", |_, cx| cx.new(|_| Separator).into())
    }
}

/// A size of a spacer within what it can be; a size that is no number is
/// the default.
fn spacer_size(size: f32) -> f32 {
    if size.is_finite() {
        size.clamp(0., LONGEST_SPACER)
    } else {
        SPACER_SIZE
    }
}

/// Empty room along a bar: as long as `size`, or all the room left where
/// it has none.
struct Spacer {
    size: Option<Pixels>,
}

impl Render for Spacer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let room = div();
        match (self.size, axis_of(window, cx)) {
            (Some(size), Axis::Horizontal) => room.flex_none().w(size).h_full(),
            (Some(size), Axis::Vertical) => room.flex_none().h(size).w_full(),
            // The panel lets the slot of a flexible spacer grow.
            (None, Axis::Horizontal) => room.flex_1().h_full(),
            (None, Axis::Vertical) => room.flex_1().w_full(),
        }
    }
}

/// A thin line across a bar, between the widgets on either side of it.
struct Separator;

impl Render for Separator {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let color = theme.colors.outline.opacity(LINE_STRENGTH);
        let viewport = window.viewport_size();
        let line = div().flex_none().rounded(theme.shape.full).bg(color);
        // Outside a bar, as in a popup, the line stays short.
        let across = |depth: Pixels| (depth * LINE_SHARE).min(LONGEST_LINE);
        match axis_of(window, cx) {
            Axis::Horizontal => line.w(LINE).h(across(viewport.height)),
            Axis::Vertical => line.h(LINE).w(across(viewport.width)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spacer_is_as_long_as_it_can_be() {
        assert_eq!(spacer_size(20.), 20.);
        assert_eq!(spacer_size(-5.), 0.);
        assert_eq!(spacer_size(1e9), LONGEST_SPACER);
        assert_eq!(spacer_size(f32::NAN), SPACER_SIZE);
    }
}
