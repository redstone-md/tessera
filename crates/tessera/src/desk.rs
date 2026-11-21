// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Cards: panels of widgets that lie on the desktop, below every window.

use std::time::Duration;

use gpui::{
    AnyView, App, AppContext, AsyncApp, Bounds, Context, IntoElement, ParentElement, Pixels,
    Render, Window, WindowHandle, WindowOptions, px, size,
};
use serde::Deserialize;

use crate::config::{Displays, WidgetSpec};
use crate::module::WidgetRegistry;
use crate::motion::Spring;
use crate::platform::{NativeSurface, ScreenEdge, ScreenRect};
use crate::popup::{MARGIN, floating_panel, floating_window};
use crate::theme::Theme;
use crate::ui::release_hover;

/// How often a card looks whether the room windows may use has changed, as
/// it does when a surface gets another thickness.
const PLACE_CHECK: Duration = Duration::from_secs(1);

/// The part of the display a card is put at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

impl Corner {
    pub const ALL: [Self; 5] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
        Self::Center,
    ];

    /// How the configuration writes the corner.
    pub fn name(self) -> &'static str {
        match self {
            Self::TopLeft => "top-left",
            Self::TopRight => "top-right",
            Self::BottomLeft => "bottom-left",
            Self::BottomRight => "bottom-right",
            Self::Center => "center",
        }
    }

    /// What the settings call the corner.
    pub fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "Top left",
            Self::TopRight => "Top right",
            Self::BottomLeft => "Bottom left",
            Self::BottomRight => "Bottom right",
            Self::Center => "In the center",
        }
    }
}

/// A panel of widgets on the desktop.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardConfig {
    #[serde(default)]
    pub corner: Corner,
    /// How far the panel is from the two sides of its corner, in logical
    /// pixels. In the center the two shift it right and down.
    #[serde(default = "CardConfig::default_distance")]
    pub x: f32,
    #[serde(default = "CardConfig::default_distance")]
    pub y: f32,
    /// Size of the panel, in logical pixels.
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub displays: Displays,
    /// Widgets, stacked from the top.
    #[serde(default)]
    pub content: Vec<WidgetSpec>,
}

impl CardConfig {
    fn default_distance() -> f32 {
        24.
    }

