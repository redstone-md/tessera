// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Cards: panels of widgets that lie on the desktop, below every window.

use std::time::{Duration, Instant};

use gpui::{
    AnyView, App, AppContext, AsyncApp, Bounds, Context, CursorStyle, Div, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Render, Stateful, Styled,
    Window, WindowHandle, div, px, size,
};
use serde::Deserialize;

use crate::config::{Displays, WidgetSpec};
use crate::module::WidgetRegistry;
use crate::motion::Spring;
use crate::panel::{Arranging, BREATH, HANDLE_FILL, OUTLINE_REST, OUTLINE_SWING, rewrite};
use crate::platform::{
    NativeSurface, ScreenEdge, ScreenRect, SmallBirth, pointer_at, pointer_held,
};
use crate::popup::{MARGIN, floating_panel, floating_window};
use crate::theme::Theme;
use crate::ui::release_hover;

/// How often a card looks whether the room windows may use has changed, as
/// it does when a surface gets another thickness.
const PLACE_CHECK: Duration = Duration::from_secs(1);

/// How often a card that is held looks where the pointer is.
const FOLLOW: Duration = Duration::from_millis(6);
/// How much stronger the handle of a card is while it is held.
const HELD: f32 = 2.;

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

    /// The corner and the two distances with which [`Self::frame`] gives
    /// `frame` in `room`: the corner is the one nearest to the middle of
    /// the frame, the distances whole logical pixels, none below zero.
    fn placed(frame: ScreenRect, room: ScreenRect, scale: f32) -> (Corner, f32, f32) {
        let logical = |length: i32| (length as f32 / scale + MARGIN).round().max(0.);
        let left = frame.left + frame.right < room.left + room.right;
        let top = frame.top + frame.bottom < room.top + room.bottom;
        let corner = match (left, top) {
            (true, true) => Corner::TopLeft,
            (false, true) => Corner::TopRight,
            (true, false) => Corner::BottomLeft,
            (false, false) => Corner::BottomRight,
        };
        let x = if left {
            frame.left - room.left
        } else {
            room.right - frame.right
        };
        let y = if top {
            frame.top - room.top
        } else {
            room.bottom - frame.bottom
        };
        (corner, logical(x), logical(y))
    }
}

/// Where a card was taken hold of.
#[derive(Clone, Copy)]
struct Grip {
    pointer: (i32, i32),
    frame: ScreenRect,
    /// The room the card stays in while it is held.
    room: ScreenRect,
}

impl Grip {
    /// The frame of the card when the pointer is at `pointer`. It does not
    /// leave the room; a card larger than the room keeps its left and top
    /// sides in it.
    fn follows(self, pointer: (i32, i32)) -> ScreenRect {
        let along = |moved: i32, low: i32, high: i32, least: i32, most: i32| {
            moved.min(most - high).max(least - low)
        };
        let Self { frame, room, .. } = self;
        let x = along(
            pointer.0 - self.pointer.0,
            frame.left,
            frame.right,
            room.left,
            room.right,
        );
        let y = along(
            pointer.1 - self.pointer.1,
            frame.top,
            frame.bottom,
            room.top,
            room.bottom,
        );
        ScreenRect {
            left: frame.left + x,
            top: frame.top + y,
            right: frame.right + x,
            bottom: frame.bottom + y,
        }
    }
}

/// The cards that are open.
#[derive(Default)]
pub struct Cards {
    /// Each open card with the place of its configuration among the cards.
    open: Vec<(usize, WindowHandle<CardView>)>,
    /// What the open cards were opened for.
    shown: Vec<CardConfig>,
}

