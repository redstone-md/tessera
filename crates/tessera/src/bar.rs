// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The top bar.

use gpui::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div,
};

use crate::clock::Clock;
use crate::theme::Theme;
use crate::widgets::{centered_row, pill};

/// Status content shown along the top edge of a display.
pub struct Bar {
    clock: Entity<Clock>,
}

impl Bar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            clock: cx.new(Clock::new),
        }
    }
}

impl Render for Bar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let quit = pill(&theme)
            .id("quit")
            .text_color(theme.colors.on_surface_variant)
            .hover(|style| style.bg(theme.colors.surface_container_highest))
            .on_click(|_, _, cx| cx.quit())
            .child("Quit Tessera");

        centered_row(
            &theme,
            div(),
            pill(&theme).child(self.clock.clone()),
            div().child(quit),
        )
    }
}
