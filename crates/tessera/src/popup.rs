// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Popups: floating panels of widgets that open next to a surface.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    AnyView, App, AppContext, Bounds, Context, DisplayId, Div, Global, IntoElement, ParentElement,
    Pixels, Render, Styled, Subscription, TitlebarOptions, Window, WindowBackgroundAppearance,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, div, point, px, size,
};

use crate::config::PopupConfig;
use crate::module::WidgetRegistry;
use crate::motion::Spring;
use crate::platform::{DisplayKey, NativeSurface, ScreenEdge, ScreenRect};
use crate::theme::Theme;
use crate::ui::release_hover;

/// Transparent space around the panel of a floating window, which its
/// motion moves through.
pub const MARGIN: f32 = 12.;

/// How long after a popup closes a request to open it again is ignored. The
/// click that asks to close an open popup first takes focus from it, which
/// closes it; without this the same click would then reopen it.
const REOPEN_GUARD: Duration = Duration::from_millis(250);

/// Where a floating window should appear. Positions are in physical pixels.
#[derive(Clone, Copy)]
pub struct PopupAnchor {
    /// The surface the popup belongs to.
    pub surface: ScreenRect,
    /// The edge that surface is docked to; the popup opens away from it.
    pub edge: ScreenEdge,
    /// The horizontal screen position to center the popup on.
    pub x: i32,
    /// The display to stay on.
    pub screen: ScreenRect,
    /// Physical pixels per logical one on that display.
    pub scale: f32,
    pub display_id: Option<DisplayId>,
    pub display: Option<DisplayKey>,
}

impl PopupAnchor {
    /// The anchor at `x`, a horizontal position in `window`, which is a
    /// surface docked to the edge of its display that it is nearer to.
    pub fn at(window: &Window, x: Pixels, cx: &App) -> anyhow::Result<Self> {
        let native = NativeSurface::of(window)?;
        let (surface, screen) = (native.frame()?, native.display_frame()?);
        let scale = window.scale_factor();
        let edge = if surface.top + surface.bottom < screen.top + screen.bottom {
            ScreenEdge::Top
        } else {
            ScreenEdge::Bottom
        };
        Ok(Self {
            surface,
            edge,
            x: surface.left + (x / px(1.) * scale).round() as i32,
            screen,
            scale,
            display_id: window.display(cx).map(|display| display.id()),
            display: Some(native.display()),
        })
    }

    /// The frame of a floating window whose panel is `width` by `height`
    /// logical pixels: touching the surface, so the panel sits one margin
    /// away from it, centered on the anchor and kept on the display.
    pub fn frame(&self, width: f32, height: f32) -> ScreenRect {
        let physical = |length: f32| ((length + MARGIN * 2.) * self.scale).round() as i32;
        let (width, height) = (physical(width), physical(height));
        let rightmost = (self.screen.right - width).max(self.screen.left);
        let left = (self.x - width / 2).clamp(self.screen.left, rightmost);
        let top = match self.edge {
            ScreenEdge::Top => self.surface.bottom,
            ScreenEdge::Bottom => self.surface.top - height,
        };
        ScreenRect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }
}

/// The options of a floating window on the display `display_id`. GPUI sizes
/// the window for a frame that adopting it removes, so `rough` only picks
/// the display, and the window is to be placed once it is open. `focus`
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
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    }
}

/// How far from its place the panel of a floating window is while it is
/// `visibility` of the way open: it comes from the surface at `edge`.
pub fn floating_travel(edge: ScreenEdge, visibility: f32) -> Pixels {
    let travel = px(MARGIN * (1. - visibility));
    match edge {
        ScreenEdge::Top => -travel,
        ScreenEdge::Bottom => travel,
    }
}

/// The content of a floating window: a themed panel, which `fill` gives
/// its children, inside the margin it moves through.
pub fn floating_panel(
    theme: &Theme,
    edge: ScreenEdge,
    visibility: f32,
    fill: impl FnOnce(Div) -> Div,
) -> Div {
    let panel = div()
        .size_full()
        .relative()
        .top(floating_travel(edge, visibility))
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
}

impl Global for Popups {}

impl Popups {
    pub fn new(configs: &[PopupConfig]) -> Self {
        let mut known = HashMap::new();
        for config in configs {
            if known.insert(config.name.clone(), config.clone()).is_some() {
                eprintln!(
                    "tessera: several popups are named `{}`, the last one is used",
                    config.name
                );
            }
        }
        Self {
            configs: known,
            open: None,
            closed: None,
        }
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
            eprintln!("tessera: there is no popup named `{name}`");
            return;
        };
        match Self::open(&config, &anchor, cx) {
            Ok(window) => cx.global_mut::<Self>().open = Some((config.name, window)),
            Err(error) => eprintln!("tessera: the popup `{name}` did not open: {error:#}"),
        }
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
        let frame = anchor.frame(config.width, config.height);
        let rough = Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(px(config.width), px(config.height)),
        };
        let options = floating_window(rough, anchor.display_id, true);
        let (edge, display) = (anchor.edge, anchor.display);
        cx.open_window(options, |window, cx| {
            let adopted = NativeSurface::of(window).and_then(|native| {
                native.adopt_as_popup()?;
                native.place(frame)?;
                native.activate();
                Ok(())
            });
            if let Err(error) = adopted {
                eprintln!("tessera: a popup is not where it belongs: {error:#}");
            }
            let content = config
                .content
                .iter()
                .filter_map(|spec| WidgetRegistry::build(spec, display, cx))
                .collect();
            cx.new(|cx| PopupView::new(config.name.clone(), edge, content, window, cx))
        })
    }
}

/// The root view of a popup window: a themed panel around stacked widgets.
pub struct PopupView {
    name: String,
    /// The edge the popup's surface is docked to; it moves in from there.
    edge: ScreenEdge,
    content: Vec<AnyView>,
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
            content,
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
            window.remove_window();
        }
        floating_panel(&theme, self.edge, visibility, |panel| {
            panel.children(self.content.iter().cloned())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(edge: ScreenEdge, surface: ScreenRect, x: i32) -> PopupAnchor {
        PopupAnchor {
            surface,
            edge,
            x,
            screen: ScreenRect {
                left: 0,
                top: 0,
                right: 1000,
                bottom: 800,
            },
            scale: 1.,
            display_id: None,
            display: None,
        }
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
}
