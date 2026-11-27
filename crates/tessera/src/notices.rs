// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Notices of Tessera itself, given where Explorer gives them and nothing
//! else does once Explorer is not the shell: a drive that was plugged in,
//! a device that may now be pulled out or could not be ejected, a battery
//! running low.
//!
//! A notice comes as a card in the lower right corner of what the bars
//! leave of the display the pointer is on, where Windows shows its own
//! notifications, with buttons for what can be done about it; a click on
//! the card does what its first button does. The banners of `banners.rs`
//! show the notifications of the notification center of Windows and have
//! no way yet to take one of Tessera's own, so notices have this window of
//! their own. [`Notice`] and [`NoticeAction`] are made so that a notice can
//! be handed to the banners as it is once they take them.
//!
//! Do not disturb (`quiet` in `[notifications]`) holds notices back but
//! for urgent ones, such as a battery about to run out, which also stay
//! until they are put away.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AsyncApp, Context, CursorStyle, Global, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Window, WindowHandle, div, point, px,
    size,
};
use serde::Deserialize;

use crate::banners::Banners;
use crate::modules::devices::Drives;
use crate::motion::{Interaction, Spring};
use crate::platform::{NativeSurface, ScreenRect, shell_mode};
use crate::popup::{MARGIN, open_floating, retire_floating};
use crate::power_warnings::PowerWarnings;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, one_line, release_hover, state_layer};

/// How often the time the notices have left is counted down.
const TICK: Duration = Duration::from_millis(200);

/// The width of a notice and the height of the place it takes, which
/// includes the gap above it, in logical pixels.
const WIDTH: f32 = 360.;
const SLOT: f32 = 128.;
const GAP: f32 = 8.;

/// How far to the right a notice starts on its way in.
const TRAVEL: f32 = 48.;

/// How many notices are shown at once; older ones leave for newer ones.
const SHOWN: usize = 3;

/// The seconds a notice that is not urgent stays unless the configuration
/// says otherwise, and the fewest and the most it may say.
const STAY: u32 = 8;
const STAY_RANGE: (u32, u32) = (2, 60);

/// The size of captions, as a share of the label size.
const CAPTION_SHARE: f32 = 0.85;

/// What stands for Tessera on a notice that brings no glyph of its own.
const ICON_NOTICE: &str = "\u{e7e7}";
const ICON_CLOSE: &str = "\u{e711}";

/// What the configuration says about the notices Tessera gives in place of
/// Explorer, under `[notices]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoticesConfig {
    /// Whether a drive that is plugged in is told of, with a way to open
    /// it; without the key, only while Tessera is the shell.
    drives: Option<bool>,
    /// Whether a battery running low is warned of at the levels of the
    /// power plan; without the key, only while Tessera is the shell.
    battery: Option<bool>,
    /// How many seconds a notice that is not urgent stays.
    seconds: Option<u32>,
}

impl NoticesConfig {
    /// Whether drives that arrive are told of.
    pub fn drives(&self) -> bool {
        self.drives.unwrap_or_else(shell_mode)
    }

    /// Whether a battery running low is warned of.
    pub fn battery(&self) -> bool {
        self.battery.unwrap_or_else(shell_mode)
    }

    /// How long a notice that is not urgent stays.
    fn stay(&self) -> Duration {
        let (least, most) = STAY_RANGE;
        Duration::from_secs(self.seconds.unwrap_or(STAY).clamp(least, most).into())
    }
}

/// A button of a notice and what it does.
#[derive(Clone)]
pub struct NoticeAction {
    label: SharedString,
    run: Rc<dyn Fn(&mut App)>,
}

impl NoticeAction {
    pub fn new(label: impl Into<SharedString>, run: impl Fn(&mut App) + 'static) -> Self {
        Self {
            label: label.into(),
            run: Rc::new(run),
        }
    }
}

/// A notice of Tessera itself, to post with [`Notices::post`].
#[derive(Clone, Default)]
pub struct Notice {
    /// Who tells: the part of Tessera, such as "Devices".
    pub app: String,
    pub title: String,
    pub body: String,
    /// A character of the icon font shown beside it.
    pub glyph: Option<&'static str>,
    /// Its buttons; a click on the card does what the first one does.
    pub actions: Vec<NoticeAction>,
    /// Whether it comes even while do not disturb holds notices back, and
    /// stays until it is put away, as a warning of an empty battery should.
    pub urgent: bool,
}

