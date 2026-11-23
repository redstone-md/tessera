// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How bright the display of a surface is. Widgets: `brightness`, a
//! compact readout, and `brightness-slider`, the control to change it.

use std::collections::HashMap;

use gpui::{
    App, AppContext, AsyncApp, Context, Entity, FontWeight, Global, IntoElement, ParentElement,
    Render, Styled, Subscription, WeakEntity, Window, div,
};

use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::platform::{DisplayKey, ScreenBrightness};
use crate::theme::Theme;
use crate::ui::{Slider, icon, pill};

const ICON_BRIGHTNESS: &str = "\u{e706}";

pub struct BrightnessModule;

impl Module for BrightnessModule {
    fn name(&self) -> &'static str {
        "brightness"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("brightness", |context, cx| {
            let brightness = Brightness::shared(context.display, cx);
            cx.new(|cx| BrightnessReadout {
                _changes: cx.observe(&brightness, |_, _, cx| cx.notify()),
                brightness,
            })
            .into()
        })?;
        widgets.register_panel("brightness-slider", |context, cx| {
            let brightness = Brightness::shared(context.display, cx);
            cx.new(|cx| BrightnessSlider::new(brightness, cx)).into()
        })
    }
}

fn percent(share: f32) -> String {
    format!("{:.0}%", share * 100.)
}

/// The brightness of one display, shared by the widgets on it.
struct Brightness {
    display: Option<DisplayKey>,
    /// From 0 to 1; `None` while the display has not said, or does not.
    share: Option<f32>,
    /// What the display is to be set to once it is done with the last.
    wanted: Option<f32>,
    /// Whether the display is being asked or set now, which takes a part
    /// of a second each time.
    busy: bool,
}

#[derive(Default)]
struct SharedBrightness(HashMap<Option<DisplayKey>, Entity<Brightness>>);

impl Global for SharedBrightness {}

impl Brightness {
    /// The one instance of `display`. The display is asked whenever a
    /// widget is built, as it is when a popup opens, and not in between:
    /// what its own buttons change is seen then.
    fn shared(display: Option<DisplayKey>, cx: &mut App) -> Entity<Self> {
        let known = cx.default_global::<SharedBrightness>().0.get(&display);
        let brightness = match known.cloned() {
            Some(brightness) => brightness,
            None => {
                let brightness = cx.new(|_| Self {
                    display,
                    share: None,
                    wanted: None,
                    busy: false,
                });
                let shared = cx.global_mut::<SharedBrightness>();
                shared.0.insert(display, brightness.clone());
                brightness
            }
        };
        brightness.update(cx, Self::ask);
        brightness
    }

    /// Asks the display how bright it is, unless it is busy.
    fn ask(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let display = self.display;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx
                .background_executor()
                .spawn(async move { ScreenBrightness::read(display) });
            let share = read.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                if this.wanted.is_some() {
                    this.send(cx);
                } else if this.share != share {
                    this.share = share;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Shows `share` at once and sets the display to it when it is free.
    fn set(&mut self, share: f32, cx: &mut Context<Self>) {
        let share = share.clamp(0., 1.);
        self.share = Some(share);
        self.wanted = Some(share);
        cx.notify();
        self.send(cx);
    }

    /// Sets the display to the last share that was wanted, and again if
    /// another one was wanted meanwhile.
    fn send(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(share) = self.wanted.take() else {
            return;
        };
        self.busy = true;
        let display = self.display;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let set = cx
                .background_executor()
                .spawn(async move { ScreenBrightness::set(display, share) });
            let set = set.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match set {
                    Ok(()) => this.send(cx),
                    Err(error) => {
                        report!("{error:#}");
                        this.wanted = None;
                        this.ask(cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }
}

/// Shows the brightness as an icon and a percentage, and nothing for a
/// display that does not say.
struct BrightnessReadout {
    brightness: Entity<Brightness>,
    _changes: Subscription,
}

impl Render for BrightnessReadout {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        match self.brightness.read(cx).share {
            Some(share) => pill(&theme)
                .font_weight(FontWeight::MEDIUM)
                .child(icon(&theme, ICON_BRIGHTNESS))
                .child(percent(share))
                .into_any_element(),
            None => div().into_any_element(),
        }
    }
}

/// An icon, a track to drag or scroll over, and the percentage.
struct BrightnessSlider {
    brightness: Entity<Brightness>,
    slider: Slider,
    _changes: Subscription,
}

impl BrightnessSlider {
    fn new(brightness: Entity<Brightness>, cx: &mut Context<Self>) -> Self {
        let theme = *cx.global::<Theme>();
        let share = brightness.read(cx).share.unwrap_or(0.);
        Self {
            _changes: cx.observe(&brightness, |_, _, cx| cx.notify()),
            brightness,
            slider: Slider::new(&theme, share),
        }
    }
}

impl Render for BrightnessSlider {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let slider = theme.slider;
        let Some(share) = self.brightness.read(cx).share else {
            let text = "This display does not let its brightness be set";
            return div()
                .min_h(slider.handle_height)
                .flex()
                .items_center()
                .text_color(theme.colors.on_surface_variant)
                .child(Language::of(cx).say(text))
                .into_any_element();
        };
        let track = self.slider.track(
            "brightness-track",
            share,
            window,
            cx,
            |this: &mut Self| &mut this.slider,
            |this, share, cx| {
                this.brightness
                    .update(cx, |brightness, cx| brightness.set(share, cx));
            },
        );
        div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(
                div()
                    .size(slider.handle_height)
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(&theme, ICON_BRIGHTNESS)),
            )
            .child(track)
            .child(
                div()
                    .flex_none()
                    .w(slider.value_width)
                    .flex()
                    .justify_end()
                    .font_weight(FontWeight::MEDIUM)
                    .child(percent(share)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_share_is_shown_as_a_percentage() {
        assert_eq!(percent(0.426), "43%");
        assert_eq!(percent(1.), "100%");
    }
}
