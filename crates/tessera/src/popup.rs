// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Popups: floating panels of widgets that open next to a surface.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    AnyView, AnyWindowHandle, App, AppContext, Bounds, Context, DisplayId, Div, Global,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, StatefulInteractiveElement,
    Styled, Subscription, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, div, point, px, size,
};

use crate::config::{PopupConfig, PopupPlace, WidgetSpec};
use crate::module::WidgetRegistry;
use crate::motion::Spring;
use crate::platform::{DisplayKey, NativeSurface, ScreenEdge, ScreenRect, SmallBirth, Veil};
use crate::surface::edge_of;
use crate::theme::Theme;
use crate::ui::release_hover;

/// Transparent space around the panel of a floating window, which its
/// motion moves through.
pub const MARGIN: f32 = 12.;

/// How long after a popup closes a request to open it again is ignored. The
/// click that asks to close an open popup first takes focus from it, which
/// closes it; without this the same click would then reopen it.
pub(crate) const REOPEN_GUARD: Duration = Duration::from_millis(250);

/// How dark the veil behind a popup in the middle of the display is.
const VEIL: f32 = 0.42;

/// How far below its place such a popup starts, in logical pixels.
const RISE: f32 = 28.;

/// Where a floating window should appear. Positions are in physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct PopupAnchor {
    /// The surface the popup belongs to.
    pub surface: ScreenRect,
    /// The edge that surface is docked to; the popup opens away from it,
    /// below a surface at the top, to the right of one at the left.
    pub edge: ScreenEdge,
    /// The horizontal screen position to center the popup on, beside a
    /// surface at the top or the bottom.
    pub x: i32,
    /// The vertical screen position to center the popup on, beside a
    /// surface at the left or the right.
    pub y: i32,
    /// The display to stay on.
    pub screen: ScreenRect,
    /// Physical pixels per logical one on that display.
    pub scale: f32,
    pub display_id: Option<DisplayId>,
    pub display: Option<DisplayKey>,
}

impl PopupAnchor {
    /// The anchor at `x` of `surface`, which is docked to `edge` of the
    /// display `screen`; beside a surface at the left or the right it is
    /// at the middle of the surface's height, until `y` says otherwise.
    pub fn new(
        surface: ScreenRect,
        edge: ScreenEdge,
        x: i32,
        screen: ScreenRect,
        scale: f32,
        display_id: Option<DisplayId>,
        display: Option<DisplayKey>,
    ) -> Self {
        Self {
            surface,
            edge,
            x,
            y: surface.top + (surface.bottom - surface.top) / 2,
            screen,
            scale,
            display_id,
            display,
        }
    }

    /// The anchor at `x`, a horizontal position in `window`, which is a
    /// surface docked to an edge of its display. Beside a surface at the
    /// left or the right it is where the pointer is, along the surface.
    pub fn at(window: &Window, x: Pixels, cx: &App) -> anyhow::Result<Self> {
        let pointer = window.mouse_position();
        Self::at_point(window, gpui::point(x, pointer.y), cx)
    }

    /// The anchor at `position` of `window`, which is a surface docked to
    /// an edge of its display: what opens there opens away from the
    /// surface, centered on `position` along the surface.
    pub fn at_point(
        window: &Window,
        position: gpui::Point<Pixels>,
        cx: &App,
    ) -> anyhow::Result<Self> {
        let native = NativeSurface::of(window)?;
        let (surface, screen) = (native.frame()?, native.display_frame()?);
        let scale = window.scale_factor();
        let edge = edge_of(window, cx).unwrap_or_else(|| nearest_edge(surface, screen));
        let physical = |length: Pixels| (length / px(1.) * scale).round() as i32;
        let anchor = Self::new(
            surface,
            edge,
            surface.left + physical(position.x),
            screen,
            scale,
            window.display(cx).map(|display| display.id()),
            Some(native.display()),
        );
        Ok(Self {
            y: surface.top + physical(position.y),
            ..anchor
        })
    }

