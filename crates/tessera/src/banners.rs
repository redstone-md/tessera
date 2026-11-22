// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Banners: a notification that arrives in the notification center of
//! Windows is shown for a few seconds in a corner of a display, unless
//! banners are held back.
//!
//! Windows tells a program like this one about notifications only through
//! the list of its notification center, which is read every two seconds,
//! so a banner may come that much after the notification. Windows shows a
//! banner of its own as well unless it is told not to in its settings.

use std::collections::HashSet;
use std::time::Duration;

use gpui::{
    App, AsyncApp, Context, CursorStyle, Global, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, SharedString, StatefulInteractiveElement, Styled, Task, WeakEntity, Window,
    WindowHandle, div, point, px, size,
};
use serde::Deserialize;

use crate::motion::Spring;
use crate::platform::{NativeSurface, Notification, Notifications, ScreenRect};
use crate::popup::{MARGIN, open_floating, retire_floating};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{one_line, release_hover};

/// How often the notification center is read.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// How often the time the banners have left is counted down.
const TICK: Duration = Duration::from_millis(200);

/// The width of a banner and the height of the place it takes, which
/// includes the gap below it, in logical pixels.
const WIDTH: f32 = 360.;
const SLOT: f32 = 100.;
const GAP: f32 = 8.;

/// How far to the right a banner starts on its way in.
const TRAVEL: f32 = 48.;

/// How many banners are shown at once; older ones leave for newer ones.
const SHOWN: usize = 4;

/// The seconds a banner stays unless the configuration says otherwise,
/// and the fewest and the most it may say.
const STAY: u32 = 6;
const STAY_RANGE: (u32, u32) = (2, 60);

/// The size of captions, as a share of the label size.
const CAPTION_SHARE: f32 = 0.85;

/// What the configuration says about notifications.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationsConfig {
    /// Whether a notification that arrives is shown as a banner.
    #[serde(default)]
    pub banners: bool,
    /// Whether banners are held back: do not disturb.
    #[serde(default)]
    pub quiet: bool,
    /// How many seconds a banner stays.
    seconds: Option<u32>,
}

impl NotificationsConfig {
    /// How long a banner stays.
    fn stay(&self) -> Duration {
        let (least, most) = STAY_RANGE;
        Duration::from_secs(self.seconds.unwrap_or(STAY).clamp(least, most).into())
    }
}

/// The banners and what looks out for notifications to show.
#[derive(Default)]
pub struct Banners {
    config: NotificationsConfig,
    /// The notifications seen in the notification center so far; none
    /// before it was first read.
    known: Option<HashSet<u32>>,
    /// Whether the last reading failed, which is reported once.
    failed: bool,
    open: Option<WindowHandle<BannerView>>,
    watch: Option<Task<()>>,
}

impl Global for Banners {}

impl Banners {
    /// Whether banners are shown at all, and whether they are held back.
    pub fn state(cx: &mut App) -> (bool, bool) {
        let config = &cx.default_global::<Self>().config;
        (config.banners, config.quiet)
    }

    /// Takes over what the configuration says: the notification center is
    /// watched while banners are wanted.
    pub fn configure(config: &NotificationsConfig, cx: &mut App) {
        let this = cx.default_global::<Self>();
        this.config = config.clone();
        if !config.banners {
            this.watch = None;
            this.known = None;
            return;
        }
        if this.watch.is_some() {
            return;
        }
        let watch = cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                let listed = cx
                    .background_executor()
                    .spawn(async { Notifications::list() })
                    .await;
                if cx.update(|cx| Self::take(listed, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH_INTERVAL).await;
            }
        });
        cx.global_mut::<Self>().watch = Some(watch);
    }

    /// Shows the notifications among `listed` that were not there before,
    /// unless banners are held back.
    fn take(listed: Result<Vec<Notification>, String>, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        let listed = match listed {
            Ok(listed) => listed,
            Err(error) => {
                if !this.failed {
                    report!("there are no banners: {error}");
                    this.failed = true;
                }
                return;
            }
        };
        this.failed = false;
        let arrived = arrived(&mut this.known, listed);
        if arrived.is_empty() || this.config.quiet {
            return;
        }
        let stay = this.config.stay();
        if let Some(open) = this.open {
            let shown = open.update(cx, |view, window, cx| {
                view.show(arrived.clone(), stay, window, cx)
            });
            if matches!(shown, Ok(true)) {
                return;
            }
        }
        Self::open(arrived, stay, cx);
    }

    /// Opens the window of the banners with `arrived` in it, in the upper
    /// right corner of what the bars leave of the display the pointer is
    /// on.
    fn open(arrived: Vec<Notification>, stay: Duration, cx: &mut App) {
        let Some(anchor) = Shell::middle_anchor(cx) else {
            report!("the banners have nowhere to open");
            return;
        };
        let area = anchor.display.and_then(|display| display.work_area());
        let area = area.unwrap_or(anchor.screen);
        let corner = Corner {
            right: area.right,
            top: area.top,
            scale: anchor.scale,
        };
        let frame = corner.frame(arrived.len().min(SHOWN));
        let rough = gpui::Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(px(WIDTH), px(SLOT)),
        };
        let opened = open_floating(rough, anchor.display_id, false, cx, |window, cx| {
            let adopted = NativeSurface::of(window).and_then(|native| {
                native.adopt_as_popup()?;
                native.place(frame)?;
                native.reveal(false);
                Ok(())
            });
            if let Err(error) = adopted {
                report!("the banners are not where they belong: {error:#}");
            }
            BannerView::new(arrived, stay, corner, cx)
        });
        match opened {
            Ok(window) => cx.global_mut::<Self>().open = Some(window),
            Err(error) => report!("the banners did not open: {error:#}"),
        }
    }
}

