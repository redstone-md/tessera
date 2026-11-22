// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The flyout of the volume: a pill that shows the level of the default
//! audio output for a moment whenever it changes, at the lower side of
//! what the bars leave of the display the pointer is on.
//!
//! Tessera is not told of a change; it reads the level a few times a
//! second while the flyout is wanted, and not at all otherwise.

use std::time::Duration;

use gpui::{
    App, AsyncApp, Context, FontWeight, Global, IntoElement, ParentElement, Render, Styled, Task,
    WeakEntity, Window, WindowHandle, div, point, px, size,
};
use serde::Deserialize;

use crate::motion::Spring;
use crate::platform::{AudioOutput, NativeSurface, ScreenRect};
use crate::popup::{MARGIN, Popups, open_floating, retire_floating};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, wave_bar};

/// How often the level is read while the flyout is wanted.
const READ_INTERVAL: Duration = Duration::from_millis(250);

/// How often the flyout counts down the time it has left.
const TICK: Duration = Duration::from_millis(100);

/// How long the flyout stays after the last change.
const STAY: Duration = Duration::from_millis(1600);

/// The size of the pill, in logical pixels.
const WIDTH: f32 = 280.;
const HEIGHT: f32 = 48.;

/// How far above the lower side of the space the pill hangs.
const LIFT: f32 = 24.;

/// How far below its place the pill starts and ends.
const TRAVEL: f32 = 16.;

/// The room between the ends of the pill and what is in it.
const PADDING: f32 = 20.;

/// The room of the number beside the line.
const NUMBER_WIDTH: f32 = 40.;

pub const ICON_MUTED: &str = "\u{e74f}";
pub const ICON_LEVELS: [&str; 4] = ["\u{e992}", "\u{e993}", "\u{e994}", "\u{e995}"];

/// The icon of a sound output at `volume`, from 0 to 1.
pub fn volume_icon(volume: f32, muted: bool) -> &'static str {
    if muted {
        return ICON_MUTED;
    }
    let step = (volume * 3.).ceil().clamp(0., 3.) as usize;
    ICON_LEVELS[step]
}

/// What the configuration says about flyouts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlyoutConfig {
    /// Whether a change of the volume is shown.
    #[serde(default)]
    volume: bool,
}

/// What the default output is set to.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Level {
    volume: f32,
    muted: bool,
}

impl Level {
    fn read() -> Option<Self> {
        let output = AudioOutput::default_device().ok()?;
        Some(Self {
            volume: output.volume().ok()?,
            muted: output.is_muted().ok()?,
        })
    }
}

/// The flyout and what looks out for changes to show.
#[derive(Default)]
pub struct Flyouts {
    /// The level read last; none before the first reading and while
    /// there is no output.
    known: Option<Level>,
    open: Option<WindowHandle<FlyoutView>>,
    watch: Option<Task<()>>,
}

impl Global for Flyouts {}

impl Flyouts {
    /// Takes over what the configuration says: the level is read while
    /// the flyout is wanted.
    pub fn configure(config: &FlyoutConfig, cx: &mut App) {
        let this = cx.default_global::<Self>();
        if !config.volume {
            this.watch = None;
            this.known = None;
            return;
        }
        if this.watch.is_some() {
            return;
        }
        let watch = cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                // Asking Windows takes a while, and nothing is drawn
                // meanwhile if the thread of the windows asks.
                let level = cx
                    .background_executor()
                    .spawn(async { Level::read() })
                    .await;
                if cx.update(|cx| Self::take(level, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(READ_INTERVAL).await;
            }
        });
        cx.global_mut::<Self>().watch = Some(watch);
    }

    /// Shows `level` if it is another one than the one before. The first
    /// one read is no change, and neither is one that comes while a popup
    /// is open, whose own controls show it.
    fn take(level: Option<Level>, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        let before = std::mem::replace(&mut this.known, level);
        let (Some(_), Some(level)) = (before, level) else {
            return;
        };
        if before == Some(level) || Popups::is_open(cx) {
            return;
        }
        if let Some(open) = cx.global::<Self>().open {
            let shown = open.update(cx, |view, _, cx| view.show(level, cx));
            if matches!(shown, Ok(true)) {
                return;
            }
        }
        Self::open(level, cx);
    }

    /// Opens the window of the flyout with `level` in it.
    fn open(level: Level, cx: &mut App) {
        let Some(anchor) = Shell::middle_anchor(cx) else {
            return;
        };
        let area = anchor.display.and_then(|display| display.work_area());
        let area = area.unwrap_or(anchor.screen);
        let frame = frame(area, anchor.scale);
        let rough = gpui::Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(px(WIDTH), px(HEIGHT)),
        };
        let opened = open_floating(rough, anchor.display_id, false, cx, |window, cx| {
            let adopted = NativeSurface::of(window).and_then(|native| {
                native.adopt_as_popup()?;
                native.place(frame)?;
                native.reveal(false);
                Ok(())
            });
            if let Err(error) = adopted {
                report!("the flyout is not where it belongs: {error:#}");
            }
            FlyoutView::new(level, cx)
        });
        match opened {
            Ok(window) => cx.global_mut::<Self>().open = Some(window),
            Err(error) => report!("the flyout did not open: {error:#}"),
        }
    }
}