/// The notices and what the configuration says of them.
#[derive(Default)]
pub struct Notices {
    config: NoticesConfig,
    open: Option<WindowHandle<NoticeView>>,
}

impl Global for Notices {}

impl Notices {
    /// Takes over what the configuration says: drives that arrive and
    /// batteries that run low are looked out for while they are to be told
    /// of.
    pub fn configure(config: &NoticesConfig, cx: &mut App) {
        cx.default_global::<Self>().config = config.clone();
        Drives::tell_arrivals(config.drives(), cx);
        PowerWarnings::configure(config.battery(), cx);
    }

    /// Shows `notice`, unless do not disturb holds it back.
    pub fn post(notice: Notice, cx: &mut App) {
        let (_, quiet) = Banners::state(cx);
        if quiet && !notice.urgent {
            return;
        }
        let stay = (!notice.urgent).then(|| cx.default_global::<Self>().config.stay());
        if let Some(open) = cx.global::<Self>().open {
            let shown = open.update(cx, |view, window, cx| {
                view.show(notice.clone(), stay, window, cx)
            });
            if matches!(shown, Ok(true)) {
                return;
            }
        }
        Self::open(notice, stay, cx);
    }

    /// Opens the window of the notices with `notice` in it, in the lower
    /// right corner of what the bars leave of the display the pointer is
    /// on.
    fn open(notice: Notice, stay: Option<Duration>, cx: &mut App) {
        let Some(anchor) = Shell::middle_anchor(cx) else {
            report!("a notice has nowhere to open: {}", notice.title);
            return;
        };
        let area = anchor.display.and_then(|display| display.work_area());
        let area = area.unwrap_or(anchor.screen);
        let corner = Corner {
            right: area.right,
            bottom: area.bottom,
            scale: anchor.scale,
        };
        let frame = corner.frame(1);
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
                report!("the notices are not where they belong: {error:#}");
            }
            NoticeView::new(notice, stay, corner, cx)
        });
        match opened {
            Ok(window) => cx.global_mut::<Self>().open = Some(window),
            Err(error) => report!("a notice did not open: {error:#}"),
        }
    }
}

/// The corner of a display the notices stand in, in physical pixels, and
/// the physical pixels per logical one there.
#[derive(Clone, Copy)]
struct Corner {
    right: i32,
    bottom: i32,
    scale: f32,
}

impl Corner {
    /// The frame of the window around `count` notices, which grows upward
    /// from the corner.
    fn frame(self, count: usize) -> ScreenRect {
        let physical = |length: f32| (length * self.scale).round() as i32;
        let width = physical(WIDTH + MARGIN * 2.);
        let height = physical(count.max(1) as f32 * SLOT + MARGIN * 2.);
        ScreenRect {
            left: self.right - width,
            top: self.bottom - height,
            right: self.right,
            bottom: self.bottom,
        }
    }
}

/// One notice and the motion it is in.
struct Shown {
    notice: Notice,
    /// Tells the notices apart in the elements that draw them.
    number: usize,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    /// How long it still stays; `None` for one that stays until it is put
    /// away.
    left: Option<Duration>,
    leaving: bool,
    /// The card, its buttons and its cross.
    card: Interaction,
    buttons: Vec<Interaction>,
    close: Interaction,
}

impl Shown {
    fn leave(&mut self) {
        self.leaving = true;
        self.presence.animate_to(0.);
    }
}

/// The root view of the window of the notices.
pub struct NoticeView {
    /// Oldest first, including notices that are still leaving.
    shown: Vec<Shown>,
    corner: Corner,
    /// For how many notices the window is sized.
    room: usize,
    /// While the pointer is on the notices, none of them leaves.
    hovered: bool,
    /// The window was put away and shows no more notices.
    gone: bool,
    /// The number the next notice gets.
    next: usize,
}

impl NoticeView {
    fn new(notice: Notice, stay: Option<Duration>, corner: Corner, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(TICK).await;
                if !matches!(this.update(cx, |this, cx| this.tick(cx)), Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            shown: Vec::new(),
            corner,
            room: 1,
            hovered: false,
            gone: false,
            next: 0,
        };
        this.add(notice, stay, cx);
        this
    }

