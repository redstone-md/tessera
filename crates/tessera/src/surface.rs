// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shell surfaces: borderless windows docked to a display edge, any of the
//! four. Several may share one edge of a display: those that keep their
//! edge stand one after the other, the first of the configuration at the
//! edge, as whoever keeps the edge bars fits them; those that hide stand
//! one after the other from the edge in the order they are given.

use std::collections::HashMap;
use std::panic::Location;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Bounds, Context, Element, ElementId, Entity, Global,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, ParentElement, Pixels,
    PlatformDisplay, Render, Styled, Subscription, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowId, WindowKind, WindowOptions,
    div, point, px, size,
};

pub use gpui::Axis;

use crate::config::{Hiding, SolidWhen, SurfaceConfig};
use crate::motion::Spring;
use crate::panel::Arranging;
use crate::platform::{
    AppWindow, DisplayKey, EdgeBand, EdgeReservation, EdgeSpan, HidingClaim, NativeSurface,
    ScreenEdge, ScreenRect, SmallBirth, pointer_on_own_window,
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

/// How often a surface that turns solid by maximized windows looks for
/// them.
const SOLID_CHECK: Duration = Duration::from_millis(250);

/// How long a surface takes to slide away before its window is hidden.
const SLIDE: Duration = Duration::from_millis(450);

/// How deep, in physical pixels, the edge of the display is that calls a
/// hidden surface back.
const CALLING_EDGE: i32 = 2;

/// How far, in physical pixels, a maximized window may be from a surface
/// and still touch it: Windows lets a maximized window reach a few pixels
/// past the room it gives it, under its invisible borders.
const SOLID_REACH: i32 = 16;

/// How a surface looks, beyond what the theme says.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SurfaceLook {
    /// How much of the background of the strip is drawn; none where
    /// `None`.
    pub opacity: Option<f32>,
    /// Whether the widgets have fields of their own, where the surface
    /// says instead of the theme.
    pub floating: Option<bool>,
    /// Whether the background turns opaque while a maximized window
    /// touches the surface.
    pub solid_when_maximized: bool,
}

/// Describes a strip along one edge of a display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeSurface {
    pub edge: ScreenEdge,
    pub thickness: Pixels,
    /// When the strip goes away; one that does reserves no screen space.
    pub hide: Hiding,
    /// How far from the edge a strip that hides stands, past those that
    /// hide at the same edge before it. One that keeps its edge is put in
    /// its place by whoever keeps the edge bars.
    pub inset: Pixels,
    /// The part of the edge's length the strip takes.
    pub span: EdgeSpan,
    /// How long the pointer rests at the edge before a hidden strip comes
    /// back, and how long it is elsewhere before the strip goes.
    pub show_delay: Duration,
    pub hide_delay: Duration,
    pub look: SurfaceLook,
}

/// The windows of the open surfaces and the edge each is at, for what
/// lays itself out by the edge of the surface it is on.
#[derive(Default)]
struct SurfaceEdges(HashMap<WindowId, ScreenEdge>);

impl Global for SurfaceEdges {}

/// The edge of its display that the surface in `window` is docked to;
/// `None` for a window that is no surface, as a popup.
pub fn edge_of(window: &Window, cx: &App) -> Option<ScreenEdge> {
    let edges = cx.try_global::<SurfaceEdges>()?;
    edges.0.get(&window.window_handle().window_id()).copied()
}

/// The direction the surface in `window` runs in: across the display for
/// one at the top or the bottom, down it for one at the left or the
/// right, and across for a window that is no surface.
pub fn axis_of(window: &Window, cx: &App) -> Axis {
    edge_of(window, cx).map_or(Axis::Horizontal, ScreenEdge::along)
}

/// Until when the surfaces that hide on a display are shown regardless of
/// the pointer.
#[derive(Default)]
struct Reveals(HashMap<DisplayKey, Instant>);

impl Global for Reveals {}

impl Reveals {
    fn hold(display: DisplayKey, cx: &App) -> bool {
        let reveals = cx.try_global::<Self>();
        let until = reveals.and_then(|reveals| reveals.0.get(&display));
        until.is_some_and(|until| Instant::now() < *until)
    }
}

