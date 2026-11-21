// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The content of an edge surface: widgets arranged in three slots.

use gpui::{AnyView, App, Context, Div, IntoElement, ParentElement, Render, Styled, Window, div};

use crate::config::SurfaceConfig;
use crate::module::{WidgetContext, WidgetRegistry};
use crate::theme::Theme;
use crate::ui::centered_row;

/// Hosts the widgets a layout assigns to one surface.
pub struct Panel {
    start: Vec<AnyView>,
    center: Vec<AnyView>,
    end: Vec<AnyView>,
}

impl Panel {
    /// Instantiates the widgets `layout` names. Unknown names are reported
    /// and left out.
    pub fn new(layout: &SurfaceConfig, context: WidgetContext, cx: &mut App) -> Self {
        let mut build = |ids: &[String]| -> Vec<AnyView> {
            ids.iter()
                .filter_map(|id| {
                    let widget = WidgetRegistry::build(id, &context, cx);
                    if widget.is_none() {
                        eprintln!("tessera: no module provides the widget `{id}`");
                    }
                    widget
                })
                .collect()
        };
        Self {
            start: build(&layout.start),
            center: build(&layout.center),
            end: build(&layout.end),
        }
    }

    fn slot(theme: &Theme, widgets: &[AnyView]) -> Div {
        div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .children(widgets.iter().cloned())
    }
}

impl Render for Panel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        centered_row(
            &theme,
            Self::slot(&theme, &self.start),
            Self::slot(&theme, &self.center),
            Self::slot(&theme, &self.end),
        )
    }
}
