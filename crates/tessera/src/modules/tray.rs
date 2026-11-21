// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The icons applications keep in the notification area. Widget: `tray`.

use std::time::Duration;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity,
    Window, div, img, px,
};

use crate::icons::{Icon, icon_of_pixels};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring};
use crate::platform::{Tray, TrayButton, TrayIcon};
use crate::theme::Theme;
use crate::ui::state_layer;

/// How often the icons are checked for changes.
const REFRESH_INTERVAL: Duration = Duration::from_millis(250);

/// The side of an icon, as a share of the height of a pill.
const ICON_SHARE: f32 = 0.5;

/// How much a pressed icon shrinks, as a share of its size.
const PRESS_SHRINK: f32 = 0.16;

/// The widest the description of the icon under the pointer gets.
const TIP_WIDTH: Pixels = px(260.);

/// The longest description shown, in characters.
const TIP_LENGTH: usize = 48;

pub struct TrayModule;

impl Module for TrayModule {
    fn name(&self) -> &'static str {
        "tray"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("tray", |context, cx| {
            let tips = match context.text("tips") {
                None => None,
                Some("left") => Some(TipSide::Left),
                Some("right") => Some(TipSide::Right),
                Some(other) => {
                    eprintln!(
                        "tessera: the `tips` of a tray must be \"left\" or \"right\",                          `{other}` is ignored"
                    );
                    None
                }
            };
            cx.new(|cx| TrayStrip::new(tips, cx)).into()
        })
    }
}

/// The end of the strip at which the description of the icon under the
/// pointer unrolls. It must be the end that can grow without moving the
/// icons, or they would slide away from the pointer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TipSide {
    Left,
    Right,
}

/// One icon and the motion it is in.
struct TrayItem {
    icon: TrayIcon,
    image: Icon,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    interaction: Interaction,
    /// The application removed the icon and it is shrinking away.
    leaving: bool,
}

/// Shows the notification area and passes clicks on to the applications.
struct TrayStrip {
    /// In the order shown, including icons that are still leaving.
    items: Vec<TrayItem>,
    /// The state of the notification area the items were taken from.
    generation: Option<u64>,
    /// Where descriptions are shown, if at all.
    tips: Option<TipSide>,
}

