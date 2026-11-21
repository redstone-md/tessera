// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Building blocks shared by shell surfaces.

use gpui::{Div, ParentElement, Styled, div};

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
