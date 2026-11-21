// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Control over the running shell. Widget: `quit`.

use gpui::{
    AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window,
};

use crate::module::{Module, WidgetRegistry};
use crate::theme::Theme;
use crate::ui::pill;

pub struct SessionModule;

impl Module for SessionModule {
    fn name(&self) -> &'static str {
        "session"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("quit", |_, cx| cx.new(|_| QuitButton).into())
    }
}

/// Closes the shell, which brings the Windows taskbar back.
struct QuitButton;

impl Render for QuitButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        pill(&theme)
            .id("quit")
            .text_color(theme.colors.on_surface_variant)
            .hover(|style| style.bg(theme.colors.surface_container_highest))
            .on_click(|_, _, cx| cx.quit())
            .child("Quit Tessera")
    }
}