impl Cards {
    /// Closes the open cards and opens those of `configs` on the displays
    /// each one asks for. One that does not open is reported and left out.
    ///
    /// Cards that were only put elsewhere by hand keep their windows.
    pub fn show(&mut self, configs: &[CardConfig], cx: &mut App) {
        if self.moved_by_hand(configs, cx) {
            for (index, card) in &self.open {
                let config = configs[*index].clone();
                card.update(cx, |card, _, cx| card.put(config, cx)).ok();
            }
            self.shown = configs.to_vec();
            return;
        }
        for (_, card) in self.open.drain(..) {
            card.update(cx, |_, window, _| window.remove_window()).ok();
        }
        self.shown = configs.to_vec();
        let primary = cx.primary_display().map(|display| display.id());
        for display in cx.displays() {
            for (index, config) in configs.iter().enumerate() {
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
                let options = floating_window(rough, Some(display.id()), false);
                let _small = SmallBirth::begin();
                let opened = cx.open_window(options, |window, cx| {
                    let scale = window.scale_factor();
                    let native = NativeSurface::of(window)
                        .and_then(|native| {
                            native.place(physical(rough, scale))?;
                            native.adopt_as_card()?;
                            native.reveal(false);
                            Ok(native)
                        })
                        .inspect_err(|error| report!("a card is not on the desktop: {error:#}"))
                        .ok();
                    let key = native.map(NativeSurface::display);
                    let content = config
                        .content
                        .iter()
                        .filter_map(|spec| WidgetRegistry::build(spec, key, cx))
                        .collect();
                    cx.new(|cx| CardView::new(index, config.clone(), native, scale, content, cx))
                });
                match opened {
                    Ok(card) => self.open.push((index, card)),
                    Err(error) => report!("a card did not open: {error:#}"),
                }
            }
        }
    }

    /// Whether `configs` differ from what is shown only by where an open
    /// card has been dragged to.
    fn moved_by_hand(&self, configs: &[CardConfig], cx: &App) -> bool {
        let dragged_to = |index: usize, config: &CardConfig| {
            self.open.iter().any(|(at, card)| {
                *at == index && card.read(cx).is_ok_and(|card| card.config == *config)
            })
        };
        configs.len() == self.shown.len()
            && configs != self.shown
            && configs
                .iter()
                .zip(&self.shown)
                .enumerate()
                .all(|(index, (config, shown))| config == shown || dragged_to(index, config))
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
    /// The place of the configuration among the cards.
    index: usize,
    config: CardConfig,
    native: Option<NativeSurface>,
    scale: f32,
    content: Vec<AnyView>,
    /// 0 while hidden, 1 when fully shown.
    visibility: Spring,
    /// The room the window was last placed in.
    room: Option<ScreenRect>,
    /// Whether the card is being dragged.
    held: bool,
    /// How far the handle that arranging gives the card has come in.
    chrome: Spring,
    began: Instant,
}

impl CardView {
    fn new(
        index: usize,
        config: CardConfig,
        native: Option<NativeSurface>,
        scale: f32,
        content: Vec<AnyView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let motion = cx.global::<Theme>().motion;
        let mut visibility = Spring::new(motion.spatial, 0.);
        visibility.animate_to(1.);
        Self::keep_placed(cx);
        Self {
            index,
            config,
            native,
            scale,
            content,
            visibility,
            room: None,
            held: false,
            chrome: Spring::new(motion.effects, 0.),
            began: Instant::now(),
        }
    }

    /// Puts the window where the configuration says, now and whenever the
    /// room on its display changes, until the view `cx` makes is gone.
    fn keep_placed(cx: &mut Context<Self>) {
        cx.spawn(async move |view, cx: &mut AsyncApp| {
            while view.update(cx, |card, _| card.settle()).is_ok() {
                cx.background_executor().timer(PLACE_CHECK).await;
            }
        })
        .detach();
    }

    /// Places the window if the room on its display is not the one it was
    /// last placed in. A card that is held stays with the pointer.
    fn settle(&mut self) {
        let Some(native) = self.native.filter(|_| !self.held) else {
            return;
        };
        // Above the icons of the desktop, which may have come or gone.
        native.rest_on_desktop();
        let room = native.display().work_area();
        if let Some(room) = room.filter(|_| room != self.room) {
            self.room = Some(room);
            if let Err(error) = native.place(self.config.frame(room, self.scale)) {
                report!("a card is not where it belongs: {error:#}");
            }
        }
    }

    /// Makes `config` the configuration of the card and places it anew.
    fn put(&mut self, config: CardConfig, cx: &mut Context<Self>) {
        if self.config != config {
            self.config = config;
            self.room = None;
        }
        self.settle();
        cx.notify();
    }