    /// Shows `notice` after those there are. `false` when the window was
    /// put away and another one is needed.
    fn show(
        &mut self,
        notice: Notice,
        stay: Option<Duration>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.gone {
            return false;
        }
        self.add(notice, stay, cx);
        self.fit(window);
        true
    }

    fn add(&mut self, notice: Notice, stay: Option<Duration>, cx: &mut Context<Self>) {
        let motion = cx.global::<Theme>().motion;
        let mut presence = Spring::new(motion.spatial, 0.);
        presence.animate_to(1.);
        let buttons = notice
            .actions
            .iter()
            .map(|_| Interaction::new(&motion))
            .collect();
        self.shown.push(Shown {
            notice,
            number: self.next,
            presence,
            left: stay,
            leaving: false,
            card: Interaction::new(&motion),
            buttons,
            close: Interaction::new(&motion),
        });
        self.next += 1;
        let staying = self.shown.iter().filter(|shown| !shown.leaving).count();
        let over = staying.saturating_sub(SHOWN);
        let oldest = self.shown.iter_mut().filter(|shown| !shown.leaving);
        for shown in oldest.take(over) {
            shown.leave();
        }
        cx.notify();
    }

    /// Sizes the window for the notices there are.
    fn fit(&mut self, window: &mut Window) {
        let room = self.shown.len();
        if room == self.room || room == 0 {
            return;
        }
        self.room = room;
        let placed =
            NativeSurface::of(window).and_then(|native| native.place(self.corner.frame(room)));
        if let Err(error) = placed {
            report!("the notices keep their room: {error:#}");
        }
    }

    /// Counts down the time the notices have left. `false` once the window
    /// was put away.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        if self.gone {
            return false;
        }
        if self.hovered {
            return true;
        }
        for shown in self.shown.iter_mut().filter(|shown| !shown.leaving) {
            let Some(left) = &mut shown.left else {
                continue;
            };
            *left = left.saturating_sub(TICK);
            if left.is_zero() {
                shown.leave();
                cx.notify();
            }
        }
        true
    }

    /// Has the notice `number` leave, after doing its action `action` if
    /// one is named.
    fn answer(&mut self, number: usize, action: Option<usize>, cx: &mut Context<Self>) {
        let Some(shown) = self.shown.iter_mut().find(|shown| shown.number == number) else {
            return;
        };
        if let Some(action) = action.and_then(|index| shown.notice.actions.get(index)) {
            let run = action.run.clone();
            // The action may open windows, which is done outside the
            // update of this one.
            cx.defer(move |cx| run(cx));
        }
        shown.leave();
        cx.notify();
    }

    fn render_notice(
        &mut self,
        index: usize,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let caption_size = theme.typography.label_size * CAPTION_SHARE;
        let shown = &mut self.shown[index];
        let presence = shown.presence.value(window);
        let number = shown.number;
        let card_state = shown.card.state(window);
        let close_state = shown.close.state(window);
        let button_states: Vec<_> = shown
            .buttons
            .iter_mut()
            .map(|button| button.state(window))
            .collect();
        let notice = &shown.notice;
        let first = (!notice.actions.is_empty()).then_some(0);
        let ground = theme.colors.surface_container_highest;
        let close = div()
            .id(("notice-close", number))
            .flex_none()
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(theme, ground, close_state))
            .text_color(theme.colors.on_surface_variant)
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.answer(number, None, cx);
            }))
            .child(icon(theme, ICON_CLOSE).text_size(px(12.)));
        let close = Interaction::track(close, cx, move |this: &mut Self| {
            let shown = this.shown.iter_mut().find(|shown| shown.number == number)?;
            Some(&mut shown.close)
        });
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(
                icon(theme, notice.glyph.unwrap_or(ICON_NOTICE))
                    .flex_none()
                    .text_color(theme.colors.primary),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(caption_size)
                    .text_color(theme.colors.primary)
                    .child(one_line(notice.app.clone())),
            )
            .child(close);
        let title = div()
            .flex_none()
            .min_w_0()
            .text_color(theme.colors.on_surface)
            .child(one_line(notice.title.clone()));
        let body = (!notice.body.is_empty()).then(|| {
            div()
                .line_clamp(2)
                .text_size(caption_size)
                .text_color(theme.colors.on_surface_variant)
                .child(SharedString::from(notice.body.clone()))
        });
        let buttons: Vec<_> = notice
            .actions
            .iter()
            .enumerate()
            .map(|(slot, action)| {
                let state = button_states[slot];
                let button = div()
                    .id(("notice-button", number * 8 + slot))
                    .flex_none()
                    .px(theme.spacing.small + theme.spacing.tiny)
                    .py(theme.spacing.tiny)
                    .rounded(theme.shape.full)
                    .text_color(theme.colors.primary)
                    .bg(state_layer(theme, theme.colors.secondary_container, state))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.answer(number, Some(slot), cx);
                    }))
                    .child(one_line(action.label.clone()));
                Interaction::track(button, cx, move |this: &mut Self| {
                    let shown = this.shown.iter_mut().find(|shown| shown.number == number)?;
                    shown.buttons.get_mut(slot)
                })
            })
            .collect();
        let row = (!buttons.is_empty()).then(|| {
            div()
                .flex_none()
                .pt(theme.spacing.tiny)
                .flex()
                .gap(theme.spacing.small)
                .children(buttons)
        });
        let urgent = notice.urgent.then(|| {
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(4.))
                .bg(theme.colors.primary)
        });
        let card = div()
            .id(("notice", number))
            .relative()
            .left(px(TRAVEL * (1. - presence)))
            .w_full()
            .h(px(SLOT - GAP))
            .px(theme.spacing.medium)
            .py(theme.spacing.small)
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(2.))
            .overflow_hidden()
            .rounded(theme.shape.large)
            .bg(state_layer(theme, ground, card_state))
            .cursor(CursorStyle::PointingHand)
            .on_click(cx.listener(move |this, _, _, cx| this.answer(number, first, cx)))
            .children(urgent)
            .child(header)
            .child(title)
            .children(body)
            .children(row);
        let card = Interaction::track(card, cx, move |this: &mut Self| {
            let shown = this.shown.iter_mut().find(|shown| shown.number == number)?;
            Some(&mut shown.card)
        });
        // The place includes the gap above the card, so that the cards
        // above move smoothly when one comes or goes.
        div()
            .w_full()
            .h(px(SLOT * presence.max(0.)))
            .flex_none()
            .flex()
            .flex_col()
            .justify_end()
            .overflow_hidden()
            .opacity(presence.clamp(0., 1.))
            .child(card)
    }
}