    /// The anchor under the point `position` of an ordinary window, such
    /// as the one a click with the right button names: what opens there
    /// hangs from that point.
    pub fn under(window: &Window, position: gpui::Point<Pixels>, cx: &App) -> anyhow::Result<Self> {
        let native = NativeSurface::of(window)?;
        let (frame, screen) = (native.frame()?, native.display_frame()?);
        let scale = window.scale_factor();
        let x = frame.left + (position.x / px(1.) * scale).round() as i32;
        let y = frame.top + (position.y / px(1.) * scale).round() as i32;
        let point = ScreenRect {
            left: x,
            top: y,
            right: x,
            bottom: y,
        };
        let display_id = window.display(cx).map(|display| display.id());
        Ok(Self::new(
            point,
            ScreenEdge::Top,
            x,
            screen,
            scale,
            display_id,
            Some(native.display()),
        ))
    }

    /// The frame of a floating window whose panel is `width` by `height`
    /// logical pixels and stands in the middle of the display, or fills
    /// the display where it is smaller than that.
    pub fn middle(&self, width: f32, height: f32) -> ScreenRect {
        let physical = |length: f32| ((length + MARGIN * 2.) * self.scale).round() as i32;
        let screen = self.screen;
        let width = physical(width).min(screen.right - screen.left);
        let height = physical(height).min(screen.bottom - screen.top);
        let left = (screen.left + screen.right - width) / 2;
        let top = (screen.top + screen.bottom - height) / 2;
        ScreenRect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    /// The frame of a floating window whose panel is `width` by `height`
    /// logical pixels and stands in the middle of the width of the
    /// display, a sixth of its height below its top.
    ///
    /// Beside a bar at the left or the right of the display it stands in
    /// the middle of the width that bar leaves, as far as it fits there.
    pub fn near_top(&self, width: f32, height: f32) -> ScreenRect {
        let middle = self.middle(width, height);
        let screen = self.screen;
        let height = middle.bottom - middle.top;
        let top = (screen.top + (screen.bottom - screen.top) / 6).min(screen.bottom - height);
        let top = top.max(screen.top);
        let wide = middle.right - middle.left;
        let (from, to) = self.room_beside();
        let left = match to - from >= wide {
            true => from + (to - from - wide) / 2,
            false => middle.left,
        };
        ScreenRect {
            left,
            top,
            right: left + wide,
            bottom: top + height,
        }
    }

    /// The part of the width of the display a bar at its left or right
    /// leaves, as left and right; all of it beside a bar at the top or
    /// the bottom, or one that is a point.
    fn room_beside(&self) -> (i32, i32) {
        let (screen, surface) = (self.screen, self.surface);
        let bar = surface.right > surface.left && surface.bottom > surface.top;
        match self.edge {
            ScreenEdge::Left if bar => (surface.right.min(screen.right), screen.right),
            ScreenEdge::Right if bar => (screen.left, surface.left.max(screen.left)),
            _ => (screen.left, screen.right),
        }
    }

    /// The frame of a floating window whose panel is `width` by `height`
    /// logical pixels: touching the surface, so the panel sits one margin
    /// away from it, centered on the anchor and kept on the display. Below
    /// a surface at the top, above one at the bottom, and to the right or
    /// the left of one at the left or the right.
    pub fn frame(&self, width: f32, height: f32) -> ScreenRect {
        let physical = |length: f32| ((length + MARGIN * 2.) * self.scale).round() as i32;
        let (width, height) = (physical(width), physical(height));
        let screen = self.screen;
        let centered = |middle: i32, length: i32, from: i32, to: i32| {
            (middle - length / 2).clamp(from, (to - length).max(from))
        };
        let (left, top) = match self.edge {
            ScreenEdge::Top => (
                centered(self.x, width, screen.left, screen.right),
                self.surface.bottom,
            ),
            ScreenEdge::Bottom => (
                centered(self.x, width, screen.left, screen.right),
                self.surface.top - height,
            ),
            ScreenEdge::Left => (
                self.surface.right,
                centered(self.y, height, screen.top, screen.bottom),
            ),
            ScreenEdge::Right => (
                self.surface.left - width,
                centered(self.y, height, screen.top, screen.bottom),
            ),
        };
        ScreenRect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }
}

/// The edge of `screen` the surface at `surface` is docked to, judged by
/// its shape and where it lies: a strip taller than wide is at the left or
/// the right, one wider than tall at the top or the bottom.
fn nearest_edge(surface: ScreenRect, screen: ScreenRect) -> ScreenEdge {
    let tall = surface.bottom - surface.top > surface.right - surface.left;
    match tall {
        true if surface.left + surface.right < screen.left + screen.right => ScreenEdge::Left,
        true => ScreenEdge::Right,
        false if surface.top + surface.bottom < screen.top + screen.bottom => ScreenEdge::Top,
        false => ScreenEdge::Bottom,
    }
}

/// The options of a floating window on the display `display_id`. GPUI sizes
/// the window for a frame that adopting it removes, so `rough` only picks
/// the display. The window opens hidden: it is to be adopted, placed and
/// then revealed, or it is seen for a moment with its frame. `focus`
/// says whether it takes the keyboard.
pub fn floating_window(
    rough: Bounds<Pixels>,
    display_id: Option<DisplayId>,
    focus: bool,
) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(rough)),
        // See `EdgeSurface::open` for why a title bar is requested.
        titlebar: Some(TitlebarOptions::default()),
        focus,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    }
}

