// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The tiles of the quick settings: a button with an icon and a name over
//! a note, filled with the primary color while what it stands for is on,
//! and the narrow button beside one that unfolds what it has more to say.

use gpui::{
    Div, ElementId, FontWeight, InteractiveElement, ParentElement, Pixels, SharedString, Stateful,
    Styled, div, px,
};

use super::{icon, one_line, state_layer};
use crate::motion::{InteractionState, mix};
use crate::theme::Theme;

/// How high a tile is.
pub const TILE_HEIGHT: Pixels = px(56.);

/// How wide the button beside a tile is that unfolds its details.
pub const TILE_MORE_WIDTH: Pixels = px(28.);

/// The size of the note under the name of a tile.
const NOTE_SIZE: Pixels = px(11.);

/// How much of a tile shows while it cannot be used.
const UNUSABLE_OPACITY: f32 = 0.45;

/// A chevron pointing down, and one pointing up.
const ICON_UNFOLD: &str = "\u{e70d}";
const ICON_FOLD: &str = "\u{e70e}";

/// What a tile says.
#[derive(Clone, Debug, PartialEq)]
pub struct TileFace {
    /// A character of the theme's icon font.
    pub glyph: &'static str,
    pub title: SharedString,
    /// What stands under the title: the state, or why it cannot be used.
    pub note: SharedString,
    /// Whether what the tile stands for is on.
    pub lit: bool,
    /// Whether a click on the tile does something.
    pub usable: bool,
}

/// A tile showing `face`, `lit` (0 to 1) of the way to the look of
/// something that is on, under a pointer in `state`. One that is not
/// usable is pale; whoever builds it gives it no click.
pub fn tile(
    theme: &Theme,
    id: impl Into<ElementId>,
    face: &TileFace,
    lit: f32,
    state: InteractionState,
) -> Stateful<Div> {
    let lit = lit.clamp(0., 1.);
    let colors = theme.colors;
    let ground = mix(colors.surface_container_highest, colors.primary, lit);
    let ink = mix(colors.on_surface, colors.surface_container, lit);
    let ground = match face.usable {
        true => state_layer(theme, ground, state),
        false => ground,
    };
    let note_ink = mix(colors.on_surface_variant, ink, lit);
    div()
        .id(id)
        .h(TILE_HEIGHT)
        .w_full()
        .min_w_0()
        .px(theme.spacing.medium)
        .flex()
        .items_center()
        .gap(theme.spacing.small)
        .rounded(theme.shape.medium)
        .bg(ground)
        .text_color(ink)
        .opacity(if face.usable { 1. } else { UNUSABLE_OPACITY })
        .child(icon(theme, face.glyph).flex_none())
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
                        .child(one_line(face.title.clone())),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_size(NOTE_SIZE)
                        .text_color(note_ink)
                        .child(one_line(face.note.clone())),
                ),
        )
}

/// The narrow button beside a tile that unfolds its details, or folds
/// them while they are `unfolded`.
pub fn tile_more(
    theme: &Theme,
    id: impl Into<ElementId>,
    unfolded: bool,
    state: InteractionState,
) -> Stateful<Div> {
    let colors = theme.colors;
    let ground = match unfolded {
        true => colors.secondary_container,
        false => colors.surface_container_highest,
    };
    let glyph = if unfolded { ICON_FOLD } else { ICON_UNFOLD };
    div()
        .id(id)
        .flex_none()
        .h(TILE_HEIGHT)
        .w(TILE_MORE_WIDTH)
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.shape.medium)
        .bg(state_layer(theme, ground, state))
        .text_color(colors.on_surface_variant)
        .child(icon(theme, glyph).text_size(NOTE_SIZE))
}
