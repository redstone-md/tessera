// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Building blocks shared by shell surfaces.

use gpui::{Div, Hsla, ParentElement, Styled, div};

use crate::motion::{InteractionState, mix};
use crate::theme::Theme;

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
