// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The snap layouts: small pictures of ways to divide a display, each
//! zone of which puts the window they were opened for there once it is
//! clicked. They open under the right end of the title bar of the window
//! in front for the action `snap-layouts`, taking the keyboard so that Esc
//! closes them, and at the pointer when it rests on the maximize button of
//! a window; then they leave the keyboard where it is and close once the
//! pointer has moved away from them.

use std::time::{Duration, Instant};

use gpui::{
    App, AsyncApp, Context, CursorStyle, Div, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Subscription,
    WeakEntity, Window, WindowHandle, div, px,
};

use super::{Kind, Snapper, Zone, anchor_at, open_at};
use crate::motion::Spring;
use crate::platform::{AppWindowId, Display, ScreenEdge, ScreenRect, pointer_at};
use crate::popup::{floating_panel, retire_floating};
use crate::theme::Theme;
use crate::ui::release_hover;

/// The width and the height of the picture of a layout, in logical
/// pixels.
const CARD: (f32, f32) = (104., 66.);

/// How many pictures stand in a row at most.
const COLUMNS: usize = 3;

/// The space between two zones of a picture.
const ZONE_GAP: f32 = 3.;

/// How far left of the right end of a window the layouts hang from for
/// the action, and how far below its top, in logical pixels.
pub const REACH: f32 = 150.;
pub const BELOW: f32 = 36.;

/// How far from the layouts the pointer may go before they count as left,
/// in logical pixels, and how long it may stay away.
const LEEWAY: f32 = 24.;
const AWAY: Duration = Duration::from_millis(350);

/// How often it is looked whether the pointer has left.
const POINTER_LOOK: Duration = Duration::from_millis(100);

/// In how many columns and rows `count` pictures stand.
fn grid(count: usize) -> (usize, usize) {
    let columns = count.clamp(1, COLUMNS);
    (columns, count.max(1).div_ceil(columns))
}

/// The logical width and height of the panel of `count` pictures, `gap`
/// apart and `padding` from its border.
fn extent(count: usize, gap: f32, padding: f32) -> (f32, f32) {
    let (columns, rows) = grid(count);
    let along = |many: usize, length: f32| {
        many as f32 * length + many.saturating_sub(1) as f32 * gap + padding * 2.
    };
    (along(columns, CARD.0), along(rows, CARD.1))
}

/// Whether `point` is within `frame` grown by `leeway` on every side.
fn near(frame: ScreenRect, (x, y): (i32, i32), leeway: i32) -> bool {
    (frame.left - leeway..frame.right + leeway).contains(&x)
        && (frame.top - leeway..frame.bottom + leeway).contains(&y)
}

/// The root view of the window of the snap layouts.
pub struct LayoutsView {
    /// The window they place.
    target: AppWindowId,
    display: Display,
    layouts: Vec<Vec<Zone>>,
    /// The layout and the zone under the pointer.
    hovered: Option<(usize, usize)>,
    /// 0 while hidden, 1 when fully open.
    visibility: Spring,
    closing: bool,
    /// Whether the window has had keyboard focus; before that, not having
    /// it does not mean the user turned away.
    was_active: bool,
    focus: FocusHandle,
    focused: bool,
    _activation: Subscription,
}

