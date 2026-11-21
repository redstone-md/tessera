// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The notifications of the Windows notification center, to read and to
//! dismiss. Widget: `notifications`.

use std::time::Duration;

use chrono::{DateTime, Local};
use gpui::{
    AnyElement, AppContext, AsyncApp, Context, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring};
use crate::platform::{Notification, Notifications};
use crate::theme::Theme;
use crate::ui::{icon, state_layer};

/// How often the notification center is read.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// The height of one notification: a caption, a title and two lines of
/// text. It is fixed so that a notification can grow in and shrink away.
const ITEM_HEIGHT: Pixels = px(108.);

/// The size of captions, as a share of the label size.
const CAPTION_SHARE: f32 = 0.85;

/// The glyph of the button that dismisses a notification.
const DISMISS_GLYPH: &str = "\u{E711}";

pub struct NotificationsModule;

impl Module for NotificationsModule {
    fn name(&self) -> &'static str {
        "notifications"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("notifications", |_, cx| {
            cx.new(NotificationList::new).into()
        })
    }
}

/// One notification and the motion it is in.
struct Item {
    notification: Notification,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    dismiss: Interaction,
    /// It was dismissed and is shrinking away.
    leaving: bool,
}

/// Lists the notifications, newest first.
struct NotificationList {
    /// In the order shown, including notifications that are still leaving.
    items: Vec<Item>,
    /// Why there is nothing to show, if reading failed.
    error: Option<String>,
    clear: Interaction,
}

impl NotificationList {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let listed = cx
                    .background_executor()
                    .spawn(async { Notifications::list() });
                let listed = listed.await;
                if this.update(cx, |this, cx| this.take(listed, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH_INTERVAL).await;
            }
        })
        .detach();
        Self {
            items: Vec::new(),
            error: None,
            clear: Interaction::new(&cx.global::<Theme>().motion),
        }
    }

    /// Takes over what the notification center holds: known notifications
    /// stay, new ones grow in at their place, missing ones start leaving.
    fn take(&mut self, listed: Result<Vec<Notification>, String>, cx: &mut Context<Self>) {
        let listed = match listed {
            Ok(listed) => {
                self.error = None;
                listed
            }
            Err(error) => {
                if self.error.as_ref() != Some(&error) {
                    report!("{error}");
                    self.error = Some(error);
                    cx.notify();
                }
                return;
            }
        };
        let motion = cx.global::<Theme>().motion;
        let mut changed = false;
        for item in &mut self.items {
            let present = listed.iter().any(|new| new.id == item.notification.id);
            if !present && !item.leaving {
                item.leaving = true;
                item.presence.animate_to(0.);
                changed = true;
            }
        }
        // The list is newest first, so a new notification goes before the
        // first older one that is shown.
        for notification in listed {
            if let Some(item) = self
                .items
                .iter_mut()
                .find(|item| item.notification.id == notification.id)
            {
                if item.notification != notification {
                    item.notification = notification;
                    changed = true;
                }
                continue;
            }
            let at = self
                .items
                .iter()
                .position(|item| item.notification.posted < notification.posted)
                .unwrap_or(self.items.len());
            let mut presence = Spring::new(motion.spatial, 0.);
            presence.animate_to(1.);
            self.items.insert(
                at,
                Item {
                    notification,
                    presence,
                    dismiss: Interaction::new(&motion),
                    leaving: false,
                },
            );
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// Removes the notification `id` from the notification center; it
    /// starts leaving at once.
    fn dismiss(&mut self, id: u32, cx: &mut Context<Self>) {
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|item| item.notification.id == id)
        {
            item.leaving = true;
            item.presence.animate_to(0.);
        }
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = Notifications::dismiss(id) {
                    report!("{error}");
                }
            })
            .detach();
        cx.notify();
    }

    /// Removes every notification from the notification center.
    fn clear(&mut self, cx: &mut Context<Self>) {
        for item in &mut self.items {
            item.leaving = true;
            item.presence.animate_to(0.);
        }
        cx.background_executor()
            .spawn(async {
                if let Err(error) = Notifications::clear() {
                    report!("{error}");
                }
            })
            .detach();
        cx.notify();
    }

    fn render_item(
        item: &mut Item,
        now: i64,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let presence = item.presence.value(window);
        let state = item.dismiss.state(window);
        let notification = &item.notification;
        let id = notification.id;
        let caption_size = theme.typography.label_size * CAPTION_SHARE;

        let dismiss = icon(&theme, DISMISS_GLYPH)
            .id(("dismiss", id as usize))
            .size(theme.typography.icon_size + theme.spacing.small)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .text_size(caption_size)
            .text_color(theme.colors.on_surface_variant)
            .bg(state_layer(
                &theme,
                theme.colors.surface_container_highest,
                state,
            ))
            .on_click(cx.listener(move |this, _, _, cx| this.dismiss(id, cx)));
        let dismiss = Interaction::track(dismiss, cx, move |this: &mut Self| {
            let item = this
                .items
                .iter_mut()
                .find(|item| item.notification.id == id)?;
            Some(&mut item.dismiss)
        });

        let caption = match (notification.app.as_str(), age(now - notification.posted)) {
            ("", age) => age,
            (app, age) => format!("{app} · {age}"),
        };
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(caption_size)
                    .text_color(theme.colors.primary)
                    .child(SharedString::from(caption)),
            )
            .child(dismiss);
        let title = (!notification.title.is_empty()).then(|| {
            div()
                .flex_none()
                .truncate()
                .text_color(theme.colors.on_surface)
                .child(SharedString::from(notification.title.clone()))
        });
        let body = (!notification.body.is_empty()).then(|| {
            div()
                .line_clamp(if notification.title.is_empty() { 3 } else { 2 })
                .text_size(caption_size)
                .text_color(theme.colors.on_surface_variant)
                .child(SharedString::from(notification.body.clone()))
        });
        let card = div()
            .h(ITEM_HEIGHT - theme.spacing.small)
            .px(theme.spacing.medium - theme.spacing.tiny)
            .py(theme.spacing.small)
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container_highest)
            .child(header)
            .children(title)
            .children(body);

        // The slot includes the gap below the card, so that the cards
        // below move smoothly when one comes or goes.
        div()
            .w_full()
            .h(ITEM_HEIGHT * presence.max(0.))
            .flex_none()
            .overflow_hidden()
            .opacity(presence.clamp(0., 1.))
            .child(card)
            .into_any_element()
    }
}

