// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the editor of the rules is drawn of: cards, chips to choose with,
//! round and worded buttons, labelled rows and fields, in the colors and
//! shapes of the theme.

use gpui::{
    Context, Div, ElementId, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, SharedString, Stateful, Styled, Window, div, px,
};

use crate::field::TextField;
use crate::motion::{InteractionState, mix};
use crate::theme::Theme;
use crate::ui::{HOVER_LAYER, icon, one_line, switch};

/// The side of a round button, and the height of a chip and a button.
pub const BUTTON: Pixels = px(28.);

/// How wide the label before a control is.
pub const LABEL_WIDTH: Pixels = px(168.);

/// How see-through a rule is drawn while it is carried to another place.
pub const LIFTED: f32 = 0.35;

/// The colors of something to click on `ground`, and as the pointer is
/// over it.
fn hovered(theme: &Theme, ground: gpui::Hsla) -> gpui::Hsla {
    mix(ground, theme.colors.on_surface, HOVER_LAYER)
}

/// A card of the page: a title over what it holds.
pub fn card(theme: &Theme, title: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(theme.spacing.small)
        .p(theme.spacing.medium)
        .rounded(theme.shape.large)
        .bg(theme.colors.surface_container)
        .text_color(theme.colors.on_surface)
        .child(div().font_weight(FontWeight::MEDIUM).child(one_line(title)))
}

/// A smaller text that tells something of what is around it.
pub fn note(theme: &Theme, text: impl IntoElement) -> Div {
    div()
        .text_size(theme.typography.label_size * 0.92)
        .text_color(theme.colors.on_surface_variant)
        .child(text)
}

/// A row of things that wraps.
pub fn wrapping(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(theme.spacing.tiny)
}

/// A word to choose, standing out where it is `chosen`.
pub fn chip(
    theme: &Theme,
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    chosen: bool,
) -> Stateful<Div> {
    let (ground, color) = match chosen {
        true => (theme.colors.secondary_container, theme.colors.primary),
        false => (
            theme.colors.surface_container_highest,
            theme.colors.on_surface,
        ),
    };
    let hover = hovered(theme, ground);
    div()
        .id(id)
        .flex_none()
        .h(BUTTON)
        .px(theme.spacing.small + theme.spacing.tiny)
        .flex()
        .items_center()
        .rounded(theme.shape.full)
        .bg(ground)
        .hover(move |style| style.bg(hover))
        .cursor_pointer()
        .text_color(color)
        .child(one_line(text))
}

/// A round button with `glyph`, a character of the icon font.
pub fn glyph_button(theme: &Theme, id: impl Into<ElementId>, glyph: &'static str) -> Stateful<Div> {
    let ground = theme.colors.surface_container_highest;
    let hover = hovered(theme, ground);
    div()
        .id(id)
        .flex_none()
        .size(BUTTON)
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.shape.full)
        .bg(ground)
        .hover(move |style| style.bg(hover))
        .cursor_pointer()
        .text_color(theme.colors.on_surface_variant)
        .child(icon(theme, glyph).text_size(theme.typography.label_size))
}

/// A button with a glyph and a word; `prominent` for the one that does
/// what the card is for.
pub fn text_button(
    theme: &Theme,
    id: impl Into<ElementId>,
    glyph: &'static str,
    text: impl Into<SharedString>,
    prominent: bool,
) -> Stateful<Div> {
    let (ground, color) = match prominent {
        true => (theme.colors.primary, theme.colors.surface_container),
        false => (
            theme.colors.surface_container_highest,
            theme.colors.on_surface,
        ),
    };
    let hover = hovered(theme, ground);
    div()
        .id(id)
        .flex_none()
        .h(BUTTON)
        .px(theme.spacing.small + theme.spacing.tiny)
        .flex()
        .items_center()
        .gap(theme.spacing.tiny)
        .rounded(theme.shape.full)
        .bg(ground)
        .hover(move |style| style.bg(hover))
        .cursor_pointer()
        .text_color(color)
        .child(icon(theme, glyph).text_size(theme.typography.label_size))
        .child(one_line(text))
}

/// `item` after `label`.
pub fn labelled(theme: &Theme, label: impl Into<SharedString>, item: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(theme.spacing.small)
        .child(div().flex_none().w(LABEL_WIDTH).child(one_line(label)))
        .child(div().flex_1().min_w_0().child(item))
}

/// A text field on a ground of its own.
pub fn field_box(theme: &Theme, field: &Entity<TextField>) -> Div {
    div()
        .flex_1()
        .min_w(px(64.))
        .flex()
        .px(theme.spacing.small)
        .rounded(theme.shape.medium)
        .bg(theme.colors.surface_container_highest)
        .child(field.clone())
}

/// A switch that is on or off, still.
pub fn still_switch(theme: &Theme, id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    let state = InteractionState {
        hover: 0.,
        press: 0.,
    };
    switch(theme, id, if on { 1. } else { 0. }, state).cursor_pointer()
}

/// What follows the pointer while a rule is carried to another place.
pub struct Carried {
    pub from: usize,
    pub title: SharedString,
}

/// How a carried rule is drawn.
pub struct CarriedFace {
    pub title: SharedString,
}

impl Render for CarriedFace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        div()
            .h(BUTTON)
            .px(theme.spacing.small + theme.spacing.tiny)
            .flex()
            .items_center()
            .rounded(theme.shape.full)
            .bg(theme.colors.primary)
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.surface_container)
            .whitespace_nowrap()
            .child(self.title.clone())
    }
}
