// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the lists in popups are made of: rows, the titles over groups of
//! them, and a switch.

use gpui::{
    Div, ElementId, FontWeight, InteractiveElement, ParentElement, Pixels, SharedString, Stateful,
    Styled, div, px,
};

use super::{icon, one_line, state_layer};
use crate::motion::{InteractionState, mix};
use crate::theme::Theme;

/// How high a row of a list is.
pub const ROW_HEIGHT: Pixels = px(56.);

/// The least height of a list that takes the room the rest of a popup
/// leaves: its title and two rows and a half, which shows there are more.
pub const LIST_MIN_HEIGHT: Pixels = px(188.);

/// The size of the note under the title of a row.
const NOTE_SIZE: Pixels = px(11.);

/// A switch, and the dot that crosses it.
const SWITCH_WIDTH: f32 = 44.;
const SWITCH_HEIGHT: f32 = 24.;
const SWITCH_DOT: f32 = 16.;

/// A row of a list: an icon, then a title over a note. `lit` marks the row
/// of what is in use. What is added to it comes after the text.
pub fn list_row(
    theme: &Theme,
    glyph: &'static str,
    lit: bool,
    title: impl Into<SharedString>,
    note: impl Into<SharedString>,
) -> Div {
    let (ground, mark) = if lit {
        (theme.colors.secondary_container, theme.colors.primary)
    } else {
        (
            theme.colors.surface_container_highest,
            theme.colors.on_surface_variant,
        )
    };
    div()
        .h(ROW_HEIGHT)
        .flex_none()
        .px(theme.spacing.medium)
        .flex()
        .items_center()
        .gap(theme.spacing.medium)
        .rounded(theme.shape.medium)
        .bg(ground)
        .child(icon(theme, glyph).flex_none().text_color(mark))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .min_w_0()
                        .font_weight(FontWeight::MEDIUM)
                        .child(one_line(title)),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_size(NOTE_SIZE)
                        .text_color(theme.colors.on_surface_variant)
                        .child(one_line(note)),
                ),
        )
}

/// The title over a group of rows.
pub fn list_title(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px(theme.spacing.small)
        .pt(theme.spacing.tiny)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.on_surface_variant)
        .child(one_line(text))
}

/// A switch whose dot is `travel` of its way to the side that means on,
/// under a pointer in `state`.
pub fn switch(
    theme: &Theme,
    id: impl Into<ElementId>,
    travel: f32,
    state: InteractionState,
) -> Stateful<Div> {
    let shown = travel.clamp(0., 1.);
    // The track of a switch that is off stands out from the surface the
    // switch lies on.
    let off = theme.colors.surface_container_highest;
    let track = mix(off, theme.colors.primary, shown);
    let dot = mix(theme.colors.outline, theme.colors.surface_container, shown);
    let inset = (SWITCH_HEIGHT - SWITCH_DOT) / 2.;
    let way = SWITCH_WIDTH - SWITCH_DOT - inset * 2.;
    div()
        .id(id)
        .flex_none()
        .relative()
        .w(px(SWITCH_WIDTH))
        .h(px(SWITCH_HEIGHT))
        .rounded(theme.shape.full)
        .bg(state_layer(theme, track, state))
        .child(
            div()
                .absolute()
                .top(px(inset))
                .left(px(inset + way * travel))
                .size(px(SWITCH_DOT))
                .rounded(theme.shape.full)
                .bg(dot),
        )
}
