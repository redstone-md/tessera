// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Fixed content to place on a surface, typically to open a popup.
//! Widget: `label`.

use gpui::{
    AppContext, Context, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled,
    Window,
};

use crate::module::{Module, WidgetRegistry};
use crate::theme::Theme;
use crate::ui::{icon, pill};

pub struct LabelModule;

impl Module for LabelModule {
    fn name(&self) -> &'static str {
        "label"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("label", |context, cx| {
            let label = Label {
                icon: context.text("icon").map(SharedString::new),
                text: context.text("text").map(SharedString::new),
            };
            cx.new(|_| label).into()
        })
    }
}

/// An icon, a text, or both.
struct Label {
    /// A character of the theme's icon font.
    icon: Option<SharedString>,
    text: Option<SharedString>,
}

impl Render for Label {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        pill(&theme)
            .font_weight(FontWeight::MEDIUM)
            .children(self.icon.clone().map(|glyph| icon(&theme, glyph)))
            .children(self.text.clone())
    }
}