/// `theme` as a bar of `look` has it: with fields behind the widgets or
/// without, as the bar says where it does.
pub fn bar_theme(mut theme: Theme, floating: Option<bool>) -> Theme {
    if let Some(floating) = floating {
        theme.bar.flat = !floating;
    }
    theme
}

impl EdgeSurface {
    /// The strip `layout` describes, standing `inset` from its edge if it
    /// hides.
    pub fn of(layout: &SurfaceConfig, inset: Pixels) -> Self {
        Self {
            edge: layout.edge,
            thickness: px(layout.thickness),
            hide: layout.hide,
            inset,
            span: layout.span(),
            show_delay: layout.show_after(),
            hide_delay: layout.hide_after(),
            look: SurfaceLook {
                opacity: layout.opacity(),
                floating: layout.floating,
                solid_when_maximized: layout.solid_when == Some(SolidWhen::Maximized),
            },
        }
    }

    /// Shows the surfaces that hide on `display` for `duration`, wherever
    /// the pointer is, as the dock does for an application that asks for
    /// attention. A later call for the same display replaces the time.
    #[allow(dead_code, reason = "for the dock, which reveals itself for attention")]
    pub fn reveal(display: DisplayKey, duration: Duration, cx: &mut App) {
        let reveals = cx.default_global::<Reveals>();
        reveals.0.retain(|_, until| Instant::now() < *until);
        reveals.0.insert(display, Instant::now() + duration);
    }

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
            let id = window.window_handle().window_id();
            cx.default_global::<SurfaceEdges>().0.insert(id, self.edge);
            let display = NativeSurface::of(window).ok().map(NativeSurface::display);
            let settled = self.settle(window);
            let native = NativeSurface::of(window).ok();
            let band = self.band(window);
            cx.new(|cx| {
                if let Some(native) = native {
                    Self::give_way(native, cx);
                    Self::follow_pointer(native, cx);
                }
                let released = cx.on_release(move |_, cx| {
                    if let Some(edges) = cx.try_global::<SurfaceEdges>()
                        && edges.0.contains_key(&id)
                    {
                        cx.global_mut::<SurfaceEdges>().0.remove(&id);
                    }
                });
                let motion = cx.global::<Theme>().motion;
                EdgeSurfaceView {
                    surface: self,
                    content: cx.new(|cx| build(display, cx)),
                    reservation: settled.reservation,
                    claim: settled.claim,
                    band,
                    shown: Spring::new(motion.spatial, 1.),
                    solid: Spring::new(motion.effects, 0.),
                    away: None,
                    called: None,
                    concealed: false,
                    placed_on: native.and_then(|native| native.display_frame().ok()),
                    placed_for: EdgeReservation::generation(),
                    _released: released,
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
    /// its surface is one that hides, and turn solid by maximized windows,
    /// where it does. Ends with the view.
    fn follow_pointer<V: Render>(native: NativeSurface, cx: &mut Context<EdgeSurfaceView<V>>) {
        cx.spawn(async move |view, cx: &mut AsyncApp| {
            while let Ok(wait) = view.update(cx, |view, cx| view.follow(native, cx)) {
                cx.background_executor().timer(wait).await;
            }
        })
        .detach();
    }

    /// The bounds of the strip on a display of `display` bounds, in
    /// logical pixels, as the window is first opened with.
    fn bounds_on(self, display: Bounds<Pixels>) -> Bounds<Pixels> {
        let (width, height) = (display.size.width, display.size.height);
        let length = self.edge.along_of(width, height);
        let (from, to) = (self.span.from.clamp(0., 1.), self.span.to.clamp(0., 1.));
        let start = length * from;
        let part = (length * (to - from)).max(px(1.));
        let (thickness, inset) = (self.thickness, self.inset);
        let (origin, extent) = match self.edge {
            ScreenEdge::Top => (point(start, inset), size(part, thickness)),
            ScreenEdge::Bottom => (
                point(start, height - thickness - inset),
                size(part, thickness),
            ),
            ScreenEdge::Left => (point(inset, start), size(thickness, part)),
            ScreenEdge::Right => (
                point(width - thickness - inset, start),
                size(thickness, part),
            ),
        };
        Bounds {
            origin: display.origin + origin,
            size: extent,
        }
    }

    /// Where the strip lies on the display of `window`, in physical pixels.
    fn band(self, window: &Window) -> EdgeBand {
        let physical = |length: Pixels| (length / px(1.) * window.scale_factor()).round() as i32;
        EdgeBand {
            edge: self.edge,
            depth: physical(self.thickness),
            inset: physical(self.inset),
            span: self.span,
        }
    }

    /// Makes the window a shell surface and reserves its screen edge.
    fn dock(self, window: &Window) -> anyhow::Result<EdgeReservation> {
        let surface = NativeSurface::of(window)?;
        surface.adopt_as_shell_surface()?;
        // Windows may leave the first window of a program hidden when it
        // was started that way.
        surface.reveal(false);
        surface.reserve_band(self.band(window))
    }

    /// Makes the window a shell surface at its screen edge without
    /// reserving the edge, and registers it as the bar that hides at its
    /// edge where no other is.
    fn float(self, window: &Window) -> anyhow::Result<Option<HidingClaim>> {
        let surface = NativeSurface::of(window)?;
        surface.adopt_as_shell_surface()?;
        surface.reveal(false);
        surface.place(surface.band(self.band(window))?)?;
        Ok(surface.claim_hiding_edge(self.edge))
    }

    /// Docks the window, or floats it when the surface is one that hides.
    /// What fails is reported.
    fn settle(self, window: &Window) -> Settled {
        let settled = if self.hide.hides() {
            self.float(window).map(|claim| Settled {
                reservation: None,
                claim,
            })
        } else {
            self.dock(window).map(|reservation| Settled {
                reservation: Some(reservation),
                claim: None,
            })
        };
        settled.unwrap_or_else(|error| {
            report!("a surface is not docked to its screen edge: {error:#}");
            Settled::default()
        })
    }

    /// Whether the strip lies elsewhere than `other` puts it.
    fn moves_from(self, other: Self) -> bool {
        (self.edge, self.thickness, self.hide, self.inset, self.span)
            != (
                other.edge,
                other.thickness,
                other.hide,
                other.inset,
                other.span,
            )
    }
}

/// What holds a surface's place: the reservation of its edge, or the
/// registration of a surface that hides.
#[derive(Default)]
struct Settled {
    reservation: Option<EdgeReservation>,
    claim: Option<HidingClaim>,
}

/// Whether `frame` reaches within `reach` of `band`.
fn touches(frame: ScreenRect, band: ScreenRect, reach: i32) -> bool {
    frame.left - reach < band.right
        && band.left < frame.right + reach
        && frame.top - reach < band.bottom
        && band.top < frame.bottom + reach
}

/// How strong the background of a surface is: `opacity` of it, or nothing
/// where `None`, and all of it as far as it is `solid`.
fn background_strength(opacity: Option<f32>, solid: f32) -> f32 {
    let base = opacity.unwrap_or(0.);
    (base + (1. - base) * solid.clamp(0., 1.)).clamp(0., 1.)
}

/// The root view of an edge surface: a transparent strip around its content.
pub struct EdgeSurfaceView<V> {
    surface: EdgeSurface,
    content: Entity<V>,
    /// Held so maximized windows keep clear of the surface while it exists;
    /// a surface that hides has none.
    reservation: Option<EdgeReservation>,
    /// Held while the surface hides and is the bar that hides at its edge.
    claim: Option<HidingClaim>,
    /// Where the strip lies, in physical pixels of its display.
    band: EdgeBand,
    /// 0 while the surface is away, 1 while it is in its place.
    shown: Spring,
    /// 1 while a maximized window touches a surface that turns solid by it.
    solid: Spring,
    /// Since when the pointer has been elsewhere.
    away: Option<Instant>,
    /// Since when the pointer has been at the edge of a hidden surface.
    called: Option<Instant>,
    /// Whether the window is hidden, after the surface slid away.
    concealed: bool,
    /// The bounds of the display the surface was last placed on.
    placed_on: Option<ScreenRect>,
    /// What [`EdgeReservation::generation`] said when it was placed.
    placed_for: u64,
    _released: Subscription,
}

impl<V: Render> EdgeSurfaceView<V> {
    pub fn content(&self) -> &Entity<V> {
        &self.content
    }

    /// Makes the strip what `surface` describes and replaces its content
    /// with the view `build` returns. The window is kept, and so is its
    /// reservation of the screen edge while it keeps one.
    pub fn refill(
        &mut self,
        surface: EdgeSurface,
        window: &Window,
        cx: &mut Context<Self>,
        build: impl FnOnce(Option<DisplayKey>, &mut Context<V>) -> V,
    ) {
        let moved = surface.moves_from(self.surface);
        let hides = surface.hide.hides();
        self.surface = surface;
        let id = window.window_handle().window_id();
        cx.default_global::<SurfaceEdges>()
            .0
            .insert(id, surface.edge);
        if moved || (self.reservation.is_none() && !hides) {
            self.band = surface.band(window);
            let kept = match &self.reservation {
                Some(reservation) if !hides => Some(reservation.occupy_band(surface.band(window))),
                _ => None,
            };
            match kept {
                Some(Ok(())) => {}
                Some(Err(error)) => {
                    report!("a surface is not docked to its screen edge: {error:#}");
                }
                None => {
                    // The edge is given back before the surface floats.
                    self.reservation = None;
                    self.claim = None;
                    let settled = surface.settle(window);
                    (self.reservation, self.claim) = (settled.reservation, settled.claim);
                }
            }
        }
        let display = NativeSurface::of(window).ok().map(NativeSurface::display);
        self.content = cx.new(|cx| build(display, cx));
        cx.notify();
    }

    /// Has the surface go or come back by where the pointer is, and turn
    /// solid by maximized windows, and says how long to wait before
    /// looking again.
    fn follow(&mut self, native: NativeSurface, cx: &mut Context<Self>) -> Duration {
        self.realign(native, cx);
        let solid_wait = self.follow_windows(native, cx);
        if !self.surface.hide.hides() {
            if self.concealed || self.shown.target() != 1. {
                self.come_back(native, cx);
            }
            return HIDE_CHECK.min(solid_wait);
        }
        let band = self.band;
        let held = Reveals::hold(native.display(), cx) || Arranging::is_on(cx);
        let wanted = if self.concealed {
            // Not over a game shown full screen, which has the edge. The
            // edge of the display calls it, wherever the strip stands.
            let calling = EdgeBand {
                depth: CALLING_EDGE,
                inset: 0,
                ..band
            };
            let called = native.pointer_in_band(calling) && !native.display_is_filled();
            match called {
                true => {
                    let since = self.called.get_or_insert_with(Instant::now);
                    since.elapsed() >= self.surface.show_delay
                }
                false => {
                    self.called = None;
                    false
                }
            }
        } else {
            native.pointer_in_band(band) || pointer_on_own_window()
        };
        // A surface that goes only under windows stays while none lies in
        // its place.
        let clear = self.surface.hide == Hiding::Overlap
            && !native.display_is_filled()
            && !Self::covered(native, band);
        if wanted || clear || held || Popups::is_open(cx) {
            self.away = None;
            if wanted || clear || held {
                self.called = None;
            }
            if self.concealed || self.shown.target() != 1. {
                self.come_back(native, cx);
            }
            return POINTER_CHECK;
        }
        let hide_after = self.surface.hide_delay;
        let away = self.away.get_or_insert_with(Instant::now).elapsed();
        if away >= hide_after && self.shown.target() != 0. {
            self.shown.animate_to(0.);
            cx.notify();
        }
        if away >= hide_after + SLIDE && !self.concealed {
            self.concealed = true;
            native.conceal();
        }
        POINTER_CHECK
    }

    /// Has the background turn opaque while a maximized window touches a
    /// surface that turns solid by it, and says how long to wait before
    /// looking again.
    fn follow_windows(&mut self, native: NativeSurface, cx: &mut Context<Self>) -> Duration {
        if !self.surface.look.solid_when_maximized {
            if self.solid.target() != 0. {
                self.solid.animate_to(0.);
                cx.notify();
            }
            return HIDE_CHECK;
        }
        let touched = native.band(self.band).is_ok_and(|band| {
            AppWindow::enumerate().iter().any(|window| {
                window.id.is_maximized()
                    && window
                        .id
                        .frame()
                        .is_some_and(|frame| touches(frame, band, SOLID_REACH))
            })
        });
        let wanted = if touched { 1. } else { 0. };
        if self.solid.target() != wanted {
            self.solid.animate_to(wanted);
            cx.notify();
        }
        SOLID_CHECK
    }

    /// Puts the surface back at its screen edge after its display changed,
    /// as it does when the resolution is set anew, or after the other edge
    /// bars moved.
    fn realign(&mut self, native: NativeSurface, cx: &mut Context<Self>) {
        let Ok(display) = native.display_frame() else {
            return;
        };
        let bars = EdgeReservation::generation();
        if self.placed_on == Some(display) && self.placed_for == bars {
            return;
        }
        self.placed_on = Some(display);
        self.placed_for = bars;
        let band = self.band;
        let moved = match &self.reservation {
            Some(reservation) => reservation.occupy_band(band),
            None => native.band(band).and_then(|area| native.place(area)),
        };
        if let Err(error) = moved {
            report!("a surface did not follow its display: {error:#}");
        }
        // Whoever keeps the bars, or the display, may have changed.
        if let Some(claim) = &mut self.claim
            && !claim.renew()
        {
            self.claim = None;
        }
        cx.notify();
    }

    /// Whether a window of an application lies in `band` of the surface's
    /// display.
    fn covered(native: NativeSurface, band: EdgeBand) -> bool {
        let Ok(strip) = native.band(band) else {
            return false;
        };
        let meets = |window: &AppWindow| {
            // A minimized window is far off the screen.
            window.id.is_shown()
                && window
                    .id
                    .frame()
                    .is_some_and(|frame| touches(frame, strip, 0))
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

    /// The background of the strip, where it has one now.
    fn background(&mut self, theme: &Theme, window: &Window) -> Option<Hsla> {
        let solid = self.solid.value(window);
        let strength = background_strength(self.surface.look.opacity, solid);
        (strength > 0.001).then(|| theme.colors.surface_container.opacity(strength))
    }
}

impl<V: Render> Render for EdgeSurfaceView<V> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = bar_theme(*cx.global::<Theme>(), self.surface.look.floating);
        let shown = self.shown.value(window).clamp(0., 1.);
        let offset = self
            .surface
            .edge
            .travel(self.surface.thickness * (1. - shown));
        let background = self.background(&theme, window).map(|color| {
            let ground = div().absolute().top_0().left_0().size_full().bg(color);
            match self.surface.span.is_whole() {
                true => ground,
                false => ground.rounded(theme.shape.large),
            }
        });
        let differs = theme.bar.flat != cx.global::<Theme>().bar.flat;
        let content: AnyElement = match differs {
            true => ThemeScope {
                theme,
                child: self.content.clone().into_any_element(),
            }
            .into_any_element(),
            false => self.content.clone().into_any_element(),
        };
        div()
            .size_full()
            .font_family(theme.typography.family)
            .child(
                div()
                    .relative()
                    .left(offset.x)
                    .top(offset.y)
                    .size_full()
                    .opacity(shown)
                    .children(background)
                    .child(content),
            )
    }
}

/// Draws its child with another theme than the one in use: the theme is
/// put in place while the child is laid out and painted, and the one in
/// use is put back after each. What a widget reads of the theme as it is
/// drawn follows; what it read before, in a handler of the pointer, does
/// not.
struct ThemeScope {
    theme: Theme,
    child: AnyElement,
}

impl ThemeScope {
    /// Runs `work` with the theme of the scope in place.
    fn within<R>(theme: Theme, cx: &mut App, work: impl FnOnce(&mut App) -> R) -> R {
        let outer = std::mem::replace(cx.global_mut::<Theme>(), theme);
        let result = work(cx);
        *cx.global_mut::<Theme>() = outer;
        result
    }
}

impl IntoElement for ThemeScope {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for ThemeScope {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = &mut self.child;
        let layout = Self::within(self.theme, cx, |cx| child.request_layout(window, cx));
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        Self::within(self.theme, cx, |cx| {
            child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        Self::within(self.theme, cx, |cx| child.paint(window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    fn surface(edge: ScreenEdge) -> EdgeSurface {
        EdgeSurface {
            edge,
            thickness: px(40.),
            hide: Hiding::Never,
            inset: px(0.),
            span: EdgeSpan::WHOLE,
            show_delay: Duration::ZERO,
            hide_delay: Duration::from_millis(700),
            look: SurfaceLook::default(),
        }
    }

    #[test]
    fn a_strip_opens_at_its_edge_and_in_its_part_of_it() {
        let display = Bounds {
            origin: point(px(100.), px(0.)),
            size: size(px(1000.), px(800.)),
        };
        let at = |surface: EdgeSurface| {
            let bounds = surface.bounds_on(display);
            (
                bounds.origin.x / px(1.),
                bounds.origin.y / px(1.),
                bounds.size.width / px(1.),
                bounds.size.height / px(1.),
            )
        };
        assert_eq!(at(surface(ScreenEdge::Top)), (100., 0., 1000., 40.));
        assert_eq!(at(surface(ScreenEdge::Bottom)), (100., 760., 1000., 40.));
        assert_eq!(at(surface(ScreenEdge::Left)), (100., 0., 40., 800.));
        assert_eq!(at(surface(ScreenEdge::Right)), (1060., 0., 40., 800.));
        let short = EdgeSurface {
            inset: px(10.),
            span: EdgeSpan {
                from: 0.25,
                to: 0.75,
            },
            ..surface(ScreenEdge::Left)
        };
        assert_eq!(at(short), (110., 200., 40., 400.));
    }

    #[test]
    fn a_window_touches_a_strip_it_reaches_near() {
        let bar = rect(0, 0, 1920, 48);
        // A maximized window starts a few pixels above the room it is given.
        assert!(touches(rect(-8, 40, 1928, 1088), bar, SOLID_REACH));
        assert!(touches(rect(0, 60, 1920, 1080), bar, SOLID_REACH));
        assert!(!touches(rect(0, 100, 1920, 1080), bar, SOLID_REACH));
        assert!(!touches(rect(0, 48, 1920, 1080), bar, 0));
        assert!(touches(rect(0, 47, 1920, 1080), bar, 0));
    }

    #[test]
    fn the_background_turns_solid_from_its_opacity() {
        assert_eq!(background_strength(None, 0.), 0.);
        assert_eq!(background_strength(None, 1.), 1.);
        assert_eq!(background_strength(Some(0.5), 0.), 0.5);
        assert_eq!(background_strength(Some(0.5), 0.5), 0.75);
        assert_eq!(background_strength(Some(0.4), 2.), 1.);
    }

    #[test]
    fn a_surface_moves_only_for_what_places_it() {
        let bar = surface(ScreenEdge::Top);
        let looks = EdgeSurface {
            look: SurfaceLook {
                opacity: Some(0.5),
                ..SurfaceLook::default()
            },
            hide_delay: Duration::from_secs(2),
            ..bar
        };
        assert!(!looks.moves_from(bar));
        let thicker = EdgeSurface {
            thickness: px(48.),
            ..bar
        };
        assert!(thicker.moves_from(bar));
        let shorter = EdgeSurface {
            span: EdgeSpan { from: 0., to: 0.5 },
            ..bar
        };
        assert!(shorter.moves_from(bar));
    }
}
