// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shell surfaces: borderless windows docked to a display edge.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Bounds, Context, Entity, IntoElement, ParentElement, Pixels,
    PlatformDisplay, Render, Styled, TitlebarOptions, Window, WindowBackgroundAppearance,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, div, point, px, size,
};

use crate::platform::{DisplayKey, EdgeReservation, NativeSurface, ScreenEdge};
use crate::theme::Theme;
use crate::ui::release_hover;

/// How often a surface looks whether an application fills its display.
const FILLED_CHECK: Duration = Duration::from_millis(300);

/// Describes a strip along one edge of a display.
#[derive(Clone, Copy, Debug)]
pub struct EdgeSurface {
    pub edge: ScreenEdge,
    pub thickness: Pixels,
}

impl EdgeSurface {
    /// Opens the strip on `display` and fills it with the view `build` returns.
    /// `build` is told which display the strip ended up on, when known.
    pub fn open<V: Render>(
        self,
        display: &Rc<dyn PlatformDisplay>,
        cx: &mut App,
        build: impl FnOnce(Option<DisplayKey>, &mut Context<V>) -> V,
    ) -> anyhow::Result<WindowHandle<EdgeSurfaceView<V>>> {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(self.bounds_on(display.bounds()))),
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
            let display = NativeSurface::of(window).ok().map(NativeSurface::display);
            let reservation = self
                .dock(window)
                .inspect_err(|error| {
                    report!("a surface is not docked to its screen edge: {error:#}");
                })
                .ok();
            let native = NativeSurface::of(window).ok();
            cx.new(|cx| {
                if let Some(native) = native {
                    Self::give_way(native, cx);
                }
                EdgeSurfaceView {
                    surface: self,
                    content: cx.new(|cx| build(display, cx)),
                    reservation,
                }
            })
        })
    }

    /// Has the surface `native` go below all windows while an application
    /// fills its display, as a game shown full screen does, and come back
    /// above them afterwards. Ends with the view `cx` makes.
    fn give_way<V: Render>(native: NativeSurface, cx: &mut Context<EdgeSurfaceView<V>>) {
        cx.spawn(async move |view, cx: &mut AsyncApp| {
            let mut below = false;
            while view.upgrade().is_some() {
                let filled = native.display_is_filled();
                // Said again each time while it is below: what raises the
                // surface meanwhile must not leave it over the game.
                if filled || below {
                    below = filled;
                    native.keep_above(!below);
                }
                cx.background_executor().timer(FILLED_CHECK).await;
            }
        })
        .detach();
    }

    fn bounds_on(self, display: Bounds<Pixels>) -> Bounds<Pixels> {
        let offset = match self.edge {
            ScreenEdge::Top => px(0.),
            ScreenEdge::Bottom => display.size.height - self.thickness,
        };
        Bounds {
            origin: display.origin + point(px(0.), offset),
            size: size(display.size.width, self.thickness),
        }
    }

    /// Makes the window a shell surface and reserves its screen edge.
    fn dock(self, window: &Window) -> anyhow::Result<EdgeReservation> {
        let surface = NativeSurface::of(window)?;
        surface.adopt_as_shell_surface()?;
        // Windows may leave the first window of a program hidden when it
        // was started that way.
        surface.reveal(false);
        surface.reserve_edge(self.edge, self.depth(window))
    }

    /// The thickness in physical pixels of `window`'s display.
    fn depth(self, window: &Window) -> i32 {
        (self.thickness / px(1.) * window.scale_factor()).round() as i32
    }
}

/// The root view of an edge surface: a transparent strip around its content.
pub struct EdgeSurfaceView<V> {
    surface: EdgeSurface,
    content: Entity<V>,
    /// Held so maximized windows keep clear of the surface while it exists.
    reservation: Option<EdgeReservation>,
}

impl<V: Render> EdgeSurfaceView<V> {
    pub fn content(&self) -> &Entity<V> {
        &self.content
    }

    /// Makes the strip `thickness` thick and replaces its content with the
    /// view `build` returns. The window and its reservation of the screen
    /// edge are kept.
    pub fn refill(
        &mut self,
        thickness: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
        build: impl FnOnce(Option<DisplayKey>, &mut Context<V>) -> V,
    ) {
        if thickness != self.surface.thickness || self.reservation.is_none() {
            self.surface.thickness = thickness;
            let docked = match &self.reservation {
                Some(reservation) => reservation
                    .occupy(self.surface.edge, self.surface.depth(window))
                    .map(|()| None),
                None => self.surface.dock(window).map(Some),
            };
            match docked {
                Ok(Some(reservation)) => self.reservation = Some(reservation),
                Ok(None) => {}
                Err(error) => {
                    report!("a surface is not docked to its screen edge: {error:#}");
                }
            }
        }
        let display = NativeSurface::of(window).ok().map(NativeSurface::display);
        self.content = cx.new(|cx| build(display, cx));
        cx.notify();
    }
}

impl<V: Render> Render for EdgeSurfaceView<V> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        div()
            .size_full()
            .font_family(cx.global::<Theme>().typography.family)
            .child(self.content.clone())
    }
}
