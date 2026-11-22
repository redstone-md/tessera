// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shell surfaces: borderless windows docked to a display edge.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, AsyncApp, Bounds, Context, Entity, IntoElement, ParentElement, Pixels,
    PlatformDisplay, Render, Styled, TitlebarOptions, Window, WindowBackgroundAppearance,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, div, point, px, size,
};

use crate::config::Hiding;
use crate::motion::Spring;
use crate::platform::{
    AppWindow, DisplayKey, EdgeReservation, NativeSurface, ScreenEdge, SmallBirth,
    pointer_on_own_window,
};
use crate::popup::Popups;
use crate::theme::Theme;
use crate::ui::release_hover;

/// How often a surface looks whether an application fills its display.
const FILLED_CHECK: Duration = Duration::from_millis(300);

/// How often a surface that hides looks where the pointer is.
const POINTER_CHECK: Duration = Duration::from_millis(100);

/// How often a surface that does not hide looks whether it is to.
const HIDE_CHECK: Duration = Duration::from_secs(1);

/// How long the pointer is elsewhere before a surface that hides goes.
const HIDE_AFTER: Duration = Duration::from_millis(700);

/// How long a surface takes to slide away before its window is hidden.
const SLIDE: Duration = Duration::from_millis(450);

/// How deep, in physical pixels, the edge of the display is that calls a
/// hidden surface back.
const CALLING_EDGE: i32 = 2;