/// The notifications among `listed` that are not `known`, oldest first;
/// all of `listed` are known afterwards. What the notification center
/// holds when it is first read has arrived before and is not news.
fn arrived(known: &mut Option<HashSet<u32>>, listed: Vec<Notification>) -> Vec<Notification> {
    let now: HashSet<u32> = listed.iter().map(|notification| notification.id).collect();
    let Some(before) = known.replace(now) else {
        return Vec::new();
    };
    let mut arrived: Vec<_> = listed
        .into_iter()
        .filter(|notification| !before.contains(&notification.id))
        .collect();
    arrived.sort_by_key(|notification| notification.posted);
    arrived
}

/// The corner of a display the banners hang in, in physical pixels, and
/// the physical pixels per logical one there.
#[derive(Clone, Copy)]
struct Corner {
    right: i32,
    top: i32,
    scale: f32,
}

impl Corner {
    /// The frame of the window around `count` banners.
    fn frame(self, count: usize) -> ScreenRect {
        let physical = |length: f32| (length * self.scale).round() as i32;
        let width = physical(WIDTH + MARGIN * 2.);
        let height = physical(count.max(1) as f32 * SLOT + MARGIN * 2.);
        ScreenRect {
            left: self.right - width,
            top: self.top,
            right: self.right,
            bottom: self.top + height,
        }
    }
}

/// One banner and the motion it is in.
struct Banner {
    notification: Notification,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    /// How long it still stays.
    left: Duration,
    leaving: bool,
}

/// The root view of the window of the banners.
pub struct BannerView {
    /// Newest first, including banners that are still leaving.
    banners: Vec<Banner>,
    corner: Corner,
    /// For how many banners the window is sized.
    room: usize,
    /// While the pointer is on the banners, none of them leaves.
    hovered: bool,
    /// The window was put away and shows no more banners.
    gone: bool,
}

impl BannerView {
    fn new(
        arrived: Vec<Notification>,
        stay: Duration,
        corner: Corner,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(TICK).await;
                if !matches!(this.update(cx, |this, cx| this.tick(cx)), Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            banners: Vec::new(),
            corner,
            room: arrived.len().min(SHOWN),
            hovered: false,
            gone: false,
        };
        this.add(arrived, stay, cx);
        this
    }

    /// Shows `arrived` above the banners there are. `false` when the
    /// window was put away and another one is needed.
    fn show(
        &mut self,
        arrived: Vec<Notification>,
        stay: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.gone {
            return false;
        }
        self.add(arrived, stay, cx);
        self.fit(window);
        true
    }

    fn add(&mut self, arrived: Vec<Notification>, stay: Duration, cx: &mut Context<Self>) {
        let motion = cx.global::<Theme>().motion;
        for notification in arrived {
            let mut presence = Spring::new(motion.spatial, 0.);
            presence.animate_to(1.);
            self.banners.insert(
                0,
                Banner {
                    notification,
                    presence,
                    left: stay,
                    leaving: false,
                },
            );
        }
        let staying = self.banners.iter_mut().filter(|banner| !banner.leaving);
        for banner in staying.skip(SHOWN) {
            banner.leave();
        }
        cx.notify();
    }

    /// Sizes the window for the banners there are.
    fn fit(&mut self, window: &mut Window) {
        let room = self.banners.len();
        if room == self.room || room == 0 {
            return;
        }
        self.room = room;
        let placed =
            NativeSurface::of(window).and_then(|native| native.place(self.corner.frame(room)));
        if let Err(error) = placed {
            report!("the banners keep their room: {error:#}");
        }
    }

    /// Counts down the time the banners have left. `false` once the
    /// window was put away.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        if self.gone {
            return false;
        }
        if self.hovered {
            return true;
        }
        for banner in self.banners.iter_mut().filter(|banner| !banner.leaving) {
            banner.left = banner.left.saturating_sub(TICK);
            if banner.left.is_zero() {
                banner.leave();
                cx.notify();
            }
        }
        true
    }

    /// Has the banner of the notification `id` leave.
    fn hide(&mut self, id: u32, cx: &mut Context<Self>) {
        let banner = self.banners.iter_mut();
        if let Some(banner) = banner
            .into_iter()
            .find(|banner| banner.notification.id == id)
        {
            banner.leave();
            cx.notify();
        }
    }
}