impl TrayStrip {
    fn new(tips: Option<TipSide>, cx: &mut Context<Self>) -> Self {
        match Tray::start() {
            Ok(()) => cx
                .spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    loop {
                        cx.background_executor().timer(REFRESH_INTERVAL).await;
                        if this.update(cx, Self::refresh).is_err() {
                            break;
                        }
                    }
                })
                .detach(),
            Err(error) => eprintln!("tessera: the notification area stays empty: {error}"),
        }
        Self {
            items: Vec::new(),
            generation: None,
            tips,
        }
    }

    /// Takes over the icons if they changed: known ones keep their place,
    /// new ones are appended, removed ones start leaving.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.generation == Some(Tray::generation()) {
            return;
        }
        let (generation, mut icons) = Tray::icons();
        self.generation = Some(generation);
        let motion = cx.global::<Theme>().motion;

        for item in &mut self.items {
            let found = icons.iter().position(|icon| icon.id == item.icon.id);
            let present = found.is_some();
            if let Some(icon) = found.map(|index| icons.remove(index)) {
                if icon.image_version != item.icon.image_version {
                    item.image = image_of(&icon);
                }
                item.icon = icon;
            }
            if item.leaving == present {
                item.leaving = !present;
                item.presence.animate_to(if present { 1. } else { 0. });
            }
        }
        for icon in icons {
            let mut presence = Spring::new(motion.spatial, 0.);
            presence.animate_to(1.);
            self.items.push(TrayItem {
                image: image_of(&icon),
                icon,
                presence,
                interaction: Interaction::new(&motion),
                leaving: false,
            });
        }
        cx.notify();
    }

    fn render_item(
        item: &mut TrayItem,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let presence = item.presence.value(window);
        let state = item.interaction.state(window);
        let id = item.icon.id;
        let side = theme.bar.pill_height * ICON_SHARE;
        let icon_side = side * (1. - PRESS_SHRINK * state.press);
        let button_side = side + theme.spacing.small;

        let button = div()
            .id(("tray-icon", id as usize))
            .size(button_side)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(&theme, theme.colors.surface_container, state))
            .on_click(move |_, _, _| Tray::press(id, TrayButton::Left))
            .on_mouse_up(MouseButton::Right, move |_, _, _| {
                Tray::press(id, TrayButton::Right);
            })
            .child(match item.image.clone() {
                Some(image) => img(image).size(icon_side).into_any_element(),
                None => div()
                    .size(icon_side)
                    .rounded(theme.shape.small)
                    .bg(theme.colors.outline)
                    .into_any_element(),
            });
        let button = Interaction::track(button, cx, move |this: &mut Self| {
            let item = this.items.iter_mut().find(|item| item.icon.id == id)?;
            Some(&mut item.interaction)
        });

        // The slot includes the spacing between icons, so that an icon that
        // comes or goes moves its neighbors smoothly.
        let slot = button_side + theme.spacing.tiny;
        div()
            .w(slot * presence.max(0.))
            .h(button_side)
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .opacity(presence.clamp(0., 1.))
            .child(button)
            .into_any_element()
    }

    /// The description of the icon the pointer is on, as wide as the
    /// pointer has settled there.
    fn render_tip(&mut self, theme: Theme, window: &Window) -> Option<AnyElement> {
        let side = self.tips?;
        let (reveal, item) = self
            .items
            .iter_mut()
            .map(|item| (item.interaction.state(window).hover.clamp(0., 1.), &*item))
            .max_by(|a, b| a.0.total_cmp(&b.0))?;
        let tip = tip_line(&item.icon.tip);
        if tip.is_empty() || reveal < 0.01 {
            return None;
        }
        let label = div()
            .max_w(TIP_WIDTH * reveal)
            .overflow_hidden()
            .whitespace_nowrap()
            .opacity(reveal)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface_variant)
            .child(SharedString::from(tip));
        let gap = theme.spacing.small * reveal;
        Some(match side {
            TipSide::Left => label.pl(gap).pr(gap / 2.).into_any_element(),
            TipSide::Right => label.pr(gap).pl(gap / 2.).into_any_element(),
        })
    }
}

/// The first line of an icon's description, shortened to fit a bar.
fn tip_line(tip: &str) -> String {
    let line = tip.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= TIP_LENGTH {
        return line.to_owned();
    }
    let kept: String = line.chars().take(TIP_LENGTH - 1).collect();
    format!("{}…", kept.trim_end())
}

fn image_of(icon: &TrayIcon) -> Icon {
    icon_of_pixels(icon.pixels.as_deref()?)
}

impl Render for TrayStrip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let elements: Vec<AnyElement> = self
            .items
            .iter_mut()
            .map(|item| Self::render_item(item, theme, window, cx))
            .collect();
        // An icon that has finished leaving was just drawn at no width.
        self.items
            .retain(|item| !(item.leaving && item.presence.is_settled()));

        if elements.is_empty() {
            return div();
        }
        let tip = self.render_tip(theme, window);
        let (before, after) = match self.tips {
            Some(TipSide::Left) => (tip, None),
            _ => (None, tip),
        };
        div()
            .h(theme.bar.pill_height)
            .px(theme.spacing.small - theme.spacing.tiny / 2.)
            .flex()
            .items_center()
            .rounded(theme.shape.full)
            .bg(theme.colors.surface_container)
            .children(before)
            .children(elements)
            .children(after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_is_cut_to_one_short_line() {
        assert_eq!(tip_line(""), "");
        assert_eq!(
            tip_line(
                "Volume: 40%
Speakers"
            ),
            "Volume: 40%"
        );
        let long = "word ".repeat(20);
        let shown = tip_line(&long);
        assert_eq!(shown.chars().count(), TIP_LENGTH);
        assert!(shown.ends_with("wo…"), "got {shown:?}");
    }
}
