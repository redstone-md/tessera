// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The seconds before a screenshot is taken, counted down in a small
//! window in the middle of the display under the pointer. The window lies
//! above every other, never takes the keyboard and lets the pointer
//! through to what is under it, so that the user can go on setting up
//! what is to be in the picture; it closes itself before the picture is
//! taken, so that it is never in it. Asking for a screenshot again while
//! the seconds run stops the count.

use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Bounds, Context, FontWeight, IntoElement, ParentElement, Render,
    Styled, Window, WindowHandle, div, point, px, size,
};

use super::{Screenshots, gpui_display};
use crate::platform::{Display, NativeSurface, ScreenRect, SmallBirth, pointer_at};
use crate::popup::floating_window;
use crate::theme::Theme;

/// The side of the window, in logical pixels.
const SIDE: f32 = 168.;

/// How large the seconds are written.
const FIGURE: f32 = 88.;

/// How long the compositor is given to take the window off the screen
/// before the picture is taken.
const GONE: Duration = Duration::from_millis(180);

/// The window that counts down.
pub struct Countdown {
    left: u32,
}

impl Countdown {
    /// Counts `seconds` down, one a second, and closes the window again;
    /// true once the time ran out, false where the count was stopped. A
    /// window that cannot open is reported, and the seconds are waited for
    /// all the same.
    pub async fn run(seconds: u32, cx: &mut AsyncApp) -> bool {
        let opened = cx.update(|cx| {
            Screenshots::counting(true, cx);
            Self::open(seconds, cx)
        });
        let window = match opened {
            Ok(Ok(window)) => Some(window),
            Ok(Err(error)) => {
                report!("the seconds before the screenshot are not shown: {error:#}");
                None
            }
            Err(_) => return false,
        };
        let mut left = seconds;
        let mut counted = true;
        while left > 0 {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            left -= 1;
            let stopped = cx.update(Screenshots::count_stopped).unwrap_or(true);
            if stopped {
                counted = false;
                break;
            }
            if let Some(window) = window {
                window
                    .update(cx, |view, _, cx| {
                        view.left = left;
                        cx.notify();
                    })
                    .ok();
            }
        }
        if let Some(window) = window {
            window
                .update(cx, |_, window, _| window.remove_window())
                .ok();
            cx.background_executor().timer(GONE).await;
        }
        cx.update(|cx| Screenshots::counting(false, cx)).ok();
        counted
    }

    /// Opens the window in the middle of the display under the pointer.
    fn open(seconds: u32, cx: &mut App) -> anyhow::Result<WindowHandle<Self>> {
        let display = pointer_at()
            .and_then(Display::at)
            .or_else(|| Display::all().into_iter().next())
            .ok_or_else(|| anyhow::anyhow!("there is no display"))?;
        let side = (SIDE * display.scale).round() as i32;
        let frame = display.frame;
        let left = (frame.left + frame.right - side) / 2;
        let top = (frame.top + frame.bottom - side) / 2;
        let placed = ScreenRect {
            left,
            top,
            right: left + side,
            bottom: top + side,
        };
        let rough = Bounds {
            origin: point(
                px(left as f32 / display.scale),
                px(top as f32 / display.scale),
            ),
            size: size(px(SIDE), px(SIDE)),
        };
        let options = floating_window(rough, gpui_display(&display, cx), false);
        let _small = SmallBirth::begin();
        cx.open_window(options, |window, cx| {
            let adopted = NativeSurface::of(window).and_then(|native| {
                native.adopt_as_click_through()?;
                native.place(placed)?;
                native.reveal(false);
                Ok(())
            });
            if let Err(error) = adopted {
                report!("the count before a screenshot is not where it belongs: {error:#}");
            }
            cx.new(|_| Self { left: seconds })
        })
    }
}

impl Render for Countdown {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let ground = theme.colors.surface_container.opacity(0.92);
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .size(px(SIDE - 8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme.shape.full)
                    .bg(ground)
                    .border_2()
                    .border_color(theme.colors.primary)
                    .text_color(theme.colors.primary)
                    .text_size(px(FIGURE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.left.to_string()),
            )
    }
}
