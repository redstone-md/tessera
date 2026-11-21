// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The top bar surface.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, div, size,
};

use crate::clock::Clock;
use crate::theme::Theme;

/// A full-width strip along the top edge of the primary display.
pub struct Bar {
    clock: Entity<Clock>,
}

impl Bar {
    /// Opens the bar on the primary display.
    pub fn open(cx: &mut App) -> gpui::Result<WindowHandle<Self>> {
        let theme = *cx.global::<Theme>();
        let display = cx
            .primary_display()
            .ok_or_else(|| anyhow::anyhow!("no primary display is available"))?;
        let bounds = Bounds {
            origin: display.bounds().origin,
            size: size(display.bounds().size.width, theme.bar_height),
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            display_id: Some(display.id()),
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };
        cx.open_window(options, |_, cx| {
            cx.new(|cx| Self {
                clock: cx.new(Clock::new),
            })
        })
    }
}

impl Render for Bar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .px(theme.bar_inset)
            .bg(theme.colors.surface_container)
            .child(self.clock.clone())
    }
}