impl Banner {
    fn leave(&mut self) {
        self.leaving = true;
        self.presence.animate_to(0.);
    }
}

impl Render for BannerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let caption_size = theme.typography.label_size * CAPTION_SHARE;
        let slots: Vec<_> = self
            .banners
            .iter_mut()
            .map(|banner| {
                let presence = banner.presence.value(window);
                let notification = &banner.notification;
                let id = notification.id;
                let caption = (!notification.app.is_empty()).then(|| {
                    div()
                        .flex_none()
                        .text_size(caption_size)
                        .text_color(theme.colors.primary)
                        .child(one_line(notification.app.clone()))
                });
                let title = (!notification.title.is_empty()).then(|| {
                    div()
                        .flex_none()
                        .min_w_0()
                        .text_color(theme.colors.on_surface)
                        .child(one_line(notification.title.clone()))
                });
                let body = (!notification.body.is_empty()).then(|| {
                    div()
                        .line_clamp(if notification.title.is_empty() { 3 } else { 2 })
                        .text_size(caption_size)
                        .text_color(theme.colors.on_surface_variant)
                        .child(SharedString::from(notification.body.clone()))
                });
                let card = div()
                    .id(("banner", id as usize))
                    .relative()
                    .left(px(TRAVEL * (1. - presence)))
                    .w_full()
                    .h(px(SLOT - GAP))
                    .px(theme.spacing.medium)
                    .py(theme.spacing.small)
                    .flex()
                    .flex_col()
                    .justify_center()
                    .overflow_hidden()
                    .rounded(theme.shape.large)
                    .bg(theme.colors.surface_container_highest)
                    .cursor(CursorStyle::PointingHand)
                    .on_click(cx.listener(move |this, _, _, cx| this.hide(id, cx)))
                    .children(caption)
                    .children(title)
                    .children(body);
                // The place includes the gap below the card, so that the
                // cards below move smoothly when one comes or goes.
                div()
                    .w_full()
                    .h(slot_height(presence))
                    .flex_none()
                    .overflow_hidden()
                    .opacity(presence.clamp(0., 1.))
                    .child(card)
            })
            .collect();
        // A banner that has finished leaving was just drawn at no height.
        let before = self.banners.len();
        self.banners
            .retain(|banner| !(banner.leaving && banner.presence.is_settled()));
        if self.banners.is_empty() {
            if !self.gone {
                self.gone = true;
                retire_floating(window, cx);
            }
        } else if self.banners.len() != before {
            cx.on_next_frame(window, |this, window, _| this.fit(window));
        }
        div()
            .id("banners")
            .size_full()
            .p(px(MARGIN))
            .flex()
            .flex_col()
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            .on_hover(cx.listener(|this, hovered: &bool, _, _| this.hovered = *hovered))
            .children(slots)
    }
}

/// The height of the place of a banner that is `presence` there.
fn slot_height(presence: f32) -> Pixels {
    px(SLOT * presence.max(0.))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(id: u32, posted: i64) -> Notification {
        Notification {
            id,
            app: String::new(),
            title: String::new(),
            body: String::new(),
            posted,
        }
    }

    #[test]
    fn what_was_there_at_first_is_no_news() {
        let mut known = None;
        let first = vec![notification(1, 10), notification(2, 20)];
        assert!(arrived(&mut known, first).is_empty());
        let next = vec![
            notification(4, 40),
            notification(3, 30),
            notification(2, 20),
        ];
        let ids: Vec<u32> = arrived(&mut known, next)
            .iter()
            .map(|notification| notification.id)
            .collect();
        // Oldest first, so that the newest ends up on top.
        assert_eq!(ids, [3, 4]);
        // One that was dismissed and one that stays are no news either.
        assert!(arrived(&mut known, vec![notification(4, 40)]).is_empty());
    }

    #[test]
    fn a_banner_stays_as_long_as_the_configuration_says_within_limits() {
        let stay = |seconds| {
            NotificationsConfig {
                seconds,
                ..Default::default()
            }
            .stay()
        };
        assert_eq!(stay(None), Duration::from_secs(6));
        assert_eq!(stay(Some(10)), Duration::from_secs(10));
        assert_eq!(stay(Some(0)), Duration::from_secs(2));
        assert_eq!(stay(Some(600)), Duration::from_secs(60));
    }

    #[test]
    fn the_window_hangs_in_the_corner_and_grows_downwards() {
        let corner = Corner {
            right: 3000,
            top: 40,
            scale: 1.,
        };
        let one = corner.frame(1);
        assert_eq!(
            (one.left, one.top, one.right, one.bottom),
            (2616, 40, 3000, 164)
        );
        let three = corner.frame(3);
        assert_eq!((three.top, three.bottom), (40, 364));
    }
}