impl Render for NoticeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let cards: Vec<_> = (0..self.shown.len())
            .map(|index| {
                self.render_notice(index, &theme, window, cx)
                    .into_any_element()
            })
            .collect();
        // A notice that has finished leaving was just drawn at no height.
        let before = self.shown.len();
        self.shown
            .retain(|shown| !(shown.leaving && shown.presence.is_settled()));
        if self.shown.is_empty() {
            if !self.gone {
                self.gone = true;
                retire_floating(window, cx);
            }
        } else if self.shown.len() != before {
            cx.on_next_frame(window, |this, window, _| this.fit(window));
        }
        div()
            .id("notices")
            .size_full()
            .p(px(MARGIN))
            .flex()
            .flex_col()
            .justify_end()
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            .on_hover(cx.listener(|this, hovered: &bool, _, _| this.hovered = *hovered))
            .children(cards)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_stays_as_long_as_the_configuration_says_within_limits() {
        let stay = |seconds| {
            NoticesConfig {
                seconds,
                ..Default::default()
            }
            .stay()
        };
        assert_eq!(stay(None), Duration::from_secs(8));
        assert_eq!(stay(Some(20)), Duration::from_secs(20));
        assert_eq!(stay(Some(0)), Duration::from_secs(2));
        assert_eq!(stay(Some(600)), Duration::from_secs(60));
    }

    #[test]
    fn what_is_told_of_can_be_chosen() {
        let config: NoticesConfig =
            toml::from_str("drives = false\nbattery = true\n").expect("a valid table");
        assert!(!config.drives());
        assert!(config.battery());
        assert!(toml::from_str::<NoticesConfig>("sound = true\n").is_err());
    }

    #[test]
    fn the_window_stands_in_the_corner_and_grows_upward() {
        let corner = Corner {
            right: 3000,
            bottom: 2000,
            scale: 1.,
        };
        let one = corner.frame(1);
        assert_eq!(
            (one.left, one.top, one.right, one.bottom),
            (2616, 1848, 3000, 2000)
        );
        let two = corner.frame(2);
        assert_eq!((two.top, two.bottom), (1720, 2000));
    }
}
