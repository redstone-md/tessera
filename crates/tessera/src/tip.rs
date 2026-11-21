// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Tips: a line of text that comes out of a surface under what the pointer
//! rests on, in a small window of its own, so that nothing on the surface
//! moves for it.

use std::time::Duration;

use gpui::{
    App, AsyncApp, Bounds, Context, Global, IntoElement, ParentElement, Render, SharedString,
    Styled, Window, WindowHandle, div, point, px, size,
};

use crate::motion::Spring;
use crate::platform::{NativeSurface, ScreenEdge};
use crate::popup::{MARGIN, PopupAnchor, floating_travel, open_floating, retire_floating};
use crate::theme::Theme;
use crate::ui::{one_line, text_width};

/// How long the pointer rests before a tip opens.
const OPEN_DELAY: Duration = Duration::from_millis(350);

/// How long after the pointer left a tip stays, so that it can go on to
/// the next thing with a tip without the tip closing in between.
const CLOSE_DELAY: Duration = Duration::from_millis(90);

/// The height of a tip, in logical pixels.
const HEIGHT: f32 = 28.;

/// The widest a tip gets; a longer text goes on over more lines.
const WIDEST: f32 = 360.;

/// The height of each line of a tip after its first.
const LINE: f32 = 18.;

/// The most lines a tip has; what does not fit them is cut.
const MOST_LINES: usize = 4;

/// The part of a line that words fill on average when a text is broken
/// between them.
const FILLED: f32 = 0.9;