/// How many closed floating windows wait, hidden, to be opened again: a
/// popup or a menu and the tip over it.
const SPARES_KEPT: usize = 2;

/// Floating windows that closed and wait, hidden, to be opened again with
/// another view: showing a window takes less than making one.
#[derive(Default)]
struct Spares {
    kept: Vec<AnyWindowHandle>,
}

impl Global for Spares {}

/// Opens a floating window whose root view `build` makes: one that waits
/// is taken if there is one, and a window is made otherwise. `build`
/// adopts, places and reveals the window either way, and a window that
/// was another kind of floating window before is adopted anew by that.
pub fn open_floating<V: Render + 'static>(
    rough: Bounds<Pixels>,
    display_id: Option<DisplayId>,
    focus: bool,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V,
) -> anyhow::Result<WindowHandle<V>> {
    let mut build = Some(build);
    while let Some(spare) = cx.default_global::<Spares>().kept.pop() {
        // A window that is gone does not run this, and `build` is kept.
        let taken = spare.update(cx, |_, window, cx| {
            if let Some(build) = build.take() {
                window.replace_root(cx, build);
            }
        });
        if taken.is_ok() {
            return Ok(WindowHandle::new(spare.window_id()));
        }
    }
    let Some(build) = build else {
        anyhow::bail!("the view of a floating window was given away");
    };
    let _small = SmallBirth::begin();
    cx.open_window(floating_window(rough, display_id, focus), |window, cx| {
        cx.new(|cx| build(window, cx))
    })
}

/// Puts away the floating window `window`, whose view has closed: it is
/// hidden to wait for [`open_floating`], or removed when enough wait. The
/// view lets go of what it holds before.
pub fn retire_floating(window: &mut Window, cx: &mut App) {
    let handle = window.window_handle();
    let native = NativeSurface::of(window);
    let kept = &mut cx.default_global::<Spares>().kept;
    if kept.contains(&handle) {
        return;
    }
    match native {
        Ok(native) if kept.len() < SPARES_KEPT => {
            native.conceal();
            kept.push(handle);
        }
        _ => window.remove_window(),
    }
}

/// How far down from its place the panel of a floating window is while it
/// is `visibility` of the way open: it comes from the surface at `edge`.
/// Beside a surface at the left or the right it comes from the side, as
/// [`floating_offset`] says, and is not moved down.
pub fn floating_travel(edge: ScreenEdge, visibility: f32) -> Pixels {
    floating_offset(edge, visibility).y
}

