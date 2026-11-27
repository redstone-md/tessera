// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The snap assistant: once a window took part of a display, the other
//! windows are offered for the rest, one part after another. A click on
//! one puts it there; Esc, or a click elsewhere, leaves the rest as it is.
//! A click elsewhere is also seen when Windows did not give the assistant
//! the keyboard, and so is another window coming in front.

use std::time::Duration;

use gpui::{
    App, AsyncApp, Context, CursorStyle, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, WindowHandle, div, img, px,
};

use super::{Kind, Snapper, move_floating, open_at};
use crate::i18n::Language;
use crate::icons::{AppIcons, Icon};
use crate::motion::{Interaction, Spring, staggered};
use crate::platform::{
    AppWindow, AppWindowId, Display, ScreenEdge, ScreenRect, pointer_at, pointer_held,
};
use crate::popup::{floating_panel, retire_floating};
use crate::theme::Theme;
use crate::tiling::Tiler;
use crate::ui::{one_line, release_hover, state_layer};

/// How many windows are offered at most: the ones used last.
const OFFERED: usize = 12;

/// The width and the height of the tile of a window, and the side of its
/// icon, in logical pixels.
const TILE: (f32, f32) = (152., 104.);
const ICON: f32 = 40.;

/// How often it is looked whether the user turned to something else.
const LOOK: Duration = Duration::from_millis(100);

/// A window that is offered.
struct Entry {
    id: AppWindowId,
    title: SharedString,
    icon: Icon,
}

/// The root view of the window of the assistant.
pub struct AssistView {
    /// The parts of the display still to fill, the one offered for first.
    zones: Vec<ScreenRect>,
    entries: Vec<Entry>,
    tiles: Vec<Interaction>,
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

impl AssistView {
    /// Offers the windows other than `placed` for `zones` of `display`,
    /// over the first of them; `None` when there is no other window to
    /// offer.
    pub fn offer(
        placed: AppWindowId,
        display: Display,
        zones: Vec<ScreenRect>,
        cx: &mut App,
    ) -> anyhow::Result<Option<WindowHandle<Self>>> {
        let Some(&first) = zones.first() else {
            return Ok(None);
        };
        let icons = cx.global::<AppIcons>();
        let entries: Vec<Entry> = { AppWindow::enumerate().into_iter() }
            .filter(|window| window.id != placed)
            .filter(|window| !Tiler::manages(window.id, cx))
            .take(OFFERED)
            .map(|window| Entry {
                id: window.id,
                title: window.id.title().into(),
                icon: window
                    .program()
                    .and_then(|program| icons.of_executable(&program)),
            })
            .collect();
        if entries.is_empty() {
            return Ok(None);
        }
        let opened = open_at(first, &display, Kind::Popup, cx, move |window, cx| {
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
            Self::close_when_turned_away(placed, cx);
            Self {
                zones,
                tiles: entries.iter().map(|_| Interaction::new(&motion)).collect(),
                entries,
                visibility,
                closing: false,
                was_active: false,
                focus: cx.focus_handle(),
                focused: false,
                _activation: activation,
            }
        });
        opened.map(Some)
    }

    /// Closes the assistant once a button of the pointer is pressed away
    /// from it, or a window of another program than Tessera comes in front
    /// other than `placed`, which was in front when it opened. Windows may
    /// not have given it the keyboard, so that it is never left and Esc
    /// does not reach it.
    fn close_when_turned_away(placed: AppWindowId, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(LOOK).await;
                let front = AppWindowId::foreground();
                let elsewhere = front
                    .is_some_and(|front| front != placed && front.process() != std::process::id());
                let pointer = pointer_at().filter(|_| pointer_held());
                let going = this.update(cx, |this, cx| {
                    let away = pointer
                        .zip(this.zones.first())
                        .is_some_and(|((x, y), zone)| {
                            !((zone.left..zone.right).contains(&x)
                                && (zone.top..zone.bottom).contains(&y))
                        });
                    if elsewhere || away {
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
        if self.closing {
            return;
        }
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            self.close(cx);
        }
    }

    /// Puts the window of the entry at `index` into the part offered for,
    /// and offers the rest for the next part, if there is one.
    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing || index >= self.entries.len() || self.zones.is_empty() {
            return;
        }
        let entry = self.entries.remove(index);
        self.tiles.remove(index);
        let zone = self.zones.remove(0);
        let id = entry.id;
        cx.defer(move |cx| Snapper::place_chosen(id, zone, cx));
        match self.zones.first() {
            Some(&next) if !self.entries.is_empty() => move_floating(window, next, cx),
            _ => {
                // The last window chosen comes in front once it is placed.
                cx.defer(move |_| id.activate());
                self.close(cx);
            }
        }
        cx.notify();
    }
}

impl Render for AssistView {
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
        let count = self.entries.len();
        let tiles: Vec<_> = (0..count)
            .map(|index| {
                let entry = &self.entries[index];
                let state = self.tiles[index].state(window);
                let arrival = staggered(visibility, index, count);
                let icon = match entry.icon.clone() {
                    Some(icon) => img(icon).size(px(ICON)).flex_none().into_any_element(),
                    None => div()
                        .size(px(ICON))
                        .flex_none()
                        .rounded(theme.shape.small)
                        .bg(theme.colors.outline)
                        .into_any_element(),
                };
                let tile = div()
                    .id(("snap-assist", index))
                    .w(px(TILE.0))
                    .h(px(TILE.1))
                    .flex_none()
                    .px(theme.spacing.small)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(theme.spacing.small)
                    .rounded(theme.shape.large)
                    .bg(state_layer(
                        &theme,
                        theme.colors.surface_container_highest,
                        state,
                    ))
                    .opacity(arrival)
                    .cursor(CursorStyle::PointingHand)
                    .child(icon)
                    .child(div().max_w_full().child(one_line(entry.title.clone())));
                let tile =
                    Interaction::track(tile, cx, move |this: &mut Self| this.tiles.get_mut(index));
                tile.on_click(
                    cx.listener(move |this, _, window, cx| this.choose(index, window, cx)),
                )
            })
            .collect();
        let heading = Language::of(cx).say("Choose a window for this space");
        floating_panel(&theme, ScreenEdge::Top, visibility, |panel| {
            panel
                .bg(theme.colors.surface_container.opacity(0.92))
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.colors.on_surface_variant)
                        .child(SharedString::new_static(heading)),
                )
                .child(
                    div()
                        .id("snap-assist-tiles")
                        .track_focus(&self.focus)
                        .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_wrap()
                        .content_start()
                        .justify_center()
                        .gap(theme.spacing.small)
                        .children(tiles),
                )
        })
    }
}
