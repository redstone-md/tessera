// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Previews: a live picture of a window, shown while the pointer rests on
//! what stands for it.

use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Bounds, Context, Global, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, WindowHandle,
    div, point, px, size,
};

use crate::motion::{Interaction, Spring};
use crate::platform::{AppWindowId, NativeSurface, ScreenEdge, ScreenRect, Thumbnail};
use crate::popup::{MARGIN, PopupAnchor, floating_panel, floating_travel, floating_window};
use crate::theme::Theme;
use crate::ui::{icon, one_line, release_hover, state_layer};

/// How long the pointer rests before a preview opens.
const OPEN_DELAY: Duration = Duration::from_millis(400);

/// How long it rests before an open preview shows another window.
const SWITCH_DELAY: Duration = Duration::from_millis(120);

/// How long after the pointer left a preview stays, so that the pointer
/// can travel onto it.
const CLOSE_DELAY: Duration = Duration::from_millis(300);

/// The largest picture, in logical pixels.
const PICTURE_WIDTH: f32 = 260.;
const PICTURE_HEIGHT: f32 = 160.;

/// The width of a preview whose picture is narrower or missing.
const LEAST_WIDTH: f32 = 160.;

/// The height of the row with the title.
const TITLE_HEIGHT: f32 = 24.;

const ICON_CLOSE: &str = "\u{e8bb}";

/// The open preview and what the pointer last asked for.
#[derive(Default)]
pub struct Previews {
    open: Option<(AppWindowId, WindowHandle<PreviewView>)>,
    /// The window the pointer rests on what stands for.
    wanted: Option<AppWindowId>,
    /// Counts the requests, so that a delayed one that was overtaken by
    /// another is dropped.
    requests: u64,
}

impl Global for Previews {}

impl Previews {
    /// The pointer rests on what stands for `target`: its preview opens at
    /// `anchor` after a moment, unless the pointer moves on before.
    pub fn request(target: AppWindowId, anchor: PopupAnchor, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        this.wanted = Some(target);
        if this.open.as_ref().is_some_and(|(open, _)| *open == target) {
            Self::keep(cx);
            return;
        }
        let delay = if this.open.is_some() {
            SWITCH_DELAY
        } else {
            OPEN_DELAY
        };
        Self::later(delay, cx, move |cx| {
            Self::close(cx);
            match Self::open(target, &anchor, cx) {
                Ok(window) => cx.global_mut::<Self>().open = Some((target, window)),
                Err(error) => report!("a preview did not open: {error:#}"),
            }
        });
    }

    /// The pointer left what stands for `target`, or rests on something
    /// that stands for no window.
    pub fn leave(target: Option<AppWindowId>, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if target.is_none() || this.wanted == target {
            this.wanted = None;
        }
        Self::dismiss(cx);
    }

    /// The pointer left: the preview closes after a moment, unless the
    /// pointer comes back or reaches the preview before. Where the pointer
    /// went may have been told before it is told to have left.
    pub fn dismiss(cx: &mut App) {
        Self::later(CLOSE_DELAY, cx, |cx| {
            let this = cx.global::<Self>();
            if this.wanted.is_some() {
                return;
            }
            let open = this.open.as_ref().map(|(_, window)| *window);
            let hovered = open.and_then(|window| {
                let hovered = |_: &mut PreviewView, window: &mut Window, _: &mut Context<_>| {
                    window.is_window_hovered()
                };
                window.update(cx, hovered).ok()
            });
            if hovered != Some(true) {
                Self::close(cx);
            }
        });
    }

    /// Drops what was asked for and has not happened yet.
    pub fn keep(cx: &mut App) {
        cx.global_mut::<Self>().requests += 1;
    }

    /// Closes the preview now, and drops what was asked for.
    pub fn close(cx: &mut App) {
        let this = cx.global_mut::<Self>();
        this.requests += 1;
        if let Some((_, window)) = this.open.take() {
            window.update(cx, |view, _, cx| view.close(cx)).ok();
        }
    }

    /// The window whose preview is open.
    pub fn shown(cx: &App) -> Option<AppWindowId> {
        cx.global::<Self>().open.as_ref().map(|(open, _)| *open)
    }