/// Where the panel of a floating window is from its place while it is
/// `visibility` of the way open: it comes from the surface at `edge`.
pub fn floating_offset(edge: ScreenEdge, visibility: f32) -> gpui::Point<Pixels> {
    edge.travel(px(MARGIN * (1. - visibility)))
}

/// The content of a floating window: a themed panel, which `fill` gives
/// its children, inside the margin it moves through.
pub fn floating_panel(
    theme: &Theme,
    edge: ScreenEdge,
    visibility: f32,
    fill: impl FnOnce(Div) -> Div,
) -> Div {
    let offset = floating_offset(edge, visibility);
    let panel = div()
        .size_full()
        .relative()
        .left(offset.x)
        .top(offset.y)
        .p(theme.spacing.medium)
        .flex()
        .flex_col()
        .gap(theme.spacing.medium)
        .overflow_hidden()
        .rounded(theme.shape.large)
        .bg(theme.colors.surface_container);
    div()
        .size_full()
        .p(px(MARGIN))
        .font_family(theme.typography.family)
        .text_size(theme.typography.label_size)
        .text_color(theme.colors.on_surface)
        .opacity(visibility.clamp(0., 1.))
        .child(fill(panel))
}

/// The configured popups and the one that is open.
pub struct Popups {
    configs: HashMap<String, PopupConfig>,
    open: Option<(String, WindowHandle<PopupView>)>,
    closed: Option<(String, Instant)>,
    /// Where in a surface a widget stands that opens a popup by itself,
    /// rather than by a click on the whole of it.
    stands: HashMap<(AnyWindowHandle, String), Pixels>,
}

impl Global for Popups {}

impl Popups {
    pub fn new(configs: &[PopupConfig]) -> Self {
        let mut known = HashMap::new();
        for config in configs {
            if known.insert(config.name.clone(), config.clone()).is_some() {
                report!(
                    "several popups are named `{}`, the last one is used",
                    config.name
                );
            }
        }
        Self {
            configs: known,
            open: None,
            closed: None,
            stands: HashMap::new(),
        }
    }

    /// Remembers that the popup `name` is opened from `x` of the surface
    /// in `window`.
    pub fn stand(name: &str, x: Pixels, window: &Window, cx: &mut App) {
        let key = (window.window_handle(), name.to_owned());
        if cx.global::<Self>().stands.get(&key) != Some(&x) {
            cx.global_mut::<Self>().stands.insert(key, x);
        }
    }

    /// Where the popup `name` is opened from in the surface in `window`,
    /// when a widget there said so.
    pub fn standing(name: &str, window: &Window, cx: &App) -> Option<Pixels> {
        let key = (window.window_handle(), name.to_owned());
        cx.global::<Self>().stands.get(&key).copied()
    }

    /// Makes `configs` the configured popups without closing the open one,
    /// which keeps the content it was opened with.
    pub fn reconfigure(configs: &[PopupConfig], cx: &mut App) {
        let known = Self::new(configs).configs;
        cx.global_mut::<Self>().configs = known;
    }

    /// Opens the popup `name` at `anchor`, or closes it when it is open.
    /// Any other open popup closes.
    pub fn toggle(name: &str, anchor: PopupAnchor, cx: &mut App) {
        let open = cx.global::<Self>().open.as_ref();
        let reopened = open.is_some_and(|(open, _)| open == name);
        Self::close(cx);
        if reopened {
            return;
        }
        let this = cx.global::<Self>();
        if let Some((closed, at)) = &this.closed
            && closed == name
            && at.elapsed() < REOPEN_GUARD
        {
            return;
        }
        let known = this
            .configs
            .get(name)
            .or_else(|| cx.global::<WidgetRegistry>().popup(name));
        let Some(config) = known.cloned() else {
            report!("there is no popup named `{name}`");
            return;
        };
        match Self::open(&config, &anchor, cx) {
            Ok(window) => cx.global_mut::<Self>().open = Some((config.name, window)),
            Err(error) => report!("the popup `{name}` did not open: {error:#}"),
        }
    }

