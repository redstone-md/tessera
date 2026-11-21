// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Control over the running shell. Widget: `quit`.

use gpui::{
    AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::Interaction;
use crate::theme::Theme;
use crate::ui::{pill, state_layer};

pub struct SessionModule;

impl Module for SessionModule {
    fn name(&self) -> &'static str {
        "session"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("quit", |_, cx| {
            let interaction = Interaction::new(&cx.global::<Theme>().motion);
            cx.new(|_| QuitButton { interaction }).into()
        })
    }
}

/// Closes the shell, which brings the Windows taskbar back.
struct QuitButton {
    interaction: Interaction,
}

impl Render for QuitButton {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let state = self.interaction.state(window);
        let button = pill(&theme)
            .id("quit")
            .bg(state_layer(&theme, theme.colors.surface_container, state))
            .text_color(theme.colors.on_surface_variant)
            .on_click(|_, _, cx| cx.quit())
            .child("Quit Tessera");
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}
