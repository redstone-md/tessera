// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How bright the display of a surface is. Widgets: `brightness`, a
//! compact readout, and `brightness-slider`, the control to change it.
//! Action: `brightness`, which makes the display under the pointer a step
//! brighter or darker.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Context, Entity, FontWeight, Global, IntoElement, ParentElement,
    Render, Styled, Subscription, WeakEntity, Window, div,
};

use crate::actions::{Action, Actions};
use crate::flyout::{Flyouts, Reading};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::platform::{BrightnessLevel, DisplayKey, ScreenBrightness};
use crate::theme::Theme;
use crate::ui::{Slider, icon, pill};

const ICON_BRIGHTNESS: &str = "\u{e706}";

/// How often the built-in panel of a laptop is asked while a widget shows
/// it: its keys change it without a word.
const PANEL_INTERVAL: Duration = Duration::from_secs(1);

/// How often a monitor is asked, over DDC/CI, which is slow and which many
/// monitors take badly when asked often.
const MONITOR_INTERVAL: Duration = Duration::from_secs(5);

/// How much one step of the action changes the brightness, as the keys of
/// a laptop do.
const STEP: f32 = 0.1;

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

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let brightness =
            Action::new(|arg, cx| step(arg == Some("up"), cx)).checking(|arg| match arg {
                Some("up" | "down") => Ok(()),
                _ => Err("`brightness` takes `up` or `down`".to_owned()),
            });
        actions.register("brightness", brightness)
    }
}

fn percent(share: f32) -> String {
    format!("{:.0}%", share * 100.)
}

/// The share one step from `share`, on the steps of a tenth.
fn stepped(share: f32, up: bool) -> f32 {
    let steps = (share / STEP).round() + if up { 1. } else { -1. };
    (steps * STEP).clamp(0., 1.)
}

/// Makes the display under the pointer a step brighter or darker, and
/// shows what it is set to. The step goes through the brightness the
/// widgets of that display share, so it waits for the display to be done
/// with what it was asked before, and steps in quick succession add up.
fn step(up: bool, cx: &mut App) {
    let brightness = Brightness::shared(DisplayKey::under_pointer(), cx);
    brightness.update(cx, |brightness, cx| brightness.step(up, cx));
}

/// The brightness of one display, shared by the widgets on it.
struct Brightness {
    display: Option<DisplayKey>,
    /// From 0 to 1; `None` while the display has not said, or does not.
    share: Option<f32>,
    /// Whether the display is asked over DDC/CI, and so seldom: a display
    /// that does not say is asked as seldom.
    slow: bool,
    /// Whether the display was the built-in panel when last asked, which
    /// is then asked first.
    panel: bool,
    /// The steps of the action yet to be made, up for each above 0; they
    /// wait for the display to say how bright it is.
    steps: i32,
    /// The brightness itself, while the steps of the action are made: no
    /// widget may keep it.
    held: Option<Entity<Self>>,
    /// What the display is to be set to once it is done with the last.
    wanted: Option<f32>,
    /// Whether the display is being asked or set now, which takes a part
    /// of a second each time.
    busy: bool,
}

#[derive(Default)]
struct SharedBrightness(HashMap<Option<DisplayKey>, WeakEntity<Brightness>>);

impl Global for SharedBrightness {}

impl Brightness {
    /// The one instance of `display`, which lives for as long as a widget
    /// keeps it. The display is asked whenever a widget is built, as it is
    /// when a popup opens, and then every second (every five for a
    /// monitor), so a change made with its own buttons or keys shows.
    /// No display is the primary one, which is shared with the widgets
    /// that name it.
    fn shared(display: Option<DisplayKey>, cx: &mut App) -> Entity<Self> {
        let display = display.or_else(DisplayKey::primary);
        let known = cx.default_global::<SharedBrightness>().0.get(&display);
        let brightness = match known.and_then(WeakEntity::upgrade) {
            Some(brightness) => brightness,
            None => {
                let brightness = cx.new(|cx: &mut Context<Self>| {
                    Self::watch(cx);
                    Self {
                        display,
                        share: None,
                        slow: true,
                        panel: false,
                        steps: 0,
                        held: None,
                        wanted: None,
                        busy: false,
                    }
                });
                let shared = cx.global_mut::<SharedBrightness>();
                shared.0.retain(|_, known| known.upgrade().is_some());
                shared.0.insert(display, brightness.downgrade());
                brightness
            }
        };
        brightness.update(cx, Self::ask);
        brightness
    }

    /// Asks the display again and again, until the last widget is gone.
    fn watch(cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let Ok(slow) = this.read_with(cx, |this, _| this.slow) else {
                    break;
                };
                let interval = if slow {
                    MONITOR_INTERVAL
                } else {
                    PANEL_INTERVAL
                };
                cx.background_executor().timer(interval).await;
                if this.update(cx, Self::ask).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Asks the display how bright it is, unless it is busy.
    fn ask(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let (display, panel) = (self.display, self.panel);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx
                .background_executor()
                .spawn(async move { ScreenBrightness::read(display, panel) });
            let level = read.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                if this.wanted.is_some() {
                    this.send(cx);
                } else {
                    this.take(level, cx);
                    this.make_steps(cx);
                }
                this.settle();
            })
            .ok();
        })
        .detach();
    }

    fn take(&mut self, level: Option<BrightnessLevel>, cx: &mut Context<Self>) {
        // A display that does not say is not asked every second either:
        // it may be a monitor with DDC/CI switched off.
        self.slow = level.is_none_or(|level| level.slow);
        self.panel = level.is_some_and(|level| !level.slow);
        let share = level.map(|level| level.share);
        if self.share != share {
            self.share = share;
            cx.notify();
        }
    }

    /// Has the display made a step brighter or darker, as soon as it is
    /// known how bright it is, and keeps this until it is set.
    fn step(&mut self, up: bool, cx: &mut Context<Self>) {
        self.steps += if up { 1 } else { -1 };
        self.held = Some(cx.entity());
        if !self.busy || self.share.is_some() {
            self.make_steps(cx);
        }
        self.settle();
    }

    /// Makes the steps of the action from the brightness last set or
    /// read, and shows what came of them in the flyout.
    fn make_steps(&mut self, cx: &mut Context<Self>) {
        if self.steps == 0 {
            return;
        }
        let Some(mut share) = self.share else {
            report!("the display does not let its brightness be set");
            self.steps = 0;
            return;
        };
        for _ in 0..self.steps.unsigned_abs() {
            share = stepped(share, self.steps > 0);
        }
        self.steps = 0;
        self.set(share, cx);
        Flyouts::show(Reading::Brightness(share), cx);
    }

    /// Lets go of itself once the steps of the action are made.
    fn settle(&mut self) {
        if !self.busy && self.wanted.is_none() && self.steps == 0 {
            self.held = None;
        }
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
                this.settle();
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

    #[test]
    fn a_step_goes_to_the_next_tenth() {
        let near = |one: f32, other: f32| (one - other).abs() < 1e-6;
        assert!(near(stepped(0.5, true), 0.6));
        assert!(near(stepped(0.5, false), 0.4));
        // From between two steps to the next one beyond the nearest.
        assert!(near(stepped(0.43, true), 0.5));
        assert!(near(stepped(0.97, true), 1.));
        assert!(near(stepped(0.02, false), 0.));
    }
}