impl LayoutsView {
    /// Opens the layouts for the window `target` on `display`, hanging
    /// from `point`; `hovered` says that the pointer resting on a maximize
    /// button opened them.
    pub fn open(
        target: AppWindowId,
        display: Display,
        point: (i32, i32),
        layouts: Vec<Vec<Zone>>,
        hovered: bool,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<Self>> {
        let theme = *cx.global::<Theme>();
        let gap = theme.spacing.small / px(1.);
        let padding = theme.spacing.medium / px(1.);
        let (width, height) = extent(layouts.len(), gap, padding);
        let frame = anchor_at(&display, point, cx).frame(width, height);
        let kind = if hovered { Kind::Surface } else { Kind::Popup };
        open_at(frame, &display, kind, cx, move |window, cx| {
            let motion = cx.global::<Theme>().motion;
            let mut visibility = Spring::new(motion.spatial, 0.);
            visibility.animate_to(1.);
            let activation = cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if window.is_window_active() {
                    this.was_active = true;
                } else if this.was_active {
                    this.close(cx);
                }
            });
            if hovered {
                Self::close_when_left(frame, (LEEWAY * display.scale) as i32, cx);
            }
            Self {
                target,
                display,
                layouts,
                hovered: None,
                visibility,
                closing: false,
                was_active: false,
                focus: cx.focus_handle(),
                focused: hovered,
                _activation: activation,
            }
        })
    }

    /// Closes the layouts once the pointer has been away from `frame`,
    /// grown by `leeway`, for a moment.
    fn close_when_left(frame: ScreenRect, leeway: i32, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut away: Option<Instant> = None;
            loop {
                cx.background_executor().timer(POINTER_LOOK).await;
                let here = pointer_at().is_some_and(|pointer| near(frame, pointer, leeway));
                let now = Instant::now();
                away = if here { None } else { away.or(Some(now)) };
                let gone = away.is_some_and(|since| now.duration_since(since) >= AWAY);
                let going = this.update(cx, |this, cx| {
                    if gone {
                        this.close(cx);
                    }
                    !this.closing
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Whether the view is going away.
    pub fn is_closing(&self) -> bool {
        self.closing
    }

    /// Starts closing; the window goes away once the motion has finished.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            self.close(cx);
        }
    }

    /// Puts the window into `zone` of the layout at `layout`.
    fn choose(&mut self, layout: usize, zone: usize, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let Some(zones) = self.layouts.get(layout).cloned() else {
            return;
        };
        let (target, display) = (self.target, self.display);
        self.close(cx);
        // Other windows are opened and placed once this click is handled.
        cx.defer(move |cx| Snapper::place_in_layout(target, display, zones, zone, cx));
    }

    /// The picture of the layout at `index`.
    fn card(&self, index: usize, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let zones = self.layouts[index].iter().enumerate().map(|(place, zone)| {
            let lit = self.hovered == Some((index, place));
            let color = match lit {
                true => theme.colors.primary,
                false => theme.colors.secondary_container,
            };
            let room = |share: f32, length: f32| px(share * length);
            div()
                .id(("snap-zone", index * 64 + place))
                .absolute()
                .left(room(zone.x, CARD.0) + px(ZONE_GAP / 2.))
                .top(room(zone.y, CARD.1) + px(ZONE_GAP / 2.))
                .w((room(zone.width, CARD.0) - px(ZONE_GAP)).max(px(1.)))
                .h((room(zone.height, CARD.1) - px(ZONE_GAP)).max(px(1.)))
                .rounded(theme.shape.small)
                .bg(color)
                .cursor(CursorStyle::PointingHand)
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    let spot = Some((index, place));
                    match (*hovered, this.hovered == spot) {
                        (true, false) => this.hovered = spot,
                        (false, true) => this.hovered = None,
                        _ => return,
                    }
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, _, _, cx| this.choose(index, place, cx)))
        });
        div()
            .relative()
            .flex_none()
            .w(px(CARD.0))
            .h(px(CARD.1))
            .p(px(ZONE_GAP / 2.))
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container_highest)
            .children(zones)
    }
}

impl Render for LayoutsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus);
        }
        let visibility = self.visibility.value(window);
        if self.closing && self.visibility.is_settled() {
            retire_floating(window, cx);
        }
        let mut cards = Vec::with_capacity(self.layouts.len());
        for index in 0..self.layouts.len() {
            cards.push(self.card(index, &theme, cx));
        }
        let (columns, _) = grid(cards.len());
        let gap = theme.spacing.small;
        let width = px(columns as f32 * CARD.0) + gap * (columns.saturating_sub(1) as f32);
        floating_panel(&theme, ScreenEdge::Top, visibility, |panel| {
            panel.child(
                div()
                    .id("snap-layouts")
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
                    .w(width)
                    .flex()
                    .flex_wrap()
                    .gap(gap)
                    .children(cards),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layouts_stand_three_in_a_row() {
        assert_eq!(grid(6), (3, 2));
        assert_eq!(grid(2), (2, 1));
        assert_eq!(grid(0), (1, 1));
        assert_eq!(
            extent(6, 8., 12.),
            (3. * 104. + 16. + 24., 2. * 66. + 8. + 24.)
        );
    }

    #[test]
    fn the_pointer_may_stray_a_little() {
        let frame = ScreenRect {
            left: 100,
            top: 100,
            right: 200,
            bottom: 200,
        };
        assert!(near(frame, (90, 150), 24));
        assert!(!near(frame, (60, 150), 24));
        assert!(near(frame, (150, 223), 24));
    }
}