    /// The name of a popup that holds the widget `widget`: of the
    /// configured ones, else of those the modules bring, the first by
    /// name.
    pub fn holding(widget: &str, cx: &App) -> Option<String> {
        let holds = |config: &PopupConfig| {
            config
                .content
                .iter()
                .chain(&config.side)
                .any(|spec| spec.widget == widget)
        };
        let configured = { cx.try_global::<Self>() }
            .and_then(|this| {
                this.configs
                    .values()
                    .filter(|config| holds(config))
                    .map(|config| &config.name)
                    .min()
            })
            .cloned();
        configured.or_else(|| {
            let registry = cx.try_global::<WidgetRegistry>()?;
            let mut names = registry.popup_names();
            names.sort();
            names
                .into_iter()
                .find(|name| registry.popup(name).is_some_and(holds))
        })
    }

    /// Whether a popup is open.
    pub fn is_open(cx: &App) -> bool {
        cx.global::<Self>().open.is_some()
    }

    /// The name of the popup that is open, if one is, however it was
    /// opened; one that is closing is not.
    pub fn open_name(cx: &App) -> Option<&str> {
        let open = cx.global::<Self>().open.as_ref();
        open.map(|(name, _)| name.as_str())
    }

    /// Closes the open popup, if there is one.
    pub fn close(cx: &mut App) {
        if let Some((_, window)) = cx.global_mut::<Self>().open.take() {
            window.update(cx, |view, _, cx| view.close(cx)).ok();
        }
    }

    fn open(
        config: &PopupConfig,
        anchor: &PopupAnchor,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<PopupView>> {
        let frame = match config.place {
            PopupPlace::Surface => anchor.frame(config.width, config.height),
            PopupPlace::Center => anchor.middle(config.width, config.height),
            PopupPlace::Top => anchor.near_top(config.width, config.height),
        };
        let (place, screen) = (config.place, anchor.screen);
        let rough = Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(px(config.width), px(config.height)),
        };
        let (edge, display) = (anchor.edge, anchor.display);
        open_floating(rough, anchor.display_id, true, cx, |window, cx| {
            let adopted = NativeSurface::of(window).and_then(|native| {
                native.adopt_as_popup()?;
                native.place(frame)?;
                // The display behind a popup in its middle is dimmed.
                let veil = match place {
                    PopupPlace::Surface => None,
                    PopupPlace::Center => Some(Veil::open(screen, native)?),
                    PopupPlace::Top => None,
                };
                native.reveal(true);
                Ok(veil)
            });
            let veil = adopted.unwrap_or_else(|error| {
                report!("a popup is not where it belongs: {error:#}");
                None
            });
            let mut built = |specs: &[WidgetSpec]| -> Vec<AnyView> {
                specs
                    .iter()
                    .filter_map(|spec| WidgetRegistry::build(spec, display, cx))
                    .collect()
            };
            let (content, side) = (built(&config.content), built(&config.side));
            let centered = place == PopupPlace::Center;
            let centered = centered || place == PopupPlace::Top;
            let mut view = PopupView::new(config.name.clone(), edge, content, window, cx);
            (view.centered, view.veil) = (centered, veil);
            view.side = side;
            view
        })
    }
}

/// The root view of a popup window: a themed panel around stacked widgets.
pub struct PopupView {
    name: String,
    /// The edge the popup's surface is docked to; it moves in from there.
    edge: ScreenEdge,
    /// Whether the popup stands in the middle of the display and rises
    /// into place, instead of coming out of its surface.
    centered: bool,
    /// What dims the display behind such a popup.
    veil: Option<Veil>,
    content: Vec<AnyView>,
    /// What stands in a second column, to the right of the content.
    side: Vec<AnyView>,
    /// 0 while hidden, 1 when fully open.
    visibility: Spring,
    closing: bool,
    /// Whether the window has had keyboard focus; before that, not having
    /// it does not mean the user turned away.
    was_active: bool,
    _activation: Subscription,
}