/// Describes a strip along one edge of a display.
#[derive(Clone, Copy, Debug)]
pub struct EdgeSurface {
    pub edge: ScreenEdge,
    pub thickness: Pixels,
    /// When the strip goes away; one that does reserves no screen space.
    pub hide: Hiding,
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
        let _small = SmallBirth::begin();
        cx.open_window(options, |window, cx| {
            let display = NativeSurface::of(window).ok().map(NativeSurface::display);
            let reservation = self
                .settle(window)
                .inspect_err(|error| {
                    report!("a surface is not docked to its screen edge: {error:#}");
                })
                .ok()
                .flatten();
            let native = NativeSurface::of(window).ok();
            let depth = self.depth(window);
            cx.new(|cx| {
                if let Some(native) = native {
                    Self::give_way(native, cx);
                    Self::follow_pointer(native, cx);
                }
                EdgeSurfaceView {
                    surface: self,
                    content: cx.new(|cx| build(display, cx)),
                    reservation,
                    depth,
                    shown: Spring::new(cx.global::<Theme>().motion.spatial, 1.),
                    away: None,
                    concealed: false,
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

    /// Has the view `cx` makes hide and come back with the pointer, while
    /// its surface is one that hides. Ends with the view.
    fn follow_pointer<V: Render>(native: NativeSurface, cx: &mut Context<EdgeSurfaceView<V>>) {
        cx.spawn(async move |view, cx: &mut AsyncApp| {
            while let Ok(wait) = view.update(cx, |view, cx| view.follow(native, cx)) {
                cx.background_executor().timer(wait).await;
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

    /// Makes the window a shell surface at its screen edge without
    /// reserving the edge.
    fn float(self, window: &Window) -> anyhow::Result<()> {
        let surface = NativeSurface::of(window)?;
        surface.adopt_as_shell_surface()?;
        surface.reveal(false);
        surface.place(surface.strip(self.edge, self.depth(window))?)
    }

    /// Docks the window, or floats it when the surface is one that hides.
    fn settle(self, window: &Window) -> anyhow::Result<Option<EdgeReservation>> {
        if self.hide.hides() {
            self.float(window).map(|()| None)
        } else {
            self.dock(window).map(Some)
        }
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
    /// Held so maximized windows keep clear of the surface while it exists;
    /// a surface that hides has none.
    reservation: Option<EdgeReservation>,
    /// The thickness in physical pixels.
    depth: i32,
    /// 0 while the surface is away, 1 while it is in its place.
    shown: Spring,
    /// Since when the pointer has been elsewhere.
    away: Option<Instant>,
    /// Whether the window is hidden, after the surface slid away.
    concealed: bool,
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
        hide: Hiding,
        window: &Window,
        cx: &mut Context<Self>,
        build: impl FnOnce(Option<DisplayKey>, &mut Context<V>) -> V,
    ) {
        let changed = thickness != self.surface.thickness || hide != self.surface.hide;
        if changed || (self.reservation.is_none() && !hide.hides()) {
            self.surface.thickness = thickness;
            self.surface.hide = hide;
            self.depth = self.surface.depth(window);
            let docked = match &self.reservation {
                Some(reservation) if !hide.hides() => reservation
                    .occupy(self.surface.edge, self.depth)
                    .map(|()| None),
                _ => {
                    // The edge is given back before the surface floats.
                    self.reservation = None;
                    self.surface.settle(window)
                }
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

impl<V: Render> EdgeSurfaceView<V> {
    /// Has the surface go or come back by where the pointer is, and says
    /// how long to wait before looking again.
    fn follow(&mut self, native: NativeSurface, cx: &mut Context<Self>) -> Duration {
        if !self.surface.hide.hides() {
            if self.concealed || self.shown.target() != 1. {
                self.come_back(native, cx);
            }
            return HIDE_CHECK;
        }
        let edge = self.surface.edge;
        let wanted = if self.concealed {
            // Not over a game shown full screen, which has the edge.
            native.pointer_in_strip(edge, CALLING_EDGE) && !native.display_is_filled()
        } else {
            native.pointer_in_strip(edge, self.depth) || pointer_on_own_window()
        };
        // A surface that goes only under windows stays while none lies in
        // its place.
        let clear = self.surface.hide == Hiding::Overlap
            && !native.display_is_filled()
            && !Self::covered(native, edge, self.depth);
        if wanted || clear || Popups::is_open(cx) {
            self.away = None;
            if self.concealed || self.shown.target() != 1. {
                self.come_back(native, cx);
            }
            return POINTER_CHECK;
        }
        let away = self.away.get_or_insert_with(Instant::now).elapsed();
        if away >= HIDE_AFTER && self.shown.target() != 0. {
            self.shown.animate_to(0.);
            cx.notify();
        }
        if away >= HIDE_AFTER + SLIDE && !self.concealed {
            self.concealed = true;
            native.conceal();
        }
        POINTER_CHECK
    }

    /// Whether a window of an application lies in the strip of the surface.
    fn covered(native: NativeSurface, edge: ScreenEdge, depth: i32) -> bool {
        let Ok(strip) = native.strip(edge, depth) else {
            return false;
        };
        let meets = |window: &AppWindow| {
            // A minimized window is far off the screen.
            window.id.is_shown()
                && window.id.frame().is_some_and(|frame| {
                    frame.left < strip.right
                        && strip.left < frame.right
                        && frame.top < strip.bottom
                        && strip.top < frame.bottom
                })
        };
        AppWindow::enumerate().iter().any(meets)
    }

    fn come_back(&mut self, native: NativeSurface, cx: &mut Context<Self>) {
        if self.concealed {
            self.concealed = false;
            native.reveal(false);
        }
        self.shown.animate_to(1.);
        cx.notify();
    }
}

impl<V: Render> Render for EdgeSurfaceView<V> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let shown = self.shown.value(window).clamp(0., 1.);
        let travel = self.surface.thickness * (1. - shown);
        let offset = match self.surface.edge {
            ScreenEdge::Top => -travel,
            ScreenEdge::Bottom => travel,
        };
        div()
            .size_full()
            .font_family(cx.global::<Theme>().typography.family)
            .child(
                div()
                    .relative()
                    .top(offset)
                    .size_full()
                    .opacity(shown)
                    .child(self.content.clone()),
            )
    }
}