/// How long ago something happened `seconds` ago, in a few characters.
fn age(seconds: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    match seconds {
        ..MINUTE => "now".to_owned(),
        MINUTE..HOUR => format!("{} min", seconds / MINUTE),
        HOUR..DAY => format!("{} h", seconds / HOUR),
        _ => format!("{} d", seconds / DAY),
    }
}

impl Render for NotificationList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let now = DateTime::<Local>::from(std::time::SystemTime::now()).timestamp();
        let cards: Vec<AnyElement> = self
            .items
            .iter_mut()
            .map(|item| Self::render_item(item, now, theme, window, cx))
            .collect();
        // A notification that has finished leaving was just drawn at no
        // height.
        self.items
            .retain(|item| !(item.leaving && item.presence.is_settled()));
        let waiting = self.items.iter().filter(|item| !item.leaving).count();

        let state = self.clear.state(window);
        let clear = div()
            .id("clear-notifications")
            .px(theme.spacing.small + theme.spacing.tiny)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .text_color(theme.colors.primary)
            .bg(state_layer(&theme, theme.colors.surface_container, state))
            .on_click(cx.listener(|this, _, _, cx| this.clear(cx)))
            .child("Clear all");
        let clear = Interaction::track(clear, cx, |this: &mut Self| Some(&mut this.clear));
        let header = div()
            .h(theme.bar.pill_height)
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .text_color(theme.colors.on_surface)
            .child("Notifications")
            .children((waiting > 0).then_some(clear));

        let note = |text: SharedString| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.colors.on_surface_variant)
                .child(text)
                .into_any_element()
        };
        let list = match (&self.error, cards.is_empty()) {
            (Some(error), _) => note(SharedString::from(error.clone())),
            (None, true) => note("No notifications".into()),
            (None, false) => div()
                .id("notification-list")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(div().w_full().flex().flex_col().children(cards))
                .into_any_element(),
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .text_size(theme.typography.label_size)
            .child(header)
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_age_is_told_in_the_largest_fitting_unit() {
        assert_eq!(age(-5), "now");
        assert_eq!(age(59), "now");
        assert_eq!(age(60), "1 min");
        assert_eq!(age(3_599), "59 min");
        assert_eq!(age(7_200), "2 h");
        assert_eq!(age(3 * 86_400 + 5), "3 d");
    }
}