/// The frame of the window of the flyout in the space `area`, in physical
/// pixels, of which there are `scale` in a logical one: in the middle of
/// the lower side, a little above it.
fn frame(area: ScreenRect, scale: f32) -> ScreenRect {
    let physical = |length: f32| (length * scale).round() as i32;
    let width = physical(WIDTH + MARGIN * 2.);
    let height = physical(HEIGHT + MARGIN * 2. + TRAVEL);
    let left = (area.left + area.right - width) / 2;
    let bottom = area.bottom - physical(LIFT - MARGIN);
    ScreenRect {
        left,
        top: bottom - height,
        right: left + width,
        bottom,
    }
}

/// The root view of the window of the flyout.
pub struct FlyoutView {
    level: Level,
    /// The length of the line, which follows the volume.
    line: Spring,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    /// How long it still stays.
    left: Duration,
    /// The window was put away and shows nothing more.
    gone: bool,
}

impl FlyoutView {
    fn new(level: Level, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(TICK).await;
                if !matches!(this.update(cx, |this, cx| this.tick(cx)), Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        let motion = cx.global::<Theme>().motion;
        let mut presence = Spring::new(motion.spatial, 0.);
        presence.animate_to(1.);
        Self {
            level,
            line: Spring::new(motion.spatial, level.volume),
            presence,
            left: STAY,
            gone: false,
        }
    }

    /// Shows `level` and stays on. `false` when the window was put away
    /// and another one is needed.
    fn show(&mut self, level: Level, cx: &mut Context<Self>) -> bool {
        if self.gone {
            return false;
        }
        self.level = level;
        self.line.animate_to(level.volume);
        self.presence.animate_to(1.);
        self.left = STAY;
        cx.notify();
        true
    }

    /// Counts down the time the flyout has left. `false` once the window
    /// was put away.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        if self.gone {
            return false;
        }
        if !self.left.is_zero() {
            self.left = self.left.saturating_sub(TICK);
            if self.left.is_zero() {
                self.presence.animate_to(0.);
                cx.notify();
            }
        }
        true
    }
}

impl Render for FlyoutView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let presence = self.presence.value(window);
        let line = self.line.value(window).clamp(0., 1.);
        if self.left.is_zero() && self.presence.is_settled() && !self.gone {
            self.gone = true;
            retire_floating(window, cx);
        }
        let level = self.level;
        let colors = theme.colors;
        let shown = match level.muted {
            true => colors.on_surface_variant,
            false => colors.on_surface,
        };
        let pill = div()
            .relative()
            .top(px(TRAVEL * (1. - presence)))
            .w(px(WIDTH))
            .h(px(HEIGHT))
            .px(px(PADDING))
            .gap(theme.spacing.medium)
            .flex()
            .items_center()
            .rounded(theme.shape.full)
            .bg(colors.surface_container_highest)
            .text_color(shown)
            .child(icon(&theme, volume_icon(level.volume, level.muted)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(wave_bar(&theme, if level.muted { 0. } else { line })),
            )
            .child(
                div()
                    .w(px(NUMBER_WIDTH))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("{:.0}", level.volume * 100.)),
            );
        div()
            .size_full()
            .p(px(MARGIN))
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .opacity(presence.clamp(0., 1.))
            .child(pill)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flyout_hangs_in_the_middle_above_the_lower_side() {
        let area = ScreenRect {
            left: 100,
            top: 0,
            right: 1100,
            bottom: 800,
        };
        let frame = frame(area, 1.);
        assert_eq!(frame.right - frame.left, 304);
        assert_eq!(frame.left - area.left, area.right - frame.right);
        assert_eq!(frame.bottom, 788);
        assert_eq!(frame.bottom - frame.top, 88);
        let doubled = super::frame(area, 2.);
        assert_eq!(doubled.right - doubled.left, 608);
    }

    #[test]
    fn the_icon_follows_the_volume() {
        assert_eq!(volume_icon(0.5, true), ICON_MUTED);
        assert_eq!(volume_icon(0., false), ICON_LEVELS[0]);
        assert_eq!(volume_icon(1., false), ICON_LEVELS[3]);
    }
}
