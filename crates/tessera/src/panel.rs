// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The content of an edge surface: widgets arranged in three slots.

use gpui::{AnyView, App, Context, Div, IntoElement, ParentElement, Render, Styled, Window, div};

use crate::config::SurfaceConfig;
use crate::module::{WidgetContext, WidgetRegistry};
use crate::motion::Spring;
use crate::platform::ScreenEdge;
use crate::theme::Theme;
use crate::ui::centered_row;

/// Hosts the widgets a layout assigns to one surface.
pub struct Panel {
    start: Vec<AnyView>,
    center: Vec<AnyView>,
    end: Vec<AnyView>,
    /// The edge the content slides in from when the surface opens.
    edge: ScreenEdge,
    /// How far the content has come in: 0 is hidden behind the edge.
    entrance: Spring,
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
        let (start, center, end) = (
            build(&layout.start),
            build(&layout.center),
            build(&layout.end),
        );
        let mut entrance = Spring::new(cx.global::<Theme>().motion.spatial, 0.);
        entrance.animate_to(1.);
        Self {
            start,
            center,
            end,
            edge: layout.edge,
            entrance,
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let entrance = self.entrance.value(window);
        let hidden = window.viewport_size().height * (1. - entrance);
        let offset = match self.edge {
            ScreenEdge::Top => -hidden,
            ScreenEdge::Bottom => hidden,
        };
        div()
            .size_full()
            .relative()
            .top(offset)
            .opacity(entrance.clamp(0., 1.))
            .child(centered_row(
                &theme,
                Self::slot(&theme, &self.start),
                Self::slot(&theme, &self.center),
                Self::slot(&theme, &self.end),
            ))
    }
}