    /// The frame of the window of the card in `room`, the part of a display
    /// that windows may use, where a logical pixel is `scale` physical
    /// ones. The window is one margin larger than the panel on every side.
    fn frame(&self, room: ScreenRect, scale: f32) -> ScreenRect {
        let physical = |length: f32| (length * scale).round() as i32;
        let width = physical(self.width + MARGIN * 2.);
        let height = physical(self.height + MARGIN * 2.);
        let (x, y) = (physical(self.x - MARGIN), physical(self.y - MARGIN));
        let (left, top) = match self.corner {
            Corner::TopLeft => (room.left + x, room.top + y),
            Corner::TopRight => (room.right - x - width, room.top + y),
            Corner::BottomLeft => (room.left + x, room.bottom - y - height),
            Corner::BottomRight => (room.right - x - width, room.bottom - y - height),
            Corner::Center => (
                (room.left + room.right - width) / 2 + physical(self.x),
                (room.top + room.bottom - height) / 2 + physical(self.y),
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

/// The cards that are open.
#[derive(Default)]
pub struct Cards {
    open: Vec<WindowHandle<CardView>>,
}

impl Cards {
    /// Closes the open cards and opens those of `configs` on the displays
    /// each one asks for. One that does not open is reported and left out.
    pub fn show(&mut self, configs: &[CardConfig], cx: &mut App) {
        for card in self.open.drain(..) {
            card.update(cx, |_, window, _| window.remove_window()).ok();
        }
        let primary = cx.primary_display().map(|display| display.id());
        for display in cx.displays() {
            for config in configs {
                if config.displays == Displays::Primary && Some(display.id()) != primary {
                    continue;
                }
                // The window opens at the size of its panel: one as large
                // as the display would be drawn into once before it is
                // placed, which takes memory for that long.
                let rough = Bounds {
                    origin: display.bounds().origin,
                    size: size(px(config.width), px(config.height)),
                };
                // It is shown once it is a card, without taking the
                // keyboard. A window that is not shown is not put at its
                // bounds either, so the card does that itself.
                let options = WindowOptions {
                    show: false,
                    ..floating_window(rough, Some(display.id()), false)
                };
                let opened = cx.open_window(options, |window, cx| {
                    let scale = window.scale_factor();
                    let native = NativeSurface::of(window)
                        .and_then(|native| {
                            native.place(physical(rough, scale))?;
                            native.adopt_as_card().map(|()| native)
                        })
                        .inspect_err(|error| report!("a card is not on the desktop: {error:#}"))
                        .ok();
                    let key = native.map(NativeSurface::display);
                    let content = config
                        .content
                        .iter()
                        .filter_map(|spec| WidgetRegistry::build(spec, key, cx))
                        .collect();
                    cx.new(|cx| CardView::new(config.clone(), native, scale, content, cx))
                });
                match opened {
                    Ok(card) => self.open.push(card),
                    Err(error) => report!("a card did not open: {error:#}"),
                }
            }
        }
    }
}

/// The bounds of a window in physical pixels, where a logical pixel is
/// `scale` of them.
fn physical(bounds: Bounds<Pixels>, scale: f32) -> ScreenRect {
    let physical = |length: Pixels| (f32::from(length) * scale).round() as i32;
    let (left, top) = (physical(bounds.origin.x), physical(bounds.origin.y));
    ScreenRect {
        left,
        top,
        right: left + physical(bounds.size.width),
        bottom: top + physical(bounds.size.height),
    }
}

/// The root view of a card: a themed panel around stacked widgets.
pub struct CardView {
    content: Vec<AnyView>,
    /// 0 while hidden, 1 when fully shown.
    visibility: Spring,
}

impl CardView {
    fn new(
        config: CardConfig,
        native: Option<NativeSurface>,
        scale: f32,
        content: Vec<AnyView>,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(native) = native {
            Self::keep_placed(config, native, scale, cx);
        }
        let mut visibility = Spring::new(cx.global::<Theme>().motion.spatial, 0.);
        visibility.animate_to(1.);
        Self {
            content,
            visibility,
        }
    }

    /// Puts the window where `config` says, now and whenever the room on
    /// its display changes, until the view `cx` makes is gone.
    fn keep_placed(config: CardConfig, native: NativeSurface, scale: f32, cx: &mut Context<Self>) {
        cx.spawn(async move |view, cx: &mut AsyncApp| {
            let mut placed = None;
            while view.upgrade().is_some() {
                let room = native.display().work_area();
                if let Some(room) = room.filter(|_| room != placed) {
                    placed = Some(room);
                    if let Err(error) = native.place(config.frame(room, scale)) {
                        report!("a card is not where it belongs: {error:#}");
                    }
                }
                cx.background_executor().timer(PLACE_CHECK).await;
            }
        })
        .detach();
    }
}

impl Render for CardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window);
        floating_panel(&theme, ScreenEdge::Top, visibility, |panel| {
            panel.children(self.content.iter().cloned())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: ScreenRect = ScreenRect {
        left: 1000,
        top: 40,
        right: 3000,
        bottom: 1040,
    };

    fn card(corner: Corner) -> CardConfig {
        CardConfig {
            corner,
            x: 24.,
            y: 36.,
            width: 300.,
            height: 200.,
            displays: Displays::All,
            content: Vec::new(),
        }
    }

    fn sides(frame: ScreenRect) -> (i32, i32, i32, i32) {
        (frame.left, frame.top, frame.right, frame.bottom)
    }

    #[test]
    fn the_panel_of_a_card_keeps_its_distance_from_its_corner() {
        // The window is 12 larger than the panel on every side.
        let frame = card(Corner::TopLeft).frame(ROOM, 1.);
        assert_eq!(sides(frame), (1012, 64, 1336, 288));
        let frame = card(Corner::TopRight).frame(ROOM, 1.);
        assert_eq!(sides(frame), (2664, 64, 2988, 288));
        let frame = card(Corner::BottomLeft).frame(ROOM, 1.);
        assert_eq!(sides(frame), (1012, 792, 1336, 1016));
        let frame = card(Corner::BottomRight).frame(ROOM, 1.);
        assert_eq!(sides(frame), (2664, 792, 2988, 1016));
    }

    #[test]
    fn a_card_in_the_center_is_shifted_by_its_distances() {
        let frame = card(Corner::Center).frame(ROOM, 1.);
        assert_eq!(sides(frame), (1862, 464, 2186, 688));
    }

    #[test]
    fn a_card_is_measured_in_the_pixels_of_its_display() {
        let frame = card(Corner::TopLeft).frame(ROOM, 1.5);
        assert_eq!(sides(frame), (1018, 76, 1504, 412));
    }

    #[test]
    fn the_bounds_of_a_window_are_measured_in_physical_pixels() {
        let bounds = Bounds {
            origin: gpui::point(px(-100.), px(20.)),
            size: size(px(260.), px(150.)),
        };
        assert_eq!(sides(physical(bounds, 1.5)), (-150, 30, 240, 255));
    }

    #[test]
    fn a_corner_is_read_by_the_name_it_is_written_with() {
        for corner in Corner::ALL {
            let text = format!("corner = \"{}\"\nwidth = 1\nheight = 1", corner.name());
            assert_eq!(toml::from_str::<CardConfig>(&text).unwrap().corner, corner);
        }
    }

    #[test]
    fn a_card_is_read_from_the_configuration() {
        let card: CardConfig = toml::from_str(
            "corner = \"bottom-left\"\nwidth = 240\nheight = 120\ncontent = [\"clock\"]",
        )
        .unwrap();
        assert_eq!(card.corner, Corner::BottomLeft);
        assert_eq!((card.x, card.y), (24., 24.));
        assert_eq!(card.content[0].widget, "clock");
        assert!(toml::from_str::<CardConfig>("width = 1\nheight = 1\nsize = 2").is_err());
    }
}
