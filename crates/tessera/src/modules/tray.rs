// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The icons applications keep in the notification area. Widget: `tray`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Bounds, Context, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Render, StatefulInteractiveElement, Styled, WeakEntity,
    Window, canvas, div, img, px,
};

use crate::icons::{Icon, icon_of_pixels};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{NativeSurface, ScreenRect, Tray, TrayButton, TrayIcon};
use crate::popup::PopupAnchor;
use crate::theme::Theme;
use crate::tip::Tips;
use crate::ui::{icon, state_layer};

/// How often the icons are checked for changes.
const REFRESH_INTERVAL: Duration = Duration::from_millis(250);

/// The side of an icon, as a share of the height of a pill.
const ICON_SHARE: f32 = 0.5;

/// On the button that shows and hides the icons over the limit.
const ICON_MORE: &str = "\u{e712}";

/// How much a pressed icon shrinks, as a share of its size.
const PRESS_SHRINK: f32 = 0.16;

/// The longest description shown, in characters.
const TIP_LENGTH: usize = 64;

/// What the tips of the icons are told apart from other tips by.
const TIP_KIND: &str = "tray";

pub struct TrayModule;

impl Module for TrayModule {
    fn name(&self) -> &'static str {
        "tray"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("tray", |context, cx| {
            // Earlier the option named the side the description unrolled
            // at; a layout that still does wants descriptions.
            let named_side = context.options.get("tips").is_some_and(toml::Value::is_str);
            let tips = named_side || context.flag("tips").unwrap_or(true);
            let limit = context.number("limit").map(|limit| limit.max(0.) as usize);
            let bluetooth = context.flag("bluetooth").unwrap_or(false);
            cx.new(|cx| TrayStrip::new(tips, limit, bluetooth, cx))
                .into()
        })
    }
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
    /// Where the icon was drawn in its surface, once it has been.
    drawn: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// Shows the notification area and passes clicks on to the applications.
struct TrayStrip {
    /// In the order shown, including icons that are still leaving.
    items: Vec<TrayItem>,
    /// The state of the notification area the items were taken from.
    generation: Option<u64>,
    /// Whether the description of the icon under the pointer is shown.
    tips: bool,
    /// How many icons are shown at all times; the others only while the
    /// strip is unrolled. Without it every icon is.
    limit: Option<usize>,
    /// Whether the Bluetooth icon of Windows is shown among the others.
    bluetooth: bool,
    /// 0 while only the icons within the limit are shown, 1 when all are.
    unrolled: Spring,
    /// 0 while no icon is over the limit, 1 while the button for them is
    /// shown.
    crowded: Spring,
    more: Interaction,
}

impl TrayStrip {
    fn new(tips: bool, limit: Option<usize>, bluetooth: bool, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
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
            Err(error) => report!("the notification area stays empty: {error}"),
        }
        Self {
            items: Vec::new(),
            generation: None,
            tips,
            limit,
            bluetooth,
            unrolled: Spring::new(motion.spatial, 0.),
            crowded: Spring::new(motion.spatial, 0.),
            more: Interaction::new(&motion),
        }
    }

    /// Takes over the icons if they changed: known ones keep their place,
    /// new ones are appended, removed ones start leaving.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.generation == Some(Tray::generation()) {
            return;
        }
        let (generation, mut icons) = Tray::icons();
        icons.retain(|icon| self.bluetooth || !icon.is_of_bluetooth());
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
                drawn: Rc::default(),
            });
        }
        cx.notify();
    }

    /// The pointer came onto the icon `id`, or left it: its description
    /// comes out of the bar under it after a moment, or goes. The
    /// application hears of it, and one that draws its own description
    /// shows that instead.
    fn tip(&self, id: u64, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        let key = (TIP_KIND, id);
        let item = self.items.iter().find(|item| item.icon.id == id);
        let target = item
            .filter(|item| hovered && !item.leaving)
            .and_then(|item| Some((item, tip_line(&item.icon.tip), item.drawn.get()?)));
        let Some((item, tip, drawn)) = target else {
            Tips::leave(key, cx);
            Tray::hover(id, false);
            return;
        };
        // The strip of every display shows the icon; the application is
        // told the place in the one the pointer is on.
        if let Some(place) = SurfacePlace::of_window(window) {
            let (frame, menu) = place.of(drawn);
            Tray::locate(id, frame, menu);
        }
        Tray::hover(id, true);
        if item.icon.draws_own_tip {
            return;
        }
        let middle = drawn.origin.x + drawn.size.width / 2.;
        match PopupAnchor::at(window, middle, cx) {
            Ok(anchor) if self.tips => Tips::request(key, tip.into(), anchor, cx),
            Ok(_) => {}
            Err(error) => report!("a description has nowhere to open: {error:#}"),
        }
    }

    fn render_item(
        item: &mut TrayItem,
        shown: f32,
        place: Option<SurfacePlace>,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let presence = item.presence.value(window) * shown;
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
            .bg(state_layer(
                &theme,
                theme.on_bar(theme.colors.surface_container),
                state,
            ))
            .on_click(move |event, _, cx| {
                Tips::close(cx);
                let button = match event.click_count() {
                    2 => TrayButton::Double,
                    _ => TrayButton::Left,
                };
                Tray::press(id, button);
            })
            .on_mouse_up(MouseButton::Right, move |_, _, cx| {
                Tips::close(cx);
                Tray::press(id, TrayButton::Right);
            })
            .child({
                let drawn = item.drawn.clone();
                let measure = move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
                    if drawn.replace(Some(bounds)) != Some(bounds)
                        && let Some(place) = place
                    {
                        let (frame, menu) = place.of(bounds);
                        Tray::locate(id, frame, menu);
                    }
                };
                canvas(measure, |_, (), _, _| {}).absolute().size_full()
            })
            .child(match item.image.clone() {
                Some(image) => img(image).size(icon_side).into_any_element(),
                None => div()
                    .size(icon_side)
                    .rounded(theme.shape.small)
                    .bg(theme.colors.outline)
                    .into_any_element(),
            });
        let button = Interaction::track_hover(
            button,
            cx,
            move |this: &mut Self| {
                let item = this.items.iter_mut().find(|item| item.icon.id == id)?;
                Some(&mut item.interaction)
            },
            move |this, hovered, window, cx| this.tip(id, hovered, window, cx),
        );

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

    /// The button that unrolls the strip to the icons over the limit and
    /// rolls it up again, as wide as there are such icons.
    fn render_more(&mut self, theme: Theme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let crowded = self.crowded.value(window);
        let unrolled = self.unrolled.value(window).clamp(0., 1.);
        let state = self.more.state(window);
        let colors = theme.colors;
        let side = theme.bar.pill_height * ICON_SHARE + theme.spacing.small;
        let ground = mix(
            theme.on_bar(colors.surface_container),
            colors.secondary_container,
            unrolled,
        );
        let button = div()
            .id("tray-more")
            .size(side)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(&theme, ground, state))
            .text_color(mix(colors.on_surface_variant, colors.on_surface, unrolled))
            .on_click(cx.listener(|this, _, _, cx| {
                let unrolled = this.unrolled.target() > 0.5;
                this.unrolled.animate_to(if unrolled { 0. } else { 1. });
                cx.notify();
            }))
            .child(icon(&theme, ICON_MORE));
        let button = Interaction::track(button, cx, |this: &mut Self| Some(&mut this.more));
        div()
            .w((side + theme.spacing.tiny) * crowded.max(0.))
            .h(side)
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .opacity(crowded.clamp(0., 1.))
            .child(button)
            .into_any_element()
    }
}

