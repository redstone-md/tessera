// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The top bar surface.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Pixels, Render, Styled,
    TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, div, px, size,
};

use crate::clock::Clock;
use crate::platform::{EdgeReservation, NativeSurface, ScreenEdge};
use crate::theme::Theme;

/// A full-width strip along the top edge of the primary display.
pub struct Bar {
    clock: Entity<Clock>,
    /// Held so maximized windows keep clear of the bar while it exists.
    _reservation: Option<EdgeReservation>,
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
            // GPUI insets the client area of windows without a title bar by the
            // system frame thickness. Asking for an opaque title bar disables
            // that; the caption itself is removed when the surface is adopted.
            titlebar: Some(TitlebarOptions::default()),
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
        cx.open_window(options, |window, cx| {
            let reservation = Self::dock(window, theme.bar_height)
                .inspect_err(|error| {
                    eprintln!("tessera: the bar is not docked to the screen edge: {error:#}");
                })
                .ok();
            cx.new(|cx| Self {
                clock: cx.new(Clock::new),
                _reservation: reservation,
            })
        })
    }

    /// Pins the bar above other windows and reserves the top screen edge.
    fn dock(window: &Window, height: Pixels) -> anyhow::Result<EdgeReservation> {
        let surface = NativeSurface::of(window)?;
        surface.adopt_as_shell_surface()?;
        let thickness = (height / px(1.) * window.scale_factor()).round() as i32;
        surface.reserve_edge(ScreenEdge::Top, thickness)
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