impl PopupView {
    fn new(
        name: String,
        edge: ScreenEdge,
        content: Vec<AnyView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut visibility = Spring::new(cx.global::<Theme>().motion.spatial, 0.);
        visibility.animate_to(1.);
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.was_active = true;
            } else if this.was_active {
                this.close(cx);
            }
        });
        Self {
            name,
            edge,
            centered: false,
            veil: None,
            content,
            side: Vec::new(),
            visibility,
            closing: false,
            was_active: false,
            _activation: activation,
        }
    }

    /// Starts closing; the window goes away once the motion has finished.
    fn close(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.visibility.animate_to(0.);
        let popups = cx.global_mut::<Popups>();
        popups.closed = Some((self.name.clone(), Instant::now()));
        if popups
            .open
            .as_ref()
            .is_some_and(|(open, _)| *open == self.name)
        {
            popups.open = None;
        }
        cx.notify();
    }
}

impl Render for PopupView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window);
        if self.closing && self.visibility.is_settled() {
            // What the popup showed goes now, not when the window is
            // opened again.
            self.veil = None;
            self.content.clear();
            self.side.clear();
            retire_floating(window, cx);
        }
        if let Some(veil) = &self.veil {
            veil.darken(VEIL * visibility);
        }
        floating_panel(&theme, self.edge, visibility, |panel| {
            let panel = if self.side.is_empty() {
                // What does not fit under one another is scrolled to.
                panel.child(
                    div()
                        .id("popup-content")
                        .size_full()
                        .flex()
                        .flex_col()
                        .gap(theme.spacing.medium)
                        .overflow_y_scroll()
                        .children(self.content.iter().cloned()),
                )
            } else {
                let column = |widgets: &[AnyView]| {
                    div()
                        .h_full()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(theme.spacing.medium)
                        .children(widgets.iter().cloned())
                };
                panel
                    .flex_row()
                    .child(column(&self.content))
                    .child(column(&self.side))
            };
            match self.centered {
                true => panel.top(px(RISE * (1. - visibility))),
                false => panel,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1000,
        bottom: 800,
    };

    fn anchor(edge: ScreenEdge, surface: ScreenRect, x: i32) -> PopupAnchor {
        PopupAnchor::new(surface, edge, x, SCREEN, 1., None, None)
    }

    fn sides(rect: ScreenRect) -> (i32, i32, i32, i32) {
        (rect.left, rect.top, rect.right, rect.bottom)
    }

    /// `anchor` moved to `y` along a bar at the left or the right.
    fn down(anchor: PopupAnchor, y: i32) -> PopupAnchor {
        PopupAnchor { y, ..anchor }
    }

    #[test]
    fn a_popup_opens_beside_a_bar_at_the_left_or_the_right() {
        let left = ScreenRect {
            left: 0,
            top: 0,
            right: 48,
            bottom: 800,
        };
        // To the right of a left bar, at the middle of the widget.
        let beside = framed(&down(anchor(ScreenEdge::Left, left, 24), 300));
        assert_eq!(sides(beside), (48, 250, 248, 350));
        // Kept on the display at its ends.
        let low = framed(&down(anchor(ScreenEdge::Left, left, 24), 790));
        assert_eq!((low.top, low.bottom), (700, 800));
        let high = framed(&down(anchor(ScreenEdge::Left, left, 24), 5));
        assert_eq!(high.top, 0);
        // To the left of a right bar.
        let right = ScreenRect {
            left: 952,
            top: 0,
            right: 1000,
            bottom: 800,
        };
        let beside = framed(&down(anchor(ScreenEdge::Right, right, 976), 400));
        assert_eq!(sides(beside), (752, 350, 952, 450));
        // A new anchor stands at the middle of a vertical bar.
        assert_eq!(anchor(ScreenEdge::Left, left, 24).y, 400);
    }

    #[test]
    fn a_popup_near_the_top_keeps_clear_of_a_bar_at_the_side() {
        let left = ScreenRect {
            left: 0,
            top: 0,
            right: 200,
            bottom: 800,
        };
        let frame = anchor(ScreenEdge::Left, left, 100).near_top(500., 300.);
        let margin = (MARGIN * 2.).round() as i32;
        assert_eq!(frame.left + frame.right, 200 + 1000);
        assert_eq!(frame.right - frame.left, 500 + margin);
        // Where the room beside it is too narrow, it stands in the middle.
        let wide = anchor(ScreenEdge::Left, left, 100).near_top(900., 300.);
        assert_eq!(wide.left + wide.right, 1000);
    }

    #[test]
    fn the_edge_of_a_surface_is_judged_by_its_shape() {
        let strip = |left, top, right, bottom| ScreenRect {
            left,
            top,
            right,
            bottom,
        };
        assert_eq!(nearest_edge(strip(0, 0, 1000, 40), SCREEN), ScreenEdge::Top);
        assert_eq!(
            nearest_edge(strip(0, 760, 1000, 800), SCREEN),
            ScreenEdge::Bottom
        );
        assert_eq!(nearest_edge(strip(0, 0, 40, 800), SCREEN), ScreenEdge::Left);
        assert_eq!(
            nearest_edge(strip(960, 100, 1000, 700), SCREEN),
            ScreenEdge::Right
        );
    }

    #[test]
    fn a_panel_comes_in_from_its_edge() {
        let half = floating_offset(ScreenEdge::Left, 0.5);
        assert_eq!(half, gpui::point(px(-MARGIN / 2.), px(0.)));
        assert_eq!(floating_travel(ScreenEdge::Left, 0.5), px(0.));
        assert_eq!(floating_travel(ScreenEdge::Top, 0.), px(-MARGIN));
        assert_eq!(
            floating_offset(ScreenEdge::Right, 1.),
            gpui::point(px(0.), px(0.))
        );
    }

    fn framed(anchor: &PopupAnchor) -> ScreenRect {
        anchor.frame(176., 76.)
    }

    #[test]
    fn a_popup_opens_away_from_its_surface_without_covering_it() {
        let bar = ScreenRect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 48,
        };
        let below = framed(&anchor(ScreenEdge::Top, bar, 500));
        assert_eq!(
            (below.left, below.top, below.right, below.bottom),
            (400, 48, 600, 148)
        );

        let dock = ScreenRect {
            left: 0,
            top: 724,
            right: 1000,
            bottom: 800,
        };
        let above = framed(&anchor(ScreenEdge::Bottom, dock, 500));
        assert_eq!((above.top, above.bottom), (624, 724));
    }

    #[test]
    fn a_popup_stays_on_its_display() {
        let bar = ScreenRect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 48,
        };
        let frame = framed(&anchor(ScreenEdge::Top, bar, 990));
        assert_eq!((frame.left, frame.right), (800, 1000));
        let frame = framed(&anchor(ScreenEdge::Top, bar, 3));
        assert_eq!(frame.left, 0);
    }

    #[test]
    fn a_popup_near_the_top_stands_in_the_middle_of_the_width() {
        let bar = ScreenRect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 48,
        };
        let frame = anchor(ScreenEdge::Top, bar, 100).near_top(600., 300.);
        let margin = (MARGIN * 2.).round() as i32;
        assert_eq!(frame.left + frame.right, 1000);
        assert_eq!(frame.right - frame.left, 600 + margin);
        assert_eq!(frame.top, 800 / 6);
        assert_eq!(frame.bottom - frame.top, 300 + margin);
        // Where it does not fit below that, it rises.
        let tall = anchor(ScreenEdge::Top, bar, 100).near_top(600., 760.);
        assert_eq!(tall.bottom, 800);
    }
}