/// Where the surface of a strip is on the screen, to tell where its icons
/// are there.
#[derive(Clone, Copy)]
struct SurfacePlace {
    surface: ScreenRect,
    /// Whether the surface is in the upper half of its display.
    at_top: bool,
    /// Physical pixels per logical one.
    scale: f32,
}

impl SurfacePlace {
    fn of_window(window: &Window) -> Option<Self> {
        let native = NativeSurface::of(window).ok()?;
        let (surface, display) = (native.frame().ok()?, native.display_frame().ok()?);
        Some(Self {
            surface,
            at_top: surface.top + surface.bottom < display.top + display.bottom,
            scale: window.scale_factor(),
        })
    }

    /// The frame on the screen of what was drawn at `bounds` in the
    /// surface, and the point where a menu of it belongs: under its
    /// middle, at the edge of the surface that faces the display.
    fn of(self, bounds: Bounds<Pixels>) -> (ScreenRect, (i32, i32)) {
        let physical = |length: Pixels| (length / px(1.) * self.scale).round() as i32;
        let frame = ScreenRect {
            left: self.surface.left + physical(bounds.origin.x),
            top: self.surface.top + physical(bounds.origin.y),
            right: self.surface.left + physical(bounds.origin.x + bounds.size.width),
            bottom: self.surface.top + physical(bounds.origin.y + bounds.size.height),
        };
        let below = if self.at_top {
            self.surface.bottom
        } else {
            self.surface.top
        };
        (frame, (i32::midpoint(frame.left, frame.right), below))
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
        let limit = self.limit.unwrap_or(usize::MAX);
        let staying = self.items.iter().filter(|item| !item.leaving).count();
        self.crowded
            .animate_to(if staying > limit { 1. } else { 0. });
        let unrolled = self.unrolled.value(window).max(0.);
        let place = SurfacePlace::of_window(window);
        let mut elements: Vec<AnyElement> = self
            .items
            .iter_mut()
            .enumerate()
            .map(|(index, item)| {
                let shown = if index < limit { 1. } else { unrolled };
                Self::render_item(item, shown, place, theme, window, cx)
            })
            .collect();
        if self.limit.is_some() && !elements.is_empty() {
            elements.push(self.render_more(theme, window, cx));
        }
        // An icon that has finished leaving was just drawn at no width.
        self.items
            .retain(|item| !(item.leaving && item.presence.is_settled()));

        if elements.is_empty() {
            return div();
        }
        div()
            .h(theme.bar.pill_height)
            .px(theme.spacing.small - theme.spacing.tiny / 2.)
            .flex()
            .items_center()
            .rounded(theme.shape.full)
            .bg(theme.on_bar(theme.colors.surface_container))
            .children(elements)
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
        assert!(shown.ends_with("wor…"), "got {shown:?}");
    }
}