    /// Runs `act` after `delay` unless something else is asked for before.
    fn later(delay: Duration, cx: &mut App, act: impl FnOnce(&mut App) + 'static) {
        let this = cx.global_mut::<Self>();
        this.requests += 1;
        let request = this.requests;
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor().timer(delay).await;
            cx.update(|cx| {
                if cx.global::<Self>().requests == request {
                    act(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn open(
        target: AppWindowId,
        anchor: &PopupAnchor,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<PreviewView>> {
        let padding = cx.global::<Theme>().spacing.medium / px(1.);
        let (edge, scale, anchor) = (anchor.edge, anchor.scale, *anchor);
        let rough = Bounds {
            origin: point(
                px(anchor.x as f32 / scale),
                px(anchor.surface.top as f32 / scale),
            ),
            size: size(px(PICTURE_WIDTH), px(PICTURE_HEIGHT)),
        };
        cx.open_window(
            floating_window(rough, anchor.display_id, false),
            |window, cx| {
                let native = NativeSurface::of(window);
                let thumbnail = native
                    .as_ref()
                    .ok()
                    .and_then(|native| Thumbnail::of(target, *native).ok());
                let source = thumbnail.as_ref().and_then(Thumbnail::source_size);
                let picture = source.map(|source| fit(source, scale));
                let (width, height) = extent(picture, padding);
                let placed = native.and_then(|native| {
                    native.adopt_as_shell_surface()?;
                    native.place(anchor.frame(width, height))
                });
                if let Err(error) = placed {
                    report!("a preview is not where it belongs: {error:#}");
                }
                let theme = cx.global::<Theme>();
                let mut visibility = Spring::new(theme.motion.spatial, 0.);
                visibility.animate_to(1.);
                let close_button = Interaction::new(&theme.motion);
                cx.new(|_| PreviewView {
                    target,
                    title: target.title().into(),
                    thumbnail: thumbnail.filter(|_| picture.is_some()),
                    picture: picture.unwrap_or_default(),
                    edge,
                    visibility,
                    closing: false,
                    close_button,
                })
            },
        )
    }
}

/// The logical size of the picture of a window of `source` physical pixels
/// on a display of `scale`: as large as the limits allow, never enlarged.
fn fit((width, height): (i32, i32), scale: f32) -> (f32, f32) {
    let (width, height) = (width as f32 / scale, height as f32 / scale);
    let factor = (PICTURE_WIDTH / width).min(PICTURE_HEIGHT / height).min(1.);
    (width * factor, height * factor)
}

/// The logical width and height of the panel around `picture`.
fn extent(picture: Option<(f32, f32)>, padding: f32) -> (f32, f32) {
    let (width, height) = match picture {
        Some((width, height)) => (width.max(LEAST_WIDTH), TITLE_HEIGHT + padding + height),
        None => (LEAST_WIDTH, TITLE_HEIGHT),
    };
    (width + padding * 2., height + padding * 2.)
}

/// The root view of a preview: the title of a window, a button that closes
/// it, and its picture, which switches to it.
pub struct PreviewView {
    target: AppWindowId,
    title: SharedString,
    /// Absent when Windows gives no picture.
    thumbnail: Option<Thumbnail>,
    /// The logical size of the picture.
    picture: (f32, f32),
    /// The edge the preview's surface is docked to; it moves in from there.
    edge: ScreenEdge,
    /// 0 while hidden, 1 when fully open.
    visibility: Spring,
    closing: bool,
    close_button: Interaction,
}

impl PreviewView {
    /// Starts closing; the window goes away once the motion has finished.
    fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }

    /// Where Windows draws the picture, in physical pixels of the window.
    fn picture_area(&self, theme: &Theme, visibility: f32, window: &Window) -> ScreenRect {
        let padding = theme.spacing.medium / px(1.);
        let (width, height) = self.picture;
        let inner = width.max(LEAST_WIDTH);
        let left = MARGIN + padding + (inner - width) / 2.;
        let travel = floating_travel(self.edge, visibility) / px(1.);
        let top = MARGIN + travel + padding + TITLE_HEIGHT + padding;
        let physical = |length: f32| (length * window.scale_factor()).round() as i32;
        ScreenRect {
            left: physical(left),
            top: physical(top),
            right: physical(left + width),
            bottom: physical(top + height),
        }
    }
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window);
        if self.closing && self.visibility.is_settled() {
            window.remove_window();
        }
        if let Some(thumbnail) = &self.thumbnail {
            thumbnail.place(self.picture_area(&theme, visibility, window), visibility);
        }
        let state = self.close_button.state(window);
        let target = self.target;
        let (width, height) = self.picture;

        let close = div()
            .id("preview-close")
            .size(px(TITLE_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(&theme, theme.colors.surface_container, state))
            .text_color(theme.colors.on_surface_variant)
            .on_click(move |_, _, cx| {
                target.close();
                // This window cannot be told to close while it handles
                // the click.
                cx.defer(Previews::close);
            })
            .child(icon(&theme, ICON_CLOSE).text_size(theme.typography.label_size));
        let close = Interaction::track(close, cx, |this: &mut Self| Some(&mut this.close_button));
        let title = div()
            .h(px(TITLE_HEIGHT))
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(div().flex_1().min_w_0().child(one_line(self.title.clone())))
            .child(close);
        // Windows draws the picture over this.
        let picture = self.thumbnail.is_some().then(|| {
            div()
                .id("preview-picture")
                .w(px(width))
                .h(px(height))
                .mx_auto()
                .flex_none()
                .bg(theme.colors.surface_container_highest)
                .on_click(move |_, _, cx| {
                    target.activate();
                    cx.defer(Previews::close);
                })
        });
        floating_panel(&theme, self.edge, visibility, |panel| {
            panel.child(title).children(picture)
        })
        .id("preview")
        .on_hover(|hovered, _, cx| {
            if *hovered {
                Previews::keep(cx);
            } else {
                Previews::dismiss(cx);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_keeps_its_shape_within_the_limits() {
        // A wide window is as wide as allowed, a tall one as tall.
        assert_eq!(fit((2600, 1000), 1.), (260., 100.));
        assert_eq!(fit((800, 1600), 1.), (80., 160.));
        // On a denser display the same pixels are a smaller window.
        assert_eq!(fit((2600, 1000), 2.), (260., 100.));
        // A small window is not enlarged.
        assert_eq!(fit((100, 50), 1.), (100., 50.));
    }

    #[test]
    fn the_panel_surrounds_the_title_and_the_picture() {
        assert_eq!(extent(Some((260., 100.)), 12.), (284., 160.));
        // A narrow picture leaves room for the title.
        assert_eq!(extent(Some((80., 160.)), 12.), (184., 220.));
        assert_eq!(extent(None, 12.), (184., 48.));
    }
}