    /// Lets the window follow the pointer until its button is let go, and
    /// then writes the corner and the distances it was left at.
    fn take_hold(&mut self, cx: &mut Context<Self>) {
        let Some(native) = self.native.filter(|_| !self.held) else {
            return;
        };
        let (Some(pointer), Ok(frame), Some(room)) =
            (pointer_at(), native.frame(), native.display().work_area())
        else {
            return;
        };
        let grip = Grip {
            pointer,
            frame,
            room,
        };
        self.held = true;
        cx.notify();
        cx.spawn(async move |view, cx: &mut AsyncApp| {
            while pointer_held() {
                if let Some(pointer) = pointer_at() {
                    native.place(grip.follows(pointer)).ok();
                }
                cx.background_executor().timer(FOLLOW).await;
            }
            let left_at = native.frame().unwrap_or(grip.frame);
            let moved = view.update(cx, |card, cx| {
                card.held = false;
                cx.notify();
                let (corner, x, y) = CardConfig::placed(left_at, grip.room, card.scale);
                let moved = (corner, x, y) != (card.config.corner, card.config.x, card.config.y);
                (card.config.corner, card.config.x, card.config.y) = (corner, x, y);
                card.room = None;
                card.settle();
                moved.then_some((card.index, corner, x, y))
            });
            let Ok(Some((index, corner, x, y))) = moved else {
                return;
            };
            cx.update(|cx| {
                rewrite(cx, |file| {
                    file.set_card(index, "corner", Some(corner.name().into()))
                        && file.set_card(index, "x", Some((x as i64).into()))
                        && file.set_card(index, "y", Some((y as i64).into()))
                });
            })
            .ok();
        })
        .detach();
    }

    /// What lies over the card while the widgets are arranged: an outline
    /// that breathes, by which the card is dragged.
    fn handle(&self, chrome: f32, theme: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let breath = self.began.elapsed().as_secs_f32() * BREATH;
        let held = if self.held { HELD } else { 1. };
        let strength = (OUTLINE_REST + OUTLINE_SWING * (0.5 + 0.5 * breath.sin())) * chrome * held;
        let color = theme.colors.primary;
        div()
            .id("card-handle")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .rounded(theme.shape.large)
            .border_1()
            .border_color(color.opacity(strength.clamp(0., 1.)))
            .bg(color.opacity((strength * HANDLE_FILL).clamp(0., 1.)))
            .cursor(if self.held {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|card, _: &MouseDownEvent, _, cx| card.take_hold(cx)),
            )
    }
}

impl Render for CardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window);
        let arranging = Arranging::is_on(cx);
        if (self.chrome.target() > 0.5) != arranging {
            self.chrome.animate_to(if arranging { 1. } else { 0. });
        }
        let chrome = self.chrome.value(window);
        if arranging {
            // The outline breathes for as long as the card can be dragged.
            crate::platform::Pace::quicken();
            window.request_animation_frame();
        }
        let handle = (chrome > 0.).then(|| self.handle(chrome, &theme, cx));
        floating_panel(&theme, ScreenEdge::Top, visibility, |panel| {
            panel
                .children(self.content.iter().cloned())
                .children(handle)
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
    fn a_frame_gives_back_the_corner_and_the_distances_it_was_made_with() {
        for corner in [
            Corner::TopLeft,
            Corner::TopRight,
            Corner::BottomLeft,
            Corner::BottomRight,
        ] {
            for scale in [1., 1.5] {
                let frame = card(corner).frame(ROOM, scale);
                assert_eq!(CardConfig::placed(frame, ROOM, scale), (corner, 24., 36.));
            }
        }
        // One left outside the room is as near to its corner as zero.
        let outside = ScreenRect {
            left: 900,
            top: 0,
            right: 1224,
            bottom: 224,
        };
        assert_eq!(
            CardConfig::placed(outside, ROOM, 1.),
            (Corner::TopLeft, 0., 0.)
        );
    }

    #[test]
    fn a_card_that_is_held_stays_in_the_room() {
        let grip = Grip {
            pointer: (1100, 100),
            frame: card(Corner::TopLeft).frame(ROOM, 1.),
            room: ROOM,
        };
        assert_eq!(sides(grip.follows((1150, 90))), (1062, 54, 1386, 278));
        assert_eq!(sides(grip.follows((0, 0))), (1000, 40, 1324, 264));
        assert_eq!(sides(grip.follows((9000, 9000))), (2676, 816, 3000, 1040));
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