/// Tells what a tip is about: the kind of thing and its number there.
pub type TipKey = (&'static str, u64);

/// The open tip and what the pointer last asked for.
#[derive(Default)]
pub struct Tips {
    open: Option<(TipKey, WindowHandle<TipView>)>,
    /// What the pointer rests on.
    wanted: Option<TipKey>,
    /// Counts the requests, so that a delayed one that was overtaken by
    /// another is dropped.
    requests: u64,
}

impl Global for Tips {}

impl Tips {
    /// The pointer rests on `key`: `text` opens at `anchor` after a moment,
    /// or at once if a tip is open already, which then moves there.
    pub fn request(key: TipKey, text: SharedString, anchor: PopupAnchor, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        this.wanted = Some(key);
        this.requests += 1;
        if text.is_empty() {
            Self::close(cx);
            return;
        }
        if let Some((_, window)) = this.open {
            let moved = window.update(cx, |view, window, cx| {
                view.show(text.clone(), anchor, window, cx);
            });
            if moved.is_ok() {
                cx.global_mut::<Self>().open = Some((key, window));
                return;
            }
            cx.global_mut::<Self>().open = None;
        }
        Self::later(OPEN_DELAY, cx, move |cx| {
            match Self::open(text, anchor, cx) {
                Ok(window) => cx.global_mut::<Self>().open = Some((key, window)),
                Err(error) => report!("a tip did not open: {error:#}"),
            }
        });
    }

    /// The pointer left `key`: its tip closes after a moment, unless the
    /// pointer comes onto something else with a tip before.
    pub fn leave(key: TipKey, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if this.wanted != Some(key) {
            return;
        }
        this.wanted = None;
        Self::later(CLOSE_DELAY, cx, Self::close);
    }

    /// Closes the tip now, and drops what was asked for.
    pub fn close(cx: &mut App) {
        let this = cx.global_mut::<Self>();
        this.requests += 1;
        if let Some((_, window)) = this.open.take() {
            // A tip whose window is gone already is closed, which is fine.
            window.update(cx, |view, _, cx| view.close(cx)).ok();
        }
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
        text: SharedString,
        anchor: PopupAnchor,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<TipView>> {
        let rough = Bounds {
            origin: point(
                px(anchor.x as f32 / anchor.scale),
                px(anchor.surface.top as f32 / anchor.scale),
            ),
            size: size(px(WIDEST), px(HEIGHT)),
        };
        open_floating(rough, anchor.display_id, false, cx, |window, cx| {
            let theme = *cx.global::<Theme>();
            let (width, lines) = extent(&text, &theme, window);
            let frame = anchor.frame(width, height(lines));
            let native = NativeSurface::of(window)
                .and_then(|native| native.adopt_as_shell_surface().map(|()| native));
            match native {
                // The window learns its size only if it is given while
                // nothing of the user interface is being worked on, so
                // it is placed and shown right after this. It lets the
                // pointer through once it is shown.
                Ok(native) => cx
                    .spawn(async move |_, _| {
                        let shown = native.place(frame).and_then(|()| {
                            native.reveal(false);
                            native.adopt_as_overlay()
                        });
                        if let Err(error) = shown {
                            report!("a tip is not where it belongs: {error:#}");
                        }
                    })
                    .detach(),
                Err(error) => report!("a tip is not where it belongs: {error:#}"),
            }
            let mut visibility = Spring::new(theme.motion.spatial, 0.);
            visibility.animate_to(1.);
            TipView {
                text,
                lines,
                edge: anchor.edge,
                visibility,
                closing: false,
            }
        })
    }
}

/// The logical width of the tip that says `text`, and how many lines it
/// takes.
fn extent(text: &SharedString, theme: &Theme, window: &Window) -> (f32, usize) {
    let typography = theme.typography;
    let text = text_width(text, typography.family, typography.label_size, window) / px(1.);
    let padding = theme.spacing.medium / px(1.) * 2.;
    // A pixel more than measured, so that rounding cuts nothing.
    let text = text.ceil() + 1.;
    let room = WIDEST - padding;
    (text.min(room) + padding, lines(text, room))
}

/// How many lines a text `text` wide takes where a line has `room`.
fn lines(text: f32, room: f32) -> usize {
    if text <= room {
        1
    } else {
        ((text / (room * FILLED)).ceil() as usize).clamp(2, MOST_LINES)
    }
}

/// The logical height of a tip of `lines` lines.
fn height(lines: usize) -> f32 {
    HEIGHT + LINE * lines.saturating_sub(1) as f32
}

/// The root view of a tip: one line of text in a pill.
pub struct TipView {
    text: SharedString,
    /// How many lines the text takes.
    lines: usize,
    /// The edge the tip's surface is docked to; it moves in from there.
    edge: ScreenEdge,
    /// 0 while hidden, 1 when fully open.
    visibility: Spring,
    closing: bool,
}

impl TipView {
    /// Says `text` at `anchor` from now on.
    fn show(
        &mut self,
        text: SharedString,
        anchor: PopupAnchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme = *cx.global::<Theme>();
        let (width, lines) = extent(&text, &theme, window);
        let frame = anchor.frame(width, height(lines));
        match NativeSurface::of(window) {
            // The window learns its new size only if it is given while
            // nothing of the user interface is being worked on, as it is
            // here.
            Ok(native) => cx
                .spawn(async move |_, _| {
                    if let Err(error) = native.place(frame) {
                        report!("a tip is not where it belongs: {error:#}");
                    }
                })
                .detach(),
            Err(error) => report!("a tip is not where it belongs: {error:#}"),
        }
        self.text = text;
        self.lines = lines;
        self.edge = anchor.edge;
        self.closing = false;
        self.visibility.animate_to(1.);
        cx.notify();
    }

    /// Starts closing; the window goes away once the motion has finished.
    fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }
}

impl Render for TipView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window);
        if self.closing && self.visibility.is_settled() {
            retire_floating(window, cx);
        }
        let pill = div()
            .size_full()
            .relative()
            .top(floating_travel(self.edge, visibility))
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .bg(theme.colors.surface_container_highest);
        let pill = if self.lines > 1 {
            let text: SharedString = self.text.replace(['\n', '\r'], " ").into();
            pill.rounded(theme.shape.medium)
                .line_height(px(LINE))
                .text_center()
                .child(div().w_full().child(text))
        } else {
            pill.rounded(theme.shape.full)
                .child(one_line(self.text.clone()))
        };
        div()
            .size_full()
            .p(px(MARGIN))
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            .opacity(visibility.clamp(0., 1.))
            .child(pill)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_tip_goes_on_over_more_lines() {
        assert_eq!(lines(120., 300.), 1);
        assert_eq!(lines(300., 300.), 1);
        assert_eq!(lines(301., 300.), 2);
        assert_eq!(lines(700., 300.), 3);
        assert_eq!(lines(9000., 300.), MOST_LINES);
        assert_eq!(height(1), HEIGHT);
        assert_eq!(height(3), HEIGHT + LINE * 2.);
    }
}
